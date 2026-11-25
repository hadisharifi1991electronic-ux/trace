//! Board cut-outs: a window of a board through every copper layer, with
//! every net in it, solved by the 3D FEM. Traces, arcs, pads and pours sit
//! on their own layers, vias and through-hole pins are barrels with their
//! real nets, each trace that leaves the window is a wave port (a pair
//! leaving side by side is one differential port), and two-pad parts on
//! the outer layers are lumped ports, as in `planar`.

#![allow(clippy::type_complexity, clippy::needless_range_loop)]

use crate::fem3d::{Copper, Fill, Lumped, Mode, Model, Port, Shape, Sheet, Solid, Wall};
use crate::geometry::{LayerRef, Pad, PadShape};
use crate::planar::{part_ports, Part};
use crate::Board;
use std::collections::{BTreeMap, HashMap};

const MIL: f64 = 25.4e-6;

/// Where a trace crosses the window's edge (board mil).
struct Crossing {
    layer: usize,
    net: u32,
    board_net: Option<u32>,
    axis: usize,
    high: bool,
    /// Position along the face (board mil) and the trace's width.
    at: f64,
    w: f64,
}

fn in_poly(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let n = v.len();
    for i in 0..n {
        let (a, b) = (v[i], v[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

/// A window `half` mil about via `via` and the vias beside it (within
/// 80 mil) on its own net and on its differential pair partner's net, as
/// (x0, y0, x1, y1) mil, with how many vias it centres on and whether a
/// partner was found.
pub fn around_via(board: &Board, via: usize, half: f64) -> Option<([f64; 4], usize, bool)> {
    let vias = &board.geometry.vias;
    let first = vias.get(via)?;
    let partner = first.net.and_then(|id| {
        let net = &board.nets.get(id as usize)?.name;
        board
            .diff_pairs
            .iter()
            .cloned()
            .chain(crate::zcheck::name_pairs(board))
            .find_map(|p| {
                if p.positive.eq_ignore_ascii_case(net) {
                    Some(p.negative)
                } else if p.negative.eq_ignore_ascii_case(net) {
                    Some(p.positive)
                } else {
                    None
                }
            })
            .and_then(|n| board.nets.iter().position(|x| x.name == n))
            .map(|n| n as u32)
    });
    let at = |v: &crate::geometry::Via| [v.at.x.mil(), v.at.y.mil()];
    let c0 = at(first);
    let near: Vec<[f64; 2]> = vias
        .iter()
        .filter(|v| {
            (first.net.is_some() && v.net == first.net) || (partner.is_some() && v.net == partner)
        })
        .map(at)
        .filter(|p| (p[0] - c0[0]).hypot(p[1] - c0[1]) < 80.0)
        .chain(first.net.is_none().then_some(c0))
        .collect();
    let n = near.len() as f64;
    let c = [
        near.iter().map(|p| p[0]).sum::<f64>() / n,
        near.iter().map(|p| p[1]).sum::<f64>() / n,
    ];
    Some((
        [c[0] - half, c[1] - half, c[0] + half, c[1] + half],
        near.len(),
        partner.is_some(),
    ))
}

/// The FEM model of the window `win` (x0, y0, x1, y1, mil) through every
/// copper layer, its parts, and notes on what was assumed.
pub fn model(
    board: &Board,
    win: [f64; 4],
    lossy: bool,
    rough_um: f64,
) -> Result<(Model, Vec<Part>, Vec<String>), String> {
    let st = crate::fdtd::stack(&board.stackup);
    let nl = st.sheet.len();
    if nl < 2 || st.slabs.len() + 1 < nl {
        return Err("the stack-up needs its copper layers and the dielectrics between them".into());
    }
    let mut notes = Vec::new();
    let (x0, y0) = (win[0].min(win[2]), win[1].min(win[3]));
    let (x1, y1) = (win[0].max(win[2]), win[1].max(win[3]));
    let (wx, wy) = ((x1 - x0) * MIL, (y1 - y0) * MIL);
    let xy = |x: f64, y: f64| -> [f64; 2] { [(x - x0) * MIL, (y - y0) * MIL] };
    let inwin = |x: f64, y: f64, m: f64| x >= x0 - m && x <= x1 + m && y >= y0 - m && y <= y1 + m;
    // z: the top copper at 0, down through the board.
    let z = |l: usize| -st.sheet[l] * MIL;
    let t_top = st.slabs[0].1 - st.slabs[0].0;
    let t_bot = st.slabs[st.slabs.len() - 1].1 - st.slabs[st.slabs.len() - 1].0;
    let (air_top, air_bot) = (5.0 * t_top * MIL, 5.0 * t_bot * MIL);
    let lo = [0.0, 0.0, z(nl - 1) - air_bot];
    let hi = [wx, wy, z(0) + air_top];
    let mut solids = Vec::new();
    for s in &st.slabs {
        solids.push(Solid {
            shape: Shape::Box {
                lo: [0.0, 0.0, -s.1 * MIL],
                hi: [wx, wy, -s.0 * MIL],
            },
            fill: Fill::Dielectric {
                er: s.2,
                tand: if lossy { s.3 } else { 0.0 },
            },
        });
    }
    // Ground: the net with the most poured copper, and nets named GND/VSS.
    let geo = &board.geometry;
    let mut poured: HashMap<u32, usize> = HashMap::new();
    for p in &geo.polygons {
        if matches!(p.layer, LayerRef::Copper(_)) {
            if let Some(n) = p.net {
                *poured.entry(n).or_insert(0) += p.outline.len();
            }
        }
    }
    let ref_net = poured.into_iter().max_by_key(|x| x.1).map(|x| x.0);
    let is_gnd = |n: Option<u32>| -> bool {
        n.is_some_and(|n| {
            Some(n) == ref_net
                || board.nets.get(n as usize).is_some_and(|x| {
                    let u = x.name.to_ascii_uppercase();
                    u.contains("GND") || u.contains("VSS")
                })
        })
    };
    let mnet = |n: Option<u32>| -> u32 {
        match n {
            _ if is_gnd(n) => 0,
            Some(n) => n + 1,
            None => u32::MAX - 1,
        }
    };
    let name = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map_or("?".to_string(), |x| x.name.clone())
    };
    let sheet = |l: usize, poly: Vec<[f64; 2]>, net: u32| Sheet {
        axis: 2,
        at: z(l),
        poly,
        holes: Vec::new(),
        disc: None,
        net,
    };
    let mut sheets: Vec<Sheet> = Vec::new();
    // Pours by layer: (sheet index, outline in board mil), for antipads
    // and port references.
    let mut pours: Vec<Vec<(usize, Vec<[f64; 2]>)>> = vec![Vec::new(); nl];
    let mut crossings: Vec<Crossing> = Vec::new();
    let mut nets_seen: std::collections::BTreeSet<u32> = Default::default();
    let mut parts_pads: [BTreeMap<u32, Vec<&Pad>>; 2] = Default::default();
    for l in 0..nl {
        let on = |r: &LayerRef| *r == LayerRef::Copper(l as u8) || *r == LayerRef::AllCopper;
        // Tracks, and arcs as chords.
        let mut segs: Vec<([f64; 2], [f64; 2], f64, Option<u32>)> = Vec::new();
        for t in geo.tracks.iter().filter(|t| on(&t.layer) && !t.text) {
            segs.push((
                [t.start.x.mil(), t.start.y.mil()],
                [t.end.x.mil(), t.end.y.mil()],
                t.width.mil(),
                t.net,
            ));
        }
        for a in geo
            .arcs
            .iter()
            .filter(|a| on(&a.layer) && a.component.is_none())
        {
            let (cx, cy, r) = (a.center.x.mil(), a.center.y.mil(), a.radius.mil());
            if !inwin(cx, cy, r + a.width.mil()) {
                continue;
            }
            let mut sweep = a.end_deg - a.start_deg;
            if sweep <= 0.0 {
                sweep += 360.0;
            }
            let n = ((sweep / 15.0).ceil() as usize).max(1);
            let pt = |k: usize| {
                let t = (a.start_deg + sweep * k as f64 / n as f64).to_radians();
                [cx + r * t.cos(), cy + r * t.sin()]
            };
            for k in 0..n {
                segs.push((pt(k), pt(k + 1), a.width.mil(), a.net));
            }
        }
        for (a, b, w, bnet) in segs {
            if !inwin(a[0], a[1], w) && !inwin(b[0], b[1], w) {
                continue;
            }
            let d = [b[0] - a[0], b[1] - a[1]];
            let len = d[0].hypot(d[1]).max(1e-9);
            let (ux, uy) = (d[0] / len, d[1] / len);
            let (nx, ny) = (-uy * w / 2.0, ux * w / 2.0);
            let (ax, ay) = (a[0] - ux * w / 2.0, a[1] - uy * w / 2.0);
            let (bx, by) = (b[0] + ux * w / 2.0, b[1] + uy * w / 2.0);
            let net = mnet(bnet);
            nets_seen.insert(net);
            sheets.push(sheet(
                l,
                vec![
                    xy(ax + nx, ay + ny),
                    xy(ax - nx, ay - ny),
                    xy(bx - nx, by - ny),
                    xy(bx + nx, by + ny),
                ],
                net,
            ));
            if net == 0 {
                continue;
            }
            for (p, q) in [(a, b), (b, a)] {
                if inwin(p[0], p[1], 0.0) || !inwin(q[0], q[1], 0.0) {
                    continue;
                }
                // The face it enters by: the last one crossed from p.
                let mut best = (0usize, false, x0, -1.0f64);
                for (a, high, face) in
                    [(0, false, x0), (0, true, x1), (1, false, y0), (1, true, y1)]
                {
                    let outside = if high { p[a] > face } else { p[a] < face };
                    if outside {
                        let t = (face - p[a]) / (q[a] - p[a]);
                        if t > best.3 {
                            best = (a, high, face, t);
                        }
                    }
                }
                let (axis, high, face) = (best.0, best.1, best.2);
                let along = 1 - axis;
                let k = ((face - p[axis]) / (q[axis] - p[axis])).clamp(0.0, 1.0);
                crossings.push(Crossing {
                    layer: l,
                    net,
                    board_net: bnet,
                    axis,
                    high,
                    at: p[along] + (q[along] - p[along]) * k,
                    w,
                });
            }
        }
        // Pads: per-layer lands where the board has them.
        for p in &geo.pads {
            let land = if p.stack.is_empty() {
                on(&p.layer).then(|| {
                    (
                        p.shape,
                        p.size_x.mil(),
                        p.size_y.mil(),
                        p.rotation_deg,
                        &p.outline,
                    )
                })
            } else {
                p.stack.iter().find(|s| s.layer as usize == l).map(|s| {
                    (
                        s.shape,
                        s.size_x.mil(),
                        s.size_y.mil(),
                        s.rotation_deg,
                        &s.outline,
                    )
                })
            };
            let Some((shape, sx, sy, rot, outline)) = land else {
                continue;
            };
            let (cx, cy) = (p.at.x.mil(), p.at.y.mil());
            if !inwin(cx, cy, sx.max(sy)) {
                continue;
            }
            if p.hole.0 == 0 && (l == 0 || l == nl - 1) {
                if let Some(c) = p.component {
                    parts_pads[(l != 0) as usize].entry(c).or_default().push(p);
                }
            }
            let net = mnet(p.net);
            nets_seen.insert(net);
            if !outline.is_empty() {
                sheets.push(sheet(
                    l,
                    outline.iter().map(|q| xy(q.x.mil(), q.y.mil())).collect(),
                    net,
                ));
            } else if shape == PadShape::Round && (sx - sy).abs() < 1e-6 {
                let mut s = sheet(l, Vec::new(), net);
                s.disc = Some((xy(cx, cy), sx / 2.0 * MIL));
                sheets.push(s);
            } else {
                let (c, sn) = (rot.to_radians().cos(), rot.to_radians().sin());
                let corner = |u: f64, v: f64| xy(cx + u * c - v * sn, cy + u * sn + v * c);
                sheets.push(sheet(
                    l,
                    vec![
                        corner(-sx / 2.0, -sy / 2.0),
                        corner(sx / 2.0, -sy / 2.0),
                        corner(sx / 2.0, sy / 2.0),
                        corner(-sx / 2.0, sy / 2.0),
                    ],
                    net,
                ));
            }
        }
        // Pours (holes joined in by zero-width cuts).
        for p in geo.polygons.iter().filter(|p| on(&p.layer)) {
            let (mut lx, mut ly, mut hx, mut hy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for q in &p.outline {
                lx = lx.min(q.x.mil());
                ly = ly.min(q.y.mil());
                hx = hx.max(q.x.mil());
                hy = hy.max(q.y.mil());
            }
            if p.outline.len() < 3 || !(lx < x1 && hx > x0 && ly < y1 && hy > y0) {
                continue;
            }
            let f: Vec<[f64; 2]> = crate::geometry::fracture(&p.outline, &p.holes)
                .iter()
                .map(|q| [q[0] / 0.0254, q[1] / 0.0254])
                .collect();
            let net = mnet(p.net);
            nets_seen.insert(net);
            pours[l].push((sheets.len(), f.clone()));
            sheets.push(sheet(l, f.iter().map(|q| xy(q[0], q[1])).collect(), net));
        }
    }
    // Barrels: vias with their nets, and through-hole pins.
    let mut barrels: Vec<([f64; 2], f64, f64, usize, usize, u32)> = Vec::new();
    for v in &geo.vias {
        let (cx, cy) = (v.at.x.mil(), v.at.y.mil());
        if !inwin(cx, cy, 0.0) {
            continue;
        }
        let (a, b) = (v.from_layer as usize, (v.to_layer as usize).min(nl - 1));
        let net = mnet(v.net);
        nets_seen.insert(net);
        let hole = v.hole.mil().max(1.0);
        for l in a..=b {
            let d = if v.rings.is_empty() {
                v.diameter.mil()
            } else {
                v.rings
                    .iter()
                    .find(|r| r.0 as usize == l)
                    .map_or(v.diameter.mil(), |r| r.1.mil())
            };
            if d > hole + 0.5 {
                let mut s = sheet(l, Vec::new(), net);
                s.disc = Some((xy(cx, cy), d / 2.0 * MIL));
                sheets.push(s);
            }
        }
        barrels.push(([cx, cy], hole / 2.0, v.diameter.mil() / 2.0, a, b, net));
    }
    for p in geo.pads.iter().filter(|p| p.hole.0 > 0) {
        let (cx, cy) = (p.at.x.mil(), p.at.y.mil());
        if inwin(cx, cy, 0.0) {
            let r = p.size_x.mil().min(p.size_y.mil()) / 2.0;
            barrels.push(([cx, cy], p.hole.mil() / 2.0, r, 0, nl - 1, mnet(p.net)));
        }
    }
    // A barrel through a pour of another net with no clearance hole there
    // (a negative plane, or a reader that lost the voids) gets one.
    let mut cleared = 0;
    for (c, r_b, pad, a, b, net) in &barrels {
        solids.push(Solid {
            shape: Shape::Cyl {
                axis: 2,
                c: xy(c[0], c[1]),
                r: r_b * MIL,
                lo: z(*b),
                hi: z(*a),
            },
            fill: Fill::Pec { net: *net },
        });
        for l in *a..=*b {
            for (si, poly) in &pours[l] {
                if sheets[*si].net == *net {
                    continue;
                }
                let k = xy(c[0], c[1]);
                if in_poly(poly, *c) {
                    sheets[*si].holes.push((k, k, (pad + 6.0) * MIL));
                    cleared += 1;
                } else {
                    // The pour's own void: named as a round hole too, so
                    // the mesh keeps the ring between pad and plane.
                    let n = poly.len();
                    let r = (0..n)
                        .map(|i| crate::fdtd::seg_dist(*c, poly[i], poly[(i + 1) % n]))
                        .fold(f64::MAX, f64::min);
                    // To 0.1 mil, so the layers' rings share grid lines.
                    let r = (r * 10.0).round() / 10.0;
                    if r > *pad && r < 4.0 * pad.max(*r_b) {
                        sheets[*si].holes.push((k, k, r * MIL));
                    }
                }
            }
        }
    }
    if cleared > 0 {
        notes.push(format!(
            "{cleared} barrel crossings of other nets' pours had no clearance: given a {}-mil ring",
            6
        ));
    }
    // Ground copper on layer k at a point (for the ports' references).
    let gnd_at = |k: usize, p: [f64; 2]| {
        pours[k]
            .iter()
            .any(|(si, poly)| sheets[*si].net == 0 && in_poly(poly, p))
    };
    // Ports: one per crossing, between the ground copper above and below
    // it at the edge; a pair crossing side by side is one odd-mode port,
    // and other neighbours that overlap split the gap between them.
    struct Span {
        c: usize,
        lo: f64,
        hi: f64,
        zlo: f64,
        zhi: f64,
        minus: Option<usize>,
    }
    let mut pairs: Vec<(Option<u32>, Option<u32>)> = Vec::new();
    {
        let idx: HashMap<&str, u32> = board
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.as_str(), i as u32))
            .collect();
        for p in board
            .diff_pairs
            .iter()
            .cloned()
            .chain(crate::zcheck::name_pairs(board))
        {
            pairs.push((
                idx.get(p.positive.as_str()).copied(),
                idx.get(p.negative.as_str()).copied(),
            ));
        }
    }
    let is_pair = |a: Option<u32>, b: Option<u32>| {
        a.is_some()
            && b.is_some()
            && pairs
                .iter()
                .any(|&(p, n)| (p, n) == (a, b) || (p, n) == (b, a))
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut no_ref = 0;
    for (ci, c) in crossings.iter().enumerate() {
        // A point just inside the window at the crossing.
        let inset = 1.0;
        let p = if c.axis == 0 {
            [if c.high { x1 - inset } else { x0 + inset }, c.at]
        } else {
            [c.at, if c.high { y1 - inset } else { y0 + inset }]
        };
        let above = (0..c.layer).rev().find(|&k| gnd_at(k, p));
        let below = (c.layer + 1..nl).find(|&k| gnd_at(k, p));
        if above.is_none() && below.is_none() {
            no_ref += 1;
            continue;
        }
        let zhi = above.map_or(hi[2], z);
        let zlo = below.map_or(lo[2], z);
        let h = [
            above.map(|k| z(k) - z(c.layer)),
            below.map(|k| z(c.layer) - z(k)),
        ]
        .into_iter()
        .flatten()
        .fold(f64::MAX, f64::min)
            / MIL;
        let half = c.w / 2.0 + 3.0 * h;
        spans.push(Span {
            c: ci,
            lo: c.at - half,
            hi: c.at + half,
            zlo,
            zhi,
            minus: None,
        });
    }
    if no_ref > 0 {
        notes.push(format!(
            "{no_ref} traces leave the window with no ground copper above or below them at the edge: no port (left open)"
        ));
    }
    // Neighbours on the same face and layer, in order along it.
    spans.sort_by(|a, b| {
        let (ca, cb) = (&crossings[a.c], &crossings[b.c]);
        (ca.axis, ca.high, ca.layer)
            .cmp(&(cb.axis, cb.high, cb.layer))
            .then(ca.at.total_cmp(&cb.at))
    });
    let mut merged: Vec<Span> = Vec::new();
    for s in spans {
        if let Some(m) = merged.last_mut() {
            let (cm, cs) = (&crossings[m.c], &crossings[s.c]);
            let same = (cm.axis, cm.high, cm.layer) == (cs.axis, cs.high, cs.layer);
            if same && s.lo < m.hi {
                if m.minus.is_none() && cm.net != cs.net && is_pair(cm.board_net, cs.board_net) {
                    m.hi = s.hi;
                    m.minus = Some(s.c);
                    continue;
                }
                let mid = (crossings[m.minus.unwrap_or(m.c)].at + cs.at) / 2.0;
                m.hi = m.hi.min(mid);
                let mut s = s;
                s.lo = s.lo.max(mid);
                merged.push(s);
                continue;
            }
        }
        merged.push(s);
    }
    let layer_name = |l: usize| st.names.get(l).cloned().unwrap_or(format!("L{}", l + 1));
    let mut ports = Vec::new();
    for s in &merged {
        let c = &crossings[s.c];
        let (lo_a, hi_a, wa) = if c.axis == 0 {
            (y0, y1, wy)
        } else {
            (x0, x1, wx)
        };
        let (a, b) = (
            ((s.lo.max(lo_a)) - lo_a) * MIL,
            ((s.hi.min(hi_a)) - lo_a) * MIL,
        );
        let (mode, label) = match s.minus {
            Some(m) => (
                Mode::Diff {
                    plus: c.net,
                    minus: crossings[m].net,
                },
                format!("{}/{}", name(c.board_net), name(crossings[m].board_net)),
            ),
            None => (Mode::Tem { signal: c.net }, name(c.board_net)),
        };
        ports.push(Port {
            name: format!("P{} {} {}", ports.len() + 1, label, layer_name(c.layer)),
            axis: c.axis,
            high: c.high,
            lo: [a.max(0.0), s.zlo],
            hi: [b.min(wa), s.zhi],
            mode,
            deembed: 0.0,
        });
    }
    if ports.is_empty() {
        return Err(
            "no trace leaves the window over ground copper: nothing to drive (widen the window so the lines cross its edge)"
                .into(),
        );
    }
    // Two-pad parts on the outer layers.
    let (mut lumped, mut parts): (Vec<Lumped>, Vec<Part>) = (Vec::new(), Vec::new());
    for (side, l) in [(0usize, 0usize), (1, nl - 1)] {
        let (lp, pp) = part_ports(board, &parts_pads[side], &xy, z(l), &mut notes);
        lumped.extend(lp);
        parts.extend(pp);
    }
    for (k, p) in parts.iter_mut().enumerate() {
        p.port = ports.len() + k;
    }
    notes.insert(
        0,
        format!(
            "{} layers, {} nets (ground as one), {} barrels",
            nl,
            nets_seen.len(),
            barrels.len()
        ),
    );
    let m = Model {
        lo,
        hi,
        // Lids over the air are perfect conductor (as in `fem3d::via`);
        // the sides absorb, so the planes run on past the window.
        walls: [
            Wall::Radiation,
            Wall::Radiation,
            Wall::Radiation,
            Wall::Radiation,
            Wall::Pec,
            Wall::Pec,
        ],
        er: 1.0,
        solids,
        sheets,
        ports,
        copper: lossy.then_some(Copper {
            rough_um,
            walls: [false; 6],
        }),
        lumped,
    };
    Ok((m, parts, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fem3d, Length, Net, Point, Polygon, Track, Via};

    /// A top microstrip into a via down to an L3 stripline, between
    /// ground pours on L2 and L4 (drawn with no antipads), and a ground
    /// via beside it.
    fn board() -> Board {
        let mut b = Board {
            stackup: crate::stackfile::parse(
                "TOP, copper, 1.4mil\nP1, dielectric, 5mil, 3.5, 0.0\nL2, copper, 1.4mil\nP2, dielectric, 5mil, 3.5, 0.0\nL3, copper, 1.4mil\nP3, dielectric, 5mil, 3.5, 0.0\nBOTTOM, copper, 1.4mil\n",
            )
            .expect("stack-up"),
            ..Default::default()
        };
        for name in ["GND", "SIG"] {
            b.nets.push(Net {
                name: name.into(),
                routed_length: Length(0),
                via_length: Length(0),
                vias: 0,
                pads: 0,
                pieces: 1,
                has_pour: true,
            });
        }
        let g = &mut b.geometry;
        let tr = |l: u8, x0: f64, x1: f64, w: f64| Track {
            layer: LayerRef::Copper(l),
            net: Some(1),
            start: Point::mil(x0, 60.0),
            end: Point::mil(x1, 60.0),
            width: Length::from_mil(w),
            component: None,
            text: false,
        };
        g.tracks.push(tr(0, -40.0, 100.0, 10.0));
        g.tracks.push(tr(2, 100.0, 240.0, 4.0));
        let via = |x: f64, y: f64, net: u32| Via {
            net: Some(net),
            from_layer: 0,
            to_layer: 3,
            at: Point::mil(x, y),
            diameter: Length::from_mil(18.0),
            hole: Length::from_mil(8.0),
            rings: Vec::new(),
            fab: Default::default(),
        };
        g.vias.push(via(100.0, 60.0, 1));
        g.vias.push(via(100.0, 25.0, 0));
        for l in [1u8, 3] {
            g.polygons.push(Polygon {
                layer: LayerRef::Copper(l),
                net: Some(0),
                outline: vec![
                    Point::mil(-100.0, -100.0),
                    Point::mil(300.0, -100.0),
                    Point::mil(300.0, 220.0),
                    Point::mil(-100.0, 220.0),
                ],
                holes: Vec::new(),
                pour: None,
                component: None,
                teardrop: false,
            });
        }
        b
    }

    #[test]
    fn a_via_transition_through_the_planes() {
        let (m, _, notes) = model(&board(), [0.0, 0.0, 200.0, 120.0], false, 0.0).unwrap();
        eprintln!("{notes:?}");
        assert_eq!(m.ports.len(), 2, "{:?}", m.ports);
        // The signal via had no antipads drawn on L2 and L4: given them.
        assert!(
            notes.iter().any(|n| n.contains("2 barrel crossings")),
            "{notes:?}"
        );
        let setup = fem3d::Setup {
            f_adapt: 5.0,
            sweep: vec![1.0, 5.0],
            max_unknowns: 80_000,
            z_ref: None,
            ..fem3d::Setup::default()
        };
        let sol = fem3d::solve(&m, &setup, |l| {
            eprintln!("{l}");
            true
        })
        .unwrap();
        for (k, s) in sol.s.iter().enumerate() {
            let (s11, s21) = (s[0].norm(), s[2].norm());
            eprintln!("{} GHz S11 {s11:.3} S21 {s21:.3}", sol.f[k]);
            assert!(s21 > 0.8, "S21 {s21}");
            assert!(s11 * s11 + s21 * s21 < 1.03);
        }
    }
}
