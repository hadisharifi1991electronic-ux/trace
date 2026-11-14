//! Autorouter: routes the connections still open, net by net, with tracks
//! and through vias around everything already on the board.
//!
//! Each net's pieces of copper are joined along their shortest links (a
//! spanning tree between the pads of different pieces). Links are routed
//! shortest first by an A* search on a grid over the routing layers: steps
//! of 0°, 45° and 90° on a layer, and a via to another layer where one
//! fits on every layer. The path is pulled tight per layer, kept only if
//! every segment keeps clearance, and becomes an obstacle for the next.
//! Links that fail are tried again after the others with a wider search.

use crate::connectivity::islands;
use crate::edit::Edit;
use crate::route::{board_clearance, board_width, common_via, Obstacles, P};
use crate::walk::{tighten, Ask};
use crate::{Board, LayerRef, Length, Point, Track, Via};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// What to route and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// Nets to route (all with open connections when `None`).
    pub nets: Option<Vec<u32>>,
    /// Copper layers to route on (0 = top); `None` for every layer that is
    /// not a plane.
    pub layers: Option<Vec<u8>>,
    /// Track width and clearance (mil); the board's rules when `None`.
    pub width: Option<f64>,
    pub clearance: Option<f64>,
    /// Via pad and drill (mil); the board's most used when `None`.
    pub via: Option<(f64, f64)>,
    /// What a via costs, as track length (mil).
    pub via_cost: f64,
    /// Leave nets with copper pours to the pours.
    pub skip_poured: bool,
}

impl Default for Goal {
    fn default() -> Self {
        Goal {
            nets: None,
            layers: None,
            width: None,
            clearance: None,
            via: None,
            via_cost: 60.0,
            skip_poured: true,
        }
    }
}

/// One link between two pads that was not routed.
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub net: u32,
    pub from: usize,
    pub to: usize,
}

/// What the autorouter did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub links: usize,
    pub routed: usize,
    pub open: Vec<Open>,
    pub tracks: usize,
    pub vias: usize,
    /// Track length added (mil).
    pub length: f64,
    pub layers: Vec<u8>,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "routed {} of {} connections: {} tracks, {} vias, {:.0} mil of track; {} left open",
            self.routed,
            self.links,
            self.tracks,
            self.vias,
            self.length,
            self.open.len()
        )
    }
}

