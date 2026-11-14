//! BGA fanout: a short track (a "dogbone") from each ball pad to a via
//! between the balls, or a via in the pad, so the inner layers can route
//! the part. The via of each pad goes to the corner of its grid cell away
//! from the part's centre (or towards it), as Altium's and Allegro's
//! fanout commands place them.

use crate::edit::Edit;
use crate::place::Placement;
use crate::route::{pt, Obstacles, P};
use crate::stitch::{hole_ring, Planes, ViaSpec};
use crate::{Board, LayerRef, Length, Point, Polygon, Track, Via};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Where a pad's via goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A short track to a via between the pads.
    Dogbone,
    /// The via in the pad itself (filled and capped vias).
    InPad,
}

/// Which way the dogbones point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Away from the part's centre, quadrant by quadrant.
    Out,
    /// Towards the centre.
    In,
}

#[derive(Clone, Debug)]
pub struct FanoutGoal {
    /// The parts to fan out, by index.
    pub parts: Vec<usize>,
    pub via: ViaSpec,
    pub style: Style,
    pub direction: Direction,
    /// Neck width (mil); `None` takes the board's width rule, kept no
    /// wider than the pad.
    pub width: Option<f64>,
    pub clearance: f64,
    /// Outer rows left alone, to be routed out on the surface.
    pub skip_rows: usize,
    /// Fan out pads without a net too.
    pub no_net: bool,
}

impl FanoutGoal {
    pub fn of(board: &Board, parts: Vec<usize>) -> FanoutGoal {
        FanoutGoal {
            parts,
            via: ViaSpec::of(board),
            style: Style::Dogbone,
            direction: Direction::Out,
            width: None,
            clearance: crate::route::board_clearance(board),
            skip_rows: 0,
            no_net: false,
        }
    }
}

/// What a fanout placed and what it left out.
#[derive(Clone, Debug, Default)]
pub struct Fanout {
    pub vias: Vec<Via>,
    pub tracks: Vec<Track>,
    /// Clearance holes to cut in other nets' planes the vias pass
    /// through: (polygon, ring).
    pub holes: Vec<(usize, Vec<Point>)>,
    /// Pads fanned out.
    pub pads: usize,
    pub routed: usize,
    pub outer: usize,
    pub crowded: usize,
    pub no_net: usize,
    /// Parts that have no surface pads to fan out.
    pub not_smd: Vec<String>,
    /// The grid pitch found for each part (mil), by designator.
    pub pitch: Vec<(String, f64)>,
}

impl Fanout {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} pads fanned out with {} vias",
            self.pads,
            self.vias.len()
        );
        if !self.pitch.is_empty() {
            let p: Vec<String> = self
                .pitch
                .iter()
                .take(4)
                .map(|(d, p)| format!("{d} pitch {p:.1} mil"))
                .collect();
            s.push_str(&format!(" ({})", p.join(", ")));
        }
        let mut left = Vec::new();
        for (n, what) in [
            (self.routed, "already routed"),
            (self.outer, "in the outer rows"),
            (self.crowded, "no room"),
            (self.no_net, "no net"),
        ] {
            if n > 0 {
                left.push(format!("{n} {what}"));
            }
        }
        if !left.is_empty() {
            s.push_str(&format!("; left out: {}", left.join(", ")));
        }
        if !self.not_smd.is_empty() {
            s.push_str(&format!("; no surface pads on {}", self.not_smd.join(", ")));
        }
        s
    }

    /// The edits that add the vias and tracks, and cut the planes.
    pub fn edits(&self, board: &Board) -> Vec<Edit> {
        let g = &board.geometry;
        let mut out = Vec::new();
        if !self.holes.is_empty() {
            let before = g.polygons.clone();
            let after = cut_planes(&before, &self.holes);
            out.push(Edit::SetPolygons { before, after });
        }
        for (k, v) in self.vias.iter().enumerate() {
            out.push(Edit::InsertVia {
                index: g.vias.len() + k,
                via: v.clone(),
            });
        }
        for (k, t) in self.tracks.iter().enumerate() {
            out.push(Edit::InsertTrack {
                index: g.tracks.len() + k,
                track: t.clone(),
            });
        }
        out
    }
}

