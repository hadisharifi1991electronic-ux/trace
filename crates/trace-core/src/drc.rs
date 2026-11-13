//! Design rule check: clearance between copper of different nets, shorts,
//! track widths and nets left unrouted, against the board's own rules; and
//! the manufacturing rules (hole size and spacing, annular ring, solder
//! mask slivers, silkscreen over mask openings, copper near the board
//! edge), in `fab`.
//!
//! Rule scopes are read as Altium query expressions: nets, classes and
//! pairs (`InNet`, `InNetClass`, `InDifferentialPair`, ...), kinds of
//! object (`IsTrack`, `IsVia`, `IsPad`, `InPolygon`, ...), parts
//! (`InComponent`, `HasFootprint`, `InComponentClass`), `OnLayer`,
//! `HoleSize`, and rooms (`WithinRoom`, `TouchesRoom`, from the board's
//! room definitions). Each rule is first judged by net and kind of object;
//! where that cannot decide, on the two objects themselves. A term Trace
//! cannot judge leaves the rule as a maybe, and the smaller gap wins.

use crate::route::{inside, land, pt, seg_seg, Shape, P};
use crate::{Board, LayerRef, Length, Rule};
use std::collections::{HashMap, HashSet};

#[path = "drc_fab.rs"]
mod fab;
#[path = "drc_si.rs"]
mod si;
pub use fab::{mask_rule, MaskRule};
pub use si::{GAP_RUN, PARALLEL_LIMIT};

/// The kinds of copper object rules tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Obj {
    Track,
    Arc,
    Via,
    SmdPad,
    ThPad,
    Pour,
    Region,
    Hole,
    /// A whole part, as placement rules see it.
    Part,
}

impl Obj {
    pub fn label(self) -> &'static str {
        match self {
            Obj::Track => "track",
            Obj::Arc => "arc",
            Obj::Via => "via",
            Obj::SmdPad => "SMD pad",
            Obj::ThPad => "through-hole pad",
            Obj::Pour => "pour",
            Obj::Region => "copper region",
            Obj::Hole => "hole",
            Obj::Part => "part",
        }
    }
    /// Name in Altium's per-object clearance table.
    fn altium(self) -> &'static str {
        match self {
            Obj::Track => "Track",
            Obj::Arc => "Arc",
            Obj::Via => "Via",
            Obj::SmdPad => "SMDPad",
            Obj::ThPad => "THPad",
            Obj::Pour => "Poly",
            Obj::Region => "Region",
            Obj::Hole => "Hole",
            Obj::Part => "Component",
        }
    }
    const ALL: [Obj; 8] = [
        Obj::Track,
        Obj::Arc,
        Obj::Via,
        Obj::SmdPad,
        Obj::ThPad,
        Obj::Pour,
        Obj::Region,
        Obj::Hole,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Clearance,
    Short,
    Width,
    /// Copper closer to the board edge than its rule allows.
    BoardEdge,
    HoleToHole,
    AnnularRing,
    HoleSize,
    /// A strip of solder mask between two openings too thin to stay on.
    MaskSliver,
    /// Silkscreen on or too near a solder mask opening.
    SilkToMask,
    /// Silkscreen of one part on or too near another's.
    SilkToSilk,
    /// Two parts on the same side closer than their rule allows.
    PartClearance,
    /// A track over a void or a split in the planes next to it.
    ReturnPath,
    /// Two nets running side by side too close for too long.
    Crosstalk,
    Unrouted,
}

impl Kind {
    pub const ALL: [Kind; 14] = [
        Kind::Short,
        Kind::Clearance,
        Kind::Width,
        Kind::BoardEdge,
        Kind::HoleToHole,
        Kind::AnnularRing,
        Kind::HoleSize,
        Kind::MaskSliver,
        Kind::SilkToMask,
        Kind::SilkToSilk,
        Kind::PartClearance,
        Kind::ReturnPath,
        Kind::Crosstalk,
        Kind::Unrouted,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Clearance => "Clearance",
            Kind::Short => "Short circuit",
            Kind::Width => "Width",
            Kind::BoardEdge => "Board edge",
            Kind::HoleToHole => "Hole to hole",
            Kind::AnnularRing => "Annular ring",
            Kind::HoleSize => "Hole size",
            Kind::MaskSliver => "Mask sliver",
            Kind::SilkToMask => "Silk to mask",
            Kind::SilkToSilk => "Silk to silk",
            Kind::PartClearance => "Part clearance",
            Kind::ReturnPath => "Return path",
            Kind::Crosstalk => "Crosstalk",
            Kind::Unrouted => "Unrouted net",
        }
    }
    /// A manufacturing rule rather than an electrical one.
    pub fn is_fab(self) -> bool {
        matches!(
            self,
            Kind::BoardEdge
                | Kind::HoleToHole
                | Kind::AnnularRing
                | Kind::HoleSize
                | Kind::MaskSliver
                | Kind::SilkToMask
                | Kind::SilkToSilk
                | Kind::PartClearance
        )
    }
}

/// The board item a violation is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    Track(usize),
    Arc(usize),
    Via(usize),
    Pad(usize),
    Polygon(usize),
    /// A filled shape off copper (`Geometry::areas`), such as a solder
    /// mask opening drawn by hand.
    Area(usize),
    /// A whole part (`Board::components`).
    Part(usize),
}

#[derive(Clone, Debug)]
pub struct Violation {
    pub kind: Kind,
    /// Copper layer, `None` when it is not on one layer.
    pub layer: Option<u8>,
    /// Where to look, in mil.
    pub at: P,
    pub nets: [Option<u32>; 2],
    pub items: [Option<Item>; 2],
    pub objects: [Option<Obj>; 2],
    /// What was found and what the rule asks for, in mil.
    pub found: f64,
    pub need: f64,
    pub rule: String,
}

// ---------------------------------------------------------------- scopes

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Open,
    Close,
    Other(char),
}

fn tokens(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            '\'' | '"' => {
                let q = c[i];
                let start = i + 1;
                i += 1;
                while i < c.len() && c[i] != q {
                    i += 1;
                }
                out.push(Tok::Str(c[start..i.min(c.len())].iter().collect()));
                i += 1;
            }
            ch if ch.is_alphanumeric() || ch == '_' => {
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Word(c[start..i].iter().collect()));
            }
            ch => {
                out.push(Tok::Other(ch));
                i += 1;
            }
        }
    }
    out
}

/// The part an object belongs to, as far as a scope can ask.
#[derive(Clone, Copy)]
enum Part<'a> {
    /// Not known here: terms about parts are left undecided.
    Unknown,
    /// Belongs to no part.
    Free,
    In {
        designator: &'a str,
        footprint: &'a str,
        classes: &'a [String],
        /// Whether any of its pads has a hole.
        thru: bool,
    },
}

