//! ODB++ reader. The job's matrix names every layer and its type, so layers
//! are recognised exactly; features are converted into the same layer data
//! the Gerber path uses, and nets, component pins and holes come from the
//! job's EDA data and drill layers.

use crate::build::{Layer, LayerData};
use crate::detect::{CuPos, Role, Side};
use crate::excellon::{Drill, Hole};
use crate::geom::{self, Pt};
use crate::gerber::{Aperture, Gerber, Item, Obj, ObjAttrs, Prim, PrimKind, Shape, Xform};
use crate::{Error, Import, Result};
use std::collections::HashMap;

/// Files of an ODB++ job, keyed by lower-case path relative to the job root.
pub struct Job {
    files: HashMap<String, Vec<u8>>,
}

impl Job {
    /// Builds a job from archive or folder entries; `None` when there is no
    /// `matrix/matrix` file.
    pub fn from_entries(entries: Vec<(String, Vec<u8>)>) -> Option<Job> {
        let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
        let root = entries.iter().find_map(|(n, _)| {
            let n = norm(n);
            let n = n.strip_suffix(".z").unwrap_or(&n).to_string();
            n.strip_suffix("matrix/matrix").map(|r| r.to_string())
        })?;
        let files = entries
            .into_iter()
            .filter_map(|(n, d)| norm(&n).strip_prefix(&root).map(|r| (r.to_string(), d)))
            .collect();
        Some(Job { files })
    }

    fn get(&self, path: &str) -> Option<Vec<u8>> {
        let p = path.to_ascii_lowercase();
        if let Some(d) = self.files.get(&p) {
            return Some(d.clone());
        }
        for ext in [".z", ".gz"] {
            if let Some(d) = self.files.get(&format!("{p}{ext}")) {
                return decompress(d);
            }
        }
        None
    }

    fn text(&self, path: &str) -> Option<String> {
        self.get(path)
            .map(|d| String::from_utf8_lossy(&d).into_owned())
    }

    fn has(&self, path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        self.files.contains_key(&p) || self.files.contains_key(&format!("{p}.z"))
    }
}

fn decompress(d: &[u8]) -> Option<Vec<u8>> {
    if d.starts_with(&[0x1f, 0x9d]) {
        return unix_uncompress(d);
    }
    if d.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        use std::io::Read;
        flate2::read::GzDecoder::new(d).read_to_end(&mut out).ok()?;
        return Some(out);
    }
    Some(d.to_vec())
}

/// Decoder for Unix `compress` (.Z, LZW) data, used by some ODB++ writers.
pub fn unix_uncompress(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 3 || d[0] != 0x1f || d[1] != 0x9d {
        return None;
    }
    let max_bits = (d[2] & 0x1f) as u32;
    let block = d[2] & 0x80 != 0;
    if !(9..=16).contains(&max_bits) {
        return None;
    }
    let data = &d[3..];
    let mut prefix: Vec<u16> = vec![0; 1 << max_bits];
    let mut suffix: Vec<u8> = (0..=255u16)
        .map(|c| c as u8)
        .chain(std::iter::repeat_n(0, (1 << max_bits) - 256))
        .collect();
    let first_free: u32 = if block { 257 } else { 256 };
    let mut free = first_free;
    let mut bits: u32 = 9;
    let mut out = Vec::with_capacity(d.len() * 4);
    let mut bitpos: usize = 0;
    // Codes are read in groups of `bits` bytes; a width change or a clear
    // skips to the end of the current group.
    let mut group_start: usize = 0;
    let mut old: Option<u32> = None;
    let mut fin: u8 = 0;
    let mut stack: Vec<u8> = Vec::new();
    loop {
        if bitpos + bits as usize > data.len() * 8 {
            break;
        }
        let byte = bitpos / 8;
        let mut v: u32 = 0;
        for k in 0..3 {
            if let Some(b) = data.get(byte + k) {
                v |= (*b as u32) << (8 * k);
            }
        }
        let code = (v >> (bitpos % 8)) & ((1 << bits) - 1);
        bitpos += bits as usize;
        if block && code == 256 {
            let group = bits as usize * 8;
            let used = bitpos - group_start;
            bitpos = group_start + used.div_ceil(group) * group;
            group_start = bitpos;
            free = first_free;
            bits = 9;
            old = None;
            continue;
        }
        let Some(prev) = old else {
            if code > 255 {
                return None;
            }
            fin = code as u8;
            out.push(fin);
            old = Some(code);
            continue;
        };
        let mut c = code;
        stack.clear();
        if c >= free {
            if c > free {
                return None;
            }
            stack.push(fin);
            c = prev;
        }
        while c >= 256 {
            stack.push(suffix[c as usize]);
            c = prefix[c as usize] as u32;
        }
        fin = c as u8;
        stack.push(fin);
        out.extend(stack.iter().rev());
        if free < (1 << max_bits) {
            prefix[free as usize] = prev as u16;
            suffix[free as usize] = fin;
            free += 1;
            if free >= (1 << bits) && bits < max_bits {
                let group = bits as usize * 8;
                let used = bitpos - group_start;
                bitpos = group_start + used.div_ceil(group) * group;
                group_start = bitpos;
                bits += 1;
            }
        }
        old = Some(code);
    }
    Some(out)
}

