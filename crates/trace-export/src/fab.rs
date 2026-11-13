//! Fabrication outputs: Gerber X2 for every copper, mask, paste, silkscreen
//! and mechanical layer, the board outline, Excellon drill files (plated,
//! non-plated, and one per blind or buried span), an IPC-D-356A test
//! netlist, a pick-and-place file and a bill of materials.
//!
//! Gerber coordinates are millimetres with six decimals, so a board's
//! nanometre coordinates are written exactly. Copper objects carry X2 net,
//! part and pin attributes, so the files read back with their nets.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use trace_core::{Board, LayerKind, LayerRef, Length, Pad, PadShape, Point, Side};

/// One output file.
#[derive(Clone, Debug)]
pub struct FabFile {
    pub name: String,
    pub data: Vec<u8>,
}

/// Choices for the outputs; [`FabOptions::from_board`] takes them from the
/// board's rules.
#[derive(Clone, Debug, PartialEq)]
pub struct FabOptions {
    /// How far solder mask openings grow past the copper of a pad.
    pub mask_expansion: Length,
    /// How far paste openings grow past the copper of a surface pad
    /// (negative shrinks them).
    pub paste_expansion: Length,
    /// Vias covered by mask on the top and bottom side.
    pub tent_top: bool,
    pub tent_bottom: bool,
}

impl Default for FabOptions {
    fn default() -> Self {
        FabOptions {
            mask_expansion: Length::from_mil(2.0),
            paste_expansion: Length(0),
            tent_top: true,
            tent_bottom: true,
        }
    }
}

impl FabOptions {
    /// Mask and paste expansion and via tenting from the board's
    /// board-wide rules (Altium `SolderMaskExpansion` and
    /// `PasteMaskExpansion`), defaults where it has none.
    pub fn from_board(board: &Board) -> FabOptions {
        let mut o = FabOptions::default();
        let m = trace_core::drc::mask_rule(board);
        o.mask_expansion = m.expansion;
        o.tent_top = m.tent_top;
        o.tent_bottom = m.tent_bottom;
        let rule = |kind: &str| {
            board
                .rules
                .iter()
                .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case(kind))
                .filter(|r| {
                    r.scope
                        .first()
                        .is_none_or(|s| s.eq_ignore_ascii_case("All"))
                })
                .min_by_key(|r| r.priority)
        };
        if let Some(r) = rule("PasteMaskExpansion") {
            if let Some(e) = r.params.get("EXPANSION").and_then(|v| Length::parse(v)) {
                o.paste_expansion = e;
            }
        }
        o
    }
}

/// Altium layer numbers of the paste and mask layers, which importers keep
/// as `LayerRef::Other` for primitives drawn straight on them.
const TOP_PASTE: u8 = 35;
const BOTTOM_PASTE: u8 = 36;
const TOP_MASK: u8 = 37;
const BOTTOM_MASK: u8 = 38;

/// Every fabrication file for the board, named `<stem>.<ext>` the way
/// Altium names them (GTL, G1…, GBL, GTS, GTO, GKO, …).
pub fn fab_files(board: &Board, stem: &str, opt: &FabOptions) -> Vec<FabFile> {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let mut out = Vec::new();
    let mut put = |ext: String, data: Vec<u8>| {
        out.push(FabFile {
            name: format!("{stem}.{ext}"),
            data,
        })
    };
    for l in 0..n {
        let ext = if l == 0 {
            "GTL".to_string()
        } else if l == n - 1 {
            "GBL".to_string()
        } else {
            format!("G{l}")
        };
        put(ext, copper(board, l, n).into_bytes());
    }
    for side in [Side::Top, Side::Bottom] {
        let t = side == Side::Top;
        put(
            if t { "GTS" } else { "GBS" }.into(),
            mask(board, side, n, opt).into_bytes(),
        );
        put(
            if t { "GTP" } else { "GBP" }.into(),
            paste(board, side, n, opt).into_bytes(),
        );
        put(
            if t { "GTO" } else { "GBO" }.into(),
            silk(board, side).into_bytes(),
        );
    }
    if board.geometry.outline.len() >= 3 {
        put("GKO".into(), outline(board).into_bytes());
    }
    for (m, text) in mechanical(board) {
        put(format!("GM{m}"), text.into_bytes());
    }
    for (name, text) in drills(board, n) {
        out.push(FabFile {
            name: format!("{stem}-{name}.DRL"),
            data: text.into_bytes(),
        });
    }
    let mut put = |ext: &str, data: String| {
        out.push(FabFile {
            name: format!("{stem}{ext}"),
            data: data.into_bytes(),
        })
    };
    put(".ipc", ipc356(board, stem, n));
    put("-PickPlace.csv", pick_place(board));
    put("-BOM.csv", bom(board));
    out
}

/// The files packed in one zip archive.
pub fn fab_zip(files: &[FabFile]) -> std::io::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for f in files {
        w.start_file(f.name.as_str(), o)
            .map_err(std::io::Error::other)?;
        w.write_all(&f.data)?;
    }
    Ok(w.finish().map_err(std::io::Error::other)?.into_inner())
}

// ---------------------------------------------------------------- Gerber

/// A coordinate in nm as Gerber 4.6 mm: the nm value itself.
fn c(v: i64) -> String {
    v.to_string()
}

/// A size in nm as decimal mm, for aperture definitions.
fn mm(v: i64) -> String {
    let s = format!("{:.6}", v as f64 / 1e6);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

/// Text of an X2 attribute field, with the reserved characters escaped.
fn field(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' | '*' | ',' | '\\' => {
                let _ = write!(o, "\\u{:04X}", ch as u32);
            }
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Ap {
    Circle(i64),
    Rect(i64, i64),
    Obround(i64, i64),
    /// A polygon around the flash point, grown by the second value (as
    /// mask openings are) with round corners.
    Outline(Vec<(i64, i64)>, i64),
    /// A round-ended bar between two points around the flash point.
    Bar((i64, i64), (i64, i64), i64),
}

/// Who a copper object belongs to, for writers that list nets by object
/// (ODB++ EDA data).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    /// A pad, by index in the board's pads.
    Pad(usize),
    /// A via, by index.
    Via(usize),
    /// A track or arc of a net.
    Trace(Option<u32>),
    /// A copper area of a net.
    Plane(Option<u32>),
}

