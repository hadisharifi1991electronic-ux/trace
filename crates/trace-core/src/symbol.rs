//! Schematic symbols as a library editor holds them: the component's
//! body graphics, its pins with their numbers, names and electrical
//! types, its parts (U1A, U1B...), and its parameters. Readers and writers
//! of library files (Altium `.SchLib`) turn this into records and back;
//! `LibSymbol`s for placing come from the same records, so what the
//! editor shows is what a sheet gets.
//!
//! Coordinates are in mil with y up, the symbol's origin at 0,0. A pin's
//! `base` is the end on the body; it reaches `len` mil out in its
//! direction to the end that wires connect to.

use crate::sch_edit::Xf;
use crate::schematic::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The grid symbols are drawn on, in mil. Pin ends belong on it so wires
/// meet them on a sheet's 100 mil grid.
pub const SYM_GRID: f64 = 10.0;

/// Altium's default colours: dark blue lines, pale yellow body, pins and
/// texts black and blue.
pub const LINE_BLUE: Rgb = [0, 0, 128];
pub const BODY_YELLOW: Rgb = [255, 255, 176];
pub const TEXT_BLUE: Rgb = [0, 0, 128];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolDef {
    /// The component's name in the library (Altium's library reference).
    pub name: String,
    pub description: String,
    /// The designator with its question mark: `U?`, `R?`.
    pub designator: String,
    /// The comment (value) a placed part shows; `*` shows the name.
    pub comment: String,
    /// How many parts (gates) the component has, 1 or more.
    pub parts: u32,
    /// PCB footprints the component may use, the first being current.
    pub footprints: Vec<String>,
    pub params: Vec<SymParam>,
    pub pins: Vec<SymPin>,
    pub shapes: Vec<SymShape>,
    pub texts: Vec<SymText>,
    /// Where the designator and the comment are drawn.
    pub designator_at: Pt,
    pub comment_at: Pt,
    pub show_comment: bool,
    /// Fields of the file's records the editor does not change (unique
    /// ids, flags), written back as they were: the component's, the
    /// designator's and the comment's.
    #[serde(default)]
    pub raw: Vec<(String, String)>,
    #[serde(default)]
    pub designator_raw: Vec<(String, String)>,
    #[serde(default)]
    pub comment_raw: Vec<(String, String)>,
    /// Records the editor does not model (model links, hidden
    /// parameters' trees, notes), with their index in the file, written
    /// back as they were.
    #[serde(default)]
    pub kept: Vec<KeptRec>,
    /// The component's streams as read; while it is not edited they are
    /// written back byte for byte. Any edit clears them (`touch`).
    #[serde(skip)]
    pub source: Vec<(String, Arc<Vec<u8>>)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeptRec {
    /// The record's place in the component as read (0 is the component).
    pub index: usize,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SymParam {
    pub name: String,
    pub value: String,
    pub visible: bool,
    pub at: Pt,
    #[serde(default)]
    pub raw: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymPin {
    /// Which part it is on (1 for A; 0 on every part).
    pub part: u32,
    /// The end on the body.
    pub base: Pt,
    pub len: f64,
    /// Which way it points from the body: 0 right, 1 up, 2 left, 3 down.
    pub orient: u8,
    pub name: String,
    pub number: String,
    pub kind: PinKind,
    pub hidden: bool,
    pub show_name: bool,
    pub show_number: bool,
    /// A bubble at the body (negated) and a clock wedge inside.
    pub dot: bool,
    pub clock: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymShape {
    pub part: u32,
    pub geom: SchGeom,
    /// Line width in mil (Altium has 0, 10, 30 and 50 as small to large).
    pub width: f64,
    pub stroke: Rgb,
    pub fill: Option<Rgb>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymText {
    pub part: u32,
    pub at: Pt,
    pub text: String,
    /// Font height in mil.
    pub size: f64,
    pub rot: u8,
    pub just: u8,
    pub color: Rgb,
}

/// Whether something on part `of` (0: on every part) shows on `part`.
pub fn on_part(of: u32, part: u32) -> bool {
    of == 0 || of == part
}

/// What a point on the editor's canvas is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymHit {
    Pin(usize),
    Shape(usize),
    Text(usize),
    Designator,
    Comment,
}

/// The unit vector a pin orientation points along.
pub fn orient_dir(o: u8) -> Pt {
    match o % 4 {
        0 => [1.0, 0.0],
        1 => [0.0, 1.0],
        2 => [-1.0, 0.0],
        _ => [0.0, -1.0],
    }
}

impl SymPin {
    pub fn new(part: u32, base: Pt, orient: u8, number: &str, name: &str) -> SymPin {
        SymPin {
            part,
            base,
            len: 200.0,
            orient,
            name: name.to_string(),
            number: number.to_string(),
            kind: PinKind::Passive,
            hidden: false,
            show_name: true,
            show_number: true,
            dot: false,
            clock: false,
            description: String::new(),
        }
    }

    /// The end wires connect to.
    pub fn hot(&self) -> Pt {
        let d = orient_dir(self.orient);
        [
            self.base[0] + d[0] * self.len,
            self.base[1] + d[1] * self.len,
        ]
    }
}

impl Default for SymbolDef {
    fn default() -> Self {
        SymbolDef {
            name: String::new(),
            description: String::new(),
            designator: "U?".into(),
            comment: "*".into(),
            parts: 1,
            footprints: Vec::new(),
            params: Vec::new(),
            pins: Vec::new(),
            shapes: Vec::new(),
            texts: Vec::new(),
            designator_at: [0.0, 100.0],
            comment_at: [0.0, -100.0],
            show_comment: true,
            raw: Vec::new(),
            designator_raw: Vec::new(),
            comment_raw: Vec::new(),
            kept: Vec::new(),
            source: Vec::new(),
        }
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    };
    (a[0] + t * dx - p[0]).hypot(a[1] + t * dy - p[1])
}

/// The corners of a shape, for boxes and hits.
fn geom_points(g: &SchGeom) -> Vec<Pt> {
    match g {
        SchGeom::Line(v) | SchGeom::Poly(v) => v.clone(),
        SchGeom::Rect { min, max, .. } => vec![*min, [max[0], min[1]], *max, [min[0], max[1]]],
        SchGeom::Arc { c, r, .. } => vec![[c[0] - r[0], c[1] - r[1]], [c[0] + r[0], c[1] + r[1]]],
    }
}

/// How far `p` is from the outline of a shape (0 inside a filled one).
pub fn geom_dist(g: &SchGeom, filled: bool, p: Pt) -> f64 {
    let ring = |v: &[Pt], closed: bool| {
        let mut d = f64::MAX;
        for s in v.windows(2) {
            d = d.min(seg_dist(p, s[0], s[1]));
        }
        if closed && v.len() > 2 {
            d = d.min(seg_dist(p, v[v.len() - 1], v[0]));
        }
        if v.len() == 1 {
            d = d.min((p[0] - v[0][0]).hypot(p[1] - v[0][1]));
        }
        d
    };
    match g {
        SchGeom::Line(v) => ring(v, false),
        SchGeom::Poly(v) => {
            if filled && inside(v, p) {
                0.0
            } else {
                ring(v, true)
            }
        }
        SchGeom::Rect { min, max, .. } => {
            let within = p[0] >= min[0] && p[0] <= max[0] && p[1] >= min[1] && p[1] <= max[1];
            if filled && within {
                0.0
            } else {
                ring(&geom_points(g), true)
            }
        }
        SchGeom::Arc { c, r, start, end } => {
            let (dx, dy) = (
                (p[0] - c[0]) / r[0].max(1e-6),
                (p[1] - c[1]) / r[1].max(1e-6),
            );
            let rr = dx.hypot(dy);
            let a = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            let s = start.rem_euclid(360.0);
            let span = (end - start).abs().min(360.0);
            let on = span >= 359.99 || (a - s).rem_euclid(360.0) <= span;
            if filled && rr <= 1.0 && on {
                0.0
            } else if on {
                ((rr - 1.0) * r[0].max(r[1])).abs()
            } else {
                let e = |deg: f64| {
                    let t = deg.to_radians();
                    [c[0] + r[0] * t.cos(), c[1] + r[1] * t.sin()]
                };
                let (a, b) = (e(*start), e(*end));
                (p[0] - a[0])
                    .hypot(p[1] - a[1])
                    .min((p[0] - b[0]).hypot(p[1] - b[1]))
            }
        }
    }
}

fn inside(v: &[Pt], p: Pt) -> bool {
    let mut c = false;
    let n = v.len();
    for i in 0..n {
        let (a, b) = (v[i], v[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
    }
    c
}

/// A pin from a pin list: number, name, type and side of the body.
#[derive(Clone, Debug, PartialEq)]
pub struct ListPin {
    pub number: String,
    pub name: String,
    pub kind: PinKind,
    /// L, R, T or B.
    pub side: char,
}

/// Reads an electrical type as people write it in pin lists and
/// datasheets: in, out, io/bidir, pwr/power/gnd, oc, oe, hiz, passive.
pub fn pin_kind_from(s: &str) -> Option<PinKind> {
    let s = s.trim().to_ascii_lowercase();
    Some(match s.as_str() {
        "i" | "in" | "input" => PinKind::Input,
        "o" | "out" | "output" => PinKind::Output,
        "io" | "i/o" | "bidir" | "bidirectional" | "inout" => PinKind::Bidir,
        "p" | "pwr" | "power" | "gnd" | "ground" | "supply" => PinKind::Power,
        "oc" | "open collector" | "opencollector" | "od" | "open drain" => PinKind::OpenCollector,
        "oe" | "open emitter" | "openemitter" => PinKind::OpenEmitter,
        "z" | "hiz" | "hi-z" | "high-z" | "tristate" | "tri-state" => PinKind::HiZ,
        "passive" | "pas" | "nc" | "" => PinKind::Passive,
        _ => return None,
    })
}

/// Reads a pin list, one pin per line: `number, name, type, side`, split
/// by commas, tabs or semicolons (as copied from a spreadsheet). Type and
/// side may be left out: then the pin is passive, and inputs and power go
/// on the left, the rest on the right. Lines that start with `#` and a
/// header line (`Pin, Name...`) are skipped.
pub fn parse_pin_list(text: &str) -> Vec<ListPin> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = if l.contains('\t') {
            l.split('\t').collect()
        } else if l.contains(',') {
            l.split(',').collect()
        } else if l.contains(';') {
            l.split(';').collect()
        } else {
            l.split_whitespace().collect()
        };
        let cols: Vec<&str> = cols.into_iter().map(str::trim).collect();
        let number = cols.first().copied().unwrap_or_default().to_string();
        if number.is_empty() {
            continue;
        }
        let low = number.to_ascii_lowercase();
        if matches!(
            low.as_str(),
            "pin" | "number" | "no" | "no." | "pin number" | "#"
        ) {
            continue;
        }
        let name = cols.get(1).copied().unwrap_or_default().to_string();
        let kind_text = cols.get(2).copied().unwrap_or_default();
        let kind = pin_kind_from(kind_text).unwrap_or(PinKind::Passive);
        let side = cols
            .get(3)
            .and_then(|s| s.trim().chars().next())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| "LRTB".contains(*c))
            .unwrap_or(match kind {
                PinKind::Input | PinKind::Power => 'L',
                _ => 'R',
            });
        out.push(ListPin {
            number,
            name,
            kind,
            side,
        });
    }
    out
}

/// Natural order for pin numbers: 2 before 10, A2 before A10.
pub fn pin_order(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let p = s.trim_end_matches(|c: char| c.is_ascii_digit());
        let n: u64 = s[p.len()..].parse().unwrap_or(0);
        (p.to_ascii_uppercase(), n, s.to_string())
    };
    split(a).cmp(&split(b))
}

impl SymbolDef {
    /// A new component with an empty 600 x 400 mil body.
    pub fn new(name: &str) -> SymbolDef {
        let mut d = SymbolDef {
            name: name.to_string(),
            ..Default::default()
        };
        d.shapes.push(SymShape {
            part: 1,
            geom: SchGeom::Rect {
                min: [0.0, -400.0],
                max: [600.0, 0.0],
                r: [0.0, 0.0],
            },
            width: 10.0,
            stroke: LINE_BLUE,
            fill: Some(BODY_YELLOW),
        });
        d.designator_at = [0.0, 50.0];
        d.comment_at = [0.0, -500.0];
        d
    }

    /// Marks the component as edited: it is written from the model from
    /// now on, not copied from the file it came from.
    pub fn touch(&mut self) {
        self.source.clear();
    }

    /// The part letter: A for 1.
    pub fn letter(part: u32) -> char {
        (b'A' + (part.clamp(1, 26) - 1) as u8) as char
    }

    /// A box holding everything drawn for `part`: (min, max).
    pub fn bounds(&self, part: u32) -> (Pt, Pt) {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut add = |p: Pt| {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        };
        for s in self.shapes.iter().filter(|s| on_part(s.part, part)) {
            geom_points(&s.geom).into_iter().for_each(&mut add);
        }
        for p in self.pins.iter().filter(|p| on_part(p.part, part)) {
            add(p.base);
            add(p.hot());
        }
        if b[0] > b[2] {
            return ([0.0, 0.0], [0.0, 0.0]);
        }
        ([b[0], b[1]], [b[2], b[3]])
    }

    /// The next free pin number: one past the highest plain number.
    pub fn next_number(&self) -> String {
        let n = self
            .pins
            .iter()
            .filter_map(|p| p.number.trim().parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        (n + 1).to_string()
    }

    /// Adds a pin on `part` with the next number. Returns its index.
    pub fn add_pin(&mut self, part: u32, base: Pt, orient: u8) -> usize {
        let n = self.next_number();
        self.touch();
        self.pins.push(SymPin::new(part, base, orient, &n, &n));
        self.pins.len() - 1
    }

    /// What lies under `p` on `part`, nearest first within `tol` mil:
    /// pins win over texts, texts over shapes.
    pub fn hit(&self, p: Pt, part: u32, tol: f64) -> Option<SymHit> {
        let mut best: Option<(f64, SymHit)> = None;
        let mut offer = |d: f64, h: SymHit, bias: f64| {
            if d <= tol && best.is_none_or(|b| d - bias < b.0) {
                best = Some((d - bias, h));
            }
        };
        for (i, q) in self
            .pins
            .iter()
            .enumerate()
            .filter(|(_, q)| on_part(q.part, part))
        {
            offer(seg_dist(p, q.base, q.hot()), SymHit::Pin(i), tol * 0.5);
        }
        let near = |a: Pt| (p[0] - a[0]).hypot(p[1] - a[1]);
        for (i, t) in self
            .texts
            .iter()
            .enumerate()
            .filter(|(_, t)| on_part(t.part, part))
        {
            offer(near(t.at), SymHit::Text(i), tol * 0.3);
        }
        offer(near(self.designator_at), SymHit::Designator, tol * 0.3);
        if self.show_comment {
            offer(near(self.comment_at), SymHit::Comment, tol * 0.3);
        }
        for (i, s) in self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| on_part(s.part, part))
        {
            offer(
                geom_dist(&s.geom, s.fill.is_some(), p),
                SymHit::Shape(i),
                0.0,
            );
        }
        best.map(|b| b.1)
    }

    /// Moves what `h` is by `d` mil.
    pub fn move_hit(&mut self, h: SymHit, d: Pt) {
        self.touch();
        let f = |p: &mut Pt| *p = [p[0] + d[0], p[1] + d[1]];
        match h {
            SymHit::Pin(i) => {
                if let Some(q) = self.pins.get_mut(i) {
                    f(&mut q.base);
                }
            }
            SymHit::Shape(i) => {
                if let Some(s) = self.shapes.get_mut(i) {
                    Xf::Shift(d).geom(&mut s.geom);
                }
            }
            SymHit::Text(i) => {
                if let Some(t) = self.texts.get_mut(i) {
                    f(&mut t.at);
                }
            }
            SymHit::Designator => f(&mut self.designator_at),
            SymHit::Comment => f(&mut self.comment_at),
        }
    }

    /// Turns what `h` is a quarter turn counter-clockwise: a pin about its
    /// body end, so it stays on the body edge.
    pub fn turn_hit(&mut self, h: SymHit) {
        self.touch();
        match h {
            SymHit::Pin(i) => {
                if let Some(q) = self.pins.get_mut(i) {
                    q.orient = (q.orient + 1) % 4;
                }
            }
            SymHit::Shape(i) => {
                if let Some(s) = self.shapes.get_mut(i) {
                    let pts = geom_points(&s.geom);
                    let n = pts.len().max(1) as f64;
                    let c = pts
                        .iter()
                        .fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
                    let c = [
                        (c[0] / SYM_GRID).round() * SYM_GRID,
                        (c[1] / SYM_GRID).round() * SYM_GRID,
                    ];
                    Xf::Turn(c).geom(&mut s.geom);
                }
            }
            SymHit::Text(i) => {
                if let Some(t) = self.texts.get_mut(i) {
                    t.rot = (t.rot + 1) % 4;
                }
            }
            _ => {}
        }
    }

    /// Removes what `h` is (the designator and comment stay; the comment
    /// is hidden instead).
    pub fn delete_hit(&mut self, h: SymHit) {
        self.touch();
        match h {
            SymHit::Pin(i) if i < self.pins.len() => {
                self.pins.remove(i);
            }
            SymHit::Shape(i) if i < self.shapes.len() => {
                self.shapes.remove(i);
            }
            SymHit::Text(i) if i < self.texts.len() => {
                self.texts.remove(i);
            }
            SymHit::Comment => self.show_comment = false,
            _ => {}
        }
    }

    /// Adds an empty part after the last. Returns its number.
    pub fn add_part(&mut self) -> u32 {
        self.touch();
        self.parts = (self.parts + 1).min(26);
        self.parts
    }

    /// Adds a part drawn like `part`, with the next free pin numbers and
    /// the same pin names (a second op-amp of a dual). Returns its number.
    pub fn duplicate_part(&mut self, part: u32) -> u32 {
        if self.parts >= 26 {
            return self.parts;
        }
        let new = self.add_part();
        let shapes: Vec<SymShape> = self
            .shapes
            .iter()
            .filter(|s| s.part == part)
            .cloned()
            .map(|mut s| {
                s.part = new;
                s
            })
            .collect();
        self.shapes.extend(shapes);
        let texts: Vec<SymText> = self
            .texts
            .iter()
            .filter(|t| t.part == part)
            .cloned()
            .map(|mut t| {
                t.part = new;
                t
            })
            .collect();
        self.texts.extend(texts);
        let pins: Vec<SymPin> = self
            .pins
            .iter()
            .filter(|p| p.part == part)
            .cloned()
            .collect();
        for mut p in pins {
            p.part = new;
            // Shared supply pins stay shared: numbered as they were.
            if p.kind != PinKind::Power {
                p.number = self.next_number();
            }
            self.pins.push(p);
        }
        new
    }

    /// Removes part `part` and what is drawn on it; later parts move down
    /// a letter.
    pub fn remove_part(&mut self, part: u32) {
        if self.parts <= 1 || part == 0 || part > self.parts {
            return;
        }
        self.touch();
        self.pins.retain(|p| p.part != part);
        self.shapes.retain(|s| s.part != part);
        self.texts.retain(|t| t.part != part);
        for p in &mut self.pins {
            if p.part > part {
                p.part -= 1;
            }
        }
        for s in &mut self.shapes {
            if s.part > part {
                s.part -= 1;
            }
        }
        for t in &mut self.texts {
            if t.part > part {
                t.part -= 1;
            }
        }
        self.parts -= 1;
    }

    /// What is wrong with the component, for the editor to show before
    /// saving: pins with no number or a number used twice on different
    /// parts (power pins shared by every part may repeat), ends off the
    /// 10 mil grid, empty parts.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push("The component has no name.".into());
        }
        let mut seen: std::collections::HashMap<String, Vec<&SymPin>> = Default::default();
        for p in &self.pins {
            if p.number.trim().is_empty() {
                out.push(format!(
                    "A pin on part {} named '{}' has no number.",
                    Self::letter(p.part),
                    p.name
                ));
                continue;
            }
            seen.entry(p.number.trim().to_ascii_uppercase())
                .or_default()
                .push(p);
        }
        let mut dups: Vec<_> = seen
            .iter()
            .filter(|(_, v)| {
                v.len() > 1 && {
                    // The same pin repeated on each part (shared supply) is fine.
                    let same_part = v
                        .iter()
                        .any(|a| v.iter().filter(|b| b.part == a.part).count() > 1);
                    let shared = v
                        .iter()
                        .all(|q| q.name == v[0].name && q.kind == PinKind::Power);
                    same_part || !shared
                }
            })
            .map(|(k, _)| k.clone())
            .collect();
        dups.sort_by(|a, b| pin_order(a, b));
        if !dups.is_empty() {
            out.push(format!(
                "Pin numbers used more than once: {}.",
                dups.join(", ")
            ));
        }
        let off = |v: f64| ((v / SYM_GRID).round() * SYM_GRID - v).abs() > 0.01;
        let off_grid: Vec<&str> = self
            .pins
            .iter()
            .filter(|p| {
                let h = p.hot();
                off(h[0]) || off(h[1])
            })
            .map(|p| p.number.as_str())
            .collect();
        if !off_grid.is_empty() {
            out.push(format!(
                "Pins whose ends are off the {SYM_GRID} mil grid (wires may miss them): {}.",
                off_grid.join(", ")
            ));
        }
        for part in 1..=self.parts {
            if !self.pins.iter().any(|p| on_part(p.part, part)) && self.parts > 1 {
                out.push(format!("Part {} has no pins.", Self::letter(part)));
            }
        }
        out
    }

    /// Lays out a box symbol from a pin list: pins on the sides they ask
    /// for, 100 mil apart, the body wide enough for the names.
    pub fn from_pin_list(name: &str, pins: &[ListPin]) -> SymbolDef {
        let mut d = SymbolDef::new(name);
        d.shapes.clear();
        let side = |c: char| pins.iter().filter(move |p| p.side == c);
        let count = |c: char| side(c).count() as f64;
        let longest = |c: char| side(c).map(|p| p.name.chars().count()).max().unwrap_or(0) as f64;
        // Names take about 60 mil a letter at Altium's 10 pt.
        let across_lr = longest('L') + longest('R');
        let tb = count('T').max(count('B'));
        let width = ((across_lr * 60.0 + 200.0).max((tb + 1.0) * 100.0) / 100.0).ceil() * 100.0;
        let width = width.max(400.0);
        let rows = count('L').max(count('R'));
        let tall_tb = longest('T').max(longest('B'));
        let height =
            ((rows + 1.0) * 100.0 + if tb > 0.0 { tall_tb * 120.0 } else { 0.0 }).max(200.0);
        let height = (height / 100.0).ceil() * 100.0;
        d.shapes.push(SymShape {
            part: 1,
            geom: SchGeom::Rect {
                min: [0.0, -height],
                max: [width, 0.0],
                r: [0.0, 0.0],
            },
            width: 10.0,
            stroke: LINE_BLUE,
            fill: Some(BODY_YELLOW),
        });
        let top_gap = if count('T') > 0.0 {
            tall_tb * 60.0
        } else {
            0.0
        };
        let top_gap = (top_gap / 100.0).ceil() * 100.0;
        let (mut l, mut r, mut t, mut b) = (0.0, 0.0, 0.0, 0.0);
        for p in pins {
            let (base, orient) = match p.side {
                'L' => {
                    l += 100.0;
                    ([0.0, -l - top_gap], 2)
                }
                'R' => {
                    r += 100.0;
                    ([width, -r - top_gap], 0)
                }
                'T' => {
                    t += 100.0;
                    ([t, 0.0], 1)
                }
                _ => {
                    b += 100.0;
                    ([b, -height], 3)
                }
            };
            let mut q = SymPin::new(1, base, orient, &p.number, &p.name);
            q.kind = p.kind;
            d.pins.push(q);
        }
        d.designator_at = [0.0, 50.0];
        d.comment_at = [0.0, -height - 100.0];
        d
    }

    /// Splits pins whose names say they are supplies (VDD, VCC, GND, VSS,
    /// AGND...) into a part of their own, as big parts are drawn: logic
    /// on part A, power on the last part.
    pub fn split_power_part(&mut self) -> Option<u32> {
        let is_power = |p: &SymPin| p.kind == PinKind::Power;
        let n = self.pins.iter().filter(|p| is_power(p)).count();
        if n == 0 || n == self.pins.len() {
            return None;
        }
        let list: Vec<ListPin> = self
            .pins
            .iter()
            .filter(|p| is_power(p))
            .map(|p| ListPin {
                number: p.number.clone(),
                name: p.name.clone(),
                kind: p.kind,
                side: if p.name.to_ascii_uppercase().contains("GND")
                    || p.name.to_ascii_uppercase().starts_with("VSS")
                {
                    'R'
                } else {
                    'L'
                },
            })
            .collect();
        let rest: Vec<ListPin> = self
            .pins
            .iter()
            .filter(|p| !is_power(p))
            .map(|p| ListPin {
                number: p.number.clone(),
                name: p.name.clone(),
                kind: p.kind,
                side: if p.orient == 2 {
                    'L'
                } else if p.orient == 0 {
                    'R'
                } else if p.orient == 1 {
                    'T'
                } else {
                    'B'
                },
            })
            .collect();
        let logic = SymbolDef::from_pin_list(&self.name, &rest);
        let power = SymbolDef::from_pin_list(&self.name, &list);
        self.touch();
        self.parts = 2;
        self.pins = logic.pins;
        self.shapes = logic.shapes;
        self.texts.clear();
        for mut p in power.pins {
            p.part = 2;
            self.pins.push(p);
        }
        for mut s in power.shapes {
            s.part = 2;
            self.shapes.push(s);
        }
        self.comment_at = logic.comment_at;
        Some(2)
    }

    /// Sorts the pins by part, then by number in natural order (the pin
    /// table's order).
    pub fn sort_pins(&mut self) {
        self.pins.sort_by(|a, b| {
            a.part
                .cmp(&b.part)
                .then_with(|| pin_order(&a.number, &b.number))
        });
    }
}

