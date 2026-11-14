//! Teardrops: copper that widens a track where it meets a pad or via, so
//! a drill off centre or an etched notch does not cut the joint, and the
//! impedance step is gentler.
//!
//! Each teardrop is a copper area of the track's net (`Polygon::teardrop`)
//! from inside the pad out along the track, its sides curving in from most
//! of the pad's width to the track's. One that would come nearer other
//! copper than the clearance is made shorter and narrower, or left out.

use crate::edit::Edit;
use crate::route::{pt, Obstacles, P};
use crate::stitch::Area;
use crate::{Board, LayerRef, Point, Polygon};

#[derive(Clone, Debug)]
pub struct TeardropGoal {
    /// Only these nets; empty for every net.
    pub nets: Vec<u32>,
    /// Length beyond the pad's edge, as a share of the pad's width.
    pub length: f64,
    /// Width where it leaves the pad, as a share of the pad's width.
    pub width: f64,
    pub vias: bool,
    /// Through-hole pads.
    pub th_pads: bool,
    /// Surface-mount pads.
    pub smd_pads: bool,
    /// Copper to copper, mil.
    pub clearance: f64,
}

impl TeardropGoal {
    pub fn of(board: &Board) -> TeardropGoal {
        TeardropGoal {
            nets: Vec::new(),
            length: 1.0,
            width: 0.9,
            vias: true,
            th_pads: true,
            smd_pads: false,
            clearance: crate::route::board_clearance(board),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Teardrops {
    pub added: Vec<Polygon>,
    /// Joints left out: too near other copper even made small.
    pub crowded: usize,
    /// Joints where the track is as wide as the pad, or too short.
    pub not_needed: usize,
}

impl Teardrops {
    /// The teardrops as one edit; teardrops already on the board are
    /// replaced.
    pub fn edits(&self, board: &Board) -> Vec<Edit> {
        let before = board.geometry.polygons.clone();
        let mut after: Vec<Polygon> = before.iter().filter(|p| !p.teardrop).cloned().collect();
        after.extend(self.added.iter().cloned());
        vec![Edit::SetPolygons { before, after }]
    }

    pub fn summary(&self) -> String {
        format!(
            "{} teardrops; left out: {} too near other copper, {} where the track is as wide as the pad or too short",
            self.added.len(),
            self.crowded,
            self.not_needed
        )
    }
}

/// Takes every teardrop off the board, as one edit.
pub fn remove(board: &Board) -> Option<Edit> {
    let before = board.geometry.polygons.clone();
    if !before.iter().any(|p| p.teardrop) {
        return None;
    }
    let after = before.iter().filter(|p| !p.teardrop).cloned().collect();
    Some(Edit::SetPolygons { before, after })
}

/// A pad or via a track can end on, on one copper layer.
struct End {
    c: P,
    net: u32,
    layer: u8,
    /// Size along its own x and y, and its turn, mil and degrees.
    sx: f64,
    sy: f64,
    rot: f64,
}

impl End {
    /// Half its extent along the unit direction `u`.
    fn reach(&self, u: P) -> f64 {
        let (s, c) = self.rot.to_radians().sin_cos();
        // u in the pad's own frame.
        let (x, y) = (u[0] * c + u[1] * s, -u[0] * s + u[1] * c);
        (x.abs() * self.sx + y.abs() * self.sy) / 2.0
    }
}

fn ends(board: &Board, goal: &TeardropGoal) -> Vec<End> {
    let g = &board.geometry;
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let want = |net: Option<u32>| net.filter(|n| goal.nets.is_empty() || goal.nets.contains(n));
    let mut out = Vec::new();
    if goal.vias {
        for v in &g.vias {
            let Some(net) = want(v.net) else { continue };
            let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
            for l in lo..=hi.min(n - 1) {
                let d = v
                    .rings
                    .iter()
                    .find(|r| r.0 == l)
                    .map(|r| r.1.mil())
                    .unwrap_or(v.diameter.mil());
                if d > v.hole.mil() {
                    out.push(End {
                        c: pt(v.at),
                        net,
                        layer: l,
                        sx: d,
                        sy: d,
                        rot: 0.0,
                    });
                }
            }
        }
    }
    for p in &g.pads {
        let Some(net) = want(p.net) else { continue };
        let th = p.hole.0 > 0;
        if (th && !goal.th_pads) || (!th && !goal.smd_pads) || !p.outline.is_empty() {
            continue;
        }
        let layers: Vec<(u8, f64, f64, f64)> = if !p.stack.is_empty() {
            p.stack
                .iter()
                .map(|l| (l.layer, l.size_x.mil(), l.size_y.mil(), l.rotation_deg))
                .collect()
        } else {
            let (sx, sy) = (p.size_x.mil(), p.size_y.mil());
            match p.layer {
                LayerRef::Copper(l) => vec![(l, sx, sy, p.rotation_deg)],
                LayerRef::AllCopper => (0..n).map(|l| (l, sx, sy, p.rotation_deg)).collect(),
                _ => Vec::new(),
            }
        };
        for (layer, sx, sy, rot) in layers {
            out.push(End {
                c: pt(p.at),
                net,
                layer,
                sx,
                sy,
                rot,
            });
        }
    }
    out
}

/// The teardrop's outline: from `s0` to `s1` along `u` from `c`, `hw0`
/// wide (half) up to the pad's edge at `edge`, then curving in to `hw1`.
fn shape(c: P, u: P, s0: f64, edge: f64, s1: f64, hw0: f64, hw1: f64) -> Vec<P> {
    let v = [-u[1], u[0]];
    let n = 10;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut put = |s: f64, hw: f64| {
        let m = [c[0] + u[0] * s, c[1] + u[1] * s];
        left.push([m[0] + v[0] * hw, m[1] + v[1] * hw]);
        right.push([m[0] - v[0] * hw, m[1] - v[1] * hw]);
    };
    put(s0, hw0);
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let hw = hw1 + (hw0 - hw1) * (1.0 - t) * (1.0 - t);
        put(edge + (s1 - edge) * t, hw);
    }
    right.reverse();
    left.extend(right);
    left
}

/// Teardrops for every joint of a chosen net's track with a pad or via.
pub fn add(board: &Board, goal: &TeardropGoal) -> Teardrops {
    let g = &board.geometry;
    let ends = ends(board, goal);
    let obstacles = Obstacles::new(board);
    // Other nets' copper areas that stay as they are (a pour is poured
    // again around a teardrop; a fixed area is not).
    let mut fixed: Vec<(u8, Option<u32>, Area)> = g
        .polygons
        .iter()
        .filter(|p| p.pour.is_none() && !p.teardrop)
        .filter_map(|p| {
            let LayerRef::Copper(l) = p.layer else {
                return None;
            };
            let mut rings: Vec<&[Point]> = vec![&p.outline];
            rings.extend(p.holes.iter().map(|h| h.as_slice()));
            Area::new(Some(l), p.net, &rings).map(|a| (l, p.net, a))
        })
        .collect();
    let mut out = Teardrops::default();
    // Pads and vias by cell, for finding the one a track ends on.
    let cell = |p: P| ((p[0] / 100.0).floor() as i32, (p[1] / 100.0).floor() as i32);
    let mut by: std::collections::HashMap<(u8, i32, i32), Vec<usize>> = Default::default();
    for (i, e) in ends.iter().enumerate() {
        let (x, y) = cell(e.c);
        by.entry((e.layer, x, y)).or_default().push(i);
    }
    let mut done: std::collections::HashSet<(usize, i64, i64)> = Default::default();
    for t in &g.tracks {
        let LayerRef::Copper(l) = t.layer else {
            continue;
        };
        if t.text || t.component.is_some() {
            continue;
        }
        let Some(net) = t.net else { continue };
        let w = t.width.mil();
        let (a, b) = (pt(t.start), pt(t.end));
        for (e0, e1) in [(a, b), (b, a)] {
            let (x, y) = cell(e0);
            let hit = (x - 1..=x + 1)
                .flat_map(|x| (y - 1..=y + 1).map(move |y| (x, y)))
                .filter_map(|(x, y)| by.get(&(l, x, y)))
                .flatten()
                .copied()
                .filter(|&i| ends[i].net == net)
                .find(|&i| {
                    let e = &ends[i];
                    let d = (e0[0] - e.c[0]).hypot(e0[1] - e.c[1]);
                    d < e.sx.min(e.sy) / 2.0
                });
            let Some(i) = hit else { continue };
            let e = &ends[i];
            let to = [e1[0] - e.c[0], e1[1] - e.c[1]];
            let len = to[0].hypot(to[1]);
            if len < 1e-6 {
                continue;
            }
            let u = [to[0] / len, to[1] / len];
            // One teardrop per pad and direction.
            let key = (i, (u[0] * 1e3).round() as i64, (u[1] * 1e3).round() as i64);
            if !done.insert(key) {
                continue;
            }
            let edge = e.reach(u);
            let across = e.reach([-u[1], u[0]]);
            let (hw1, full) = (w / 2.0, goal.width * across);
            if full <= hw1 * 1.05 || len <= edge + w {
                out.not_needed += 1;
                continue;
            }
            let mut made = None;
            for k in [1.0, 0.7, 0.45] {
                let hw0 = hw1 + (full - hw1) * k;
                let s1 = (edge + goal.length * 2.0 * across * k).min(len);
                if s1 <= edge + w / 2.0 {
                    break;
                }
                let ring = shape(e.c, u, edge * 0.5, edge, s1, hw0, hw1);
                // Its outline must keep the clearance from other nets.
                let clear = ring.windows(2).all(|s| {
                    obstacles
                        .check_track(l, s[0], s[1], 0.0, Some(net), goal.clearance)
                        .is_none()
                }) && {
                    // Points along the outline, closer than the clearance.
                    let step = (goal.clearance / 2.0).max(0.5);
                    let pts: Vec<P> = ring
                        .windows(2)
                        .flat_map(|s| {
                            let d = (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1]);
                            let n = (d / step).ceil().max(1.0) as usize;
                            (0..n).map(move |k| {
                                let t = k as f64 / n as f64;
                                [
                                    s[0][0] + (s[1][0] - s[0][0]) * t,
                                    s[0][1] + (s[1][1] - s[0][1]) * t,
                                ]
                            })
                        })
                        .collect();
                    fixed
                        .iter()
                        .filter(|(fl, fnet, _)| *fl == l && *fnet != Some(net))
                        .all(|(_, _, a)| {
                            pts.iter()
                                .all(|&p| !a.contains(p) && !a.edge_within(p, goal.clearance))
                        })
                };
                if clear {
                    made = Some(ring);
                    break;
                }
            }
            if let Some(ring) = &made {
                // Later teardrops keep clear of this one too.
                let pts: Vec<Point> = ring.iter().map(|p| Point::mil(p[0], p[1])).collect();
                if let Some(a) = Area::new(Some(l), Some(net), &[&pts]) {
                    fixed.push((l, Some(net), a));
                }
            }
            match made {
                Some(ring) => out.added.push(Polygon {
                    layer: LayerRef::Copper(l),
                    net: Some(net),
                    outline: ring.iter().map(|p| Point::mil(p[0], p[1])).collect(),
                    holes: Vec::new(),
                    pour: None,
                    component: None,
                    teardrop: true,
                }),
                None => out.crowded += 1,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Length, Net, Pad, PadShape, Track, Via};

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["A", "B"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        for _ in 0..2 {
            b.stackup.layers.push(crate::StackLayer {
                name: "L".into(),
                altium_id: None,
                kind: crate::LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.geometry.vias.push(Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: Point::mil(0.0, 0.0),
            diameter: Length::from_mil(24.0),
            hole: Length::from_mil(12.0),
            rings: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: Point::mil(0.0, 0.0),
            end: Point::mil(200.0, 0.0),
            width: Length::from_mil(6.0),
            component: None,
            text: false,
        });
        b.geometry.pads.push(Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: None,
            name: "1".into(),
            at: Point::mil(200.0, 0.0),
            size_x: Length::from_mil(40.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b
    }

    #[test]
    fn teardrops_at_vias_and_pads_and_clear_of_others() {
        let b = board();
        let mut goal = TeardropGoal::of(&b);
        goal.clearance = 5.0;
        let t = add(&b, &goal);
        assert_eq!(t.added.len(), 1, "{}", t.summary());
        let ring: Vec<P> = t.added[0].outline.iter().map(|&p| pt(p)).collect();
        // Out along +x from the via, 0.9 of its width at the pad's edge.
        let xmax = ring.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        let ymax = ring.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        assert!((xmax - (12.0 + 24.0)).abs() < 1e-6, "{xmax}");
        assert!((ymax - 0.9 * 12.0).abs() < 1e-6, "{ymax}");
        // With SMD pads too, the pad gets one pointing back along -x.
        goal.smd_pads = true;
        let t = add(&b, &goal);
        assert_eq!(t.added.len(), 2, "{}", t.summary());
        // A track of another net beside the joint: the teardrop shrinks.
        let mut b2 = b.clone();
        b2.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: Point::mil(0.0, 17.0),
            end: Point::mil(100.0, 17.0),
            width: Length::from_mil(6.0),
            component: None,
            text: false,
        });
        goal.smd_pads = false;
        let t = add(&b2, &goal);
        for p in &t.added {
            let ymax = p.outline.iter().map(|q| q.y.mil()).fold(f64::MIN, f64::max);
            assert!(ymax <= 17.0 - 3.0 - 5.0 + 1e-6, "{ymax}");
        }
        assert_eq!(t.added.len(), 1, "{}", t.summary());
        // Adding again replaces, and removing takes them all.
        let mut b3 = b.clone();
        for e in add(&b, &goal).edits(&b) {
            e.apply(&mut b3);
        }
        for e in add(&b3, &goal).edits(&b3) {
            e.apply(&mut b3);
        }
        assert_eq!(b3.geometry.polygons.len(), 1);
        remove(&b3).unwrap().apply(&mut b3);
        assert!(b3.geometry.polygons.is_empty());
    }
}