/// Where a layer's objects are drawn: a Gerber file or an ODB++ layer.
pub(crate) trait Plot {
    fn attrs(&mut self, net: &str, pin: &str);
    fn owner(&mut self, _o: Owner) {}
    fn flash(&mut self, ap: Ap, function: &'static str, at: Point);
    fn line(&mut self, d: i64, function: &'static str, a: Point, b: Point);
    fn arc(&mut self, d: i64, function: &'static str, a: &trace_core::Arc);
    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]);
    /// The conductors (tracks and arcs) follow.
    fn conductors(&mut self) {}
}

impl Plot for Gerber {
    fn attrs(&mut self, net: &str, pin: &str) {
        Gerber::attrs(self, net, pin)
    }
    fn flash(&mut self, ap: Ap, function: &'static str, at: Point) {
        Gerber::flash(self, ap, function, at)
    }
    fn line(&mut self, d: i64, function: &'static str, a: Point, b: Point) {
        Gerber::line(self, d, function, a, b)
    }
    fn arc(&mut self, d: i64, function: &'static str, a: &trace_core::Arc) {
        Gerber::arc(self, d, function, a)
    }
    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        Gerber::polygon(self, outline, holes)
    }
    fn conductors(&mut self) {
        self.body.push_str("%TA.AperFunction,Conductor*%\n");
    }
}

#[derive(Default)]
struct Gerber {
    head: String,
    body: String,
    aps: Vec<(Ap, &'static str)>,
    index: HashMap<(Ap, &'static str), usize>,
    current: Option<usize>,
    at: Option<(i64, i64)>,
    attrs: (String, String),
    /// Object attributes are written (copper layers).
    x2_objects: bool,
}

impl Gerber {
    fn new(function: &str, comment: &str, x2_objects: bool) -> Gerber {
        let mut head = String::new();
        let _ = writeln!(head, "G04 {comment}*");
        let _ = writeln!(
            head,
            "%TF.GenerationSoftware,Trace,Trace,{}*%",
            env!("CARGO_PKG_VERSION")
        );
        head.push_str("%TF.SameCoordinates,Original*%\n");
        let _ = writeln!(head, "%TF.FileFunction,{function}*%");
        head.push_str("%TF.FilePolarity,Positive*%\n");
        head.push_str("%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG75*\nG01*\n");
        Gerber {
            head,
            x2_objects,
            ..Default::default()
        }
    }

    fn select(&mut self, ap: Ap, function: &'static str) {
        let key = (ap, function);
        let i = match self.index.get(&key) {
            Some(&i) => i,
            None => {
                self.aps.push(key.clone());
                self.index.insert(key, self.aps.len() - 1);
                self.aps.len() - 1
            }
        };
        if self.current != Some(i) {
            let _ = writeln!(self.body, "D{}*", i + 10);
            self.current = Some(i);
        }
    }

    /// Sets the X2 object attributes for the objects that follow: the net
    /// and, for a pad, `part,pin`.
    fn attrs(&mut self, net: &str, pin: &str) {
        if !self.x2_objects || (self.attrs.0 == net && self.attrs.1 == pin) {
            return;
        }
        if !self.attrs.0.is_empty() || !self.attrs.1.is_empty() {
            self.body.push_str("%TD*%\n");
        }
        if !net.is_empty() {
            let _ = writeln!(self.body, "%TO.N,{}*%", field(net));
        }
        if let Some((part, p)) = pin.split_once('\u{1}') {
            let _ = writeln!(self.body, "%TO.P,{},{}*%", field(part), field(p));
            let _ = writeln!(self.body, "%TO.C,{}*%", field(part));
        }
        self.attrs = (net.to_string(), pin.to_string());
    }

    fn flash(&mut self, ap: Ap, function: &'static str, at: Point) {
        self.select(ap, function);
        let _ = writeln!(self.body, "X{}Y{}D03*", c(at.x.0), c(at.y.0));
        self.at = Some((at.x.0, at.y.0));
    }

    fn line(&mut self, d: i64, function: &'static str, a: Point, b: Point) {
        self.select(Ap::Circle(d.max(1)), function);
        if self.at != Some((a.x.0, a.y.0)) {
            let _ = writeln!(self.body, "X{}Y{}D02*", c(a.x.0), c(a.y.0));
        }
        let _ = writeln!(self.body, "X{}Y{}D01*", c(b.x.0), c(b.y.0));
        self.at = Some((b.x.0, b.y.0));
    }

    /// A counter-clockwise arc; equal start and end angle a full circle.
    fn arc(&mut self, d: i64, function: &'static str, a: &trace_core::Arc) {
        let sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        let full = sweep < 1e-9;
        if full && (a.end_deg - a.start_deg).abs() < 1.0 {
            return;
        }
        let r = a.radius.0 as f64;
        let at = |deg: f64| {
            let (s, co) = deg.to_radians().sin_cos();
            (
                a.center.x.0 + (r * co).round() as i64,
                a.center.y.0 + (r * s).round() as i64,
            )
        };
        let s = at(a.start_deg);
        let e = if full { s } else { at(a.end_deg) };
        self.select(Ap::Circle(d.max(1)), function);
        if self.at != Some(s) {
            let _ = writeln!(self.body, "X{}Y{}D02*", c(s.0), c(s.1));
        }
        let _ = writeln!(
            self.body,
            "G03X{}Y{}I{}J{}D01*\nG01*",
            c(e.0),
            c(e.1),
            c(a.center.x.0 - s.0),
            c(a.center.y.0 - s.1)
        );
        self.at = Some(e);
    }

    /// A filled area: one contour, holes already joined in.
    fn region(&mut self, pts: &[(i64, i64)]) {
        if pts.len() < 3 {
            return;
        }
        self.body.push_str("G36*\n");
        let _ = writeln!(self.body, "X{}Y{}D02*", c(pts[0].0), c(pts[0].1));
        for p in &pts[1..] {
            let _ = writeln!(self.body, "X{}Y{}D01*", c(p.0), c(p.1));
        }
        let _ = writeln!(self.body, "X{}Y{}D01*", c(pts[0].0), c(pts[0].1));
        self.body.push_str("G37*\n");
        self.at = Some(pts[0]);
    }

    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        let f = trace_core::fracture(outline, holes);
        let pts: Vec<(i64, i64)> = f
            .iter()
            .map(|p| ((p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64))
            .collect();
        self.region(&pts);
    }

    fn finish(mut self) -> String {
        let mut defs = String::new();
        for (i, (ap, function)) in self.aps.iter().enumerate() {
            let d = i + 10;
            if !function.is_empty() {
                let _ = writeln!(defs, "%TA.AperFunction,{function}*%");
            }
            match ap {
                Ap::Circle(v) => {
                    let _ = writeln!(defs, "%ADD{d}C,{}*%", mm(*v));
                }
                Ap::Rect(w, h) => {
                    let _ = writeln!(defs, "%ADD{d}R,{}X{}*%", mm(*w), mm(*h));
                }
                Ap::Obround(w, h) => {
                    let _ = writeln!(defs, "%ADD{d}O,{}X{}*%", mm(*w), mm(*h));
                }
                Ap::Outline(pts, grow) => {
                    let _ = write!(defs, "%AMP{d}*\n4,1,{}", pts.len());
                    for p in pts.iter().chain(pts.first()) {
                        let _ = write!(defs, ",{},{}", mm(p.0), mm(p.1));
                    }
                    defs.push_str(",0*\n");
                    if *grow > 0 {
                        let w = mm(2 * grow);
                        for (k, p) in pts.iter().enumerate() {
                            let q = pts[(k + 1) % pts.len()];
                            let _ = writeln!(
                                defs,
                                "20,1,{w},{},{},{},{},0*\n1,1,{w},{},{},0*",
                                mm(p.0),
                                mm(p.1),
                                mm(q.0),
                                mm(q.1),
                                mm(p.0),
                                mm(p.1)
                            );
                        }
                    }
                    defs.push_str("%\n");
                    let _ = writeln!(defs, "%ADD{d}P{d}*%");
                }
                Ap::Bar(a, b, w) => {
                    let w = mm(*w);
                    let _ = writeln!(
                        defs,
                        "%AMP{d}*\n1,1,{w},{},{},0*\n1,1,{w},{},{},0*\n20,1,{w},{},{},{},{},0*%",
                        mm(a.0),
                        mm(a.1),
                        mm(b.0),
                        mm(b.1),
                        mm(a.0),
                        mm(a.1),
                        mm(b.0),
                        mm(b.1)
                    );
                    let _ = writeln!(defs, "%ADD{d}P{d}*%");
                }
            }
        }
        if !self.aps.is_empty() {
            defs.push_str("%TD*%\n");
        }
        if self.x2_objects && (!self.attrs.0.is_empty() || !self.attrs.1.is_empty()) {
            self.body.push_str("%TD*%\n");
        }
        let mut out = self.head;
        out.push_str(&defs);
        out.push_str(&self.body);
        out.push_str("M02*\n");
        out
    }
}

/// The copper of a pad on one layer: shape, sizes, rotation, outline.
struct Land<'a> {
    shape: PadShape,
    sx: i64,
    sy: i64,
    rot: f64,
    outline: &'a [Point],
}

/// The pad's copper on copper layer `l`, if it has any there.
fn land_on(p: &Pad, l: u8, n: u8) -> Option<Land<'_>> {
    let own = || Land {
        shape: p.shape,
        sx: p.size_x.0,
        sy: p.size_y.0,
        rot: p.rotation_deg,
        outline: &p.outline,
    };
    match p.layer {
        LayerRef::Copper(i) if i == l && i < n => Some(own()),
        LayerRef::AllCopper if p.stack.is_empty() => Some(own()),
        LayerRef::AllCopper => p.stack.iter().find(|s| s.layer == l).map(|s| Land {
            shape: s.shape,
            sx: s.size_x.0,
            sy: s.size_y.0,
            rot: s.rotation_deg,
            outline: &s.outline,
        }),
        _ => None,
    }
}

