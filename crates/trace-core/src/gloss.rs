//! Gloss: tidies routed copper as Allegro's gloss and Altium's clean-up do.
//!
//! - zero-length tracks and exact duplicates go;
//! - two tracks of one width meeting in a straight line, with nothing else
//!   at the joint, become one;
//! - dangling ends (a track end touching nothing of its net: no track, arc,
//!   pad, via or copper area) are cut back, over and over, as far as they go;
//! - square corners, where two tracks meet at 90° with nothing else there,
//!   get a 45° chamfer when the chamfer keeps the clearance.

use crate::edit::Edit;
use crate::route::{point_seg, pt, Obstacles, P};
use crate::{Board, LayerRef, Point, Track};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct GlossGoal {
    /// Only these nets; empty for every net.
    pub nets: Vec<u32>,
    pub merge: bool,
    pub dangling: bool,
    pub corners: bool,
    pub clearance: f64,
}

impl GlossGoal {
    pub fn of(board: &Board) -> GlossGoal {
        GlossGoal {
            nets: Vec::new(),
            merge: true,
            dangling: true,
            corners: false,
            clearance: crate::route::board_clearance(board),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Glossed {
    /// Board tracks taken out, by index.
    pub removed: Vec<usize>,
    /// Tracks put in (new and changed ones).
    pub added: Vec<Track>,
    pub zero: usize,
    pub duplicate: usize,
    pub merged: usize,
    pub dangling: usize,
    pub corners: usize,
}

impl Glossed {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} zero-length and {} duplicate tracks removed, {} joins merged, {} dangling tracks cut, {} corners chamfered",
            self.zero, self.duplicate, self.merged, self.dangling, self.corners
        )
    }

    /// As edits: removals from the highest index down, then the new tracks
    /// at the end.
    pub fn edits(&self, board: &Board) -> Vec<Edit> {
        let g = &board.geometry;
        let mut removed = self.removed.clone();
        removed.sort_unstable();
        removed.dedup();
        let mut out: Vec<Edit> = removed
            .iter()
            .rev()
            .filter_map(|&i| {
                g.tracks.get(i).map(|t| Edit::RemoveTrack {
                    index: i,
                    track: t.clone(),
                })
            })
            .collect();
        let n = g.tracks.len() - removed.len();
        out.extend(
            self.added
                .iter()
                .enumerate()
                .map(|(k, t)| Edit::InsertTrack {
                    index: n + k,
                    track: t.clone(),
                }),
        );
        out
    }
}

fn key(p: P) -> (i64, i64) {
    (
        (p[0] * 1000.0).round() as i64,
        (p[1] * 1000.0).round() as i64,
    )
}

const CELL: f64 = 100.0;

/// (layer, cell) → (net, centre, reach).
type SpotMap = HashMap<(u8, i32, i32), Vec<(u32, P, f64)>>;
/// A track as layer, net, its ends in order, width.
type DupKey = (u8, u32, (i64, i64), (i64, i64), i64);

