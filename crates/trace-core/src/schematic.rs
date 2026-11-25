use serde::{Deserialize, Serialize};

/// One schematic sheet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    pub symbols: Vec<Symbol>,
    pub net_labels: Vec<String>,
    pub power_ports: Vec<String>,
    pub ports: Vec<String>,
    /// Child sheets referenced by sheet symbols.
    pub sheet_refs: Vec<String>,
    pub wires: usize,
    pub junctions: usize,
    /// What the sheet looks like, for the schematic viewer.
    #[serde(default, skip_serializing_if = "Drawing::is_empty")]
    pub drawing: Drawing,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Symbol {
    pub designator: String,
    pub library_ref: String,
    pub description: String,
    pub value: Option<String>,
    pub pins: usize,
    /// The footprint the part uses on the board (its current PCB model).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub footprint: String,
}

/// A schematic symbol library (an Altium `.SchLib`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SymbolLib {
    pub name: String,
    /// One entry per part of each component (U1A and U1B are two).
    pub symbols: Vec<LibSymbol>,
}

/// One placeable symbol: a component, or one part of a multi-part one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibSymbol {
    /// The component's name in the library.
    pub name: String,
    /// Which part this is (1 for A) and how many the component has.
    pub part: u32,
    pub parts: u32,
    /// Designator (such as `R?`), description, value, footprint.
    pub symbol: Symbol,
    /// The graphics, with the symbol's origin at 0,0: one part, index 0.
    pub drawing: Drawing,
}