/// The eight ways a symbol can be placed: mirrored or not, then 0 to 3
/// quarter turns about its origin.
fn placements() -> impl Iterator<Item = (bool, u8)> {
    [false, true]
        .into_iter()
        .flat_map(|m| (0..4u8).map(move |t| (m, t)))
}

fn place_xfs(mirror: bool, turns: u8, shift: Pt) -> Vec<Xf> {
    let mut v = Vec::new();
    if mirror {
        v.push(Xf::MirrorX(0.0));
    }
    for _ in 0..turns {
        v.push(Xf::Turn([0.0, 0.0]));
    }
    v.push(Xf::Shift(shift));
    v
}

fn apply_all(xfs: &[Xf], p: Pt) -> Pt {
    xfs.iter().fold(p, |p, x| x.apply(p))
}

/// What `update_from_library` did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateReport {
    /// Parts redrawn, by designator as shown (U1A).
    pub updated: Vec<String>,
    /// Pins whose wires were moved to where the new symbol puts them.
    pub wires_moved: usize,
    /// Pins the placed part had that the new symbol lacks (their wires
    /// are left loose).
    pub lost_pins: Vec<String>,
}

impl Sheet {
    /// Redraws every placed part of library component `lib` (all its
    /// parts as `LibSymbol`s) from the library: body, pins and texts come
    /// from the library, placed where and how the part is now (worked out
    /// from its pins), while the designator and value stay as they are.
    /// Wires on a pin that moved follow it.
    pub fn update_from_library(&mut self, lib: &[LibSymbol]) -> UpdateReport {
        let mut rep = UpdateReport::default();
        let Some(name) = lib.first().map(|l| l.name.clone()) else {
            return rep;
        };
        let mut i = 0;
        while i < self.drawing.parts.len() {
            let q = self.drawing.parts[i].clone();
            let sym = self.symbols.get(q.symbol).cloned().unwrap_or_default();
            if !sym.library_ref.eq_ignore_ascii_case(&name) {
                i += 1;
                continue;
            }
            let want = q.gate.max(1);
            let Some(l) = lib.iter().find(|l| l.part == want).or_else(|| lib.first()) else {
                i += 1;
                continue;
            };
            self.redraw_part(i, l, &mut rep);
            if let Some(s) = self.symbols.get_mut(q.symbol) {
                s.pins = self
                    .drawing
                    .pins
                    .iter()
                    .filter(|p| p.part == i as u32)
                    .count();
                if s.description.is_empty() {
                    s.description = l.symbol.description.clone();
                }
            }
            rep.updated.push(q.full_designator());
            i += 1;
        }
        if !rep.updated.is_empty() {
            self.drawing.connect();
        }
        rep
    }

