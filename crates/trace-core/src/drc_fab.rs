//! Manufacturing rules, read as Altium has them: hole size, hole to hole
//! spacing, annular ring, solder mask slivers, silkscreen over mask
//! openings and copper near the board edge. Each runs only where the
//! board has the rule.

use super::{
    center, judge, obj_of, param, shape_gap, Copper, Item, Kind, NetInfo, Obj, Tri, Violation, CELL,
};
use crate::route::{inside, land, pt, seg_seg, Shape, P};
use crate::{Board, LayerRef, Length, Rule};
use std::collections::{HashMap, HashSet};

/// Altium layer numbers of the solder mask layers, which importers keep as
/// `LayerRef::Other` for shapes drawn straight on them.
const TOP_MASK: u8 = 37;
const BOTTOM_MASK: u8 = 38;

/// The board-wide solder mask rule: how far openings grow past the copper
/// and whether vias are covered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskRule {
    pub expansion: Length,
    pub tent_top: bool,
    pub tent_bottom: bool,
}

/// The board's `SolderMaskExpansion` rule for everything (scope `All`), or
/// 2 mil with tented vias where it has none.
pub fn mask_rule(board: &Board) -> MaskRule {
    let mut m = MaskRule {
        expansion: Length::from_mil(2.0),
        tent_top: true,
        tent_bottom: true,
    };
    let rule = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("SolderMaskExpansion"))
        .filter(|r| {
            r.scope
                .first()
                .is_none_or(|s| s.trim().eq_ignore_ascii_case("All"))
        })
        .min_by_key(|r| r.priority);
    if let Some(r) = rule {
        if let Some(e) = param(r, "EXPANSION").and_then(Length::parse) {
            m.expansion = e;
        }
        let yes = |k: &str| param(r, k).map(|v| v.eq_ignore_ascii_case("TRUE"));
        if let Some(t) = yes("ISTENTINGTOP") {
            m.tent_top = t;
        }
        if let Some(t) = yes("ISTENTINGBOTTOM") {
            m.tent_bottom = t;
        }
    }
    m
}

/// An object as rule scopes see it: net, kind, part.
type Who = (Option<u32>, Obj, Option<u32>);

/// The enabled rules of one kind, by priority, and their scopes judged
/// once per object.
struct Rules<'a> {
    list: Vec<&'a Rule>,
    info: &'a NetInfo,
    cache: HashMap<(usize, usize, Who), Tri>,
}

impl<'a> Rules<'a> {
    fn new(board: &'a Board, info: &'a NetInfo, kind: &str) -> Self {
        let mut list: Vec<&Rule> = board
            .rules
            .iter()
            .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case(kind))
            .collect();
        list.sort_by_key(|r| r.priority);
        Rules {
            list,
            info,
            cache: HashMap::new(),
        }
    }

    fn one(&mut self, r: usize, k: usize, w: Who) -> Tri {
        let rule = self.list[r];
        let info = self.info;
        *self.cache.entry((r, k, w)).or_insert_with(|| {
            let scope = rule.scope.get(k).map(String::as_str).unwrap_or("All");
            judge(scope, &info.subject_in(w.0, w.1, w.2))
        })
    }

    fn pair(&mut self, r: usize, a: Who, b: Who) -> Tri {
        let x = and(self.one(r, 0, a), self.one(r, 1, b));
        let y = and(self.one(r, 0, b), self.one(r, 1, a));
        match (x, y) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        }
    }

    /// The rules that may decide, by priority: each one that might take
    /// the object (its scope asks what Trace cannot judge), up to and
    /// including the first that does.
    fn deciding(&mut self, mut takes: impl FnMut(&mut Self, usize) -> Tri) -> Vec<usize> {
        let mut out = Vec::new();
        for r in 0..self.list.len() {
            match takes(self, r) {
                Some(false) => {}
                None => out.push(r),
                Some(true) => {
                    out.push(r);
                    break;
                }
            }
        }
        out
    }

    fn mil(&self, r: usize, k: &str) -> Option<f64> {
        param(self.list[r], k)
            .and_then(Length::parse)
            .map(Length::mil)
    }
}