#[derive(Debug, Default, Clone)]
struct MatrixLayer {
    row: u32,
    context: String,
    kind: String,
    name: String,
    negative: bool,
    start: String,
    end: String,
}

fn parse_matrix(t: &str) -> (Vec<String>, Vec<MatrixLayer>) {
    let mut steps = Vec::new();
    let mut layers = Vec::new();
    let mut block: Option<(String, HashMap<String, String>)> = None;
    for line in t.lines() {
        let l = line.trim();
        if let Some(name) = l.strip_suffix('{') {
            block = Some((name.trim().to_ascii_uppercase(), HashMap::new()));
        } else if l == "}" {
            if let Some((kind, kv)) = block.take() {
                let get = |k: &str| kv.get(k).cloned().unwrap_or_default();
                if kind == "STEP" {
                    steps.push(get("NAME"));
                } else if kind == "LAYER" {
                    layers.push(MatrixLayer {
                        row: get("ROW").parse().unwrap_or(0),
                        context: get("CONTEXT").to_ascii_uppercase(),
                        kind: get("TYPE").to_ascii_uppercase(),
                        name: get("NAME"),
                        negative: get("POLARITY").eq_ignore_ascii_case("NEGATIVE"),
                        start: get("START_NAME"),
                        end: get("END_NAME"),
                    });
                }
            }
        } else if let (Some((_, kv)), Some((k, v))) = (block.as_mut(), l.split_once('=')) {
            kv.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
        }
    }
    layers.sort_by_key(|l| l.row);
    (steps, layers)
}

/// One feature record of a features file.
enum Feature {
    Line {
        a: Pt,
        b: Pt,
        sym: usize,
        dark: bool,
    },
    Pad {
        at: Pt,
        sym: usize,
        resize: f64,
        dark: bool,
        xf: Xform,
    },
    Arc {
        a: Pt,
        b: Pt,
        c: Pt,
        sym: usize,
        dark: bool,
        cw: bool,
    },
    Surface {
        dark: bool,
        contours: Vec<(bool, Vec<Pt>)>,
    },
    /// Text, barcodes and anything else kept only for numbering.
    Other,
}

struct Features {
    /// Symbol names, by index.
    symbols: Vec<String>,
    features: Vec<Feature>,
    /// Feature index -> attribute text (after `;`), for drill types.
    attrs: Vec<String>,
    attr_names: HashMap<usize, String>,
    /// mm per coordinate unit, and mm per symbol unit.
    unit: f64,
    sym_unit: f64,
}