/// What a scope term is judged on.
struct Subject<'a> {
    net: &'a str,
    obj: Obj,
    classes: &'a [String],
    pair: Option<&'a str>,
    pair_classes: &'a [String],
    part: Part<'a>,
    /// The object's hole (mil), for `HoleSize` comparisons.
    hole: Option<f64>,
    /// The copper layer's name, for `OnLayer`.
    layer: Option<&'a str>,
    /// The object's extent (mil), for `WithinRoom` and `TouchesRoom`.
    bbox: Option<[f64; 4]>,
    rooms: &'a [Room],
}

/// The board's rooms (Altium room definitions): each name and outline in
/// mil, for drawing.
pub fn rooms(board: &Board) -> Vec<(String, Vec<[f64; 2]>)> {
    Room::all(board)
        .into_iter()
        .map(|r| (r.name, r.ring))
        .collect()
}

/// An Altium room: a named area that rule scopes can ask about.
pub(crate) struct Room {
    name: String,
    ring: Vec<P>,
    bb: [f64; 4],
}

impl Room {
    /// Rooms are `RoomDefinition` rules with an outline as `VX0`/`VY0`...
    /// (arc vertices taken as corners). Disabled ones still name an area.
    fn all(board: &Board) -> Vec<Room> {
        board
            .rules
            .iter()
            .filter(|r| r.kind.eq_ignore_ascii_case("RoomDefinition"))
            .filter_map(|r| {
                let mut ring = Vec::new();
                for i in 0.. {
                    let at = |k: &str| param(r, &format!("{k}{i}")).and_then(Length::parse);
                    let (Some(x), Some(y)) = (at("VX"), at("VY")) else {
                        break;
                    };
                    ring.push([x.mil(), y.mil()]);
                }
                if ring.len() > 1 && ring.first() == ring.last() {
                    ring.pop();
                }
                (ring.len() >= 3).then(|| Room {
                    name: r.name.clone(),
                    bb: ring
                        .iter()
                        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                            [
                                b[0].min(p[0]),
                                b[1].min(p[1]),
                                b[2].max(p[0]),
                                b[3].max(p[1]),
                            ]
                        }),
                    ring,
                })
            })
            .collect()
    }

    fn corners(b: [f64; 4]) -> [P; 4] {
        [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
    }

    /// Whether the extent lies wholly inside the room.
    fn holds(&self, b: [f64; 4]) -> bool {
        Self::corners(b).iter().all(|&c| inside(c, &self.ring))
    }

    /// Whether the extent and the room overlap.
    fn touches(&self, b: [f64; 4]) -> bool {
        let r = self.bb;
        if b[2] < r[0] || b[0] > r[2] || b[3] < r[1] || b[1] > r[3] {
            return false;
        }
        let edges = |v: &[P]| {
            (0..v.len())
                .map(|i| (v[i], v[(i + 1) % v.len()]))
                .collect::<Vec<_>>()
        };
        let mine = Self::corners(b);
        Self::corners(b).iter().any(|&c| inside(c, &self.ring))
            || self
                .ring
                .iter()
                .any(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3])
            || edges(&mine).iter().any(|e| {
                edges(&self.ring)
                    .iter()
                    .any(|f| seg_seg(e.0, e.1, f.0, f.1) <= 0.0)
            })
    }
}

/// Altium footprint patterns: `*` for any run of characters, `?` for one,
/// without case.
fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(st) = star {
            i = st + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

struct Parser<'a> {
    t: Vec<Tok>,
    i: usize,
    s: &'a Subject<'a>,
}

// Three-valued (Kleene): `None` where a term asks about something Trace
// cannot judge here, such as a room without the object's place; `false
// and unknown` is still false. A
// scope that does not parse is unknown as a whole.
type Tri = Option<bool>;

