//! Altium Designer boards (`.PcbDoc`, the binary format Altium Designer
//! has used since version 6).
//!
//! A PcbDoc is an OLE compound file with one storage per kind of object,
//! each holding a `Header` stream (the record count) and a `Data` stream.
//! Board settings, nets and components are text records (`|KEY=VALUE|`);
//! tracks, arcs, vias and pads are binary records. The binary layouts follow
//! KiCad's Altium importer; fields Trace has no use for keep the values a
//! board saved by Altium Designer carries.
//!
//! Copper pours are written as solid copper regions, so they look and
//! connect exactly as imported. Pads and vias whose copper changes from
//! layer to layer use Altium's full-stack mode with a size per layer.

use std::io::{Cursor, Write};
use trace_core::footprint::{Footprint, Library};
use trace_core::{Arc, Board, LayerKind, LayerRef, Length, Pad, PadShape, Point, Side};

/// The board as the bytes of a `.PcbDoc` file. `name` is the file name
/// Altium shows for it.
pub fn to_altium(board: &Board, name: &str) -> std::io::Result<Vec<u8>> {
    Writer::new(board).run(name)
}

/// The same, keeping coordinates as they are: for boards that came from
/// Altium (an ASCII PcbDoc), whose coordinates are already Altium's.
pub fn to_altium_in_place(board: &Board, name: &str) -> std::io::Result<Vec<u8>> {
    let mut w = Writer::new(board);
    w.shift = (0, 0);
    w.run(name)
}

/// A footprint library as the bytes of a `.PcbLib` file: one storage per
/// footprint, whose `Data` stream holds its name and then its primitives,
/// each as in a board's streams. `name` is the file name Altium shows.
pub fn to_pcblib(lib: &Library, name: &str) -> std::io::Result<Vec<u8>> {
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
    let mut head = Vec::new();
    let pascal = |out: &mut Vec<u8>, s: &str| {
        let b: Vec<u8> = s
            .chars()
            .take(255)
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        out.push(b.len() as u8);
        out.extend_from_slice(&b);
    };
    // "PCB 6.0 Binary Library File", its version and an id.
    let title = "PCB 6.0 Binary Library File";
    head.extend_from_slice(&(title.len() as u32).to_le_bytes());
    pascal(&mut head, title);
    head.extend_from_slice(&5.01f64.to_le_bytes());
    head.extend_from_slice(&8u32.to_le_bytes());
    let empty = Board::default();
    let mut ids = Writer::new(&empty);
    let id = ids.unique_id();
    pascal(&mut head, &id);
    cf.create_stream("/FileHeader")?.write_all(&head)?;

    // Storage names: at most 31 characters, none of / \ : !, each once.
    let mut used = std::collections::HashSet::new();
    let storages: Vec<String> = lib
        .footprints
        .iter()
        .map(|f| {
            let base: String = f
                .name
                .chars()
                .map(|c| {
                    if "/\\:!".contains(c) || (c as u32) < 32 {
                        '_'
                    } else {
                        c
                    }
                })
                .take(31)
                .collect();
            let base = if base.is_empty() {
                "Footprint".into()
            } else {
                base
            };
            let mut s = base.clone();
            let mut k = 1;
            while !used.insert(s.to_ascii_uppercase()) {
                let tag = format!("~{k}");
                s = format!(
                    "{}{tag}",
                    base.chars().take(31 - tag.len()).collect::<String>()
                );
                k += 1;
            }
            s
        })
        .collect();

    // The library record: its kind, then the footprint names.
    let stack_board = Board {
        stackup: two_layer_stack(),
        ..Default::default()
    };
    let mut props = vec![
        p("FILENAME", name),
        p("KIND", "Protel_Advanced_PCB_Library"),
        p("VERSION", "3.00"),
        p("DATE", "01.01.2026"),
        p("TIME", "00:00:00"),
    ];
    Writer::new(&stack_board).stack(&mut props);
    let mut data = Vec::new();
    text_record(&mut data, &props);
    data.extend_from_slice(&(lib.footprints.len() as u32).to_le_bytes());
    for f in &lib.footprints {
        let mut n = Vec::new();
        pascal(&mut n, &f.name);
        data.extend_from_slice(&(n.len() as u32).to_le_bytes());
        data.extend_from_slice(&n);
    }
    cf.create_storage("/Library")?;
    cf.create_stream("/Library/Header")?
        .write_all(&1u32.to_le_bytes())?;
    cf.create_stream("/Library/Data")?.write_all(&data)?;
    cf.create_stream("/Library/EmbeddedFonts")?
        .write_all(&0u32.to_le_bytes())?;
    for s in ["Models", "ModelsNoEmbed", "Textures"] {
        cf.create_storage(format!("/Library/{s}"))?;
        cf.create_stream(format!("/Library/{s}/Header"))?
            .write_all(&0u32.to_le_bytes())?;
        cf.create_stream(format!("/Library/{s}/Data"))?;
    }
    // A table of contents Altium shows in its library list.
    let mut toc = Vec::new();
    for f in &lib.footprints {
        let mut line = format!(
            "Name={}|Pad Count={}|Height={}|Description={}",
            f.name,
            f.pads.len(),
            trim(f.height.map_or(0.0, |h| h.mil()), 4),
            f.description.replace('|', "/")
        );
        line.push_str("\r\n");
        toc.extend(
            line.chars()
                .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }),
        );
    }
    cf.create_storage("/Library/ComponentParamsTOC")?;
    let mut tocd = (toc.len() as u32).to_le_bytes().to_vec();
    tocd.extend_from_slice(&toc);
    cf.create_stream("/Library/ComponentParamsTOC/Header")?
        .write_all(&1u32.to_le_bytes())?;
    cf.create_stream("/Library/ComponentParamsTOC/Data")?
        .write_all(&tocd)?;

    for (f, st) in lib.footprints.iter().zip(&storages) {
        let (count, prims, wide) = footprint_records(f);
        let mut d = Vec::new();
        let mut n = Vec::new();
        pascal(&mut n, &f.name);
        d.extend_from_slice(&(n.len() as u32).to_le_bytes());
        d.extend_from_slice(&n);
        d.extend_from_slice(&prims);
        cf.create_storage(format!("/{st}"))?;
        cf.create_stream(format!("/{st}/Header"))?
            .write_all(&count.to_le_bytes())?;
        cf.create_stream(format!("/{st}/Data"))?.write_all(&d)?;
        let mut params = Vec::new();
        let mut pv = vec![p("PATTERN", f.name.clone())];
        if let Some(h) = f.height {
            pv.push(p("HEIGHT", mil(h)));
        }
        pv.push(p("DESCRIPTION", f.description.clone()));
        text_record(&mut params, &pv);
        cf.create_stream(format!("/{st}/Parameters"))?
            .write_all(&params)?;
        let mut w = Vec::new();
        let wv: Vec<(String, String)> = wide
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let codes: Vec<String> = t.chars().map(|c| (c as u32).to_string()).collect();
                (format!("ENCODEDTEXT{i}"), codes.join(","))
            })
            .collect();
        if !wv.is_empty() {
            text_record(&mut w, &wv);
        }
        cf.create_stream(format!("/{st}/WideStrings"))?
            .write_all(&w)?;
    }
    cf.flush()?;
    Ok(cf.into_inner().into_inner())
}

/// Top and bottom copper, for a library's layer list.
fn two_layer_stack() -> trace_core::Stackup {
    let layer = |name: &str, kind| trace_core::StackLayer {
        name: name.into(),
        altium_id: None,
        kind,
        thickness: Length::from_mil(if kind == LayerKind::Copper { 1.4 } else { 12.6 }),
        dielectric_constant: (kind == LayerKind::Dielectric).then_some(4.8),
        loss_tangent: None,
        roughness_um: None,
        material: None,
    };
    trace_core::Stackup {
        layers: vec![
            layer("Top Layer", LayerKind::Copper),
            layer("Dielectric 1", LayerKind::Dielectric),
            layer("Bottom Layer", LayerKind::Copper),
        ],
    }
}

