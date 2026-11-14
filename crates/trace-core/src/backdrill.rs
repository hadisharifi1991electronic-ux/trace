//! Via stubs and backdrilling.
//!
//! A through via that joins its net on inner layers only leaves the rest
//! of its plated barrel hanging off the signal: a stub, which rings at a
//! frequency set by its length and hurts fast links. Backdrilling drills
//! the barrel out from the board's surface down to just short of the
//! layer it joins.
//!
//! Each via's stub is measured from the stack-up: from the face of the
//! outermost layer it joins to the end of its barrel, less what a backdrill
//! takes off ([`LEFT`] is what the drill leaves). The longest stub allowed
//! comes from the board's back drilling rules (Altium's Max Via Stub Length,
//! or any rule whose kind names stubs or backdrilling, with its length) or,
//! without one, [`MAX_STUB`] for nets of differential pairs.

use crate::drc::{Item, Kind, Obj, Scopes, Thing, Violation};
use crate::edit::Edit;
use crate::route::{point_seg, pt, P};
use crate::{Backdrill, Board, LayerRef, Length, Rule};
use std::collections::HashMap;

/// Longest stub allowed on pair nets when the board has no rule (mil).
pub const MAX_STUB: f64 = 10.0;
/// Stub a backdrill leaves above the layer it must not cut (mil): the
/// depth tolerance fabricators work to.
pub const LEFT: f64 = 8.0;
/// A backdrill is this much (mil) larger than the hole.
pub const OVERSIZE: f64 = 8.0;
/// Board thickness taken when the stack-up has none (mil).
const DEFAULT_THICKNESS: f64 = 62.0;

/// The top and bottom face of each copper layer, as depth below the top
/// surface in mil. A stack-up without thicknesses is spread evenly over a
/// 62 mil board.
pub fn faces(board: &Board) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut depth = 0.0;
    for l in &board.stackup.layers {
        let t = l.thickness.mil().max(0.0);
        match l.kind {
            crate::LayerKind::Copper => {
                out.push((depth, depth + t));
                depth += t;
            }
            crate::LayerKind::Dielectric => depth += t,
            _ => {}
        }
    }
    if depth <= 0.0 {
        let n = out.len().max(1);
        let step = DEFAULT_THICKNESS / (n.max(2) - 1) as f64;
        return (0..n).map(|i| (i as f64 * step, i as f64 * step)).collect();
    }
    out
}

/// A via's stub, as found.
#[derive(Clone, Debug)]
pub struct Stub {
    pub via: usize,
    pub net: u32,
    /// Copper layers where its net's copper joins it, top first.
    pub joins: Vec<u8>,
    /// Barrel above the topmost and below the bottommost joined layer, in
    /// mil, before and after backdrilling.
    pub top: f64,
    pub bottom: f64,
    pub top_left: f64,
    pub bottom_left: f64,
    /// The longest stub its rule allows, and the rule.
    pub limit: Option<(f64, String)>,
    /// How its rule backdrills: how much larger than the hole (mil), and
    /// whether from the top and from the bottom.
    pub oversize: f64,
    pub sides: (bool, bool),
}

/// A stub rule: scope, longest stub, name, oversize, sides.
#[derive(Clone)]
struct StubRule {
    scope: String,
    max: f64,
    name: String,
    oversize: f64,
    sides: (bool, bool),
}

/// Where copper of each net may join a via: track and arc ends and
/// surface pads by net, pour areas by net and layer.
struct Joins<'a> {
    board: &'a Board,
    ends: HashMap<u32, Vec<(u8, P)>>,
    pads: HashMap<u32, Vec<(u8, P, f64)>>,
    polys: HashMap<(u32, u8), Vec<usize>>,
}