impl Parser<'_> {
    fn peek_word(&self, w: &str) -> bool {
        matches!(self.t.get(self.i), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }
    fn expr(&mut self) -> Result<Tri, ()> {
        let mut v = self.and()?;
        while self.peek_word("or") {
            self.i += 1;
            let r = self.and()?;
            v = match (v, r) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
        }
        Ok(v)
    }
    fn and(&mut self) -> Result<Tri, ()> {
        let mut v = self.unary()?;
        while self.peek_word("and") {
            self.i += 1;
            let r = self.unary()?;
            v = match (v, r) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
        }
        Ok(v)
    }
    fn unary(&mut self) -> Result<Tri, ()> {
        if self.peek_word("not") {
            self.i += 1;
            return self.unary().map(|v| v.map(|v| !v));
        }
        match self.t.get(self.i).cloned() {
            Some(Tok::Open) => {
                self.i += 1;
                let v = self.expr()?;
                if self.t.get(self.i) == Some(&Tok::Close) {
                    self.i += 1;
                }
                Ok(v)
            }
            Some(Tok::Word(w)) => {
                self.i += 1;
                let mut args = Vec::new();
                if self.t.get(self.i) == Some(&Tok::Open) {
                    self.i += 1;
                    while let Some(t) = self.t.get(self.i).cloned() {
                        self.i += 1;
                        match t {
                            Tok::Close => break,
                            Tok::Str(s) => args.push(s),
                            Tok::Word(s) => args.push(s),
                            _ => {}
                        }
                    }
                }
                // Comparisons: a whole part's kind and name are known
                // (`ObjectKind = 'Component'`, `Name Like 'WASHER*'`); the
                // rest (`HoleSize = 45`) are beyond these terms: skip the
                // operator and value, unknown.
                if args.is_empty() && matches!(self.t.get(self.i), Some(Tok::Other(_)))
                    || self.peek_word("like")
                {
                    let mut op = String::new();
                    let mut value = None;
                    while matches!(
                        self.t.get(self.i),
                        Some(Tok::Other(_)) | Some(Tok::Word(_)) | Some(Tok::Str(_))
                    ) && !self.peek_word("and")
                        && !self.peek_word("or")
                    {
                        match &self.t[self.i] {
                            Tok::Other(c) => op.push(*c),
                            Tok::Word(x) if value.is_none() && x.eq_ignore_ascii_case("like") => {
                                op = "like".into()
                            }
                            Tok::Str(v) | Tok::Word(v) if value.is_none() => {
                                value = Some(v.clone())
                            }
                            _ => {}
                        }
                        self.i += 1;
                    }
                    return Ok(value.and_then(|v| self.compare(&w, &op, &v)));
                }
                Ok(self.term(&w, &args))
            }
            _ => Err(()),
        }
    }
    /// A comparison a whole part can answer: its kind or its designator.
    fn compare(&self, w: &str, op: &str, v: &str) -> Tri {
        let s = self.s;
        if w.eq_ignore_ascii_case("holesize") {
            let (h, v) = (s.hole?, Length::parse(v).map(Length::mil)?);
            let e = 1e-3;
            return match op {
                "=" => Some((h - v).abs() < e),
                "<>" => Some((h - v).abs() >= e),
                "<" => Some(h < v - e),
                "<=" => Some(h <= v + e),
                ">" => Some(h > v + e),
                ">=" => Some(h >= v - e),
                _ => None,
            };
        }
        if s.obj != Obj::Part {
            return None;
        }
        let Part::In { designator, .. } = s.part else {
            return None;
        };
        let w = w.to_ascii_lowercase();
        let found = match w.as_str() {
            "objectkind" => v.eq_ignore_ascii_case("component"),
            // A 3D body's name: never a part's.
            "identifier" => false,
            "name" => match op {
                "like" => glob(v, designator),
                _ => v.eq_ignore_ascii_case(designator),
            },
            _ => return None,
        };
        match op {
            "=" | "like" => Some(found),
            "<>" => Some(!found),
            _ => None,
        }
    }

    fn term(&self, w: &str, args: &[String]) -> Tri {
        let s = self.s;
        let arg = args.first().map(String::as_str).unwrap_or("");
        let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let w = w.to_ascii_lowercase();
        let part = |f: &dyn Fn(&str, &str, &[String]) -> bool| match s.part {
            Part::Unknown => None,
            Part::Free => Some(false),
            Part::In {
                designator,
                footprint,
                classes,
                ..
            } => Some(f(designator, footprint, classes)),
        };
        // Whether a whole part is through-hole: only asked of parts.
        let thru = match (s.obj, s.part) {
            (Obj::Part, Part::In { thru, .. }) => Some(thru),
            _ => None,
        };
        match w.as_str() {
            "hasfootprint" => return part(&|_, f, _| glob(arg, f)),
            "incomponent" => return part(&|d, _, _| eq(d, arg)),
            "incomponentclass" => return part(&|_, _, c| c.iter().any(|c| eq(c, arg))),
            "inanycomponent" => return part(&|_, _, _| true),
            "isfree" => return part(&|_, _, _| true).map(|v| !v),
            "hasfootprintpad" if matches!(s.part, Part::Free) => return Some(false),
            "isthrucomponent" => return thru,
            "withinroom" | "touchesroom" => {
                // A room the board does not have holds nothing.
                let Some(room) = s.rooms.iter().find(|r| eq(&r.name, arg)) else {
                    return Some(false);
                };
                let b = s.bbox?;
                return Some(if w == "withinroom" {
                    room.holds(b)
                } else {
                    room.touches(b)
                });
            }
            "issmtcomponent" => return thru.map(|t| !t),
            _ => {}
        }
        Some(match w.as_str() {
            "all" | "oncopper" | "onsignal" => true,
            "innet" => eq(arg, s.net),
            "onlayer" => eq(arg, s.layer?),
            "innetclass" => s.classes.iter().any(|c| eq(c, arg)),
            "indifferentialpair" => s.pair.is_some_and(|p| eq(p, arg)),
            "inanydifferentialpair" => s.pair.is_some(),
            "indifferentialpairclass" => s.pair_classes.iter().any(|c| eq(c, arg)),
            "inpolygon" | "ispolygon" | "inpoly" | "ispoly" | "inanypolygon" => s.obj == Obj::Pour,
            "istrack" => s.obj == Obj::Track,
            "isarc" => s.obj == Obj::Arc,
            "isvia" => s.obj == Obj::Via,
            "ispad" => matches!(s.obj, Obj::SmdPad | Obj::ThPad),
            "issmdpad" => s.obj == Obj::SmdPad,
            "isthpad" => s.obj == Obj::ThPad,
            "isregion" => s.obj == Obj::Region,
            "isfill" => false,
            "iskeepout" => false,
            _ => return None,
        })
    }
}

/// One object as a rule scope sees it, for judging scopes outside DRC.
pub(crate) struct Thing {
    pub net: Option<u32>,
    pub obj: Obj,
    pub component: Option<u32>,
    /// Hole size in mil, for pads and vias.
    pub hole: Option<f64>,
    /// Copper layer index.
    pub layer: Option<u8>,
    /// Extent in mil, for rooms.
    pub bbox: Option<[f64; 4]>,
}

/// Judges rule scopes against single objects of one board.
pub(crate) struct Scopes {
    info: NetInfo,
}

impl Scopes {
    pub(crate) fn new(board: &Board) -> Scopes {
        Scopes {
            info: NetInfo::new(board),
        }
    }

    /// Whether `scope` takes the object: yes, no, or `None` when it asks
    /// what Trace cannot tell (a room).
    pub(crate) fn judge(&self, scope: &str, t: &Thing) -> Option<bool> {
        let mut s = if t.obj == Obj::Pour {
            self.info.subject(t.net, t.obj)
        } else {
            self.info.subject_in(t.net, t.obj, t.component)
        };
        s.hole = t.hole;
        s.layer = t
            .layer
            .and_then(|l| self.info.layers.get(l as usize))
            .map(String::as_str);
        s.bbox = t.bbox;
        judge(scope, &s)
    }
}

fn judge(scope: &str, s: &Subject) -> Option<bool> {
    let mut p = Parser {
        t: tokens(scope),
        i: 0,
        s,
    };
    match p.expr() {
        Ok(v) if p.i >= p.t.len() => v,
        _ => None,
    }
}

/// Each net's classes and pair, for judging scopes.
struct NetInfo {
    names: Vec<String>,
    classes: Vec<Vec<String>>,
    pairs: Vec<Option<String>>,
    pair_classes: Vec<Vec<String>>,
    /// Each component's designator, footprint and classes.
    parts: Vec<(String, String, Vec<String>)>,
    /// Whether each component has a pad with a hole.
    thru: Vec<bool>,
    rooms: Vec<Room>,
    /// Copper layer names, top first.
    layers: Vec<String>,
}