/// One footprint's primitive records, how many there are, and the texts
/// they point to in the `WideStrings` stream.
fn footprint_records(f: &Footprint) -> (u32, Vec<u8>, Vec<String>) {
    let mut stackup = two_layer_stack();
    // Footprints with inner layers keep them.
    for k in 2..f.copper.max(2) {
        stackup.layers.insert(
            stackup.layers.len() - 1,
            trace_core::StackLayer {
                name: format!("Mid-Layer {}", k - 1),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            },
        );
    }
    let mut board = Board {
        stackup,
        ..Default::default()
    };
    let g = &mut board.geometry;
    g.pads = f.pads.clone();
    g.tracks = f.tracks.clone();
    g.arcs = f.arcs.clone();
    g.polygons = f.regions.clone();
    for q in &mut g.pads {
        q.net = None;
        q.component = None;
    }
    for t in &mut g.tracks {
        t.net = None;
        t.component = None;
    }
    for a in &mut g.arcs {
        a.net = None;
        a.component = None;
    }
    for r in &mut g.polygons {
        r.net = None;
        r.component = None;
    }
    let mut w = Writer::new(&board);
    w.shift = (0, 0);
    w.copper = f.copper.clamp(2, 32);
    let (a, arcs) = w.arcs6();
    let (n, pads, extra) = w.pads6();
    let (t, tracks) = w.tracks6();
    let (r, regions) = w.regions6(&extra);
    let mut out = Vec::new();
    out.extend(arcs);
    out.extend(pads);
    out.extend(tracks);
    out.extend(regions);
    let mut wide = Vec::new();
    let mut texts = 0;
    for tx in &f.texts {
        let Some(l) = w.layer(&tx.layer) else {
            continue;
        };
        let mut b = TEXT;
        b[0] = l.v6;
        put_i32(&mut b, 13, unit(tx.at.x));
        put_i32(&mut b, 17, unit(tx.at.y));
        put_i32(&mut b, 21, unit(tx.height));
        put_f64(&mut b, 27, tx.rotation_deg);
        b[35] = tx.mirror as u8;
        put_i32(&mut b, 36, unit(tx.width));
        b[40] = tx.text.eq_ignore_ascii_case(".Comment") as u8;
        b[41] = tx.text.eq_ignore_ascii_case(".Designator") as u8;
        put_u32(&mut b, 115, wide.len() as u32);
        put_u32(&mut b, 226, l.v7);
        wide.push(tx.text.clone());
        let mut s = Vec::new();
        let bytes: Vec<u8> = tx
            .text
            .chars()
            .take(255)
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        s.push(bytes.len() as u8);
        s.extend_from_slice(&bytes);
        binary_record(&mut out, 5, &[&b, &s]);
        texts += 1;
    }
    (a + n + t + r + texts, out, wide)
}

/// Template records from a board saved by Altium Designer. Offsets of the
/// fields Trace fills in are named where they are written.
const TRACK: [u8; 49] = [
    0x45, 0x0c, 0x00, 0xff, 0xff, 0xff, 0xff, 0x69, 0x00, 0xff, 0xff, 0xff, 0xff, 0xc0, 0x6b, 0xfe,
    0x01, 0x3c, 0x53, 0xa2, 0x01, 0xc0, 0x20, 0x04, 0x02, 0x3c, 0x53, 0xa2, 0x01, 0xca, 0x99, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x02, 0x01, 0x00, 0x00, 0x00,
    0x00,
];
const ARC: [u8; 60] = [
    0x21, 0x0c, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x5c, 0x68, 0xdb,
    0x00, 0x08, 0xe2, 0xaf, 0x01, 0x60, 0xea, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x76, 0x40, 0xf0, 0x49, 0x02, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00,
];
const VIA: [u8; 300] = [
    0x4a, 0x6c, 0x00, 0x18, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x04, 0xda, 0x70,
    0x01, 0xfa, 0x77, 0x19, 0x01, 0x40, 0x0d, 0x03, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x01, 0x20, 0x00,
    0xa0, 0x86, 0x01, 0x00, 0x04, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x40, 0x0d, 0x03, 0x00, 0xa0, 0x86,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x9c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01,
    0x01, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40,
    0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x0f, 0x00, 0x03, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x40, 0x9c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0x7f, 0xff, 0xff, 0xff, 0x7f, 0x00,
];
/// A stroke-font text from a footprint library saved by Altium Designer.
pub(crate) const TEXT: [u8; 240] = [
    0x3a, 0x0c, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xf2, 0x61, 0xd1,
    0xff, 0xe0, 0x5e, 0xf8, 0xff, 0x50, 0xc3, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x10, 0x27, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x41, 0x00,
    0x72, 0x00, 0x69, 0x00, 0x61, 0x00, 0x6c, 0x00, 0x00, 0x00, 0x56, 0x40, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x04, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x40, 0x0d,
    0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x9c, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbf, 0x4d, 0x5d, 0x00,
    0x40, 0x42, 0x0f, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0xa0, 0x37, 0xa0, 0x00, 0x20, 0x0b, 0x20,
    0x00, 0x40, 0x0d, 0x03, 0x00, 0x40, 0x0d, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01,
    0x00, 0x41, 0x00, 0x72, 0x00, 0x69, 0x00, 0x61, 0x00, 0x6c, 0x00, 0x00, 0x00, 0x53, 0x48, 0x41,
    0x50, 0x45, 0x42, 0x41, 0x53, 0x45, 0x44, 0x3d, 0x46, 0x41, 0x4c, 0x53, 0x45, 0x7c, 0x43, 0x41,
    0x56, 0x49, 0x54, 0x59, 0x48, 0x45, 0x49, 0x47, 0x48, 0x54, 0x3d, 0x30, 0x6d, 0x69, 0x6c, 0x7c,
    0x53, 0x54, 0x41, 0x4e, 0x44, 0x4f, 0x46, 0x46, 0x48, 0x45, 0x49, 0x47, 0x48, 0x54, 0x3d, 0x30,
    0x6d, 0x01, 0x02, 0x00, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x80,
];
const PAD: [u8; 171] = [
    0x01, 0x0c, 0x00, 0xff, 0xff, 0xff, 0xff, 0x6c, 0x00, 0xff, 0xff, 0xff, 0xff, 0xf2, 0x43, 0xe7,
    0x00, 0x56, 0x24, 0xb1, 0x01, 0xe5, 0x01, 0x06, 0x00, 0xe5, 0x01, 0x06, 0x00, 0xe5, 0x01, 0x06,
    0x00, 0xe5, 0x01, 0x06, 0x00, 0xe5, 0x01, 0x06, 0x00, 0xe5, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x66, 0x40, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x04, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x40, 0x0d,
    0x03, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xf2, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xff, 0xff, 0xff, 0x7f, 0xff, 0xff, 0xff, 0x7f, 0x00,
];

/// Altium's internal unit is 1/10,000 mil (2.54 nm).
pub(crate) fn unit(l: Length) -> i32 {
    (l.0 as f64 / 2.54).round() as i32
}

pub(crate) fn mil(l: Length) -> String {
    let v = l.0 as f64 / 25_400.0;
    format!("{}mil", trim(v, 4))
}