impl LibSymbol {
    /// The name shown in lists: with the part letter when there are
    /// several (LM358 B).
    pub fn title(&self) -> String {
        if self.parts > 1 {
            let letter = (b'A' + (self.part.clamp(1, 26) - 1) as u8) as char;
            format!("{} {letter}", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// A point on a sheet in mil, y up.
pub type Pt = [f64; 2];

/// An RGB colour.
pub type Rgb = [u8; 3];

/// The graphics of one sheet, already placed: every coordinate is in sheet
/// mil with y up, and parts are drawn in their placed position and rotation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Drawing {
    /// Sheet size in mil.
    pub size: [f64; 2],
    pub paper: Rgb,
    pub shapes: Vec<SchShape>,
    pub texts: Vec<SchText>,
    pub pins: Vec<SchPin>,
    /// Wires as polylines; each is one object for connectivity.
    pub wires: Vec<Vec<Pt>>,
    pub junctions: Vec<Pt>,
    /// Net labels, power ports, ports and sheet entries: things that name a net.
    pub tags: Vec<NetTag>,
    /// Placed parts; `symbol` indexes `Sheet::symbols`.
    pub parts: Vec<SchPart>,
    /// Schematic nets found from the wiring, filled by `Drawing::connect`.
    pub nets: Vec<SchNet>,
    /// Pictures (logos, block diagrams) stored in the sheet.
    #[serde(default)]
    pub images: Vec<SchImage>,
    /// Sheet symbols: the box and the file of the sheet they stand for.
    #[serde(default)]
    pub links: Vec<SheetLink>,
    /// Buses, as polylines. A bus draws a bundle of nets; its members
    /// connect by their own names (D0..D7 under a D[0..7] label), never
    /// through the bus itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<Vec<Pt>>,
    /// Bus entries: the short 45 degree strokes from a bus to a wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bus_entries: Vec<[Pt; 2]>,
    /// Harness connectors: boxes whose entries gather signals into one
    /// signal harness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<Harness>,
    /// Signal harnesses: the thick lines that carry a harness between
    /// connectors, ports and sheet entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signal_harnesses: Vec<Vec<Pt>>,
    /// Harness bundles found by `Drawing::connect`.
    #[serde(skip)]
    pub bundles: Vec<Bundle>,
    /// No-ERC markers: pins and wires they sit on are not checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_erc: Vec<Pt>,
}

/// A harness connector: a box with named entries on one edge, and the
/// point on the other edge where a signal harness carries them all away.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Harness {
    pub min: Pt,
    pub max: Pt,
    /// The harness type it defines (USB, SPI...).
    pub kind: String,
    /// How far below the top edge the signal harness attaches, in mil.
    pub primary: f64,
    pub entries: Vec<HarnessEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub name: String,
    /// 1 on the right edge, 0 on the left.
    pub side: u8,
    /// Below the top edge, in mil.
    pub dist: f64,
}

impl Harness {
    /// Where entry `e` connects: on its edge, `dist` below the top.
    pub fn entry_at(&self, e: &HarnessEntry) -> Pt {
        let x = if e.side == 1 {
            self.max[0]
        } else {
            self.min[0]
        };
        [x, self.max[1] - e.dist]
    }

    /// Whether the entries sit on the right edge (most of them do).
    pub fn entries_right(&self) -> bool {
        let right = self.entries.iter().filter(|e| e.side == 1).count();
        right * 2 > self.entries.len()
    }

    /// Where the signal harness attaches: the edge away from the entries.
    pub fn bundle_at(&self) -> Pt {
        let x = if self.entries_right() {
            self.min[0]
        } else {
            self.max[0]
        };
        [x, self.max[1] - self.primary]
    }

    /// A connector of type `kind` with its top-left corner at `top_left`
    /// and one entry per name on its right edge (on the left when
    /// `left`), 100 mil apart, sized as Altium places them.
    pub fn new(top_left: Pt, kind: &str, names: &[&str], left: bool) -> Harness {
        let n = names.len().max(1) as f64;
        let h = 100.0 * (n + 1.0);
        let w = names.iter().map(|s| s.len()).max().unwrap_or(4).max(4) as f64 * 60.0 + 100.0;
        let w = (w / 100.0).ceil() * 100.0;
        Harness {
            min: [top_left[0], top_left[1] - h],
            max: [top_left[0] + w, top_left[1]],
            kind: kind.to_string(),
            primary: (h / 2.0 / 100.0).round() * 100.0,
            entries: names
                .iter()
                .enumerate()
                .map(|(i, s)| HarnessEntry {
                    name: s.to_string(),
                    side: if left { 0 } else { 1 },
                    dist: 100.0 * (i as f64 + 1.0),
                })
                .collect(),
        }
    }
}

/// A harness bundle on one sheet: the connectors, signal harnesses and
/// the ports, sheet entries and labels they touch.
#[derive(Clone, Debug, Default)]
pub struct Bundle {
    /// Its name: a port's, a sheet entry's or a label's on it, else the
    /// first connector's harness type.
    pub name: String,
    pub harnesses: Vec<u32>,
    /// Ports, sheet entries and labels on it.
    pub tags: Vec<u32>,
    /// Each entry name and the sheet net it is on.
    pub members: Vec<(String, u32)>,
}

/// The member names of a bus name: `D[0..7]` gives D0 to D7 and
/// `A[3..0]B` gives A3B down to A0B. None for a plain name.
pub fn bus_members(name: &str) -> Option<Vec<String>> {
    let open = name.find('[')?;
    let close = open + name[open..].find(']')?;
    let (a, b) = name[open + 1..close].split_once("..")?;
    let (a, b): (i64, i64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    if (a - b).abs() > 4096 {
        return None;
    }
    let (pre, post) = (&name[..open], &name[close + 1..]);
    let range: Vec<i64> = if a <= b {
        (a..=b).collect()
    } else {
        (b..=a).rev().collect()
    };
    Some(
        range
            .into_iter()
            .map(|i| format!("{pre}{i}{post}"))
            .collect(),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SheetLink {
    pub min: Pt,
    pub max: Pt,
    pub file: String,
    /// The sheet symbol's name: its channel (CH1), or Repeat(CH,1,4) for
    /// four channels from one symbol.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl SheetLink {
    /// The channels it places: one per sheet symbol, or one per number of
    /// a Repeat(name,first,last).
    pub fn channels(&self) -> Vec<String> {
        let n = self.name.trim();
        if let Some(inner) = n
            .get(..7)
            .filter(|h| h.eq_ignore_ascii_case("repeat("))
            .and_then(|_| n[7..].strip_suffix(')'))
        {
            let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
            if let [name, a, b] = parts[..] {
                if let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) {
                    if a <= b && b - a < 1000 {
                        return (a..=b).map(|i| format!("{name}{i}")).collect();
                    }
                }
            }
        }
        vec![n.to_string()]
    }
}

/// A picture placed on the sheet: the file's own bytes (BMP, PNG or JPEG).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchImage {
    pub min: Pt,
    pub max: Pt,
    pub keep_aspect: bool,
    /// Kept as base64 in Trace files.
    #[serde(default, with = "b64")]
    pub data: std::sync::Arc<Vec<u8>>,
}

/// Picture bytes as base64 text.
mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(v.len() * 4 / 3 + 4);
        for c in v.chunks(3) {
            let n = (c[0] as u32) << 16
                | (*c.get(1).unwrap_or(&0) as u32) << 8
                | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= c.len() {
                    out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        let text = String::deserialize(d)?;
        let mut out = Vec::with_capacity(text.len() * 3 / 4);
        let (mut acc, mut bits) = (0u32, 0);
        for b in text.bytes() {
            let Some(v) = ABC.iter().position(|&a| a == b) else {
                continue;
            };
            acc = acc << 6 | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        Ok(Arc::new(out))
    }
}

impl Drawing {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.texts.is_empty() && self.wires.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SchGeom {
    /// An open polyline.
    Line(Vec<Pt>),
    /// A closed polygon.
    Poly(Vec<Pt>),
    /// An axis-aligned rectangle with optional corner radii.
    Rect { min: Pt, max: Pt, r: [f64; 2] },
    /// An elliptical arc, angles in degrees counter-clockwise from +x. A full
    /// ellipse has start 0 and end 360.
    Arc {
        c: Pt,
        r: [f64; 2],
        start: f64,
        end: f64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchShape {
    pub geom: SchGeom,
    /// Stroke width in mil; 0 draws the thinnest visible line.
    pub width: f64,
    pub stroke: Option<Rgb>,
    pub fill: Option<Rgb>,
    pub dashed: bool,
    pub part: Option<u32>,
    /// Which wire or bus this is, for highlighting a net.
    pub wire: Option<u32>,
    /// The power port or port (`Drawing::tags`) this is the symbol of,
    /// so it moves and goes with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchText {
    pub at: Pt,
    pub text: String,
    /// Font height (em) in mil.
    pub size: f64,
    pub color: Rgb,
    /// Quarter turns counter-clockwise.
    pub rot: u8,
    /// 0..8: bottom/centre/top rows of left/centre/right, as Altium numbers them
    /// (0 bottom-left, 4 centre, 8 top-right).
    pub just: u8,
    pub bold: bool,
    pub italic: bool,
    /// Wrap inside this width (text frames), in mil.
    pub wrap: Option<f64>,
    pub part: Option<u32>,
}

/// A pin's electrical type, as Altium and Allegro name them, for the
/// electrical rule check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PinKind {
    Input,
    Bidir,
    Output,
    OpenCollector,
    #[default]
    Passive,
    HiZ,
    OpenEmitter,
    Power,
}

impl PinKind {
    pub const ALL: [PinKind; 8] = [
        PinKind::Input,
        PinKind::Bidir,
        PinKind::Output,
        PinKind::OpenCollector,
        PinKind::Passive,
        PinKind::HiZ,
        PinKind::OpenEmitter,
        PinKind::Power,
    ];

    /// From Altium's `ELECTRICAL` number.
    pub fn altium(n: i64) -> PinKind {
        match n {
            0 => PinKind::Input,
            1 => PinKind::Bidir,
            2 => PinKind::Output,
            3 => PinKind::OpenCollector,
            5 => PinKind::HiZ,
            6 => PinKind::OpenEmitter,
            7 => PinKind::Power,
            _ => PinKind::Passive,
        }
    }

    pub fn altium_number(self) -> i64 {
        match self {
            PinKind::Input => 0,
            PinKind::Bidir => 1,
            PinKind::Output => 2,
            PinKind::OpenCollector => 3,
            PinKind::Passive => 4,
            PinKind::HiZ => 5,
            PinKind::OpenEmitter => 6,
            PinKind::Power => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PinKind::Input => "input",
            PinKind::Bidir => "I/O",
            PinKind::Output => "output",
            PinKind::OpenCollector => "open collector",
            PinKind::Passive => "passive",
            PinKind::HiZ => "high-Z",
            PinKind::OpenEmitter => "open emitter",
            PinKind::Power => "power",
        }
    }

    fn is_passive(&self) -> bool {
        *self == PinKind::Passive
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchPin {
    /// The connection end.
    pub at: Pt,
    /// The end on the part body.
    pub base: Pt,
    pub part: u32,
    pub name: String,
    /// Pin number, matching the footprint's pad name.
    pub number: String,
    #[serde(default, skip_serializing_if = "PinKind::is_passive")]
    pub kind: PinKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagKind {
    Label,
    Power,
    Port,
    SheetEntry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetTag {
    /// Where it connects; a port connects at either end.
    pub at: Vec<Pt>,
    pub text: String,
    pub kind: TagKind,
    /// The text that shows its name (`Drawing::texts`): a net label's
    /// text, or the name drawn on a port, power port or sheet entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphic: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchPart {
    pub designator: String,
    pub symbol: usize,
    pub min: Pt,
    pub max: Pt,
    /// Which part of a multi-part component this is (1 for A); 0 for a
    /// component in one part.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gate: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl SchPart {
    /// The designator with its part letter (U1A), as drawn.
    pub fn full_designator(&self) -> String {
        if self.gate > 0 {
            format!(
                "{}{}",
                self.designator,
                (b'A' + (self.gate.clamp(1, 26) - 1) as u8) as char
            )
        } else {
            self.designator.clone()
        }
    }
}

/// A net within one sheet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchNet {
    /// Name from a power port, net label or port, if it has one.
    pub name: Option<String>,
    pub wires: Vec<u32>,
    pub pins: Vec<u32>,
    pub tags: Vec<u32>,
    /// Harness entries on it: (connector, entry).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness: Vec<[u32; 2]>,
}

fn on_segment(p: Pt, a: Pt, b: Pt, tol: f64) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy <= tol * tol
}

fn same(a: Pt, b: Pt, tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

impl Drawing {
    /// Where three or more connections meet with no junction stored: a
    /// wire end on the middle (or a corner) of another wire, or three
    /// wire ends and pins at one point. Altium draws these dots itself
    /// and does not save them, so the view adds them.
    pub fn auto_junctions(&self) -> Vec<Pt> {
        use std::collections::HashMap;
        let key = |p: Pt| (p[0].round() as i64, p[1].round() as i64);
        // Count of connections at each wire end.
        let mut ends: HashMap<(i64, i64), (u32, Pt)> = HashMap::new();
        for w in self.wires.iter().filter(|w| w.len() >= 2) {
            for p in [w[0], w[w.len() - 1]] {
                ends.entry(key(p)).or_insert((0, p)).0 += 1;
            }
        }
        if ends.is_empty() {
            return Vec::new();
        }
        for pin in &self.pins {
            if let Some(e) = ends.get_mut(&key(pin.at)) {
                e.0 += 1;
            }
        }
        // Ends by row and by column, to find those lying along a segment.
        let mut by_y: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
        let mut by_x: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
        for &k in ends.keys() {
            by_y.entry(k.1).or_default().push(k);
            by_x.entry(k.0).or_default().push(k);
        }
        let mut through: Vec<(i64, i64)> = Vec::new();
        for w in self.wires.iter().filter(|w| w.len() >= 2) {
            // Inner corners: the wire passes through, two connections.
            for &p in &w[1..w.len() - 1] {
                if ends.contains_key(&key(p)) {
                    through.push(key(p));
                }
            }
            for s in w.windows(2) {
                let (a, b) = (key(s[0]), key(s[1]));
                let inside = |k: &(i64, i64)| *k != a && *k != b;
                if a.1 == b.1 {
                    let (lo, hi) = (a.0.min(b.0), a.0.max(b.0));
                    if let Some(v) = by_y.get(&a.1) {
                        through.extend(v.iter().filter(|k| k.0 > lo && k.0 < hi && inside(k)));
                    }
                } else if a.0 == b.0 {
                    let (lo, hi) = (a.1.min(b.1), a.1.max(b.1));
                    if let Some(v) = by_x.get(&a.0) {
                        through.extend(v.iter().filter(|k| k.1 > lo && k.1 < hi && inside(k)));
                    }
                } else {
                    // A slanted segment: test every end against it.
                    let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
                    let l2 = dx * dx + dy * dy;
                    for k in ends.keys().filter(|k| inside(k)) {
                        let (px, py) = ((k.0 - a.0) as f64, (k.1 - a.1) as f64);
                        let t = (px * dx + py * dy) / l2;
                        if t > 0.0 && t < 1.0 && (px * dy - py * dx).abs() / l2.sqrt() < 1.0 {
                            through.push(*k);
                        }
                    }
                }
            }
        }
        for k in through {
            if let Some(e) = ends.get_mut(&k) {
                e.0 += 2;
            }
        }
        let stored: std::collections::HashSet<(i64, i64)> =
            self.junctions.iter().map(|&p| key(p)).collect();
        let mut out: Vec<Pt> = ends
            .into_iter()
            .filter(|(k, (n, _))| *n >= 3 && !stored.contains(k))
            .map(|(_, (_, p))| p)
            .collect();
        out.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        out
    }

    /// Works out the sheet's nets: wires join where an end or a junction
    /// lies on another wire, pins and tags join the wire they touch, and
    /// tags with the same text join each other.
    pub fn connect(&mut self) {
        // Sheets drawn in metric and read as mil land a fraction of a mil
        // apart where they meet; grids are 10 mil or more.
        const TOL: f64 = 1.0;
        let (nw, np, nt) = (self.wires.len(), self.pins.len(), self.tags.len());
        // Harness entries, one element each after the tags.
        let ents: Vec<[u32; 2]> = self
            .harnesses
            .iter()
            .enumerate()
            .flat_map(|(h, c)| (0..c.entries.len()).map(move |e| [h as u32, e as u32]))
            .collect();
        let mut d = Dsu((0..nw + np + nt + ents.len()).collect());
        let pin = |i: usize| nw + i;
        let tag = |i: usize| nw + np + i;
        let ent = |i: usize| nw + np + nt + i;
        let on_wire = |p: Pt, w: &[Pt]| w.windows(2).any(|s| on_segment(p, s[0], s[1], TOL));
        // Points that join whatever wire they touch.
        let mut points: Vec<(Pt, usize)> = Vec::new();
        for (i, w) in self.wires.iter().enumerate() {
            if let (Some(&a), Some(&b)) = (w.first(), w.last()) {
                points.push((a, i));
                points.push((b, i));
            }
        }
        for (i, p) in self.pins.iter().enumerate() {
            points.push((p.at, pin(i)));
        }
        for (i, t) in self.tags.iter().enumerate() {
            for &p in &t.at {
                points.push((p, tag(i)));
            }
        }
        for (i, &[h, e]) in ents.iter().enumerate() {
            let c = &self.harnesses[h as usize];
            points.push((c.entry_at(&c.entries[e as usize]), ent(i)));
        }
        let junction_wires: Vec<Vec<usize>> = self
            .junctions
            .iter()
            .map(|&j| {
                (0..nw)
                    .filter(|&w| on_wire(j, &self.wires[w]))
                    .collect::<Vec<_>>()
            })
            .collect();
        for ws in junction_wires {
            for w in ws.windows(2) {
                d.join(w[0], w[1]);
            }
        }
        // Wire bounding boxes keep the search cheap.
        let boxes: Vec<[f64; 4]> = self
            .wires
            .iter()
            .map(|w| {
                w.iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                        [
                            b[0].min(p[0]),
                            b[1].min(p[1]),
                            b[2].max(p[0]),
                            b[3].max(p[1]),
                        ]
                    })
            })
            .collect();
        for &(p, owner) in &points {
            for (w, b) in boxes.iter().enumerate() {
                if p[0] < b[0] - TOL || p[0] > b[2] + TOL || p[1] < b[1] - TOL || p[1] > b[3] + TOL
                {
                    continue;
                }
                if owner != w && on_wire(p, &self.wires[w]) {
                    d.join(owner, w);
                }
            }
        }
        // Pins and tags that touch directly.
        let mut sorted: Vec<(Pt, usize)> = points.iter().filter(|p| p.1 >= nw).copied().collect();
        sorted.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
        for i in 0..sorted.len() {
            for j in i + 1..sorted.len() {
                if sorted[j].0[0] - sorted[i].0[0] > TOL {
                    break;
                }
                if same(sorted[i].0, sorted[j].0, TOL) {
                    d.join(sorted[i].1, sorted[j].1);
                }
            }
        }
        // Same text, same net: labels, power ports and ports within one
        // sheet. Sheet entries join only by wire (two sheet symbols may
        // both have a CLK entry), and through their sheet's ports.
        let mut by_name: std::collections::HashMap<&str, usize> = Default::default();
        for (i, t) in self.tags.iter().enumerate() {
            if t.text.is_empty() || t.kind == TagKind::SheetEntry {
                continue;
            }
            match by_name.get(t.text.as_str()) {
                Some(&o) => d.join(tag(i), o),
                None => {
                    by_name.insert(&t.text, tag(i));
                }
            }
        }
        // Harness bundles: entries of one name on connectors that one
        // signal harness ties together are one net.
        let mut bundles = self.find_bundles();
        for b in &bundles {
            let mut by: std::collections::HashMap<String, usize> = Default::default();
            for &h in &b.harnesses {
                for e in 0..self.harnesses[h as usize].entries.len() {
                    let Some(i) = ents.iter().position(|x| *x == [h, e as u32]) else {
                        continue;
                    };
                    let k = self.harnesses[h as usize].entries[e]
                        .name
                        .trim()
                        .to_ascii_uppercase();
                    if k.is_empty() {
                        continue;
                    }
                    match by.get(&k) {
                        Some(&o) => d.join(ent(i), o),
                        None => {
                            by.insert(k, ent(i));
                        }
                    }
                }
            }
        }
        let mut index: std::collections::HashMap<usize, usize> = Default::default();
        let mut nets: Vec<SchNet> = Vec::new();
        let mut net_of = |d: &mut Dsu, i: usize, nets: &mut Vec<SchNet>| {
            let r = d.find(i);
            *index.entry(r).or_insert_with(|| {
                nets.push(SchNet::default());
                nets.len() - 1
            })
        };
        for w in 0..nw {
            let n = net_of(&mut d, w, &mut nets);
            nets[n].wires.push(w as u32);
        }
        for p in 0..np {
            let n = net_of(&mut d, pin(p), &mut nets);
            nets[n].pins.push(p as u32);
        }
        for t in 0..nt {
            let n = net_of(&mut d, tag(t), &mut nets);
            nets[n].tags.push(t as u32);
        }
        for (i, &he) in ents.iter().enumerate() {
            let n = net_of(&mut d, ent(i), &mut nets);
            nets[n].harness.push(he);
        }
        for n in &mut nets {
            let rank = |k: TagKind| match k {
                TagKind::Power => 0,
                TagKind::Label => 1,
                TagKind::Port => 2,
                TagKind::SheetEntry => 3,
            };
            n.name = n
                .tags
                .iter()
                .map(|&t| &self.tags[t as usize])
                .filter(|t| !t.text.is_empty())
                .min_by_key(|t| rank(t.kind))
                .map(|t| t.text.clone());
        }
        // Each bundle's members, and a name for harness nets with none:
        // bundle.entry, as Altium names them.
        for b in &mut bundles {
            for &h in &b.harnesses {
                for (e, en) in self.harnesses[h as usize].entries.iter().enumerate() {
                    let Some(ni) = nets.iter().position(|n| n.harness.contains(&[h, e as u32]))
                    else {
                        continue;
                    };
                    if !b.members.iter().any(|m| m.0.eq_ignore_ascii_case(&en.name)) {
                        b.members.push((en.name.clone(), ni as u32));
                    }
                    if nets[ni].name.is_none() && !b.name.is_empty() && !en.name.is_empty() {
                        nets[ni].name = Some(format!("{}.{}", b.name, en.name));
                    }
                }
            }
        }
        self.bundles = bundles;
        self.nets = nets;
    }

    /// The sheet's harness bundles: connectors, signal harnesses and the
    /// ports, sheet entries and labels that touch them, grouped by where
    /// they meet. Members are left for `connect` to fill.
    pub fn find_bundles(&self) -> Vec<Bundle> {
        const TOL: f64 = 1.0;
        let (nh, ns) = (self.harnesses.len(), self.signal_harnesses.len());
        if nh == 0 && ns == 0 {
            return Vec::new();
        }
        let mut d = Dsu((0..nh + ns + self.tags.len()).collect());
        let on = |p: Pt, line: &[Pt]| line.windows(2).any(|s| on_segment(p, s[0], s[1], TOL));
        let bundle_pts: Vec<[Pt; 2]> = self
            .harnesses
            .iter()
            .map(|h| {
                let y = h.max[1] - h.primary;
                [[h.min[0], y], [h.max[0], y]]
            })
            .collect();
        for (si, line) in self.signal_harnesses.iter().enumerate() {
            let s = nh + si;
            let ends = [line.first().copied(), line.last().copied()];
            for (h, pts) in bundle_pts.iter().enumerate() {
                // The connector's own side first: the edge away from its
                // entries; either edge will do for files that differ.
                if pts.iter().any(|&p| on(p, line))
                    || ends
                        .iter()
                        .flatten()
                        .any(|&e| pts.iter().any(|&p| same(p, e, TOL)))
                {
                    d.join(h, s);
                }
            }
            for (oi, other) in self.signal_harnesses.iter().enumerate().skip(si + 1) {
                let touch = |a: &[Pt], b: &[Pt]| {
                    [a.first(), a.last()]
                        .into_iter()
                        .flatten()
                        .any(|&e| on(e, b))
                };
                if touch(line, other) || touch(other, line) {
                    d.join(s, nh + oi);
                }
            }
            for (t, tag) in self.tags.iter().enumerate() {
                if tag.kind == TagKind::Power {
                    continue;
                }
                if tag.at.iter().any(|&p| on(p, line)) {
                    d.join(s, nh + ns + t);
                }
            }
        }
        // A harness port straight on a connector, with no line between.
        for (h, pts) in bundle_pts.iter().enumerate() {
            for (t, tag) in self.tags.iter().enumerate() {
                if matches!(tag.kind, TagKind::Port | TagKind::SheetEntry)
                    && tag.at.iter().any(|&a| pts.iter().any(|&p| same(p, a, TOL)))
                {
                    d.join(h, nh + ns + t);
                }
            }
        }
        // Groups with a connector or a signal harness are bundles.
        let mut groups: std::collections::HashMap<usize, (Bundle, bool)> = Default::default();
        let mut order = Vec::new();
        for i in 0..nh + ns + self.tags.len() {
            let r = d.find(i);
            let (b, real) = groups.entry(r).or_insert_with(|| {
                order.push(r);
                (Bundle::default(), false)
            });
            if i < nh {
                b.harnesses.push(i as u32);
                *real = true;
            } else if i < nh + ns {
                *real = true;
            } else {
                b.tags.push((i - nh - ns) as u32);
            }
        }
        let mut out = Vec::new();
        for r in order {
            let Some((mut b, true)) = groups.remove(&r) else {
                continue;
            };
            let rank = |k: TagKind| match k {
                TagKind::Port => 0,
                TagKind::SheetEntry => 1,
                TagKind::Label => 2,
                TagKind::Power => 3,
            };
            b.name = b
                .tags
                .iter()
                .map(|&t| &self.tags[t as usize])
                .filter(|t| !t.text.trim().is_empty())
                .min_by_key(|t| rank(t.kind))
                .map(|t| t.text.trim().to_string())
                .or_else(|| {
                    b.harnesses
                        .iter()
                        .map(|&h| self.harnesses[h as usize].kind.trim().to_string())
                        .find(|k| !k.is_empty())
                })
                .unwrap_or_default();
            out.push(b);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_junctions_at_tees_not_crossings() {
        let d = Drawing {
            wires: vec![
                vec![[0.0, 0.0], [100.0, 0.0]],
                vec![[50.0, 0.0], [50.0, 50.0]],  // tee: a dot
                vec![[0.0, 20.0], [100.0, 20.0]], // crossing: no dot
                vec![[200.0, 0.0], [300.0, 0.0]],
                vec![[300.0, 0.0], [300.0, 100.0]], // corner of two ends: no dot
                vec![[0.0, 100.0], [100.0, 100.0], [100.0, 200.0]],
                vec![[100.0, 100.0], [150.0, 100.0]], // on a corner: a dot
            ],
            pins: vec![SchPin {
                at: [200.0, 0.0],
                base: [210.0, 0.0],
                part: 0,
                name: String::new(),
                number: "1".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(d.auto_junctions(), vec![[50.0, 0.0], [100.0, 100.0]]);
        let stored = Drawing {
            junctions: vec![[50.0, 0.0]],
            ..d.clone()
        };
        assert_eq!(stored.auto_junctions(), vec![[100.0, 100.0]]);
    }

    #[test]
    fn joins_tees_and_labels() {
        let mut d = Drawing {
            wires: vec![
                vec![[0.0, 0.0], [100.0, 0.0]],
                vec![[50.0, 0.0], [50.0, 50.0]],  // tee onto wire 0
                vec![[0.0, 20.0], [100.0, 20.0]], // crosses wire 1, no junction
                vec![[200.0, 0.0], [300.0, 0.0]],
            ],
            pins: vec![SchPin {
                at: [300.0, 0.0],
                base: [310.0, 0.0],
                part: 0,
                name: String::new(),
                number: "1".into(),
                ..Default::default()
            }],
            tags: vec![
                NetTag {
                    at: vec![[10.0, 0.0]],
                    text: "A".into(),
                    kind: TagKind::Label,
                    graphic: None,
                },
                NetTag {
                    at: vec![[250.0, 0.0]],
                    text: "A".into(),
                    kind: TagKind::Label,
                    graphic: None,
                },
            ],
            ..Default::default()
        };
        d.connect();
        let net = |w: u32| d.nets.iter().position(|n| n.wires.contains(&w)).unwrap();
        assert_eq!(net(0), net(1));
        assert_ne!(net(0), net(2));
        assert_eq!(net(0), net(3));
        assert_eq!(d.nets[net(3)].pins, vec![0]);
        assert_eq!(d.nets[net(0)].name.as_deref(), Some("A"));
    }
}

/// A Trace schematic file (`.tracesch`): every sheet of a project as
/// edited, and the sheets as they were when the board last matched them,
/// so changes can still be carried to the board after reopening.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchFile {
    /// Trace version that wrote it.
    pub version: String,
    /// The project it came from, if any.
    #[serde(default)]
    pub project: Option<String>,
    pub sheets: Vec<SavedSheet>,
    /// Designators renamed on the sheets and not yet on the board: now ->
    /// as on the board.
    #[serde(default)]
    pub renames: std::collections::BTreeMap<String, String>,
    /// Every designator renamed since the sheets were first read: now -> as
    /// in the original board file.
    #[serde(default)]
    pub original_renames: std::collections::BTreeMap<String, String>,
    /// How the sheets' nets join.
    #[serde(default, skip_serializing_if = "is_automatic")]
    pub scope: crate::hier::Scope,
}

fn is_automatic(s: &crate::hier::Scope) -> bool {
    *s == crate::hier::Scope::Automatic
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedSheet {
    /// The sheet's file name, as sheet symbols refer to it.
    pub file: String,
    pub sheet: Sheet,
    /// The drawing as the board last matched it (no pictures).
    #[serde(default)]
    pub synced: Option<Drawing>,
    /// The drawing as first read, which the original board file matches.
    #[serde(default)]
    pub original: Option<Drawing>,
}

impl SchFile {
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let f = std::io::BufWriter::new(std::fs::File::create(path)?);
        serde_json::to_writer(f, self).map_err(std::io::Error::other)
    }

    pub fn load(path: &std::path::Path) -> std::io::Result<SchFile> {
        let f = std::io::BufReader::new(std::fs::File::open(path)?);
        serde_json::from_reader(f).map_err(std::io::Error::other)
    }
}