fn parse_features(t: &str) -> Features {
    let mut f = Features {
        symbols: Vec::new(),
        features: Vec::new(),
        attrs: Vec::new(),
        attr_names: HashMap::new(),
        unit: 25.4,
        sym_unit: 0.0254,
    };
    let num = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    let mut surface: Option<(bool, Vec<(bool, Vec<Pt>)>, String)> = None;
    for line in t.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (body, attr) = match line.split_once(';') {
            Some((b, a)) => (b.trim(), a.trim().to_string()),
            None => (line, String::new()),
        };
        if let Some(u) = body
            .strip_prefix("UNITS=")
            .or_else(|| body.strip_prefix("U "))
        {
            if u.trim().eq_ignore_ascii_case("MM") {
                f.unit = 1.0;
                f.sym_unit = 0.001;
            }
            continue;
        }
        let tok: Vec<&str> = body.split_whitespace().collect();
        let u = f.unit;
        let p = |i: usize| -> Pt {
            [
                num(tok.get(i).copied().unwrap_or("0")) * u,
                num(tok.get(i + 1).copied().unwrap_or("0")) * u,
            ]
        };
        if surface.is_some() && tok.first() == Some(&"SE") {
            let (dark, contours, a) = surface.take().unwrap();
            f.features.push(Feature::Surface { dark, contours });
            f.attrs.push(a);
            continue;
        }
        if let Some((_, contours, _)) = &mut surface {
            match tok.first().copied() {
                Some("OB") => contours.push((tok.get(3).is_none_or(|s| *s != "H"), vec![p(1)])),
                Some("OS") => {
                    if let Some(c) = contours.last_mut() {
                        c.1.push(p(1));
                    }
                }
                Some("OC") => {
                    if let Some(c) = contours.last_mut() {
                        let from = *c.1.last().unwrap();
                        let (end, centre) = (p(1), p(3));
                        let cw = tok.get(5).is_some_and(|s| s.eq_ignore_ascii_case("Y"));
                        let full = geom::dist(from, end) < 1e-9;
                        c.1.extend(
                            geom::arc_points(centre, from, end, cw, full)
                                .into_iter()
                                .skip(1),
                        );
                    }
                }
                _ => {}
            }
            continue;
        }
        match tok.first().copied() {
            Some(s) if s.starts_with('$') => f.symbols.push(tok.get(1).unwrap_or(&"").to_string()),
            Some(s) if s.starts_with('@') => {
                if let Ok(i) = s[1..].parse::<usize>() {
                    f.attr_names
                        .insert(i, tok.get(1).unwrap_or(&"").to_string());
                }
            }
            Some("L") if tok.len() >= 7 => {
                f.features.push(Feature::Line {
                    a: p(1),
                    b: p(3),
                    sym: tok[5].parse().unwrap_or(0),
                    dark: tok[6] != "N",
                });
                f.attrs.push(attr);
            }
            Some("A") if tok.len() >= 10 => {
                f.features.push(Feature::Arc {
                    a: p(1),
                    b: p(3),
                    c: p(5),
                    sym: tok[7].parse().unwrap_or(0),
                    dark: tok[8] != "N",
                    cw: tok.get(10).is_some_and(|s| s.eq_ignore_ascii_case("Y")),
                });
                f.attrs.push(attr);
            }
            Some("P") if tok.len() >= 6 => {
                // P x y apt pol dcode orient [angle]; apt may be "-1 sym resize".
                let (sym, resize, rest) = if tok[3] == "-1" {
                    (
                        tok.get(4).and_then(|s| s.parse().ok()).unwrap_or(0),
                        num(tok.get(5).copied().unwrap_or("0")),
                        6,
                    )
                } else {
                    (tok[3].parse().unwrap_or(0), 0.0, 4)
                };
                let dark = tok.get(rest).is_none_or(|s| *s != "N");
                let orient: u32 = tok.get(rest + 2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let angle = tok.get(rest + 3).map(|s| num(s)).unwrap_or(0.0);
                let (mirror, cw_deg) = match orient {
                    0..=3 => (false, orient as f64 * 90.0),
                    4..=7 => (true, (orient - 4) as f64 * 90.0),
                    8 => (false, angle),
                    _ => (true, angle),
                };
                f.features.push(Feature::Pad {
                    at: p(1),
                    sym,
                    resize,
                    dark,
                    // ODB++ turns clockwise; Gerber counter-clockwise.
                    xf: Xform {
                        mirror_x: mirror,
                        mirror_y: false,
                        rot: -cw_deg,
                        scale: 1.0,
                    },
                });
                f.attrs.push(attr);
            }
            Some("S") => surface = Some((tok.get(1).is_none_or(|s| *s != "N"), Vec::new(), attr)),
            Some("T") | Some("B") => {
                f.features.push(Feature::Other);
                f.attrs.push(attr);
            }
            _ => {}
        }
    }
    f
}

/// Numbers in a symbol name such as `rect20x30xr5`.
fn nums(s: &str) -> Vec<f64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

struct Symbols<'a> {
    job: &'a Job,
    cache: HashMap<String, Shape>,
}

