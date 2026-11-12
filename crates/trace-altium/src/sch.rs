//! Altium schematic sheets (`.SchDoc`): the part list for `trace inspect`,
//! and the full drawing for the schematic viewer.
//!
//! In a `.SchDoc` every primitive is stored in sheet coordinates, already
//! placed and rotated, so a part's graphics need no transform. Coordinates
//! are in units of 10 mil plus a `_FRAC` field in 1/100000 of a unit.

use crate::records::{split_sch, Props, SchRecord};
use crate::{Container, Error, Result};
use std::collections::HashMap;
use std::path::Path;
use trace_core::{
    Drawing, LibSymbol, NetTag, Pt, Rgb, SchGeom, SchPart, SchPin, SchShape, SchText, Sheet,
    Symbol, SymbolLib, TagKind,
};

// Record ids in the schematic `FileHeader` stream.
const COMPONENT: i64 = 1;
const PIN: i64 = 2;
const LABEL: i64 = 4;
const BEZIER: i64 = 5;
const POLYLINE: i64 = 6;
const POLYGON: i64 = 7;
const ELLIPSE: i64 = 8;
const PIECHART: i64 = 9;
const ROUND_RECT: i64 = 10;
const ELLIPTICAL_ARC: i64 = 11;
const ARC: i64 = 12;
const LINE: i64 = 13;
const RECTANGLE: i64 = 14;
const SHEET_SYMBOL: i64 = 15;
const SHEET_ENTRY: i64 = 16;
const POWER_PORT: i64 = 17;
const PORT: i64 = 18;
const NO_ERC: i64 = 22;
const NET_LABEL: i64 = 25;
const BUS: i64 = 26;
const WIRE: i64 = 27;
const TEXT_FRAME: i64 = 28;
const JUNCTION: i64 = 29;
const IMAGE: i64 = 30;
const SHEET: i64 = 31;
const SHEET_NAME: i64 = 32;
const SHEET_FILE_NAME: i64 = 33;
const DESIGNATOR: i64 = 34;
const BUS_ENTRY: i64 = 37;
const PARAMETER: i64 = 41;
const PARAMETER_SET: i64 = 43;
const IMPLEMENTATION: i64 = 45;
const NOTE: i64 = 209;
const BLANKET: i64 = 225;
const HYPERLINK: i64 = 226;

/// Reads one `.SchDoc` sheet: symbols with designators and values, net labels,
/// power ports, ports, links to child sheets, and the drawing.
pub fn read_schdoc(path: impl AsRef<Path>) -> Result<Sheet> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Sheets saved in Altium's ASCII format: one `|KEY=VALUE|...` record per line.
    let head = std::fs::read(path).map_err(|source| Error::Open {
        path: path.display().to_string(),
        source,
    })?;
    if head.starts_with(b"|HEADER=") || head.starts_with(b"|RECORD=") {
        let text: String = head.iter().map(|&b| b as char).collect();
        let records: Vec<SchRecord> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| SchRecord::Text(Props::parse(l)))
            .collect();
        return Ok(read_records(&records, name, &file_name));
    }
    let mut c = Container::open(path)?;
    let data = c
        .stream("/FileHeader")?
        .ok_or_else(|| Error::MissingStream("FileHeader".into()))?;
    let storage = c
        .stream("/Storage")?
        .map(|s| read_storage(&s))
        .unwrap_or_default();
    let records = split_sch(&data);
    let mut sheet = read_records(&records, name, &file_name);
    // Blankets (dashed outlines that apply directives to an area) are kept
    // in a side stream.
    if let Some(extra) = c.stream("/Additional")? {
        for r in split_sch(&extra) {
            let SchRecord::Text(p) = r else { continue };
            if p.int("RECORD") == Some(BLANKET) {
                sheet.drawing.shapes.push(trace_core::SchShape {
                    geom: SchGeom::Poly(points(&p)),
                    width: line_width(&p),
                    stroke: Some(color(&p, "COLOR", 0xff)),
                    fill: None,
                    dashed: true,
                    part: None,
                    wire: None,
                });
            }
        }
    }
    // Embedded pictures, matched to their records by file name.
    for p in to_props(&records) {
        if p.int("RECORD") != Some(IMAGE) || !p.bool("EMBEDIMAGE") {
            continue;
        }
        let want = p.str("FILENAME");
        let found = storage.iter().find(|(n, _)| *n == want).or_else(|| {
            storage
                .iter()
                .find(|(n, _)| want.ends_with(n.as_str()) || n.ends_with(&want))
        });
        if let Some((_, bytes)) = found {
            let (a, c) = (at(&p), corner(&p));
            sheet.drawing.images.push(trace_core::SchImage {
                min: [a[0].min(c[0]), a[1].min(c[1])],
                max: [a[0].max(c[0]), a[1].max(c[1])],
                keep_aspect: p.bool("KEEPASPECT"),
                data: bytes.clone(),
            });
        }
    }
    Ok(sheet)
}

/// Reads an Altium symbol library (`.SchLib`): one storage per component,
/// whose `Data` stream holds the component record and then its graphics
/// and pins (owned by it without saying so), in symbol coordinates.
/// Multi-part components give one symbol per part.
pub fn read_schlib(path: impl AsRef<Path>) -> Result<SymbolLib> {
    let path = path.as_ref();
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
    // The library's fonts, as a sheet record would give them.
    let mut fonts = Props::default();
    fonts.set("RECORD", SHEET.to_string());
    for (k, v) in head.keys() {
        let font_key = k == "FONTIDCOUNT"
            || ["SIZE", "BOLD", "ITALIC", "FONTNAME"]
                .iter()
                .any(|f| k.strip_prefix(f).is_some_and(|n| n.parse::<u32>().is_ok()));
        if font_key {
            fonts.set(k, v.clone());
        }
    }
    let storages: Vec<String> = c
        .cfb
        .read_root_storage()
        .filter(|e| e.is_storage())
        .map(|e| e.name().to_string())
        .collect();
    let mut lib = SymbolLib {
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        symbols: Vec::new(),
    };
    for st in storages {
        let Some(data) = c.stream(&format!("/{st}/Data"))? else {
            continue;
        };
        lib.symbols.extend(lib_symbols(&split_sch(&data), &fonts));
    }
    lib.symbols.sort_by(|a, b| {
        (a.name.to_ascii_lowercase(), a.part).cmp(&(b.name.to_ascii_lowercase(), b.part))
    });
    Ok(lib)
}