impl NetInfo {
    fn new(board: &Board) -> Self {
        let n = board.nets.len();
        let index: HashMap<&str, usize> = board
            .nets
            .iter()
            .enumerate()
            .map(|(i, x)| (x.name.as_str(), i))
            .collect();
        let mut classes = vec![Vec::new(); n];
        for c in &board.net_classes {
            for m in &c.members {
                if let Some(&i) = index.get(m.as_str()) {
                    classes[i].push(c.name.clone());
                }
            }
        }
        let mut pairs = vec![None; n];
        let mut pair_classes = vec![Vec::new(); n];
        for p in &board.diff_pairs {
            let cls: Vec<String> = board
                .diff_pair_classes
                .iter()
                .filter(|c| c.members.contains(&p.name))
                .map(|c| c.name.clone())
                .collect();
            for m in [&p.positive, &p.negative] {
                if let Some(&i) = index.get(m.as_str()) {
                    pairs[i] = Some(p.name.clone());
                    pair_classes[i] = cls.clone();
                }
            }
        }
        let parts = board
            .components
            .iter()
            .map(|c| {
                let cls = board
                    .component_classes
                    .iter()
                    .filter(|k| {
                        k.members
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(&c.designator))
                    })
                    .map(|k| k.name.clone())
                    .collect();
                (c.designator.clone(), c.footprint.clone(), cls)
            })
            .collect();
        let mut thru = vec![false; board.components.len()];
        for p in &board.geometry.pads {
            if let Some(t) = p
                .component
                .and_then(|c| thru.get_mut(c as usize))
                .filter(|_| p.hole.mil() > 0.0)
            {
                *t = true;
            }
        }
        NetInfo {
            names: board.nets.iter().map(|x| x.name.clone()).collect(),
            classes,
            pairs,
            pair_classes,
            parts,
            thru,
            rooms: Room::all(board),
            layers: board
                .stackup
                .copper_layers()
                .map(|l| l.name.clone())
                .collect(),
        }
    }

    /// As `subject`, for one object whose part is known: `None` for a
    /// free object.
    fn subject_in(&self, net: Option<u32>, obj: Obj, component: Option<u32>) -> Subject<'_> {
        let mut s = self.subject(net, obj);
        s.part = match component.and_then(|c| self.parts.get(c as usize)) {
            Some((d, f, c)) => Part::In {
                designator: d,
                footprint: f,
                classes: c,
                thru: component.is_some_and(|i| self.thru[i as usize]),
            },
            None => Part::Free,
        };
        s
    }

    fn subject(&self, net: Option<u32>, obj: Obj) -> Subject<'_> {
        static NONE: Vec<String> = Vec::new();
        // Pours never belong to a part.
        let part = if obj == Obj::Pour {
            Part::Free
        } else {
            Part::Unknown
        };
        match net.map(|n| n as usize).filter(|&n| n < self.names.len()) {
            Some(n) => Subject {
                net: &self.names[n],
                obj,
                classes: &self.classes[n],
                pair: self.pairs[n].as_deref(),
                pair_classes: &self.pair_classes[n],
                part,
                hole: None,
                layer: None,
                bbox: None,
                rooms: &self.rooms,
            },
            None => Subject {
                net: "No Net",
                obj,
                classes: &NONE,
                pair: None,
                pair_classes: &NONE,
                part,
                hole: None,
                layer: None,
                bbox: None,
                rooms: &self.rooms,
            },
        }
    }
}

// ---------------------------------------------------------------- rules

/// A clearance rule ready to apply: its gap for each pair of objects, and
/// which (net, object) each of its two scopes covers.
struct ClearanceRule {
    name: String,
    gaps: HashMap<(Obj, Obj), f64>,
    gap: f64,
    /// `[scope][net + 1][object]`; net 0 is no net. `None` where the scope
    /// asks about something Trace cannot judge (a room, a footprint).
    covers: [Vec<[Option<bool>; 8]>; 2],
    pad_pad_in_part: bool,
    scopes: [String; 2],
}

fn param<'a>(r: &'a Rule, k: &str) -> Option<&'a str> {
    r.params
        .iter()
        .find(|(pk, _)| pk.eq_ignore_ascii_case(k))
        .map(|(_, v)| v.as_str())
}

fn obj_of(name: &str) -> Option<Obj> {
    let n = name.trim().trim_start_matches("ClearanceObj_");
    // Fills have their own column in Altium's table; Trace reads them as
    // regions but leaves their column alone rather than guess.
    Obj::ALL
        .into_iter()
        .find(|o| o.altium().eq_ignore_ascii_case(n))
}

impl ClearanceRule {
    fn new(r: &Rule, info: &NetInfo) -> Option<Self> {
        let gap = param(r, "GAP").and_then(Length::parse)?.mil();
        let mut gaps = HashMap::new();
        for item in param(r, "OBJECTCLEARANCES").unwrap_or("").split(';') {
            let Some((pair, v)) = item.split_once(':') else {
                continue;
            };
            let Some((a, b)) = pair.split_once('-') else {
                continue;
            };
            if let (Some(a), Some(b), Ok(v)) = (obj_of(a), obj_of(b), v.trim().parse::<f64>()) {
                gaps.insert((a, b), v / 10_000.0);
                gaps.insert((b, a), v / 10_000.0);
            }
        }
        let scopes = [
            r.scope.first().map(String::as_str).unwrap_or("All"),
            r.scope.get(1).map(String::as_str).unwrap_or("All"),
        ];
        let mut covers: [Vec<[Option<bool>; 8]>; 2] = [Vec::new(), Vec::new()];
        for (k, scope) in scopes.iter().enumerate() {
            for n in 0..=info.names.len() {
                let net = n.checked_sub(1).map(|n| n as u32);
                let mut row = [None; 8];
                for (j, o) in Obj::ALL.into_iter().enumerate() {
                    row[j] = judge(scope, &info.subject(net, o));
                }
                covers[k].push(row);
            }
        }
        Some(ClearanceRule {
            name: r.name.clone(),
            gaps,
            gap,
            covers,
            pad_pad_in_part: !param(r, "IGNOREPADTOPADCLEARANCEINFOOTPRINT")
                .is_some_and(|v| v.eq_ignore_ascii_case("TRUE")),
            scopes: scopes.map(String::from),
        })
    }

    fn covers(&self, scope: usize, net: Option<u32>, obj: Obj) -> Option<bool> {
        let n = net.map(|n| n as usize + 1).unwrap_or(0);
        let j = Obj::ALL.iter().position(|&o| o == obj).unwrap_or(0);
        self.covers[scope].get(n).map_or(Some(false), |row| row[j])
    }