impl Symbols<'_> {
    /// Converts a standard or user symbol into an aperture shape in mm.
    fn shape(&mut self, name: &str, sym_unit: f64) -> Shape {
        let key = format!("{name}@{sym_unit}");
        if let Some(s) = self.cache.get(&key) {
            return s.clone();
        }
        let s = self.build(name, sym_unit);
        self.cache.insert(key, s.clone());
        s
    }

    fn build(&mut self, name: &str, k: f64) -> Shape {
        let n: Vec<f64> = nums(name).into_iter().map(|v| v * k).collect();
        let v = |i: usize| n.get(i).copied().unwrap_or(0.0);
        let lname = name.to_ascii_lowercase();
        let o = [0.0, 0.0];
        let outline = |pts: Vec<Pt>| {
            Shape::Macro(vec![Prim {
                on: true,
                kind: PrimKind::Outline(pts),
            }])
        };
        let standard = |prefix: &str| {
            lname.starts_with(prefix)
                && lname[prefix.len()..].starts_with(|c: char| c.is_ascii_digit() || c == '.')
        };
        if standard("r") {
            return Shape::Circle(v(0));
        }
        if standard("s") {
            return Shape::Rect(v(0), v(0));
        }
        if standard("rect") {
            return Shape::Rect(v(0), v(1));
        }
        if standard("oval") || standard("el") {
            return Shape::Obround(v(0), v(1));
        }
        if standard("di") {
            return outline(vec![
                [v(0) / 2.0, 0.0],
                [0.0, v(1) / 2.0],
                [-v(0) / 2.0, 0.0],
                [0.0, -v(1) / 2.0],
            ]);
        }
        if standard("octagon") {
            let (w, h, r) = (v(0) / 2.0, v(1) / 2.0, v(2));
            return outline(vec![
                [w, h - r],
                [w - r, h],
                [-w + r, h],
                [-w, h - r],
                [-w, -h + r],
                [-w + r, -h],
                [w - r, -h],
                [w, -h + r],
            ]);
        }
        if standard("hex_l") {
            let (w, h, r) = (v(0) / 2.0, v(1) / 2.0, v(2));
            return outline(vec![
                [w, 0.0],
                [w - r, h],
                [-w + r, h],
                [-w, 0.0],
                [-w + r, -h],
                [w - r, -h],
            ]);
        }
        if standard("hex_s") {
            let (w, h, r) = (v(0) / 2.0, v(1) / 2.0, v(2));
            return outline(vec![
                [0.0, h],
                [-w, h - r],
                [-w, -h + r],
                [0.0, -h],
                [w, -h + r],
                [w, h - r],
            ]);
        }
        if standard("tri") {
            return outline(vec![
                [-v(0) / 2.0, -v(1) / 2.0],
                [v(0) / 2.0, -v(1) / 2.0],
                [0.0, v(1) / 2.0],
            ]);
        }
        if standard("donut_r") {
            return Shape::Macro(vec![
                Prim {
                    on: true,
                    kind: PrimKind::Circle { c: o, d: v(0) },
                },
                Prim {
                    on: false,
                    kind: PrimKind::Circle { c: o, d: v(1) },
                },
            ]);
        }
        if standard("donut_s") || standard("donut_sr") || standard("donut_rc") {
            return Shape::Macro(vec![
                Prim {
                    on: true,
                    kind: PrimKind::Outline(geom::rect(o, v(0), v(0))),
                },
                Prim {
                    on: false,
                    kind: PrimKind::Outline(geom::rect(o, v(1), v(1))),
                },
            ]);
        }
        if standard("thr")
            || standard("ths")
            || standard("s_th")
            || standard("sr_ths")
            || standard("rc_th")
            || standard("s_tho")
        {
            let (od, id, gap) = (v(0), v(1), n.get(4).copied().unwrap_or(0.0));
            let angle = nums(name).get(2).copied().unwrap_or(45.0);
            let prims = geom::thermal_quarters(o, od, id, gap)
                .into_iter()
                .map(|q| Prim {
                    on: true,
                    kind: PrimKind::Outline(
                        q.into_iter()
                            .map(|p| geom::rotate(p, angle - 0.0))
                            .collect(),
                    ),
                })
                .collect();
            return Shape::Macro(prims);
        }
        if standard("moire") || standard("hole") {
            return Shape::Circle(v(0));
        }
        if lname.starts_with("null") {
            return Shape::Circle(0.0);
        }
        self.user(&lname)
    }

    /// A user-defined symbol from `symbols/<name>/features`.
    fn user(&mut self, name: &str) -> Shape {
        let Some(t) = self.job.text(&format!("symbols/{name}/features")) else {
            return Shape::Circle(0.0);
        };
        let f = parse_features(&t);
        let mut prims = Vec::new();
        let xf = Xform::default();
        for feat in &f.features {
            match feat {
                Feature::Surface { dark, contours } => {
                    for (island, c) in contours {
                        prims.push(Prim {
                            on: *dark == *island,
                            kind: PrimKind::Outline(c.clone()),
                        });
                    }
                }
                Feature::Pad {
                    at,
                    sym,
                    dark,
                    xf: pxf,
                    ..
                } => {
                    let s = f.symbols.get(*sym).cloned().unwrap_or_default();
                    let shape = if s.eq_ignore_ascii_case(name) {
                        Shape::Circle(0.0)
                    } else {
                        self.shape(&s, f.sym_unit)
                    };
                    let ap = Aperture {
                        dcode: 0,
                        shape,
                        hole: 0.0,
                        function: None,
                    };
                    for (on, ring) in geom::flash_shapes(&ap, *at, pxf) {
                        prims.push(Prim {
                            on: on == *dark,
                            kind: PrimKind::Outline(ring),
                        });
                    }
                }
                Feature::Line { a, b, sym, dark } => {
                    let s = f.symbols.get(*sym).cloned().unwrap_or_default();
                    let ap = Aperture {
                        dcode: 0,
                        shape: self.shape(&s, f.sym_unit),
                        hole: 0.0,
                        function: None,
                    };
                    prims.push(Prim {
                        on: *dark,
                        kind: PrimKind::Outline(geom::line_shape(&ap, *a, *b)),
                    });
                }
                Feature::Arc {
                    a,
                    b,
                    c,
                    sym,
                    dark,
                    cw,
                } => {
                    let s = f.symbols.get(*sym).cloned().unwrap_or_default();
                    let ap = Aperture {
                        dcode: 0,
                        shape: self.shape(&s, f.sym_unit),
                        hole: 0.0,
                        function: None,
                    };
                    let pts = geom::arc_points(*c, *a, *b, *cw, geom::dist(*a, *b) < 1e-9);
                    for ring in geom::stroke_path(&pts, geom::aperture_width(&ap)) {
                        prims.push(Prim {
                            on: *dark,
                            kind: PrimKind::Outline(ring),
                        });
                    }
                }
                Feature::Other => {}
            }
        }
        let _ = xf;
        if prims.is_empty() {
            Shape::Circle(0.0)
        } else {
            Shape::Macro(prims)
        }
    }
}

