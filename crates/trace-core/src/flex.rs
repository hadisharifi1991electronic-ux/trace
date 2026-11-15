//! Rigid-flex: zones of the board with their own layer stack, bend
//! areas, and the checks a flex circuit needs.
//!
//! A zone is an area of the board that has only some of the stack-up's
//! layers (a flex zone keeps the polyimide core and its copper, a rigid
//! zone has them all). Bends sit in flex zones with an inner radius and
//! an angle. The checks follow IPC-2223: no vias or parts in flex (unless
//! stiffened) and none near the rigid edge, traces crossing a bend at
//! right angles and without corners on it, no copper on layers a zone
//! does not have, no solid copper or stacked traces in a bend, and a bend
//! radius of at least so many times the flex's thickness.

use crate::{Board, LayerKind, LayerRef, Length, Point};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// An area of the board with its own layers.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Zone {
    pub name: String,
    /// Flex (bends allowed) rather than rigid.
    pub flex: bool,
    /// Closed ring.
    pub outline: Vec<Point>,
    /// Stack-up layer names present in the zone, top to bottom.
    pub layers: Vec<String>,
    /// Stiffened: parts may sit on it although it is flex.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stiffener: bool,
}

/// Where the flex folds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Bend {
    pub name: String,
    /// Closed ring of the bend area.
    pub area: Vec<Point>,
    /// Inner bend radius.
    pub radius: Length,
    /// Fold angle, degrees.
    pub angle_deg: f64,
    /// Flexes in use (hinges, cables), not only bent once to fit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dynamic: bool,
    /// Direction of the fold line, degrees (none: square to the tracks
    /// that cross the bend, or along the area's longest side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_deg: Option<f64>,
}

/// Limits for the checks.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Vias and pads this far from a rigid-flex edge at least, mil.
    pub edge_keepout: f64,
    /// Largest angle between a track and the bend's normal, degrees.
    pub cross_angle: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            edge_keepout: 30.0,
            cross_angle: 10.0,
        }
    }
}

/// A flex problem.
#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: &'static str,
    pub message: String,
    pub at: [f64; 2],
}

fn pt(p: &Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

/// Point in a closed ring (even-odd).
pub fn inside(ring: &[Point], p: [f64; 2]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut c = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pt(&ring[i]), pt(&ring[j]));
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
        j = i;
    }
    c
}

/// Distance from a point to a closed ring's edge, mil.
pub fn edge_distance(ring: &[Point], p: [f64; 2]) -> f64 {
    let n = ring.len();
    let mut d = f64::MAX;
    for i in 0..n {
        let (a, b) = (pt(&ring[i]), pt(&ring[(i + 1) % n]));
        d = d.min(seg_dist(a, b, p));
    }
    d
}

fn seg_dist(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// The bend's fold line direction, unit vector: as set, else square to
/// the way most track length runs through the bend, else along the
/// area's longest side.
pub fn fold_axis(board: &Board, b: &Bend) -> Option<[f64; 2]> {
    if let Some(a) = b.axis_deg {
        let r = a.to_radians();
        return Some([r.cos(), r.sin()]);
    }
    // Track length by direction (mod 180, 5 degree bins).
    let mut bins = [0.0f64; 36];
    for t in &board.geometry.tracks {
        if t.text || t.component.is_some() || !matches!(t.layer, LayerRef::Copper(_)) {
            continue;
        }
        let (a, c) = (pt(&t.start), pt(&t.end));
        let mid = [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0];
        if !inside(&b.area, mid) {
            continue;
        }
        let len = (c[0] - a[0]).hypot(c[1] - a[1]);
        let ang = (c[1] - a[1])
            .atan2(c[0] - a[0])
            .to_degrees()
            .rem_euclid(180.0);
        bins[((ang / 5.0) as usize).min(35)] += len;
    }
    if let Some((k, &w)) = bins.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)) {
        if w > 0.0 {
            let r = ((k as f64 + 0.5) * 5.0 + 90.0).to_radians();
            return Some([r.cos(), r.sin()]);
        }
    }
    longest_side(b)
}

fn longest_side(b: &Bend) -> Option<[f64; 2]> {
    let n = b.area.len();
    let mut best = (0.0, None);
    for i in 0..n {
        let (a, c) = (pt(&b.area[i]), pt(&b.area[(i + 1) % n]));
        let l = (c[0] - a[0]).hypot(c[1] - a[1]);
        if l > best.0 {
            best = (l, Some([(c[0] - a[0]) / l, (c[1] - a[1]) / l]));
        }
    }
    best.1
}