    /// Whether the rule takes this pair: yes, no, or `None` for maybe.
    fn applies(&self, a: (Option<u32>, Obj), b: (Option<u32>, Obj)) -> Option<bool> {
        let and = |x: Option<bool>, y: Option<bool>| match (x, y) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        };
        let or = |x: Option<bool>, y: Option<bool>| match (x, y) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        };
        or(
            and(self.covers(0, a.0, a.1), self.covers(1, b.0, b.1)),
            and(self.covers(0, b.0, b.1), self.covers(1, a.0, a.1)),
        )
    }

    /// As `applies`, judged on the two objects themselves: their parts,
    /// layer and extent (for rooms). Answers are kept in `seen` by rule
    /// `k`, scope and object.
    fn applies_to(
        &self,
        k: usize,
        info: &NetInfo,
        a: &Copper,
        b: &Copper,
        seen: &Seen,
    ) -> Option<bool> {
        let one = |scope: usize, c: &Copper| -> Option<bool> {
            let key = (k, scope, c.item);
            if let Some(v) = seen.borrow().get(&key) {
                return *v;
            }
            let mut s = if c.obj == Obj::Pour {
                info.subject(c.net, c.obj)
            } else {
                info.subject_in(c.net, c.obj, c.component)
            };
            s.layer = c
                .layer
                .and_then(|l| info.layers.get(l as usize))
                .map(String::as_str);
            s.bbox = Some(c.shape.bbox());
            let v = judge(&self.scopes[scope], &s);
            seen.borrow_mut().insert(key, v);
            v
        };
        let and = |x: Option<bool>, y: Option<bool>| match (x, y) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        };
        let either = |x: Option<bool>, y: Option<bool>| match (x, y) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        };
        let first = and(self.covers(0, a.net, a.obj), self.covers(1, b.net, b.obj));
        let first = if first.is_none() {
            and(one(0, a), one(1, b))
        } else {
            first
        };
        if first == Some(true) {
            return first;
        }
        let second = and(self.covers(0, b.net, b.obj), self.covers(1, a.net, a.obj));
        let second = if second.is_none() {
            and(one(0, b), one(1, a))
        } else {
            second
        };
        either(first, second)
    }

    fn gap(&self, a: Obj, b: Obj) -> f64 {
        self.gaps.get(&(a, b)).copied().unwrap_or(self.gap)
    }
}

// ---------------------------------------------------------------- copper

/// Scope answers per clearance rule, scope and object.
type Seen = std::cell::RefCell<HashMap<(usize, usize, Item), Option<bool>>>;

struct Copper {
    layer: Option<u8>,
    net: Option<u32>,
    obj: Obj,
    shape: Shape,
    item: Item,
    component: Option<u32>,
}

const CELL: f64 = 100.0;

fn shape_gap(x: &Shape, y: &Shape) -> f64 {
    match (x, y) {
        (Shape::Capsule { a, b, r }, other) | (other, Shape::Capsule { a, b, r }) => {
            other.gap(*a, *b, *r)
        }
        (Shape::Poly(u), Shape::Poly(v)) => {
            if u.len() < 3 || v.len() < 3 {
                return f64::MAX;
            }
            if inside(u[0], v) || inside(v[0], u) {
                return -1.0;
            }
            let mut best = f64::MAX;
            for i in 0..u.len() {
                let (p, q) = (u[i], u[(i + 1) % u.len()]);
                for j in 0..v.len() {
                    best = best.min(seg_seg(p, q, v[j], v[(j + 1) % v.len()]));
                }
            }
            if best == 0.0 {
                -1.0
            } else {
                best
            }
        }
    }
}

fn center(s: &Shape) -> P {
    match s {
        Shape::Capsule { a, b, .. } => [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
        Shape::Poly(v) => {
            let n = v.len().max(1) as f64;
            v.iter()
                .fold([0.0, 0.0], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n])
        }
    }
}

/// A copper area: layer, polygon index, outline then holes, bounding box.
type Area = (u8, usize, Vec<Vec<P>>, [f64; 4]);

fn copper(board: &Board) -> (Vec<Copper>, Vec<Area>) {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let g = &board.geometry;
    let mut out = Vec::new();
    let layer_of = |l: &LayerRef| match l {
        LayerRef::Copper(i) => Some(Some(*i)),
        LayerRef::AllCopper => Some(None),
        _ => None,
    };
    for (i, t) in g.tracks.iter().enumerate() {
        if let Some(Some(layer)) = layer_of(&t.layer) {
            out.push(Copper {
                layer: Some(layer),
                net: t.net,
                obj: Obj::Track,
                shape: Shape::Capsule {
                    a: pt(t.start),
                    b: pt(t.end),
                    r: t.width.mil() / 2.0,
                },
                item: Item::Track(i),
                component: None,
            });
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        let Some(Some(layer)) = layer_of(&a.layer) else {
            continue;
        };
        let (c, r) = (pt(a.center), a.radius.mil());
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let k = (sweep / 5.0).ceil().max(1.0) as usize;
        let at = |j: usize| {
            let t = (a.start_deg + sweep * j as f64 / k as f64).to_radians();
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        };
        for j in 0..k {
            out.push(Copper {
                layer: Some(layer),
                net: a.net,
                obj: Obj::Arc,
                shape: Shape::Capsule {
                    a: at(j),
                    b: at(j + 1),
                    r: a.width.mil() / 2.0,
                },
                item: Item::Arc(i),
                component: None,
            });
        }
    }
    for (i, v) in g.vias.iter().enumerate() {
        let c = pt(v.at);
        let (lo, hi) = (
            v.from_layer.min(v.to_layer),
            v.from_layer.max(v.to_layer).min(n - 1),
        );
        for l in lo..=hi {
            let d = v
                .rings
                .iter()
                .find(|(rl, _)| *rl == l)
                .map(|(_, d)| d.mil())
                .unwrap_or(v.diameter.mil());
            out.push(Copper {
                layer: Some(l),
                net: v.net,
                obj: if d <= v.hole.mil() {
                    Obj::Hole
                } else {
                    Obj::Via
                },
                shape: Shape::Capsule {
                    a: c,
                    b: c,
                    r: d.max(v.hole.mil()) / 2.0,
                },
                item: Item::Via(i),
                component: None,
            });
        }
    }
    for (i, p) in g.pads.iter().enumerate() {
        let c = pt(p.at);
        let obj = if p.hole.0 > 0 {
            Obj::ThPad
        } else {
            Obj::SmdPad
        };
        let layers: Vec<u8> = match layer_of(&p.layer) {
            Some(Some(l)) => vec![l],
            Some(None) => (0..n).collect(),
            None => continue,
        };
        for l in layers {
            let shape = if p.stack.is_empty() {
                Some(land(
                    c,
                    p.shape,
                    p.size_x.mil(),
                    p.size_y.mil(),
                    p.rotation_deg,
                    &p.outline,
                ))
            } else {
                p.stack.iter().find(|s| s.layer == l).map(|s| {
                    land(
                        c,
                        s.shape,
                        s.size_x.mil(),
                        s.size_y.mil(),
                        s.rotation_deg,
                        &s.outline,
                    )
                })
            };
            // No land on this layer (removed as unused): just the hole.
            let (obj, shape) = match shape {
                Some(s) => (obj, s),
                None if p.hole.0 > 0 => (
                    Obj::Hole,
                    Shape::Capsule {
                        a: c,
                        b: c,
                        r: p.hole.mil() / 2.0,
                    },
                ),
                None => continue,
            };
            out.push(Copper {
                layer: Some(l),
                net: p.net,
                obj,
                shape,
                item: Item::Pad(i),
                component: p.component,
            });
        }
    }
    // Copper areas: their edges, each as a thin capsule, for gaps; their
    // rings separately, for what sits inside them.
    let mut areas = Vec::new();
    for (i, f) in g.polygons.iter().enumerate() {
        let LayerRef::Copper(l) = f.layer else {
            continue;
        };
        let obj = if f.pour.is_some() {
            Obj::Pour
        } else {
            Obj::Region
        };
        let rings: Vec<Vec<P>> = std::iter::once(&f.outline)
            .chain(f.holes.iter())
            .map(|r| r.iter().map(|&q| pt(q)).collect::<Vec<P>>())
            .filter(|r| r.len() >= 3)
            .collect();
        if rings.is_empty() {
            continue;
        }
        for r in &rings {
            for j in 0..r.len() {
                out.push(Copper {
                    layer: Some(l),
                    net: f.net,
                    obj,
                    shape: Shape::Capsule {
                        a: r[j],
                        b: r[(j + 1) % r.len()],
                        r: 0.0,
                    },
                    item: Item::Polygon(i),
                    component: f.component,
                });
            }
        }
        let b = rings[0]
            .iter()
            .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                [
                    b[0].min(p[0]),
                    b[1].min(p[1]),
                    b[2].max(p[0]),
                    b[3].max(p[1]),
                ]
            });
        areas.push((l, i, rings, b));
    }
    (out, areas)
}