impl<'a> Joins<'a> {
    fn new(board: &'a Board) -> Self {
        let g = &board.geometry;
        let mut ends: HashMap<u32, Vec<(u8, P)>> = HashMap::new();
        for t in &g.tracks {
            if let (LayerRef::Copper(l), Some(n), false) = (&t.layer, t.net, t.text) {
                let e = ends.entry(n).or_default();
                e.push((*l, pt(t.start)));
                e.push((*l, pt(t.end)));
            }
        }
        for a in &g.arcs {
            if let (LayerRef::Copper(l), Some(n)) = (&a.layer, a.net) {
                let (c, r) = (pt(a.center), a.radius.mil());
                for d in [a.start_deg, a.end_deg] {
                    let t = d.to_radians();
                    ends.entry(n)
                        .or_default()
                        .push((*l, [c[0] + r * t.cos(), c[1] + r * t.sin()]));
                }
            }
        }
        let mut pads: HashMap<u32, Vec<(u8, P, f64)>> = HashMap::new();
        for p in &g.pads {
            if let (LayerRef::Copper(l), Some(n)) = (&p.layer, p.net) {
                let r = p.size_x.mil().max(p.size_y.mil()) / 2.0;
                pads.entry(n).or_default().push((*l, pt(p.at), r));
            }
        }
        let mut polys: HashMap<(u32, u8), Vec<usize>> = HashMap::new();
        for (i, p) in g.polygons.iter().enumerate() {
            if let (LayerRef::Copper(l), Some(n)) = (&p.layer, p.net) {
                polys.entry((n, *l)).or_default().push(i);
            }
        }
        Joins {
            board,
            ends,
            pads,
            polys,
        }
    }