/// Net and component pin for each feature of each layer, from `eda/data`.
#[derive(Default)]
struct Netlist {
    /// layer name -> feature index -> attributes
    by_layer: HashMap<String, HashMap<usize, ObjAttrs>>,
}

fn parse_eda(t: &str, comps: &HashMap<(bool, usize), (String, Vec<String>)>) -> Netlist {
    let mut nl = Netlist::default();
    let mut lyr: Vec<String> = Vec::new();
    let mut net: Option<String> = None;
    let mut pin: Option<(String, String)> = None;
    for line in t.lines() {
        let l = line.split(';').next().unwrap_or("").trim();
        let tok: Vec<&str> = l.split_whitespace().collect();
        match tok.first().copied() {
            Some("LYR") => lyr = tok[1..].iter().map(|s| s.to_ascii_lowercase()).collect(),
            Some("NET") => {
                let name = tok.get(1).copied().unwrap_or("");
                net = (name != "$NONE$" && !name.is_empty()).then(|| name.to_string());
                pin = None;
            }
            Some("SNT") => {
                pin = None;
                // SNT TOP <T|B> <component> <toeprint>
                if tok.get(1) == Some(&"TOP") {
                    let bottom = tok.get(2) == Some(&"B");
                    let c: usize = tok
                        .get(3)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(usize::MAX);
                    let p: usize = tok
                        .get(4)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(usize::MAX);
                    if let Some((refdes, pins)) = comps.get(&(bottom, c)) {
                        if let Some(pn) = pins.get(p) {
                            pin = Some((refdes.clone(), pn.clone()));
                        }
                    }
                }
            }
            Some("FID") if tok.len() >= 4 && tok[1] == "C" => {
                let (Ok(li), Ok(fi)) = (tok[2].parse::<usize>(), tok[3].parse::<usize>()) else {
                    continue;
                };
                let Some(layer) = lyr.get(li) else { continue };
                let a = ObjAttrs {
                    net: net.clone(),
                    component: pin.as_ref().map(|p| p.0.clone()),
                    pin: pin.as_ref().map(|p| p.1.clone()),
                };
                nl.by_layer.entry(layer.clone()).or_default().insert(fi, a);
            }
            _ => {}
        }
    }
    nl
}

/// Components per side: index -> (refdes, pin names by toeprint index),
/// plus the parts themselves.
fn parse_components(
    t: &str,
    bottom: bool,
) -> (Vec<(String, Vec<String>)>, Vec<trace_core::Component>) {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut parts = Vec::new();
    let mut unit = 25.4;
    for line in t.lines() {
        let l = line.split(';').next().unwrap_or("").trim();
        if l.eq_ignore_ascii_case("UNITS=MM") || l.eq_ignore_ascii_case("U MM") {
            unit = 1.0;
        }
        let tok: Vec<&str> = l.split_whitespace().collect();
        match tok.first().copied() {
            Some("CMP") => {
                let num = |i: usize| {
                    tok.get(i)
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0)
                };
                let refdes = tok.get(6).unwrap_or(&"").to_string();
                parts.push(trace_core::Component {
                    designator: refdes.clone(),
                    footprint: tok.get(7).unwrap_or(&"").to_string(),
                    library_ref: None,
                    x: trace_core::Length::from_mm(num(2) * unit),
                    y: trace_core::Length::from_mm(num(3) * unit),
                    rotation_deg: (-num(4)).rem_euclid(360.0),
                    side: if bottom {
                        trace_core::Side::Bottom
                    } else {
                        trace_core::Side::Top
                    },
                    sheet: None,
                    params: Default::default(),
                });
                out.push((refdes, Vec::new()));
            }
            Some("TOP") => {
                if let Some(c) = out.last_mut() {
                    c.1.push(tok.last().unwrap_or(&"").to_string());
                }
            }
            _ => {}
        }
    }
    (out, parts)
}