fn and(x: Tri, y: Tri) -> Tri {
    match (x, y) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// Judged against every rule that may decide: a violation only when it
/// fails all of them, reported against the last (the one that applies).
fn fails(rules: &[usize], mut need: impl FnMut(usize) -> Option<f64>) -> Option<(usize, f64)> {
    let mut last = None;
    for &r in rules {
        last = Some((r, need(r)?));
    }
    last
}

fn bbox_of(s: &Shape) -> [f64; 4] {
    s.bbox()
}

fn cells(b: [f64; 4], reach: f64) -> impl Iterator<Item = (i32, i32)> {
    let (x0, x1) = (
        ((b[0] - reach) / CELL).floor() as i32,
        ((b[2] + reach) / CELL).floor() as i32,
    );
    let (y0, y1) = (
        ((b[1] - reach) / CELL).floor() as i32,
        ((b[3] + reach) / CELL).floor() as i32,
    );
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

#[allow(clippy::too_many_arguments)]
fn viol(
    kind: Kind,
    layer: Option<u8>,
    at: P,
    a: (Option<u32>, Item, Obj),
    b: Option<(Option<u32>, Item, Obj)>,
    found: f64,
    need: f64,
    rule: &str,
) -> Violation {
    Violation {
        kind,
        layer,
        at,
        nets: [a.0, b.and_then(|b| b.0)],
        items: [Some(a.1), b.map(|b| b.1)],
        objects: [Some(a.2), b.map(|b| b.2)],
        found,
        need,
        rule: rule.to_string(),
    }
}

/// A drilled hole: its centre line and radius, and the layers it runs
/// between.
struct Hole {
    a: P,
    b: P,
    r: f64,
    span: (u8, u8),
    who: Who,
    item: Item,
}

fn holes(board: &Board, n: u8) -> Vec<Hole> {
    let g = &board.geometry;
    let mut out = Vec::new();
    for (i, p) in g.pads.iter().enumerate() {
        if p.hole.0 <= 0 {
            continue;
        }
        let c = pt(p.at);
        let r = p.hole.mil() / 2.0;
        let half = (p.fab.slot.mil() / 2.0 - r).max(0.0);
        let (s, k) = p.fab.slot_deg.to_radians().sin_cos();
        out.push(Hole {
            a: [c[0] - half * k, c[1] - half * s],
            b: [c[0] + half * k, c[1] + half * s],
            r,
            span: (0, n - 1),
            who: (p.net, Obj::ThPad, p.component),
            item: Item::Pad(i),
        });
    }
    for (i, v) in g.vias.iter().enumerate() {
        let c = pt(v.at);
        out.push(Hole {
            a: c,
            b: c,
            r: v.hole.mil() / 2.0,
            span: (
                v.from_layer.min(v.to_layer),
                v.from_layer.max(v.to_layer).min(n - 1),
            ),
            who: (v.net, Obj::Via, None),
            item: Item::Via(i),
        });
    }
    out
}

fn mid(a: P, b: P) -> P {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// Hole size and hole to hole spacing.
fn drills(board: &Board, info: &NetInfo, holes: &[Hole], out: &mut Vec<Violation>) {
    let mut size = Rules::new(board, info, "HoleSize");
    if !size.list.is_empty() {
        for h in holes {
            let rules = size.deciding(|s, r| s.one(r, 0, h.who));
            let d = h.r * 2.0;
            let hit = fails(&rules, |r| {
                // Sizes given as a share of the pad are not judged.
                if param(size.list[r], "ABSOLUTEVALUES")
                    .is_some_and(|v| v.eq_ignore_ascii_case("FALSE"))
                {
                    return None;
                }
                let min = size.mil(r, "MINLIMIT").unwrap_or(0.0);
                let max = size.mil(r, "MAXLIMIT").unwrap_or(f64::MAX);
                if d < min - 0.01 {
                    Some(min)
                } else if d > max + 0.01 {
                    Some(max)
                } else {
                    None
                }
            });
            if let Some((r, need)) = hit {
                out.push(viol(
                    Kind::HoleSize,
                    None,
                    mid(h.a, h.b),
                    (h.who.0, h.item, h.who.1),
                    None,
                    d,
                    need,
                    &size.list[r].name,
                ));
            }
        }
    }

    let mut gap = Rules::new(board, info, "HoleToHoleClearance");
    if gap.list.is_empty() {
        return;
    }
    let reach = (0..gap.list.len())
        .filter_map(|r| gap.mil(r, "GAP"))
        .fold(0.0, f64::max)
        + 0.01;
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, h) in holes.iter().enumerate() {
        let b = bbox_of(&Shape::Capsule {
            a: h.a,
            b: h.b,
            r: h.r,
        });
        for c in cells(b, 0.0) {
            grid.entry(c).or_default().push(i);
        }
    }
    let mut seen = HashSet::new();
    for (i, h) in holes.iter().enumerate() {
        let b = bbox_of(&Shape::Capsule {
            a: h.a,
            b: h.b,
            r: h.r,
        });
        for c in cells(b, reach) {
            let Some(list) = grid.get(&c) else { continue };
            for &j in list {
                if j <= i || !seen.insert((i, j)) {
                    continue;
                }
                let k = &holes[j];
                // Drills that share no dielectric (stacked microvias, blind
                // vias from either side) cannot run into each other.
                if h.span.0.max(k.span.0) >= h.span.1.min(k.span.1) && h.span != k.span {
                    continue;
                }
                let d = seg_seg(h.a, h.b, k.a, k.b) - h.r - k.r;
                if d >= reach {
                    continue;
                }
                let rules = gap.deciding(|s, r| s.pair(r, h.who, k.who));
                let hit = fails(&rules, |r| {
                    let need = gap.mil(r, "GAP").unwrap_or(0.0);
                    (d < need - 0.01).then_some(need)
                });
                if let Some((r, need)) = hit {
                    out.push(viol(
                        Kind::HoleToHole,
                        None,
                        mid(mid(h.a, h.b), mid(k.a, k.b)),
                        (h.who.0, h.item, h.who.1),
                        Some((k.who.0, k.item, k.who.1)),
                        d.max(0.0),
                        need,
                        &gap.list[r].name,
                    ));
                }
            }
        }
    }
}

/// The narrowest copper ring around each plated hole.
fn rings(board: &Board, info: &NetInfo, n: u8, out: &mut Vec<Violation>) {
    let mut rules = Rules::new(board, info, "MinimumAnnularRing");
    if rules.list.is_empty() {
        return;
    }
    let g = &board.geometry;
    // (ring, layer) of the narrowest land.
    let mut check = |who: Who, item: Item, at: P, worst: Option<(f64, u8)>| {
        let Some((ring, layer)) = worst else { return };
        let list = rules.deciding(|s, r| s.one(r, 0, who));
        let hit = fails(&list, |r| {
            let need = rules.mil(r, "MINIMUMRING").unwrap_or(0.0);
            (ring < need - 0.01).then_some(need)
        });
        if let Some((r, need)) = hit {
            out.push(viol(
                Kind::AnnularRing,
                Some(layer),
                at,
                (who.0, item, who.1),
                None,
                ring,
                need,
                &rules.list[r].name,
            ));
        }
    };
    for (i, v) in g.vias.iter().enumerate() {
        let (lo, hi) = (
            v.from_layer.min(v.to_layer),
            v.from_layer.max(v.to_layer).min(n - 1),
        );
        let hole = v.hole.mil();
        let mut worst: Option<(f64, u8)> = None;
        for l in lo..=hi {
            let d = match v.rings.iter().find(|(rl, _)| *rl == l) {
                // A ring the size of the hole: no pad on this layer.
                Some((_, d)) if d.mil() <= hole => continue,
                Some((_, d)) => d.mil(),
                None => v.diameter.mil(),
            };
            let ring = (d - hole) / 2.0;
            if worst.is_none_or(|w| ring < w.0) {
                worst = Some((ring, l));
            }
        }
        check((v.net, Obj::Via, None), Item::Via(i), pt(v.at), worst);
    }
    for (i, p) in g.pads.iter().enumerate() {
        if p.hole.0 <= 0 || p.fab.unplated || !p.outline.is_empty() {
            continue;
        }
        let hole = p.hole.mil();
        let slot = p.fab.slot.mil().max(hole);
        let ring = |sx: f64, sy: f64| ((sx.min(sy) - hole) / 2.0).min((sx.max(sy) - slot) / 2.0);
        let mut worst: Option<(f64, u8)> = None;
        let mut take = |r: f64, l: u8| {
            if worst.is_none_or(|w| r < w.0) {
                worst = Some((r, l));
            }
        };
        if p.stack.is_empty() {
            let l = match p.layer {
                LayerRef::Copper(l) => l,
                _ => 0,
            };
            take(ring(p.size_x.mil(), p.size_y.mil()), l);
        } else {
            for s in p.stack.iter().filter(|s| s.outline.is_empty()) {
                take(ring(s.size_x.mil(), s.size_y.mil()), s.layer);
            }
        }
        check(
            (p.net, Obj::ThPad, p.component),
            Item::Pad(i),
            pt(p.at),
            worst,
        );
    }
}

/// A solder mask opening: the copper it is for (or its own shape) and how
/// far it grows past it.
struct Opening {
    side: u8,
    shape: Shape,
    grow: f64,
    who: Who,
    item: Item,
}

fn openings(board: &Board, items: &[Copper], n: u8) -> Vec<Opening> {
    let rule = mask_rule(board);
    let g = &board.geometry;
    let bottom = n - 1;
    let mut out = Vec::new();
    for c in items {
        let Some(l) = c.layer else { continue };
        if l != 0 && l != bottom {
            continue;
        }
        let top = l == 0;
        let (fab, comp) = match c.item {
            Item::Pad(i) if matches!(c.obj, Obj::SmdPad | Obj::ThPad) => {
                (g.pads[i].fab, g.pads[i].component)
            }
            Item::Via(i) if c.obj == Obj::Via => {
                let v = &g.vias[i];
                let tent = if top {
                    v.fab.tent_top || rule.tent_top
                } else {
                    v.fab.tent_bottom || rule.tent_bottom
                };
                if tent {
                    continue;
                }
                (v.fab, None)
            }
            _ => continue,
        };
        if if top { fab.tent_top } else { fab.tent_bottom } {
            continue;
        }
        out.push(Opening {
            side: l,
            shape: c.shape.clone(),
            grow: fab.mask.unwrap_or(rule.expansion).mil(),
            who: (c.net, c.obj, comp),
            item: c.item,
        });
    }
    // Openings drawn on the mask layers themselves.
    let side_of = |l: &LayerRef| match l {
        LayerRef::Other(TOP_MASK) => Some(0),
        LayerRef::Other(BOTTOM_MASK) => Some(bottom),
        _ => None,
    };
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(side) = side_of(&p.layer) {
            out.push(Opening {
                side,
                shape: land(
                    pt(p.at),
                    p.shape,
                    p.size_x.mil(),
                    p.size_y.mil(),
                    p.rotation_deg,
                    &p.outline,
                ),
                grow: 0.0,
                who: (p.net, Obj::SmdPad, p.component),
                item: Item::Pad(i),
            });
        }
    }
    for (i, a) in g.areas.iter().enumerate() {
        if let Some(side) = side_of(&a.layer) {
            if a.outline.len() >= 3 {
                out.push(Opening {
                    side,
                    shape: Shape::Poly(a.outline.iter().map(|&q| pt(q)).collect()),
                    grow: 0.0,
                    who: (a.net, Obj::Region, a.component),
                    item: Item::Area(i),
                });
            }
        }
    }
    out
}

fn grid_of(open: &[Opening], reach: f64) -> HashMap<(u8, i32, i32), Vec<usize>> {
    let mut grid: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
    for (i, o) in open.iter().enumerate() {
        for (x, y) in cells(o.shape.bbox(), o.grow.max(0.0) + reach) {
            grid.entry((o.side, x, y)).or_default().push(i);
        }
    }
    grid
}

/// Strips of mask between openings too thin to stay on.
fn slivers(board: &Board, info: &NetInfo, open: &[Opening], out: &mut Vec<Violation>) {
    let mut rules = Rules::new(board, info, "MinimumSolderMaskSliver");
    if rules.list.is_empty() || open.is_empty() {
        return;
    }
    let reach = (0..rules.list.len())
        .filter_map(|r| rules.mil(r, "MINSOLDERMASKWIDTH"))
        .fold(0.0, f64::max)
        + 0.01;
    if reach <= 0.02 {
        return;
    }
    let grid = grid_of(open, 0.0);
    let mut seen = HashSet::new();
    for (i, a) in open.iter().enumerate() {
        for (x, y) in cells(a.shape.bbox(), a.grow.max(0.0) + reach) {
            let Some(list) = grid.get(&(a.side, x, y)) else {
                continue;
            };
            for &j in list {
                if j <= i || !seen.insert((i, j)) {
                    continue;
                }
                let b = &open[j];
                if a.item == b.item {
                    continue;
                }
                let gap = shape_gap(&a.shape, &b.shape);
                // Openings that touch or overlap leave no mask between.
                if gap <= 0.0 {
                    continue;
                }
                let sliver = gap - a.grow - b.grow;
                if sliver <= 0.01 || sliver >= reach {
                    continue;
                }
                let list = rules.deciding(|s, r| s.pair(r, a.who, b.who));
                let hit = fails(&list, |r| {
                    let need = rules.mil(r, "MINSOLDERMASKWIDTH").unwrap_or(0.0);
                    (sliver < need - 0.01).then_some(need)
                });
                if let Some((r, need)) = hit {
                    out.push(viol(
                        Kind::MaskSliver,
                        Some(a.side),
                        mid(center(&a.shape), center(&b.shape)),
                        (a.who.0, a.item, a.who.1),
                        Some((b.who.0, b.item, b.who.1)),
                        sliver,
                        need,
                        &rules.list[r].name,
                    ));
                }
            }
        }
    }
}

/// Silkscreen on or near mask openings, which the board house clips away.
fn silk(board: &Board, info: &NetInfo, open: &[Opening], n: u8, out: &mut Vec<Violation>) {
    let mut rules = Rules::new(board, info, "SilkToSolderMaskClearance");
    if rules.list.is_empty() || open.is_empty() {
        return;
    }
    let reach = (0..rules.list.len())
        .filter_map(|r| rules.mil(r, "MINSILKSCREENTOMASKGAP"))
        .fold(0.0, f64::max)
        + 0.01;
    let grid = grid_of(open, 0.0);
    let g = &board.geometry;
    // Silk strokes: (side, a, b, half width, item, part).
    let mut strokes: Vec<(u8, P, P, f64, Item, Option<u32>)> = Vec::new();
    let side = |l: &LayerRef| match l {
        LayerRef::TopSilk => Some(0),
        LayerRef::BottomSilk => Some(n - 1),
        _ => None,
    };
    for (i, t) in g.tracks.iter().enumerate() {
        if let Some(s) = side(&t.layer) {
            strokes.push((
                s,
                pt(t.start),
                pt(t.end),
                t.width.mil() / 2.0,
                Item::Track(i),
                t.component,
            ));
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        let Some(s) = side(&a.layer) else { continue };
        let (c, r) = (pt(a.center), a.radius.mil());
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let k = (sweep / 10.0).ceil().max(1.0) as usize;
        let at = |j: usize| {
            let t = (a.start_deg + sweep * j as f64 / k as f64).to_radians();
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        };
        for j in 0..k {
            strokes.push((
                s,
                at(j),
                at(j + 1),
                a.width.mil() / 2.0,
                Item::Arc(i),
                a.component,
            ));
        }
    }
    // One report per piece of silk (a part's silk counts as one) and
    // opening, the worst kept.
    let mut worst: HashMap<(Option<u32>, Item, Item), usize> = HashMap::new();
    for &(s, a, b, w, item, comp) in &strokes {
        let bb = [
            a[0].min(b[0]) - w,
            a[1].min(b[1]) - w,
            a[0].max(b[0]) + w,
            a[1].max(b[1]) + w,
        ];
        let mut seen = HashSet::new();
        for (x, y) in cells(bb, reach) {
            let Some(list) = grid.get(&(s, x, y)) else {
                continue;
            };
            for &j in list {
                if !seen.insert(j) {
                    continue;
                }
                let o = &open[j];
                let edge = o.shape.gap(a, b, w);
                if edge - o.grow.max(0.0) >= reach {
                    continue;
                }
                let silk_who = (None, Obj::Track, comp);
                let list = rules.deciding(|s, r| s.pair(r, o.who, silk_who));
                let hit = fails(&list, |r| {
                    let need = rules.mil(r, "MINSILKSCREENTOMASKGAP").unwrap_or(0.0);
                    // To the copper the opening shows, or to the opening.
                    let to_copper = param(rules.list[r], "CLEARANCETOEXPOSEDCOPPER")
                        .is_some_and(|v| v.eq_ignore_ascii_case("TRUE"));
                    let gap = if to_copper { edge } else { edge - o.grow };
                    (gap < need - 0.01).then_some(need)
                });
                let Some((r, need)) = hit else { continue };
                let to_copper = param(rules.list[r], "CLEARANCETOEXPOSEDCOPPER")
                    .is_some_and(|v| v.eq_ignore_ascii_case("TRUE"));
                let gap = if to_copper { edge } else { edge - o.grow };
                let key = (
                    comp,
                    if comp.is_some() { Item::Track(0) } else { item },
                    o.item,
                );
                let v = viol(
                    Kind::SilkToMask,
                    Some(s),
                    center(&o.shape),
                    (o.who.0, o.item, o.who.1),
                    Some((None, item, Obj::Track)),
                    gap.max(0.0),
                    need,
                    &rules.list[r].name,
                );
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

/// Copper too near the board outline, or outside it.
fn edge(board: &Board, info: &NetInfo, items: &[Copper], out: &mut Vec<Violation>) {
    let mut rules = Rules::new(board, info, "BoardOutlineClearance");
    let outline: Vec<P> = board.geometry.outline.iter().map(|&q| pt(q)).collect();
    if rules.list.is_empty() || outline.len() < 3 {
        return;
    }
    // Each rule's gap, and its gaps for single kinds of object.
    let gaps: Vec<(f64, HashMap<Obj, f64>)> = rules
        .list
        .iter()
        .map(|r| {
            let gap = param(r, "GAP")
                .and_then(Length::parse)
                .map_or(0.0, Length::mil);
            let mut per = HashMap::new();
            for item in param(r, "OBJECTCLEARANCES").unwrap_or("").split(';') {
                let Some((pair, v)) = item.split_once(':') else {
                    continue;
                };
                let Some((a, b)) = pair.split_once('-') else {
                    continue;
                };
                let edge = |s: &str| s.trim().ends_with("OutlineEdge");
                let obj = if edge(b) {
                    obj_of(a)
                } else if edge(a) {
                    obj_of(b)
                } else {
                    None
                };
                if let (Some(o), Ok(v)) = (obj, v.trim().parse::<f64>()) {
                    per.insert(o, v / 10_000.0);
                }
            }
            (gap, per)
        })
        .collect();
    let reach = gaps
        .iter()
        .flat_map(|(g, per)| std::iter::once(*g).chain(per.values().copied()))
        .fold(0.0, f64::max)
        + 0.01;
    let segs: Vec<(P, P)> = (0..outline.len())
        .map(|i| (outline[i], outline[(i + 1) % outline.len()]))
        .collect();
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        let bb = [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ];
        for c in cells(bb, 0.0) {
            grid.entry(c).or_default().push(i);
        }
    }
    // One report per board item and layer, the worst kept.
    let mut worst: HashMap<(Item, Option<u8>), usize> = HashMap::new();
    for c in items {
        if c.obj == Obj::Hole {
            continue;
        }
        let mut d = f64::MAX;
        let mut seen = HashSet::new();
        for cell in cells(c.shape.bbox(), reach) {
            let Some(list) = grid.get(&cell) else {
                continue;
            };
            for &s in list {
                if seen.insert(s) {
                    d = d.min(c.shape.gap(segs[s].0, segs[s].1, 0.0));
                }
            }
        }
        let within = inside(center(&c.shape), &outline);
        if d >= reach && (within || c.net.is_none()) {
            // Clear of the edge; or copper without a net drawn wholly off
            // the board, such as layer names beside it.
            continue;
        }
        if !within {
            d = d.min(0.0);
        }
        let who = (c.net, c.obj, c.component);
        let list = rules.deciding(|s, r| s.one(r, 0, who));
        let hit = fails(&list, |r| {
            let need = gaps[r].1.get(&c.obj).copied().unwrap_or(gaps[r].0);
            (d < need - 0.01).then_some(need)
        });
        let Some((r, need)) = hit else { continue };
        let v = viol(
            Kind::BoardEdge,
            c.layer,
            center(&c.shape),
            (c.net, c.item, c.obj),
            None,
            d.max(0.0),
            need,
            &rules.list[r].name,
        );
        let key = (c.item, c.layer);
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

/// Runs every manufacturing rule the board has.
pub(super) fn check(board: &Board, info: &NetInfo, items: &[Copper], out: &mut Vec<Violation>) {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let holes = holes(board, n);
    drills(board, info, &holes, out);
    rings(board, info, n, out);
    let open = openings(board, items, n);
    slivers(board, info, &open, out);
    silk(board, info, &open, n, out);
    edge(board, info, items, out);
}

/// What a board item is, for messages: "pad U3-A1", "via", "track".
fn what(board: &Board, item: Option<Item>, obj: Option<Obj>) -> String {
    let g = &board.geometry;
    let part = |c: Option<u32>| {
        c.and_then(|c| board.components.get(c as usize))
            .map(|c| c.designator.clone())
    };
    match item {
        Some(Item::Pad(i)) => {
            let p = &g.pads[i];
            match part(p.component) {
                Some(d) => format!("pad {d}-{}", p.name),
                None if p.name.is_empty() => "pad".into(),
                None => format!("pad {}", p.name),
            }
        }
        Some(Item::Via(_)) => "via".into(),
        Some(Item::Track(i)) => {
            let t = &g.tracks[i];
            match (&t.layer, part(t.component)) {
                (LayerRef::TopSilk | LayerRef::BottomSilk, Some(d)) => format!("silkscreen of {d}"),
                (LayerRef::TopSilk | LayerRef::BottomSilk, None) => "silkscreen".into(),
                _ => "track".into(),
            }
        }
        Some(Item::Arc(i)) => {
            let a = &g.arcs[i];
            match (&a.layer, part(a.component)) {
                (LayerRef::TopSilk | LayerRef::BottomSilk, Some(d)) => format!("silkscreen of {d}"),
                (LayerRef::TopSilk | LayerRef::BottomSilk, None) => "silkscreen".into(),
                _ => "arc".into(),
            }
        }
        Some(Item::Polygon(_)) => obj.map(Obj::label).unwrap_or("copper area").into(),
        Some(Item::Area(_)) => "mask opening".into(),
        None => String::new(),
    }
}

pub(super) fn describe(board: &Board, v: &Violation) -> String {
    let who = |k: usize| {
        let w = what(board, v.items[k], v.objects[k]);
        match v.nets[k].and_then(|n| board.nets.get(n as usize)) {
            Some(n) => format!("{w} ({})", n.name),
            None => w,
        }
    };
    match v.kind {
        Kind::BoardEdge if v.found <= 0.0 => format!(
            "{} reaches the board edge, needs {:.2} mil ({})",
            who(0),
            v.need,
            v.rule
        ),
        Kind::BoardEdge => format!(
            "{} is {:.2} mil from the board edge, needs {:.2} ({})",
            who(0),
            v.found,
            v.need,
            v.rule
        ),
        Kind::HoleToHole if v.found <= 0.0 => format!(
            "Holes of {} and {} overlap, need {:.2} mil apart ({})",
            who(0),
            who(1),
            v.need,
            v.rule
        ),
        Kind::HoleToHole => format!(
            "Holes of {} and {} are {:.2} mil apart, need {:.2} ({})",
            who(0),
            who(1),
            v.found,
            v.need,
            v.rule
        ),
        Kind::AnnularRing => format!(
            "{}: annular ring {:.2} mil, needs {:.2} ({})",
            who(0),
            v.found,
            v.need,
            v.rule
        ),
        Kind::HoleSize => format!(
            "{}: hole {:.2} mil, rule allows {} {:.2} ({})",
            who(0),
            v.found,
            if v.found < v.need {
                "at least"
            } else {
                "at most"
            },
            v.need,
            v.rule
        ),
        Kind::MaskSliver => format!(
            "Solder mask between {} and {} is {:.2} mil wide, needs {:.2} ({})",
            who(0),
            who(1),
            v.found,
            v.need,
            v.rule
        ),
        Kind::SilkToMask => {
            let silk = what(board, v.items[1], None);
            let mut c = silk.chars();
            let silk: String = c
                .next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default();
            let opening = match v.items[0] {
                Some(Item::Area(_)) => "a solder mask opening".to_string(),
                _ => who(0),
            };
            if v.found <= 0.0 {
                format!("{silk} overlaps {opening} ({})", v.rule)
            } else {
                format!(
                    "{silk} is {:.2} mil from {opening}, needs {:.2} ({})",
                    v.found, v.need, v.rule
                )
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{check as drc, glob};
    use super::*;
    use crate::*;
    use std::collections::BTreeMap;

    fn p(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
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

    fn via(x: f64, y: f64, d: f64, hole: f64) -> Via {
        Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: p(x, y),
            diameter: Length::from_mil(d),
            hole: Length::from_mil(hole),
            rings: Vec::new(),
            fab: Default::default(),
        }
    }

    fn pad(x: f64, y: f64, size: f64, component: Option<u32>) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component,
            name: "1".into(),
            at: p(x, y),
            size_x: Length::from_mil(size),
            size_y: Length::from_mil(size),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    fn board() -> Board {
        let mut b = Board::default();
        for name in ["Top", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: name.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                material: None,
            });
        }
        b.nets.push(Net {
            name: "GND".into(),
            pieces: 1,
            ..Default::default()
        });
        b.components.push(Component {
            designator: "T4".into(),
            footprint: "BALUN-0805".into(),
            library_ref: None,
            x: Length(0),
            y: Length(0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        b.geometry.outline = vec![
            p(0.0, 0.0),
            p(2000.0, 0.0),
            p(2000.0, 1000.0),
            p(0.0, 1000.0),
        ];
        b
    }

    fn kinds(b: &Board, k: Kind) -> Vec<Violation> {
        drc(b, 1000).into_iter().filter(|v| v.kind == k).collect()
    }

    #[test]
    fn holes_rings_and_sizes() {
        let mut b = board();
        b.rules.push(rule(
            "HoleToHoleClearance",
            ["All", "All"],
            1,
            &[("GAP", "8mil")],
        ));
        b.rules.push(rule(
            "MinimumAnnularRing",
            ["All", "All"],
            1,
            &[("MINIMUMRING", "4mil")],
        ));
        b.rules.push(rule(
            "HoleSize",
            ["All", "All"],
            1,
            &[
                ("MINLIMIT", "8mil"),
                ("MAXLIMIT", "100mil"),
                ("ABSOLUTEVALUES", "TRUE"),
            ],
        ));
        // Holes 10 mil wide, 15 mil apart centre to centre: 5 mil between.
        b.geometry.vias.push(via(500.0, 500.0, 20.0, 10.0));
        b.geometry.vias.push(via(515.0, 500.0, 20.0, 10.0));
        // Far off, a ring of 2 mil and a 6 mil hole.
        b.geometry.vias.push(via(900.0, 500.0, 14.0, 10.0));
        b.geometry.vias.push(via(1200.0, 500.0, 16.0, 6.0));
        let v = kinds(&b, Kind::HoleToHole);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!((v[0].found - 5.0).abs() < 1e-6 && v[0].need == 8.0);
        let v = kinds(&b, Kind::AnnularRing);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!((v[0].found - 2.0).abs() < 1e-6);
        let v = kinds(&b, Kind::HoleSize);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].items[0], Some(Item::Via(3)));
        assert!(describe(&b, &v[0]).contains("at least 8.00"));
    }

    #[test]
    fn slivers_follow_part_scopes() {
        let mut b = board();
        b.rules.push(rule(
            "SolderMaskExpansion",
            ["All", "All"],
            1,
            &[("EXPANSION", "2mil")],
        ));
        b.rules.push(rule(
            "MinimumSolderMaskSliver",
            ["All", "All"],
            2,
            &[("MINSOLDERMASKWIDTH", "3mil")],
        ));
        b.rules.push(rule(
            "MinimumSolderMaskSliver",
            ["InComponent('T4')", "All"],
            1,
            &[("MINSOLDERMASKWIDTH", "0mil")],
        ));
        // Pads 20 mil square, 26 mil centre to centre: 6 mil of copper gap, 2 mil of
        // mask once both openings grow by 2.
        b.geometry.pads.push(pad(100.0, 100.0, 20.0, None));
        b.geometry.pads.push(pad(126.0, 100.0, 20.0, None));
        // The same, on the balun, whose rule allows any sliver.
        b.geometry.pads.push(pad(300.0, 100.0, 20.0, Some(0)));
        b.geometry.pads.push(pad(326.0, 100.0, 20.0, Some(0)));
        let v = kinds(&b, Kind::MaskSliver);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!((v[0].found - 2.0).abs() < 1e-6, "{v:?}");
        assert_eq!(v[0].items, [Some(Item::Pad(0)), Some(Item::Pad(1))]);
        assert!(glob("0402*", "0402_IPC") && !glob("0603*", "0402"));
    }

    #[test]
    fn silk_on_pads_and_copper_by_the_edge() {
        let mut b = board();
        b.rules.push(rule(
            "SilkToSolderMaskClearance",
            ["All", "All"],
            1,
            &[("MINSILKSCREENTOMASKGAP", "0mil")],
        ));
        b.rules.push(rule(
            "BoardOutlineClearance",
            ["OnCopper and IsPoly", "All"],
            1,
            &[
                ("GAP", "25mil"),
                (
                    "OBJECTCLEARANCES",
                    "ClearanceObj_Poly-ClearanceObj_OutlineEdge:150000",
                ),
            ],
        ));
        b.rules.push(rule(
            "BoardOutlineClearance",
            ["OnCopper", "All"],
            2,
            &[("GAP", "25mil")],
        ));
        b.geometry.pads.push(pad(500.0, 500.0, 40.0, Some(0)));
        b.geometry.tracks.push(Track {
            layer: LayerRef::TopSilk,
            net: None,
            start: p(450.0, 500.0),
            end: p(550.0, 500.0),
            width: Length::from_mil(6.0),
            component: Some(0),
        });
        let v = kinds(&b, Kind::SilkToMask);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            describe(&b, &v[0]),
            "Silkscreen of T4 overlaps pad T4-1 (GND) (SilkToSolderMaskClearance1)"
        );
        // A track 10 mil from the left edge breaks 25 mil; a pour 20 mil
        // in keeps its own 15.
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: p(15.0, 300.0),
            end: p(15.0, 700.0),
            width: Length::from_mil(10.0),
            component: None,
        });
        b.geometry.polygons.push(Polygon {
            layer: LayerRef::Copper(1),
            net: Some(0),
            outline: vec![
                p(20.0, 20.0),
                p(1980.0, 20.0),
                p(1980.0, 980.0),
                p(20.0, 980.0),
            ],
            holes: Vec::new(),
            pour: Some(0),
            component: None,
        });
        let v = kinds(&b, Kind::BoardEdge);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!((v[0].found - 10.0).abs() < 1e-6 && v[0].need == 25.0);
    }
}