pub(crate) fn trim(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Altium writes floating-point properties in Delphi's exponent form.
pub(crate) fn delphi(v: f64) -> String {
    if v == 0.0 {
        return " 0.00000000000000E+0000".into();
    }
    let e = v.abs().log10().floor() as i32;
    let m = v / 10f64.powi(e);
    let sign = if m < 0.0 { "-" } else { " " };
    format!(
        "{sign}{:.14}E{}{:04}",
        m.abs(),
        if e < 0 { '-' } else { '+' },
        e.abs()
    )
}

pub(crate) fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_i32(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_f64(b: &mut [u8], at: usize, v: f64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// The corner radius of a pad whose outline is the rounded rectangle of its
/// own size, as Altium's rounded-rectangle pads are read.
fn round_corner(pad: &Pad) -> Option<Length> {
    let first = *pad.outline.first()?;
    let (s, c) = (-pad.rotation_deg).to_radians().sin_cos();
    let (dx, dy) = (
        (first.x.0 - pad.at.x.0) as f64,
        (first.y.0 - pad.at.y.0) as f64,
    );
    let y = dx * s + dy * c;
    let r = Length((pad.size_y.0 as f64 / 2.0 - y).round() as i64);
    if r.0 <= 0 {
        return None;
    }
    let again =
        trace_core::geometry::round_rect(pad.at, pad.size_x, pad.size_y, pad.rotation_deg, r);
    let near = |a: &Point, b: &Point| (a.x.0 - b.x.0).abs() <= 3 && (a.y.0 - b.y.0).abs() <= 3;
    (again.len() == pad.outline.len() && again.iter().zip(&pad.outline).all(|(a, b)| near(a, b)))
        .then_some(r)
}

/// One text record: little-endian length, then the properties and a NUL.
pub(crate) fn text_record(out: &mut Vec<u8>, props: &[(String, String)]) {
    let mut s = String::from("|");
    for (k, v) in props {
        s.push_str(k);
        s.push('=');
        // `|` separates properties and cannot appear in a value.
        s.push_str(&v.replace('|', "/"));
        s.push('|');
    }
    s.pop();
    let mut bytes: Vec<u8> = s
        .chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect();
    bytes.push(0);
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&bytes);
}

/// One binary record: type byte, then each subrecord with its length.
pub(crate) fn binary_record(out: &mut Vec<u8>, kind: u8, subs: &[&[u8]]) {
    out.push(kind);
    for s in subs {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s);
    }
}

pub(crate) fn p(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// Layer numbers in both of Altium's schemes: the byte used before
/// Altium Designer 19 and the 32-bit id used since.
#[derive(Clone, Copy)]
struct AltLayer {
    v6: u8,
    v7: u32,
    name: &'static str,
}

const TOP_OVERLAY: AltLayer = AltLayer {
    v6: 33,
    v7: 0x0103_0006,
    name: "TOPOVERLAY",
};
const BOTTOM_OVERLAY: AltLayer = AltLayer {
    v6: 34,
    v7: 0x0103_0007,
    name: "BOTTOMOVERLAY",
};
const KEEPOUT: AltLayer = AltLayer {
    v6: 56,
    v7: 0x0103_000D,
    name: "KEEPOUT",
};
const MULTI: AltLayer = AltLayer {
    v6: 74,
    v7: 0x0103_000F,
    name: "MULTILAYER",
};

const MID_NAMES: [&str; 30] = [
    "MID1", "MID2", "MID3", "MID4", "MID5", "MID6", "MID7", "MID8", "MID9", "MID10", "MID11",
    "MID12", "MID13", "MID14", "MID15", "MID16", "MID17", "MID18", "MID19", "MID20", "MID21",
    "MID22", "MID23", "MID24", "MID25", "MID26", "MID27", "MID28", "MID29", "MID30",
];
const MECH_NAMES: [&str; 16] = [
    "MECHANICAL1",
    "MECHANICAL2",
    "MECHANICAL3",
    "MECHANICAL4",
    "MECHANICAL5",
    "MECHANICAL6",
    "MECHANICAL7",
    "MECHANICAL8",
    "MECHANICAL9",
    "MECHANICAL10",
    "MECHANICAL11",
    "MECHANICAL12",
    "MECHANICAL13",
    "MECHANICAL14",
    "MECHANICAL15",
    "MECHANICAL16",
];

pub(crate) struct Writer<'a> {
    b: &'a Board,
    copper: u8,
    /// Added to every coordinate so the board sits inside Altium's
    /// workspace (internal units).
    shift: (i64, i64),
    ids: u64,
    /// Altium layer byte of each copper layer (top first), when writing
    /// into an existing board whose layer numbers are set; empty numbers
    /// them in order.
    copper_ids: Vec<u8>,
}

impl<'a> Writer<'a> {
    fn new(b: &'a Board) -> Self {
        let g = &b.geometry;
        let mut copper = b.stackup.copper_layers().count() as u8;
        let mut see = |l: &LayerRef| {
            if let LayerRef::Copper(n) = l {
                copper = copper.max(n + 1);
            }
        };
        g.tracks.iter().for_each(|t| see(&t.layer));
        g.arcs.iter().for_each(|t| see(&t.layer));
        g.pads.iter().for_each(|t| see(&t.layer));
        g.polygons.iter().for_each(|t| see(&t.layer));
        for v in &g.vias {
            copper = copper.max(v.to_layer.max(v.from_layer) + 1);
        }
        let copper = copper.clamp(2, 32);
        // Bounding box of everything, so the board can be moved to
        // 1000 mil from Altium's origin.
        let mut lo = (i64::MAX, i64::MAX);
        let mut grow = |q: &Point| {
            lo.0 = lo.0.min(q.x.0);
            lo.1 = lo.1.min(q.y.0);
        };
        g.outline.iter().for_each(&mut grow);
        g.tracks.iter().for_each(|t| {
            grow(&t.start);
            grow(&t.end)
        });
        g.pads.iter().for_each(|t| grow(&t.at));
        g.vias.iter().for_each(|t| grow(&t.at));
        g.polygons
            .iter()
            .flat_map(|q| q.outline.iter())
            .for_each(&mut grow);
        if lo.0 == i64::MAX {
            lo = (0, 0);
        }
        let margin = Length::from_mil(1000.0).0;
        let shift = (
            ((margin - lo.0) as f64 / 2.54).round() as i64,
            ((margin - lo.1) as f64 / 2.54).round() as i64,
        );
        Writer {
            b,
            copper,
            shift,
            ids: 0,
            copper_ids: Vec::new(),
        }
    }

    fn x(&self, l: Length) -> i32 {
        (unit(l) as i64 + self.shift.0) as i32
    }
    fn y(&self, l: Length) -> i32 {
        (unit(l) as i64 + self.shift.1) as i32
    }
    fn xmil(&self, l: Length) -> String {
        format!("{}mil", trim(self.x(l) as f64 / 10_000.0, 4))
    }
    fn ymil(&self, l: Length) -> String {
        format!("{}mil", trim(self.y(l) as f64 / 10_000.0, 4))
    }

    /// A writer for records that go into an existing board: coordinates
    /// as they are, copper layers numbered as that board numbers them.
    pub(crate) fn in_place(b: &'a Board, copper_ids: Vec<u8>) -> Self {
        let mut w = Writer::new(b);
        w.shift = (0, 0);
        w.copper = (copper_ids.len() as u8).clamp(2, 32);
        w.copper_ids = copper_ids;
        w
    }

    /// Altium's eight-letter unique ids.
    pub(crate) fn unique_id(&mut self) -> String {
        self.ids += 1;
        let mut v = self.ids.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (0..8)
            .map(|_| {
                let c = (b'A' + (v % 26) as u8) as char;
                v /= 26;
                c
            })
            .collect()
    }

    /// Starts unique ids from `seed`, so they differ from a board's own.
    pub(crate) fn seed(&mut self, seed: u64) {
        self.ids = seed;
    }

    pub(crate) fn guid(&mut self) -> String {
        self.ids += 1;
        let a = self.ids.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let b = a.rotate_left(29) ^ 0xD1B5_4A32_D192_ED03;
        format!(
            "{{{:08X}-{:04X}-4{:03X}-{:04X}-{:012X}}}",
            (a >> 32) as u32,
            (a >> 16) as u16,
            (a & 0xFFF) as u16,
            0x8000 | ((b >> 48) as u16 & 0x3FFF),
            b & 0xFFFF_FFFF_FFFF
        )
    }

    fn copper_layer(&self, n: u8) -> AltLayer {
        if let Some(&id) = self.copper_ids.get(n as usize) {
            return match id {
                1 => AltLayer {
                    v6: 1,
                    v7: 0x0100_0001,
                    name: "TOP",
                },
                32 => AltLayer {
                    v6: 32,
                    v7: 0x0100_FFFF,
                    name: "BOTTOM",
                },
                id => AltLayer {
                    v6: id,
                    v7: 0x0100_0000 + id as u32,
                    name: MID_NAMES[(id as usize).clamp(2, 31) - 2],
                },
            };
        }
        let last = self.copper - 1;
        if n == 0 {
            AltLayer {
                v6: 1,
                v7: 0x0100_0001,
                name: "TOP",
            }
        } else if n >= last {
            AltLayer {
                v6: 32,
                v7: 0x0100_FFFF,
                name: "BOTTOM",
            }
        } else {
            AltLayer {
                v6: n + 1,
                v7: 0x0100_0001 + n as u32,
                name: MID_NAMES[n as usize - 1],
            }
        }
    }

    fn layer(&self, l: &LayerRef) -> Option<AltLayer> {
        Some(match l {
            LayerRef::Copper(n) => self.copper_layer(*n),
            LayerRef::TopSilk => TOP_OVERLAY,
            LayerRef::BottomSilk => BOTTOM_OVERLAY,
            LayerRef::Keepout => KEEPOUT,
            LayerRef::AllCopper => MULTI,
            LayerRef::Mechanical(k) => {
                let k = (*k).clamp(1, 16);
                AltLayer {
                    v6: 56 + k,
                    v7: 0x0102_0000 + k as u32,
                    name: MECH_NAMES[k as usize - 1],
                }
            }
            // Paste and solder mask, by their old numbers.
            LayerRef::Other(id @ 35..=38) => AltLayer {
                v6: *id,
                v7: 0x0103_0008 + (*id - 35) as u32,
                name: ["TOPPASTE", "BOTTOMPASTE", "TOPSOLDER", "BOTTOMSOLDER"][(*id - 35) as usize],
            },
            LayerRef::Other(_) => return None,
        })
    }

    fn net(&self, n: Option<u32>) -> u16 {
        n.filter(|&n| (n as usize) < self.b.nets.len())
            .map(|n| n as u16)
            .unwrap_or(u16::MAX)
    }

    fn run(mut self, name: &str) -> std::io::Result<Vec<u8>> {
        let mut streams: Vec<(&str, u32, Vec<u8>)> = Vec::new();
        let board = self.board6(name);
        streams.push(("Board6", 1, board));
        let (n, nets) = self.nets6();
        streams.push(("Nets6", n, nets));
        let (n, comps) = self.components6();
        streams.push(("Components6", n, comps));
        let (n, tracks) = self.tracks6();
        streams.push(("Tracks6", n, tracks));
        let (n, arcs) = self.arcs6();
        streams.push(("Arcs6", n, arcs));
        let (n, vias) = self.vias6();
        streams.push(("Vias6", n, vias));
        let (n, pads, extra_regions) = self.pads6();
        streams.push(("Pads6", n, pads));
        let (n, regions) = self.regions6(&extra_regions);
        streams.push(("Regions6", n, regions));
        for s in [
            "Classes6",
            "Rules6",
            "Polygons6",
            "Fills6",
            "Texts6",
            "Dimensions6",
            "ShapeBasedRegions6",
            "ComponentBodies6",
            "DifferentialPairs6",
            "WideStrings6",
            "Models",
            "Connections6",
            "FromTos6",
            "Coordinates6",
            "Embeddeds6",
            "EmbeddedBoards6",
        ] {
            streams.push((s, 0, Vec::new()));
        }

        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        // "PCB 6.0 Binary File" header, as Altium writes it.
        let mut header = 19u32.to_le_bytes().to_vec();
        for c in "PCB 5.0 Bi".encode_utf16() {
            header.extend_from_slice(&c.to_le_bytes());
        }
        cf.create_stream("/FileHeader")?.write_all(&header)?;
        let mut six = 19u32.to_le_bytes().to_vec();
        six.push(19);
        six.extend_from_slice(b"PCB 6.0 Binary File");
        six.extend_from_slice(&5.01f64.to_le_bytes());
        let guid = self.guid();
        six.extend_from_slice(&38u32.to_le_bytes());
        six.push(38);
        six.extend_from_slice(guid.as_bytes());
        cf.create_stream("/FileHeaderSix")?.write_all(&six)?;
        for (name, count, data) in streams {
            cf.create_storage(format!("/{name}"))?;
            cf.create_stream(format!("/{name}/Header"))?
                .write_all(&count.to_le_bytes())?;
            cf.create_stream(format!("/{name}/Data"))?
                .write_all(&data)?;
        }
        cf.flush()?;
        Ok(cf.into_inner().into_inner())
    }

    fn board6(&mut self, name: &str) -> Vec<u8> {
        let b = self.b;
        let mut v = vec![
            p("SELECTION", "FALSE"),
            p("LAYER", "UNKNOWN"),
            p("LOCKED", "FALSE"),
            p("POLYGONOUTLINE", "FALSE"),
            p("USERROUTED", "TRUE"),
            p("KEEPOUT", "FALSE"),
            p("UNIONINDEX", "0"),
            p("FILENAME", name),
            p("KIND", "Protel_Advanced_PCB"),
            p("VERSION", "5.01"),
            p("DATE", "01.01.2026"),
            p("TIME", "00:00:00"),
            p("ORIGINX", self.xmil(Length(0))),
            p("ORIGINY", self.ymil(Length(0))),
            p("BIGVISIBLEGRIDSIZE", "10000000.000"),
            p("VISIBLEGRIDSIZE", "500000.000"),
            p("ELECTRICALGRIDRANGE", "8mil"),
            p("ELECTRICALGRIDENABLED", "TRUE"),
            p("SNAPGRIDSIZE", "50000.000000"),
            p("SNAPGRIDSIZEX", "50000.000000"),
            p("SNAPGRIDSIZEY", "50000.000000"),
            p("TRACKGRIDSIZE", "50000.000000"),
            p("VIAGRIDSIZE", "50000.000000"),
            p("COMPONENTGRIDSIZE", "50000.000000"),
            p("COMPONENTGRIDSIZEX", "50000.000000"),
            p("COMPONENTGRIDSIZEY", "50000.000000"),
            p("DOTGRID", "TRUE"),
            p("DISPLAYUNIT", "0"),
            p("DESIGNATORDISPLAYMODE", "0"),
            p("PRIMITIVELOCK", "TRUE"),
            p("POLYGONTYPE", "Polygon"),
            p("POUROVER", "FALSE"),
            p("REMOVEDEAD", "FALSE"),
            p("GRIDSIZE", "10mil"),
            p("TRACKWIDTH", "10mil"),
            p("HATCHSTYLE", "None"),
            p("USEOCTAGONS", "FALSE"),
            p("MINPRIMLENGTH", "3mil"),
        ];
        // Board shape: straight edges between the outline points.
        let mut outline: Vec<Point> = b.geometry.outline.clone();
        if outline.len() < 3 {
            outline = self.bounding_outline();
        }
        if outline.first() != outline.last() {
            outline.push(outline[0]);
        }
        for (i, q) in outline.iter().enumerate() {
            v.push(p(&format!("KIND{i}"), "0"));
            v.push(p(&format!("VX{i}"), self.xmil(q.x)));
            v.push(p(&format!("VY{i}"), self.ymil(q.y)));
            v.push(p(&format!("CX{i}"), "0mil"));
            v.push(p(&format!("CY{i}"), "0mil"));
            v.push(p(&format!("SA{i}"), delphi(0.0)));
            v.push(p(&format!("EA{i}"), delphi(0.0)));
            v.push(p(&format!("R{i}"), "0mil"));
        }
        v.extend([
            p("SHELVED", "FALSE"),
            p("RESTORENET", ""),
            p("REMOVEISLANDSBYAREA", "TRUE"),
            p("REMOVENECKS", "TRUE"),
            p("AREATHRESHOLD", "250000000000.000000"),
            p("ARCRESOLUTION", "0.5mil"),
            p("NECKWIDTHTHRESHOLD", "5mil"),
            p("POUROVERSTYLE", "2"),
            p("NAME", ""),
            p("POURINDEX", "-1"),
            p("IGNOREVIOLATIONS", "FALSE"),
            p("SHEETX", "1000mil"),
            p("SHEETY", "1000mil"),
            p("SHEETWIDTH", "10000mil"),
            p("SHEETHEIGHT", "8000mil"),
            p("SHOWSHEET", "FALSE"),
            p("LOCKSHEET", "TRUE"),
        ]);
        for i in 1..=16 {
            v.push(p(&format!("PLANE{i}NETNAME"), "(No Net)"));
        }
        self.stack(&mut v);
        let mut out = Vec::new();
        text_record(&mut out, &v);
        out
    }

    fn bounding_outline(&self) -> Vec<Point> {
        let g = &self.b.geometry;
        let pts = g
            .tracks
            .iter()
            .flat_map(|t| [t.start, t.end])
            .chain(g.pads.iter().map(|p| p.at))
            .chain(g.vias.iter().map(|v| v.at));
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for q in pts {
            x0 = x0.min(q.x.0);
            y0 = y0.min(q.y.0);
            x1 = x1.max(q.x.0);
            y1 = y1.max(q.y.0);
        }
        if x0 > x1 {
            (x0, y0, x1, y1) = (0, 0, 50_000_000, 50_000_000);
        }
        let m = 1_000_000;
        let pt = |x, y| Point {
            x: Length(x),
            y: Length(y),
        };
        vec![
            pt(x0 - m, y0 - m),
            pt(x1 + m, y0 - m),
            pt(x1 + m, y1 + m),
            pt(x0 - m, y1 + m),
        ]
    }

    /// Layer stack, in the older numbered form and in Altium 19's form.
    fn stack(&mut self, v: &mut Vec<(String, String)>) {
        let b = self.b;
        let n = self.copper;
        let names: Vec<String> = b.stackup.copper_layers().map(|l| l.name.clone()).collect();
        let copper_t: Vec<Length> = b.stackup.copper_layers().map(|l| l.thickness).collect();
        let diel: Vec<(Length, f64, String)> = b
            .stackup
            .layers
            .iter()
            .filter(|l| l.kind == LayerKind::Dielectric)
            .map(|l| {
                (
                    l.thickness,
                    l.dielectric_constant.unwrap_or(4.2),
                    l.material.clone().unwrap_or_else(|| "FR-4".into()),
                )
            })
            .collect();
        let dfs: Vec<Option<f64>> = b
            .stackup
            .layers
            .iter()
            .filter(|l| l.kind == LayerKind::Dielectric)
            .map(|l| l.loss_tangent)
            .collect();
        let name = |i: u8| -> String {
            names
                .get(i as usize)
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    if i == 0 {
                        "Top Layer".into()
                    } else if i + 1 == n {
                        "Bottom Layer".into()
                    } else {
                        format!("Mid-Layer {i}")
                    }
                })
        };
        let cu = |i: u8| {
            copper_t
                .get(i as usize)
                .copied()
                .filter(|t| t.0 > 0)
                .unwrap_or(Length::from_mm(0.035))
        };
        // Dielectric between copper i and i + 1; spread 1.6 mm when unknown.
        let gap = |i: u8| {
            diel.get(i as usize).cloned().unwrap_or((
                Length::from_mm(1.6 / (n - 1) as f64),
                4.2,
                "FR-4".into(),
            ))
        };
        // Older numbered stack: copper layers chained top to bottom.
        let ids: Vec<u8> = (0..n).map(|i| self.copper_layer(i).v6).collect();
        for (i, &id) in ids.iter().enumerate() {
            let prev = if i == 0 { 0 } else { ids[i - 1] };
            let next = ids.get(i + 1).copied().unwrap_or(0);
            let (h, k, m) = gap(i as u8);
            v.extend([
                p(&format!("LAYER{id}NAME"), name(i as u8)),
                p(&format!("LAYER{id}PREV"), prev.to_string()),
                p(&format!("LAYER{id}NEXT"), next.to_string()),
                p(&format!("LAYER{id}MECHENABLED"), "FALSE"),
                p(&format!("LAYER{id}COPTHICK"), mil(cu(i as u8))),
                p(&format!("LAYER{id}DIELTYPE"), "0"),
                p(&format!("LAYER{id}DIELCONST"), format!("{k:.3}")),
                p(&format!("LAYER{id}DIELHEIGHT"), mil(h)),
                p(&format!("LAYER{id}DIELMATERIAL"), m),
            ]);
        }
        v.extend([
            p("LAYER57NAME", "Mechanical 1"),
            p("LAYER57PREV", "0"),
            p("LAYER57NEXT", "0"),
            p("LAYER57MECHENABLED", "TRUE"),
            p("TOPTYPE", "3"),
            p("TOPCONST", "3.500"),
            p("TOPHEIGHT", "0.4mil"),
            p("TOPMATERIAL", "Solder Resist"),
            p("BOTTOMTYPE", "3"),
            p("BOTTOMCONST", "3.500"),
            p("BOTTOMHEIGHT", "0.4mil"),
            p("BOTTOMMATERIAL", "Solder Resist"),
            p("SHOWTOPDIELECTRIC", "FALSE"),
            p("SHOWBOTTOMDIELECTRIC", "FALSE"),
        ]);

        // Altium 19 stack: every physical layer, top to bottom.
        let sub = self.guid();
        v.extend([
            p("V9_SUBSTACK0_ID", sub.clone()),
            p("V9_SUBSTACK0_NAME", "Board Layer Stack"),
            p("V9_SUBSTACK0_SHOWTOPDIELECTRIC", "FALSE"),
            p("V9_SUBSTACK0_SHOWBOTTOMDIELECTRIC", "FALSE"),
            p("V9_SUBSTACK0_ISFLEX", "FALSE"),
            p("V9_SUBSTACK0_SERVICE", "FALSE"),
            p("V9_SUBSTACK0_USEDBYPRIMS", "TRUE"),
            p("V9_SUBSTACK0_TYPE", "1"),
        ]);
        enum L {
            Plain(&'static str, u32),
            Mask(&'static str, u32),
            Copper(u8),
            Diel(u8),
        }
        let mut layers = vec![
            L::Plain("Top Paste", 0x0103_0008),
            L::Plain("Top Overlay", 0x0103_0006),
            L::Mask("Top Solder", 0x0103_000A),
        ];
        for i in 0..n {
            layers.push(L::Copper(i));
            if i + 1 < n {
                layers.push(L::Diel(i));
            }
        }
        layers.extend([
            L::Mask("Bottom Solder", 0x0103_000B),
            L::Plain("Bottom Overlay", 0x0103_0007),
            L::Plain("Bottom Paste", 0x0103_0009),
        ]);
        for (k, l) in layers.iter().enumerate() {
            let key = |f: &str| format!("V9_STACK_LAYER{k}_{f}");
            v.push(p(&key(&format!("{sub}CONTEXT")), "0"));
            v.push(p(&key(&format!("{sub}USEDBYPRIMS")), "FALSE"));
            let id = self.guid();
            v.push(p(&key("ID"), id));
            match l {
                L::Plain(name, lid) => {
                    v.push(p(&key("NAME"), *name));
                    v.push(p(&key("LAYERID"), lid.to_string()));
                    v.push(p(&key("USEDBYPRIMS"), "TRUE"));
                }
                L::Mask(name, lid) => {
                    v.push(p(&key("NAME"), *name));
                    v.push(p(&key("LAYERID"), lid.to_string()));
                    v.push(p(&key("USEDBYPRIMS"), "TRUE"));
                    v.push(p(&key("DIELTYPE"), "3"));
                    v.push(p(&key("DIELCONST"), "3.500"));
                    v.push(p(&key("DIELHEIGHT"), "0.4mil"));
                    v.push(p(&key("DIELMATERIAL"), "Solder Resist"));
                    v.push(p(&key("COVERLAY_EXPANSION"), "0mil"));
                }
                L::Copper(i) => {
                    v.push(p(&key("NAME"), name(*i)));
                    v.push(p(&key("LAYERID"), self.copper_layer(*i).v7.to_string()));
                    v.push(p(&key("USEDBYPRIMS"), "TRUE"));
                    v.push(p(&key("COPTHICK"), mil(cu(*i))));
                    let place = if *i == 0 {
                        "1"
                    } else if *i + 1 == n {
                        "2"
                    } else {
                        "0"
                    };
                    v.push(p(&key("COMPONENTPLACEMENT"), place));
                }
                L::Diel(i) => {
                    let (h, k, m) = gap(*i);
                    v.push(p(&key("NAME"), format!("Dielectric {}", i + 1)));
                    v.push(p(&key("LAYERID"), (0x0104_0001 + *i as u32).to_string()));
                    v.push(p(&key("USEDBYPRIMS"), "FALSE"));
                    v.push(p(&key("DIELTYPE"), if i % 2 == 0 { "1" } else { "2" }));
                    v.push(p(&key("DIELCONST"), format!("{k:.3}")));
                    if let Some(df) = dfs.get(*i as usize).copied().flatten() {
                        v.push(p(&key("DIELLOSSTANGENT"), format!("{df:.4}")));
                    }
                    v.push(p(&key("DIELHEIGHT"), mil(h)));
                    v.push(p(&key("DIELMATERIAL"), m));
                }
            }
        }
    }

    fn nets6(&mut self) -> (u32, Vec<u8>) {
        let mut out = Vec::new();
        for n in &self.b.nets {
            out.extend(self.net_record(n));
        }
        (self.b.nets.len() as u32, out)
    }

    pub(crate) fn net_record(&mut self, n: &trace_core::Net) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let id = self.unique_id();
            let mut v = vec![
                p("SELECTION", "FALSE"),
                p("LAYER", "TOP"),
                p("LOCKED", "FALSE"),
                p("POLYGONOUTLINE", "FALSE"),
                p("USERROUTED", "TRUE"),
                p("KEEPOUT", "FALSE"),
                p("UNIONINDEX", "0"),
                p("PRIMITIVELOCK", "FALSE"),
                p("NAME", n.name.clone()),
                p("VISIBLE", "TRUE"),
                p("COLOR", "65535"),
                p("LOOPREMOVAL", "TRUE"),
                p("OVERRIDECOLORFORDRAW", "FALSE"),
            ];
            v.push(p("UNIQUEID", id));
            text_record(&mut out, &v);
        }
        out
    }

    fn components6(&mut self) -> (u32, Vec<u8>) {
        let mut out = Vec::new();
        for c in &self.b.components {
            let id = self.unique_id();
            out.extend(self.component_record(c, &id));
        }
        (self.b.components.len() as u32, out)
    }

    /// A part's record, with `id` as its unique id.
    pub(crate) fn component_record(&self, c: &trace_core::Component, id: &str) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let pattern = if c.footprint.is_empty() {
                "TRACE_PART".to_string()
            } else {
                c.footprint.clone()
            };
            let v = vec![
                p("SELECTION", "FALSE"),
                p(
                    "LAYER",
                    if c.side == Side::Bottom {
                        "BOTTOM"
                    } else {
                        "TOP"
                    },
                ),
                p("LOCKED", "FALSE"),
                p("POLYGONOUTLINE", "FALSE"),
                p("USERROUTED", "TRUE"),
                p("KEEPOUT", "FALSE"),
                p("PRIMITIVELOCK", "TRUE"),
                p("X", self.xmil(c.x)),
                p("Y", self.ymil(c.y)),
                p("PATTERN", pattern),
                p("NAMEON", "TRUE"),
                p("COMMENTON", "FALSE"),
                p("GROUPNUM", "0"),
                p("COUNT", "0"),
                p("ROTATION", delphi(c.rotation_deg.rem_euclid(360.0))),
                p("HEIGHT", "0mil"),
                p("NAMEAUTOPOSITION", "1"),
                p("COMMENTAUTOPOSITION", "3"),
                p("UNIONINDEX", "0"),
                p("SOURCEDESIGNATOR", c.designator.clone()),
                p(
                    "SOURCELIBREFERENCE",
                    c.library_ref.clone().unwrap_or_default(),
                ),
                p("UNIQUEID", id),
            ];
            text_record(&mut out, &v);
        }
        out
    }

    /// A part index as Altium stores it, or none (0xFFFF).
    fn component(&self, c: Option<u32>) -> u16 {
        c.filter(|&c| (c as usize) < self.b.components.len() && c < u16::MAX as u32)
            .map(|c| c as u16)
            .unwrap_or(u16::MAX)
    }

    fn tracks6(&mut self) -> (u32, Vec<u8>) {
        let mut out = Vec::new();
        let mut n = 0;
        for t in &self.b.geometry.tracks {
            let Some(r) = self.track_record(t) else {
                continue;
            };
            out.extend(r);
            n += 1;
        }
        // Graphic outline lines, when the board has an outline, go on
        // mechanical layer 1 as well, where fabrication drawings expect it.
        let o = &self.b.geometry.outline;
        if o.len() >= 3 {
            let l = self.layer(&LayerRef::Mechanical(1)).unwrap();
            for i in 0..o.len() {
                let (a, c) = (o[i], o[(i + 1) % o.len()]);
                if a == c {
                    continue;
                }
                let mut r = TRACK;
                r[0] = l.v6;
                put_u16(&mut r, 3, u16::MAX);
                put_u16(&mut r, 5, u16::MAX);
                put_u16(&mut r, 7, u16::MAX);
                put_i32(&mut r, 13, self.x(a.x));
                put_i32(&mut r, 17, self.y(a.y));
                put_i32(&mut r, 21, self.x(c.x));
                put_i32(&mut r, 25, self.y(c.y));
                put_i32(&mut r, 29, unit(Length::from_mm(0.1)));
                put_u32(&mut r, 41, l.v7);
                binary_record(&mut out, 4, &[&r]);
                n += 1;
            }
        }
        (n, out)
    }

    pub(crate) fn track_record(&self, t: &trace_core::Track) -> Option<Vec<u8>> {
        let l = self.layer(&t.layer)?;
        let mut r = TRACK;
        r[0] = l.v6;
        put_u16(&mut r, 3, self.net(t.net));
        put_u16(&mut r, 5, u16::MAX);
        put_u16(&mut r, 7, self.component(t.component));
        put_i32(&mut r, 13, self.x(t.start.x));
        put_i32(&mut r, 17, self.y(t.start.y));
        put_i32(&mut r, 21, self.x(t.end.x));
        put_i32(&mut r, 25, self.y(t.end.y));
        put_i32(&mut r, 29, unit(t.width));
        put_u32(&mut r, 41, l.v7);
        let mut out = Vec::new();
        binary_record(&mut out, 4, &[&r]);
        Some(out)
    }

    fn arcs6(&mut self) -> (u32, Vec<u8>) {
        let mut out = Vec::new();
        let mut n = 0;
        for a in &self.b.geometry.arcs {
            let Some(l) = self.layer(&a.layer) else {
                continue;
            };
            out.extend(self.arc_record(a, l));
            n += 1;
        }
        (n, out)
    }

    pub(crate) fn arc_record_of(&self, a: &Arc) -> Option<Vec<u8>> {
        Some(self.arc_record(a, self.layer(&a.layer)?))
    }

    fn arc_record(&self, a: &Arc, l: AltLayer) -> Vec<u8> {
        let mut r = ARC;
        r[0] = l.v6;
        put_u16(&mut r, 3, self.net(a.net));
        put_u16(&mut r, 5, u16::MAX);
        put_u16(&mut r, 7, self.component(a.component));
        put_i32(&mut r, 13, self.x(a.center.x));
        put_i32(&mut r, 17, self.y(a.center.y));
        put_i32(&mut r, 21, unit(a.radius));
        // Altium arcs run counter-clockwise from start to end angle.
        let (s, mut e) = (a.start_deg.rem_euclid(360.0), a.end_deg.rem_euclid(360.0));
        if (a.end_deg - a.start_deg).abs() >= 359.999 {
            e = s + 360.0;
        }
        put_f64(&mut r, 25, s);
        put_f64(&mut r, 33, e);
        put_i32(&mut r, 41, unit(a.width));
        put_u32(&mut r, 52, l.v7);
        let mut out = Vec::new();
        binary_record(&mut out, 1, &[&r]);
        out
    }

    fn vias6(&mut self) -> (u32, Vec<u8>) {
        let mut out = Vec::new();
        for v in &self.b.geometry.vias {
            out.extend(self.via_record(v));
        }
        (self.b.geometry.vias.len() as u32, out)
    }

    pub(crate) fn via_record(&self, v: &trace_core::Via) -> Vec<u8> {
        let mut out = Vec::new();
        let last = self.copper - 1;
        {
            let (a, z) = (
                v.from_layer.min(v.to_layer),
                v.from_layer.max(v.to_layer).min(last),
            );
            let mut r = VIA;
            put_u16(&mut r, 3, self.net(v.net));
            put_u16(&mut r, 5, u16::MAX);
            put_u16(&mut r, 7, u16::MAX);
            put_i32(&mut r, 13, self.x(v.at.x));
            put_i32(&mut r, 17, self.y(v.at.y));
            put_i32(&mut r, 21, unit(v.diameter.max(v.hole)));
            put_i32(&mut r, 25, unit(v.hole));
            r[29] = self.copper_layer(a).v6;
            r[30] = self.copper_layer(z).v6;
            // Per-layer diameters (slot 0 top, 1..30 mid layers, 31 bottom).
            let mut sizes = [unit(v.diameter.max(v.hole)); 32];
            if !v.rings.is_empty() {
                r[74] = 2; // full stack
                for &(l, d) in &v.rings {
                    let slot = (self.copper_layer(l).v6 - 1) as usize;
                    sizes[slot] = unit(d.max(v.hole));
                }
                // The main diameter is the largest ring.
                let big = v.rings.iter().map(|r| r.1).max().unwrap_or(v.diameter);
                put_i32(&mut r, 21, unit(big.max(v.hole)));
            } else {
                r[74] = 0;
            }
            for (k, s) in sizes.iter().enumerate() {
                put_i32(&mut r, 75 + 4 * k, *s);
            }
            // Solder mask expansion of its own: value at 54, manual at 66.
            if let Some(m) = v.fab.mask {
                put_i32(&mut r, 54, unit(m));
                r[66] |= 0x02;
            }
            if v.fab.tent_top {
                r[1] |= 0x20;
            }
            if v.fab.tent_bottom {
                r[1] |= 0x40;
            }
            binary_record(&mut out, 3, &[&r]);
        }
        out
    }

    /// Pads, and the copper regions that carry pads with an exact outline.
    fn pads6(&mut self) -> (u32, Vec<u8>, Vec<Region>) {
        let mut out = Vec::new();
        let mut regions = Vec::new();
        let mut n = 0;
        let comps = self.b.components.len();
        for pad in &self.b.geometry.pads {
            let Some(l) = self.layer(&pad.layer) else {
                continue;
            };
            let comp = pad
                .component
                .filter(|&c| (c as usize) < comps)
                .map(|c| c as u16)
                .unwrap_or(u16::MAX);
            let custom = pad.outline.len() >= 3 && round_corner(pad).is_none();
            if custom {
                regions.push(Region {
                    layer: if l.v6 == MULTI.v6 {
                        self.copper_layer(0)
                    } else {
                        l
                    },
                    net: self.net(pad.net),
                    component: comp,
                    outline: pad.outline.clone(),
                    holes: Vec::new(),
                });
            }
            out.extend(self.pad_record(pad, l, comp, custom));
            n += 1;
        }
        (n, out, regions)
    }

    /// A pad's record, and the copper region that carries its exact
    /// outline when it has one.
    pub(crate) fn pad_record_of(&self, pad: &Pad) -> Option<(Vec<u8>, Option<Region>)> {
        let l = self.layer(&pad.layer)?;
        let comp = self.component(pad.component);
        let custom = pad.outline.len() >= 3 && round_corner(pad).is_none();
        let region = custom.then(|| Region {
            layer: if l.v6 == MULTI.v6 {
                self.copper_layer(0)
            } else {
                l
            },
            net: self.net(pad.net),
            component: comp,
            outline: pad.outline.clone(),
            holes: Vec::new(),
        });
        Some((self.pad_record(pad, l, comp, custom), region))
    }

    /// A copper or other area as a region record.
    pub(crate) fn area_region(&self, q: &trace_core::Polygon) -> Option<Region> {
        let l = self.layer(&q.layer)?;
        (q.outline.len() >= 3).then(|| Region {
            layer: l,
            net: self.net(q.net),
            component: self.component(q.component),
            outline: q.outline.clone(),
            holes: q.holes.iter().filter(|h| h.len() >= 3).cloned().collect(),
        })
    }

    fn pad_record(&self, pad: &Pad, l: AltLayer, comp: u16, custom: bool) -> Vec<u8> {
        let shape = |s: PadShape| match s {
            PadShape::Round => 1u8,
            PadShape::Rect => 2,
            PadShape::Octagon => 3,
        };
        let mut r = PAD;
        r[0] = l.v6;
        put_u16(&mut r, 3, self.net(pad.net));
        put_u16(&mut r, 5, u16::MAX);
        put_u16(&mut r, 7, comp);
        put_i32(&mut r, 13, self.x(pad.at.x));
        put_i32(&mut r, 17, self.y(pad.at.y));
        let (mut w, mut h, mut sh) = (unit(pad.size_x), unit(pad.size_y), shape(pad.shape));
        if custom {
            // The copper is the region; the pad keeps a small round land.
            let d = unit(pad.hole).max(unit(Length::from_mm(0.1)));
            (w, h, sh) = (d, d, 1);
        }
        let hole = unit(pad.hole);
        // A land of no size takes the hole's.
        if hole > 0 && (w == 0 || h == 0) {
            w = w.max(hole);
            h = h.max(hole);
        }
        for at in [21, 29, 37] {
            put_i32(&mut r, at, w);
            put_i32(&mut r, at + 4, h);
        }
        put_i32(&mut r, 45, hole);
        r[49] = sh;
        r[50] = sh;
        r[51] = sh;
        put_f64(
            &mut r,
            52,
            if custom {
                0.0
            } else {
                pad.rotation_deg.rem_euclid(360.0)
            },
        );
        r[60] = u8::from(hole > 0 && pad.name != "NPTH" && !pad.fab.unplated);
        r[62] = 0;
        // Mask and paste of its own, and tenting.
        let fab = &pad.fab;
        if let Some(m) = fab.mask {
            r[102] = 2;
            put_i32(&mut r, 90, unit(m));
        }
        if fab.no_paste {
            r[101] = 2;
            put_i32(&mut r, 86, unit(Length::from_mm(-1000.0)));
        } else if let Some(e) = fab.paste {
            r[101] = 2;
            put_i32(&mut r, 86, unit(e));
        }
        if fab.tent_top {
            r[1] |= 0x20;
        }
        if fab.tent_bottom {
            r[1] |= 0x40;
        }
        let corner = if custom { None } else { round_corner(pad) };
        let slot = hole > 0 && fab.slot.0 > pad.hole.0;
        // Through-hole pads span top to bottom; others sit on their layer.
        let (from, to) = if l.v6 == MULTI.v6 {
            (1, 32)
        } else {
            (l.v6, l.v6)
        };
        r[114] = to;
        r[117] = from;

        let mut stack = Vec::new();
        let full = !pad.stack.is_empty() && l.v6 == MULTI.v6 && !custom;
        if full {
            r[62] = 2; // full stack
            let last = self.copper - 1;
            // Layers without a land keep a ring the size of the hole.
            let land = |n: u8| pad.stack.iter().find(|d| d.layer == n);
            let size = |n: u8| match land(n) {
                Some(d) => (
                    unit(d.size_x).max(hole),
                    unit(d.size_y).max(hole),
                    shape(d.shape),
                ),
                None => (hole.max(1), hole.max(1), 1),
            };
            let (tw, th, ts) = size(0);
            put_i32(&mut r, 21, tw);
            put_i32(&mut r, 25, th);
            r[49] = ts;
            let (bw, bh, bs) = size(last);
            put_i32(&mut r, 37, bw);
            put_i32(&mut r, 41, bh);
            r[51] = bs;
            let (mw, mh, ms) = if last > 1 { size(1) } else { (tw, th, ts) };
            put_i32(&mut r, 29, mw);
            put_i32(&mut r, 33, mh);
            r[50] = ms;
            // Mid layers 2..30: x sizes, y sizes, shapes, then hole data.
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let mut shapes = Vec::new();
            for m in 2..=30u8 {
                let (sx, sy, ss) = if m < last { size(m) } else { (mw, mh, ms) };
                xs.extend_from_slice(&sx.to_le_bytes());
                ys.extend_from_slice(&sy.to_le_bytes());
                shapes.push(ss);
            }
            stack.extend(xs);
            stack.extend(ys);
            stack.extend(shapes);
        } else if corner.is_some() || slot {
            // The same land on every mid layer.
            for _ in 2..=30 {
                stack.extend_from_slice(&w.to_le_bytes());
            }
            for _ in 2..=30 {
                stack.extend_from_slice(&h.to_le_bytes());
            }
            stack.extend(std::iter::repeat_n(sh, 29));
        }
        if !stack.is_empty() {
            stack.push(0); // unknown
            stack.push(if slot { 2 } else { 0 }); // hole shape
            let len = if slot { unit(fab.slot) } else { 0 };
            stack.extend_from_slice(&len.to_le_bytes()); // slot length
            let turn = if slot {
                (fab.slot_deg - pad.rotation_deg).rem_euclid(360.0)
            } else {
                0.0
            };
            stack.extend_from_slice(&turn.to_le_bytes()); // slot rotation
            stack.extend(std::iter::repeat_n(0u8, 32 * 4 * 2)); // hole offsets
            stack.push(0);
            // Rounded rectangles: alternative shape 9 with the corner as a
            // percentage of half the short side.
            let (alt, pct) = match corner {
                Some(c) => {
                    let half = pad.size_x.0.min(pad.size_y.0) as f64 / 2.0;
                    (
                        9u8,
                        (c.0 as f64 / half * 100.0).round().clamp(1.0, 100.0) as u8,
                    )
                }
                None => (0, 0),
            };
            stack.extend(std::iter::repeat_n(alt, 32)); // alternative shapes
            stack.extend(std::iter::repeat_n(pct, 32)); // corner radii
            debug_assert_eq!(stack.len(), 596);
        }

        let name = pad.name.as_bytes();
        let mut sub0 = vec![name.len().min(255) as u8];
        sub0.extend_from_slice(&name[..name.len().min(255)]);
        let sub2 = [4u8, b'|', b'&', b'|', b'0'];
        let mut out = Vec::new();
        binary_record(&mut out, 2, &[&sub0, &[0], &sub2, &[0], &r, &stack]);
        out
    }

    fn regions6(&mut self, extra: &[Region]) -> (u32, Vec<u8>) {
        let mut all: Vec<Region> = Vec::new();
        for q in &self.b.geometry.polygons {
            let Some(l) = self.layer(&q.layer) else {
                continue;
            };
            if q.outline.len() < 3 {
                continue;
            }
            all.push(Region {
                layer: l,
                net: self.net(q.net),
                component: u16::MAX,
                outline: q.outline.clone(),
                holes: q.holes.iter().filter(|h| h.len() >= 3).cloned().collect(),
            });
        }
        // Silkscreen, mask, paste and mechanical areas.
        for q in &self.b.geometry.areas {
            if let Some(r) = self.area_region(q) {
                all.push(r);
            }
        }
        all.extend(extra.iter().cloned());
        let mut out = Vec::new();
        for g in &all {
            out.extend(self.region_record(g, None));
        }
        (all.len() as u32, out)
    }

    /// A region record; `pour` ties it to a polygon pour (by index).
    pub(crate) fn region_record(&self, g: &Region, pour: Option<u16>) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut h = [0u8; 18];
            h[0] = g.layer.v6;
            h[1] = 0x0c;
            put_u16(&mut h, 3, g.net);
            put_u16(&mut h, 5, pour.unwrap_or(u16::MAX));
            put_u16(&mut h, 7, g.component);
            h[9..13].copy_from_slice(&[0xff; 4]);
            put_u16(&mut h, 14, g.holes.len().min(u16::MAX as usize) as u16);
            let mut body = h.to_vec();
            text_record(
                &mut body,
                &[
                    p("V7_LAYER", g.layer.name),
                    p("NAME", " "),
                    p("KIND", "0"),
                    p("SUBPOLYINDEX", "-1"),
                    p("UNIONINDEX", "0"),
                    p("ARCRESOLUTION", "0.5mil"),
                    p("ISSHAPEBASED", "FALSE"),
                    p("CAVITYHEIGHT", "0mil"),
                ],
            );
            let ring = |body: &mut Vec<u8>, pts: &[Point]| {
                let mut pts = pts.to_vec();
                if pts.len() > 1 && pts.first() == pts.last() {
                    pts.pop();
                }
                body.extend_from_slice(&(pts.len() as u32).to_le_bytes());
                for q in pts {
                    body.extend_from_slice(&(self.x(q.x) as f64).to_le_bytes());
                    body.extend_from_slice(&(self.y(q.y) as f64).to_le_bytes());
                }
            };
            ring(&mut body, &g.outline);
            for hole in g.holes.iter().take(u16::MAX as usize) {
                ring(&mut body, hole);
            }
            binary_record(&mut out, 11, &[&body]);
        }
        out
    }

    /// The same region as `ShapeBasedRegions6` keeps it: integer
    /// vertices (straight edges), the outline closed, holes as doubles.
    pub(crate) fn shape_region_record(&self, g: &Region, pour: Option<u16>) -> Vec<u8> {
        let plain = self.region_record(g, pour);
        // Header and properties as in the plain record.
        let plen = u32::from_le_bytes(plain[5 + 18..5 + 22].try_into().unwrap()) as usize;
        let mut body = plain[5..5 + 22 + plen].to_vec();
        let mut pts = g.outline.clone();
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        body.extend_from_slice(&(pts.len() as u32).to_le_bytes());
        for q in pts.iter().chain(pts.first()) {
            body.push(0);
            body.extend_from_slice(&self.x(q.x).to_le_bytes());
            body.extend_from_slice(&self.y(q.y).to_le_bytes());
            body.extend_from_slice(&[0u8; 12]);
            body.extend_from_slice(&[0u8; 16]);
        }
        for hole in g.holes.iter().take(u16::MAX as usize) {
            let mut h = hole.clone();
            if h.len() > 1 && h.first() == h.last() {
                h.pop();
            }
            body.extend_from_slice(&(h.len() as u32).to_le_bytes());
            for q in h {
                body.extend_from_slice(&(self.x(q.x) as f64).to_le_bytes());
                body.extend_from_slice(&(self.y(q.y) as f64).to_le_bytes());
            }
        }
        let mut out = Vec::new();
        binary_record(&mut out, 11, &[&body]);
        out
    }
}