fn pt(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn mil(p: P) -> Point {
    Point::mil(p[0], p[1])
}

/// Layers mostly covered by one net's copper areas: planes, not for
/// routing.
fn planes(board: &Board, copper: u8) -> HashSet<u8> {
    let area = |r: &[Point]| {
        let n = r.len();
        (0..n)
            .map(|i| {
                let (a, b) = (r[i], r[(i + 1) % n]);
                a.x.mil() * b.y.mil() - b.x.mil() * a.y.mil()
            })
            .sum::<f64>()
            .abs()
            / 2.0
    };
    let outline = &board.geometry.outline;
    if outline.len() < 3 {
        return HashSet::new();
    }
    let total = area(outline);
    let mut by: HashMap<(u8, u32), f64> = HashMap::new();
    for p in &board.geometry.polygons {
        if let (&LayerRef::Copper(l), Some(n)) = (&p.layer, p.net) {
            if l < copper && p.outline.len() >= 3 {
                *by.entry((l, n)).or_default() += area(&p.outline);
            }
        }
    }
    // Outer layers carry parts: never planes.
    by.into_iter()
        .filter(|&((l, _), a)| a > total * 0.5 && l > 0 && l + 1 < copper)
        .map(|((l, _), _)| l)
        .collect()
}

/// The layers routed by default: every copper layer but the planes.
pub fn routing_layers(board: &Board) -> Vec<u8> {
    let copper = board.stackup.copper_layers().count().max(1) as u8;
    let p = planes(board, copper);
    (0..copper).filter(|l| !p.contains(l)).collect()
}

/// The copper layers a pad is on, of those routed.
fn pad_layers(board: &Board, pad: usize, layers: &[u8], copper: u8) -> Vec<u8> {
    match board.geometry.pads[pad].layer {
        LayerRef::Copper(l) => layers.iter().copied().filter(|&x| x == l).collect(),
        LayerRef::AllCopper => layers.iter().copied().filter(|&x| x < copper).collect(),
        _ => Vec::new(),
    }
}

/// Each net's links: the shortest that join its pieces (Kruskal over the
/// pads of different pieces).
fn links(board: &Board, goal: &Goal) -> Vec<(u32, usize, usize, f64)> {
    let pads = &board.geometry.pads;
    let mut out = Vec::new();
    let mut groups: Vec<(u32, Vec<Vec<usize>>)> = islands(board).into_iter().collect();
    groups.sort_by_key(|g| g.0);
    for (net, isl) in groups {
        if isl.len() < 2 {
            continue;
        }
        if goal.nets.as_ref().is_some_and(|n| !n.contains(&net)) {
            continue;
        }
        let poured = board.nets.get(net as usize).is_some_and(|n| n.has_pour);
        if goal.skip_poured && poured && goal.nets.is_none() {
            continue;
        }
        // Closest pads between each two pieces.
        let mut edges = Vec::new();
        for i in 0..isl.len() {
            for j in i + 1..isl.len() {
                let mut best = (f64::MAX, 0, 0);
                for &a in &isl[i] {
                    for &b in &isl[j] {
                        let d = (pads[a].at.x.mil() - pads[b].at.x.mil())
                            .hypot(pads[a].at.y.mil() - pads[b].at.y.mil());
                        if d < best.0 {
                            best = (d, a, b);
                        }
                    }
                }
                edges.push((best.0, i, j, best.1, best.2));
            }
        }
        edges.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut root: Vec<usize> = (0..isl.len()).collect();
        fn find(r: &mut [usize], x: usize) -> usize {
            let mut x = x;
            while r[x] != x {
                r[x] = r[r[x]];
                x = r[x];
            }
            x
        }
        for (d, i, j, a, b) in edges {
            let (ri, rj) = (find(&mut root, i), find(&mut root, j));
            if ri != rj {
                root[ri] = rj;
                out.push((net, a, b, d));
            }
        }
    }
    out.sort_by(|a, b| a.3.total_cmp(&b.3));
    out
}

#[derive(PartialEq)]
struct Node {
    f: f64,
    g: f64,
    at: (i32, i32, u8),
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| self.g.total_cmp(&o.g))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

struct Search<'a> {
    obstacles: &'a Obstacles,
    layers: &'a [u8],
    copper: u8,
    net: u32,
    r: f64,
    via_r: f64,
    clearance: f64,
    via_cost: f64,
    /// Vias of the net already there: a new one keeps its hole clear of
    /// them.
    own_vias: &'a [P],
}

/// A routed path: runs of points per layer, joined by vias.
type Path = Vec<(u8, Vec<P>)>;