/// A pad drawn on a mask or paste layer as it is.
fn own_land(p: &Pad) -> Land<'_> {
    Land {
        shape: p.shape,
        sx: p.size_x.0,
        sy: p.size_y.0,
        rot: p.rotation_deg,
        outline: &p.outline,
    }
}

fn turn(x: f64, y: f64, deg: f64) -> (i64, i64) {
    let (s, co) = deg.to_radians().sin_cos();
    (
        (x * co - y * s).round() as i64,
        (x * s + y * co).round() as i64,
    )
}

/// The aperture that flashes `land` at `at`, grown by `grow` all round
/// (negative shrinks; outlines are not shrunk).
fn aperture(land: &Land, at: Point, grow: i64) -> Option<Ap> {
    if !land.outline.is_empty() {
        let pts: Vec<(i64, i64)> = land
            .outline
            .iter()
            .map(|q| (q.x.0 - at.x.0, q.y.0 - at.y.0))
            .collect();
        if pts.len() < 3 {
            return None;
        }
        if grow == 0 {
            return Some(Ap::Outline(pts, 0));
        }
        // Convex outlines (rounded rectangles) move their edges; others
        // grow with round corners and cannot shrink.
        return match offset_convex(&pts, grow as f64) {
            Some(q) if q.len() < 3 => None,
            Some(q) => Some(Ap::Outline(q, 0)),
            None if grow > 0 => Some(Ap::Outline(pts, grow)),
            None => Some(Ap::Outline(pts, 0)),
        };
    }
    let (w, h) = (land.sx + 2 * grow, land.sy + 2 * grow);
    if w <= 0 || h <= 0 {
        return None;
    }
    let r = land.rot.rem_euclid(360.0);
    let quarter = (r / 90.0).round();
    let square = (r - quarter * 90.0).abs() < 1e-6;
    let (aw, ah) = if square && quarter as i64 % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    Some(match land.shape {
        PadShape::Round if w == h => Ap::Circle(w),
        PadShape::Round if square => Ap::Obround(aw, ah),
        PadShape::Round => {
            let (long, d) = (w.max(h), w.min(h));
            let half = (long - d) as f64 / 2.0;
            let (x, y) = if w >= h { (half, 0.0) } else { (0.0, half) };
            let a = turn(x, y, r);
            Ap::Bar(a, (-a.0, -a.1), d)
        }
        PadShape::Rect if square => Ap::Rect(aw, ah),
        PadShape::Rect => {
            let (x, y) = (w as f64 / 2.0, h as f64 / 2.0);
            let pts = [(x, y), (-x, y), (-x, -y), (x, -y)]
                .iter()
                .map(|&(u, v)| turn(u, v, r))
                .collect();
            Ap::Outline(pts, 0)
        }
        PadShape::Octagon => {
            let (x, y) = (w as f64 / 2.0, h as f64 / 2.0);
            let k = 0.2929 * w.min(h) as f64;
            let pts = [
                (x, y - k),
                (x - k, y),
                (-x + k, y),
                (-x, y - k),
                (-x, -y + k),
                (-x + k, -y),
                (x - k, -y),
                (x, -y + k),
            ]
            .iter()
            .map(|&(u, v)| turn(u, v, r))
            .collect();
            Ap::Outline(pts, 0)
        }
    })
}

