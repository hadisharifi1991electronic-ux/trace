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
    let partner = first.net.and_then(|id| partner(board, id));
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

/// The differential pair partner of net `id` (from the board's pairs, or
/// by name: _P/_N, +/-).
pub fn partner(board: &Board, id: u32) -> Option<u32> {
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
}

/// The nets a via study drives: the via's own and its pair partner's.
pub fn via_nets(board: &Board, via: usize) -> Vec<u32> {
    let Some(id) = board.geometry.vias.get(via).and_then(|v| v.net) else {
        return Vec::new();
    };
    std::iter::once(id).chain(partner(board, id)).collect()
}

/// The FEM model of the window `win` (x0, y0, x1, y1, mil) through every
/// copper layer, its parts, and notes on what was assumed. With `focus`
/// (board net ids), only the ports of those nets are driven: every other
/// trace leaving the window is a matched termination, which costs no
/// solves; and where a focus net ends on a pad inside the window (a BGA
/// ball, a chip pin) rather than leaving it, that pad gets a lumped port:
/// to its pair partner's pad (100 ohm), else to the nearest ground pad.
pub fn model(
    board: &Board,
    win: [f64; 4],
    lossy: bool,
    rough_um: f64,
    focus: &[u32],
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
    // Every surface pad in the window, by side (for pin ports).
    let mut side_pads: [Vec<&Pad>; 2] = Default::default();
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
            // The part of the segment inside the window (Liang-Barsky):
            // a trace can cross it with both ends outside.
            let d = [b[0] - a[0], b[1] - a[1]];
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            let mut hit = true;
            for (k, lo, hi) in [(0usize, x0 - w, x1 + w), (1, y0 - w, y1 + w)] {
                if d[k].abs() < 1e-12 {
                    if a[k] < lo || a[k] > hi {
                        hit = false;
                    }
                    continue;
                }
                let (ta, tb) = ((lo - a[k]) / d[k], (hi - a[k]) / d[k]);
                let (tn, tf) = (ta.min(tb), ta.max(tb));
                t0 = t0.max(tn);
                t1 = t1.min(tf);
            }
            if !hit || t0 > t1 {
                continue;
            }
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
            // Where its centre line crosses the window's own edge, going
            // in (from a) and out (to b).
            let (mut s0, mut s1) = (0.0f64, 1.0f64);
            let (mut e0, mut e1) = (None, None);
            for (k, lo, hi) in [(0usize, x0, x1), (1, y0, y1)] {
                if d[k].abs() < 1e-12 {
                    if a[k] < lo || a[k] > hi {
                        s0 = 2.0;
                    }
                    continue;
                }
                let (ta, tb) = ((lo - a[k]) / d[k], (hi - a[k]) / d[k]);
                let (tn, tf, hn) = if ta < tb {
                    (ta, tb, false)
                } else {
                    (tb, ta, true)
                };
                if tn > s0 {
                    s0 = tn;
                    e0 = Some((k, hn, tn));
                }
                if tf < s1 {
                    s1 = tf;
                    e1 = Some((k, !hn, tf));
                }
            }
            if s0 > s1 {
                continue;
            }
            for (axis, high, t) in [e0, e1].into_iter().flatten() {
                let along = 1 - axis;
                crossings.push(Crossing {
                    layer: l,
                    net,
                    board_net: bnet,
                    axis,
                    high,
                    at: a[along] + d[along] * t,
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
                if inwin(cx, cy, 0.0) {
                    side_pads[(l != 0) as usize].push(p);
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
    let focused = |n: Option<u32>| focus.is_empty() || n.is_some_and(|n| focus.contains(&n));
    let mut ports = Vec::new();
    // Driven ports per focus net (a pair port counts for both).
    let mut driven_of: HashMap<u32, usize> = HashMap::new();
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
        let passive =
            !(focused(c.board_net) || s.minus.is_some_and(|m| focused(crossings[m].board_net)));
        if !passive {
            for n in [Some(c.board_net), s.minus.map(|m| crossings[m].board_net)]
                .into_iter()
                .flatten()
                .flatten()
            {
                *driven_of.entry(n).or_insert(0) += 1;
            }
        }
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
            name: format!("{label} {}", layer_name(c.layer)),
            axis: c.axis,
            high: c.high,
            lo: [a.max(0.0), s.zlo],
            hi: [b.min(wa), s.zhi],
            mode,
            deembed: 0.0,
            passive,
        });
    }
    // Driven ports first (S counts them in order), then the terminations.
    ports.sort_by_key(|p| p.passive);
    let nd = ports.iter().filter(|p| !p.passive).count();
    for (k, p) in ports.iter_mut().enumerate() {
        p.name = if p.passive {
            format!("T{} {}", k + 1 - nd, p.name)
        } else {
            format!("P{} {}", k + 1, p.name)
        };
    }
    if nd < ports.len() {
        notes.push(format!(
            "{} other traces leave the window: matched terminations, not driven",
            ports.len() - nd
        ));
    }
    // Focus nets that end on a pad inside the window, not at its edge.
    let centre = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let pad_of = |n: u32| -> Option<(usize, &Pad)> {
        (0..2)
            .flat_map(|side| side_pads[side].iter().map(move |p| (side, *p)))
            .filter(|(_, p)| p.net == Some(n))
            .min_by(|a, b| {
                let d = |p: &Pad| (p.at.x.mil() - centre[0]).hypot(p.at.y.mil() - centre[1]);
                d(a.1).total_cmp(&d(b.1))
            })
    };
    let ends: Vec<u32> = if focus.is_empty() {
        Vec::new()
    } else {
        focus
            .iter()
            .copied()
            .filter(|n| driven_of.get(n).copied().unwrap_or(0) < 2 && pad_of(*n).is_some())
            .collect()
    };
    let mut pins: Vec<Lumped> = Vec::new();
    let mut done: Vec<u32> = Vec::new();
    for &n in &ends {
        if done.contains(&n) {
            continue;
        }
        let Some((side, pad)) = pad_of(n) else {
            continue;
        };
        let zs = if side == 0 { z(0) } else { z(nl - 1) };
        // Its pair partner's pad beside it: one differential pin port.
        let mate = ends
            .iter()
            .copied()
            .filter(|&m| m != n && !done.contains(&m) && is_pair(Some(n), Some(m)))
            .find_map(|m| pad_of(m).filter(|(s2, _)| *s2 == side).map(|(_, p)| (m, p)));
        if let Some((m, mp)) = mate {
            if let Some((lo, hi, dir)) = crate::planar::pad_gap(pad, mp, &xy) {
                // Positive end at the high side of the gap.
                let (hi_net, lo_net) = if [mp.at.x.mil(), mp.at.y.mil()][dir]
                    > [pad.at.x.mil(), pad.at.y.mil()][dir]
                {
                    (m, n)
                } else {
                    (n, m)
                };
                pins.push(Lumped {
                    name: format!("pin {}/{}", name(Some(hi_net)), name(Some(lo_net))),
                    z: zs,
                    lo,
                    hi,
                    dir,
                    r: 100.0,
                });
                done.extend([n, m]);
                continue;
            }
        }
        // Else to the nearest ground pad side by side with it.
        let g = side_pads[side]
            .iter()
            .filter(|p| is_gnd(p.net))
            .filter_map(|p| crate::planar::pad_gap(pad, p, &xy).map(|g| (p, g)))
            .min_by(|a, b| {
                let d =
                    |p: &Pad| (p.at.x.mil() - pad.at.x.mil()).hypot(p.at.y.mil() - pad.at.y.mil());
                d(a.0).total_cmp(&d(b.0))
            });
        match g {
            Some((_, (lo, hi, dir))) => {
                pins.push(Lumped {
                    name: format!("pin {}", name(Some(n))),
                    z: zs,
                    lo,
                    hi,
                    dir,
                    r: 50.0,
                });
                done.push(n);
            }
            None => notes.push(format!(
                "{} ends on a pad in the window with no ground pad beside it along x or y: no port there",
                name(Some(n))
            )),
        }
    }
    if !pins.is_empty() {
        notes.push(format!(
            "{} pad(s) where the via's nets end inside the window: lumped ports ({})",
            pins.len(),
            pins.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if nd == 0 && pins.is_empty() {
        return Err(
            "no trace leaves the window over ground copper: nothing to drive (widen the window so the lines cross its edge)"
                .into(),
        );
    }
    // Two-pad parts on the outer layers.
    let (mut lumped, mut parts): (Vec<Lumped>, Vec<Part>) = (Vec::new(), Vec::new());
    let mut links: Vec<String> = Vec::new();
    for (side, l) in [(0usize, 0usize), (1, nl - 1)] {
        let (lp, pp) = part_ports(board, &parts_pads[side], &xy, z(l), &mut notes);
        for (lu, part) in lp.into_iter().zip(pp) {
            // A 0-ohm link is copper: as a port shorted afterwards it
            // takes the whole signal through a 50-ohm sheet and back out
            // through Γ = -1, which turns each few-percent error of the
            // solve into a large one in S (the DAC39RF12 EVM SYSREF pair
            // runs through four of them).
            if crate::planar::parse_load(&part.designator, &part.value)
                == Some(crate::planar::Load::Short)
            {
                let net = board
                    .components
                    .iter()
                    .position(|c| c.designator == part.designator)
                    .and_then(|c| parts_pads[side].get(&(c as u32)))
                    .map_or(u32::MAX - 1, |p| mnet(p[0].net));
                let d = lu.dir;
                // Into each pad by a mil, so the link overlaps both.
                let (mut a, mut b) = (lu.lo, lu.hi);
                a[d] -= MIL;
                b[d] += MIL;
                let mut s = sheet(l, vec![a, [b[0], a[1]], b, [a[0], b[1]]], net);
                s.at = lu.z;
                sheets.push(s);
                links.push(part.designator.clone());
                continue;
            }
            lumped.push(lu);
            parts.push(part);
        }
    }
    if !links.is_empty() {
        notes.push(format!(
            "0-ohm links solved as copper (no ports): {}",
            links.join(", ")
        ));
    }
    for (k, p) in parts.iter_mut().enumerate() {
        p.port = nd + k;
    }
    lumped.extend(pins);
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

    /// The via's net ending on a bottom pad inside the window (a BGA
    /// ball): a pin port to the ground pad beside it, and the other trace
    /// leaving the window only a termination when the via's net is the
    /// focus.
    #[test]
    fn a_net_ending_on_a_pad_gets_a_pin_port() {
        let mut b = board();
        // The L3 line now stops short of the edge, and a second net runs
        // across the window on L3 (a termination).
        b.geometry.tracks[1].end = Point::mil(100.0, 60.0);
        b.nets.push(Net {
            name: "OTHER".into(),
            routed_length: Length(0),
            via_length: Length(0),
            vias: 0,
            pads: 0,
            pieces: 1,
            has_pour: false,
        });
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(2),
            net: Some(2),
            start: Point::mil(-40.0, 100.0),
            end: Point::mil(240.0, 100.0),
            width: Length::from_mil(4.0),
            component: None,
            text: false,
        });
        let pad = |x: f64, y: f64, net: u32| Pad {
            layer: LayerRef::Copper(3),
            net: Some(net),
            component: None,
            name: "B".into(),
            at: Point::mil(x, y),
            size_x: Length::from_mil(14.0),
            size_y: Length::from_mil(14.0),
            hole: Length(0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        b.geometry.pads.push(pad(140.0, 60.0, 1));
        b.geometry.pads.push(pad(140.0, 92.0, 0));
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(3),
            net: Some(1),
            start: Point::mil(100.0, 60.0),
            end: Point::mil(140.0, 60.0),
            width: Length::from_mil(6.0),
            component: None,
            text: false,
        });
        // The bottom pour gives way about the via and the pads.
        b.geometry.polygons[1].holes = vec![vec![
            Point::mil(90.0, 45.0),
            Point::mil(155.0, 45.0),
            Point::mil(155.0, 105.0),
            Point::mil(90.0, 105.0),
        ]];
        let (m, parts, notes) = model(&b, [0.0, 0.0, 200.0, 120.0], false, 0.0, &[1]).unwrap();
        eprintln!("{notes:?}");
        assert!(parts.is_empty());
        let driven: Vec<&str> = m
            .ports
            .iter()
            .filter(|p| !p.passive)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(driven.len(), 1, "{:?}", m.ports);
        assert!(m.ports.iter().any(|p| p.passive), "{:?}", m.ports);
        assert_eq!(m.lumped.len(), 1, "{notes:?}");
        assert_eq!(m.lumped[0].name, "pin SIG");
        assert_eq!(m.lumped[0].dir, 1);
        // Without the focus: every trace leaving is driven, no pin.
        let (m2, _, _) = model(&b, [0.0, 0.0, 200.0, 120.0], false, 0.0, &[]).unwrap();
        assert!(m2.ports.iter().all(|p| !p.passive));
        assert!(m2.lumped.is_empty());
    }

    #[test]
    fn a_via_transition_through_the_planes() {
        let (m, _, notes) = model(&board(), [0.0, 0.0, 200.0, 120.0], false, 0.0, &[]).unwrap();
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
