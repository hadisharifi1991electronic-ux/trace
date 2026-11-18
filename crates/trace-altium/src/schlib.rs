//! Altium symbol libraries (`.SchLib`) for the symbol editor: each
//! component read into a `SymbolDef` (body, pins, parts, parameters) and
//! written back.
//!
//! A `.SchLib` holds a `FileHeader` stream (library settings, fonts and
//! the component list) and one storage per component, whose `Data` stream
//! is the component record followed by its graphics and pins (pins as
//! binary records, everything owned by the component by order). Long or
//! awkward names get a short storage name, listed in `SectionKeys`.
//!
//! Components that were not edited are written back byte for byte, with
//! the other streams Altium keeps beside `Data`. Edited ones are written
//! from the model, keeping the fields and records it does not hold.

use super::write::{framed, unique_id, width_code, Rec};
use super::*;
use std::io::{Cursor, Write};
use std::sync::Arc;
use trace_core::schematic::PinKind;
use trace_core::symbol::{KeptRec, SymParam, SymPin, SymShape, SymText, SymbolDef};

/// A component's streams: name and bytes.
type Streams = Vec<(String, Arc<Vec<u8>>)>;

/// A library as the editor holds it.
#[derive(Clone, Debug, Default)]
pub struct SchLibFile {
    pub name: String,
    /// The header's own fields (sheet settings), without the component
    /// list, which is written from `symbols`.
    pub header: Vec<(String, String)>,
    /// Fonts: (size in points, bold, italic, name). Font id n is entry n-1.
    pub fonts: Vec<(i64, bool, bool, String)>,
    pub symbols: Vec<SymbolDef>,
    /// The `Storage` stream (pictures), as read.
    pub storage: Option<Vec<u8>>,
}

fn sorted_fields(p: &Props) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = p.keys().map(|(k, v)| (k.clone(), v.clone())).collect();
    v.sort();
    v
}

fn get<'a>(f: &'a [(String, String)], k: &str) -> Option<&'a str> {
    f.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