/// The zone holding a point (the last listed wins where zones overlap).
pub fn zone_at(board: &Board, p: [f64; 2]) -> Option<usize> {
    board.zones.iter().rposition(|z| inside(&z.outline, p))
}

/// Stack-up of a zone: its layers in board order, with thickness.
pub fn zone_stack<'a>(board: &'a Board, z: &Zone) -> Vec<&'a crate::StackLayer> {
    board
        .stackup
        .layers
        .iter()
        .filter(|l| z.layers.iter().any(|n| n.eq_ignore_ascii_case(&l.name)))
        .collect()
}

/// Thickness of a zone, mil, and its copper layer count.
pub fn zone_thickness(board: &Board, z: &Zone) -> (f64, usize) {
    let st = zone_stack(board, z);
    (
        st.iter().map(|l| l.thickness.mil()).sum(),
        st.iter().filter(|l| l.kind == LayerKind::Copper).count(),
    )
}

/// Least inner bend radius for a flex this thick with this many copper
/// layers (IPC-2223 static: 6x one layer, 12x two, 24x more; dynamic:
/// 100x), mil.
pub fn min_radius(thickness: f64, copper: usize, dynamic: bool) -> f64 {
    let k = if dynamic {
        100.0
    } else {
        match copper {
            0 | 1 => 6.0,
            2 => 12.0,
            _ => 24.0,
        }
    };
    k * thickness
}

/// Copper layer index (0 = top) of each copper stack-up layer name.
fn copper_names(board: &Board) -> Vec<String> {
    board
        .stackup
        .copper_layers()
        .map(|l| l.name.clone())
        .collect()
}

/// A track piece in a bend: bend, layer, ends and width.
type InBend = (usize, u8, [f64; 2], [f64; 2], f64);