/// Checks the board. At most `limit` violations are returned.
pub fn check(board: &Board, limit: usize) -> Vec<Violation> {
    let info = NetInfo::new(board);
    let mut rules: Vec<&Rule> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("Clearance"))
        .collect();
    rules.sort_by_key(|r| r.priority);
    let clearances: Vec<ClearanceRule> = rules
        .iter()
        .filter_map(|r| ClearanceRule::new(r, &info))
        .collect();
    let fallback = crate::route::board_clearance(board);
    // The first rule by priority that takes the pair. A rule that may take
    // it (its scope names a room or a footprint Trace does not judge) is
    // given the benefit of the doubt: the smaller gap wins.
    let seen: Seen = Default::default();
    let need = |a: &Copper, b: &Copper| -> Option<(f64, &str)> {
        let mut maybe: Option<(f64, &str)> = None;
        for (k, r) in clearances.iter().enumerate() {
            let fits = match r.applies((a.net, a.obj), (b.net, b.obj)) {
                None => r.applies_to(k, &info, a, b, &seen),
                known => known,
            };
            match fits {
                Some(false) => {}
                None => {
                    let g = r.gap(a.obj, b.obj);
                    if maybe.is_none_or(|m| g < m.0) {
                        maybe = Some((g, r.name.as_str()));
                    }
                }
                Some(true) => {
                    if !r.pad_pad_in_part && a.component.is_some() && a.component == b.component {
                        return None;
                    }
                    let g = r.gap(a.obj, b.obj);
                    return Some(match maybe {
                        Some(m) if m.0 < g => m,
                        _ => (g, r.name.as_str()),
                    });
                }
            }
        }
        // Without clearance rules (Gerber, ODB++, Allegro) only copper that
        // overlaps is reported: a guessed gap would flag the whole board.
        maybe.or(clearances.is_empty().then_some((0.02, "short")))
    };

    let (items, areas) = copper(board);
    let mut grid: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
    let max_gap = clearances
        .iter()
        .flat_map(|r| std::iter::once(r.gap).chain(r.gaps.values().copied()))
        .fold(fallback, f64::max);
    for (i, c) in items.iter().enumerate() {
        let Some(l) = c.layer else { continue };
        let b = c.shape.bbox();
        for gx in (b[0] / CELL).floor() as i32..=(b[2] / CELL).floor() as i32 {
            for gy in (b[1] / CELL).floor() as i32..=(b[3] / CELL).floor() as i32 {
                grid.entry((l, gx, gy)).or_default().push(i);
            }
        }
    }

    let mut out: Vec<Violation> = Vec::new();
    // One report per pair of board items (a track against a pour, not
    // against every edge of it), the worst gap kept.
    let mut worst: HashMap<(Item, Item, u8), usize> = HashMap::new();
    for (i, a) in items.iter().enumerate() {
        let Some(l) = a.layer else { continue };
        let b = a.shape.bbox();
        let reach = max_gap + 0.01;
        let mut seen = HashSet::new();
        for gx in ((b[0] - reach) / CELL).floor() as i32..=((b[2] + reach) / CELL).floor() as i32 {
            for gy in
                ((b[1] - reach) / CELL).floor() as i32..=((b[3] + reach) / CELL).floor() as i32
            {
                let Some(list) = grid.get(&(l, gx, gy)) else {
                    continue;
                };
                for &j in list {
                    if j <= i || !seen.insert(j) {
                        continue;
                    }
                    let c = &items[j];
                    if a.item == c.item || a.net == c.net {
                        continue;
                    }
                    // Copper without a net (a free fill, a logo) is not
                    // checked against nets, as Altium has it join whatever
                    // it touches.
                    if a.net.is_none() || c.net.is_none() {
                        continue;
                    }
                    let Some((gap_need, rule)) = need(a, c) else {
                        continue;
                    };
                    let gap = shape_gap(&a.shape, &c.shape);
                    // Poured copper is flattened into short chords, which
                    // can cut up to a few tenths of a mil into a gap.
                    // Arcs too, by a hair.
                    let slack = if a.obj == Obj::Pour || c.obj == Obj::Pour {
                        0.35
                    } else if a.obj == Obj::Arc || c.obj == Obj::Arc {
                        0.03
                    } else {
                        0.01
                    };
                    if gap >= gap_need - slack {
                        continue;
                    }
                    let (x, y) = if a.item <= c.item { (a, c) } else { (c, a) };
                    let key = (x.item, y.item, l);
                    let ca = center(&x.shape);
                    let cb = center(&y.shape);
                    let v = Violation {
                        kind: if gap <= 0.0 {
                            Kind::Short
                        } else {
                            Kind::Clearance
                        },
                        layer: Some(l),
                        at: [(ca[0] + cb[0]) / 2.0, (ca[1] + cb[1]) / 2.0],
                        nets: [x.net, y.net],
                        items: [Some(x.item), Some(y.item)],
                        objects: [Some(x.obj), Some(y.obj)],
                        found: gap.max(0.0),
                        need: gap_need,
                        rule: rule.to_string(),
                    };
                    match worst.get(&key) {
                        Some(&k) if out[k].found <= v.found => {}
                        Some(&k) => out[k] = v,
                        None => {
                            worst.insert(key, out.len());
                            out.push(v);
                        }
                    }
                }
            }
        }
    }
    // Copper sitting inside a copper area of another net.
    for a in items
        .iter()
        .filter(|c| !matches!(c.obj, Obj::Pour | Obj::Region))
    {
        let Some(l) = a.layer else { continue };
        let p = center(&a.shape);
        for (al, ai, rings, bb) in &areas {
            if *al != l || p[0] < bb[0] || p[0] > bb[2] || p[1] < bb[1] || p[1] > bb[3] {
                continue;
            }
            let f = &board.geometry.polygons[*ai];
            if f.net == a.net || f.net.is_none() || a.net.is_none() {
                continue;
            }
            if inside(p, &rings[0]) && !rings[1..].iter().any(|h| inside(p, h)) {
                let key = (
                    a.item.min(Item::Polygon(*ai)),
                    a.item.max(Item::Polygon(*ai)),
                    l,
                );
                if worst.contains_key(&key) {
                    if let Some(&k) = worst.get(&key) {
                        out[k].kind = Kind::Short;
                        out[k].found = 0.0;
                    }
                    continue;
                }
                worst.insert(key, out.len());
                out.push(Violation {
                    kind: Kind::Short,
                    layer: Some(l),
                    at: p,
                    nets: [a.net, f.net],
                    items: [Some(a.item), Some(Item::Polygon(*ai))],
                    objects: [
                        Some(a.obj),
                        Some(if f.pour.is_some() {
                            Obj::Pour
                        } else {
                            Obj::Region
                        }),
                    ],
                    found: 0.0,
                    need: 0.0,
                    rule: "short".into(),
                });
            }
        }
    }

    // Track widths.
    let mut widths: Vec<&Rule> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("Width"))
        .collect();
    widths.sort_by_key(|r| r.priority);
    type Limits = Option<(f64, f64, String)>;
    let mut width_rule: HashMap<(Option<u32>, Obj), Limits> = HashMap::new();
    let mut width_of = |net: Option<u32>, obj: Obj| {
        width_rule
            .entry((net, obj))
            .or_insert_with(|| {
                widths.iter().find_map(|r| {
                    let scope = r.scope.first().map(String::as_str).unwrap_or("All");
                    if judge(scope, &info.subject(net, obj)) != Some(true) {
                        return None;
                    }
                    let min = param(r, "MINLIMIT")
                        .and_then(Length::parse)
                        .map(Length::mil)
                        .unwrap_or(0.0);
                    let max = param(r, "MAXLIMIT")
                        .and_then(Length::parse)
                        .map(Length::mil)
                        .unwrap_or(f64::MAX);
                    Some((min, max, r.name.clone()))
                })
            })
            .clone()
    };
    let g = &board.geometry;
    let mut width_check = |item: Item,
                           net: Option<u32>,
                           obj: Obj,
                           w: f64,
                           layer: Option<u8>,
                           at: P,
                           out: &mut Vec<Violation>| {
        if let Some((min, max, rule)) = width_of(net, obj) {
            let need = if w < min - 0.01 {
                min
            } else if w > max + 0.01 {
                max
            } else {
                return;
            };
            out.push(Violation {
                kind: Kind::Width,
                layer,
                at,
                nets: [net, None],
                items: [Some(item), None],
                objects: [Some(obj), None],
                found: w,
                need,
                rule,
            });
        }
    };
    for (i, t) in g.tracks.iter().enumerate() {
        if let LayerRef::Copper(l) = t.layer {
            let (a, b) = (pt(t.start), pt(t.end));
            width_check(
                Item::Track(i),
                t.net,
                Obj::Track,
                t.width.mil(),
                Some(l),
                [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
                &mut out,
            );
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        if let LayerRef::Copper(l) = a.layer {
            width_check(
                Item::Arc(i),
                a.net,
                Obj::Arc,
                a.width.mil(),
                Some(l),
                pt(a.center),
                &mut out,
            );
        }
    }

    // Nets whose pads are not all joined (nets with pours are left to the
    // pours, which this check does not trace through).
    for (n, net) in board.nets.iter().enumerate() {
        if net.pieces > 1 && !net.has_pour {
            let at = g
                .pads
                .iter()
                .find(|p| p.net == Some(n as u32))
                .map(|p| pt(p.at))
                .unwrap_or([0.0, 0.0]);
            out.push(Violation {
                kind: Kind::Unrouted,
                layer: None,
                at,
                nets: [Some(n as u32), None],
                items: [None, None],
                objects: [None, None],
                found: net.pieces as f64,
                need: 1.0,
                rule: format!("{} separate pieces", net.pieces),
            });
        }
    }

    fab::check(board, &info, &items, &mut out);
    si::check(board, &info, &mut out);

    out.sort_by(|a, b| {
        let rank = |k: Kind| Kind::ALL.iter().position(|&x| x == k).unwrap_or(0);
        rank(a.kind).cmp(&rank(b.kind)).then(
            a.found
                .partial_cmp(&b.found)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out.truncate(limit);
    out
}

/// One line for a violation, with net names.
pub fn describe(board: &Board, v: &Violation) -> String {
    let net = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "no net".into())
    };
    let obj = |o: Option<Obj>| o.map(Obj::label).unwrap_or("");
    match v.kind {
        Kind::Clearance => format!(
            "{} ({}) to {} ({}): {:.2} mil, needs {:.2} ({})",
            net(v.nets[0]),
            obj(v.objects[0]),
            net(v.nets[1]),
            obj(v.objects[1]),
            v.found,
            v.need,
            v.rule
        ),
        Kind::Short => format!(
            "{} ({}) touches {} ({})",
            net(v.nets[0]),
            obj(v.objects[0]),
            net(v.nets[1]),
            obj(v.objects[1])
        ),
        Kind::Width => format!(
            "{} {}: {:.2} mil wide, rule allows {} {:.2} ({})",
            net(v.nets[0]),
            obj(v.objects[0]),
            v.found,
            if v.found < v.need {
                "at least"
            } else {
                "at most"
            },
            v.need,
            v.rule
        ),
        Kind::Unrouted => format!("{}: {}", net(v.nets[0]), v.rule),
        Kind::ReturnPath | Kind::Crosstalk => si::describe(board, v),
        _ => fab::describe(board, v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use std::collections::BTreeMap;

    fn p(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        }
    }

    fn track(net: u32, y: f64, w: f64) -> Track {
        Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(net),
            start: p(0.0, y),
            end: p(1000.0, y),
            width: Length::from_mil(w),
            text: false,
        }
    }

    fn rule(kind: &str, scope: [&str; 2], prio: i64, params: &[(&str, &str)]) -> Rule {
        Rule {
            kind: kind.into(),
            name: format!("{kind}{prio}"),
            enabled: true,
            priority: prio,
            scope: scope.iter().map(|s| s.to_string()).collect(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["A", "B", "CLK_P"] {
            b.nets.push(Net {
                name: n.into(),
                pieces: 1,
                ..Default::default()
            });
        }
        b.net_classes.push(NetClass {
            name: "Fast".into(),
            members: vec!["CLK_P".into()],
        });
        b.rules
            .push(rule("Clearance", ["All", "All"], 2, &[("GAP", "8mil")]));
        b.rules.push(rule(
            "Clearance",
            ["InNetClass('Fast')", "All"],
            1,
            &[("GAP", "20mil")],
        ));
        b.rules.push(rule(
            "Width",
            ["All", "All"],
            1,
            &[("MINLIMIT", "5mil"), ("MAXLIMIT", "50mil")],
        ));
        b
    }

    #[test]
    fn scopes() {
        let b = board();
        let info = NetInfo::new(&b);
        let s = info.subject(Some(2), Obj::Track);
        assert_eq!(
            judge("InNetClass('Fast') and not InNet('A')", &s),
            Some(true)
        );
        assert_eq!(judge("(InNet('A') or IsVia)", &s), Some(false));
        // A room the board does not have holds nothing.
        assert_eq!(judge("WithinRoom('X')", &s), Some(false));
        assert_eq!(judge("IsPad and HoleSize = 45", &s), Some(false));
        assert_eq!(judge("IsTrack and HoleSize = 45", &s), None);
        let mut s = info.subject(Some(2), Obj::ThPad);
        s.hole = Some(39.0);
        assert_eq!(judge("IsPad and HoleSize <= 39", &s), Some(true));
        assert_eq!(judge("IsPad and HoleSize = 45", &s), Some(false));
        assert_eq!(judge("OnLayer('L2')", &s), None);
        s.layer = Some("L2");
        assert_eq!(judge("OnLayer('L2') and IsPad", &s), Some(true));
        assert_eq!(judge("not InNet('A') or WithinRoom('X')", &s), Some(true));
    }

    #[test]
    fn rooms() {
        let mut b = board();
        let mut room = rule("RoomDefinition", ["All", "All"], 1, &[]);
        room.name = "DAC".into();
        room.enabled = false;
        for (i, (x, y)) in [(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]
            .into_iter()
            .enumerate()
        {
            room.params.insert(format!("VX{i}"), format!("{x}mil"));
            room.params.insert(format!("VY{i}"), format!("{y}mil"));
        }
        b.rules.push(room);
        let info = NetInfo::new(&b);
        let mut s = info.subject(Some(0), Obj::Via);
        assert_eq!(judge("WithinRoom('DAC')", &s), None, "no place");
        s.bbox = Some([10.0, 10.0, 20.0, 20.0]);
        assert_eq!(judge("WithinRoom('DAC')", &s), Some(true));
        assert_eq!(judge("WithinRoom('FMC')", &s), Some(false));
        s.bbox = Some([90.0, 10.0, 110.0, 20.0]);
        assert_eq!(judge("WithinRoom('DAC')", &s), Some(false));
        assert_eq!(judge("TouchesRoom('DAC')", &s), Some(true));
        s.bbox = Some([-50.0, -50.0, 150.0, 150.0]);
        assert_eq!(judge("TouchesRoom('DAC')", &s), Some(true), "around it");
        s.bbox = Some([200.0, 10.0, 210.0, 20.0]);
        assert_eq!(judge("TouchesRoom('DAC')", &s), Some(false));
    }

    #[test]
    fn room_rules_apply_inside_their_room() {
        let mut b = board();
        // A and B 10 mil apart edge to edge: fine under 8 mil, not under
        // the 12 mil of a rule for room R.
        b.geometry.tracks.push(track(0, 0.0, 10.0));
        b.geometry.tracks.push(track(1, 20.0, 10.0));
        b.rules.push(rule(
            "Clearance",
            ["WithinRoom('R')", "All"],
            1,
            &[("GAP", "12mil")],
        ));
        let mut room = rule("RoomDefinition", ["All", "All"], 1, &[]);
        room.name = "R".into();
        let mut add = |x: f64, y: f64, i: usize| {
            room.params.insert(format!("VX{i}"), format!("{x}mil"));
            room.params.insert(format!("VY{i}"), format!("{y}mil"));
        };
        // Far from the tracks first: the rule does not apply.
        for (i, (x, y)) in [(0.0, 500.0), (100.0, 500.0), (100.0, 600.0), (0.0, 600.0)]
            .into_iter()
            .enumerate()
        {
            add(x, y, i);
        }
        b.rules.push(room.clone());
        assert!(check(&b, 100).is_empty());
        // Around them: it does.
        let room = b.rules.last_mut().unwrap();
        for (i, (x, y)) in [
            (-50.0, -50.0),
            (1050.0, -50.0),
            (1050.0, 50.0),
            (-50.0, 50.0),
        ]
        .into_iter()
        .enumerate()
        {
            room.params.insert(format!("VX{i}"), format!("{x}mil"));
            room.params.insert(format!("VY{i}"), format!("{y}mil"));
        }
        let v = check(&b, 100);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].need, 12.0);
    }

    #[test]
    fn clearance_short_and_width() {
        let mut b = board();
        // A and B 10 mil apart edge to edge: fine under 8 mil.
        b.geometry.tracks.push(track(0, 0.0, 10.0));
        b.geometry.tracks.push(track(1, 20.0, 10.0));
        assert!(check(&b, 100).is_empty());
        // The clock 15 mil from B breaks its 20 mil class rule.
        b.geometry.tracks.push(track(2, 40.0, 10.0));
        let v = check(&b, 100);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].kind, Kind::Clearance);
        assert!((v[0].found - 10.0).abs() < 1e-6 && v[0].need == 20.0);
        // Crossing tracks short.
        b.geometry.tracks.push(Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: p(500.0, -100.0),
            end: p(500.0, 100.0),
            width: Length::from_mil(4.0),
            text: false,
        });
        let v = check(&b, 100);
        assert_eq!(
            v.iter().filter(|v| v.kind == Kind::Short).count(),
            2,
            "{v:?}"
        );
        assert_eq!(v.iter().filter(|v| v.kind == Kind::Width).count(), 1);
    }
}
