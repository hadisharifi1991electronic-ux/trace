//! Test points for in-circuit test (bed of nails) and flying probes, as
//! Altium's testpoint manager and Allegro's testprep assign them: one
//! probe point per net on a via or pad reachable from the probed side,
//! large enough to hit, clear of part bodies, the board edge and the other
//! probes. Marked points keep the flag in the pad's or via's fab details.

use crate::body::{bodiless, part_bodies};
use crate::edit::Edit;
use crate::{Board, LayerRef, Length, Point};
use std::collections::HashMap;
use std::fmt::Write;

/// Sides probes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    Top,
    Bottom,
    Both,
}

#[derive(Clone, Debug)]
pub struct TestGoal {
    pub sides: Sides,
    pub vias: bool,
    /// Through-hole pads (connector and part leads).
    pub th_pads: bool,
    /// Surface pads of parts (test point parts' pads are always used).
    pub smd_pads: bool,
    /// Smallest land a probe can hit.
    pub min_size: Length,
    /// Closest two probes on one side may be, centre to centre.
    pub spacing: Length,
    /// Room between a probe point and a part body on its side.
    pub part_clearance: Length,
    /// Room from the board edge.
    pub edge: Length,
    /// Keep the test points already marked.
    pub keep: bool,
    /// Vias covered by solder mask can be probed (the mask is opened).
    pub tented: bool,
}