/// A convex outline with every edge moved out by `d` (in by a negative
/// `d`); `None` when the outline is not convex. An outline shrunk to
/// nothing comes back empty.
fn offset_convex(pts: &[(i64, i64)], d: f64) -> Option<Vec<(i64, i64)>> {
    let mut v: Vec<[f64; 2]> = pts.iter().map(|p| [p.0 as f64, p.1 as f64]).collect();
    v.dedup();
    if v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    let n = v.len();
    if n < 3 {
        return None;
    }
    let area: f64 = (0..n)
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0;
    if area < 0.0 {
        v.reverse();
    }
    // Convex: every turn the same way (counter-clockwise now).
    for i in 0..n {
        let (a, b, c) = (v[i], v[(i + 1) % n], v[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross < -1e-3 * ((b[0] - a[0]).hypot(b[1] - a[1]) * (c[0] - b[0]).hypot(c[1] - b[1])) {
            return None;
        }
    }
    // Each edge's outward normal; corners where moved edges meet.
    let normal = |a: [f64; 2], b: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy).max(1e-9);
        [dy / l, -dx / l]
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (p, a, b) = (v[(i + n - 1) % n], v[i], v[(i + 1) % n]);
        let (n1, n2) = (normal(p, a), normal(a, b));
        let m = [n1[0] + n2[0], n1[1] + n2[1]];
        let k = (1.0 + n1[0] * n2[0] + n1[1] * n2[1]).max(1e-6);
        out.push((
            (a[0] + d * m[0] / k).round() as i64,
            (a[1] + d * m[1] / k).round() as i64,
        ));
    }
    // Shrunk past nothing: the outline turned inside out.
    let shrunk: f64 = (0..n)
        .map(|i| {
            let (a, b) = (out[i], out[(i + 1) % n]);
            a.0 as f64 * b.1 as f64 - b.0 as f64 * a.1 as f64
        })
        .sum();
    if d < 0.0 && shrunk <= 0.0 {
        return Some(Vec::new());
    }
    Some(out)
}

fn net_name(board: &Board, net: Option<u32>) -> &str {
    net.and_then(|n| board.nets.get(n as usize))
        .map(|n| n.name.as_str())
        .unwrap_or("")
}

fn pin_key(board: &Board, p: &Pad) -> String {
    match p.component.and_then(|c| board.components.get(c as usize)) {
        Some(c) if !p.name.is_empty() => format!("{}\u{1}{}", c.designator, p.name),
        _ => String::new(),
    }
}

fn copper(board: &Board, l: u8, n: u8) -> String {
    let function = if l == 0 {
        "Copper,L1,Top".to_string()
    } else if l == n - 1 {
        format!("Copper,L{},Bot", l + 1)
    } else {
        format!("Copper,L{},Inr", l + 1)
    };
    let name = board
        .stackup
        .copper_layers()
        .nth(l as usize)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("Copper {}", l + 1));
    let mut g = Gerber::new(&function, &format!("Copper layer {}: {name}", l + 1), true);
    plot_copper(&mut g, board, l, n);
    g.finish()
}

/// The copper of layer `l` of `n`: areas, tracks, arcs, pads and vias.
pub(crate) fn plot_copper(g: &mut dyn Plot, board: &Board, l: u8, n: u8) {
    let geo = &board.geometry;
    let on = LayerRef::Copper(l);
    let mut polys: Vec<_> = geo.polygons.iter().filter(|p| p.layer == on).collect();
    polys.sort_by_key(|p| p.net);
    for p in polys {
        g.attrs(net_name(board, p.net), "");
        g.owner(Owner::Plane(p.net));
        g.polygon(&p.outline, &p.holes);
    }
    g.conductors();
    let mut tracks: Vec<_> = geo.tracks.iter().filter(|t| t.layer == on).collect();
    tracks.sort_by_key(|t| t.net);
    for t in tracks {
        g.attrs(net_name(board, t.net), "");
        g.owner(Owner::Trace(t.net));
        g.line(t.width.0, "Conductor", t.start, t.end);
    }
    let mut arcs: Vec<_> = geo.arcs.iter().filter(|a| a.layer == on).collect();
    arcs.sort_by_key(|a| a.net);
    for a in arcs {
        g.attrs(net_name(board, a.net), "");
        g.owner(Owner::Trace(a.net));
        g.arc(a.width.0, "Conductor", a);
    }
    for (i, p) in geo.pads.iter().enumerate() {
        let Some(land) = land_on(p, l, n) else {
            continue;
        };
        let Some(ap) = aperture(&land, p.at, 0) else {
            continue;
        };
        let function = if p.hole.0 > 0 {
            "ComponentPad"
        } else {
            "SMDPad,CuDef"
        };
        g.attrs(net_name(board, p.net), &pin_key(board, p));
        g.owner(Owner::Pad(i));
        g.flash(ap, function, p.at);
    }
    for (i, v) in geo.vias.iter().enumerate() {
        if l < v.from_layer || l > v.to_layer {
            continue;
        }
        let d = v
            .rings
            .iter()
            .find(|r| r.0 == l)
            .map(|r| r.1)
            .unwrap_or(v.diameter);
        if !v.rings.is_empty() && !v.rings.iter().any(|r| r.0 == l) || d.0 <= v.hole.0 {
            continue;
        }
        g.attrs(net_name(board, v.net), "");
        g.owner(Owner::Via(i));
        g.flash(Ap::Circle(d.0), "ViaPad", v.at);
    }
}

fn mask(board: &Board, side: Side, n: u8, opt: &FabOptions) -> String {
    let top = side == Side::Top;
    let mut g = Gerber::new(
        if top {
            "Soldermask,Top"
        } else {
            "Soldermask,Bot"
        },
        "Solder mask openings",
        false,
    );
    plot_mask(&mut g, board, side, n, opt);
    g.finish()
}

/// Solder mask openings on one side.
pub(crate) fn plot_mask(g: &mut dyn Plot, board: &Board, side: Side, n: u8, opt: &FabOptions) {
    let top = side == Side::Top;
    let l = if top { 0 } else { n - 1 };
    let only = LayerRef::Other(if top { TOP_MASK } else { BOTTOM_MASK });
    let geo = &board.geometry;
    for p in &geo.pads {
        if p.layer == only {
            if let Some(ap) = aperture(&own_land(p), p.at, 0) {
                g.flash(ap, "", p.at);
            }
            continue;
        }
        if if top {
            p.fab.tent_top
        } else {
            p.fab.tent_bottom
        } {
            continue;
        }
        let e = p.fab.mask.unwrap_or(opt.mask_expansion).0;
        if let Some(ap) = land_on(p, l, n).and_then(|land| aperture(&land, p.at, e)) {
            g.flash(ap, "", p.at);
        }
    }
    for v in &geo.vias {
        let reaches = if top {
            v.from_layer == 0
        } else {
            v.to_layer == n - 1
        };
        let tented = if top {
            v.fab.tent_top || opt.tent_top
        } else {
            v.fab.tent_bottom || opt.tent_bottom
        };
        if reaches && !tented {
            let e = v.fab.mask.unwrap_or(opt.mask_expansion).0;
            g.flash(Ap::Circle(v.diameter.0 + 2 * e), "", v.at);
        }
    }
    draw_layer(g, board, &only);
}