/// The polygons with the holes cut, holes that run into each other
/// joined first (a plane under a fine-pitch part turns into one cut-out).
fn cut_planes(polys: &[Polygon], holes: &[(usize, Vec<Point>)]) -> Vec<Polygon> {
    let mut out: Vec<Polygon> = Vec::new();
    for (i, p) in polys.iter().enumerate() {
        let clip: Vec<Vec<P>> = holes
            .iter()
            .filter(|(k, _)| *k == i)
            .map(|(_, r)| r.iter().map(|&q| pt(q)).collect())
            .collect();
        if clip.is_empty() {
            out.push(p.clone());
            continue;
        }
        let none: Vec<Vec<P>> = Vec::new();
        let clip: Vec<Vec<P>> = clip
            .overlay(&none, OverlayRule::Union, FillRule::NonZero)
            .into_iter()
            .flatten()
            .collect();
        let mut subject: Vec<Vec<P>> = vec![p.outline.iter().map(|&q| pt(q)).collect()];
        subject.extend(p.holes.iter().map(|h| h.iter().map(|&q| pt(q)).collect()));
        let shapes = subject.overlay(&clip, OverlayRule::Difference, FillRule::EvenOdd);
        if shapes.is_empty() {
            out.push(p.clone());
            continue;
        }
        for sh in shapes {
            let mut it = sh.into_iter();
            let Some(outer) = it.next() else {
                continue;
            };
            let mut q = p.clone();
            q.outline = outer.into_iter().map(point).collect();
            q.holes = it.map(|h| h.into_iter().map(point).collect()).collect();
            out.push(q);
        }
    }
    out
}

fn point(p: P) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

/// The pads' grid pitch: the most common distance to the nearest
/// neighbour.
fn pitch_of(pts: &[P]) -> Option<f64> {
    if pts.len() < 2 {
        return None;
    }
    let mut d: Vec<f64> = pts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            pts.iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
                .fold(f64::MAX, f64::min)
        })
        .filter(|v| *v > 1e-3)
        .collect();
    d.sort_by(f64::total_cmp);
    d.get(d.len() / 2).copied()
}

