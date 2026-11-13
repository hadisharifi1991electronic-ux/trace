//! Altium footprint libraries (`.PcbLib`): one storage per footprint, whose
//! `Data` stream holds the footprint's name and then its primitives, each a
//! type byte followed by the same blocks as in a board's `*6/Data` streams.

use crate::pcb::{
    parse_arc, parse_fill, parse_pad, parse_region, parse_text, parse_track, LayerMap,
};
use crate::records::{split_pcb, Props};
use crate::{Container, Error, Result};
use std::collections::HashMap;
use std::path::Path;
use trace_core::footprint::{Footprint, FpText, Library};
use trace_core::*;

/// Blocks per primitive type: arc 1, pad 6, via 1, track 1, text 2, fill 1,
/// region 1, 3D body 1.
fn blocks_of(kind: u8) -> Option<usize> {
    match kind {
        1 | 3 | 4 | 6 | 11 | 12 => Some(1),
        2 => Some(6),
        5 => Some(2),
        _ => None,
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// Each record of a footprint stream: its type and its blocks.
type Records<'a> = Vec<(u8, Vec<&'a [u8]>)>;

/// The primitives of a footprint `Data` stream: its name and each record's
/// type and blocks.
fn split_footprint(d: &[u8]) -> (String, Records<'_>) {
    let mut out = Vec::new();
    let Some(len) = u32_at(d, 0) else {
        return (String::new(), out);
    };
    let name = d
        .get(4..4 + len)
        .and_then(|b| {
            let n = *b.first()? as usize;
            b.get(1..1 + n)
        })
        .map(|b| b.iter().map(|&c| c as char).collect())
        .unwrap_or_default();
    let mut i = 4 + len;
    'records: while i < d.len() {
        let kind = d[i];
        let Some(n) = blocks_of(kind) else { break };
        i += 1;
        let mut blocks = Vec::with_capacity(n);
        for _ in 0..n {
            let Some(len) = u32_at(d, i) else {
                break 'records;
            };
            i += 4;
            let Some(b) = d.get(i..i + len) else {
                break 'records;
            };
            blocks.push(b);
            i += len;
        }
        out.push((kind, blocks));
    }
    (name, out)
}

/// Text as a footprint keeps it, from Altium's text record.
fn fp_text(t: &crate::pcb::AltText) -> FpText {
    let (height, width) = if t.truetype {
        (
            Length((t.height.0 as f64 * 0.7) as i64),
            Length((t.height.0 as f64 * if t.bold { 0.13 } else { 0.09 }) as i64),
        )
    } else {
        (t.height, t.width)
    };
    FpText {
        layer: t.layer.clone(),
        at: t.at,
        height,
        width,
        rotation_deg: t.rotation_deg,
        mirror: t.mirror,
        text: t.text.clone(),
    }
}

/// Wide text of a footprint: `|ENCODEDTEXTn=c,c,c...` with character codes.
fn wide_strings(d: &[u8]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    for p in split_pcb(d, 0) {
        for (k, v) in p.keys() {
            let Some(i) = k.strip_prefix("ENCODEDTEXT").and_then(|n| n.parse().ok()) else {
                continue;
            };
            let text: String = v
                .split(',')
                .filter_map(|c| c.trim().parse::<u32>().ok().and_then(char::from_u32))
                .collect();
            out.insert(i, text);
        }
    }
    out
}

/// A two-layer frame: Altium's top (1) and bottom (32) copper.
fn frame() -> LayerMap {
    let layer = |id: u8| StackLayer {
        name: String::new(),
        altium_id: Some(id),
        kind: LayerKind::Copper,
        thickness: Length(0),
        dielectric_constant: None,
        loss_tangent: None,
        roughness_um: None,
        material: None,
    };
    LayerMap::new(&Stackup {
        layers: vec![layer(1), layer(32)],
    })
}

/// One footprint from its `Data` stream and its `Parameters` and
/// `WideStrings` streams.
fn footprint(data: &[u8], params: Option<&Props>, wide: &HashMap<u32, String>) -> Footprint {
    let layers = frame();
    let (name, records) = split_footprint(data);
    let mut fp = Footprint {
        name: params
            .map(|p| p.str("PATTERN"))
            .filter(|n| !n.is_empty())
            .unwrap_or(name),
        description: params.map(|p| p.str("DESCRIPTION")).unwrap_or_default(),
        height: params.and_then(|p| p.get("HEIGHT")).and_then(Length::parse),
        copper: 2,
        ..Default::default()
    };
    for (kind, b) in &records {
        let first = b.first().copied().unwrap_or(&[]);
        match kind {
            1 => fp.arcs.extend(parse_arc(first, &layers)),
            2 => fp.pads.extend(parse_pad(b, &layers)),
            4 => fp.tracks.extend(parse_track(first, &layers)),
            5 => fp
                .texts
                .extend(parse_text(b, &layers, wide).map(|t| fp_text(&t))),
            6 => fp.regions.extend(parse_fill(first, &layers)),
            11 => fp.regions.extend(parse_region(first, &layers)),
            _ => {}
        }
    }
    let none = |n: &mut Option<u32>, c: &mut Option<u32>| {
        *n = None;
        *c = None;
    };
    for p in &mut fp.pads {
        none(&mut p.net, &mut p.component);
    }
    for t in &mut fp.tracks {
        none(&mut t.net, &mut t.component);
    }
    for a in &mut fp.arcs {
        none(&mut a.net, &mut a.component);
    }
    for r in &mut fp.regions {
        none(&mut r.net, &mut r.component);
        r.pour = None;
    }
    fp
}

/// Reads every footprint of an Altium `.PcbLib`.
pub fn read_pcblib(path: impl AsRef<Path>) -> Result<Library> {
    pcblib_from(&mut Container::open(path.as_ref())?)
}

/// The footprints of a `.PcbLib` already open (on disk or in memory).
pub(crate) fn pcblib_from<F: std::io::Read + std::io::Seek>(
    c: &mut Container<F>,
) -> Result<Library> {
    if !c.cfb.is_stream("/Library/Data") {
        return Err(Error::MissingStream("Library/Data".into()));
    }
    let storages: Vec<String> = c
        .cfb
        .read_root_storage()
        .filter(|e| {
            e.is_storage()
                && !["Library", "FileVersionInfo"]
                    .iter()
                    .any(|n| e.name().eq_ignore_ascii_case(n))
        })
        .map(|e| e.name().to_string())
        .collect();
    let mut lib = Library {
        version: String::new(),
        footprints: Vec::new(),
    };
    for s in storages {
        let Some(data) = c.stream(&format!("/{s}/Data"))? else {
            continue;
        };
        let params = c
            .stream(&format!("/{s}/Parameters"))?
            .and_then(|d| split_pcb(&d, 0).into_iter().next());
        let wide = c
            .stream(&format!("/{s}/WideStrings"))?
            .map(|d| wide_strings(&d))
            .unwrap_or_default();
        lib.footprints
            .push(footprint(&data, params.as_ref(), &wide));
    }
    lib.footprints.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    Ok(lib)
}