impl SchLibFile {
    /// An empty library with Altium's usual settings.
    pub fn new(name: &str) -> SchLibFile {
        let header = [
            ("USEMBCS", "T"),
            ("ISBOC", "T"),
            ("SHEETSTYLE", "9"),
            ("BORDERON", "T"),
            ("SHEETNUMBERSPACESIZE", "12"),
            ("AREACOLOR", "16317695"),
            ("SNAPGRIDON", "T"),
            ("SNAPGRIDSIZE", "10"),
            ("VISIBLEGRIDON", "T"),
            ("VISIBLEGRIDSIZE", "10"),
            ("CUSTOMX", "18000"),
            ("CUSTOMY", "18000"),
            ("USECUSTOMSHEET", "T"),
            ("REFERENCEZONESON", "T"),
            ("DISPLAY_UNIT", "0"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        SchLibFile {
            name: name.to_string(),
            header,
            fonts: vec![(10, false, false, "Times New Roman".into())],
            symbols: Vec::new(),
            storage: None,
        }
    }

    /// The fonts as a sheet record holds them, for the drawing code.
    fn font_props(&self) -> Props {
        let mut p = Props::default();
        p.set("RECORD", SHEET.to_string());
        p.set("FONTIDCOUNT", self.fonts.len().to_string());
        for (i, (size, bold, italic, name)) in self.fonts.iter().enumerate() {
            let n = i + 1;
            p.set(&format!("SIZE{n}"), size.to_string());
            if *bold {
                p.set(&format!("BOLD{n}"), "T");
            }
            if *italic {
                p.set(&format!("ITALIC{n}"), "T");
            }
            p.set(&format!("FONTNAME{n}"), name.clone());
        }
        p
    }

    /// A font id for text `size` mil high, adding the font when none fits.
    fn font_id(&mut self, size: f64) -> usize {
        let pts = (size / FONT_SCALE).round().max(1.0) as i64;
        if let Some(i) = self.fonts.iter().position(|f| f.0 == pts && !f.1 && !f.2) {
            return i + 1;
        }
        let name = self
            .fonts
            .first()
            .map(|f| f.3.clone())
            .unwrap_or_else(|| "Times New Roman".into());
        self.fonts.push((pts, false, false, name));
        self.fonts.len()
    }

    fn font_size(&self, id: i64) -> f64 {
        let f = self.fonts.get((id.max(1) - 1) as usize);
        f.map(|f| f.0 as f64).unwrap_or(10.0) * FONT_SCALE
    }

    /// The placeable symbols of component `i`: one per part, drawn by the
    /// same code that reads library files, from the records it would
    /// write.
    pub fn lib_symbols(&self, i: usize) -> Vec<LibSymbol> {
        let Some(def) = self.symbols.get(i) else {
            return Vec::new();
        };
        match def.source.iter().find(|(n, _)| n == "Data") {
            Some((_, data)) => lib_symbols(&split_sch(data), &self.font_props()),
            None => {
                let mut me = self.clone();
                let bytes = me.data_stream(i);
                lib_symbols(&split_sch(&bytes), &me.font_props())
            }
        }
    }

    /// The whole library as a `SymbolLib`, for placing from.
    pub fn symbol_lib(&self) -> SymbolLib {
        SymbolLib {
            name: self.name.clone(),
            symbols: (0..self.symbols.len())
                .flat_map(|i| self.lib_symbols(i))
                .collect(),
        }
    }
}

/// Reads a `.SchLib` for editing.
pub fn read_schlib_file(path: impl AsRef<Path>) -> Result<SchLibFile> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut c = Container::open(path)?;
    let header = c
        .stream("/FileHeader")?
        .ok_or_else(|| Error::MissingStream("FileHeader".into()))?;
    let head = split_sch(&header)
        .into_iter()
        .find_map(|r| match r {
            SchRecord::Text(p) => Some(p),
            _ => None,
        })
        .unwrap_or_default();
    let mut lib = SchLibFile {
        name,
        ..Default::default()
    };
    let n_fonts = head.int("FONTIDCOUNT").unwrap_or(0);
    for i in 1..=n_fonts {
        lib.fonts.push((
            head.int(&format!("SIZE{i}")).unwrap_or(10),
            head.bool(&format!("BOLD{i}")),
            head.bool(&format!("ITALIC{i}")),
            head.get(&format!("FONTNAME{i}"))
                .unwrap_or("Times New Roman")
                .to_string(),
        ));
    }
    if lib.fonts.is_empty() {
        lib.fonts.push((10, false, false, "Times New Roman".into()));
    }
    let listed = |k: &str| {
        let font = [
            "SIZE",
            "BOLD",
            "ITALIC",
            "FONTNAME",
            "ROTATION",
            "UNDERLINE",
            "STRIKEOUT",
        ]
        .iter()
        .any(|f| k.strip_prefix(f).is_some_and(|n| n.parse::<u32>().is_ok()));
        let comp = ["LIBREF", "COMPDESCR", "PARTCOUNT"]
            .iter()
            .any(|f| k.strip_prefix(f).is_some_and(|n| n.parse::<u32>().is_ok()));
        font || comp || matches!(k, "HEADER" | "WEIGHT" | "COMPCOUNT" | "FONTIDCOUNT")
    };
    lib.header = sorted_fields(&head)
        .into_iter()
        .filter(|(k, _)| !listed(k))
        .collect();
    lib.storage = c.stream("/Storage")?;
    // Components in the header's order, which is the library's.
    let order: Vec<String> = (0..head.int("COMPCOUNT").unwrap_or(0))
        .map(|i| head.str(&format!("LIBREF{i}")))
        .collect();
    let storages: Vec<String> = c
        .cfb
        .read_root_storage()
        .filter(|e| e.is_storage())
        .map(|e| e.name().to_string())
        .collect();
    let mut found = Vec::new();
    for st in storages {
        let Some(data) = c.stream(&format!("/{st}/Data"))? else {
            continue;
        };
        let streams: Vec<String> = c
            .cfb
            .read_storage(format!("/{st}"))
            .map(|it| {
                it.filter(|e| e.is_stream())
                    .map(|e| e.name().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let mut def = def_from_records(&split_sch(&data), &lib);
        if def.name.is_empty() {
            continue;
        }
        let mut source = vec![("Data".to_string(), Arc::new(data))];
        for s in streams.into_iter().filter(|s| s != "Data") {
            if let Some(b) = c.stream(&format!("/{st}/{s}"))? {
                source.push((s, Arc::new(b)));
            }
        }
        def.source = source;
        found.push(def);
    }
    let rank = |d: &SymbolDef| {
        order
            .iter()
            .position(|n| *n == d.name)
            .unwrap_or(usize::MAX)
    };
    found.sort_by_key(|d| (rank(d), d.name.to_ascii_lowercase()));
    lib.symbols = found;
    Ok(lib)
}

/// Builds the editor's component from one `Data` stream's records.
fn def_from_records(recs: &[SchRecord], lib: &SchLibFile) -> SymbolDef {
    let props = to_props(recs);
    let mut d = SymbolDef {
        designator: String::new(),
        comment: String::new(),
        show_comment: false,
        ..Default::default()
    };
    let Some(comp) = props.first().filter(|p| p.int("RECORD") == Some(COMPONENT)) else {
        return d;
    };
    d.name = comp.str("LIBREFERENCE");
    d.description = comp.str("COMPONENTDESCRIPTION");
    d.parts = (comp.int("PARTCOUNT").unwrap_or(2) - 1).clamp(1, 26) as u32;
    d.raw = sorted_fields(comp);
    let part_of = |p: &Props| -> u32 {
        match p.int("OWNERPARTID").unwrap_or(1) {
            v if v <= 0 => 0,
            v => v.min(26) as u32,
        }
    };
    // Records owned (directly or not) by a model list: kept whole.
    let mut kept_tree = vec![false; props.len()];
    for (k, p) in props.iter().enumerate().skip(1) {
        let o = p.int("OWNERINDEX").map(|o| o as usize);
        if matches!(p.int("RECORD"), Some(44..=48))
            || o.is_some_and(|o| o > 0 && o < k && kept_tree[o])
        {
            kept_tree[k] = true;
        }
    }
    for (k, p) in props.iter().enumerate().skip(1) {
        let kind = p.int("RECORD").unwrap_or(-1);
        let owned_by_comp = p.int("OWNERINDEX").is_none_or(|o| o <= 0);
        let other_mode = p.int("OWNERPARTDISPLAYMODE").unwrap_or(0) != 0;
        let keep = |d: &mut SymbolDef| {
            d.kept.push(KeptRec {
                index: k,
                fields: sorted_fields(p),
            })
        };
        if kept_tree[k] || other_mode || !owned_by_comp {
            if kind == IMPLEMENTATION
                && p.str("MODELTYPE").eq_ignore_ascii_case("PCBLIB")
                && !d.footprints.contains(&p.str("MODELNAME"))
            {
                let name = p.str("MODELNAME");
                if p.bool("ISCURRENT") {
                    d.footprints.insert(0, name);
                } else {
                    d.footprints.push(name);
                }
            }
            keep(&mut d);
            continue;
        }
        let stroke = color(p, "COLOR", 0x800000);
        let fill = p
            .bool("ISSOLID")
            .then(|| color(p, "AREACOLOR", 0xffffff))
            .filter(|_| !p.bool("TRANSPARENT"));
        let mut shape = |geom: SchGeom, filled: bool| {
            d.shapes.push(SymShape {
                part: part_of(p),
                geom,
                width: line_width(p),
                stroke,
                fill: if filled { fill } else { None },
            })
        };
        match kind {
            PIN => {
                let flags = p.int("PINCONGLOMERATE").unwrap_or(0x18);
                d.pins.push(SymPin {
                    part: part_of(p),
                    base: at(p),
                    len: len(p, "PINLENGTH"),
                    orient: (flags & 3) as u8,
                    name: p.str("NAME"),
                    number: p.str("DESIGNATOR"),
                    kind: PinKind::altium(p.int("ELECTRICAL").unwrap_or(4)),
                    hidden: flags & 0x04 != 0,
                    show_name: flags & 0x08 != 0,
                    show_number: flags & 0x10 != 0,
                    dot: p.int("SYMBOL_OUTEREDGE") == Some(1),
                    clock: p.int("SYMBOL_INNEREDGE") == Some(3),
                    description: p.str("TEXT"),
                });
            }
            POLYLINE => shape(SchGeom::Line(points(p)), false),
            LINE => shape(SchGeom::Line(vec![at(p), corner(p)]), false),
            POLYGON => shape(SchGeom::Poly(points(p)), true),
            RECTANGLE => shape(rect(at(p), corner(p), [0.0, 0.0]), true),
            ROUND_RECT => {
                let r = [len(p, "CORNERXRADIUS"), len(p, "CORNERYRADIUS")];
                shape(rect(at(p), corner(p), r), true)
            }
            ELLIPSE => {
                let r = len(p, "RADIUS");
                let r2 = if p.get("SECONDARYRADIUS").is_some() {
                    len(p, "SECONDARYRADIUS")
                } else {
                    r
                };
                shape(
                    SchGeom::Arc {
                        c: at(p),
                        r: [r, r2],
                        start: 0.0,
                        end: 360.0,
                    },
                    true,
                )
            }
            ARC | ELLIPTICAL_ARC | PIECHART => {
                let r = len(p, "RADIUS");
                let r2 = if kind == ELLIPTICAL_ARC {
                    len(p, "SECONDARYRADIUS")
                } else {
                    r
                };
                let start = p.float("STARTANGLE").unwrap_or(0.0);
                let mut end = p.float("ENDANGLE").unwrap_or(360.0);
                if end <= start {
                    end += 360.0;
                }
                shape(
                    SchGeom::Arc {
                        c: at(p),
                        r: [r, r2],
                        start,
                        end,
                    },
                    kind == PIECHART,
                )
            }
            LABEL => d.texts.push(SymText {
                part: part_of(p),
                at: at(p),
                text: p.str("TEXT"),
                size: lib.font_size(p.int("FONTID").unwrap_or(1)),
                rot: p.int("ORIENTATION").unwrap_or(0).rem_euclid(4) as u8,
                just: p.int("JUSTIFICATION").unwrap_or(0).clamp(0, 8) as u8,
                color: color(p, "COLOR", 0x800000),
            }),
            DESIGNATOR => {
                d.designator = p.str("TEXT");
                d.designator_at = at(p);
                d.designator_raw = sorted_fields(p);
            }
            PARAMETER if p.str("NAME").eq_ignore_ascii_case("Comment") => {
                d.comment = p.str("TEXT");
                d.comment_at = at(p);
                d.show_comment = !p.bool("ISHIDDEN");
                d.comment_raw = sorted_fields(p);
            }
            PARAMETER => d.params.push(SymParam {
                name: p.str("NAME"),
                value: p.str("TEXT"),
                visible: !p.bool("ISHIDDEN"),
                at: at(p),
                raw: sorted_fields(p),
            }),
            _ => keep(&mut d),
        }
    }
    // Altium writes `*` as the designator of a new component.
    if d.designator.is_empty() || d.designator == "*" {
        d.designator = "U?".into();
    }
    if d.comment.is_empty() {
        d.comment = "*".into();
    }
    d
}

fn rec_from(fields: &[(String, String)], kind: i64) -> Rec {
    let mut r = Rec::new(kind);
    for (k, v) in fields {
        if k != "RECORD" {
            r.set(k, v.clone());
        }
    }
    r
}

/// A pin as the binary record Altium writes in libraries, when it fits
/// one (lengths and places on whole 10 mil steps, short Latin-1 texts).
fn binary_pin_bytes(p: &SymPin, flags: i64, color: Rgb) -> Option<Vec<u8>> {
    let whole = |v: f64| {
        let u = v / 10.0;
        ((u - u.round()).abs() < 1e-6 && u.abs() < 32000.0).then_some(u.round() as i16)
    };
    let (x, y, l) = (whole(p.base[0])?, whole(p.base[1])?, whole(p.len)?);
    let pascal = |s: &str, out: &mut Vec<u8>| -> Option<()> {
        if s.len() > 255 || !s.chars().all(|c| (c as u32) < 256 && c != '\0') {
            return None;
        }
        out.push(s.chars().count() as u8);
        out.extend(s.chars().map(|c| c as u32 as u8));
        Some(())
    };
    let mut b = Vec::new();
    b.extend((PIN as i32).to_le_bytes());
    b.push(0);
    b.extend((p.part as i16).to_le_bytes());
    b.push(0); // display mode
    b.push(if p.clock { 3 } else { 0 });
    b.push(if p.dot { 1 } else { 0 });
    b.push(0);
    b.push(0);
    pascal(&p.description, &mut b)?;
    b.push(1);
    b.push(p.kind.altium_number() as u8);
    b.push(flags as u8);
    b.extend(l.to_le_bytes());
    b.extend(x.to_le_bytes());
    b.extend(y.to_le_bytes());
    let c = color[0] as i32 | (color[1] as i32) << 8 | (color[2] as i32) << 16;
    b.extend(c.to_le_bytes());
    pascal(&p.name, &mut b)?;
    pascal(&p.number, &mut b)?;
    pascal("", &mut b)?;
    pascal("|&|", &mut b)?;
    pascal("", &mut b)?;
    Some(b)
}

/// A binary record as stored: length and type 1.
fn framed_binary(body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() as u32 & 0x00ff_ffff) | 0x0100_0000)
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(body);
    v
}

fn shape_rec(s: &SymShape) -> Rec {
    let mut r = match &s.geom {
        SchGeom::Line(v) if v.len() == 2 => {
            let mut r = Rec::new(POLYLINE);
            r.points(v);
            r
        }
        SchGeom::Line(v) => {
            let mut r = Rec::new(POLYLINE);
            r.points(v);
            r
        }
        SchGeom::Poly(v) => {
            let mut r = Rec::new(POLYGON);
            r.points(v);
            r
        }
        SchGeom::Rect { min, max, r: rr } if rr[0] == 0.0 && rr[1] == 0.0 => {
            let mut r = Rec::new(RECTANGLE);
            r.at(*min).corner(*max);
            r
        }
        SchGeom::Rect { min, max, r: rr } => {
            let mut r = Rec::new(ROUND_RECT);
            r.at(*min)
                .corner(*max)
                .len("CORNERXRADIUS", rr[0])
                .len("CORNERYRADIUS", rr[1]);
            r
        }
        SchGeom::Arc {
            c,
            r: rr,
            start,
            end,
        } => {
            let full = (end - start - 360.0).abs() < 1e-6 && start.abs() < 1e-6;
            let kind = if full {
                ELLIPSE
            } else if s.fill.is_some() {
                PIECHART
            } else if (rr[0] - rr[1]).abs() < 0.01 {
                ARC
            } else {
                ELLIPTICAL_ARC
            };
            let mut r = Rec::new(kind);
            r.at(*c).len("RADIUS", rr[0]);
            if kind == ELLIPSE || kind == ELLIPTICAL_ARC {
                r.len("SECONDARYRADIUS", rr[1]);
            }
            if !full {
                r.set("STARTANGLE", format!("{:.3}", start.rem_euclid(360.0)))
                    .set("ENDANGLE", format!("{:.3}", end.rem_euclid(360.0)));
            }
            r
        }
    };
    r.set(
        "OWNERPARTID",
        if s.part == 0 {
            "-1".to_string()
        } else {
            s.part.to_string()
        },
    )
    .set("LINEWIDTH", width_code(s.width).to_string())
    .color("COLOR", s.stroke);
    if let Some(c) = s.fill {
        r.set("ISSOLID", "T").color("AREACOLOR", c);
    }
    r
}

impl SchLibFile {
    /// The `Data` stream of component `i`, from the model.
    fn data_stream(&mut self, i: usize) -> Vec<u8> {
        let def = self.symbols[i].clone();
        let mut out = Vec::new();
        let mut count = 0usize;
        let push = |out: &mut Vec<u8>, r: &Rec| {
            out.extend(framed(&r.bytes()));
        };
        // The component.
        let mut comp = rec_from(&def.raw, COMPONENT);
        for k in [
            "OWNERINDEX",
            "LOCATION.X",
            "LOCATION.Y",
            "LOCATION.X_FRAC",
            "LOCATION.Y_FRAC",
        ] {
            comp.remove(k);
        }
        let seed = def
            .name
            .bytes()
            .fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
        comp.set("LIBREFERENCE", def.name.clone())
            .set("PARTCOUNT", (def.parts + 1).to_string())
            .set("INDEXINSHEET", "-1")
            .set("OWNERPARTID", "-1")
            .set("CURRENTPARTID", "1")
            .set("ALLPINCOUNT", def.pins.len().to_string());
        if comp.get("DISPLAYMODECOUNT").is_none() {
            comp.set("DISPLAYMODECOUNT", "1");
        }
        if comp.get("DESIGNITEMID").is_none() || def.raw.is_empty() {
            comp.set("DESIGNITEMID", def.name.clone());
        }
        if get(&def.raw, "LIBREFERENCE").is_some_and(|n| n != def.name) {
            comp.set("DESIGNITEMID", def.name.clone());
        }
        for (k, v) in [
            ("LIBRARYPATH", "*"),
            ("SOURCELIBRARYNAME", "*"),
            ("SHEETPARTFILENAME", "*"),
            ("TARGETFILENAME", "*"),
            ("AREACOLOR", "11599871"),
            ("COLOR", "128"),
            ("PARTIDLOCKED", "T"),
        ] {
            if comp.get(k).is_none() {
                comp.set(k, v);
            }
        }
        if comp.get("UNIQUEID").is_none() {
            comp.set("UNIQUEID", unique_id(seed));
        }
        if def.description.is_empty() {
            comp.remove("COMPONENTDESCRIPTION");
        } else {
            comp.set("COMPONENTDESCRIPTION", def.description.clone());
        }
        push(&mut out, &comp);
        count += 1;
        // Graphics, then pins, as Altium orders them.
        for (n, s) in def.shapes.iter().enumerate() {
            let mut r = shape_rec(s);
            r.set("INDEXINSHEET", (n + 1).to_string())
                .set("UNIQUEID", unique_id(seed ^ (n as u64 + 1) << 8));
            push(&mut out, &r);
            count += 1;
        }
        for (n, t) in def.texts.iter().enumerate() {
            let mut r = Rec::new(LABEL);
            r.set(
                "OWNERPARTID",
                if t.part == 0 {
                    "-1".to_string()
                } else {
                    t.part.to_string()
                },
            )
            .at(t.at)
            .set("ORIENTATION", t.rot.to_string())
            .set("JUSTIFICATION", t.just.to_string())
            .color("COLOR", t.color)
            .set("FONTID", self.font_id(t.size).to_string())
            .set("TEXT", t.text.clone())
            .set("UNIQUEID", unique_id(seed ^ (n as u64 + 1) << 20));
            push(&mut out, &r);
            count += 1;
        }
        for p in &def.pins {
            let mut flags = (p.orient % 4) as i64 | 0x20;
            if p.hidden {
                flags |= 0x04;
            }
            if p.show_name {
                flags |= 0x08;
            }
            if p.show_number {
                flags |= 0x10;
            }
            match binary_pin_bytes(p, flags, [0, 0, 0]) {
                Some(b) => out.extend(framed_binary(&b)),
                None => {
                    let mut r = Rec::new(PIN);
                    r.set("OWNERPARTID", p.part.to_string())
                        .set("DESCRIPTION", p.description.clone())
                        .set("FORMALTYPE", "1")
                        .set("ELECTRICAL", p.kind.altium_number().to_string())
                        .set("PINCONGLOMERATE", flags.to_string())
                        .len("PINLENGTH", p.len)
                        .at(p.base)
                        .set("NAME", p.name.clone())
                        .set("DESIGNATOR", p.number.clone());
                    if p.dot {
                        r.set("SYMBOL_OUTEREDGE", "1");
                    }
                    if p.clock {
                        r.set("SYMBOL_INNEREDGE", "3");
                    }
                    push(&mut out, &r);
                }
            }
            count += 1;
        }
        // Designator and comment.
        let mut des = rec_from(&def.designator_raw, DESIGNATOR);
        des.remove("OWNERINDEX");
        des.set("INDEXINSHEET", "-1")
            .set("OWNERPARTID", "-1")
            .at(def.designator_at)
            .set("TEXT", def.designator.clone())
            .set("NAME", "Designator")
            .set("READONLYSTATE", "1");
        for (k, v) in [("COLOR", "8388608"), ("FONTID", "1")] {
            if des.get(k).is_none() {
                des.set(k, v);
            }
        }
        if des.get("UNIQUEID").is_none() {
            des.set("UNIQUEID", unique_id(seed ^ 0xD5));
        }
        push(&mut out, &des);
        let mut com = rec_from(&def.comment_raw, PARAMETER);
        com.remove("OWNERINDEX");
        com.set("INDEXINSHEET", "-1")
            .set("OWNERPARTID", "-1")
            .at(def.comment_at)
            .set("TEXT", def.comment.clone())
            .set("NAME", "Comment")
            .flag("ISHIDDEN", !def.show_comment);
        for (k, v) in [("COLOR", "8388608"), ("FONTID", "1")] {
            if com.get(k).is_none() {
                com.set(k, v);
            }
        }
        if com.get("UNIQUEID").is_none() {
            com.set("UNIQUEID", unique_id(seed ^ 0xC0));
        }
        push(&mut out, &com);
        count += 2;
        for (n, prm) in def.params.iter().enumerate() {
            let mut r = rec_from(&prm.raw, PARAMETER);
            r.remove("OWNERINDEX");
            r.set("INDEXINSHEET", "-1")
                .set("OWNERPARTID", "-1")
                .at(prm.at)
                .set("NAME", prm.name.clone())
                .set("TEXT", prm.value.clone())
                .flag("ISHIDDEN", !prm.visible);
            for (k, v) in [("COLOR", "8388608"), ("FONTID", "1")] {
                if r.get(k).is_none() {
                    r.set(k, v);
                }
            }
            if r.get("UNIQUEID").is_none() {
                r.set("UNIQUEID", unique_id(seed ^ (n as u64 + 1) << 32));
            }
            push(&mut out, &r);
            count += 1;
        }
        // Kept records, after everything modelled. The footprint list is
        // rewritten when the footprints changed.
        let kept_names: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for k in &def.kept {
                if get(&k.fields, "RECORD") == Some("45")
                    && get(&k.fields, "MODELTYPE").is_some_and(|m| m.eq_ignore_ascii_case("PCBLIB"))
                {
                    let n = get(&k.fields, "MODELNAME").unwrap_or_default().to_string();
                    if get(&k.fields, "ISCURRENT") == Some("T") {
                        v.insert(0, n);
                    } else {
                        v.push(n);
                    }
                }
            }
            v.dedup();
            v
        };
        let same_models = kept_names == def.footprints;
        let mut map: HashMap<usize, usize> = HashMap::new();
        map.insert(0, 0);
        let mut wrote_list = false;
        for k in &def.kept {
            let kind: i64 = get(&k.fields, "RECORD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if !same_models && (44..=48).contains(&kind) {
                continue;
            }
            let mut r = rec_from(&k.fields, kind);
            if let Some(o) = get(&k.fields, "OWNERINDEX").and_then(|v| v.parse::<usize>().ok()) {
                match map.get(&o) {
                    Some(&n) => {
                        r.owner(n);
                    }
                    None => continue,
                }
            }
            if kind == 44 {
                wrote_list = true;
            }
            map.insert(k.index, count);
            push(&mut out, &r);
            count += 1;
        }
        if !wrote_list {
            push(&mut out, &Rec::new(44));
            count += 1;
            for (n, fp) in def.footprints.iter().enumerate() {
                let mut m = Rec::new(IMPLEMENTATION);
                m.set("USECOMPONENTLIBRARY", "T")
                    .set("MODELNAME", fp.clone())
                    .set("MODELTYPE", "PCBLIB")
                    .set("DATAFILECOUNT", "1")
                    .set("MODELDATAFILEENTITY0", fp.clone())
                    .set("MODELDATAFILEKIND0", "PCBLib")
                    .flag("ISCURRENT", n == 0)
                    .set("UNIQUEID", unique_id(seed ^ (n as u64 + 7) << 40));
                push(&mut out, &m);
                for kind in [46, 48] {
                    push(&mut out, &Rec::new(kind));
                }
                count += 3;
            }
        }
        let _ = count;
        out
    }

    /// The library as `.SchLib` bytes.
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut me = self.clone();
        let mut names: Vec<String> = Vec::new();
        let mut keys: Vec<(String, String)> = Vec::new();
        let mut weight = 0usize;
        let mut comps: Vec<(String, Streams)> = Vec::new();
        for i in 0..me.symbols.len() {
            let def = me.symbols[i].clone();
            let streams = if def.source.iter().any(|(n, _)| n == "Data") {
                def.source.clone()
            } else {
                vec![("Data".to_string(), Arc::new(me.data_stream(i)))]
            };
            if let Some((_, d)) = streams.iter().find(|(n, _)| n == "Data") {
                weight += split_sch(d).len();
            }
            // A storage name: the name, made safe and at most 31 long.
            let clean: String = def
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
            let mut st = if clean.is_empty() {
                "Component".to_string()
            } else {
                clean
            };
            let mut n = 1;
            while names.iter().any(|x| x.eq_ignore_ascii_case(&st)) {
                n += 1;
                let tail = format!("_{n}");
                st = st.chars().take(31 - tail.len()).collect::<String>() + &tail;
            }
            names.push(st.clone());
            if st != def.name {
                keys.push((def.name.clone(), st.clone()));
            }
            comps.push((st, streams));
        }
        let mut head = Rec::default();
        head.set(
            "HEADER",
            "Protel for Windows - Schematic Library Editor Binary File Version 5.0",
        )
        .set("WEIGHT", (weight + 1).to_string())
        .set("MINORVERSION", "9")
        .set(
            "UNIQUEID",
            unique_id(me.symbols.len() as u64 + weight as u64),
        );
        head.set("FONTIDCOUNT", me.fonts.len().to_string());
        for (i, (size, bold, italic, name)) in me.fonts.iter().enumerate() {
            let n = i + 1;
            head.set(&format!("SIZE{n}"), size.to_string());
            if *bold {
                head.set(&format!("BOLD{n}"), "T");
            }
            if *italic {
                head.set(&format!("ITALIC{n}"), "T");
            }
            head.set(&format!("FONTNAME{n}"), name.clone());
        }
        for (k, v) in &me.header {
            if k != "UNIQUEID" && k != "MINORVERSION" {
                head.set(k, v.clone());
            }
        }
        head.set("COMPCOUNT", me.symbols.len().to_string());
        for (i, def) in me.symbols.iter().enumerate() {
            head.set(&format!("LIBREF{i}"), def.name.clone());
            if !def.description.is_empty() {
                head.set(&format!("COMPDESCR{i}"), def.description.clone());
            }
            head.set(&format!("PARTCOUNT{i}"), (def.parts + 1).to_string());
        }
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        cf.create_stream("/FileHeader")?
            .write_all(&framed(&head.bytes()))?;
        let storage = me
            .storage
            .take()
            .unwrap_or_else(|| framed(b"|HEADER=Icon storage"));
        cf.create_stream("/Storage")?.write_all(&storage)?;
        if !keys.is_empty() {
            let mut r = Rec::default();
            r.set("KEYCOUNT", keys.len().to_string());
            for (i, (l, s)) in keys.iter().enumerate() {
                r.set(&format!("LIBREF{i}"), l.clone())
                    .set(&format!("SECTIONKEY{i}"), s.clone());
            }
            let mut b = (r.bytes().len() as u32 + 1).to_le_bytes().to_vec();
            b.extend(r.bytes());
            b.push(0);
            cf.create_stream("/SectionKeys")?.write_all(&b)?;
        }
        for (st, streams) in comps {
            cf.create_storage(format!("/{st}"))?;
            for (n, data) in streams {
                cf.create_stream(format!("/{st}/{n}"))?.write_all(&data)?;
            }
        }
        cf.flush()?;
        Ok(cf.into_inner().into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::symbol::{parse_pin_list, SymbolDef};

    fn round_trip(lib: &SchLibFile, tag: &str) -> SchLibFile {
        let bytes = lib.write().unwrap();
        let p = std::env::temp_dir().join(format!("trace-{tag}-{}.SchLib", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        let back = read_schlib_file(&p).unwrap();
        // The library reader used for placing reads it too.
        let placing = crate::read_schlib(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(
            placing.symbols.len(),
            back.symbols.iter().map(|d| d.parts as usize).sum::<usize>()
        );
        back
    }

    #[test]
    fn new_symbols_save_as_a_schlib_and_read_back() {
        let mut lib = SchLibFile::new("demo");
        let pins = parse_pin_list("1,VDD,pwr\n2,SCL,in\n3,SDA,io\n4,GND,pwr,R\n5,\\INT\\,oc,R\n");
        let mut s = SymbolDef::from_pin_list("SENSOR_WITH_A_VERY_LONG_NAME_OVER_31", &pins);
        s.description = "I2C sensor".into();
        s.footprints = vec!["QFN16".into()];
        s.pins[1].clock = true;
        s.pins[4].dot = true;
        s.pins[3].hidden = true;
        s.texts.push(trace_core::symbol::SymText {
            part: 1,
            at: [50.0, -50.0],
            text: "I2C".into(),
            size: 80.0,
            rot: 0,
            just: 0,
            color: [0, 0, 128],
        });
        // An odd pin length goes as a text record.
        s.pins[2].len = 255.0;
        lib.symbols.push(s.clone());
        let mut dual = SymbolDef::new("LM358");
        dual.designator = "U?".into();
        dual.add_pin(1, [0.0, -100.0], 2);
        dual.add_pin(1, [0.0, -300.0], 2);
        dual.add_pin(1, [600.0, -200.0], 0);
        dual.duplicate_part(1);
        dual.params.push(SymParam {
            name: "Manufacturer".into(),
            value: "TI".into(),
            visible: false,
            at: [0.0, 0.0],
            raw: Vec::new(),
        });
        lib.symbols.push(dual);
        let back = round_trip(&lib, "new");
        assert_eq!(back.symbols.len(), 2);
        let a = back.symbols.iter().find(|d| d.name == s.name).unwrap();
        assert_eq!(a.description, "I2C sensor");
        assert_eq!(a.footprints, ["QFN16"]);
        assert_eq!(a.pins.len(), 5);
        for (x, y) in a.pins.iter().zip(&s.pins) {
            assert_eq!(x, y);
        }
        assert_eq!(a.texts, s.texts);
        assert_eq!(a.shapes.len(), 1);
        let b = back.symbols.iter().find(|d| d.name == "LM358").unwrap();
        assert_eq!(b.parts, 2);
        assert_eq!(b.pins.iter().filter(|p| p.part == 2).count(), 3);
        assert_eq!(b.params[0].value, "TI");
        assert!(!b.params[0].visible);
        // Placeable: one symbol per part, with the pins drawn (not hidden).
        let syms = back.lib_symbols(1);
        assert_eq!(syms.len(), 2);
        assert_eq!(syms[1].drawing.pins.len(), 3);
        assert_eq!(back.lib_symbols(0)[0].drawing.pins.len(), 4);
        // Written again unchanged, the bytes are the same.
        let again = round_trip(&back, "again");
        assert_eq!(again.symbols.len(), 2);
        assert_eq!(again.symbols[0].source[0].1, back.symbols[0].source[0].1);
    }

    /// The project's own library, when it is present (not in CI).
    #[test]
    fn a_real_library_survives_an_edit() {
        let p = std::path::Path::new("/mnt/project-files/SDR_Card.SchLib");
        if !p.exists() {
            return;
        }
        let mut lib = read_schlib_file(p).unwrap();
        let before = crate::read_schlib(p).unwrap();
        assert_eq!(lib.symbols.len(), 129);
        let adrv = lib
            .symbols
            .iter()
            .position(|d| d.name == "ADRV9026")
            .unwrap();
        assert_eq!(lib.symbols[adrv].parts, 3);
        let pins = lib.symbols[adrv].pins.len();
        assert_eq!(pins, 289);
        // Edit it: written from the model now.
        lib.symbols[adrv].touch();
        lib.symbols[adrv].description = "RF transceiver".into();
        let ours = lib.lib_symbols(adrv);
        let theirs: Vec<_> = before
            .symbols
            .iter()
            .filter(|s| s.name == "ADRV9026")
            .collect();
        assert_eq!(ours.len(), theirs.len());
        for (a, b) in ours.iter().zip(&theirs) {
            assert_eq!(a.drawing.pins.len(), b.drawing.pins.len());
            assert_eq!(a.drawing.shapes.len(), b.drawing.shapes.len());
            assert_eq!(a.drawing.texts.len(), b.drawing.texts.len());
            for (x, y) in a.drawing.pins.iter().zip(&b.drawing.pins) {
                assert_eq!(
                    (x.at, &x.number, &x.name, x.kind),
                    (y.at, &y.number, &y.name, y.kind)
                );
            }
        }
        let back = round_trip(&lib, "real");
        assert_eq!(back.symbols.len(), 129);
        let a = back.symbols.iter().find(|d| d.name == "ADRV9026").unwrap();
        assert_eq!(a.description, "RF transceiver");
        assert_eq!(a.pins, lib.symbols[adrv].pins);
    }
}