fn pick_step(job: &Job, steps: &[String]) -> Option<String> {
    let lower: Vec<String> = steps.iter().map(|s| s.to_ascii_lowercase()).collect();
    let is_panel = |s: &str| {
        job.text(&format!("steps/{s}/stephdr"))
            .is_some_and(|t| t.contains("STEP-REPEAT"))
    };
    lower
        .iter()
        .find(|s| *s == "pcb")
        .or_else(|| {
            lower
                .iter()
                .find(|s| !is_panel(s) && job.has(&format!("steps/{s}/eda/data")))
        })
        .or_else(|| lower.iter().find(|s| !is_panel(s)))
        .or(lower.first())
        .cloned()
}

/// Reads an ODB++ job into a board.
pub fn read(job: &Job) -> Result<Import> {
    let matrix = job
        .text("matrix/matrix")
        .ok_or_else(|| Error::Format("ODB++ job without matrix/matrix".into()))?;
    let (steps, mlayers) = parse_matrix(&matrix);
    let step =
        pick_step(job, &steps).ok_or_else(|| Error::Format("ODB++ job has no step".into()))?;
    let dir = format!("steps/{step}");
    let board_layers: Vec<&MatrixLayer> = mlayers.iter().filter(|l| l.context == "BOARD").collect();
    let is_cu = |l: &MatrixLayer| matches!(l.kind.as_str(), "SIGNAL" | "POWER_GROUND" | "MIXED");
    let cu_rows: Vec<u32> = board_layers
        .iter()
        .filter(|l| is_cu(l))
        .map(|l| l.row)
        .collect();
    let (first_cu, last_cu) = (
        cu_rows.first().copied().unwrap_or(0),
        cu_rows.last().copied().unwrap_or(0),
    );
    let cu_number = |name: &str| -> Option<u32> {
        board_layers
            .iter()
            .filter(|l| is_cu(l))
            .position(|l| l.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32 + 1)
    };

    let mut comps: HashMap<(bool, usize), (String, Vec<String>)> = HashMap::new();
    let mut parts = Vec::new();
    for (bottom, layer) in [(false, "comp_+_top"), (true, "comp_+_bot")] {
        if let Some(t) = job.text(&format!("{dir}/layers/{layer}/components")) {
            let (pins, p) = parse_components(&t, bottom);
            for (i, c) in pins.into_iter().enumerate() {
                comps.insert((bottom, i), c);
            }
            parts.extend(p);
        }
    }
    let netlist = job
        .text(&format!("{dir}/eda/data"))
        .map(|t| parse_eda(&t, &comps))
        .unwrap_or_default();

    let mut syms = Symbols {
        job,
        cache: HashMap::new(),
    };
    let mut layers = Vec::new();
    for ml in &board_layers {
        let side = if ml.row < first_cu {
            Side::Top
        } else {
            Side::Bottom
        };
        let outer = ml.row < first_cu || ml.row > last_cu;
        let role = match ml.kind.as_str() {
            "SIGNAL" | "POWER_GROUND" | "MIXED" => {
                let n = cu_number(&ml.name).unwrap_or(0);
                Role::Copper {
                    pos: if ml.row == first_cu {
                        CuPos::Top
                    } else if ml.row == last_cu {
                        CuPos::Bottom
                    } else {
                        CuPos::Inner
                    },
                    number: Some(n),
                    order: Some(n),
                    plane: ml.kind == "POWER_GROUND",
                }
            }
            "SOLDER_MASK" if outer => Role::Mask(side),
            "SOLDER_PASTE" if outer => Role::Paste(side),
            "SILK_SCREEN" if outer => Role::Silk(side),
            "DRILL" => {
                let lname = ml.name.to_ascii_lowercase();
                let plated = !(lname.contains("non")
                    || lname.contains("npth")
                    || lname.contains("unplated"));
                let span = match (cu_number(&ml.start), cu_number(&ml.end)) {
                    (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
                    _ => None,
                };
                Role::Drill {
                    plated: Some(plated),
                    span,
                }
            }
            _ => continue,
        };
        let path = format!("{dir}/layers/{}/features", ml.name.to_ascii_lowercase());
        let Some(text) = job.text(&path) else {
            continue;
        };
        let feats = parse_features(&text);
        let names = netlist.by_layer.get(&ml.name.to_ascii_lowercase());
        let data = if let Role::Drill { plated, .. } = role {
            LayerData::Drill(drill_layer(&feats, plated, &mut syms))
        } else {
            let mut g = to_gerber(&feats, &mut syms, names);
            g.image_negative = ml.negative;
            LayerData::Gerber(g)
        };
        layers.push(Layer {
            file: format!("{} ({})", ml.name, ml.kind.to_ascii_lowercase()),
            role,
            reason: "ODB++ matrix".into(),
            data,
        });
    }
    // The step profile is the board outline.
    if let Some(t) = job.text(&format!("{dir}/profile")) {
        let feats = parse_features(&t);
        let g = to_gerber(&feats, &mut syms, None);
        layers.push(Layer {
            file: "profile".into(),
            role: Role::Outline,
            reason: "ODB++ step profile".into(),
            data: LayerData::Gerber(g),
        });
    }
    if !layers.iter().any(|l| l.role.is_copper()) {
        return Err(Error::Format(format!(
            "ODB++ step {step} has no copper layers"
        )));
    }
    let mut imp = crate::build::build_with(layers, "the ODB++ netlist", parts);
    if let Some(st) = stackup(job, &dir, &board_layers) {
        if st.copper_layers().count() == imp.board.stackup.copper_layers().count() {
            imp.board.stackup = st;
        }
    }
    Ok(imp)
}

/// A layer's attribute list (`.name = value`), values in mm where `unit`
/// says the file is in inches.
fn attrlist(t: &str) -> (HashMap<String, String>, f64) {
    let mut unit = 1.0;
    let mut kv = HashMap::new();
    for line in t.lines() {
        let l = line.trim();
        if let Some(u) = l.strip_prefix("UNITS=") {
            unit = if u.trim().eq_ignore_ascii_case("INCH") {
                25.4
            } else {
                1.0
            };
        } else if let Some((k, v)) = l.split_once('=') {
            kv.entry(k.trim().to_ascii_lowercase())
                .or_insert_with(|| v.trim().trim_matches('"').to_string());
        }
    }
    (kv, unit)
}

/// The stack-up from the matrix's copper and dielectric layers and their
/// attribute lists (copper weight, dielectric thickness, permittivity,
/// material), when the job has dielectric layers.
fn stackup(job: &Job, dir: &str, layers: &[&MatrixLayer]) -> Option<trace_core::Stackup> {
    use trace_core::{LayerKind, Length, StackLayer};
    if !layers.iter().any(|l| l.kind == "DIELECTRIC") {
        return None;
    }
    let mut st = trace_core::Stackup::default();
    for l in layers {
        let copper = matches!(l.kind.as_str(), "SIGNAL" | "POWER_GROUND" | "MIXED");
        if !copper && l.kind != "DIELECTRIC" {
            continue;
        }
        let (kv, unit) = job
            .text(&format!(
                "{dir}/layers/{}/attrlist",
                l.name.to_ascii_lowercase()
            ))
            .map(|t| attrlist(&t))
            .unwrap_or_default();
        let num = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok());
        // The design's own name when it is the layer's name spelled out.
        let name = match kv.get(".eda_layers") {
            Some(e) if e.to_ascii_lowercase().replace(' ', "_") == l.name.to_ascii_lowercase() => {
                e.clone()
            }
            _ => l.name.clone(),
        };
        let (thickness, er, material) = if copper {
            (num(".copper_weight").map(|oz| oz * 0.035), None, None)
        } else {
            (
                num(".layer_dielectric").map(|t| t * unit),
                num(".dielectric_constant"),
                kv.get(".comment").filter(|c| !c.is_empty()).cloned(),
            )
        };
        let df = if copper { None } else { num(".loss_tangent") };
        st.layers.push(StackLayer {
            name,
            altium_id: None,
            kind: if copper {
                LayerKind::Copper
            } else {
                LayerKind::Dielectric
            },
            thickness: Length::from_mm(thickness.unwrap_or(0.0)),
            dielectric_constant: er,
            loss_tangent: df,
            roughness_um: None,
            material,
        });
    }
    Some(st)
}