/// The symbols of one library component's records: one per part.
fn lib_symbols(recs: &[SchRecord], fonts: &Props) -> Vec<LibSymbol> {
    let Some(SchRecord::Text(comp)) = recs.first() else {
        return Vec::new();
    };
    if comp.int("RECORD") != Some(COMPONENT) {
        return Vec::new();
    }
    let name = comp.str("LIBREFERENCE");
    // Altium counts one more part than there are.
    let parts = (comp.int("PARTCOUNT").unwrap_or(2) - 1).clamp(1, 26) as u32;
    let mut out = Vec::new();
    for part in 1..=parts {
        let mut list = vec![SchRecord::Text(fonts.clone())];
        for (k, r) in recs.iter().enumerate() {
            list.push(match r {
                SchRecord::Text(p) if k == 0 => {
                    let mut p = p.clone();
                    p.set("CURRENTPARTID", part.to_string());
                    p.set("DISPLAYMODE", "0");
                    SchRecord::Text(p)
                }
                SchRecord::Text(p) if p.get("OWNERINDEX").is_none() => {
                    let mut p = p.clone();
                    p.set("OWNERINDEX", "0");
                    SchRecord::Text(p)
                }
                r => r.clone(),
            });
        }
        let sheet = read_records(&list, name.clone(), "");
        let mut drawing = sheet.drawing;
        if drawing.parts.is_empty() || (drawing.pins.is_empty() && drawing.shapes.is_empty()) {
            continue;
        }
        let q = &drawing.parts[0];
        drawing.size = [q.max[0] - q.min[0], q.max[1] - q.min[1]];
        let mut symbol = sheet.symbols.into_iter().next().unwrap_or_default();
        if symbol.designator.is_empty() {
            symbol.designator = "U?".into();
            drawing.parts[0].designator = symbol.designator.clone();
        }
        out.push(LibSymbol {
            name: name.clone(),
            part,
            parts,
            symbol,
            drawing,
        });
    }
    out
}

/// The `Storage` stream: a header record, then for each embedded file a
/// 5-byte tag, a short name, a u32 length and zlib-compressed bytes.
fn read_storage(s: &[u8]) -> Vec<(String, std::sync::Arc<Vec<u8>>)> {
    use std::io::Read;
    let mut out = Vec::new();
    if s.len() < 4 {
        return out;
    }
    let hl = (u32::from_le_bytes(s[0..4].try_into().unwrap()) & 0x00ff_ffff) as usize;
    let header = Props::parse(
        String::from_utf8_lossy(s.get(4..4 + hl).unwrap_or_default()).trim_end_matches('\0'),
    );
    let mut i = 4 + hl;
    for _ in 0..header.int("WEIGHT").unwrap_or(0) {
        i += 5;
        let Some(&n) = s.get(i) else { break };
        let Some(name) = s.get(i + 1..i + 1 + n as usize) else {
            break;
        };
        let name: String = name.iter().map(|&c| c as char).collect();
        i += 1 + n as usize;
        let Some(len) = s.get(i..i + 4) else { break };
        let len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        i += 4;
        let Some(z) = s.get(i..i + len) else { break };
        i += len;
        let mut bytes = Vec::new();
        if flate2::read::ZlibDecoder::new(z)
            .read_to_end(&mut bytes)
            .is_err()
        {
            bytes = z.to_vec();
        }
        out.push((name, std::sync::Arc::new(bytes)));
    }
    out
}

/// Turns raw records into property sets. Binary records are pins in some
/// file versions; they carry no owner, and follow their component.
fn to_props(records: &[SchRecord]) -> Vec<Props> {
    let mut last_component = None;
    records
        .iter()
        .enumerate()
        .map(|(k, r)| match r {
            SchRecord::Text(p) => {
                if p.int("RECORD") == Some(COMPONENT) {
                    last_component = Some(k);
                }
                p.clone()
            }
            SchRecord::Binary(b) => {
                let mut p = binary_pin(b).unwrap_or_default();
                if let Some(o) = last_component {
                    p.set("OWNERINDEX", (o as i64 - 1).to_string());
                }
                p
            }
        })
        .collect()
}

/// Decodes a binary pin record (layout as KiCad's importer reads it).
fn binary_pin(b: &[u8]) -> Option<Props> {
    let mut i = 0usize;
    let u8_ = |i: &mut usize| -> Option<u8> {
        let v = *b.get(*i)?;
        *i += 1;
        Some(v)
    };
    let i16_ = |i: &mut usize| -> Option<i16> {
        let v = i16::from_le_bytes(b.get(*i..*i + 2)?.try_into().ok()?);
        *i += 2;
        Some(v)
    };
    let i32_ = |i: &mut usize| -> Option<i32> {
        let v = i32::from_le_bytes(b.get(*i..*i + 4)?.try_into().ok()?);
        *i += 4;
        Some(v)
    };
    let pascal = |i: &mut usize| -> Option<String> {
        let n = *b.get(*i)? as usize;
        let s = b.get(*i + 1..*i + 1 + n)?;
        *i += 1 + n;
        Some(s.iter().map(|&c| c as char).collect())
    };
    if i32_(&mut i)? != PIN as i32 {
        return None;
    }
    let mut p = Props::default();
    p.set("RECORD", "2");
    u8_(&mut i)?;
    p.set("OWNERPARTID", i16_(&mut i)?.to_string());
    p.set("OWNERPARTDISPLAYMODE", u8_(&mut i)?.to_string());
    p.set("SYMBOL_INNEREDGE", u8_(&mut i)?.to_string());
    p.set("SYMBOL_OUTEREDGE", u8_(&mut i)?.to_string());
    p.set("SYMBOL_INNER", u8_(&mut i)?.to_string());
    p.set("SYMBOL_OUTER", u8_(&mut i)?.to_string());
    p.set("TEXT", pascal(&mut i)?);
    u8_(&mut i)?;
    p.set("ELECTRICAL", u8_(&mut i)?.to_string());
    p.set("PINCONGLOMERATE", u8_(&mut i)?.to_string());
    p.set("PINLENGTH", i16_(&mut i)?.to_string());
    p.set("LOCATION.X", i16_(&mut i)?.to_string());
    p.set("LOCATION.Y", i16_(&mut i)?.to_string());
    p.set("COLOR", i32_(&mut i)?.to_string());
    p.set("NAME", pascal(&mut i)?);
    p.set("DESIGNATOR", pascal(&mut i)?);
    Some(p)
}

/// A length in mil from a unit field and its `_FRAC` partner.
fn len(p: &Props, key: &str) -> f64 {
    let v = p.int(key).unwrap_or(0) as f64;
    let f = p.int(&format!("{key}_FRAC")).unwrap_or(0) as f64;
    v * 10.0 + f / 10000.0
}

fn pt(p: &Props, x: &str, y: &str) -> Pt {
    [len(p, x), len(p, y)]
}

fn at(p: &Props) -> Pt {
    pt(p, "LOCATION.X", "LOCATION.Y")
}

fn corner(p: &Props) -> Pt {
    pt(p, "CORNER.X", "CORNER.Y")
}

fn points(p: &Props) -> Vec<Pt> {
    let n = p.int("LOCATIONCOUNT").unwrap_or(0).max(0);
    (1..=n)
        .map(|i| pt(p, &format!("X{i}"), &format!("Y{i}")))
        .collect()
}