#[derive(Clone)]
pub(crate) struct Region {
    layer: AltLayer,
    net: u16,
    component: u16,
    outline: Vec<Point>,
    holes: Vec<Vec<Point>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    fn pt(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
        }
    }

    #[test]
    fn board_reads_back() {
        let mut b = Board::default();
        b.nets.push(Net {
            name: "GND".into(),
            ..Default::default()
        });
        let g = &mut b.geometry;
        g.outline = vec![pt(0.0, 0.0), pt(20.0, 0.0), pt(20.0, 10.0), pt(0.0, 10.0)];
        g.tracks.push(Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: pt(1.0, 1.0),
            end: pt(5.0, 1.0),
            width: Length::from_mm(0.2),
            text: false,
        });
        g.vias.push(Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: pt(5.0, 1.0),
            diameter: Length::from_mm(0.6),
            hole: Length::from_mm(0.3),
            rings: Vec::new(),
            fab: Default::default(),
        });
        let bytes = to_altium(&b, "t.PcbDoc").unwrap();
        let dir = std::env::temp_dir().join(format!("trace-altium-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.PcbDoc");
        std::fs::write(&path, bytes).unwrap();
        let back = trace_altium::read_pcbdoc(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.nets[0].name, "GND");
        assert_eq!(back.stackup.copper_layers().count(), 2);
        let (t, v) = (&back.geometry.tracks[0], &back.geometry.vias[0]);
        assert_eq!(t.layer, LayerRef::Copper(0));
        assert_eq!((t.net, v.net), (Some(0), Some(0)));
        let len = (t.end.x.0 - t.start.x.0) as f64 / 1e6;
        assert!((len - 4.0).abs() < 1e-5, "{len}");
        assert!((v.diameter.mm() - 0.6).abs() < 1e-5);
        // The closing point is not repeated.
        assert_eq!(back.geometry.outline.len(), 4);
    }

    #[test]
    fn footprint_library_reads_back() {
        use trace_core::footprint::{FpText, Library};
        // Lengths Altium holds exactly (0.1 mil steps).
        let mm = |v: f64| Length::from_mil((v / 0.0254 * 10.0).round() / 10.0);
        let ptm = |x: f64, y: f64| Point { x: mm(x), y: mm(y) };
        let pad = |name: &str, x: f64, layer: LayerRef| Pad {
            layer,
            net: None,
            component: None,
            name: name.into(),
            at: ptm(x, 0.0),
            size_x: mm(0.6),
            size_y: mm(1.2),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 90.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        let mut rounded = pad("2", 2.0, LayerRef::Copper(0));
        rounded.shape = PadShape::Round;
        rounded.outline = trace_core::geometry::round_rect(
            rounded.at,
            rounded.size_x,
            rounded.size_y,
            90.0,
            mm(0.15),
        );
        rounded.fab.mask = Some(mm(0.05));
        rounded.fab.no_paste = true;
        // -1 m, as Altium marks "no paste", to its 0.0001 mil steps.
        rounded.fab.paste = Some(Length(-999_999_999));
        let mut slot = pad("3", 5.0, LayerRef::AllCopper);
        slot.hole = mm(0.8);
        slot.size_x = mm(1.4);
        slot.size_y = mm(2.6);
        slot.shape = PadShape::Round;
        slot.rotation_deg = 0.0;
        slot.fab.slot = mm(2.0);
        slot.fab.slot_deg = 90.0;
        let fp = Footprint {
            name: "A_VERY_LONG_FOOTPRINT_NAME_OVER_31_CHARACTERS/X".into(),
            description: "Test part".into(),
            height: Some(mm(1.0)),
            pads: vec![pad("1", 0.0, LayerRef::Copper(0)), rounded, slot],
            tracks: vec![Track {
                layer: LayerRef::TopSilk,
                net: None,
                start: ptm(-1.0, 1.0),
                end: ptm(6.0, 1.0),
                width: mm(0.15),
                component: None,
                text: false,
            }],
            arcs: vec![Arc {
                layer: LayerRef::TopSilk,
                net: None,
                center: ptm(-1.0, 0.0),
                radius: mm(0.2),
                start_deg: 0.0,
                end_deg: 360.0,
                width: mm(0.1),
                component: None,
            }],
            regions: vec![trace_core::Polygon {
                layer: LayerRef::Other(35),
                net: None,
                outline: vec![ptm(4.0, -1.0), ptm(6.0, -1.0), ptm(6.0, 1.0)],
                holes: Vec::new(),
                pour: None,
                component: None,
            }],
            texts: vec![FpText {
                layer: LayerRef::TopSilk,
                at: ptm(0.0, 2.0),
                height: mm(1.0),
                width: mm(0.15),
                rotation_deg: 90.0,
                mirror: false,
                text: ".Designator".into(),
            }],
            ..Default::default()
        };
        let mut second = fp.clone();
        second.name.push('2');
        let lib = Library {
            version: String::new(),
            footprints: vec![fp.clone(), second],
        };
        let bytes = to_pcblib(&lib, "t.PcbLib").unwrap();
        let dir = std::env::temp_dir().join(format!("trace-pcblib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.PcbLib");
        std::fs::write(&path, bytes).unwrap();
        let back = trace_altium::read_pcblib(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.footprints.len(), 2);
        let f = &back.footprints[0];
        assert_eq!(f.name, fp.name);
        assert_eq!(f.description, "Test part");
        assert_eq!(f.height, fp.height);
        let json = |v: &dyn erased::Json| v.json();
        assert_eq!(json(&f.pads), json(&fp.pads));
        assert_eq!(json(&f.tracks), json(&fp.tracks));
        assert_eq!(json(&f.regions), json(&fp.regions));
        assert_eq!(f.texts.len(), 1);
        assert_eq!(f.texts[0].text, ".Designator");
        assert_eq!(f.texts[0].rotation_deg, 90.0);
        assert_eq!(f.texts[0].at, fp.texts[0].at);
        let a = &f.arcs[0];
        assert_eq!((a.start_deg, a.end_deg), (0.0, 360.0));
    }

    #[test]
    fn integrated_library_footprints_read_back() {
        use std::io::Write;
        use trace_core::footprint::Library;
        let fp = Footprint {
            name: "R0603".into(),
            pads: vec![Pad {
                layer: LayerRef::Copper(0),
                net: None,
                component: None,
                name: "1".into(),
                at: Point::default(),
                size_x: Length::from_mil(30.0),
                size_y: Length::from_mil(35.0),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 0.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            }],
            ..Default::default()
        };
        let lib = Library {
            version: String::new(),
            footprints: vec![fp],
        };
        let pcblib = to_pcblib(&lib, "0.pcblib").unwrap();
        // An IntLib holds each library as a tag byte (2) and zlib data.
        let mut z = flate2::write::ZlibEncoder::new(vec![2u8], flate2::Compression::default());
        z.write_all(&pcblib).unwrap();
        let packed = z.finish().unwrap();
        let dir = std::env::temp_dir().join(format!("trace-intlib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.IntLib");
        {
            let mut c = cfb::create(&path).unwrap();
            c.create_storage("/PCBLib").unwrap();
            c.create_stream("/PCBLib/0.pcblib")
                .unwrap()
                .write_all(&packed)
                .unwrap();
            c.flush().unwrap();
        }
        let back = trace_altium::read_intlib(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.name, "t");
        assert!(back.symbols.symbols.is_empty());
        assert_eq!(back.footprints.footprints.len(), 1);
        let f = &back.footprints.footprints[0];
        assert_eq!(f.name, "R0603");
        assert_eq!(f.pads.len(), 1);
        assert_eq!(f.pads[0].size_x, Length::from_mil(30.0));
    }

    /// Compares values through their JSON, as Trace saves them.
    mod erased {
        pub trait Json {
            fn json(&self) -> String;
        }
        impl<T: serde::Serialize> Json for T {
            fn json(&self) -> String {
                serde_json::to_string(self).unwrap()
            }
        }
    }
}