fn to_gerber(f: &Features, syms: &mut Symbols, names: Option<&HashMap<usize, ObjAttrs>>) -> Gerber {
    let mut g = Gerber::default();
    g.attrs.push(ObjAttrs::default());
    let mut attr_index: HashMap<ObjAttrs, u32> = HashMap::new();
    let mut aps: HashMap<(usize, u64), u32> = HashMap::new();
    let mut aperture = |g: &mut Gerber, sym: usize, resize: f64| -> u32 {
        *aps.entry((sym, resize.to_bits())).or_insert_with(|| {
            let name = f.symbols.get(sym).cloned().unwrap_or_default();
            let mut shape = syms.shape(&name, f.sym_unit);
            if resize > 0.0 {
                if let Shape::Circle(_) = shape {
                    shape = Shape::Circle(resize * f.sym_unit);
                }
            }
            g.apertures.push(Aperture {
                dcode: 10 + g.apertures.len() as u32,
                shape,
                hole: 0.0,
                function: None,
            });
            g.apertures.len() as u32 - 1
        })
    };
    for (i, feat) in f.features.iter().enumerate() {
        let attrs = match names.and_then(|m| m.get(&i)) {
            Some(a) => {
                let next = g.attrs.len() as u32;
                *attr_index.entry(a.clone()).or_insert_with(|| {
                    g.attrs.push(a.clone());
                    next
                })
            }
            None => 0,
        };
        let (obj, dark) = match feat {
            Feature::Line { a, b, sym, dark } => (
                Obj::Line {
                    ap: aperture(&mut g, *sym, 0.0),
                    a: *a,
                    b: *b,
                },
                *dark,
            ),
            Feature::Arc {
                a,
                b,
                c,
                sym,
                dark,
                cw,
            } => (
                Obj::Arc {
                    ap: aperture(&mut g, *sym, 0.0),
                    a: *a,
                    b: *b,
                    c: *c,
                    cw: *cw,
                },
                *dark,
            ),
            Feature::Pad {
                at,
                sym,
                resize,
                dark,
                xf,
            } => (
                Obj::Flash {
                    ap: aperture(&mut g, *sym, *resize),
                    at: *at,
                    xf: *xf,
                },
                *dark,
            ),
            Feature::Surface { dark, contours } => {
                // Islands counter-clockwise and holes clockwise, so a
                // non-zero union keeps the holes.
                let rings: Vec<Vec<Pt>> = contours
                    .iter()
                    .filter(|c| c.1.len() >= 3)
                    .map(|(island, c)| {
                        let mut c = c.clone();
                        if geom::dist(c[0], *c.last().unwrap()) < 1e-9 {
                            c.pop();
                        }
                        if (geom::area(&c) > 0.0) != *island {
                            c.reverse();
                        }
                        c
                    })
                    .collect();
                if rings.is_empty() {
                    continue;
                }
                (Obj::Region(rings), *dark)
            }
            Feature::Other => continue,
        };
        g.items.push(Item { obj, dark, attrs });
    }
    g
}