/// Altium colours are Windows COLORREFs: 0x00BBGGRR.
fn rgb(v: i64) -> Rgb {
    [
        (v & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        ((v >> 16) & 0xff) as u8,
    ]
}

fn color(p: &Props, key: &str, default: i64) -> Rgb {
    rgb(p.int(key).unwrap_or(default))
}

/// Line widths are an enum (smallest, small, medium, large).
fn line_width(p: &Props) -> f64 {
    match p.int("LINEWIDTH").unwrap_or(0) {
        0 => 0.0,
        1 => 10.0,
        2 => 30.0,
        _ => 50.0,
    }
}

fn dir(orient: i64) -> Pt {
    match orient.rem_euclid(4) {
        0 => [1.0, 0.0],
        1 => [0.0, 1.0],
        2 => [-1.0, 0.0],
        _ => [0.0, -1.0],
    }
}

fn add(a: Pt, b: Pt, k: f64) -> Pt {
    [a[0] + b[0] * k, a[1] + b[1] * k]
}

/// Sheet sizes in mil, by `SHEETSTYLE`.
fn sheet_size(style: i64) -> Pt {
    let s = match style {
        1 => [1550.0, 1110.0],
        2 => [2230.0, 1570.0],
        3 => [3150.0, 2230.0],
        4 => [4460.0, 3150.0],
        5 => [950.0, 750.0],
        6 => [1500.0, 950.0],
        7 => [2000.0, 1500.0],
        8 => [3200.0, 2000.0],
        9 => [4200.0, 3200.0],
        10 => [1100.0, 850.0],
        11 => [1400.0, 850.0],
        12 => [1700.0, 1100.0],
        13 => [990.0, 790.0],
        14 => [1540.0, 990.0],
        15 => [2060.0, 1560.0],
        16 => [3260.0, 2060.0],
        17 => [4280.0, 3280.0],
        _ => [1150.0, 760.0],
    };
    [s[0] * 10.0, s[1] * 10.0]
}

#[derive(Clone)]
struct Font {
    size: f64,
    bold: bool,
    italic: bool,
}

/// Text in sheet fonts: a size of 10 is 100 mil high.
const FONT_SCALE: f64 = 10.0;

struct Builder<'a> {
    recs: &'a [Props],
    d: Drawing,
    fonts: Vec<Font>,
    /// For each record: the drawing part it belongs to.
    part_of: Vec<Option<u32>>,
    /// Component record -> (current part id, display mode).
    shown: HashMap<usize, (i64, i64)>,
    doc_params: HashMap<String, String>,
    comp_params: HashMap<(usize, String), String>,
    file_name: String,
}