    fn redraw_part(&mut self, i: usize, l: &LibSymbol, rep: &mut UpdateReport) {
        let d = &mut self.drawing;
        let me = i as u32;
        let q = d.parts[i].clone();
        let sym = self.symbols.get(q.symbol).cloned().unwrap_or_default();
        let src = &l.drawing;
        let old_pins: Vec<SchPin> = d.pins.iter().filter(|p| p.part == me).cloned().collect();
        // How the part is placed: the placement that puts most of the
        // library's pins where the part's pins of the same number are.
        let mut best: Option<((usize, f64), Vec<Xf>)> = None;
        for (m, t) in placements() {
            let probe = place_xfs(m, t, [0.0, 0.0]);
            let pairs: Vec<(Pt, Pt)> = src
                .pins
                .iter()
                .filter_map(|lp| {
                    old_pins
                        .iter()
                        .find(|op| op.number == lp.number)
                        .map(|op| (apply_all(&probe, lp.at), op.at))
                })
                .collect();
            // Try the shift each pair suggests; keep the best.
            for &(a, b) in pairs.iter().take(8) {
                let s = [b[0] - a[0], b[1] - a[1]];
                let hits = pairs
                    .iter()
                    .filter(|(x, y)| {
                        (x[0] + s[0] - y[0]).abs() < 1.0 && (x[1] + s[1] - y[1]).abs() < 1.0
                    })
                    .count();
                // Ties go to the placement that puts the rest nearest.
                let miss: f64 = pairs
                    .iter()
                    .map(|(x, y)| (x[0] + s[0] - y[0]).hypot(x[1] + s[1] - y[1]))
                    .sum();
                let score = (hits, miss);
                if best
                    .as_ref()
                    .is_none_or(|b| hits > b.0 .0 || (hits == b.0 .0 && miss < b.0 .1 - 0.01))
                {
                    best = Some((score, place_xfs(m, t, s)));
                }
            }
        }
        let xfs = match best {
            Some((_, x)) => x,
            None => {
                // No pin in common: line the boxes' corners up.
                let lq = src.parts.first().cloned().unwrap_or_default();
                place_xfs(false, 0, [q.min[0] - lq.min[0], q.max[1] - lq.max[1]])
            }
        };
        // What stays: the designator and the value as shown.
        let value = sym.value.clone().unwrap_or_default();
        let full = q.full_designator();
        let keep_text = |t: &SchText| {
            t.text == q.designator || t.text == full || (!value.is_empty() && t.text == value)
        };
        let keep: Vec<bool> = d
            .texts
            .iter()
            .map(|t| t.part != Some(me) || keep_text(t))
            .collect();
        d.retain_texts(&keep);
        d.shapes.retain(|s| s.part != Some(me));
        // Pins of this part are replaced in place, so later indices hold.
        let first_pin = d
            .pins
            .iter()
            .position(|p| p.part == me)
            .unwrap_or(d.pins.len());
        d.pins.retain(|p| p.part != me);
        let lib_des = src
            .parts
            .first()
            .map(|p| p.designator.clone())
            .unwrap_or_default();
        let lib_value = l.symbol.value.clone().unwrap_or_default();
        let drop_lib_text = |t: &SchText| {
            t.text == lib_des
                || t.text
                    .strip_prefix(lib_des.as_str())
                    .is_some_and(|r| r.len() == 1)
                || t.text == "*"
                || (!lib_value.is_empty() && t.text == lib_value)
                || t.text == l.name
        };
        for s in &src.shapes {
            let mut s = s.clone();
            for x in &xfs {
                x.geom(&mut s.geom);
            }
            s.part = Some(me);
            d.shapes.push(s);
        }
        for t in src.texts.iter().filter(|t| !drop_lib_text(t)) {
            let mut t = t.clone();
            for x in &xfs {
                x.text(&mut t);
            }
            t.part = Some(me);
            d.texts.push(t);
        }
        let new_pins: Vec<SchPin> = src
            .pins
            .iter()
            .map(|p| SchPin {
                at: apply_all(&xfs, p.at),
                base: apply_all(&xfs, p.base),
                part: me,
                ..p.clone()
            })
            .collect();
        // Wires on pins that moved follow them.
        for op in &old_pins {
            match new_pins.iter().find(|np| np.number == op.number) {
                Some(np)
                    if (np.at[0] - op.at[0]).abs() > 0.01 || (np.at[1] - op.at[1]).abs() > 0.01 =>
                {
                    let mut moved = false;
                    for w in 0..d.wires.len() {
                        let n = d.wires[w].len();
                        for end in [0, n.saturating_sub(1)] {
                            let p = d.wires[w][end];
                            if (p[0] - op.at[0]).abs() < 1.0 && (p[1] - op.at[1]).abs() < 1.0 {
                                let next = if end == 0 {
                                    d.wires[w].get(1)
                                } else {
                                    d.wires[w].get(n.wrapping_sub(2))
                                }
                                .copied();
                                d.wires[w][end] = np.at;
                                // Keep the wire square: a corner where it would slant.
                                if let Some(nx) = next {
                                    if (nx[0] - np.at[0]).abs() > 0.01
                                        && (nx[1] - np.at[1]).abs() > 0.01
                                    {
                                        let corner = if (nx[1] - op.at[1]).abs() < 0.01 {
                                            [np.at[0], nx[1]]
                                        } else {
                                            [nx[0], np.at[1]]
                                        };
                                        let at = if end == 0 { 1 } else { n - 1 };
                                        d.wires[w].insert(at, corner);
                                    }
                                }
                                moved = true;
                            }
                        }
                        d.sync_wire(w);
                    }
                    if moved {
                        rep.wires_moved += 1;
                    }
                }
                Some(_) => {}
                None => rep.lost_pins.push(format!("{}.{}", full, op.number)),
            }
        }
        let at = first_pin.min(d.pins.len());
        d.pins.splice(at..at, new_pins);
        // The new box.
        if let Some(lq) = src.parts.first() {
            let (a, b) = (apply_all(&xfs, lq.min), apply_all(&xfs, lq.max));
            let p = &mut d.parts[i];
            p.min = [a[0].min(b[0]), a[1].min(b[1])];
            p.max = [a[0].max(b[0]), a[1].max(b[1])];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_lists_lay_out_a_box() {
        let pins = parse_pin_list(
            "Pin,Name,Type,Side\n1\tVDD\tpwr\n2\tSCL\tin\n3\tSDA\tio\n4\tGND\tpower\tR\n5,INT,out,R\n# note\n",
        );
        assert_eq!(pins.len(), 5);
        assert_eq!(pins[0].kind, PinKind::Power);
        assert_eq!(pins[2].side, 'R');
        assert_eq!(pins[3].side, 'R');
        let d = SymbolDef::from_pin_list("SENSOR", &pins);
        assert_eq!(d.pins.len(), 5);
        assert!(d.problems().is_empty(), "{:?}", d.problems());
        // Left pins point left from x = 0, right ones right from the far edge.
        let vdd = &d.pins[0];
        assert_eq!((vdd.base[0], vdd.orient), (0.0, 2));
        assert_eq!(vdd.hot(), [-200.0, -100.0]);
        let (_, max) = d.bounds(1);
        assert!(d.pins[2].base[0] == max[0] - 200.0 && d.pins[2].orient == 0);
        // Ends on the 100 mil grid.
        for p in &d.pins {
            let h = p.hot();
            assert_eq!(h[0] % 100.0, 0.0);
            assert_eq!(h[1] % 100.0, 0.0);
        }
    }

    #[test]
    fn parts_duplicate_number_on_and_remove() {
        let mut d = SymbolDef::new("LM358");
        d.add_pin(1, [0.0, -100.0], 2);
        d.add_pin(1, [0.0, -300.0], 2);
        d.add_pin(1, [600.0, -200.0], 0);
        let mut v = SymPin::new(1, [300.0, 0.0], 1, "8", "V+");
        v.kind = PinKind::Power;
        d.pins.push(v);
        let b = d.duplicate_part(1);
        assert_eq!((b, d.parts), (2, 2));
        let nums: Vec<&str> = d
            .pins
            .iter()
            .filter(|p| p.part == 2)
            .map(|p| p.number.as_str())
            .collect();
        assert_eq!(nums, ["9", "10", "11", "8"]);
        // The shared supply pin is not a duplicate.
        assert!(d.problems().is_empty(), "{:?}", d.problems());
        d.pins[4].number = "1".into();
        assert!(d.problems()[0].contains("1"));
        d.remove_part(1);
        assert_eq!(d.parts, 1);
        assert!(d.pins.iter().all(|p| p.part == 1));
        assert_eq!(d.pins.len(), 4);
    }

    #[test]
    fn hits_moves_and_turns() {
        let mut d = SymbolDef::new("X");
        let p = d.add_pin(1, [0.0, -100.0], 2);
        assert_eq!(d.hit([-100.0, -100.0], 1, 20.0), Some(SymHit::Pin(p)));
        assert_eq!(d.hit([300.0, -200.0], 1, 20.0), Some(SymHit::Shape(0)));
        assert_eq!(d.hit([300.0, -200.0], 2, 20.0), None);
        d.move_hit(SymHit::Pin(p), [0.0, -100.0]);
        d.turn_hit(SymHit::Pin(p));
        assert_eq!(d.pins[p].hot(), [0.0, -400.0]);
    }

    #[test]
    fn power_pins_split_into_their_own_part() {
        let pins = parse_pin_list("1,VDD,pwr\n2,A,in\n3,Y,out\n4,GND,pwr\n");
        let mut d = SymbolDef::from_pin_list("GATE", &pins);
        assert_eq!(d.split_power_part(), Some(2));
        assert_eq!(d.pins.iter().filter(|p| p.part == 2).count(), 2);
        assert_eq!(d.pins.iter().filter(|p| p.part == 1).count(), 2);
        assert!(d.shapes.iter().any(|s| s.part == 2));
    }

    fn lib(at_dx: f64) -> LibSymbol {
        // A two-pin symbol: pins 1 left and 2 right of a box.
        let mut d = Drawing {
            parts: vec![SchPart {
                designator: "R?".into(),
                min: [-100.0, -50.0],
                max: [100.0 + at_dx, 50.0],
                ..Default::default()
            }],
            ..Default::default()
        };
        d.shapes.push(SchShape {
            geom: SchGeom::Rect {
                min: [-100.0, -50.0],
                max: [100.0 + at_dx, 50.0],
                r: [0.0, 0.0],
            },
            width: 10.0,
            stroke: Some(LINE_BLUE),
            fill: None,
            dashed: false,
            part: Some(0),
            wire: None,
            tag: None,
        });
        for (n, base, at) in [
            ("1", [-100.0, 0.0], [-200.0, 0.0]),
            ("2", [100.0 + at_dx, 0.0], [200.0 + at_dx, 0.0]),
        ] {
            d.pins.push(SchPin {
                at,
                base,
                part: 0,
                number: n.into(),
                ..Default::default()
            });
        }
        d.texts.push(SchText {
            at: [0.0, 60.0],
            text: "R?".into(),
            size: 100.0,
            color: [0, 0, 0],
            rot: 0,
            just: 0,
            bold: false,
            italic: false,
            wrap: None,
            part: Some(0),
        });
        LibSymbol {
            name: "RES".into(),
            part: 1,
            parts: 1,
            symbol: Symbol {
                designator: "R?".into(),
                library_ref: "RES".into(),
                ..Default::default()
            },
            drawing: d,
        }
    }

    #[test]
    fn placed_parts_redraw_from_the_library_where_they_stand() {
        let mut sheet = Sheet::default();
        let n = sheet
            .place_symbol(&lib(0.0), [1000.0, 1000.0], "R1")
            .unwrap();
        // Turned a quarter, with a wire on pin 2.
        sheet.drawing.turn_part(n);
        let p2 = sheet
            .drawing
            .pins
            .iter()
            .find(|p| p.number == "2")
            .unwrap()
            .at;
        sheet.drawing.add_wire(vec![p2, [p2[0], p2[1] + 300.0]]);
        let texts = sheet.drawing.texts.len();
        // The library's symbol grew 100 mil longer on pin 2's side.
        let rep = sheet.update_from_library(&[lib(100.0)]);
        assert_eq!(rep.updated, ["R1"]);
        assert!(rep.lost_pins.is_empty());
        let d = &sheet.drawing;
        let p1 = d.pins.iter().find(|p| p.number == "1").unwrap().at;
        let new2 = d.pins.iter().find(|p| p.number == "2").unwrap().at;
        // Pin 1 stays put; pin 2 is 100 mil further along the turned part.
        assert_eq!(new2[0], p2[0]);
        assert_eq!((new2[1] - p2[1]).abs(), 100.0);
        assert_eq!(p1[0], p2[0]);
        assert_eq!(rep.wires_moved, 1);
        assert_eq!(d.wires[0].first(), Some(&new2));
        // Still one net through the wire and pin 2.
        let net = d.nets.iter().find(|n| n.wires.contains(&0)).unwrap();
        assert_eq!(net.pins.len(), 1);
        // The designator text kept, the library's R? not added.
        assert_eq!(d.texts.len(), texts);
        assert!(d.texts.iter().any(|t| t.text == "R1"));
    }
}
