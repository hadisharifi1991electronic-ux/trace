//! Panelization: copies of a board in a grid, held together by tabs with
//! mouse bites or separated by V-score lines, with rails carrying
//! fiducials and tooling holes.
//!
//! The panel is an ordinary board. Its outline is the panel's edge and
//! `Geometry::slots` are the openings milled through it (the gaps between
//! the boards, less the tabs). Each copy keeps its own nets and parts:
//! copies after the first get `_2`, `_3`... after net names and
//! designators, so the copies are not joined and every part is named once.

use crate::{
    Board, Component, DiffPair, Fab, LayerRef, Length, NetClass, Pad, PadShape, Point, Polygon,
    Track,
};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

type P = [f64; 2];

/// How the boards come apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Separation {
    /// Milled gaps, with tabs across them perforated by small holes.
    MouseBites,
    /// Boards edge to edge, scored on both sides along straight lines.
    VScore,
}

#[derive(Clone, Debug)]
pub struct PanelSpec {
    pub cols: u32,
    pub rows: u32,
    /// Milled gap between boards and to the rails (mil); not used with
    /// V-score.
    pub gap: f64,
    /// Width of the rails above and below the boards (mil); 0 for none.
    pub rail: f64,
    pub separation: Separation,
    /// Tabs on each edge that faces another board or a rail; 0 picks one
    /// per two inches of edge, one to four.
    pub tabs: u32,
    pub tab_width: f64,
    /// Mouse bite hole diameter and pitch (mil).
    pub bite_hole: f64,
    pub bite_pitch: f64,
    /// Three fiducials and four tooling holes on the rails.
    pub fiducials: bool,
    pub tooling: bool,
}

impl Default for PanelSpec {
    fn default() -> Self {
        let mm = |v: f64| v / 0.0254;
        PanelSpec {
            cols: 2,
            rows: 2,
            gap: mm(2.0),
            rail: mm(5.0),
            separation: Separation::MouseBites,
            tabs: 0,
            tab_width: mm(5.0),
            bite_hole: mm(0.5),
            bite_pitch: mm(0.75),
            fiducials: true,
            tooling: true,
        }
    }
}

/// What a panel came out as, besides the board.
#[derive(Clone, Debug, Default)]
pub struct Made {
    /// Panel size, mil.
    pub size: [f64; 2],
    pub tabs: usize,
    pub bites: usize,
    /// The mechanical layer the V-score lines are on.
    pub vscore_layer: Option<u8>,
}