fn drill_layer(f: &Features, plated: Option<bool>, syms: &mut Symbols) -> Drill {
    let drill_attr = f
        .attr_names
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(".drill"))
        .map(|(i, _)| *i);
    let mut d = Drill {
        plated,
        ..Default::default()
    };
    for (i, feat) in f.features.iter().enumerate() {
        // .drill options: 0 plated, 1 non_plated, 2 via.
        let kind = drill_attr.and_then(|k| {
            f.attrs[i].split(',').find_map(|kv| {
                let (a, v) = kv.split_once('=')?;
                (a.trim().parse::<usize>().ok()? == k)
                    .then(|| v.trim().parse::<u32>().ok())
                    .flatten()
            })
        });
        let hole_plated = match kind {
            Some(1) => Some(false),
            Some(_) => Some(true),
            None => plated,
        };
        let function = (kind == Some(2)).then(|| "ViaDrill".to_string());
        let dia = |sym: usize, syms: &mut Symbols| match syms.shape(
            f.symbols.get(sym).map(|s| s.as_str()).unwrap_or(""),
            f.sym_unit,
        ) {
            Shape::Circle(d) => d,
            Shape::Rect(w, h) | Shape::Obround(w, h) => w.min(h),
            _ => 0.0,
        };
        match feat {
            Feature::Pad { at, sym, .. } => d.holes.push(Hole {
                at: *at,
                end: None,
                diameter: dia(*sym, syms),
                plated: hole_plated,
                function,
            }),
            Feature::Line { a, b, sym, .. } => d.holes.push(Hole {
                at: *a,
                end: Some(*b),
                diameter: dia(*sym, syms),
                plated: hole_plated,
                function,
            }),
            _ => {}
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzw_roundtrip_sample() {
        // "TOBEORNOTTOBEORTOBEORNOT" compressed with `compress -b 16`.
        let z = [
            0x1f, 0x9d, 0x90, 0x54, 0x9e, 0x08, 0x29, 0xf2, 0x44, 0x8a, 0x93, 0x27, 0x54, 0x02,
            0x0e, 0x2c, 0xa8, 0x90, 0xa0, 0x41, 0x84, 0x0a, 0x00,
        ];
        let out = unix_uncompress(&z).unwrap();
        assert_eq!(String::from_utf8_lossy(&out), "TOBEORNOTTOBEORTOBEORNOT\n");
    }

    #[test]
    fn lzw_large_file() {
        // Set TRACE_LZW_SAMPLE=<file.Z>,<original> to check a big file.
        if let Ok(v) = std::env::var("TRACE_LZW_SAMPLE") {
            let (z, plain) = v.split_once(',').unwrap();
            let out = unix_uncompress(&std::fs::read(z).unwrap()).unwrap();
            assert!(out == std::fs::read(plain).unwrap());
        }
    }

    #[test]
    fn symbol_names() {
        assert_eq!(nums("rect20.5x30xr5"), vec![20.5, 30.0, 5.0]);
    }
}