impl Search<'_> {
    fn nets(&self) -> [Option<u32>; 1] {
        [Some(self.net)]
    }

    fn clear(&self, l: u8, a: P, b: P, r: f64) -> bool {
        self.obstacles
            .clear(l, a, b, r, &self.nets(), self.clearance, &|_| false)
    }

    fn via_fits(&self, p: P) -> bool {
        let room = 2.0 * self.via_r + self.clearance;
        self.own_vias
            .iter()
            .all(|v| (v[0] - p[0]).hypot(v[1] - p[1]) >= room)
            && (0..self.copper).all(|l| self.clear(l, p, p, self.via_r))
    }

    /// A* from `from` on any of `la` to `to` on any of `lb`, within the box
    /// around them grown by `grow` times their distance.
    fn run(&self, from: P, la: &[u8], to: P, lb: &[u8], grow: f64, cap: usize) -> Option<Path> {
        let dist = (to[0] - from[0]).hypot(to[1] - from[1]);
        let step = ((self.r + self.clearance) / 2.0)
            .clamp(1.5, 25.0)
            .max(dist / 600.0);
        let margin = (dist * grow).max(40.0 * step);
        let lo = [from[0].min(to[0]) - margin, from[1].min(to[1]) - margin];
        let hi = [from[0].max(to[0]) + margin, from[1].max(to[1]) + margin];
        let at = |c: (i32, i32)| [from[0] + c.0 as f64 * step, from[1] + c.1 as f64 * step];
        let goal = (
            ((to[0] - from[0]) / step).round() as i32,
            ((to[1] - from[1]) / step).round() as i32,
        );
        let h = |c: (i32, i32), l: u8| {
            let (dx, dy) = ((c.0 - goal.0).abs() as f64, (c.1 - goal.1).abs() as f64);
            let d = (dx.max(dy) + (std::f64::consts::SQRT_2 - 1.0) * dx.min(dy)) * step;
            d + if lb.contains(&l) { 0.0 } else { self.via_cost }
        };
        let mut free: HashMap<(i32, i32, u8), bool> = HashMap::new();
        let mut via_ok: HashMap<(i32, i32), bool> = HashMap::new();
        let mut best: HashMap<(i32, i32, u8), f64> = HashMap::new();
        let mut came: HashMap<(i32, i32, u8), (i32, i32, u8)> = HashMap::new();
        let mut open = BinaryHeap::new();
        for &l in la {
            best.insert((0, 0, l), 0.0);
            open.push(Node {
                f: h((0, 0), l),
                g: 0.0,
                at: (0, 0, l),
            });
        }
        let mut done = None;
        let mut n = 0;
        while let Some(Node { g, at: c, .. }) = open.pop() {
            if g > best.get(&c).copied().unwrap_or(f64::MAX) + 1e-9 {
                continue;
            }
            if (c.0, c.1) == goal && lb.contains(&c.2) {
                done = Some(c);
                break;
            }
            n += 1;
            if n > cap {
                break;
            }
            let mut push = |nc: (i32, i32, u8), ng: f64, open: &mut BinaryHeap<Node>| {
                if ng < best.get(&nc).copied().unwrap_or(f64::MAX) - 1e-9 {
                    best.insert(nc, ng);
                    came.insert(nc, c);
                    open.push(Node {
                        f: ng + h((nc.0, nc.1), nc.2),
                        g: ng,
                        at: nc,
                    });
                }
            };
            let p = at((c.0, c.1));
            for (k, &(dx, dy)) in DIRS.iter().enumerate() {
                let nc = (c.0 + dx, c.1 + dy, c.2);
                let q = at((nc.0, nc.1));
                if q[0] < lo[0] || q[0] > hi[0] || q[1] < lo[1] || q[1] > hi[1] {
                    continue;
                }
                let is_goal = (nc.0, nc.1) == goal && lb.contains(&nc.2);
                let ok = is_goal
                    || *free
                        .entry(nc)
                        .or_insert_with(|| self.clear(nc.2, q, q, self.r));
                if !ok {
                    continue;
                }
                // Diagonal steps must not cut a corner.
                if k % 2 == 1 && !self.clear(c.2, p, q, self.r) {
                    continue;
                }
                let cost = if k % 2 == 1 {
                    step * std::f64::consts::SQRT_2
                } else {
                    step
                };
                push(nc, g + cost, &mut open);
            }
            if self.layers.len() > 1 {
                let fits = *via_ok.entry((c.0, c.1)).or_insert_with(|| self.via_fits(p));
                if fits {
                    for &l in self.layers {
                        if l != c.2 {
                            push((c.0, c.1, l), g + self.via_cost, &mut open);
                        }
                    }
                }
            }
        }
        let end = done?;
        let mut cells = vec![end];
        let mut c = end;
        while let Some(&p) = came.get(&c) {
            cells.push(p);
            c = p;
        }
        cells.reverse();
        // Runs per layer; the ends are the pad centres exactly.
        let mut path: Path = Vec::new();
        for (i, &(x, y, l)) in cells.iter().enumerate() {
            let mut q = at((x, y));
            if i == 0 {
                q = from;
            } else if i + 1 == cells.len() {
                q = to;
            }
            match path.last_mut() {
                Some((pl, run)) if *pl == l => run.push(q),
                _ => {
                    let mut run = Vec::new();
                    if let Some((_, prev)) = path.last() {
                        run.push(*prev.last().unwrap_or(&q));
                    }
                    if run.last() != Some(&q) {
                        run.push(q);
                    }
                    path.push((l, run));
                }
            }
        }
        Some(path)
    }
}

/// Edits that take up every track, arc and via of a net.
pub fn unroute(board: &Board, net: u32) -> Vec<Edit> {
    let g = &board.geometry;
    let mut edits = Vec::new();
    for (i, t) in g.tracks.iter().enumerate().rev() {
        if t.net == Some(net) && matches!(t.layer, LayerRef::Copper(_)) {
            edits.push(Edit::RemoveTrack {
                index: i,
                track: t.clone(),
            });
        }
    }
    for (i, a) in g.arcs.iter().enumerate().rev() {
        if a.net == Some(net) && matches!(a.layer, LayerRef::Copper(_)) {
            edits.push(Edit::RemoveArc {
                index: i,
                arc: a.clone(),
            });
        }
    }
    for (i, v) in g.vias.iter().enumerate().rev() {
        if v.net == Some(net) {
            edits.push(Edit::RemoveVia {
                index: i,
                via: v.clone(),
            });
        }
    }
    edits
}