impl Builder<'_> {
    fn font(&self, id: i64) -> Font {
        self.fonts
            .get((id - 1).max(0) as usize)
            .cloned()
            .unwrap_or(Font {
                size: 10.0,
                bold: false,
                italic: false,
            })
    }

    fn owner(&self, p: &Props) -> Option<usize> {
        let o = p.int("OWNERINDEX")?;
        let k = (o + 1) as usize;
        (k < self.recs.len()).then_some(k)
    }

    /// False for graphics of another part or display mode of a multi-part
    /// component.
    fn visible(&self, p: &Props) -> bool {
        let Some(o) = self.owner(p) else { return true };
        let Some(&(part, mode)) = self.shown.get(&o) else {
            return true;
        };
        let pid = p.int("OWNERPARTID").unwrap_or(-1);
        let pmode = p.int("OWNERPARTDISPLAYMODE").unwrap_or(0);
        (pid <= 0 || pid == part) && pmode == mode
    }

    fn shape(&mut self, k: usize, geom: SchGeom, p: &Props, filled: bool) {
        let fill = (filled && p.bool("ISSOLID") && !p.bool("TRANSPARENT"))
            .then(|| color(p, "AREACOLOR", 0xffffff));
        self.d.shapes.push(SchShape {
            geom,
            width: line_width(p),
            stroke: Some(color(p, "COLOR", 0)),
            fill,
            dashed: matches!(p.int("LINESTYLE").or(p.int("LINESTYLEEXT")), Some(1..=3)),
            part: self.part_of[k],
            wire: None,
        });
    }

    fn line(&mut self, k: usize, pts: Vec<Pt>, width: f64, c: Rgb) {
        self.d.shapes.push(SchShape {
            geom: SchGeom::Line(pts),
            width,
            stroke: Some(c),
            fill: None,
            dashed: false,
            part: self.part_of[k],
            wire: None,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, k: usize, at: Pt, text: String, font: i64, c: Rgb, rot: i64, just: u8) {
        if text.is_empty() {
            return;
        }
        let f = self.font(font);
        // Altium keeps text readable: 180 and 270 degrees draw as 0 and 90
        // with the anchor on the opposite corner.
        let (rot, just) = match rot.rem_euclid(4) as u8 {
            r @ 2..=3 => (r - 2, 8 - just),
            r => (r, just),
        };
        self.d.texts.push(SchText {
            at,
            text,
            size: f.size * FONT_SCALE,
            color: c,
            rot,
            just,
            bold: f.bold,
            italic: f.italic,
            wrap: None,
            part: self.part_of[k],
        });
    }

    /// Text from a label-like record: location, orientation, justification.
    fn label(&mut self, k: usize, p: &Props, text: String) {
        let mut just = p.int("JUSTIFICATION").unwrap_or(0).clamp(0, 8) as u8;
        if p.bool("ISMIRRORED") {
            just = just / 3 * 3 + (2 - just % 3);
        }
        self.text(
            k,
            at(p),
            text,
            p.int("FONTID").unwrap_or(1),
            color(p, "COLOR", 0x800000),
            p.int("ORIENTATION").unwrap_or(0),
            just,
        );
    }

    /// `=Name` texts refer to a parameter: the owner's, then the sheet's.
    fn resolve(&self, owner: Option<usize>, text: &str) -> String {
        let Some(name) = text.strip_prefix('=') else {
            return text.to_string();
        };
        let key = name.to_ascii_uppercase();
        if key == "DOCUMENTNAME" || key == "DOCUMENTFULLPATHANDNAME" {
            return self.file_name.clone();
        }
        if let Some(o) = owner {
            if let Some(v) = self.comp_params.get(&(o, key.clone())) {
                return v.clone();
            }
        }
        if let Some(v) = self.doc_params.get(&key) {
            return if v == "*" { String::new() } else { v.clone() };
        }
        String::new()
    }
}

fn read_records(records: &[SchRecord], name: String, file_name: &str) -> Sheet {
    let recs = to_props(records);
    let rec = |p: &Props| p.int("RECORD");
    let mut sheet = Sheet {
        name,
        ..Default::default()
    };
    // Components first: the sheet's part list, and which part of each is shown.
    let mut symbol_at: HashMap<usize, usize> = HashMap::new();
    let mut shown = HashMap::new();
    let mut part_of = vec![None; recs.len()];
    let mut parts = Vec::new();
    for (k, p) in recs.iter().enumerate() {
        if rec(p) == Some(COMPONENT) {
            symbol_at.insert(k, sheet.symbols.len());
            shown.insert(
                k,
                (
                    p.int("CURRENTPARTID").unwrap_or(1),
                    p.int("DISPLAYMODE").unwrap_or(0),
                ),
            );
            part_of[k] = Some(parts.len() as u32);
            parts.push((k, sheet.symbols.len()));
            sheet.symbols.push(Symbol {
                library_ref: p.str("LIBREFERENCE"),
                description: p.str("COMPONENTDESCRIPTION"),
                ..Default::default()
            });
        }
    }
    let owner_of = |p: &Props| -> Option<usize> {
        let k = (p.int("OWNERINDEX")? + 1) as usize;
        (k < recs.len()).then_some(k)
    };
    // Children inherit their owner's part (pins, labels inside sub-records).
    for k in 0..recs.len() {
        if part_of[k].is_none() {
            if let Some(o) = owner_of(&recs[k]) {
                if o < k {
                    part_of[k] = part_of[o];
                }
            }
        }
    }

    // Each part's footprint: its current PCB model, an implementation
    // (45) in the part's implementation list (44).
    for p in &recs {
        if rec(p) != Some(IMPLEMENTATION)
            || !p.str("MODELTYPE").eq_ignore_ascii_case("PCBLIB")
            || !p.bool("ISCURRENT")
        {
            continue;
        }
        let comp = owner_of(p).and_then(|o| owner_of(&recs[o]));
        if let Some(&s) = comp.and_then(|c| symbol_at.get(&c)) {
            sheet.symbols[s].footprint = p.str("MODELNAME");
        }
    }

    let mut b = Builder {
        recs: &recs,
        d: Drawing::default(),
        fonts: Vec::new(),
        part_of,
        shown,
        doc_params: HashMap::new(),
        comp_params: HashMap::new(),
        file_name: file_name.to_string(),
    };
    for p in recs.iter() {
        match rec(p) {
            Some(PIN) => {
                if let Some(&s) = owner_of(p).and_then(|o| symbol_at.get(&o)) {
                    if b.visible(p) {
                        sheet.symbols[s].pins += 1;
                    }
                }
            }
            Some(DESIGNATOR) => {
                if let Some(&s) = owner_of(p).and_then(|o| symbol_at.get(&o)) {
                    sheet.symbols[s].designator = p.str("TEXT");
                }
            }
            Some(PARAMETER) => match owner_of(p) {
                Some(o) => {
                    let name = p.str("NAME").to_ascii_uppercase();
                    if let Some(&s) = symbol_at.get(&o) {
                        if name == "VALUE" {
                            sheet.symbols[s].value = p.get("TEXT").map(str::to_string);
                        }
                    }
                    b.comp_params.insert((o, name), p.str("TEXT"));
                }
                None => {
                    b.doc_params
                        .insert(p.str("NAME").to_ascii_uppercase(), p.str("TEXT"));
                }
            },
            Some(SHEET) => {
                let n = p.int("FONTIDCOUNT").unwrap_or(0);
                b.fonts = (1..=n)
                    .map(|i| Font {
                        size: p.int(&format!("SIZE{i}")).unwrap_or(10) as f64,
                        bold: p.bool(&format!("BOLD{i}")),
                        italic: p.bool(&format!("ITALIC{i}")),
                    })
                    .collect();
                b.d.size = if p.bool("USECUSTOMSHEET") {
                    [len(p, "CUSTOMX"), len(p, "CUSTOMY")]
                } else {
                    sheet_size(p.int("SHEETSTYLE").unwrap_or(0))
                };
                if p.int("WORKSPACEORIENTATION") == Some(1) {
                    b.d.size = [b.d.size[1], b.d.size[0]];
                }
                b.d.paper = color(p, "AREACOLOR", 0xdbffff);
                if p.bool("BORDERON") {
                    border(&mut b.d, p);
                }
            }
            Some(NET_LABEL) => sheet.net_labels.push(p.str("TEXT")),
            Some(POWER_PORT) => sheet.power_ports.push(p.str("TEXT")),
            Some(PORT) => sheet.ports.push(p.str("NAME")),
            Some(SHEET_FILE_NAME) => sheet.sheet_refs.push(p.str("TEXT")),
            Some(WIRE) => sheet.wires += 1,
            Some(JUNCTION) => sheet.junctions += 1,
            _ => {}
        }
    }
    // Designators get a part letter on multi-part components.
    for p in recs.iter() {
        if rec(p) == Some(DESIGNATOR) {
            if let Some(o) = owner_of(p) {
                let c = &recs[o];
                if rec(c) == Some(COMPONENT) && c.int("PARTCOUNT").unwrap_or(0) > 2 {
                    let id = c.int("CURRENTPARTID").unwrap_or(1).clamp(1, 26) as u8;
                    b.comp_params.insert(
                        (o, "DESIGNATOR#".into()),
                        format!("{}{}", p.str("TEXT"), (b'A' + id - 1) as char),
                    );
                }
            }
        }
    }

    // Sheet symbols, for placing their entries.
    let mut sheet_boxes: HashMap<usize, (Pt, Pt)> = HashMap::new();
    for (k, p) in recs.iter().enumerate() {
        if rec(p) != Some(COMPONENT) && !b.visible(p) {
            continue;
        }
        match rec(p) {
            Some(PIN) => pin(&mut b, k, p),
            Some(LABEL) | Some(HYPERLINK) => {
                if !p.bool("ISHIDDEN") {
                    let t = b.resolve(owner_of(p), &p.str("TEXT"));
                    b.label(k, p, t);
                }
            }
            Some(DESIGNATOR) => {
                if !p.bool("ISHIDDEN") {
                    let o = owner_of(p);
                    let t = o
                        .and_then(|o| b.comp_params.get(&(o, "DESIGNATOR#".into())).cloned())
                        .unwrap_or_else(|| p.str("TEXT"));
                    b.label(k, p, t);
                }
            }
            Some(PARAMETER) => {
                if !p.bool("ISHIDDEN") && p.get("LOCATION.X").is_some() {
                    let o = owner_of(p);
                    let mut t = b.resolve(o, &p.str("TEXT"));
                    if t == "*" {
                        t.clear();
                    }
                    if p.bool("SHOWNAME") && !t.is_empty() {
                        t = format!("{}: {t}", p.str("NAME"));
                    }
                    b.label(k, p, t);
                }
            }
            Some(BEZIER) => {
                let pts = points(p);
                let mut out = Vec::new();
                let mut i = 0;
                while i + 3 < pts.len() {
                    let (a, c1, c2, e) = (pts[i], pts[i + 1], pts[i + 2], pts[i + 3]);
                    for s in 0..=16 {
                        let t = s as f64 / 16.0;
                        let u = 1.0 - t;
                        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                        out.push([
                            w[0] * a[0] + w[1] * c1[0] + w[2] * c2[0] + w[3] * e[0],
                            w[0] * a[1] + w[1] * c1[1] + w[2] * c2[1] + w[3] * e[1],
                        ]);
                    }
                    i += 3;
                }
                if out.is_empty() {
                    out = pts;
                }
                b.shape(k, SchGeom::Line(out), p, false);
            }
            Some(POLYLINE) => b.shape(k, SchGeom::Line(points(p)), p, false),
            Some(LINE) => b.shape(k, SchGeom::Line(vec![at(p), corner(p)]), p, false),
            Some(POLYGON) => b.shape(k, SchGeom::Poly(points(p)), p, true),
            Some(RECTANGLE) => {
                let (a, c) = (at(p), corner(p));
                b.shape(k, rect(a, c, [0.0, 0.0]), p, true);
            }
            Some(ROUND_RECT) => {
                let r = [len(p, "CORNERXRADIUS"), len(p, "CORNERYRADIUS")];
                b.shape(k, rect(at(p), corner(p), r), p, true);
            }
            Some(ELLIPSE) => {
                let r = len(p, "RADIUS");
                let r2 = if p.get("SECONDARYRADIUS").is_some() {
                    len(p, "SECONDARYRADIUS")
                } else {
                    r
                };
                let g = SchGeom::Arc {
                    c: at(p),
                    r: [r, r2],
                    start: 0.0,
                    end: 360.0,
                };
                b.shape(k, g, p, true);
            }
            Some(ARC) | Some(ELLIPTICAL_ARC) | Some(PIECHART) => {
                let r = len(p, "RADIUS");
                let r2 = if rec(p) == Some(ELLIPTICAL_ARC) {
                    len(p, "SECONDARYRADIUS")
                } else {
                    r
                };
                let start = p.float("STARTANGLE").unwrap_or(0.0);
                let mut end = p.float("ENDANGLE").unwrap_or(360.0);
                if end <= start {
                    end += 360.0;
                }
                let g = SchGeom::Arc {
                    c: at(p),
                    r: [r, r2],
                    start,
                    end,
                };
                b.shape(k, g, p, rec(p) == Some(PIECHART));
            }
            Some(WIRE) | Some(BUS) => {
                let pts = points(p);
                let wire = rec(p) == Some(WIRE);
                let wi = b.d.wires.len() as u32;
                b.d.shapes.push(SchShape {
                    geom: SchGeom::Line(pts.clone()),
                    width: if wire {
                        line_width(p).max(10.0)
                    } else {
                        line_width(p).max(30.0)
                    },
                    stroke: Some(color(p, "COLOR", 0x800000)),
                    fill: None,
                    dashed: false,
                    part: None,
                    wire: wire.then_some(wi),
                });
                if wire {
                    b.d.wires.push(pts);
                }
            }
            Some(BUS_ENTRY) => {
                let c = color(p, "COLOR", 0x800000);
                b.line(k, vec![at(p), corner(p)], line_width(p).max(10.0), c);
            }
            Some(JUNCTION) => {
                let c = at(p);
                b.d.junctions.push(c);
                b.d.shapes.push(SchShape {
                    geom: SchGeom::Arc {
                        c,
                        r: [20.0, 20.0],
                        start: 0.0,
                        end: 360.0,
                    },
                    width: 0.0,
                    stroke: None,
                    fill: Some(color(p, "COLOR", 0x800000)),
                    dashed: false,
                    part: None,
                    wire: None,
                });
            }
            Some(NO_ERC) => {
                let c = at(p);
                let red = [255, 0, 0];
                for (dx, dy) in [(1.0, 1.0), (1.0, -1.0)] {
                    b.line(
                        k,
                        vec![
                            [c[0] - 30.0 * dx, c[1] - 30.0 * dy],
                            [c[0] + 30.0 * dx, c[1] + 30.0 * dy],
                        ],
                        10.0,
                        red,
                    );
                }
            }
            Some(NET_LABEL) => {
                let t = p.str("TEXT");
                let ti = b.d.texts.len();
                b.label(k, p, t.clone());
                b.d.tags.push(NetTag {
                    at: vec![at(p)],
                    text: t,
                    kind: TagKind::Label,
                    graphic: (b.d.texts.len() > ti).then_some(ti as u32),
                });
            }
            Some(POWER_PORT) => power_port(&mut b, k, p),
            Some(PORT) => port(&mut b, k, p),
            Some(SHEET_SYMBOL) => {
                let a = at(p);
                let size = [len(p, "XSIZE"), len(p, "YSIZE")];
                let min = [a[0], a[1] - size[1]];
                let max = [a[0] + size[0], a[1]];
                sheet_boxes.insert(k, (min, max));
                b.shape(k, rect(min, max, [0.0, 0.0]), p, true);
            }
            Some(SHEET_ENTRY) => {
                if let Some(&(min, max)) = owner_of(p).and_then(|o| sheet_boxes.get(&o)) {
                    sheet_entry(&mut b, k, p, min, max);
                }
            }
            Some(SHEET_NAME) | Some(SHEET_FILE_NAME) => {
                if rec(p) == Some(SHEET_FILE_NAME) {
                    if let Some(&(min, max)) = owner_of(p).and_then(|o| sheet_boxes.get(&o)) {
                        b.d.links.push(trace_core::SheetLink {
                            min,
                            max,
                            file: p.str("TEXT"),
                        });
                    }
                }
                if !p.bool("ISHIDDEN") {
                    b.label(k, p, p.str("TEXT"));
                }
            }
            Some(TEXT_FRAME) | Some(NOTE) => text_frame(&mut b, k, p),
            Some(PARAMETER_SET) => {
                // A directive: a short stem and a small flag.
                let loc = at(p);
                let d = dir(p.int("ORIENTATION").unwrap_or(0));
                let c = color(p, "COLOR", 0xff);
                let tip = add(loc, d, 40.0);
                b.line(k, vec![loc, tip], 0.0, c);
                b.d.shapes.push(SchShape {
                    geom: SchGeom::Arc {
                        c: add(tip, d, 25.0),
                        r: [25.0, 25.0],
                        start: 0.0,
                        end: 360.0,
                    },
                    width: 0.0,
                    stroke: Some(c),
                    fill: None,
                    dashed: false,
                    part: None,
                    wire: None,
                });
                let name = p.str("NAME");
                if !name.eq_ignore_ascii_case("DIFFPAIR") && !p.bool("ISHIDDEN") {
                    b.text(k, add(tip, d, 60.0), name, 1, c, 0, 1);
                }
            }
            _ => {}
        }
    }
    // Part outlines, from what each part drew.
    let mut boxes = vec![[f64::MAX, f64::MAX, f64::MIN, f64::MIN]; parts.len()];
    let mut grow = |part: Option<u32>, q: Pt| {
        if let Some(i) = part {
            let bx = &mut boxes[i as usize];
            *bx = [
                bx[0].min(q[0]),
                bx[1].min(q[1]),
                bx[2].max(q[0]),
                bx[3].max(q[1]),
            ];
        }
    };
    for s in &b.d.shapes {
        match &s.geom {
            SchGeom::Line(v) | SchGeom::Poly(v) => v.iter().for_each(|&q| grow(s.part, q)),
            SchGeom::Rect { min, max, .. } => {
                grow(s.part, *min);
                grow(s.part, *max);
            }
            SchGeom::Arc { c, r, .. } => {
                grow(s.part, [c[0] - r[0], c[1] - r[1]]);
                grow(s.part, [c[0] + r[0], c[1] + r[1]]);
            }
        }
    }
    for p in &b.d.pins {
        grow(Some(p.part), p.at);
    }
    for (i, &(k, s)) in parts.iter().enumerate() {
        let bx = boxes[i];
        let (min, max) = if bx[0] <= bx[2] {
            ([bx[0], bx[1]], [bx[2], bx[3]])
        } else {
            let a = at(&recs[k]);
            (a, a)
        };
        b.d.parts.push(SchPart {
            designator: sheet.symbols[s].designator.clone(),
            symbol: s,
            min,
            max,
        });
    }
    sheet.net_labels.sort();
    sheet.net_labels.dedup();
    sheet.power_ports.sort();
    sheet.power_ports.dedup();
    b.d.connect();
    sheet.drawing = b.d;
    sheet
}

fn rect(a: Pt, c: Pt, r: [f64; 2]) -> SchGeom {
    SchGeom::Rect {
        min: [a[0].min(c[0]), a[1].min(c[1])],
        max: [a[0].max(c[0]), a[1].max(c[1])],
        r,
    }
}

/// The sheet border with its zone letters and numbers.
fn border(d: &mut Drawing, p: &Props) {
    let [w, h] = d.size;
    let m = if p.get("MARGINWIDTH").is_some() {
        len(p, "MARGINWIDTH")
    } else {
        200.0
    };
    let c = color(p, "COLOR", 0);
    let mut frame = |min: Pt, max: Pt| {
        d.shapes.push(SchShape {
            geom: SchGeom::Rect {
                min,
                max,
                r: [0.0, 0.0],
            },
            width: 0.0,
            stroke: Some(c),
            fill: None,
            dashed: false,
            part: None,
            wire: None,
        });
    };
    frame([0.0, 0.0], [w, h]);
    frame([m, m], [w - m, h - m]);
    let xz = p
        .int("XZONES")
        .unwrap_or((w / 1950.0).round() as i64)
        .max(1);
    let yz = p.int("YZONES").unwrap_or(4).max(1);
    let size = (m * 0.6).min(120.0);
    let mut t = |at: Pt, text: String| {
        d.texts.push(SchText {
            at,
            text,
            size,
            color: c,
            rot: 0,
            just: 4,
            bold: false,
            italic: false,
            wrap: None,
            part: None,
        });
    };
    let zw = (w - 2.0 * m) / xz as f64;
    for i in 0..xz {
        let x = m + zw * (i as f64 + 0.5);
        t([x, m / 2.0], (i + 1).to_string());
        t([x, h - m / 2.0], (i + 1).to_string());
        if i > 0 {
            let x = m + zw * i as f64;
            d.shapes.push(SchShape {
                geom: SchGeom::Line(vec![[x, 0.0], [x, m]]),
                width: 0.0,
                stroke: Some(c),
                fill: None,
                dashed: false,
                part: None,
                wire: None,
            });
            d.shapes.push(SchShape {
                geom: SchGeom::Line(vec![[x, h - m], [x, h]]),
                width: 0.0,
                stroke: Some(c),
                fill: None,
                dashed: false,
                part: None,
                wire: None,
            });
        }
    }
    let zh = (h - 2.0 * m) / yz as f64;
    for i in 0..yz {
        // Letters run from the top down.
        let y = h - m - zh * (i as f64 + 0.5);
        let letter = ((b'A' + (i as u8 % 26)) as char).to_string();
        t([m / 2.0, y], letter.clone());
        t([w - m / 2.0, y], letter);
        if i > 0 {
            let y = h - m - zh * i as f64;
            for (x0, x1) in [(0.0, m), (w - m, w)] {
                d.shapes.push(SchShape {
                    geom: SchGeom::Line(vec![[x0, y], [x1, y]]),
                    width: 0.0,
                    stroke: Some(c),
                    fill: None,
                    dashed: false,
                    part: None,
                    wire: None,
                });
            }
        }
    }
}

/// Removes Altium's overbar markers: `R\E\S\E\T\` is RESET with a bar.
fn plain(s: &str) -> String {
    s.replace('\\', "")
}

fn pin(b: &mut Builder, k: usize, p: &Props) {
    let Some(part) = b.part_of[k] else { return };
    let flags = p.int("PINCONGLOMERATE").unwrap_or(0);
    if flags & 0x04 != 0 {
        return; // hidden
    }
    let orient = flags & 3;
    let base = at(p);
    let length = len(p, "PINLENGTH");
    let d = dir(orient);
    let tip = add(base, d, length);
    let c = color(p, "COLOR", 0);
    b.line(k, vec![base, tip], 10.0, c);
    b.d.pins.push(SchPin {
        at: tip,
        base,
        part,
        name: p.str("NAME"),
        number: p.str("DESIGNATOR"),
    });
    // Edge symbols: a bubble outside (negated), a wedge inside (clock).
    if p.int("SYMBOL_OUTEREDGE") == Some(1) {
        let r = 20.0;
        b.d.shapes.push(SchShape {
            geom: SchGeom::Arc {
                c: add(base, d, r),
                r: [r, r],
                start: 0.0,
                end: 360.0,
            },
            width: 0.0,
            stroke: Some(c),
            fill: None,
            dashed: false,
            part: Some(part),
            wire: None,
        });
    }
    if p.int("SYMBOL_INNEREDGE") == Some(3) {
        let n = [-d[1], d[0]];
        let a = add(base, n, 30.0);
        let e = add(base, n, -30.0);
        let m = add(base, d, -40.0);
        b.line(k, vec![a, m, e], 0.0, c);
    }
    let font = 1;
    let text_c = [0, 0, 0];
    // Horizontal pins read left to right; vertical pins bottom to top.
    let rot = if orient % 2 == 1 { 1 } else { 0 };
    if flags & 0x08 != 0 {
        let name = plain(&p.str("NAME"));
        let inside = add(base, d, -50.0);
        // Name sits inside the body, centred on the pin line.
        let just = match orient {
            0 | 1 => 5, // body is on the left/below: right-aligned
            _ => 3,
        };
        b.text(k, inside, name, font, text_c, rot, just);
    }
    if flags & 0x10 != 0 {
        let num = p.str("DESIGNATOR");
        let mid = add(base, d, length / 2.0);
        let n = [-d[1], d[0]];
        let off = if orient % 2 == 1 { -1.0 } else { 1.0 };
        b.text(k, add(mid, n, 8.0 * off), num, font, text_c, rot, 1);
    }
}

fn power_port(b: &mut Builder, k: usize, p: &Props) {
    let loc = at(p);
    let orient = p.int("ORIENTATION").unwrap_or(0);
    let d = dir(orient);
    let n = [-d[1], d[0]];
    let style = p.int("STYLE").unwrap_or(0);
    let c = color(p, "COLOR", 0x800000);
    let text = p.str("TEXT");
    b.d.tags.push(NetTag {
        at: vec![loc],
        text: text.clone(),
        kind: TagKind::Power,
        graphic: None,
    });
    // Glyphs in local (across, along) mil, after KiCad's drawings.
    let q = |u: f64, v: f64| -> Pt { [loc[0] + n[0] * u + d[0] * v, loc[1] + n[1] * u + d[1] * v] };
    let mut lines: Vec<Vec<Pt>> = Vec::new();
    let mut circle = None;
    let reach = match style {
        0 | 1 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 50.0)]);
            if style == 0 {
                circle = Some((q(0.0, 75.0), 25.0));
            } else {
                lines.push(vec![
                    q(-25.0, 50.0),
                    q(25.0, 50.0),
                    q(0.0, 100.0),
                    q(-25.0, 50.0),
                ]);
            }
            150.0
        }
        3 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 72.0)]);
            lines.push(vec![q(30.0, 50.0), q(0.0, 72.0), q(-30.0, 100.0)]);
            150.0
        }
        4 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 100.0)]);
            lines.push(vec![q(-100.0, 100.0), q(100.0, 100.0)]);
            lines.push(vec![q(-70.0, 130.0), q(70.0, 130.0)]);
            lines.push(vec![q(-40.0, 160.0), q(40.0, 160.0)]);
            lines.push(vec![q(-10.0, 190.0), q(10.0, 190.0)]);
            250.0
        }
        5 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 100.0)]);
            lines.push(vec![
                q(-100.0, 100.0),
                q(100.0, 100.0),
                q(0.0, 200.0),
                q(-100.0, 100.0),
            ]);
            250.0
        }
        6 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 100.0)]);
            lines.push(vec![
                q(-150.0, 200.0),
                q(-100.0, 100.0),
                q(100.0, 100.0),
                q(50.0, 200.0),
            ]);
            lines.push(vec![q(0.0, 100.0), q(-50.0, 200.0)]);
            250.0
        }
        7 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 100.0)]);
            lines.push(vec![q(-25.0, 50.0), q(0.0, 100.0), q(25.0, 50.0)]);
            150.0
        }
        8 | 9 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 160.0)]);
            lines.push(vec![q(-100.0, 160.0), q(100.0, 160.0)]);
            lines.push(vec![q(-60.0, 200.0), q(60.0, 200.0)]);
            lines.push(vec![q(-20.0, 240.0), q(20.0, 240.0)]);
            if style == 9 {
                circle = Some((q(0.0, 160.0), 120.0));
            }
            300.0
        }
        10 => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 200.0)]);
            lines.push(vec![q(-100.0, 200.0), q(100.0, 200.0)]);
            250.0
        }
        _ => {
            lines.push(vec![q(0.0, 0.0), q(0.0, 100.0)]);
            lines.push(vec![q(-50.0, 100.0), q(50.0, 100.0)]);
            150.0
        }
    };
    for l in lines {
        b.line(k, l, 10.0, c);
    }
    if let Some((cc, r)) = circle {
        b.d.shapes.push(SchShape {
            geom: SchGeom::Arc {
                c: cc,
                r: [r, r],
                start: 0.0,
                end: 360.0,
            },
            width: 5.0,
            stroke: Some(c),
            fill: None,
            dashed: false,
            part: None,
            wire: None,
        });
    }
    if p.bool("SHOWNETNAME") || p.get("SHOWNETNAME").is_none() {
        let just = match orient.rem_euclid(4) {
            0 => 3,
            1 => 1,
            2 => 5,
            _ => 7,
        };
        let at = add(loc, d, reach - 50.0);
        b.text(k, at, text, p.int("FONTID").unwrap_or(1), c, 0, just);
    }
}