impl Default for TestGoal {
    fn default() -> Self {
        TestGoal {
            sides: Sides::Bottom,
            vias: true,
            th_pads: true,
            smd_pads: false,
            min_size: Length::from_mm(0.5),
            spacing: Length::from_mm(1.27),
            part_clearance: Length::from_mm(0.5),
            edge: Length::from_mm(2.0),
            keep: true,
            tented: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Obj {
    Pad(usize),
    Via(usize),
}

/// One probe point.
#[derive(Clone, Debug, PartialEq)]
pub struct TestPoint {
    pub net: u32,
    pub obj: Obj,
    pub top: bool,
    pub at: Point,
    pub size: Length,
}

#[derive(Clone, Debug, Default)]
pub struct TestReport {
    pub points: Vec<TestPoint>,
    /// Nets left without a probe point, and why.
    pub missing: Vec<(u32, &'static str)>,
    /// Nets that need one: two or more pads.
    pub nets: usize,
}

impl TestReport {
    /// Share of the nets that have a probe point, in percent.
    pub fn coverage(&self) -> f64 {
        if self.nets == 0 {
            100.0
        } else {
            100.0 * (self.nets - self.missing.len()) as f64 / self.nets as f64
        }
    }
}

/// A candidate: what, side, where, size, rank (lower is better).
struct Cand {
    obj: Obj,
    top: bool,
    at: [f64; 2],
    size: f64,
    rank: u8,
}

/// Distance from a point to a closed ring, negative inside.
fn signed_dist(v: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = v.len();
    let mut d = f64::MAX;
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (v[i], v[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx * dx + dy * dy;
        let t = if l > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0)
        } else {
            0.0
        };
        d = d.min((p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy));
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    if inside {
        -d
    } else {
        d
    }
}

fn mm(p: Point) -> [f64; 2] {
    [p.x.mm(), p.y.mm()]
}

/// The test points already marked on the board.
pub fn existing(board: &Board) -> Vec<TestPoint> {
    let g = &board.geometry;
    let mut out = Vec::new();
    for (i, p) in g.pads.iter().enumerate() {
        for (on, top) in [(p.fab.test_top, true), (p.fab.test_bottom, false)] {
            if let (true, Some(net)) = (on, p.net) {
                out.push(TestPoint {
                    net,
                    obj: Obj::Pad(i),
                    top,
                    at: p.at,
                    size: Length(p.size_x.0.min(p.size_y.0)),
                });
            }
        }
    }
    for (i, v) in g.vias.iter().enumerate() {
        for (on, top) in [(v.fab.test_top, true), (v.fab.test_bottom, false)] {
            if let (true, Some(net)) = (on, v.net) {
                out.push(TestPoint {
                    net,
                    obj: Obj::Via(i),
                    top,
                    at: v.at,
                    size: v.diameter,
                });
            }
        }
    }
    out
}

/// Picks a probe point for each net with two or more pads.
pub fn assign(board: &Board, goal: &TestGoal) -> TestReport {
    let g = &board.geometry;
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let last = n - 1;
    let sides: &[bool] = match goal.sides {
        Sides::Top => &[true],
        Sides::Bottom => &[false],
        Sides::Both => &[false, true],
    };
    // Nets that need a probe.
    let mut pads_on: HashMap<u32, usize> = HashMap::new();
    for p in &g.pads {
        if let Some(net) = p.net {
            *pads_on.entry(net).or_default() += 1;
        }
    }
    let mut nets: Vec<u32> = pads_on
        .iter()
        .filter(|(_, &c)| c >= 2)
        .map(|(&n, _)| n)
        .collect();
    nets.sort_unstable();
    let outline: Vec<[f64; 2]> = g.outline.iter().map(|p| mm(*p)).collect();
    let bodies = part_bodies(board);
    let body_rings: Vec<(bool, [f64; 4], Vec<[f64; 2]>)> = bodies
        .iter()
        .map(|b| {
            let xs = b.corners.iter().map(|c| c[0]);
            let ys = b.corners.iter().map(|c| c[1]);
            let bb = [
                xs.clone().fold(f64::MAX, f64::min),
                ys.clone().fold(f64::MAX, f64::min),
                xs.fold(f64::MIN, f64::max),
                ys.fold(f64::MIN, f64::max),
            ];
            (!b.bottom, bb, b.corners.to_vec())
        })
        .collect();
    let (min_size, clear, edge, spacing) = (
        goal.min_size.mm(),
        goal.part_clearance.mm(),
        goal.edge.mm(),
        goal.spacing.mm(),
    );
    // How far each net's best candidate got: 0 none on the probed side,
    // 1 too small or masked, 2 under a part or near the edge.
    let mut stage: HashMap<u32, u8> = HashMap::new();
    let mut cands: HashMap<u32, Vec<Cand>> = HashMap::new();
    let mut consider =
        |net: u32, obj: Obj, top: bool, at: [f64; 2], size: f64, masked: bool, rank: u8| {
            let s = stage.entry(net).or_insert(0);
            *s = (*s).max(1);
            if size + 1e-9 < min_size || (masked && !goal.tented) {
                return;
            }
            *s = (*s).max(2);
            if outline.len() >= 3 && -signed_dist(&outline, at) < edge + size / 2.0 {
                return;
            }
            let r = clear + size / 2.0;
            let blocked = body_rings.iter().any(|(on_top, bb, ring)| {
                *on_top == top
                    && at[0] > bb[0] - r
                    && at[0] < bb[2] + r
                    && at[1] > bb[1] - r
                    && at[1] < bb[3] + r
                    && signed_dist(ring, at) < r
            });
            if blocked {
                return;
            }
            cands.entry(net).or_default().push(Cand {
                obj,
                top,
                at,
                size,
                rank,
            });
        };
    // Nets with surface pads left out by the goal.
    let mut surface_only: std::collections::HashSet<u32> = Default::default();
    for (i, p) in g.pads.iter().enumerate() {
        let Some(net) = p.net else { continue };
        let tp_part = p
            .component
            .and_then(|c| board.components.get(c as usize))
            .is_some_and(|c| bodiless(&c.designator));
        let (on_top, on_bottom, th) = match p.layer {
            LayerRef::AllCopper => (true, true, true),
            LayerRef::Copper(0) => (true, false, false),
            LayerRef::Copper(l) if l == last => (false, true, false),
            _ => (false, false, false),
        };
        let allowed = tp_part || (th && goal.th_pads) || (!th && goal.smd_pads);
        if !allowed {
            let reachable = sides.iter().any(|&t| (t && on_top) || (!t && on_bottom));
            if !th && reachable {
                surface_only.insert(net);
            }
            continue;
        }
        let rank = if tp_part {
            0
        } else if th {
            2
        } else {
            3
        };
        let size = p.size_x.mm().min(p.size_y.mm());
        for &top in sides {
            if (top && on_top) || (!top && on_bottom) {
                let masked = if top {
                    p.fab.tent_top
                } else {
                    p.fab.tent_bottom
                };
                consider(net, Obj::Pad(i), top, mm(p.at), size, masked, rank);
            }
        }
    }
    if goal.vias {
        for (i, v) in g.vias.iter().enumerate() {
            let Some(net) = v.net else { continue };
            let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
            for &top in sides {
                if (top && lo == 0) || (!top && hi == last) {
                    let masked = if top {
                        v.fab.tent_top
                    } else {
                        v.fab.tent_bottom
                    };
                    consider(net, Obj::Via(i), top, mm(v.at), v.diameter.mm(), masked, 1);
                }
            }
        }
    }
    // Probes placed so far, by side, in a grid of `spacing` cells.
    let cell = spacing.max(0.1);
    let key = |p: [f64; 2]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut grid: HashMap<(bool, i64, i64), Vec<[f64; 2]>> = HashMap::new();
    let room = |grid: &HashMap<(bool, i64, i64), Vec<[f64; 2]>>, top: bool, p: [f64; 2]| {
        let (x, y) = key(p);
        (x - 1..=x + 1).all(|i| {
            (y - 1..=y + 1).all(|j| {
                grid.get(&(top, i, j)).is_none_or(|v| {
                    v.iter()
                        .all(|q| (q[0] - p[0]).hypot(q[1] - p[1]) >= spacing - 1e-9)
                })
            })
        })
    };
    let mut rep = TestReport {
        nets: nets.len(),
        ..Default::default()
    };
    let mut done: std::collections::HashSet<u32> = Default::default();
    if goal.keep {
        for t in existing(board) {
            if sides.contains(&t.top) || goal.sides == Sides::Both {
                grid.entry((t.top, key(mm(t.at)).0, key(mm(t.at)).1))
                    .or_default()
                    .push(mm(t.at));
                done.insert(t.net);
                rep.points.push(t);
            }
        }
    }
    // Hardest nets first: those with the fewest places to probe.
    let mut order: Vec<u32> = nets.iter().copied().filter(|n| !done.contains(n)).collect();
    order.sort_by_key(|n| (cands.get(n).map_or(0, Vec::len), *n));
    for net in order {
        let list = cands.get_mut(&net);
        let Some(list) = list.filter(|l| !l.is_empty()) else {
            rep.missing.push((
                net,
                match stage.get(&net).copied().unwrap_or(0) {
                    0 if surface_only.contains(&net) && !goal.smd_pads => {
                        "only surface pads of parts (not used unless allowed)"
                    }
                    0 => "no via or pad on the probed side",
                    1 => "its vias and pads are too small or covered by mask",
                    2 => "every via and pad is under a part or near the edge",
                    _ => "no via or pad on the probed side",
                },
            ));
            continue;
        };
        list.sort_by(|a, b| {
            (a.rank, !matches!(goal.sides, Sides::Both) || a.top)
                .cmp(&(b.rank, !matches!(goal.sides, Sides::Both) || b.top))
                .then(b.size.total_cmp(&a.size))
        });
        match list.iter().find(|c| room(&grid, c.top, c.at)) {
            Some(c) => {
                let (x, y) = key(c.at);
                grid.entry((c.top, x, y)).or_default().push(c.at);
                rep.points.push(TestPoint {
                    net,
                    obj: c.obj,
                    top: c.top,
                    at: Point {
                        x: Length::from_mm(c.at[0]),
                        y: Length::from_mm(c.at[1]),
                    },
                    size: Length::from_mm(c.size),
                });
            }
            None => rep
                .missing
                .push((net, "no room: too close to other probe points")),
        }
    }
    rep.missing.sort_by_key(|m| m.0);
    rep
}

/// The edit that marks exactly these points (and clears the rest).
pub fn mark(board: &Board, points: &[TestPoint]) -> Option<Edit> {
    let g = &board.geometry;
    let mut want: HashMap<Obj, (bool, bool)> = HashMap::new();
    for t in points {
        let e = want.entry(t.obj).or_default();
        if t.top {
            e.0 = true;
        } else {
            e.1 = true;
        }
    }
    let mut pads = Vec::new();
    for (i, p) in g.pads.iter().enumerate() {
        let (t, b) = want.get(&Obj::Pad(i)).copied().unwrap_or_default();
        if (p.fab.test_top, p.fab.test_bottom) != (t, b) {
            let mut f = p.fab;
            f.test_top = t;
            f.test_bottom = b;
            pads.push((i, p.fab, f));
        }
    }
    let mut vias = Vec::new();
    for (i, v) in g.vias.iter().enumerate() {
        let (t, b) = want.get(&Obj::Via(i)).copied().unwrap_or_default();
        if (v.fab.test_top, v.fab.test_bottom) != (t, b) {
            let mut f = v.fab;
            f.test_top = t;
            f.test_bottom = b;
            // A probed via needs the mask open on that side.
            if t {
                f.tent_top = false;
            }
            if b {
                f.tent_bottom = false;
            }
            vias.push((i, v.fab, f));
        }
    }
    (!pads.is_empty() || !vias.is_empty()).then_some(Edit::Fab { pads, vias })
}

/// What a probe point is, for reports: "J1-3" or "via".
pub fn label(board: &Board, obj: Obj) -> String {
    match obj {
        Obj::Via(_) => "via".into(),
        Obj::Pad(i) => {
            let p = &board.geometry.pads[i];
            match p.component.and_then(|c| board.components.get(c as usize)) {
                Some(c) => format!("{}-{}", c.designator, p.name),
                None => format!("pad {}", p.name),
            }
        }
    }
}

/// The test point report: one line per probe point, then the nets left
/// without one.
pub fn report_csv(board: &Board, points: &[TestPoint], missing: &[(u32, &str)]) -> String {
    let name = |n: u32| {
        board
            .nets
            .get(n as usize)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("net{n}"))
    };
    let q = |s: &str| {
        if s.contains([',', '"']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut rows: Vec<&TestPoint> = points.iter().collect();
    rows.sort_by_key(|a| name(a.net));
    let mut o = String::from("Net,Test point,Side,X (mm),Y (mm),Size (mm)\n");
    for t in rows {
        let _ = writeln!(
            o,
            "{},{},{},{:.4},{:.4},{:.3}",
            q(&name(t.net)),
            q(&label(board, t.obj)),
            if t.top { "Top" } else { "Bottom" },
            t.at.x.mm(),
            t.at.y.mm(),
            t.size.mm()
        );
    }
    for (n, why) in missing {
        let _ = writeln!(o, "{},none,,,,{}", q(&name(*n)), q(why));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn pad(net: u32, comp: u32, x: f64, layer: LayerRef, hole: f64) -> Pad {
        Pad {
            layer,
            net: Some(net),
            component: Some(comp),
            name: "1".into(),
            at: Point::mil(x, 500.0),
            size_x: Length::from_mil(60.0),
            size_y: Length::from_mil(60.0),
            hole: Length::from_mil(hole),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    #[test]
    fn picks_one_probe_per_net_clear_of_others() {
        let mut b = Board::default();
        for name in ["TOP", "BOT"] {
            b.stackup.layers.push(StackLayer {
                name: name.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(3000.0, 0.0),
            Point::mil(3000.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        for n in ["A", "B", "C"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        let comp = |d: &str| Component {
            designator: d.into(),
            footprint: "x".into(),
            library_ref: None,
            x: Length(0),
            y: Length(0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        };
        b.components = vec![comp("J1"), comp("J2")];
        // Net A: two connector pins; net B: two pins, one via; net C:
        // only top surface pads.
        b.geometry.pads = vec![
            pad(0, 0, 500.0, LayerRef::AllCopper, 40.0),
            pad(0, 1, 2500.0, LayerRef::AllCopper, 40.0),
            pad(1, 0, 600.0, LayerRef::AllCopper, 40.0),
            pad(1, 1, 2600.0, LayerRef::AllCopper, 40.0),
            pad(2, 0, 1500.0, LayerRef::Copper(0), 0.0),
            pad(2, 1, 1700.0, LayerRef::Copper(0), 0.0),
        ];
        b.geometry.vias.push(Via {
            net: Some(1),
            from_layer: 0,
            to_layer: 1,
            at: Point::mil(1500.0, 800.0),
            diameter: Length::from_mil(30.0),
            hole: Length::from_mil(12.0),
            rings: Vec::new(),
            fab: Fab {
                tent_bottom: true,
                ..Default::default()
            },
        });
        let goal = TestGoal {
            part_clearance: Length(0),
            spacing: Length::from_mil(150.0),
            ..Default::default()
        };
        let r = assign(&b, &goal);
        assert_eq!(r.nets, 3);
        assert_eq!(r.points.len(), 2);
        assert_eq!(r.missing, vec![(2, "no via or pad on the probed side")]);
        assert!((r.coverage() - 66.666).abs() < 0.1);
        // The tented via is not used; probes keep 150 mil apart.
        assert!(r
            .points
            .iter()
            .all(|t| matches!(t.obj, Obj::Pad(_)) && !t.top));
        let (a, c) = (mm(r.points[0].at), mm(r.points[1].at));
        assert!((a[0] - c[0]).hypot(a[1] - c[1]) >= 150.0 * 0.0254 - 1e-6);
        // Marking, then keeping, then clearing.
        let e = mark(&b, &r.points).unwrap();
        e.apply(&mut b);
        assert_eq!(existing(&b).len(), 2);
        let again = assign(&b, &goal);
        assert_eq!(again.points.len(), 2);
        assert!(mark(&b, &again.points).is_none());
        let csv = report_csv(&b, &r.points, &r.missing);
        assert!(csv.contains("J1-1,Bottom") || csv.contains("J2-1,Bottom"));
        assert!(csv.contains("C,none"));
        mark(&b, &[]).unwrap().apply(&mut b);
        assert!(existing(&b).is_empty());
        // From both sides with surface pads allowed, net C's pads are
        // found but sit under the parts' bodies.
        let both = assign(
            &b,
            &TestGoal {
                sides: Sides::Both,
                smd_pads: true,
                ..goal
            },
        );
        assert_eq!(
            both.missing,
            vec![(2, "every via and pad is under a part or near the edge")]
        );
    }
}