/// Every flex problem on the board.
pub fn check(board: &Board, lim: &Limits) -> Vec<Issue> {
    let mut out = Vec::new();
    if board.zones.is_empty() {
        return out;
    }
    let g = &board.geometry;
    let cu = copper_names(board);
    let has_layer = |z: &Zone, l: u8| {
        cu.get(l as usize)
            .is_none_or(|n| z.layers.iter().any(|x| x.eq_ignore_ascii_case(n)))
    };
    let flex_zone = |p: [f64; 2]| zone_at(board, p).filter(|&i| board.zones[i].flex);
    // Edges between a flex and a rigid zone (a flex edge lying on a
    // rigid zone's edge or inside it; the board outline is not one).
    let mut edges: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for z in board.zones.iter().filter(|z| z.flex) {
        let n = z.outline.len();
        for i in 0..n {
            let (a, b) = (pt(&z.outline[i]), pt(&z.outline[(i + 1) % n]));
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let outer = edge_distance(&g.outline, mid) < 2.0;
            let rigid_beside = board.zones.iter().any(|r| {
                !r.flex && (edge_distance(&r.outline, mid) < 2.0 || inside(&r.outline, mid))
            });
            if rigid_beside && !outer {
                edges.push((a, b));
            }
        }
    }
    let boundary = |p: [f64; 2]| -> f64 {
        edges
            .iter()
            .map(|(a, b)| seg_dist(*a, *b, p))
            .fold(f64::MAX, f64::min)
    };
    // Vias.
    for v in &g.vias {
        let p = pt(&v.at);
        if let Some(i) = flex_zone(p) {
            out.push(Issue {
                kind: "via in flex",
                message: format!("via in flex zone {}", board.zones[i].name),
                at: p,
            });
        } else if boundary(p) < lim.edge_keepout {
            out.push(Issue {
                kind: "via near flex edge",
                message: format!(
                    "via {:.0} mil from the rigid-flex edge (keep {:.0})",
                    boundary(p),
                    lim.edge_keepout
                ),
                at: p,
            });
        }
    }
    // Parts.
    for (ci, c) in board.components.iter().enumerate() {
        let pads: Vec<[f64; 2]> = g
            .pads
            .iter()
            .filter(|p| p.component == Some(ci as u32))
            .map(|p| pt(&p.at))
            .collect();
        let p = [c.x.mil(), c.y.mil()];
        let zone = pads.iter().chain([&p]).find_map(|q| flex_zone(*q));
        if let Some(i) = zone {
            if !board.zones[i].stiffener {
                out.push(Issue {
                    kind: "part in flex",
                    message: format!(
                        "{} sits in flex zone {} without a stiffener",
                        c.designator, board.zones[i].name
                    ),
                    at: p,
                });
            }
        }
        if let Some(b) = board
            .bends
            .iter()
            .find(|b| pads.iter().chain([&p]).any(|q| inside(&b.area, *q)))
        {
            out.push(Issue {
                kind: "part in bend",
                message: format!("{} sits in bend {}", c.designator, b.name),
                at: p,
            });
        }
    }
    // Tracks: on layers the zone lacks, crossing bends at an angle, with
    // corners in a bend; stacked on two layers in a bend.
    let mut in_bend: Vec<InBend> = Vec::new();
    let axes: Vec<Option<[f64; 2]>> = board.bends.iter().map(|b| fold_axis(board, b)).collect();
    for (ti, t) in g.tracks.iter().enumerate() {
        let LayerRef::Copper(l) = t.layer else {
            continue;
        };
        if t.text || t.component.is_some() {
            continue;
        }
        let (a, b) = (pt(&t.start), pt(&t.end));
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        for q in [a, mid, b] {
            if let Some(i) = zone_at(board, q) {
                let z = &board.zones[i];
                if !has_layer(z, l) {
                    out.push(Issue {
                        kind: "copper on missing layer",
                        message: format!(
                            "track on {} in zone {}, which does not have that layer",
                            cu.get(l as usize).map_or("?", |s| s.as_str()),
                            z.name
                        ),
                        at: q,
                    });
                    break;
                }
            }
        }
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-6 {
            continue;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        for (bi, bend) in board.bends.iter().enumerate() {
            let ins = [
                inside(&bend.area, a),
                inside(&bend.area, mid),
                inside(&bend.area, b),
            ];
            if !ins.iter().any(|x| *x) {
                continue;
            }
            in_bend.push((ti, l, a, b, t.width.mil()));
            if let Some(ax) = axes[bi] {
                // Crossing the fold line (through the bend's centre along
                // its axis): the track should run across it, square.
                let c = centroid(&bend.area);
                let side = |q: [f64; 2]| ax[0] * (q[1] - c[1]) - ax[1] * (q[0] - c[0]);
                if (side(a) >= 0.0) == (side(b) >= 0.0) {
                    continue;
                }
                let along = (u[0] * ax[0] + u[1] * ax[1]).abs();
                let off = along.asin().to_degrees();
                if off > lim.cross_angle {
                    out.push(Issue {
                        kind: "track not across bend",
                        message: format!(
                            "track crosses bend {} {:.0} deg off square (keep under {:.0})",
                            bend.name, off, lim.cross_angle
                        ),
                        at: mid,
                    });
                }
            }
        }
    }
    // Corners in a bend: two tracks of a net meeting inside it at an angle.
    for (i, x) in in_bend.iter().enumerate() {
        for y in in_bend.iter().skip(i + 1) {
            if x.1 != y.1 {
                continue;
            }
            let (tx, ty) = (&g.tracks[x.0], &g.tracks[y.0]);
            if tx.net != ty.net {
                continue;
            }
            for (p, q) in [(x.2, y.2), (x.2, y.3), (x.3, y.2), (x.3, y.3)] {
                if (p[0] - q[0]).hypot(p[1] - q[1]) > 0.1 {
                    continue;
                }
                if !board.bends.iter().any(|b| inside(&b.area, p)) {
                    continue;
                }
                let d = |s: [f64; 2], e: [f64; 2]| {
                    let l = (e[0] - s[0]).hypot(e[1] - s[1]).max(1e-9);
                    [(e[0] - s[0]) / l, (e[1] - s[1]) / l]
                };
                let (u, v) = (d(x.2, x.3), d(y.2, y.3));
                let cos = (u[0] * v[0] + u[1] * v[1]).abs();
                if cos < 0.999 {
                    out.push(Issue {
                        kind: "corner in bend",
                        message: format!(
                            "track turns {:.0} deg inside a bend",
                            cos.clamp(-1.0, 1.0).acos().to_degrees()
                        ),
                        at: p,
                    });
                }
            }
        }
    }
    // Stacked tracks (I-beam) on neighbouring copper layers in a bend
    // (one issue per track).
    let mut stacked = std::collections::HashSet::new();
    for (i, x) in in_bend.iter().enumerate() {
        if stacked.contains(&x.0) {
            continue;
        }
        for y in in_bend.iter().skip(i + 1) {
            if x.1.abs_diff(y.1) != 1 {
                continue;
            }
            let mx = [(x.2[0] + x.3[0]) / 2.0, (x.2[1] + x.3[1]) / 2.0];
            if seg_dist(y.2, y.3, mx) < (x.4 + y.4) / 2.0 && stacked.insert(x.0) {
                out.push(Issue {
                    kind: "stacked tracks in bend",
                    message:
                        "tracks on neighbouring layers lie over each other in a bend; stagger them"
                            .into(),
                    at: mx,
                });
            }
        }
    }
    // Solid copper in a bend.
    for poly in &g.polygons {
        let LayerRef::Copper(_) = poly.layer else {
            continue;
        };
        for b in &board.bends {
            if poly
                .outline
                .iter()
                .step_by((poly.outline.len() / 64).max(1))
                .any(|p| inside(&b.area, pt(p)))
            {
                out.push(Issue {
                    kind: "solid copper in bend",
                    message: format!("copper pour reaches into bend {}; hatch it there", b.name),
                    at: poly.outline.first().map_or([0.0, 0.0], pt),
                });
                break;
            }
        }
    }
    // Bend radius against the flex thickness.
    for b in &board.bends {
        let c = centroid(&b.area);
        let Some(zi) = zone_at(board, c) else {
            out.push(Issue {
                kind: "bend outside flex",
                message: format!("bend {} is not in a flex zone", b.name),
                at: c,
            });
            continue;
        };
        let z = &board.zones[zi];
        if !z.flex {
            out.push(Issue {
                kind: "bend outside flex",
                message: format!("bend {} is in rigid zone {}", b.name, z.name),
                at: c,
            });
            continue;
        }
        let (t, n) = zone_thickness(board, z);
        let need = min_radius(t, n, b.dynamic);
        if t > 0.0 && b.radius.mil() + 1e-6 < need {
            out.push(Issue {
                kind: "bend radius",
                message: format!(
                    "bend {} radius {:.0} mil is under {:.0} mil ({}x the {:.1} mil flex, {} copper layer(s){})",
                    b.name,
                    b.radius.mil(),
                    need,
                    (need / t).round(),
                    t,
                    n,
                    if b.dynamic { ", dynamic" } else { "" }
                ),
                at: c,
            });
        }
    }
    out
}

/// Centre of a ring's points.
pub fn centroid(r: &[Point]) -> [f64; 2] {
    if r.is_empty() {
        return [0.0, 0.0];
    }
    let n = r.len() as f64;
    [
        r.iter().map(|p| p.x.mil()).sum::<f64>() / n,
        r.iter().map(|p| p.y.mil()).sum::<f64>() / n,
    ]
}

/// A text summary: zones with their stacks, bends, and the checks.
pub fn report(board: &Board, issues: &[Issue]) -> String {
    let mut o = String::new();
    if board.zones.is_empty() {
        let _ = writeln!(o, "No rigid-flex zones on this board.");
        return o;
    }
    let _ = writeln!(o, "Zones ({}):", board.zones.len());
    for z in &board.zones {
        let (t, n) = zone_thickness(board, z);
        let _ = writeln!(
            o,
            "  {:<16} {:<6} {:.1} mil, {} copper layer(s){}: {}",
            z.name,
            if z.flex { "flex" } else { "rigid" },
            t,
            n,
            if z.stiffener { ", stiffened" } else { "" },
            z.layers.join(", ")
        );
    }
    if !board.bends.is_empty() {
        let _ = writeln!(o, "Bends ({}):", board.bends.len());
        for b in &board.bends {
            let need = zone_at(board, centroid(&b.area))
                .map(|i| {
                    let (t, n) = zone_thickness(board, &board.zones[i]);
                    min_radius(t, n, b.dynamic)
                })
                .unwrap_or(0.0);
            let _ = writeln!(
                o,
                "  {:<16} radius {:.0} mil (least {:.0}), {:.0} deg{}",
                b.name,
                b.radius.mil(),
                need,
                b.angle_deg,
                if b.dynamic { ", dynamic" } else { "" }
            );
        }
    }
    let _ = writeln!(o, "Flex checks: {} issue(s)", issues.len());
    let mut kinds: Vec<&str> = issues.iter().map(|i| i.kind).collect();
    kinds.sort();
    kinds.dedup();
    for k in kinds {
        let n = issues.iter().filter(|i| i.kind == k).count();
        let _ = writeln!(o, "  {k}: {n}");
        for i in issues.iter().filter(|i| i.kind == k).take(10) {
            let _ = writeln!(o, "    ({:.0}, {:.0}) {}", i.at[0], i.at[1], i.message);
        }
    }
    o
}

/// A rectangle ring, mil.
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    vec![
        Point::mil(x0, y0),
        Point::mil(x1, y0),
        Point::mil(x1, y1),
        Point::mil(x0, y1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Geometry, StackLayer, Stackup, Track, Via};

    fn layer(name: &str, kind: LayerKind, mil: f64) -> StackLayer {
        StackLayer {
            name: name.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(mil),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        }
    }

    fn board() -> Board {
        let all: Vec<String> = ["TOP", "PP1", "FLEX1", "PI", "FLEX2", "PP2", "BOT"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        Board {
            stackup: Stackup {
                layers: vec![
                    layer("TOP", LayerKind::Copper, 1.4),
                    layer("PP1", LayerKind::Dielectric, 4.0),
                    layer("FLEX1", LayerKind::Copper, 0.7),
                    layer("PI", LayerKind::Dielectric, 2.0),
                    layer("FLEX2", LayerKind::Copper, 0.7),
                    layer("PP2", LayerKind::Dielectric, 4.0),
                    layer("BOT", LayerKind::Copper, 1.4),
                ],
            },
            zones: vec![
                Zone {
                    name: "rigid".into(),
                    flex: false,
                    outline: rect(0.0, 0.0, 1000.0, 1000.0),
                    layers: all,
                    stiffener: false,
                },
                Zone {
                    name: "flex".into(),
                    flex: true,
                    outline: rect(1000.0, 0.0, 2000.0, 1000.0),
                    layers: vec!["FLEX1".into(), "PI".into(), "FLEX2".into()],
                    stiffener: false,
                },
            ],
            bends: vec![Bend {
                name: "B1".into(),
                area: rect(1400.0, 0.0, 1600.0, 1000.0),
                radius: Length::from_mil(20.0),
                angle_deg: 180.0,
                dynamic: false,
                axis_deg: Some(90.0),
            }],
            geometry: Geometry {
                vias: vec![Via {
                    net: Some(0),
                    from_layer: 0,
                    to_layer: 3,
                    at: Point::mil(1200.0, 500.0),
                    diameter: Length::from_mil(20.0),
                    hole: Length::from_mil(10.0),
                    rings: Vec::new(),
                    fab: Default::default(),
                }],
                tracks: vec![
                    // Across the bend at 45 degrees, on FLEX1 (copper 1).
                    Track {
                        layer: LayerRef::Copper(1),
                        net: Some(0),
                        start: Point::mil(1300.0, 300.0),
                        end: Point::mil(1700.0, 700.0),
                        width: Length::from_mil(5.0),
                        component: None,
                        text: false,
                    },
                    // On TOP in the flex zone.
                    Track {
                        layer: LayerRef::Copper(0),
                        net: Some(1),
                        start: Point::mil(1100.0, 100.0),
                        end: Point::mil(1300.0, 100.0),
                        width: Length::from_mil(5.0),
                        component: None,
                        text: false,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_flex_problems() {
        let b = board();
        let issues = check(&b, &Limits::default());
        let has = |k: &str| issues.iter().any(|i| i.kind == k);
        assert!(has("via in flex"), "{}", report(&b, &issues));
        assert!(has("track not across bend"), "{}", report(&b, &issues));
        assert!(has("copper on missing layer"), "{}", report(&b, &issues));
        // 3.4 mil of flex with two copper layers: 12x = 41 mil > 20.
        assert!(has("bend radius"), "{}", report(&b, &issues));
        let (t, n) = zone_thickness(&b, &b.zones[1]);
        assert!((t - 3.4).abs() < 1e-6 && n == 2);
    }

    #[test]
    fn a_tidy_flex_passes() {
        let mut b = board();
        b.geometry.vias.clear();
        b.geometry.tracks = vec![Track {
            layer: LayerRef::Copper(1),
            net: Some(0),
            start: Point::mil(1300.0, 500.0),
            end: Point::mil(1700.0, 500.0),
            width: Length::from_mil(5.0),
            component: None,
            text: false,
        }];
        b.bends[0].radius = Length::from_mil(50.0);
        let issues = check(&b, &Limits::default());
        assert!(issues.is_empty(), "{}", report(&b, &issues));
    }
}