fn port(b: &mut Builder, k: usize, p: &Props) {
    let loc = at(p);
    let name = p.str("NAME");
    let w = len(p, "WIDTH");
    let h = if p.get("HEIGHT").is_some() {
        len(p, "HEIGHT")
    } else {
        100.0
    } / 2.0;
    let style = p.int("STYLE").unwrap_or(0);
    let vertical = style >= 4;
    // Work along the port's axis: s from 0 to w, t across.
    let q = |s: f64, t: f64| -> Pt {
        if vertical {
            [loc[0] + t, loc[1] + s]
        } else {
            [loc[0] + s, loc[1] + t]
        }
    };
    let (start_pt, end_pt) = match style % 4 {
        1 => (true, false),
        2 => (false, true),
        3 => (true, true),
        _ => (false, false),
    };
    let tip = h.min(w / 2.0);
    let mut pts = Vec::new();
    if start_pt {
        pts.push(q(0.0, 0.0));
        pts.push(q(tip, h));
    } else {
        pts.push(q(0.0, -h));
        pts.push(q(0.0, h));
    }
    if end_pt {
        pts.push(q(w - tip, h));
        pts.push(q(w, 0.0));
        pts.push(q(w - tip, -h));
    } else {
        pts.push(q(w, h));
        pts.push(q(w, -h));
    }
    if start_pt {
        pts.push(q(tip, -h));
    }
    b.d.shapes.push(SchShape {
        geom: SchGeom::Poly(pts),
        width: 0.0,
        stroke: Some(color(p, "COLOR", 0x80)),
        fill: Some(color(p, "AREACOLOR", 0x80ffff)),
        dashed: false,
        part: None,
        wire: None,
    });
    b.d.tags.push(NetTag {
        at: vec![q(0.0, 0.0), q(w, 0.0)],
        text: name.clone(),
        kind: TagKind::Port,
        graphic: None,
    });
    let (s, just) = match p.int("ALIGNMENT").unwrap_or(0) {
        1 => (w - tip, 5),
        2 => (tip, 3),
        _ => (w / 2.0, 4),
    };
    let tc = color(p, "TEXTCOLOR", 0);
    let font = p.int("TEXTFONTID").unwrap_or(1);
    b.text(
        k,
        q(s, 0.0),
        name,
        font,
        tc,
        if vertical { 1 } else { 0 },
        just,
    );
}