fn paste(board: &Board, side: Side, n: u8, opt: &FabOptions) -> String {
    let top = side == Side::Top;
    let mut g = Gerber::new(
        if top { "Paste,Top" } else { "Paste,Bot" },
        "Solder paste",
        false,
    );
    plot_paste(&mut g, board, side, n, opt);
    g.finish()
}

/// Solder paste openings on one side.
pub(crate) fn plot_paste(g: &mut dyn Plot, board: &Board, side: Side, n: u8, opt: &FabOptions) {
    let top = side == Side::Top;
    let l = if top { 0 } else { n - 1 };
    let only = LayerRef::Other(if top { TOP_PASTE } else { BOTTOM_PASTE });
    for p in &board.geometry.pads {
        if p.layer == only {
            if let Some(ap) = aperture(&own_land(p), p.at, 0) {
                g.flash(ap, "", p.at);
            }
            continue;
        }
        if p.hole.0 > 0 || p.layer != LayerRef::Copper(l) || p.fab.no_paste {
            continue;
        }
        let e = p.fab.paste.unwrap_or(opt.paste_expansion).0;
        if let Some(ap) = aperture(&own_land(p), p.at, e) {
            g.flash(ap, "", p.at);
        }
    }
    draw_layer(g, board, &only);
}

/// Pads, tracks, arcs and areas drawn on a non-copper layer.
pub(crate) fn plot_other(g: &mut dyn Plot, board: &Board, layer: &LayerRef) {
    for p in board.geometry.pads.iter().filter(|p| &p.layer == layer) {
        if let Some(ap) = aperture(&own_land(p), p.at, 0) {
            g.flash(ap, "", p.at);
        }
    }
    draw_layer(g, board, layer);
}

/// Tracks, arcs and areas drawn on a non-copper layer.
fn draw_layer(g: &mut dyn Plot, board: &Board, layer: &LayerRef) {
    let geo = &board.geometry;
    for p in geo
        .polygons
        .iter()
        .chain(&geo.areas)
        .filter(|p| &p.layer == layer)
    {
        g.polygon(&p.outline, &p.holes);
    }
    for t in geo.tracks.iter().filter(|t| &t.layer == layer) {
        g.line(t.width.0, "", t.start, t.end);
    }
    for a in geo.arcs.iter().filter(|a| &a.layer == layer) {
        g.arc(a.width.0, "", a);
    }
}

fn silk(board: &Board, side: Side) -> String {
    let top = side == Side::Top;
    let mut g = Gerber::new(
        if top { "Legend,Top" } else { "Legend,Bot" },
        "Silkscreen",
        false,
    );
    let layer = if top {
        LayerRef::TopSilk
    } else {
        LayerRef::BottomSilk
    };
    plot_other(&mut g, board, &layer);
    g.finish()
}

fn outline(board: &Board) -> String {
    let mut g = Gerber::new("Profile,NP", "Board outline", false);
    let w = Length::from_mm(0.1).0;
    // The outline, then the openings milled inside it.
    for o in std::iter::once(&board.geometry.outline).chain(&board.geometry.slots) {
        for (i, a) in o.iter().enumerate() {
            let b = o[(i + 1) % o.len()];
            if *a != b {
                g.line(w, "Profile", *a, b);
            }
        }
    }
    g.finish()
}

/// Mechanical layers that have something on them, by number.
fn mechanical(board: &Board) -> Vec<(u8, String)> {
    mechanical_layers(board)
        .into_iter()
        .map(|m| {
            let mut g = Gerber::new(
                &format!("Other,Mechanical {m}"),
                &format!("Mechanical layer {m}"),
                false,
            );
            plot_other(&mut g, board, &LayerRef::Mechanical(m));
            (m, g.finish())
        })
        .collect()
}

/// The numbers of the mechanical layers that have something on them.
pub(crate) fn mechanical_layers(board: &Board) -> Vec<u8> {
    let geo = &board.geometry;
    let mut used: Vec<u8> = geo
        .tracks
        .iter()
        .map(|t| &t.layer)
        .chain(geo.arcs.iter().map(|a| &a.layer))
        .chain(geo.polygons.iter().chain(&geo.areas).map(|p| &p.layer))
        .filter_map(|l| match l {
            LayerRef::Mechanical(m) => Some(*m),
            _ => None,
        })
        .collect();
    used.sort_unstable();
    used.dedup();
    used
}

// ---------------------------------------------------------------- Drill

/// Whether a pad's hole is plated.
pub(crate) fn plated(p: &Pad) -> bool {
    !p.fab.unplated
}