/// Routes the board's open connections. The edits add the tracks and vias
/// (one undo step when applied together).
pub fn autoroute(board: &Board, goal: &Goal) -> (Vec<Edit>, Report) {
    let copper = board.stackup.copper_layers().count().max(1) as u8;
    let layers: Vec<u8> = match &goal.layers {
        Some(l) => l.iter().copied().filter(|&x| x < copper).collect(),
        None => routing_layers(board),
    };
    let (base_w, base_c) = (board_width(board), board_clearance(board));
    let rules = crate::drc::net_rules(board);
    let (via_d, via_hole) = goal.via.unwrap_or_else(|| common_via(board));
    let mut obstacles = Obstacles::new(board);
    // Nets whose clearance rule asks more than the board's keep it from
    // everything routed near them too.
    obstacles.set_net_gaps(
        rules
            .iter()
            .enumerate()
            .filter_map(|(n, r)| r.1.filter(|&g| g > base_c).map(|g| (n as u32, g)))
            .collect(),
    );
    let mut rep = Report {
        layers: layers.clone(),
        ..Default::default()
    };
    let todo = links(board, goal);
    rep.links = todo.len();
    let mut edits = Vec::new();
    let (mut nt, mut nv) = (board.geometry.tracks.len(), board.geometry.vias.len());
    let mut failed = Vec::new();
    let mut vias_of: HashMap<u32, Vec<P>> = HashMap::new();
    for v in &board.geometry.vias {
        if let Some(n) = v.net {
            vias_of.entry(n).or_default().push(pt(v.at));
        }
    }
    for pass in 0..2 {
        let list = if pass == 0 {
            todo.clone()
        } else {
            std::mem::take(&mut failed)
        };
        for (net, a, b, _) in list {
            let rule = rules.get(net as usize).copied().unwrap_or((None, None));
            let width = goal.width.or(rule.0).unwrap_or(base_w);
            let clearance = goal.clearance.or(rule.1).unwrap_or(base_c);
            let pads = &board.geometry.pads;
            let (pa, pb) = (pt(pads[a].at), pt(pads[b].at));
            let (la, lb) = (
                pad_layers(board, a, &layers, copper),
                pad_layers(board, b, &layers, copper),
            );
            let s = Search {
                obstacles: &obstacles,
                layers: &layers,
                copper,
                net,
                r: width / 2.0,
                via_r: via_d / 2.0,
                clearance,
                via_cost: goal.via_cost * if pass == 0 { 1.0 } else { 3.0 },
                own_vias: vias_of.get(&net).map(Vec::as_slice).unwrap_or(&[]),
            };
            let found = if la.is_empty() || lb.is_empty() {
                None
            } else if pass == 0 {
                s.run(pa, &la, pb, &lb, 0.5, 150_000)
            } else {
                s.run(pa, &la, pb, &lb, 1.5, 600_000)
            };
            // Pull each run tight and check every segment.
            let path = found.and_then(|path| {
                let nets = [Some(net)];
                let mut out: Path = Vec::new();
                for (l, run) in path {
                    let ask = Ask {
                        obstacles: &obstacles,
                        layer: l,
                        r: width / 2.0,
                        nets: &nets,
                        clearance,
                        extra: &[],
                        skip: &[],
                    };
                    let tight = if run.len() > 2 {
                        tighten(&ask, run.clone())
                    } else {
                        run.clone()
                    };
                    let ok = |r: &[P]| {
                        r.windows(2).all(|w| {
                            // The ends sit in their own pads.
                            s.clear(l, w[0], w[1], width / 2.0)
                                || w[0] == pa
                                || w[1] == pb
                                || w[0] == pb
                                || w[1] == pa
                        })
                    };
                    if ok(&tight) {
                        out.push((l, tight));
                    } else if ok(&run) {
                        out.push((l, run));
                    } else {
                        return None;
                    }
                }
                // Its own vias must keep their holes apart too.
                let ends: Vec<P> = out[..out.len().saturating_sub(1)]
                    .iter()
                    .filter_map(|(_, r)| r.last().copied())
                    .collect();
                let room = via_d + clearance;
                for (i, a) in ends.iter().enumerate() {
                    if ends[i + 1..]
                        .iter()
                        .any(|b| (a[0] - b[0]).hypot(a[1] - b[1]) < room)
                    {
                        return None;
                    }
                }
                Some(out)
            });
            let Some(path) = path else {
                if pass == 0 {
                    failed.push((net, a, b, 0.0));
                } else {
                    rep.open.push(Open {
                        net,
                        from: a,
                        to: b,
                    });
                }
                continue;
            };
            rep.routed += 1;
            for (k, (l, run)) in path.iter().enumerate() {
                for w in run.windows(2) {
                    if w[0] == w[1] {
                        continue;
                    }
                    let track = Track {
                        layer: LayerRef::Copper(*l),
                        net: Some(net),
                        start: mil(w[0]),
                        end: mil(w[1]),
                        width: Length::from_mil(width),
                        component: None,
                        text: false,
                    };
                    obstacles.add(Some(*l), w[0], w[1], width / 2.0, Some(net));
                    rep.length += (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
                    edits.push(Edit::InsertTrack { index: nt, track });
                    nt += 1;
                    rep.tracks += 1;
                }
                // A via where this run hands over to the next layer.
                if k + 1 < path.len() {
                    let at = *run.last().unwrap_or(&pa);
                    let via = Via {
                        net: Some(net),
                        from_layer: 0,
                        to_layer: copper - 1,
                        at: mil(at),
                        diameter: Length::from_mil(via_d),
                        hole: Length::from_mil(via_hole),
                        rings: Vec::new(),
                        fab: Default::default(),
                    };
                    obstacles.add(None, at, at, via_d / 2.0, Some(net));
                    vias_of.entry(net).or_default().push(at);
                    edits.push(Edit::InsertVia { index: nv, via });
                    nv += 1;
                    rep.vias += 1;
                }
            }
        }
    }
    (edits, rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::analyse;
    use crate::{Net, Pad, PadShape, StackLayer};

    fn board(layers: usize) -> Board {
        let mut b = Board::default();
        let mut st = Vec::new();
        for i in 0..layers {
            st.push(StackLayer {
                name: format!("L{}", i + 1),
                altium_id: None,
                kind: crate::LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.stackup.layers = st;
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(2000.0, 0.0),
            Point::mil(2000.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        b
    }

    fn pad(b: &mut Board, x: f64, y: f64, net: u32, layer: LayerRef) -> usize {
        b.geometry.pads.push(Pad {
            layer,
            net: Some(net),
            component: None,
            name: "1".into(),
            at: Point::mil(x, y),
            size_x: Length::from_mil(40.0),
            size_y: Length::from_mil(40.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.pads.len() - 1
    }

    #[test]
    fn routes_around_a_wall_and_through_a_via() {
        let mut b = board(2);
        for n in ["A", "B", "WALL"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        // A from (200,500) to (1800,500) on top; a wall of net WALL on top
        // across the whole board between them, so A must change layer.
        pad(&mut b, 200.0, 500.0, 0, LayerRef::Copper(0));
        pad(&mut b, 1800.0, 500.0, 0, LayerRef::Copper(0));
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(2),
            start: Point::mil(1000.0, -100.0),
            end: Point::mil(1000.0, 1100.0),
            width: Length::from_mil(20.0),
            component: None,
            text: false,
        });
        // B: two pads on the bottom with a short block in the way.
        pad(&mut b, 300.0, 200.0, 1, LayerRef::Copper(1));
        pad(&mut b, 700.0, 200.0, 1, LayerRef::Copper(1));
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(1),
            net: Some(2),
            start: Point::mil(500.0, 100.0),
            end: Point::mil(500.0, 300.0),
            width: Length::from_mil(20.0),
            component: None,
            text: false,
        });
        analyse(&mut b);
        assert_eq!(b.nets[0].pieces, 2);
        let (edits, rep) = autoroute(&b, &Goal::default());
        assert_eq!(rep.links, 2, "{rep:?}");
        assert_eq!(rep.routed, 2, "{rep:?}");
        assert!(rep.vias >= 2, "{rep:?}");
        let mut done = b.clone();
        for e in &edits {
            e.apply(&mut done);
        }
        analyse(&mut done);
        assert_eq!(done.nets[0].pieces, 1);
        assert_eq!(done.nets[1].pieces, 1);
        let v = crate::drc::check(&done, 1000);
        let bad: Vec<_> = v
            .iter()
            .filter(|v| {
                matches!(
                    v.kind,
                    crate::drc::Kind::Clearance | crate::drc::Kind::Short
                )
            })
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}