fn cells(a: P, b: P, r: f64) -> impl Iterator<Item = (i32, i32)> {
    let (x0, x1) = (
        ((a[0].min(b[0]) - r) / CELL).floor() as i32,
        ((a[0].max(b[0]) + r) / CELL).floor() as i32,
    );
    let (y0, y1) = (
        ((a[1].min(b[1]) - r) / CELL).floor() as i32,
        ((a[1].max(b[1]) + r) / CELL).floor() as i32,
    );
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

/// Same-net copper other than tracks that a track end may rest on.
struct Fixed {
    /// (layer, cell) → (net, centre, reach) of pads, vias and arc ends.
    spots: SpotMap,
    /// Copper areas: (layer, net, outline).
    areas: Vec<(u8, u32, Vec<P>, [f64; 4])>,
}

impl Fixed {
    fn new(board: &Board) -> Fixed {
        let g = &board.geometry;
        let n = board.stackup.copper_layers().count().max(1) as u8;
        let mut spots: SpotMap = HashMap::new();
        let mut put = |l: u8, net: u32, c: P, r: f64| {
            for k in cells(c, c, r) {
                spots.entry((l, k.0, k.1)).or_default().push((net, c, r));
            }
        };
        for v in &g.vias {
            let Some(net) = v.net else { continue };
            for l in v.from_layer.min(v.to_layer)..=v.from_layer.max(v.to_layer).min(n - 1) {
                put(l, net, pt(v.at), v.diameter.mil().max(v.hole.mil()) / 2.0);
            }
        }
        for p in &g.pads {
            let Some(net) = p.net else { continue };
            let r = p.size_x.mil().max(p.size_y.mil()) / 2.0;
            match p.layer {
                LayerRef::Copper(l) => put(l, net, pt(p.at), r),
                LayerRef::AllCopper => (0..n).for_each(|l| put(l, net, pt(p.at), r)),
                _ => {}
            }
        }
        for a in &g.arcs {
            let (Some(net), LayerRef::Copper(l)) = (a.net, &a.layer) else {
                continue;
            };
            let c = pt(a.center);
            let r = a.radius.mil();
            for d in [a.start_deg, a.end_deg] {
                let t = d.to_radians();
                put(
                    *l,
                    net,
                    [c[0] + r * t.cos(), c[1] + r * t.sin()],
                    a.width.mil() / 2.0,
                );
            }
        }
        let areas = g
            .polygons
            .iter()
            .filter_map(|p| match (p.net, &p.layer) {
                (Some(net), LayerRef::Copper(l)) => {
                    let v: Vec<P> = p.outline.iter().map(|&q| pt(q)).collect();
                    let b = v
                        .iter()
                        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| {
                            [
                                b[0].min(q[0]),
                                b[1].min(q[1]),
                                b[2].max(q[0]),
                                b[3].max(q[1]),
                            ]
                        });
                    Some((*l, net, v, b))
                }
                _ => None,
            })
            .collect();
        Fixed { spots, areas }
    }

    /// Whether a pad, via or arc end of `net` is at `p` on layer `l`.
    fn at(&self, l: u8, net: u32, p: P, slack: f64) -> bool {
        let c = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        self.spots.get(&(l, c.0, c.1)).is_some_and(|v| {
            v.iter()
                .any(|&(n, q, r)| n == net && (q[0] - p[0]).hypot(q[1] - p[1]) <= r + slack)
        })
    }

    fn in_area(&self, l: u8, net: u32, p: P) -> bool {
        self.areas.iter().any(|(al, an, v, b)| {
            *al == l
                && *an == net
                && p[0] >= b[0]
                && p[0] <= b[2]
                && p[1] >= b[1]
                && p[1] <= b[3]
                && v.len() >= 3
                && crate::route::inside(p, v)
        })
    }
}

/// A working track.
#[derive(Clone)]
struct W {
    l: u8,
    net: u32,
    a: P,
    b: P,
    w: f64,
    /// The board track it was, unchanged so far.
    from: Option<usize>,
}