/// Drill files: plated through holes, non-plated holes, and one file per
/// blind or buried via span, as (name, text).
fn drills(board: &Board, n: u8) -> Vec<(String, String)> {
    // (from, to, plated) -> holes as (diameter, at, via, slot ends)
    type Hole = (i64, Point, bool, Option<(Point, Point)>);
    let mut sets: BTreeMap<(u8, u8, bool), Vec<Hole>> = BTreeMap::new();
    for p in &board.geometry.pads {
        if p.hole.0 > 0 {
            let slot = (p.fab.slot.0 > p.hole.0).then(|| {
                let half = (p.fab.slot.0 - p.hole.0) as f64 / 2.0;
                let (s, c) = p.fab.slot_deg.to_radians().sin_cos();
                let d = ((half * c).round() as i64, (half * s).round() as i64);
                let at = |k: i64| Point {
                    x: Length(p.at.x.0 + k * d.0),
                    y: Length(p.at.y.0 + k * d.1),
                };
                (at(-1), at(1))
            });
            sets.entry((0, n - 1, plated(p)))
                .or_default()
                .push((p.hole.0, p.at, false, slot));
        }
    }
    for v in &board.geometry.vias {
        if v.hole.0 > 0 {
            let (a, b) = (v.from_layer.min(n - 1), v.to_layer.min(n - 1));
            sets.entry((a, b, !v.fab.unplated))
                .or_default()
                .push((v.hole.0, v.at, true, None));
        }
    }
    let mut out = Vec::new();
    for ((a, b, pl), mut holes) in sets {
        let (name, function) = if !pl {
            ("NPTH".to_string(), format!("NonPlated,1,{n},NPTH"))
        } else if a == 0 && b == n - 1 {
            ("PTH".to_string(), format!("Plated,1,{n},PTH"))
        } else {
            let kind = if a == 0 || b == n - 1 {
                "Blind"
            } else {
                "Buried"
            };
            (
                format!("L{}-L{}", a + 1, b + 1),
                format!("Plated,{},{},{kind}", a + 1, b + 1),
            )
        };
        holes.sort_by_key(|h| (h.0, h.1.x.0, h.1.y.0));
        let mut tools: Vec<(i64, bool)> = holes.iter().map(|h| (h.0, h.2)).collect();
        let mm4 = |v: i64| format!("{:.4}", v as f64 / 1e6);
        tools.dedup_by_key(|t| t.0);
        let mut t = String::new();
        t.push_str("M48\n");
        let _ = writeln!(
            t,
            "; #@! TF.GenerationSoftware,Trace,Trace,{}",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(t, "; #@! TF.FileFunction,{function}");
        t.push_str("; #@! TF.FilePolarity,Positive\n");
        t.push_str("; FORMAT={-:-/ absolute / metric / decimal}\n");
        t.push_str("FMAT,2\nMETRIC\n");
        for (i, (d, via)) in tools.iter().enumerate() {
            let f = match (pl, via) {
                (false, _) => "ComponentDrill",
                (true, true) => "ViaDrill",
                (true, false) => "ComponentDrill",
            };
            let _ = writeln!(
                t,
                "; #@! TA.AperFunction,{},{f}",
                if pl { "Plated,PTH" } else { "NonPlated,NPTH" }
            );
            let _ = writeln!(t, "T{}C{:.4}", i + 1, *d as f64 / 1e6);
        }
        t.push_str("%\nG90\nG05\n");
        for (i, (d, _)) in tools.iter().enumerate() {
            let _ = writeln!(t, "T{}", i + 1);
            for h in holes.iter().filter(|h| h.0 == *d) {
                match h.3 {
                    // A slot: drilled from one end and routed to the other.
                    Some((a, b)) => {
                        let _ = writeln!(
                            t,
                            "X{}Y{}G85X{}Y{}",
                            mm4(a.x.0),
                            mm4(a.y.0),
                            mm4(b.x.0),
                            mm4(b.y.0)
                        );
                    }
                    None => {
                        let _ = writeln!(t, "X{}Y{}", mm4(h.1.x.0), mm4(h.1.y.0));
                    }
                }
            }
        }
        t.push_str("M30\n");
        out.push((name, t));
    }
    out.extend(backdrills(board, n));
    out
}

/// One drill file per backdrill: side and the layer it must not cut. The
/// file names the copper layers drilled through; the comment names the
/// layer that stays.
fn backdrills(board: &Board, n: u8) -> Vec<(String, String)> {
    // (from the top, keep) -> (diameter, at)
    let mut sets: BTreeMap<(bool, u8), Vec<(i64, Point)>> = BTreeMap::new();
    for v in &board.geometry.vias {
        if let Some(b) = v.fab.back_top {
            sets.entry((true, b.keep))
                .or_default()
                .push((b.diameter.0, v.at));
        }
        if let Some(b) = v.fab.back_bottom {
            sets.entry((false, b.keep))
                .or_default()
                .push((b.diameter.0, v.at));
        }
    }
    let layer_name = |l: u8| {
        board
            .stackup
            .copper_layers()
            .nth(l as usize)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("L{}", l + 1))
    };
    let mut out = Vec::new();
    for ((top, keep), mut holes) in sets {
        // Copper layers drilled through, 1-based.
        let (a, b) = if top {
            (1, keep as u32)
        } else {
            (keep as u32 + 2, n as u32)
        };
        if a > b {
            continue;
        }
        holes.sort_by_key(|h| (h.0, h.1.x.0, h.1.y.0));
        let mut tools: Vec<i64> = holes.iter().map(|h| h.0).collect();
        tools.dedup();
        let mm4 = |v: i64| format!("{:.4}", v as f64 / 1e6);
        let mut t = String::new();
        t.push_str("M48\n");
        let _ = writeln!(
            t,
            "; #@! TF.GenerationSoftware,Trace,Trace,{}",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(t, "; #@! TF.FileFunction,NonPlated,{a},{b},Blind");
        t.push_str("; #@! TF.FilePolarity,Positive\n");
        let _ = writeln!(
            t,
            "; Backdrill from the {} through L{a}-L{b}; must not cut L{} ({})",
            if top { "top" } else { "bottom" },
            keep + 1,
            layer_name(keep)
        );
        t.push_str("; FORMAT={-:-/ absolute / metric / decimal}\n");
        t.push_str("FMAT,2\nMETRIC\n");
        for (i, d) in tools.iter().enumerate() {
            t.push_str("; #@! TA.AperFunction,NonPlated,Blind,BackDrill\n");
            let _ = writeln!(t, "T{}C{:.4}", i + 1, *d as f64 / 1e6);
        }
        t.push_str("%\nG90\nG05\n");
        for (i, d) in tools.iter().enumerate() {
            let _ = writeln!(t, "T{}", i + 1);
            for h in holes.iter().filter(|h| h.0 == *d) {
                let _ = writeln!(t, "X{}Y{}", mm4(h.1.x.0), mm4(h.1.y.0));
            }
        }
        t.push_str("M30\n");
        let side = if top { "TOP" } else { "BOTTOM" };
        out.push((format!("BACKDRILL-{side}-L{a}-L{b}"), t));
    }
    out
}

// ---------------------------------------------------------------- IPC-356

fn ipc356(board: &Board, stem: &str, n: u8) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "C  IPC-D-356A netlist written by Trace {}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(o, "P  JOB   {}", stem);
    o.push_str("P  CODE  00\nP  UNITS CUST 1\nP  DIM   N\n");
    let _ = writeln!(o, "P  Number of etch Layers: {n}");
    // Names that do not fit the 14-character field go behind an alias.
    let mut alias: HashMap<u32, String> = HashMap::new();
    let mut name = |net: Option<u32>, o: &mut String| -> String {
        let Some(id) = net else {
            return "N/C".into();
        };
        let full = net_name(board, net);
        if full.len() <= 14 && full.is_ascii() && !full.contains(' ') {
            return full.into();
        }
        let next = alias.len() + 1;
        alias
            .entry(id)
            .or_insert_with(|| {
                let a = format!("NNAME{next}");
                let _ = writeln!(o, "P  {a} {}", full.replace(' ', "_"));
                a
            })
            .clone()
    };
    let mut lines = String::new();
    let um = |v: i64| (v as f64 / 1000.0).round() as i64;
    let size = |v: i64| um(v).clamp(0, 9999);
    let xy = |p: Point| {
        let s = |v: i64| {
            format!(
                "{}{:06}",
                if v < 0 { '-' } else { '+' },
                um(v).abs().min(999_999)
            )
        };
        format!("X{}Y{}", s(p.x.0), s(p.y.0))
    };
    let geo = &board.geometry;
    for p in &geo.pads {
        let smd = p.hole.0 == 0;
        let access = match p.layer {
            LayerRef::Copper(0) => 1,
            LayerRef::Copper(l) if l == n - 1 => n as u32,
            LayerRef::AllCopper => 0,
            _ => continue,
        };
        let net = name(p.net, &mut o);
        let part = p
            .component
            .and_then(|c| board.components.get(c as usize))
            .map(|c| c.designator.as_str())
            .unwrap_or("");
        let hole = if smd {
            "     ".to_string()
        } else {
            format!("D{:04}", size(p.hole.0))
        };
        let pl = if smd {
            ' '
        } else if plated(p) {
            'P'
        } else {
            'U'
        };
        let rot = p.rotation_deg.rem_euclid(360.0).round() as i64 % 360;
        let _ = writeln!(
            lines,
            "{}{:<14}   {:<6}-{:<4} {hole}{pl}A{:02}{}X{:04}Y{:04}R{:03} S{}",
            if smd { "327" } else { "317" },
            clip(&net, 14),
            clip(part, 6),
            clip(&p.name, 4),
            access,
            xy(p.at),
            size(p.size_x.0),
            size(p.size_y.0),
            rot,
            if smd { 0 } else { 3 }
        );
    }
    for v in &geo.vias {
        let net = name(v.net, &mut o);
        let _ = writeln!(
            lines,
            "317{:<14}   VIA   -     D{:04}PA00{}X{:04}Y0000R000 S3",
            clip(&net, 14),
            size(v.hole.0),
            xy(v.at),
            size(v.diameter.0)
        );
    }
    o.push_str(&lines);
    o.push_str("999\n");
    o
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------- Assembly

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Middle of each part's pads, or its origin when it has none.
fn centres(board: &Board) -> Vec<Point> {
    let mut b: Vec<[i64; 4]> =
        vec![[i64::MAX, i64::MAX, i64::MIN, i64::MIN]; board.components.len()];
    for p in &board.geometry.pads {
        if let Some(r) = p.component.and_then(|c| b.get_mut(c as usize)) {
            let h = p.size_x.0.max(p.size_y.0) / 2;
            *r = [
                r[0].min(p.at.x.0 - h),
                r[1].min(p.at.y.0 - h),
                r[2].max(p.at.x.0 + h),
                r[3].max(p.at.y.0 + h),
            ];
        }
    }
    board
        .components
        .iter()
        .zip(b)
        .map(|(c, r)| {
            if r[0] <= r[2] {
                Point {
                    x: Length((r[0] + r[2]) / 2),
                    y: Length((r[1] + r[3]) / 2),
                }
            } else {
                Point { x: c.x, y: c.y }
            }
        })
        .collect()
}

fn pick_place(board: &Board) -> String {
    let mut o = String::from(
        "Designator,Value,Part Number,Footprint,Layer,Center-X(mm),Center-Y(mm),Rotation,Ref-X(mm),Ref-Y(mm)\n",
    );
    let mid = centres(board);
    let mut order: Vec<usize> = (0..board.components.len()).collect();
    order.sort_by(|&a, &b| {
        natural(
            &board.components[a].designator,
            &board.components[b].designator,
        )
    });
    for i in order {
        let c = &board.components[i];
        let [value, part, ..] = part_info(c);
        let _ = writeln!(
            o,
            "{},{},{},{},{},{:.4},{:.4},{},{:.4},{:.4}",
            csv(&c.designator),
            csv(&value),
            csv(&part),
            csv(&c.footprint),
            if c.side == Side::Top { "Top" } else { "Bottom" },
            mid[i].x.mm(),
            mid[i].y.mm(),
            num(c.rotation_deg.rem_euclid(360.0)),
            c.x.mm(),
            c.y.mm()
        );
    }
    o
}

fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "360" || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// A part's value, part number, maker and description, from its
/// parameters (the part number falls back to its library reference).
fn part_info(c: &trace_core::Component) -> [String; 4] {
    let get = |names: &[&str]| c.param(names).unwrap_or("").to_string();
    let number = c
        .param(&[
            "PartNumber",
            "Part Number",
            "Manufacturer Part Number",
            "Manufacturer Part Number 1",
            "MPN",
            "MFR_PN",
        ])
        .map(str::to_string)
        .or_else(|| c.library_ref.clone())
        .unwrap_or_default();
    [
        get(&["Value", "Comment"]),
        number,
        get(&["Manufacturer", "Manufacturer 1", "Manufacturer Name", "MFR"]),
        get(&["Description"]),
    ]
}

fn bom(board: &Board) -> String {
    // Parts alike in value, part number, maker, description and footprint.
    type Key = ([String; 4], String);
    let mut groups: BTreeMap<Key, Vec<&str>> = BTreeMap::new();
    for c in &board.components {
        groups
            .entry((part_info(c), c.footprint.clone()))
            .or_default()
            .push(&c.designator);
    }
    let mut rows: Vec<_> = groups.into_iter().collect();
    for (_, d) in &mut rows {
        d.sort_by(|a, b| natural(a, b));
    }
    rows.sort_by(|a, b| natural(a.1[0], b.1[0]));
    let mut o = String::from(
        "Item,Quantity,Designators,Value,Part Number,Manufacturer,Description,Footprint\n",
    );
    for (i, (([value, part, maker, desc], fp), d)) in rows.iter().enumerate() {
        let _ = writeln!(
            o,
            "{},{},{},{},{},{},{},{}",
            i + 1,
            d.len(),
            csv(&d.join(" ")),
            csv(value),
            csv(part),
            csv(maker),
            csv(desc),
            csv(fp)
        );
    }
    o
}

/// Orders designators as people read them: R2 before R10.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut text = String::new();
        let mut digits = String::new();
        for ch in s.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                if !digits.is_empty() {
                    out.push((std::mem::take(&mut text), digits.parse().unwrap_or(0)));
                    digits.clear();
                }
                text.push(ch);
            }
        }
        out.push((text, digits.parse().unwrap_or(0)));
        out
    }
    parts(a).cmp(&parts(b)).then_with(|| a.cmp(b))
}