fn sheet_entry(b: &mut Builder, k: usize, p: &Props, min: Pt, max: Pt) {
    let dist = p.int("DISTANCEFROMTOP").unwrap_or(0) as f64 * 100.0
        + p.int("DISTANCEFROMTOP_FRAC1").unwrap_or(0) as f64 / 10000.0;
    let side = p.int("SIDE").unwrap_or(0);
    // Edge point and the inward direction.
    let (edge, inward) = match side {
        1 => ([max[0], max[1] - dist], [-1.0, 0.0]),
        2 => ([min[0] + dist, max[1]], [0.0, -1.0]),
        3 => ([min[0] + dist, min[1]], [0.0, 1.0]),
        _ => ([min[0], max[1] - dist], [1.0, 0.0]),
    };
    let name = p.str("NAME");
    let n = [-inward[1], inward[0]];
    let (l, h) = (100.0, 40.0);
    let pts = vec![
        add(edge, n, h),
        add(add(edge, inward, l - h), n, h),
        add(edge, inward, l),
        add(add(edge, inward, l - h), n, -h),
        add(edge, n, -h),
    ];
    b.d.shapes.push(SchShape {
        geom: SchGeom::Poly(pts),
        width: 0.0,
        stroke: Some(color(p, "COLOR", 0x80)),
        fill: Some(color(p, "AREACOLOR", 0x80ffff)),
        dashed: false,
        part: None,
        wire: None,
    });
    b.d.tags.push(NetTag {
        at: vec![edge],
        text: name.clone(),
        kind: TagKind::SheetEntry,
        graphic: None,
    });
    let (rot, just) = match side {
        1 => (0, 5),
        2 => (1, 5),
        3 => (1, 3),
        _ => (0, 3),
    };
    let font = p.int("TEXTFONTID").unwrap_or(1);
    let tc = color(p, "TEXTCOLOR", 0x80);
    b.text(k, add(edge, inward, l + 30.0), name, font, tc, rot, just);
}