fn mil(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn pnt(p: P) -> Point {
    Point::mil(p[0], p[1])
}

fn ring_area(r: &[P]) -> f64 {
    let mut a = 0.0;
    for k in 0..r.len() {
        let (p, q) = (r[k], r[(k + 1) % r.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<P> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Distance from `from` along `dir` to the first crossing of `ring`, if
/// within `max`.
fn ray(ring: &[P], from: P, dir: P, max: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let den = dir[0] * e[1] - dir[1] * e[0];
        if den.abs() < 1e-12 {
            continue;
        }
        let w = [a[0] - from[0], a[1] - from[1]];
        let t = (w[0] * e[1] - w[1] * e[0]) / den;
        let u = (w[0] * dir[1] - w[1] * dir[0]) / den;
        if t >= -1e-9 && t <= max && (-1e-9..=1.0 + 1e-9).contains(&u) {
            best = Some(best.map_or(t, |b: f64| b.min(t)));
        }
    }
    best
}

/// Makes the panel.
pub fn panelize(board: &Board, spec: &PanelSpec) -> Result<(Board, Made), String> {
    let g = &board.geometry;
    if g.outline.len() < 3 {
        return Err("the board has no outline to panelize".into());
    }
    let (cols, rows) = (spec.cols.max(1), spec.rows.max(1));
    let shape: Vec<P> = g.outline.iter().map(|&p| mil(p)).collect();
    let (mut bx0, mut by0, mut bx1, mut by1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &shape {
        bx0 = bx0.min(p[0]);
        by0 = by0.min(p[1]);
        bx1 = bx1.max(p[0]);
        by1 = by1.max(p[1]);
    }
    let (w, h) = (bx1 - bx0, by1 - by0);
    let bites = spec.separation == Separation::MouseBites;
    let gap = if bites { spec.gap.max(0.0) } else { 0.0 };
    let rail = spec.rail.max(0.0);
    let rail_gap = if rail > 0.0 { gap } else { 0.0 };
    let pw = cols as f64 * w + (cols - 1) as f64 * gap;
    let ph = rows as f64 * h + (rows - 1) as f64 * gap + 2.0 * (rail + rail_gap);
    let y_boards = rail + rail_gap;
    // Where copy (i, j) goes: the offset added to board coordinates.
    let offset = |i: u32, j: u32| -> P {
        [
            i as f64 * (w + gap) - bx0,
            y_boards + j as f64 * (h + gap) - by0,
        ]
    };

    let mut out = Board {
        stackup: board.stackup.clone(),
        rules: board.rules.clone(),
        ..Default::default()
    };
    out.geometry.per_layer_copper = g.per_layer_copper;
    out.geometry.models = g.models.clone();
    let copies = (cols * rows) as usize;
    let suffix = |k: usize| {
        if k == 0 {
            String::new()
        } else {
            format!("_{}", k + 1)
        }
    };
    let nn = board.nets.len() as u32;
    let nc = board.components.len() as u32;
    let np = g.pours.len() as u32;
    let mut made = Made {
        size: [pw, ph],
        ..Default::default()
    };
    for j in 0..rows {
        for i in 0..cols {
            let k = (j * cols + i) as usize;
            let o = offset(i, j);
            let mv = |p: Point| pnt([p.x.mil() + o[0], p.y.mil() + o[1]]);
            let net = |n: Option<u32>| n.map(|n| n + k as u32 * nn);
            let part = |c: Option<u32>| c.map(|c| c + k as u32 * nc);
            let sx = suffix(k);
            for n in &board.nets {
                let mut n = n.clone();
                n.name.push_str(&sx);
                out.nets.push(n);
            }
            for c in &board.components {
                out.components.push(Component {
                    designator: format!("{}{sx}", c.designator),
                    x: Length::from_mil(c.x.mil() + o[0]),
                    y: Length::from_mil(c.y.mil() + o[1]),
                    ..c.clone()
                });
            }
            for d in &board.diff_pairs {
                out.diff_pairs.push(DiffPair {
                    name: format!("{}{sx}", d.name),
                    positive: format!("{}{sx}", d.positive),
                    negative: format!("{}{sx}", d.negative),
                });
            }
            let og = &mut out.geometry;
            for t in &g.tracks {
                og.tracks.push(Track {
                    start: mv(t.start),
                    end: mv(t.end),
                    net: net(t.net),
                    component: part(t.component),
                    ..t.clone()
                });
            }
            for a in &g.arcs {
                let mut a = a.clone();
                a.center = mv(a.center);
                a.net = net(a.net);
                a.component = part(a.component);
                og.arcs.push(a);
            }
            for v in &g.vias {
                let mut v = v.clone();
                v.at = mv(v.at);
                v.net = net(v.net);
                og.vias.push(v);
            }
            for p in &g.pads {
                let mut p = p.clone();
                p.at = mv(p.at);
                p.net = net(p.net);
                p.component = part(p.component);
                p.outline.iter_mut().for_each(|q| *q = mv(*q));
                for l in &mut p.stack {
                    l.outline.iter_mut().for_each(|q| *q = mv(*q));
                }
                og.pads.push(p);
            }
            let poly = |p: &Polygon| {
                let mut p = p.clone();
                p.outline.iter_mut().for_each(|q| *q = mv(*q));
                for h in &mut p.holes {
                    h.iter_mut().for_each(|q| *q = mv(*q));
                }
                p.net = net(p.net);
                p.component = part(p.component);
                p.pour = p.pour.map(|i| i + k as u32 * np);
                p
            };
            og.polygons.extend(g.polygons.iter().map(poly));
            og.areas.extend(g.areas.iter().map(poly));
            og.cutouts.extend(g.cutouts.iter().map(poly));
            for p in &g.pours {
                let mut p = p.clone();
                p.outline.iter_mut().for_each(|q| *q = mv(*q));
                p.net = net(p.net);
                og.pours.push(p);
            }
            for b in &g.bodies {
                let mut b = b.clone();
                b.at = mv(b.at);
                b.component = part(b.component);
                og.bodies.push(b);
            }
            // Boards inside a panel keep their own slots.
            for s in &g.slots {
                og.slots.push(s.iter().map(|&q| mv(q)).collect());
            }
        }
    }
    let names = |list: &[NetClass]| -> Vec<NetClass> {
        list.iter()
            .map(|c| NetClass {
                name: c.name.clone(),
                members: (0..copies)
                    .flat_map(|k| c.members.iter().map(move |m| format!("{m}{}", suffix(k))))
                    .collect(),
            })
            .collect()
    };
    out.net_classes = names(&board.net_classes);
    out.diff_pair_classes = names(&board.diff_pair_classes);
    out.component_classes = names(&board.component_classes);

    // The panel's material: the frame, less the gaps between the boards
    // and the rails, which are milled out except where tabs bridge them.
    let frame = rect(0.0, 0.0, pw, ph);
    let mut keep: Vec<Vec<P>> = Vec::new();
    let mut place: Vec<(u32, u32, Vec<P>)> = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            let o = offset(i, j);
            let mut r: Vec<P> = shape.iter().map(|p| [p[0] + o[0], p[1] + o[1]]).collect();
            if ring_area(&r) < 0.0 {
                r.reverse();
            }
            keep.push(r.clone());
            place.push((i, j, r));
        }
    }
    let mut holes: Vec<P> = Vec::new();
    if bites && gap > 0.0 {
        let per_edge = |len: f64| -> u32 {
            if spec.tabs > 0 {
                spec.tabs
            } else {
                ((len / 2000.0).round() as u32).clamp(1, 4)
            }
        };
        let tw = spec.tab_width.min(w.min(h) / 2.0).max(spec.bite_pitch);
        // Tabs across the gap above each board (to the next row or the top
        // rail), below the first row, and right of each board.
        let mut tab = |ring_a: Option<&Vec<P>>,
                       ring_b: Option<&Vec<P>>,
                       at: f64,
                       edge: f64,
                       vertical: bool| {
            // `edge` is where the gap starts (board a's box), the gap runs
            // +gap across it; `at` is the tab centre along the edge.
            let along = |c: f64, d: f64| if vertical { [at + d, c] } else { [c, at + d] };
            let into_a = ring_a
                .and_then(|r| {
                    ray(
                        r,
                        along(edge, 0.0),
                        if vertical { [0.0, -1.0] } else { [-1.0, 0.0] },
                        w.max(h),
                    )
                })
                .unwrap_or(0.0);
            let into_b = ring_b
                .and_then(|r| {
                    ray(
                        r,
                        along(edge + gap, 0.0),
                        if vertical { [0.0, 1.0] } else { [1.0, 0.0] },
                        w.max(h),
                    )
                })
                .unwrap_or(0.0);
            let (c0, c1) = (edge - into_a - 1.0, edge + gap + into_b + 1.0);
            let (d0, d1) = (at - tw / 2.0, at + tw / 2.0);
            keep.push(if vertical {
                rect(d0, c0, d1, c1)
            } else {
                rect(c0, d0, c1, d1)
            });
            made.tabs += 1;
            // Mouse bites along each end of the tab where it meets a board.
            let n = ((tw - spec.bite_hole) / spec.bite_pitch).floor().max(0.0) as i32 + 1;
            let span = (n - 1) as f64 * spec.bite_pitch;
            for (c, present) in [
                (edge - into_a, ring_a.is_some()),
                (edge + gap + into_b, ring_b.is_some()),
            ] {
                if !present {
                    continue;
                }
                for m in 0..n {
                    let d = -span / 2.0 + m as f64 * spec.bite_pitch;
                    holes.push(along(c, d));
                }
            }
        };
        for (i, j, r) in &place {
            let o = offset(*i, *j);
            let (x0, y0, x1, y1) = (bx0 + o[0], by0 + o[1], bx1 + o[0], by1 + o[1]);
            let n = per_edge(w);
            let above = place
                .iter()
                .find(|q| q.0 == *i && q.1 == j + 1)
                .map(|q| &q.2);
            if above.is_some() || rail > 0.0 {
                for t in 0..n {
                    let at = x0 + w * (t as f64 + 0.5) / n as f64;
                    tab(Some(r), above, at, y1, true);
                }
            }
            if *j == 0 && rail > 0.0 {
                for t in 0..n {
                    let at = x0 + w * (t as f64 + 0.5) / n as f64;
                    tab(None, Some(r), at, y0 - gap, true);
                }
            }
            let n = per_edge(h);
            if let Some(right) = place
                .iter()
                .find(|q| q.0 == i + 1 && q.1 == *j)
                .map(|q| &q.2)
            {
                for t in 0..n {
                    let at = y0 + h * (t as f64 + 0.5) / n as f64;
                    tab(Some(r), Some(right), at, x1, false);
                }
            }
        }
    }
    if rail > 0.0 {
        keep.push(rect(0.0, 0.0, pw, rail));
        keep.push(rect(0.0, ph - rail, pw, ph));
    }
    let open = vec![frame.clone()].overlay(&keep, OverlayRule::Difference, FillRule::NonZero);
    let open: Vec<Vec<P>> = open
        .into_iter()
        .flatten()
        .filter(|r| ring_area(r).abs() > 1.0)
        .collect();
    let material = vec![frame].overlay(&open, OverlayRule::Difference, FillRule::NonZero);
    let mut shapes: Vec<Vec<Vec<P>>> = material
        .into_iter()
        .filter(|s| !s.is_empty() && ring_area(&s[0]).abs() > 1.0)
        .collect();
    shapes.sort_by(|a, b| ring_area(&b[0]).abs().total_cmp(&ring_area(&a[0]).abs()));
    let mut it = shapes.into_iter();
    let main = it.next().ok_or("the panel came out empty")?;
    out.geometry.outline = main[0].iter().map(|&p| pnt(p)).collect();
    for ring in main.into_iter().skip(1).chain(it.flatten()) {
        out.geometry
            .slots
            .push(ring.iter().map(|&p| pnt(p)).collect());
    }

    // Mouse bites: unplated holes.
    let hole = |at: P, d: f64| Pad {
        layer: LayerRef::AllCopper,
        net: None,
        component: None,
        name: String::new(),
        at: pnt(at),
        size_x: Length::from_mil(d),
        size_y: Length::from_mil(d),
        hole: Length::from_mil(d),
        shape: PadShape::Round,
        rotation_deg: 0.0,
        outline: Vec::new(),
        stack: Vec::new(),
        fab: Fab {
            unplated: true,
            no_paste: true,
            ..Default::default()
        },
    };
    made.bites = holes.len();
    for at in holes {
        out.geometry.pads.push(hole(at, spec.bite_hole));
    }

    // Rails: tooling holes near the corners, fiducials asymmetric so the
    // panel cannot be loaded turned round.
    let mm = |v: f64| v / 0.0254;
    let copper = board.stackup.copper_layers().count().max(1) as u8;
    if rail >= mm(4.0) && pw > mm(30.0) {
        let (lo, hi) = (rail / 2.0, ph - rail / 2.0);
        if spec.tooling {
            let d = mm(3.2).min(rail - mm(1.0));
            for at in [
                [mm(5.0), lo],
                [pw - mm(5.0), lo],
                [mm(5.0), hi],
                [pw - mm(5.0), hi],
            ] {
                out.geometry.pads.push(hole(at, d));
            }
        }
        if spec.fiducials {
            for at in [[mm(12.0), lo], [pw - mm(12.0), lo], [mm(12.0), hi]] {
                for l in [0, copper - 1] {
                    out.geometry.pads.push(Pad {
                        layer: LayerRef::Copper(l),
                        name: "FID".into(),
                        size_x: Length::from_mil(mm(1.0)),
                        size_y: Length::from_mil(mm(1.0)),
                        hole: Length(0),
                        fab: Fab {
                            mask: Some(Length::from_mil(mm(0.5))),
                            no_paste: true,
                            ..Default::default()
                        },
                        ..hole(at, mm(1.0))
                    });
                }
            }
        }
    }

    // V-score lines on the first unused mechanical layer, across the
    // whole panel at every edge between boards and rails.
    if !bites {
        let used: Vec<u8> = out
            .geometry
            .tracks
            .iter()
            .map(|t| &t.layer)
            .chain(out.geometry.arcs.iter().map(|a| &a.layer))
            .chain(out.geometry.areas.iter().map(|a| &a.layer))
            .filter_map(|l| match l {
                LayerRef::Mechanical(m) => Some(*m),
                _ => None,
            })
            .collect();
        let m = (1..=32u8).find(|m| !used.contains(m)).unwrap_or(32);
        made.vscore_layer = Some(m);
        let line = |a: P, b: P| Track {
            layer: LayerRef::Mechanical(m),
            net: None,
            start: pnt(a),
            end: pnt(b),
            width: Length::from_mil(4.0),
            component: None,
            text: false,
        };
        for i in 1..cols {
            let x = i as f64 * w;
            out.geometry.tracks.push(line([x, 0.0], [x, ph]));
        }
        let mut ys: Vec<f64> = (1..rows).map(|j| y_boards + j as f64 * h).collect();
        if rail > 0.0 {
            ys.push(rail);
            ys.push(ph - rail);
        }
        for y in ys {
            out.geometry.tracks.push(line([0.0, y], [pw, y]));
        }
    }

    out.panel = Some(crate::PanelOf {
        copies: copies as u32,
        nets: nn,
        parts: nc,
    });
    out.stats = board.stats.clone();
    out.stats.tracks = out.geometry.tracks.len();
    out.stats.vias = out.geometry.vias.len();
    out.stats.pads = out.geometry.pads.len();
    out.stats.polygons = board.stats.polygons * copies;
    crate::connectivity::analyse(&mut out);
    Ok((out, made))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["Top", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for n in ["GND", "SIG"] {
            b.nets.push(Net {
                name: n.into(),
                pieces: 1,
                ..Default::default()
            });
        }
        b.net_classes.push(NetClass {
            name: "Power".into(),
            members: vec!["GND".into()],
        });
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "0603".into(),
            library_ref: None,
            x: Length::from_mil(500.0),
            y: Length::from_mil(500.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        for (x, net) in [(470.0, 0), (530.0, 1)] {
            b.geometry.pads.push(Pad {
                layer: LayerRef::Copper(0),
                net: Some(net),
                component: Some(0),
                name: "1".into(),
                at: Point::mil(x, 500.0),
                size_x: Length::from_mil(30.0),
                size_y: Length::from_mil(30.0),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 0.0,
                outline: vec![],
                stack: vec![],
                fab: Fab::default(),
            });
        }
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: Point::mil(530.0, 500.0),
            end: Point::mil(900.0, 500.0),
            width: Length::from_mil(8.0),
            component: None,
            text: false,
        });
        // A 1000 x 800 mil board starting at (100, 200).
        b.geometry.outline = [
            (100.0, 200.0),
            (1100.0, 200.0),
            (1100.0, 1000.0),
            (100.0, 1000.0),
        ]
        .iter()
        .map(|&(x, y)| Point::mil(x, y))
        .collect();
        b
    }

    #[test]
    fn mouse_bite_panel() {
        let b = board();
        let spec = PanelSpec {
            cols: 3,
            rows: 2,
            gap: 100.0,
            rail: 200.0,
            tabs: 1,
            tab_width: 200.0,
            bite_hole: 20.0,
            bite_pitch: 30.0,
            ..Default::default()
        };
        let (p, made) = panelize(&b, &spec).unwrap();
        // 3 x 1000 + 2 x 100 wide; 2 x 800 + 100 + 2 x (200 + 100) high.
        assert_eq!(made.size, [3200.0, 2300.0]);
        assert_eq!(p.components.len(), 6);
        assert_eq!(p.components[1].designator, "R1_2");
        assert_eq!(p.nets.len(), 12);
        assert_eq!(p.nets[3].name, "SIG_2");
        assert_eq!(p.net_classes[0].members.len(), 6);
        // Copy 2 (column 1, row 0) is 1100 mil right, and starts 300 up.
        let t = &p.geometry.tracks[1];
        assert_eq!(t.net, Some(3));
        assert!((t.start.x.mil() - (530.0 - 100.0 + 1100.0)).abs() < 1e-6);
        assert!((t.start.y.mil() - (500.0 - 200.0 + 300.0)).abs() < 1e-6);
        // Tabs: up from each board (6), down to the rail from row 0 (3),
        // right between columns (2 per row).
        assert_eq!(made.tabs, 13);
        // Each tab end at a board: 200 mil wide holds 7 holes at 30 mil.
        // Ends: row 0 up (3 x 2), row 1 up to the rail (3), rail up (3),
        // between columns (4 x 2).
        assert_eq!(made.bites, 20 * 7);
        // Tooling holes and fiducials on both sides.
        let fid = p.geometry.pads.iter().filter(|q| q.name == "FID").count();
        assert_eq!(fid, 6);
        let npth = p.geometry.pads.iter().filter(|q| q.fab.unplated).count();
        assert_eq!(npth, made.bites + 4);
        // The copies stay apart: no net has pieces from two boards.
        assert!(p.nets.iter().all(|n| n.pieces <= 1), "{:?}", p.nets);
        assert_eq!(p.panel.map(|x| x.base("SIG_3", 5, 2)), Some("SIG"));
        assert_eq!(p.panel.map(|x| x.base("R1_2", 1, 1)), Some("R1"));
        // The gaps run out at the panel's sides, so they shape the outline;
        // material is inside it and outside every slot.
        let outline: Vec<P> = p.geometry.outline.iter().map(|&q| mil(q)).collect();
        let slots: Vec<Vec<P>> = p
            .geometry
            .slots
            .iter()
            .map(|s| s.iter().map(|&q| mil(q)).collect())
            .collect();
        let material = |x: f64, y: f64| {
            crate::route::inside([x, y], &outline)
                && !slots.iter().any(|s| crate::route::inside([x, y], s))
        };
        assert!(!material(1050.0, 400.0), "between columns, off the tab");
        assert!(material(1050.0, 700.0), "on the tab between columns");
        assert!(material(500.0, 700.0), "on a board");
        assert!(material(1600.0, 100.0), "on the bottom rail");
        assert!(!material(1300.0, 250.0), "between the rail and the boards");
        assert!(material(550.0, 250.0), "on a tab to the rail");
    }

    #[test]
    fn vscore_panel() {
        let b = board();
        let spec = PanelSpec {
            cols: 2,
            rows: 3,
            rail: 200.0,
            separation: Separation::VScore,
            ..Default::default()
        };
        let (p, made) = panelize(&b, &spec).unwrap();
        assert_eq!(made.size, [2000.0, 2800.0]);
        assert_eq!(made.tabs, 0);
        assert!(p.geometry.slots.is_empty(), "{:?}", p.geometry.slots);
        let m = made.vscore_layer.unwrap();
        let lines = p
            .geometry
            .tracks
            .iter()
            .filter(|t| t.layer == LayerRef::Mechanical(m))
            .count();
        // One between the columns, two between rows, two at the rails.
        assert_eq!(lines, 5);
    }
}