/// Fans out the parts of `goal`.
pub fn fanout(board: &Board, goal: &FanoutGoal) -> Fanout {
    let g = &board.geometry;
    let copper = board.stackup.copper_layers().count().max(1) as u8;
    let obstacles = Obstacles::new(board);
    let planes = Planes::new(board);
    let mut out = Fanout::default();
    let width_rule = crate::route::board_width(board);
    // Vias placed so far: centre, net.
    let mut placed: Vec<(P, Option<u32>)> = Vec::new();
    let v = &goal.via;
    let c = goal.clearance;
    for &part in &goal.parts {
        let Some(comp) = board.components.get(part) else {
            continue;
        };
        let Some(at) = Placement::of(board, part) else {
            continue;
        };
        let me = Some(part as u32);
        let outer = if comp.side == crate::Side::Bottom {
            copper - 1
        } else {
            0
        };
        let pads: Vec<usize> = (0..g.pads.len())
            .filter(|&i| {
                let p = &g.pads[i];
                p.component == me && p.hole.0 == 0 && p.layer == LayerRef::Copper(outer)
            })
            .collect();
        if pads.is_empty() {
            out.not_smd.push(comp.designator.clone());
            continue;
        }
        let local: Vec<P> = pads
            .iter()
            .map(|&i| at.to_local(pt(g.pads[i].at)))
            .collect();
        let Some(pitch) = pitch_of(&local) else {
            continue;
        };
        out.pitch.push((comp.designator.clone(), pitch));
        let lo = local.iter().fold([f64::MAX, f64::MAX], |a, p| {
            [a[0].min(p[0]), a[1].min(p[1])]
        });
        let hi = local.iter().fold([f64::MIN, f64::MIN], |a, p| {
            [a[0].max(p[0]), a[1].max(p[1])]
        });
        let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        for (k, &pi) in pads.iter().enumerate() {
            let pad = &g.pads[pi];
            let net = pad.net;
            if net.is_none() && !goal.no_net {
                out.no_net += 1;
                continue;
            }
            let q = local[k];
            let ring = [q[0] - lo[0], hi[0] - q[0], q[1] - lo[1], hi[1] - q[1]]
                .into_iter()
                .fold(f64::MAX, f64::min);
            if ((ring / pitch).round() as usize) < goal.skip_rows {
                out.outer += 1;
                continue;
            }
            let c0 = pt(pad.at);
            let half = pad.size_x.mil().min(pad.size_y.mil()) / 2.0;
            // Routed already: a track of its net ends on it, or a via sits
            // in it.
            let ends_here = |p: P| (p[0] - c0[0]).hypot(p[1] - c0[1]) <= half;
            let routed = net.is_some()
                && (g.tracks.iter().any(|t| {
                    t.net == net
                        && t.layer == LayerRef::Copper(outer)
                        && (ends_here(pt(t.start)) || ends_here(pt(t.end)))
                }) || g.vias.iter().any(|w| w.net == net && ends_here(pt(w.at))));
            if routed {
                out.routed += 1;
                continue;
            }
            let via_at = match goal.style {
                Style::InPad => c0,
                Style::Dogbone => {
                    let side = |d: f64| if d < -1e-6 { -1.0 } else { 1.0 };
                    let mut dir = [side(q[0] - mid[0]), side(q[1] - mid[1])];
                    if goal.direction == Direction::In {
                        dir = [-dir[0], -dir[1]];
                    }
                    at.to_board([q[0] + dir[0] * pitch / 2.0, q[1] + dir[1] * pitch / 2.0])
                }
            };
            let nets = [net];
            let w = goal
                .width
                .unwrap_or(width_rule)
                .min(pad.size_x.mil().min(pad.size_y.mil()));
            let clash_via = obstacles
                .check_span_via(via_at, v.pad, &nets, c, v.from, v.to)
                .is_some();
            let clash_new = placed.iter().any(|(p, n)| {
                let d = (p[0] - via_at[0]).hypot(p[1] - via_at[1]);
                d < v.pad + if *n == net && net.is_some() { 0.0 } else { c }
            });
            let clash_neck = goal.style == Style::Dogbone
                && obstacles
                    .check_track(outer, c0, via_at, w, net, c)
                    .is_some();
            if clash_via || clash_new || clash_neck {
                out.crowded += 1;
                continue;
            }
            // Other nets' copper areas: kept clear of where the via has
            // pads and the neck runs; planes between cut round the via.
            let Some(cuts) = planes.via(via_at, net, v.from, v.to, v.pad / 2.0, c) else {
                out.crowded += 1;
                continue;
            };
            if goal.style == Style::Dogbone && planes.track_hits(c0, via_at, w / 2.0, outer, net, c)
            {
                out.crowded += 1;
                continue;
            }
            let ring = hole_ring(via_at, v.pad / 2.0 + c);
            out.holes
                .extend(cuts.into_iter().map(|i| (i, ring.clone())));
            placed.push((via_at, net));
            out.vias.push(Via {
                net,
                from_layer: v.from,
                to_layer: v.to,
                at: point(via_at),
                diameter: Length::from_mil(v.pad),
                hole: Length::from_mil(v.drill),
                rings: Vec::new(),
                fab: Default::default(),
            });
            if goal.style == Style::Dogbone {
                out.tracks.push(Track {
                    layer: LayerRef::Copper(outer),
                    net,
                    start: pad.at,
                    end: point(via_at),
                    width: Length::from_mil(w),
                    component: None,
                    text: false,
                });
            }
            out.pads += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    /// A 6 by 6 ball grid at 1 mm pitch, one net per ball, turned 30°.
    fn bga(rot: f64) -> Board {
        let mut b = Board::default();
        for n in ["Top", "In1", "In2", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.components.push(Component {
            designator: "U1".into(),
            footprint: "BGA36".into(),
            library_ref: None,
            x: Length::from_mil(1000.0),
            y: Length::from_mil(1000.0),
            rotation_deg: rot,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        let at = Placement::of(&b, 0).unwrap();
        let p = 39.37;
        for i in 0..6 {
            for j in 0..6 {
                let n = b.nets.len() as u32;
                b.nets.push(Net {
                    name: format!("N{i}{j}"),
                    pieces: 1,
                    ..Default::default()
                });
                let q = at.to_board([(i as f64 - 2.5) * p, (j as f64 - 2.5) * p]);
                b.geometry.pads.push(Pad {
                    layer: LayerRef::Copper(0),
                    net: Some(n),
                    component: Some(0),
                    name: format!("{i}{j}"),
                    at: point(q),
                    size_x: Length::from_mil(16.0),
                    size_y: Length::from_mil(16.0),
                    hole: Length(0),
                    shape: PadShape::Round,
                    rotation_deg: rot,
                    outline: Vec::new(),
                    stack: Vec::new(),
                    fab: Default::default(),
                });
            }
        }
        b.rules.push(Rule {
            name: "c".into(),
            kind: "Clearance".into(),
            enabled: true,
            priority: 1,
            scope: vec!["All".into(), "All".into()],
            params: [("GAP".to_string(), "4mil".to_string())].into(),
        });
        b
    }

    fn run(b: &Board, e: &[Edit]) -> Board {
        let mut c = b.clone();
        for x in e {
            x.apply(&mut c);
        }
        c
    }

    #[test]
    fn fans_out_every_ball_clear_of_the_rest() {
        for rot in [0.0, 30.0] {
            let b = bga(rot);
            let mut goal = FanoutGoal::of(&b, vec![0]);
            goal.via = ViaSpec {
                drill: 8.0,
                pad: 18.0,
                from: 0,
                to: 3,
            };
            goal.width = Some(5.0);
            let f = fanout(&b, &goal);
            assert_eq!(f.pads, 36, "{}", f.summary());
            assert!((f.pitch[0].1 - 39.37).abs() < 0.01);
            let after = run(&b, &f.edits(&b));
            let v = drc::check(&after, 100);
            assert!(v.is_empty(), "{rot}: {v:?}");
            // A second run finds every ball routed.
            let again = fanout(&after, &goal);
            assert_eq!(again.pads, 0);
            assert_eq!(again.routed, 36);
            // The corner ball's via sits out beyond the corner.
            let corner = at_board(&b, [-2.5 * 39.37, -2.5 * 39.37]);
            let want = at_board(&b, [-3.0 * 39.37, -3.0 * 39.37]);
            let t = f
                .tracks
                .iter()
                .find(|t| (pt(t.start)[0] - corner[0]).hypot(pt(t.start)[1] - corner[1]) < 0.01)
                .unwrap();
            assert!((pt(t.end)[0] - want[0]).hypot(pt(t.end)[1] - want[1]) < 0.01);
        }
    }

    fn at_board(b: &Board, p: P) -> P {
        Placement::of(b, 0).unwrap().to_board(p)
    }

    #[test]
    fn outer_rows_in_pad_and_crowding() {
        let b = bga(0.0);
        let mut goal = FanoutGoal::of(&b, vec![0]);
        goal.skip_rows = 2;
        let f = fanout(&b, &goal);
        // 6x6: the outer two rings hold 20 + 12 balls, 4 are left.
        assert_eq!(f.outer, 32);
        assert_eq!(f.pads, 4);
        // A via too big for the gaps fits only out past the four corners.
        goal.skip_rows = 0;
        goal.via.pad = 40.0;
        let f = fanout(&b, &goal);
        assert_eq!(f.pads, 4);
        assert_eq!(f.crowded, 32);
        goal.via.pad = 12.0;
        goal.style = Style::InPad;
        let f = fanout(&b, &goal);
        assert_eq!(f.pads, 36);
        assert!(f.tracks.is_empty());
    }

    fn area(b: &mut Board, layer: u8, net: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
        let n = b.nets.len() as u32;
        b.nets.push(Net {
            name: net.into(),
            pieces: 1,
            ..Default::default()
        });
        b.geometry.polygons.push(Polygon {
            layer: LayerRef::Copper(layer),
            net: Some(n),
            outline: vec![
                Point::mil(x0, y0),
                Point::mil(x1, y0),
                Point::mil(x1, y1),
                Point::mil(x0, y1),
            ],
            holes: Vec::new(),
            pour: None,
            component: None,
            teardrop: false,
        });
    }

    #[test]
    fn keeps_clear_of_copper_areas_and_cuts_planes() {
        let mut b = bga(0.0);
        // A ground plane on In1 under the whole part, and a supply area on
        // Bottom over the right half.
        area(&mut b, 1, "GND", 600.0, 600.0, 1400.0, 1400.0);
        area(&mut b, 3, "VCC", 1000.0, 600.0, 1400.0, 1400.0);
        let mut goal = FanoutGoal::of(&b, vec![0]);
        goal.via = ViaSpec {
            drill: 8.0,
            pad: 18.0,
            from: 0,
            to: 3,
        };
        goal.width = Some(5.0);
        let f = fanout(&b, &goal);
        // Vias on the right land in the supply area on Bottom.
        assert_eq!(f.pads, 18, "{}", f.summary());
        assert_eq!(f.crowded, 18);
        assert_eq!(f.holes.len(), 18);
        let after = run(&b, &f.edits(&b));
        assert_eq!(after.geometry.polygons[0].holes.len(), 18);
        let v = drc::check(&after, 100);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn holes_that_meet_are_joined() {
        let mut b = Board::default();
        area(&mut b, 1, "GND", 0.0, 0.0, 100.0, 100.0);
        let h = vec![
            (0, hole_ring([40.0, 50.0], 8.0)),
            (0, hole_ring([52.0, 50.0], 8.0)),
        ];
        let out = cut_planes(&b.geometry.polygons, &h);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].holes.len(), 1);
    }
}