fn text_frame(b: &mut Builder, k: usize, p: &Props) {
    let (a, c) = (at(p), corner(p));
    let (min, max) = (
        [a[0].min(c[0]), a[1].min(c[1])],
        [a[0].max(c[0]), a[1].max(c[1])],
    );
    let fill = p.bool("ISSOLID").then(|| color(p, "AREACOLOR", 0xffffff));
    let stroke = p.bool("SHOWBORDER").then(|| color(p, "COLOR", 0));
    if fill.is_some() || stroke.is_some() {
        b.d.shapes.push(SchShape {
            geom: SchGeom::Rect {
                min,
                max,
                r: [0.0, 0.0],
            },
            width: line_width(p),
            stroke,
            fill,
            dashed: false,
            part: b.part_of[k],
            wire: None,
        });
    }
    let margin = len(p, "TEXTMARGIN");
    let text = p.str("TEXT").replace("~1", "\n");
    let (x, just) = match p.int("ALIGNMENT").unwrap_or(1) {
        2 => ((min[0] + max[0]) / 2.0, 7),
        3 => (max[0] - margin, 8),
        _ => (min[0] + margin, 6),
    };
    let f = b.font(p.int("FONTID").unwrap_or(1));
    if text.is_empty() {
        return;
    }
    b.d.texts.push(SchText {
        at: [x, max[1] - margin],
        text,
        size: f.size * FONT_SCALE,
        color: color(p, "TEXTCOLOR", 0),
        rot: 0,
        just,
        bold: f.bold,
        italic: f.italic,
        wrap: p
            .bool("WORDWRAP")
            .then_some((max[0] - min[0] - 2.0 * margin).max(10.0)),
        part: b.part_of[k],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> SchRecord {
        SchRecord::Text(Props::parse(s))
    }

    /// A two-part component (like a dual op-amp): each part has its own
    /// box and pins, and both share the designator and footprint.
    #[test]
    fn multi_part_library_components_give_one_symbol_per_part() {
        let recs = vec![
            text("|RECORD=1|LibReference=DUAL|ComponentDescription=Dual buffer|PartCount=3|CurrentPartId=1"),
            text("|RECORD=14|OwnerPartId=1|Location.X=0|Location.Y=0|Corner.X=40|Corner.Y=30|IsSolid=T|AreaColor=11599871"),
            text("|RECORD=2|OwnerPartId=1|Location.X=40|Location.Y=10|PinLength=20|PinConglomerate=0|Designator=1|Name=OUT"),
            text("|RECORD=2|OwnerPartId=1|Location.X=0|Location.Y=10|PinLength=20|PinConglomerate=2|Designator=2|Name=IN"),
            text("|RECORD=14|OwnerPartId=2|Location.X=0|Location.Y=0|Corner.X=50|Corner.Y=30"),
            text("|RECORD=2|OwnerPartId=2|Location.X=50|Location.Y=10|PinLength=20|PinConglomerate=0|Designator=7|Name=OUT"),
            text("|RECORD=34|OwnerPartId=-1|Location.X=0|Location.Y=40|Text=U?|Name=Designator"),
            text("|RECORD=41|OwnerPartId=-1|Location.X=0|Location.Y=-10|Text=10k|Name=Value"),
            text("|RECORD=44"),
            text("|RECORD=45|OwnerIndex=8|ModelName=SOIC8|ModelType=PCBLIB|IsCurrent=T"),
        ];
        let mut fonts = Props::default();
        fonts.set("RECORD", SHEET.to_string());
        let list = lib_symbols(&recs, &fonts);
        assert_eq!(list.len(), 2);
        let (a, b) = (&list[0], &list[1]);
        assert_eq!((a.part, a.parts, b.part), (1, 2, 2));
        assert_eq!(a.title(), "DUAL A");
        assert_eq!(a.symbol.footprint, "SOIC8");
        assert_eq!(a.symbol.designator, "U?");
        assert_eq!(a.symbol.value.as_deref(), Some("10k"));
        let pins = |s: &LibSymbol| {
            s.drawing
                .pins
                .iter()
                .map(|p| p.number.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(pins(a), ["1", "2"]);
        assert_eq!(pins(b), ["7"]);
        // The pin ends: 1 sticks out right from x=400 mil, 2 left from 0.
        assert_eq!(a.drawing.pins[0].at, [600.0, 100.0]);
        assert_eq!(a.drawing.pins[1].at, [-200.0, 100.0]);
        // The designator is drawn with the part letter.
        assert!(
            b.drawing.texts.iter().any(|t| t.text == "U?B"),
            "{:?}",
            b.drawing.texts
        );

        // Placing part B as U5 draws U5B, keeps the footprint and pins.
        let mut sheet = Sheet::default();
        let n = sheet.place_symbol(b, [1000.0, 2000.0], "U5").unwrap();
        let d = &sheet.drawing;
        assert_eq!(d.parts[n].designator, "U5");
        assert!(d.texts.iter().any(|t| t.text == "U5B"));
        assert_eq!(d.pins[0].at, [1700.0, 2100.0]);
        assert_eq!(sheet.symbols[d.parts[n].symbol].footprint, "SOIC8");
        assert_eq!(sheet.symbols[d.parts[n].symbol].library_ref, "DUAL");
    }
}