/// Number of copper layers, for callers that report what was written.
pub fn copper_count(board: &Board) -> usize {
    board
        .stackup
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Copper)
        .count()
        .max(1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use trace_core::*;

    pub(crate) fn pt(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
        }
    }

    pub(crate) fn board() -> Board {
        let mut b = Board::default();
        for n in ["Top", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                material: None,
            });
        }
        for n in ["GND", "SIG_WITH_A_VERY_LONG_NAME"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        for (d, x) in [("R10", 10.0), ("R2", 20.0)] {
            b.components.push(Component {
                designator: d.into(),
                footprint: "0603".into(),
                library_ref: Some("RC0603".into()),
                x: Length::from_mm(x),
                y: Length::from_mm(5.0),
                rotation_deg: 90.0,
                side: Side::Top,
                sheet: None,
                params: [("Value", "10k"), ("Description", "Resistor, 1%")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            });
        }
        let pad = |c: u32, name: &str, at: Point, net: u32, rot: f64| Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(c),
            name: name.into(),
            at,
            size_x: Length::from_mm(0.8),
            size_y: Length::from_mm(0.9),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: rot,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        b.geometry.pads.push(pad(0, "1", pt(10.0, 4.2), 0, 90.0));
        b.geometry.pads.push(pad(0, "2", pt(10.0, 5.8), 1, 45.0));
        b.geometry.pads.push(Pad {
            layer: LayerRef::AllCopper,
            hole: Length::from_mm(1.0),
            size_x: Length::from_mm(1.7),
            size_y: Length::from_mm(1.7),
            shape: PadShape::Round,
            ..pad(1, "1", pt(20.0, 5.0), 0, 0.0)
        });
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: pt(10.0, 5.8),
            end: pt(30.0, 5.8),
            width: Length::from_mm(0.2),
            component: None,
            text: false,
        });
        b.geometry.vias.push(Via {
            net: Some(1),
            from_layer: 0,
            to_layer: 1,
            at: pt(30.0, 5.8),
            diameter: Length::from_mm(0.6),
            hole: Length::from_mm(0.3),
            rings: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.outline = vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 12.0), pt(0.0, 12.0)];
        b
    }

    #[test]
    fn writes_every_layer_with_nets_that_read_back() {
        let b = board();
        let files = fab_files(&b, "t", &FabOptions::default());
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        for want in [
            "t.GTL",
            "t.GBL",
            "t.GTS",
            "t.GBS",
            "t.GTP",
            "t.GBP",
            "t.GTO",
            "t.GBO",
            "t.GKO",
            "t-PTH.DRL",
            "t.ipc",
            "t-PickPlace.csv",
            "t-BOM.csv",
        ] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        let gtl = String::from_utf8(files[0].data.clone()).unwrap();
        assert!(gtl.contains("%TF.FileFunction,Copper,L1,Top*%"));
        // 90° rect swaps its sides; 45° becomes an outline macro.
        assert!(gtl.contains("R,0.9X0.8*%"), "{gtl}");
        assert!(gtl.contains("%AMP"));
        let zip = fab_zip(&files).unwrap();
        let dir = std::env::temp_dir().join(format!("trace-fab-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.zip");
        std::fs::write(&path, zip).unwrap();
        let back = trace_gerber::read_path(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let g = &back.board.geometry;
        assert_eq!(back.board.stackup.copper_layers().count(), 2);
        assert_eq!(g.vias.len(), 1, "{:?}", back.warnings);
        assert_eq!(
            g.tracks
                .iter()
                .filter(|t| matches!(t.layer, LayerRef::Copper(_)))
                .count(),
            1
        );
        let names: Vec<&str> = back.board.nets.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"SIG_WITH_A_VERY_LONG_NAME"), "{names:?}");
        assert!(names.contains(&"GND"));
        let d = |name: &str| {
            back.board
                .components
                .iter()
                .find(|c| c.designator == name)
                .is_some()
        };
        assert!(d("R10") && d("R2"));
    }

    #[test]
    fn assembly_files_sort_designators_naturally() {
        let b = board();
        let pnp = pick_place(&b);
        let lines: Vec<&str> = pnp.lines().collect();
        assert!(
            lines[1].starts_with("R2,10k,RC0603,0603,Top,20.0000,5.0000,90,"),
            "{}",
            lines[1]
        );
        assert!(lines[2].starts_with("R10,10k,"));
        assert_eq!(
            bom(&b).lines().nth(1),
            Some("1,2,R2 R10,10k,RC0603,,\"Resistor, 1%\",0603")
        );
    }

    #[test]
    fn options_come_from_the_board_rules() {
        let mut b = board();
        let mut params = std::collections::BTreeMap::new();
        params.insert("EXPANSION".to_string(), "4mil".to_string());
        params.insert("ISTENTINGTOP".to_string(), "FALSE".to_string());
        b.rules.push(Rule {
            kind: "SolderMaskExpansion".into(),
            name: "m".into(),
            enabled: true,
            priority: 1,
            scope: vec!["All".into()],
            params,
        });
        let o = FabOptions::from_board(&b);
        assert_eq!(o.mask_expansion, Length::from_mil(4.0));
        assert!(!o.tent_top && o.tent_bottom);
        let gts = mask(&b, Side::Top, 2, &o);
        // The via opens: 0.6 mm + 2 × 4 mil.
        assert!(gts.contains("C,0.8032*%"), "{gts}");
    }

    #[test]
    fn backdrill_files_name_the_layers_drilled_and_kept() {
        let mut b = board();
        // Four copper layers; the via joins Top and L2, drilled from the
        // bottom up to L2.
        for n in ["L2", "L3"] {
            b.stackup.layers.insert(
                1,
                StackLayer {
                    name: n.into(),
                    altium_id: None,
                    kind: LayerKind::Copper,
                    thickness: Length(0),
                    dielectric_constant: None,
                    material: None,
                },
            );
        }
        b.stackup.layers.swap(1, 2);
        let v = &mut b.geometry.vias[0];
        v.to_layer = 3;
        v.fab.back_bottom = Some(Backdrill {
            keep: 1,
            diameter: Length::from_mm(0.5),
        });
        let files = drills(&b, 4);
        let (name, text) = files
            .iter()
            .find(|(n, _)| n.starts_with("BACKDRILL"))
            .expect("a backdrill file");
        assert_eq!(name, "BACKDRILL-BOTTOM-L3-L4");
        assert!(
            text.contains("TF.FileFunction,NonPlated,3,4,Blind"),
            "{text}"
        );
        assert!(text.contains("must not cut L2 (L2)"), "{text}");
        assert!(text.contains("BackDrill\nT1C0.5000"), "{text}");
        assert!(text.contains("X30.0000Y5.8000"), "{text}");
        // The via itself is still in the plated through-hole file.
        assert!(files.iter().any(|(n, _)| n == "PTH"));
    }
}