    /// The copper layers where via `i`'s net joins it.
    fn layers(&self, i: usize) -> Vec<u8> {
        let v = &self.board.geometry.vias[i];
        let Some(net) = v.net else {
            return Vec::new();
        };
        let c = pt(v.at);
        let r = v.diameter.mil() / 2.0;
        let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
        let mut out: Vec<u8> = Vec::new();
        let near = |p: P, d: f64| (p[0] - c[0]).hypot(p[1] - c[1]) <= d;
        for &(l, p) in self.ends.get(&net).into_iter().flatten() {
            if near(p, r + 0.5) {
                out.push(l);
            }
        }
        for &(l, p, pr) in self.pads.get(&net).into_iter().flatten() {
            if near(p, pr + 0.5) {
                out.push(l);
            }
        }
        for l in lo..=hi {
            if out.contains(&l) {
                continue;
            }
            for &k in self.polys.get(&(net, l)).into_iter().flatten() {
                let p = &self.board.geometry.polygons[k];
                if touches(p, c, r) {
                    out.push(l);
                    break;
                }
            }
        }
        out.retain(|&l| l >= lo && l <= hi);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Whether copper area `p` covers or comes within reach of a via pad of
/// radius `r` at `c`: poured solid over it, or joined by relief spokes.
fn touches(p: &crate::Polygon, c: P, r: f64) -> bool {
    let ring = |v: &[crate::Point]| -> Vec<P> { v.iter().map(|&q| pt(q)).collect() };
    let outline = ring(&p.outline);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for q in &outline {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    if c[0] < x0 - r || c[0] > x1 + r || c[1] < y0 - r || c[1] > y1 + r {
        return false;
    }
    let edge_near =
        |v: &[P]| (0..v.len()).any(|k| point_seg(c, v[k], v[(k + 1) % v.len()]) <= r + 0.5);
    if edge_near(&outline) {
        return true;
    }
    if !crate::route::inside(c, &outline) {
        return false;
    }
    // Inside the outline: on copper unless in a hole, and a hole that
    // reaches the pad is the relief round it, which spokes cross.
    for h in &p.holes {
        let h = ring(h);
        if crate::route::inside(c, &h) {
            return edge_near(&h);
        }
    }
    true
}

fn param<'a>(r: &'a Rule, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| {
        r.params
            .iter()
            .find(|(pk, _)| pk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    })
}

/// The board's stub rules, by priority. Altium's BackDrilling rule gives
/// `MAXSTUBLENGTH`, `HOLEOVERSIZE`, `USETOPBACKDRILL` and
/// `USEBOTTOMBACKDRILL`.
fn rules(board: &Board) -> Vec<StubRule> {
    let mut list: Vec<&Rule> = board
        .rules
        .iter()
        .filter(|r| {
            let k = r.kind.to_ascii_lowercase();
            r.enabled && (k.contains("stub") || k.contains("backdrill"))
        })
        .collect();
    list.sort_by_key(|r| r.priority);
    list.iter()
        .filter_map(|r| {
            let v = param(
                r,
                &[
                    "MAXSTUBLENGTH",
                    "MAXSTUB",
                    "STUBLENGTH",
                    "MAXLENGTH",
                    "LENGTH",
                    "LIMIT",
                ],
            )
            .and_then(Length::parse)?;
            let side = |k: &str| param(r, &[k]).is_none_or(|v| !v.eq_ignore_ascii_case("FALSE"));
            Some(StubRule {
                scope: r.scope.first().cloned().unwrap_or_else(|| "All".into()),
                max: v.mil(),
                name: r.name.clone(),
                oversize: param(r, &["HOLEOVERSIZE"])
                    .and_then(Length::parse)
                    .map(|l| l.mil())
                    .unwrap_or(OVERSIZE),
                sides: (side("USETOPBACKDRILL"), side("USEBOTTOMBACKDRILL")),
            })
        })
        .collect()
}

/// Every via's stub, for vias with a net that its copper joins.
pub fn stubs(board: &Board) -> Vec<Stub> {
    let joins = Joins::new(board);
    let faces = faces(board);
    let rules = rules(board);
    let scopes = (!rules.is_empty()).then(|| Scopes::new(board));
    let partners = board.pair_partners();
    let default = StubRule {
        scope: String::new(),
        max: MAX_STUB,
        name: "pair nets".into(),
        oversize: OVERSIZE,
        sides: (true, true),
    };
    let mut limits: HashMap<u32, Option<StubRule>> = HashMap::new();
    let mut out = Vec::new();
    for (i, v) in board.geometry.vias.iter().enumerate() {
        let Some(net) = v.net else { continue };
        if v.hole.0 <= 0 || v.fab.unplated {
            continue;
        }
        let joined = joins.layers(i);
        let (Some(&first), Some(&last)) = (joined.first(), joined.last()) else {
            continue;
        };
        let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
        let face = |l: u8| faces.get(l as usize).copied().unwrap_or((0.0, 0.0));
        let top = face(first).0 - face(lo).0;
        let bottom = face(hi).1 - face(last).1;
        let top_left = match v.fab.back_top {
            Some(b) if b.keep <= first => (face(first).0 - face(b.keep).0 + LEFT).min(top),
            Some(_) => 0.0,
            None => top,
        };
        let bottom_left = match v.fab.back_bottom {
            Some(b) if b.keep >= last => (face(b.keep).1 - face(last).1 + LEFT).min(bottom),
            Some(_) => 0.0,
            None => bottom,
        };
        let rule = limits
            .entry(net)
            .or_insert_with(|| match &scopes {
                Some(s) => rules.iter().find_map(|r| {
                    let t = Thing {
                        net: Some(net),
                        obj: Obj::Via,
                        component: None,
                        hole: Some(v.hole.mil()),
                        layer: None,
                        bbox: None,
                    };
                    (s.judge(&r.scope, &t) != Some(false)).then(|| r.clone())
                }),
                None => partners.contains_key(&net).then(|| default.clone()),
            })
            .clone();
        out.push(Stub {
            via: i,
            net,
            joins: joined,
            top,
            bottom,
            top_left,
            bottom_left,
            limit: rule.as_ref().map(|r| (r.max, r.name.clone())),
            oversize: rule.as_ref().map_or(OVERSIZE, |r| r.oversize),
            sides: rule.as_ref().map_or((true, true), |r| r.sides),
        });
    }
    out
}

/// Backdrills for every via whose stub is longer than its rule allows, as
/// edits (each via replaced by itself with the backdrill set). Vias that
/// already have the backdrill they need are left as they are.
pub fn plan(board: &Board) -> Vec<Edit> {
    let mut out = Vec::new();
    for s in stubs(board) {
        let Some((max, _)) = s.limit else { continue };
        let v = &board.geometry.vias[s.via];
        let mut new = v.clone();
        let d = Length::from_mil(v.hole.mil() + s.oversize);
        let (first, last) = (s.joins[0], *s.joins.last().unwrap());
        if s.sides.0 && s.top_left > max && first > v.from_layer.min(v.to_layer) {
            new.fab.back_top = Some(Backdrill {
                keep: first,
                diameter: d,
            });
        }
        if s.sides.1 && s.bottom_left > max && last < v.from_layer.max(v.to_layer) {
            new.fab.back_bottom = Some(Backdrill {
                keep: last,
                diameter: d,
            });
        }
        if new.fab != v.fab {
            out.push(Edit::RemoveVia {
                index: s.via,
                via: v.clone(),
            });
            out.push(Edit::InsertVia {
                index: s.via,
                via: new,
            });
        }
    }
    out
}

/// Backdrills the vias the board's own back drilling rules ask for, as
/// Altium does when it writes drill files. Boards without such a rule are
/// left alone. Returns how many vias were backdrilled.
pub fn apply_rules(board: &mut Board) -> usize {
    if rules(board).is_empty() {
        return 0;
    }
    let edits = plan(board);
    for e in &edits {
        e.apply(board);
    }
    edits.len() / 2
}

/// Removes every backdrill, as edits.
pub fn clear(board: &Board) -> Vec<Edit> {
    let mut out = Vec::new();
    for (i, v) in board.geometry.vias.iter().enumerate() {
        if v.fab.back_top.is_some() || v.fab.back_bottom.is_some() {
            let mut new = v.clone();
            new.fab.back_top = None;
            new.fab.back_bottom = None;
            out.push(Edit::RemoveVia {
                index: i,
                via: v.clone(),
            });
            out.push(Edit::InsertVia { index: i, via: new });
        }
    }
    out
}

fn layer_name(board: &Board, l: u8) -> String {
    board
        .stackup
        .copper_layers()
        .nth(l as usize)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("copper layer {}", l + 1))
}

/// Design rule check: stubs longer than their rule allows, backdrills that
/// cut a layer their via joins, and copper of other nets too close to a
/// backdrill on the layers it drills through.
pub(crate) fn check(board: &Board, gap: f64, out: &mut Vec<Violation>) {
    let g = &board.geometry;
    let stubs = stubs(board);
    for s in &stubs {
        let v = &g.vias[s.via];
        let at = pt(v.at);
        let viol = |kind, found, need, rule: String| Violation {
            kind,
            layer: None,
            at,
            nets: [Some(s.net), None],
            items: [Some(Item::Via(s.via)), None],
            objects: [Some(Obj::Via), None],
            found,
            need,
            rule,
        };
        if let Some((max, name)) = &s.limit {
            let (side, left) = if s.top_left >= s.bottom_left {
                ("above", s.top_left)
            } else {
                ("below", s.bottom_left)
            };
            if left > max + 1e-6 {
                let l = if side == "above" {
                    s.joins[0]
                } else {
                    *s.joins.last().unwrap()
                };
                out.push(viol(
                    Kind::ViaStub,
                    left,
                    *max,
                    format!("{side} {} ({name})", layer_name(board, l)),
                ));
            }
        }
        let (first, last) = (s.joins[0], *s.joins.last().unwrap());
        if let Some(b) = v.fab.back_top.filter(|b| b.keep > first) {
            out.push(viol(
                Kind::Backdrill,
                0.0,
                0.0,
                format!(
                    "backdrill from the top to {} cuts its connection on {}",
                    layer_name(board, b.keep),
                    layer_name(board, first)
                ),
            ));
        }
        if let Some(b) = v.fab.back_bottom.filter(|b| b.keep < last) {
            out.push(viol(
                Kind::Backdrill,
                0.0,
                0.0,
                format!(
                    "backdrill from the bottom to {} cuts its connection on {}",
                    layer_name(board, b.keep),
                    layer_name(board, last)
                ),
            ));
        }
    }
    // Other nets' copper round each backdrill.
    for (i, v) in g.vias.iter().enumerate() {
        if v.fab.back_top.is_none() && v.fab.back_bottom.is_none() {
            continue;
        }
        let c = pt(v.at);
        let mut worst: HashMap<(Item, u8), (f64, f64, Option<u32>)> = HashMap::new();
        let mut note = |item: Item, l: u8, d: f64, need: f64, net: Option<u32>| {
            if d < need - 1e-6 {
                let e = worst.entry((item, l)).or_insert((d, need, net));
                if d < e.0 {
                    *e = (d, need, net);
                }
            }
        };
        let reach = |l: u8| v.drilled(l).map(|d| d.mil() / 2.0 + gap);
        for (k, t) in g.tracks.iter().enumerate() {
            let LayerRef::Copper(l) = t.layer else {
                continue;
            };
            let (Some(need), true) = (reach(l), t.net != v.net && !t.text) else {
                continue;
            };
            let d = point_seg(c, pt(t.start), pt(t.end)) - t.width.mil() / 2.0;
            note(Item::Track(k), l, d, need, t.net);
        }
        for (k, p) in g.pads.iter().enumerate() {
            if p.net == v.net && p.net.is_some() {
                continue;
            }
            let layers: Vec<u8> = match p.layer {
                LayerRef::Copper(l) => vec![l],
                LayerRef::AllCopper => (0..64).collect(),
                _ => continue,
            };
            for l in layers {
                if let Some(need) = reach(l) {
                    let r = p.size_x.mil().min(p.size_y.mil()) / 2.0;
                    let d = (pt(p.at)[0] - c[0]).hypot(pt(p.at)[1] - c[1]) - r;
                    note(Item::Pad(k), l, d, need, p.net);
                }
            }
        }
        for (k, p) in g.polygons.iter().enumerate() {
            let LayerRef::Copper(l) = p.layer else {
                continue;
            };
            let Some(need) = reach(l) else { continue };
            // Copper of the via's own net would be cut too.
            let rings = std::iter::once(&p.outline).chain(p.holes.iter());
            let mut d = f64::MAX;
            for r in rings {
                let r: Vec<P> = r.iter().map(|&q| pt(q)).collect();
                for e in 0..r.len() {
                    d = d.min(point_seg(c, r[e], r[(e + 1) % r.len()]));
                }
            }
            let ring: Vec<P> = p.outline.iter().map(|&q| pt(q)).collect();
            let in_hole = p
                .holes
                .iter()
                .any(|h| crate::route::inside(c, &h.iter().map(|&q| pt(q)).collect::<Vec<_>>()));
            if crate::route::inside(c, &ring) && !in_hole {
                d = 0.0;
            }
            note(Item::Polygon(k), l, d, need, p.net);
        }
        let mut list: Vec<_> = worst.into_iter().collect();
        list.sort_by_key(|((item, l), _)| (*item, *l));
        for ((item, l), (d, need, net)) in list {
            // Measured from the drill's edge.
            let r = need - gap;
            let (d, need) = ((d - r).max(0.0), gap);
            out.push(Violation {
                kind: Kind::Backdrill,
                layer: Some(l),
                at: c,
                nets: [v.net, net],
                items: [Some(Item::Via(i)), Some(item)],
                objects: [Some(Obj::Via), None],
                found: d,
                need,
                rule: format!(
                    "copper {:.2} mil from the backdrill on {}, needs {:.2}",
                    d,
                    layer_name(board, l),
                    need
                ),
            });
        }
    }
}

pub(crate) fn describe(board: &Board, v: &Violation) -> String {
    let net = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "no net".into())
    };
    match v.kind {
        Kind::ViaStub => format!(
            "{} via stub {:.1} mil {}, allowed {:.1}",
            net(v.nets[0]),
            v.found,
            v.rule,
            v.need
        ),
        Kind::Backdrill if v.nets[1].is_some() || v.items[1].is_some() => {
            format!("{} via: {} ({})", net(v.nets[0]), v.rule, net(v.nets[1]))
        }
        _ => format!("{} via: {}", net(v.nets[0]), v.rule),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn p(x: f64, y: f64) -> Point {
        Point::mil(x, y)
    }

    /// Six copper layers 1.4 mil thick with 10 mil dielectric between:
    /// faces at 0, 11.4, 22.8, ... and the bottom face at 64.
    fn board() -> Board {
        let mut b = Board::default();
        for i in 0..6 {
            if i > 0 {
                b.stackup.layers.push(StackLayer {
                    name: format!("D{i}"),
                    altium_id: None,
                    kind: LayerKind::Dielectric,
                    thickness: Length::from_mil(10.0),
                    dielectric_constant: Some(4.0),
                    loss_tangent: None,
                    roughness_um: None,
                    material: None,
                });
            }
            b.stackup.layers.push(StackLayer {
                name: format!("L{}", i + 1),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for n in ["P", "N", "SLOW", "GND"] {
            b.nets.push(Net {
                name: n.into(),
                pieces: 1,
                ..Default::default()
            });
        }
        b.diff_pairs.push(DiffPair {
            name: "D".into(),
            positive: "P".into(),
            negative: "N".into(),
        });
        b
    }

    fn via(net: u32, x: f64) -> Via {
        Via {
            net: Some(net),
            from_layer: 0,
            to_layer: 5,
            at: p(x, 0.0),
            diameter: Length::from_mil(18.0),
            hole: Length::from_mil(8.0),
            rings: vec![],
            fab: Fab::default(),
        }
    }

    fn track(net: u32, layer: u8, x: f64) -> Track {
        Track {
            layer: LayerRef::Copper(layer),
            net: Some(net),
            start: p(x, 0.0),
            end: p(x + 200.0, 0.0),
            width: Length::from_mil(4.0),
            component: None,
            text: false,
        }
    }

    #[test]
    fn stubs_measured_backdrilled_and_checked() {
        let mut b = board();
        // P enters on L1 and leaves on L3: 41 mil of barrel below L3.
        b.geometry.vias.push(via(0, 0.0));
        b.geometry.tracks.push(track(0, 0, 0.0));
        b.geometry.tracks.push(track(0, 2, 0.0));
        // SLOW does the same but is no pair net.
        b.geometry.vias.push(via(2, 1000.0));
        b.geometry.tracks.push(track(2, 0, 1000.0));
        b.geometry.tracks.push(track(2, 2, 1000.0));
        let s = stubs(&b);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].joins, vec![0, 2]);
        assert!(s[0].top.abs() < 1e-9);
        let f = faces(&b);
        assert!(
            (s[0].bottom - (f[5].1 - f[2].1)).abs() < 1e-6,
            "{}",
            s[0].bottom
        );
        assert!(s[1].limit.is_none());

        let mut out = Vec::new();
        check(&b, 5.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, Kind::ViaStub);
        assert!(
            describe(&b, &out[0]).contains("below L3"),
            "{}",
            describe(&b, &out[0])
        );

        // Backdrill it: from the bottom, keeping L3; 8 mil is left.
        let edits = plan(&b);
        assert_eq!(edits.len(), 2);
        for e in &edits {
            e.apply(&mut b);
        }
        let v = &b.geometry.vias[0];
        assert_eq!(
            v.fab.back_bottom,
            Some(Backdrill {
                keep: 2,
                diameter: Length::from_mil(16.0)
            })
        );
        assert!(v.fab.back_top.is_none());
        assert_eq!(v.drilled(3), Some(Length::from_mil(16.0)));
        assert_eq!(v.drilled(2), None);
        assert!((stubs(&b)[0].bottom_left - LEFT).abs() < 1e-9);
        assert!(plan(&b).is_empty());
        let mut out = Vec::new();
        check(&b, 5.0, &mut out);
        assert!(out.is_empty(), "{:?}", out);

        // A GND track on L5 passing 9 mil from the via centre: inside the
        // backdrill (8 mil) plus 5 mil gap.
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(4),
            net: Some(3),
            start: p(-100.0, 11.0),
            end: p(100.0, 11.0),
            width: Length::from_mil(4.0),
            component: None,
            text: false,
        });
        let mut out = Vec::new();
        check(&b, 5.0, &mut out);
        assert_eq!(out.len(), 1, "{:?}", out);
        assert_eq!(out[0].kind, Kind::Backdrill);
        // 9 mil from the centre, 1 mil from the 16 mil drill.
        assert!((out[0].found - 1.0).abs() < 1e-6, "{}", out[0].found);
        // On L3, which the drill keeps, it would be fine.
        b.geometry.tracks.last_mut().unwrap().layer = LayerRef::Copper(2);
        b.geometry.tracks.push(track(0, 2, 0.0));
        let mut out = Vec::new();
        check(&b, 5.0, &mut out);
        assert!(out.is_empty(), "{:?}", out);

        // Drilling too deep cuts the L3 connection.
        b.geometry.vias[0].fab.back_bottom = Some(Backdrill {
            keep: 1,
            diameter: Length::from_mil(16.0),
        });
        let mut out = Vec::new();
        check(&b, 5.0, &mut out);
        assert!(
            out.iter()
                .any(|v| v.kind == Kind::Backdrill && v.rule.contains("cuts")),
            "{:?}",
            out
        );

        // Undo by clearing.
        for e in clear(&b) {
            e.apply(&mut b);
        }
        assert!(b.geometry.vias.iter().all(|v| v.fab.back_bottom.is_none()));
    }

    #[test]
    fn pour_joins_and_board_rules() {
        let mut b = board();
        // N joins a pour on L4 through relief spokes, and a track on L1.
        b.geometry.vias.push(via(1, 0.0));
        b.geometry.tracks.push(track(1, 0, 0.0));
        let relief: Vec<Point> = (0..16)
            .map(|k| {
                let t = k as f64 * std::f64::consts::TAU / 16.0;
                p(14.0 * t.cos(), 14.0 * t.sin())
            })
            .collect();
        b.geometry.polygons.push(Polygon {
            teardrop: false,
            layer: LayerRef::Copper(3),
            net: Some(1),
            outline: vec![
                p(-500.0, -500.0),
                p(500.0, -500.0),
                p(500.0, 500.0),
                p(-500.0, 500.0),
            ],
            holes: vec![relief],
            pour: None,
            component: None,
        });
        // A void 14 mil out leaves 5 mil round the 9 mil pad: the pour
        // keeps clear of the via there.
        let s = stubs(&b);
        assert_eq!(s[0].joins, vec![0], "a 5 mil void is not a join");
        // A hole the pad reaches is the relief round it.
        let relief: Vec<Point> = (0..16)
            .map(|k| {
                let t = k as f64 * std::f64::consts::TAU / 16.0;
                p(9.2 * t.cos(), 9.2 * t.sin())
            })
            .collect();
        b.geometry.polygons[0].holes = vec![relief];
        assert_eq!(stubs(&b)[0].joins, vec![0, 3]);

        // A board rule takes over from the default.
        b.rules.push(Rule {
            kind: "MaxViaStubLength".into(),
            name: "Stubs".into(),
            enabled: true,
            priority: 1,
            scope: vec!["InNet('SLOW')".into()],
            params: [
                ("MAXSTUBLENGTH", "5mil"),
                ("HOLEOVERSIZE", "4mil"),
                ("USETOPBACKDRILL", "FALSE"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        });
        assert!(stubs(&b)[0].limit.is_none(), "N is outside the rule");
        b.geometry.vias.push(via(2, 1000.0));
        b.geometry.tracks.push(track(2, 0, 1000.0));
        let s = stubs(&b);
        assert_eq!(s[1].limit, Some((5.0, "Stubs".to_string())));
        // SLOW joins on L1 only: drilled from the bottom, 4 mil over the
        // hole; the rule does not drill from the top.
        let edits = plan(&b);
        assert_eq!(edits.len(), 2);
        let Edit::InsertVia { via, .. } = &edits[1] else {
            panic!()
        };
        assert_eq!(
            via.fab.back_bottom.map(|d| d.diameter),
            Some(Length::from_mil(12.0))
        );
        assert_eq!(via.fab.back_bottom.map(|d| d.keep), Some(0));
        assert!(via.fab.back_top.is_none());
    }
}