pub fn gloss(board: &Board, goal: &GlossGoal) -> Glossed {
    let g = &board.geometry;
    let fixed = Fixed::new(board);
    let mut out = Glossed::default();
    let mut ws: Vec<Option<W>> = Vec::new();
    let mut seen: std::collections::HashSet<DupKey> = Default::default();
    for (i, t) in g.tracks.iter().enumerate() {
        let (LayerRef::Copper(l), Some(net)) = (&t.layer, t.net) else {
            continue;
        };
        if t.text || t.component.is_some() || !(goal.nets.is_empty() || goal.nets.contains(&net)) {
            continue;
        }
        let (a, b) = (pt(t.start), pt(t.end));
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-3 {
            out.removed.push(i);
            out.zero += 1;
            continue;
        }
        let (ka, kb) = (key(a).min(key(b)), key(a).max(key(b)));
        if !seen.insert((*l, net, ka, kb, t.width.0)) {
            out.removed.push(i);
            out.duplicate += 1;
            continue;
        }
        ws.push(Some(W {
            l: *l,
            net,
            a,
            b,
            w: t.width.mil(),
            from: Some(i),
        }));
    }
    // Track ends by layer and point.
    let ends = |ws: &[Option<W>]| {
        let mut m: HashMap<(u8, (i64, i64)), Vec<usize>> = HashMap::new();
        for (i, w) in ws.iter().enumerate() {
            if let Some(w) = w {
                m.entry((w.l, key(w.a))).or_default().push(i);
                m.entry((w.l, key(w.b))).or_default().push(i);
            }
        }
        m
    };
    let kill = |ws: &mut Vec<Option<W>>, i: usize, out: &mut Glossed| {
        if let Some(w) = ws[i].take() {
            if let Some(f) = w.from {
                out.removed.push(f);
            }
        }
    };
    // A plain joint: just these two tracks, nothing else of the net.
    // Tracks by layer and cell.
    let segs = |ws: &[Option<W>]| {
        let mut grid: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
        for (i, w) in ws.iter().enumerate() {
            if let Some(w) = w {
                for c in cells(w.a, w.b, w.w / 2.0) {
                    grid.entry((w.l, c.0, c.1)).or_default().push(i);
                }
            }
        }
        grid
    };
    // A plain joint: just these two tracks end there, and no other copper
    // of the net (a pad, via, arc end, or a third track touching) is at it.
    let plain = |ws: &[Option<W>],
                 m: &HashMap<(u8, (i64, i64)), Vec<usize>>,
                 grid: &HashMap<(u8, i32, i32), Vec<usize>>,
                 l: u8,
                 p: P| {
        let list = &m[&(l, key(p))];
        let live: Vec<usize> = list.iter().copied().filter(|&i| ws[i].is_some()).collect();
        if live.len() != 2 || live[0] == live[1] {
            return None;
        }
        let net = ws[live[0]].as_ref()?.net;
        let w = ws[live[0]].as_ref()?.w;
        if ws[live[1]].as_ref()?.net != net || fixed.at(l, net, p, w / 2.0) {
            return None;
        }
        let c = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        let touched = grid.get(&(l, c.0, c.1)).is_some_and(|v| {
            v.iter().any(|&j| {
                !live.contains(&j)
                    && ws[j]
                        .as_ref()
                        .is_some_and(|o| point_seg(p, o.a, o.b) <= (o.w + w) / 2.0)
            })
        });
        (!touched).then_some((live[0], live[1]))
    };
    let far = |w: &W, p: P| if key(w.a) == key(p) { w.b } else { w.a };
    if goal.merge {
        loop {
            let m = ends(&ws);
            let grid = segs(&ws);
            let mut changed = false;
            let mut joints: Vec<(u8, (i64, i64))> = m.keys().copied().collect();
            joints.sort_unstable();
            for (l, k) in joints {
                let p = [k.0 as f64 / 1000.0, k.1 as f64 / 1000.0];
                let Some((i, j)) = plain(&ws, &m, &grid, l, p) else {
                    continue;
                };
                let (wi, wj) = (ws[i].clone().unwrap(), ws[j].clone().unwrap());
                if (wi.w - wj.w).abs() > 1e-6 {
                    continue;
                }
                let (fi, fj) = (far(&wi, p), far(&wj, p));
                let (u, v) = ([fi[0] - p[0], fi[1] - p[1]], [fj[0] - p[0], fj[1] - p[1]]);
                let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
                let cross = (u[0] * v[1] - u[1] * v[0]) / (lu * lv);
                let dot = (u[0] * v[0] + u[1] * v[1]) / (lu * lv);
                if cross.abs() > 1e-4 || dot > 0.0 {
                    continue;
                }
                kill(&mut ws, i, &mut out);
                kill(&mut ws, j, &mut out);
                ws.push(Some(W {
                    l,
                    net: wi.net,
                    a: fi,
                    b: fj,
                    w: wi.w,
                    from: None,
                }));
                out.merged += 1;
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }
    if goal.dangling {
        loop {
            let m = ends(&ws);
            // Segments by cell, for T joints (a track ending on another).
            let mut grid: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
            for (i, w) in ws.iter().enumerate() {
                if let Some(w) = w {
                    for c in cells(w.a, w.b, w.w / 2.0) {
                        grid.entry((w.l, c.0, c.1)).or_default().push(i);
                    }
                }
            }
            let mut cut = Vec::new();
            for (i, w) in ws.iter().enumerate() {
                let Some(w) = w else { continue };
                for p in [w.a, w.b] {
                    let others = m[&(w.l, key(p))].iter().any(|&j| j != i && ws[j].is_some());
                    if others || fixed.at(w.l, w.net, p, 0.0) || fixed.in_area(w.l, w.net, p) {
                        continue;
                    }
                    let c = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
                    let on_track = grid.get(&(w.l, c.0, c.1)).is_some_and(|v| {
                        v.iter().any(|&j| {
                            j != i
                                && ws[j].as_ref().is_some_and(|o| {
                                    o.net == w.net && point_seg(p, o.a, o.b) <= o.w / 2.0 + 1e-6
                                })
                        })
                    });
                    if !on_track {
                        cut.push(i);
                        break;
                    }
                }
            }
            if cut.is_empty() {
                break;
            }
            for i in cut {
                kill(&mut ws, i, &mut out);
                out.dangling += 1;
            }
        }
    }
    if goal.corners {
        let obstacles = Obstacles::new(board);
        let m = ends(&ws);
        let grid = segs(&ws);
        let mut joints: Vec<(u8, (i64, i64))> = m.keys().copied().collect();
        joints.sort_unstable();
        for (l, k) in joints {
            let p = [k.0 as f64 / 1000.0, k.1 as f64 / 1000.0];
            let Some((i, j)) = plain(&ws, &m, &grid, l, p) else {
                continue;
            };
            let (wi, wj) = (ws[i].clone().unwrap(), ws[j].clone().unwrap());
            if (wi.w - wj.w).abs() > 1e-6 {
                continue;
            }
            let (fi, fj) = (far(&wi, p), far(&wj, p));
            let (lu, lv) = (
                (fi[0] - p[0]).hypot(fi[1] - p[1]),
                (fj[0] - p[0]).hypot(fj[1] - p[1]),
            );
            let u = [(fi[0] - p[0]) / lu, (fi[1] - p[1]) / lu];
            let v = [(fj[0] - p[0]) / lv, (fj[1] - p[1]) / lv];
            if (u[0] * v[0] + u[1] * v[1]).abs() > 1e-3 {
                continue;
            }
            let c = (lu.min(lv) * 0.4).min(4.0 * wi.w);
            if c < wi.w {
                continue;
            }
            let (pi, pj) = (
                [p[0] + u[0] * c, p[1] + u[1] * c],
                [p[0] + v[0] * c, p[1] + v[1] * c],
            );
            if obstacles
                .check_track(l, pi, pj, wi.w, Some(wi.net), goal.clearance)
                .is_some()
            {
                continue;
            }
            kill(&mut ws, i, &mut out);
            kill(&mut ws, j, &mut out);
            for (a, b) in [(fi, pi), (pi, pj), (pj, fj)] {
                ws.push(Some(W {
                    l,
                    net: wi.net,
                    a,
                    b,
                    w: wi.w,
                    from: None,
                }));
            }
            out.corners += 1;
        }
    }
    for w in ws.into_iter().flatten() {
        if w.from.is_none() {
            out.added.push(Track {
                layer: LayerRef::Copper(w.l),
                net: Some(w.net),
                start: Point::mil(w.a[0], w.a[1]),
                end: Point::mil(w.b[0], w.b[1]),
                width: crate::Length::from_mil(w.w),
                component: None,
                text: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Length, Net, Via};

    fn t(net: u32, a: (f64, f64), b: (f64, f64)) -> Track {
        Track {
            layer: LayerRef::Copper(0),
            net: Some(net),
            start: Point::mil(a.0, a.1),
            end: Point::mil(b.0, b.1),
            width: Length::from_mil(6.0),
            component: None,
            text: false,
        }
    }

    fn via(x: f64, y: f64) -> Via {
        Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: Point::mil(x, y),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: Vec::new(),
            fab: Default::default(),
        }
    }

    #[test]
    fn gloss_merges_cuts_and_chamfers() {
        let mut b = Board::default();
        b.nets.push(Net {
            name: "A".into(),
            ..Default::default()
        });
        b.geometry.vias = vec![via(0.0, 0.0), via(300.0, 100.0)];
        b.geometry.tracks = vec![
            // Via to via: straight in two pieces, then a square corner.
            t(0, (0.0, 0.0), (100.0, 0.0)),
            t(0, (100.0, 0.0), (300.0, 0.0)),
            t(0, (300.0, 0.0), (300.0, 100.0)),
            // A duplicate and a zero-length one.
            t(0, (300.0, 100.0), (300.0, 0.0)),
            t(0, (50.0, 50.0), (50.0, 50.0)),
            // A stub off the middle, two pieces long, going nowhere.
            t(0, (200.0, 0.0), (200.0, 80.0)),
            t(0, (200.0, 80.0), (220.0, 120.0)),
        ];
        let mut goal = GlossGoal::of(&b);
        goal.corners = true;
        goal.clearance = 5.0;
        let r = gloss(&b, &goal);
        assert_eq!(
            (r.zero, r.duplicate, r.dangling, r.corners),
            (1, 1, 2, 1),
            "{}",
            r.summary()
        );
        // The two straight pieces would merge, but the stub ends on the
        // joint-free middle of the second (a T): merging happens first.
        assert_eq!(r.merged, 1, "{}", r.summary());
        let mut b2 = b.clone();
        for e in r.edits(&b) {
            e.apply(&mut b2);
        }
        let tracks = &b2.geometry.tracks;
        // From the via along x to the chamfer, the chamfer, then up to the via.
        assert_eq!(tracks.len(), 3, "{tracks:?}");
        let len: f64 = tracks
            .iter()
            .map(|t| (t.end.x.mil() - t.start.x.mil()).hypot(t.end.y.mil() - t.start.y.mil()))
            .sum();
        assert!(len < 400.0 && len > 380.0, "{len}");
        // Glossing again finds nothing.
        assert!(gloss(&b2, &goal).is_empty());
    }
}
