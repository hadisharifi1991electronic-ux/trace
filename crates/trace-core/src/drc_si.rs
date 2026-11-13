//! Signal integrity checks: return paths and crosstalk.
//!
//! **Return path.** A track's return current flows in the copper planes
//! next to it in the stack-up. Each copper layer that pours or fills most
//! of the board is taken as a plane. Along every signal track Trace looks
//! at the planes on the copper layers just above and below it, and reports
//! where the track runs over none of their copper for longer than
//! [`GAP_RUN`] (a void or the edge of a pour), and where the copper under it
//! changes from one net to another (a split between two power islands)
//! with no other plane as near to carry the current across. Copper around
//! the track's own vias and pads, where planes are cleared for it, is not
//! counted. Nets that own plane copper are planes themselves and are not
//! checked.
//!
//! **Crosstalk.** Tracks of two nets on the same layer that run side by
//! side, closer than a gap, for longer than a limit in total. The board's
//! Altium Parallel Segment rules (`GAP`, `LIMIT`) are used where it has
//! them. Without them, nets of differential pairs are held to the 3W rule:
//! no more than [`PARALLEL_LIMIT`] of track within two track widths of
//! another net. Each report carries an estimate of the coupling,
//! 1 / (1 + (D/H)²) for centre spacing D and height H over the plane.

use super::{ClearanceRule, Item, Kind, NetInfo, Obj, Violation};
use crate::route::{point_seg, pt, P};
use crate::{Board, LayerRef, Rule};
use std::collections::{HashMap, HashSet};

/// A track may cross this much plane void (mil) before it is reported,
/// or five track widths when that is more.
pub const GAP_RUN: f64 = 50.0;
/// Default parallel length allowed within 3W of a pair net (mil).
pub const PARALLEL_LIMIT: f64 = 250.0;
/// Share of the board a copper layer must cover to be taken as a plane.
const PLANE_COVER: f64 = 0.3;
/// Copper this far (mil) beyond a track's own via or pad is the clearance
/// planes keep around it, not a void the track crosses.
const OWN_CLEAR: f64 = 15.0;

pub(super) fn check(board: &Board, info: &NetInfo, out: &mut Vec<Violation>) {
    let planes = Planes::new(board);
    return_path(board, &planes, out);
    crosstalk(board, info, out);
}

// ------------------------------------------------------------- planes

/// The copper of every copper layer, rasterised: per cell the net slot of
/// the copper there (0 for none).
struct Planes {
    x0: f64,
    y0: f64,
    cell: f64,
    w: usize,
    h: usize,
    /// Per copper layer: cells and the net of each slot. `None` where the
    /// layer is not a plane.
    layers: Vec<Option<Plane>>,
}

struct Plane {
    cells: Vec<u8>,
    nets: Vec<Option<u32>>,
}

impl Planes {
    fn new(board: &Board) -> Planes {
        let g = &board.geometry;
        let n = board.stackup.copper_layers().count().max(
            g.polygons
                .iter()
                .filter_map(|p| match p.layer {
                    LayerRef::Copper(l) => Some(l as usize + 1),
                    _ => None,
                })
                .max()
                .unwrap_or(0),
        );
        let outline: Vec<P> = g.outline.iter().map(|&p| pt(p)).collect();
        let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let poly_pts = g
            .polygons
            .iter()
            .flat_map(|p| p.outline.iter())
            .map(|&p| pt(p));
        for p in outline.iter().copied().chain(poly_pts) {
            bb = [
                bb[0].min(p[0]),
                bb[1].min(p[1]),
                bb[2].max(p[0]),
                bb[3].max(p[1]),
            ];
        }
        let empty = Planes {
            x0: 0.0,
            y0: 0.0,
            cell: 1.0,
            w: 0,
            h: 0,
            layers: (0..n).map(|_| None).collect(),
        };
        if bb[0] >= bb[2] || bb[1] >= bb[3] {
            return empty;
        }
        // 2 mil cells, coarser on a large board: at most 8 million a layer.
        let (sx, sy) = (bb[2] - bb[0], bb[3] - bb[1]);
        let cell = 2.0f64.max((sx * sy / 8.0e6).sqrt());
        let (w, h) = (
            (sx / cell).ceil() as usize + 1,
            (sy / cell).ceil() as usize + 1,
        );
        let board_area = if outline.len() >= 3 {
            ring_area(&outline)
        } else {
            sx * sy
        };
        let mut planes = Planes {
            x0: bb[0],
            y0: bb[1],
            cell,
            w,
            h,
            layers: Vec::new(),
        };
        let mut by_layer: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, p) in g.polygons.iter().enumerate() {
            if let LayerRef::Copper(l) = p.layer {
                by_layer[l as usize].push(i);
            }
        }
        for list in by_layer {
            // A quick look at the copper's area before filling cells.
            let area: f64 = list
                .iter()
                .map(|&i| {
                    let p = &g.polygons[i];
                    let ring = |r: &[crate::Point]| {
                        ring_area(&r.iter().map(|&q| pt(q)).collect::<Vec<_>>())
                    };
                    ring(&p.outline) - p.holes.iter().map(|h| ring(h)).sum::<f64>()
                })
                .sum();
            if area < PLANE_COVER * board_area {
                planes.layers.push(None);
                continue;
            }
            let mut plane = Plane {
                cells: vec![0; w * h],
                nets: Vec::new(),
            };
            for &i in &list {
                let p = &g.polygons[i];
                let slot = match plane.nets.iter().position(|&x| x == p.net) {
                    Some(k) => k + 1,
                    None if plane.nets.len() < 255 => {
                        plane.nets.push(p.net);
                        plane.nets.len()
                    }
                    None => 255,
                } as u8;
                let rings: Vec<Vec<P>> = std::iter::once(&p.outline)
                    .chain(p.holes.iter())
                    .map(|r| r.iter().map(|&q| pt(q)).collect())
                    .collect();
                planes.fill(&mut plane.cells, &rings, slot);
            }
            planes.layers.push(Some(plane));
        }
        planes
    }

    /// Fills the cells whose centres are inside `rings` (even-odd).
    fn fill(&self, cells: &mut [u8], rings: &[Vec<P>], slot: u8) {
        let mut xs: Vec<(usize, f64)> = Vec::new();
        for r in rings {
            for k in 0..r.len() {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                if a[1] == b[1] {
                    continue;
                }
                let (lo, hi) = (a[1].min(b[1]), a[1].max(b[1]));
                // Rows whose centre y lies in [lo, hi).
                let j0 = ((lo - self.y0) / self.cell - 0.5).ceil().max(0.0) as usize;
                let j1 = ((hi - self.y0) / self.cell - 0.5).ceil().min(self.h as f64) as usize;
                for j in j0..j1 {
                    let y = self.y0 + (j as f64 + 0.5) * self.cell;
                    if y < lo || y >= hi {
                        continue;
                    }
                    let x = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                    xs.push((j, x));
                }
            }
        }
        xs.sort_by(|p, q| p.0.cmp(&q.0).then(p.1.total_cmp(&q.1)));
        for pair in xs.chunks(2) {
            let [(j, xa), (j2, xb)] = pair else { continue };
            if j != j2 {
                continue;
            }
            let i0 = ((xa - self.x0) / self.cell - 0.5).ceil().max(0.0) as usize;
            let i1 = (((xb - self.x0) / self.cell - 0.5).floor() + 1.0).clamp(0.0, self.w as f64)
                as usize;
            for i in i0..i1 {
                cells[j * self.w + i] = slot;
            }
        }
    }

    /// The plane copper of layer `l` at `p`: `None` for no copper, else
    /// the net (which may itself be `None`, a fill without a net).
    fn at(&self, l: usize, p: P) -> Option<Option<u32>> {
        let plane = self.layers.get(l)?.as_ref()?;
        let i = ((p[0] - self.x0) / self.cell).floor();
        let j = ((p[1] - self.y0) / self.cell).floor();
        if i < 0.0 || j < 0.0 || i >= self.w as f64 || j >= self.h as f64 {
            return None;
        }
        match plane.cells[j as usize * self.w + i as usize] {
            0 => None,
            s => Some(plane.nets.get(s as usize - 1).copied().flatten()),
        }
    }

    fn is_plane(&self, l: usize) -> bool {
        self.layers.get(l).is_some_and(Option::is_some)
    }
}

fn ring_area(r: &[P]) -> f64 {
    let mut a = 0.0;
    for k in 0..r.len() {
        let (p, q) = (r[k], r[(k + 1) % r.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

/// Dielectric height (mil) from each copper layer to the copper layer
/// above and below it.
fn heights(board: &Board) -> Vec<(Option<f64>, Option<f64>)> {
    crate::impedance::layer_geometry(&board.stackup)
        .iter()
        .map(|g| (g.above, g.below))
        .collect()
}

// --------------------------------------------------------- return path

/// A piece of track to walk: its points, width and board item.
struct Run {
    layer: u8,
    net: u32,
    width: f64,
    pts: Vec<P>,
    item: Item,
}

fn runs(board: &Board) -> Vec<Run> {
    let g = &board.geometry;
    let mut out = Vec::new();
    for (i, t) in g.tracks.iter().enumerate() {
        let (LayerRef::Copper(l), Some(net)) = (&t.layer, t.net) else {
            continue;
        };
        if t.text || t.component.is_some() {
            continue;
        }
        out.push(Run {
            layer: *l,
            net,
            width: t.width.mil(),
            pts: vec![pt(t.start), pt(t.end)],
            item: Item::Track(i),
        });
    }
    for (i, a) in g.arcs.iter().enumerate() {
        let (LayerRef::Copper(l), Some(net)) = (&a.layer, a.net) else {
            continue;
        };
        if a.component.is_some() {
            continue;
        }
        let (c, r) = (pt(a.center), a.radius.mil());
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let k = (sweep / 5.0).ceil().max(1.0) as usize;
        let pts = (0..=k)
            .map(|j| {
                let t = (a.start_deg + sweep * j as f64 / k as f64).to_radians();
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect();
        out.push(Run {
            layer: *l,
            net,
            width: a.width.mil(),
            pts,
            item: Item::Arc(i),
        });
    }
    out
}

/// Where planes are cleared for each net's own vias and pads, per copper
/// layer: circles, bucketed on a coarse grid.
struct Own {
    grid: HashMap<(u32, i32, i32), Vec<(P, f64)>>,
}

const OWN_CELL: f64 = 100.0;

impl Own {
    fn new(board: &Board) -> Own {
        let g = &board.geometry;
        let mut grid: HashMap<(u32, i32, i32), Vec<(P, f64)>> = HashMap::new();
        let mut add = |net: u32, c: P, r: f64| {
            for gx in
                ((c[0] - r) / OWN_CELL).floor() as i32..=((c[0] + r) / OWN_CELL).floor() as i32
            {
                for gy in
                    ((c[1] - r) / OWN_CELL).floor() as i32..=((c[1] + r) / OWN_CELL).floor() as i32
                {
                    grid.entry((net, gx, gy)).or_default().push((c, r));
                }
            }
        };
        for v in &g.vias {
            if let Some(n) = v.net {
                add(n, pt(v.at), v.diameter.mil() / 2.0 + OWN_CLEAR);
            }
        }
        for p in &g.pads {
            if let Some(n) = p.net {
                let r = p.size_x.mil().hypot(p.size_y.mil()) / 2.0;
                add(n, pt(p.at), r + OWN_CLEAR);
            }
        }
        Own { grid }
    }

    fn near(&self, net: u32, p: P) -> bool {
        let key = (
            net,
            (p[0] / OWN_CELL).floor() as i32,
            (p[1] / OWN_CELL).floor() as i32,
        );
        self.grid.get(&key).is_some_and(|l| {
            l.iter()
                .any(|(c, r)| (p[0] - c[0]).hypot(p[1] - c[1]) <= *r)
        })
    }
}

fn layer_name(board: &Board, l: usize) -> String {
    board
        .stackup
        .copper_layers()
        .nth(l)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("copper layer {}", l + 1))
}

fn return_path(board: &Board, planes: &Planes, out: &mut Vec<Violation>) {
    let n = planes.layers.len();
    if n < 2 || !(0..n).any(|l| planes.is_plane(l)) {
        return;
    }
    // Nets that own plane copper are the planes themselves.
    let mut plane_nets: HashSet<u32> = HashSet::new();
    for p in &board.geometry.polygons {
        if let (LayerRef::Copper(l), Some(net)) = (&p.layer, p.net) {
            if planes.is_plane(*l as usize) {
                plane_nets.insert(net);
            }
        }
    }
    let hs = heights(board);
    let own = Own::new(board);
    let name = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "no net".into())
    };
    let step = planes.cell.max(1.0);
    for run in runs(board) {
        if plane_nets.contains(&run.net) {
            continue;
        }
        let l = run.layer as usize;
        // The planes next to the layer, nearest first.
        let (up, down) = hs.get(l).copied().unwrap_or((None, None));
        let mut refs: Vec<(usize, f64)> = Vec::new();
        if l > 0 && planes.is_plane(l - 1) {
            refs.push((l - 1, up.unwrap_or(f64::MAX)));
        }
        if l + 1 < n && planes.is_plane(l + 1) {
            refs.push((l + 1, down.unwrap_or(f64::MAX)));
        }
        if refs.is_empty() {
            continue;
        }
        refs.sort_by(|a, b| a.1.total_cmp(&b.1));
        let nearest: Vec<usize> = refs
            .iter()
            .filter(|r| r.1 <= refs[0].1 + 1e-6)
            .map(|r| r.0)
            .collect();
        let limit = GAP_RUN.max(5.0 * run.width);
        // Walk the track: the longest stretch over no plane copper, and
        // the first change of net on the nearest planes.
        let (mut gap, mut worst): (f64, (f64, P)) = (0.0, (0.0, [0.0, 0.0]));
        let mut gap_from = [0.0, 0.0];
        let mut last: Vec<Option<Option<u32>>> = vec![None; nearest.len()];
        let mut split: Option<(P, usize, Option<u32>, Option<u32>)> = None;
        for w in run.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            let k = (len / step).ceil().max(1.0) as usize;
            for s in 0..=k {
                let t = s as f64 / k as f64;
                let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                if own.near(run.net, p) {
                    gap = 0.0;
                    continue;
                }
                let under: Vec<Option<Option<u32>>> =
                    refs.iter().map(|r| planes.at(r.0, p)).collect();
                if under.iter().all(Option::is_none) {
                    if gap == 0.0 {
                        gap_from = p;
                    }
                    gap += len / k as f64;
                    if gap > worst.0 {
                        worst = (
                            gap,
                            [(gap_from[0] + p[0]) / 2.0, (gap_from[1] + p[1]) / 2.0],
                        );
                    }
                } else {
                    gap = 0.0;
                }
                // A split: every nearest plane is under the track here and
                // carries another net than where it last was.
                let now: Vec<Option<Option<u32>>> =
                    nearest.iter().map(|&r| planes.at(r, p)).collect();
                if split.is_none() && now.iter().all(Option::is_some) {
                    let changed = now
                        .iter()
                        .zip(&last)
                        .all(|(x, y)| matches!((x, y), (Some(a), Some(b)) if a != b));
                    if changed {
                        let (Some(was), Some(is)) = (last[0], now[0]) else {
                            unreachable!()
                        };
                        split = Some((p, nearest[0], was, is));
                    }
                    last.clone_from(&now);
                }
            }
        }
        if worst.0 > limit {
            out.push(Violation {
                kind: Kind::ReturnPath,
                layer: Some(run.layer),
                at: worst.1,
                nets: [Some(run.net), None],
                items: [Some(run.item), None],
                objects: [
                    Some(if matches!(run.item, Item::Arc(_)) {
                        Obj::Arc
                    } else {
                        Obj::Track
                    }),
                    None,
                ],
                found: worst.0,
                need: limit,
                rule: format!(
                    "over no plane copper on {}",
                    refs.iter()
                        .map(|r| layer_name(board, r.0))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            });
        }
        if let Some((p, l, was, is)) = split {
            out.push(Violation {
                kind: Kind::ReturnPath,
                layer: Some(run.layer),
                at: p,
                nets: [Some(run.net), is],
                items: [Some(run.item), None],
                objects: [
                    Some(if matches!(run.item, Item::Arc(_)) {
                        Obj::Arc
                    } else {
                        Obj::Track
                    }),
                    Some(Obj::Pour),
                ],
                found: 0.0,
                need: 0.0,
                rule: format!(
                    "crosses a split on {}, from {} to {}",
                    layer_name(board, l),
                    name(was),
                    name(is)
                ),
            });
        }
    }
}

// ----------------------------------------------------------- crosstalk

/// A parallel segment rule: within `gap` of each other, nets may run
/// side by side for at most `limit`.
struct Parallel {
    name: String,
    gap: f64,
    /// Twice the wider track instead of a fixed gap (the 3W rule).
    three_w: bool,
    limit: f64,
    scope: Option<ClearanceRule>,
}

fn rules(board: &Board, info: &NetInfo) -> Vec<Parallel> {
    let mut list: Vec<&Rule> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("ParallelSegment"))
        .collect();
    list.sort_by_key(|r| r.priority);
    list.iter()
        .filter_map(|r| {
            let limit = super::param(r, "LIMIT")
                .and_then(crate::Length::parse)?
                .mil();
            let scope = ClearanceRule::new(r, info)?;
            Some(Parallel {
                name: r.name.clone(),
                gap: scope.gap,
                three_w: false,
                limit,
                scope: Some(scope),
            })
        })
        .collect()
}

fn crosstalk(board: &Board, info: &NetInfo, out: &mut Vec<Violation>) {
    let own = rules(board, info);
    let partners = board.pair_partners();
    let default = Parallel {
        name: "3W rule for pair nets".into(),
        gap: 0.0,
        three_w: true,
        limit: PARALLEL_LIMIT,
        scope: None,
    };
    // The rule for two nets: the first that takes them.
    let rule_for = |a: u32, b: u32| -> Option<&Parallel> {
        if own.is_empty() {
            return (partners.get(&a) != Some(&b)
                && (partners.contains_key(&a) || partners.contains_key(&b)))
            .then_some(&default);
        }
        own.iter().find(|r| {
            r.scope.as_ref().is_none_or(|s| {
                s.applies((Some(a), Obj::Track), (Some(b), Obj::Track)) != Some(false)
            })
        })
    };
    let max_gap = own.iter().map(|r| r.gap).fold(0.0, f64::max);
    let g = &board.geometry;
    struct Seg {
        a: P,
        b: P,
        w: f64,
        net: u32,
        layer: u8,
        i: usize,
    }
    let segs: Vec<Seg> = g
        .tracks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| match (&t.layer, t.net) {
            (LayerRef::Copper(l), Some(net)) if !t.text && t.component.is_none() => {
                let (a, b) = (pt(t.start), pt(t.end));
                ((b[0] - a[0]).hypot(b[1] - a[1]) > 0.5).then_some(Seg {
                    a,
                    b,
                    w: t.width.mil(),
                    net,
                    layer: *l,
                    i,
                })
            }
            _ => None,
        })
        .collect();
    if segs.is_empty() {
        return;
    }
    let widest = segs.iter().map(|s| s.w).fold(0.0, f64::max);
    let reach = max_gap.max(2.0 * widest) + widest;
    const CELL: f64 = 100.0;
    let mut grid: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
    for (k, s) in segs.iter().enumerate() {
        let (x0, x1) = (s.a[0].min(s.b[0]), s.a[0].max(s.b[0]));
        let (y0, y1) = (s.a[1].min(s.b[1]), s.a[1].max(s.b[1]));
        for gx in (x0 / CELL).floor() as i32..=(x1 / CELL).floor() as i32 {
            for gy in (y0 / CELL).floor() as i32..=(y1 / CELL).floor() as i32 {
                grid.entry((s.layer, gx, gy)).or_default().push(k);
            }
        }
    }
    // Per pair of nets on a layer: total parallel length, the longest
    // stretch, where it is, the tracks and their gap and spacing.
    #[derive(Default)]
    struct Acc {
        total: f64,
        longest: f64,
        at: P,
        items: (usize, usize),
        gap: f64,
        pitch: f64,
    }
    let mut acc: HashMap<(u32, u32, u8), Acc> = HashMap::new();
    let mut rule_of: HashMap<(u32, u32), Option<usize>> = HashMap::new();
    for (k, s) in segs.iter().enumerate() {
        let (x0, x1) = (s.a[0].min(s.b[0]) - reach, s.a[0].max(s.b[0]) + reach);
        let (y0, y1) = (s.a[1].min(s.b[1]) - reach, s.a[1].max(s.b[1]) + reach);
        let mut seen = HashSet::new();
        let len = (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1]);
        let d = [(s.b[0] - s.a[0]) / len, (s.b[1] - s.a[1]) / len];
        for gx in (x0 / CELL).floor() as i32..=(x1 / CELL).floor() as i32 {
            for gy in (y0 / CELL).floor() as i32..=(y1 / CELL).floor() as i32 {
                let Some(list) = grid.get(&(s.layer, gx, gy)) else {
                    continue;
                };
                for &m in list {
                    if m <= k || !seen.insert(m) {
                        continue;
                    }
                    let o = &segs[m];
                    if o.net == s.net {
                        continue;
                    }
                    let (lo, hi) = (s.net.min(o.net), s.net.max(o.net));
                    let r = *rule_of.entry((lo, hi)).or_insert_with(|| {
                        rule_for(lo, hi).map(|r| {
                            if r.three_w {
                                usize::MAX
                            } else {
                                own.iter().position(|x| std::ptr::eq(x, r)).unwrap_or(0)
                            }
                        })
                    });
                    let Some(r) = r else { continue };
                    let rule = if r == usize::MAX { &default } else { &own[r] };
                    // Parallel within about 5 degrees.
                    let ol = (o.b[0] - o.a[0]).hypot(o.b[1] - o.a[1]);
                    let e = [(o.b[0] - o.a[0]) / ol, (o.b[1] - o.a[1]) / ol];
                    if (d[0] * e[1] - d[1] * e[0]).abs() > 0.09 {
                        continue;
                    }
                    // Overlap of the other along this one.
                    let proj = |p: P| (p[0] - s.a[0]) * d[0] + (p[1] - s.a[1]) * d[1];
                    let (t0, t1) = (proj(o.a), proj(o.b));
                    let (t0, t1) = (t0.min(t1).max(0.0), t0.max(t1).min(len));
                    if t1 - t0 < 1.0 {
                        continue;
                    }
                    let at = |t: f64| [s.a[0] + d[0] * t, s.a[1] + d[1] * t];
                    let pitch = point_seg(at(t0), o.a, o.b).max(point_seg(at(t1), o.a, o.b));
                    let gap = pitch - (s.w + o.w) / 2.0;
                    let within = if rule.three_w {
                        2.0 * s.w.max(o.w)
                    } else {
                        rule.gap
                    };
                    if gap >= within - 1e-6 || gap <= 0.0 {
                        continue;
                    }
                    let a = acc.entry((lo, hi, s.layer)).or_default();
                    a.total += t1 - t0;
                    if t1 - t0 > a.longest {
                        a.longest = t1 - t0;
                        a.at = at((t0 + t1) / 2.0);
                        a.items = (s.i, o.i);
                        a.gap = gap;
                        a.pitch = pitch;
                    }
                }
            }
        }
    }
    let hs = heights(board);
    let mut found: Vec<_> = acc.into_iter().collect();
    found.sort_by_key(|((a, b, l), _)| (*a, *b, *l));
    for ((a, b, l), x) in found {
        let r = rule_of.get(&(a, b)).copied().flatten();
        let rule = match r {
            Some(usize::MAX) => &default,
            Some(r) => &own[r],
            None => continue,
        };
        if x.total <= rule.limit {
            continue;
        }
        let h = match hs.get(l as usize) {
            Some((Some(u), Some(d))) => u.min(*d),
            Some((Some(h), None) | (None, Some(h))) => *h,
            _ => 5.0,
        };
        let coupling = 100.0 / (1.0 + (x.pitch / h).powi(2));
        out.push(Violation {
            kind: Kind::Crosstalk,
            layer: Some(l),
            at: x.at,
            nets: [Some(a), Some(b)],
            items: [Some(Item::Track(x.items.0)), Some(Item::Track(x.items.1))],
            objects: [Some(Obj::Track), Some(Obj::Track)],
            found: x.total,
            need: rule.limit,
            rule: format!(
                "{} apart at closest, about {:.1}% coupling ({})",
                fmt_mil(x.gap),
                coupling,
                rule.name
            ),
        });
    }
}

fn fmt_mil(v: f64) -> String {
    format!("{v:.2} mil")
}

pub(super) fn describe(board: &Board, v: &Violation) -> String {
    let net = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "no net".into())
    };
    match v.kind {
        Kind::ReturnPath if v.found > 0.0 => format!(
            "{} runs {:.0} mil {}, allowed {:.0}",
            net(v.nets[0]),
            v.found,
            v.rule,
            v.need
        ),
        Kind::ReturnPath => format!("{} {}", net(v.nets[0]), v.rule),
        Kind::Crosstalk => format!(
            "{} runs {:.0} mil beside {}, allowed {:.0}: {}",
            net(v.nets[0]),
            v.found,
            net(v.nets[1]),
            v.need,
            v.rule
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn p(x: f64, y: f64) -> Point {
        Point::mil(x, y)
    }

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
        vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]
    }

    fn track(net: u32, layer: u8, a: (f64, f64), b: (f64, f64), w: f64) -> Track {
        Track {
            layer: LayerRef::Copper(layer),
            net: Some(net),
            start: p(a.0, a.1),
            end: p(b.0, b.1),
            width: Length::from_mil(w),
            component: None,
            text: false,
        }
    }

    fn plane(net: u32, layer: u8, outline: Vec<Point>, holes: Vec<Vec<Point>>) -> Polygon {
        Polygon {
            layer: LayerRef::Copper(layer),
            net: Some(net),
            outline,
            holes,
            pour: None,
            component: None,
        }
    }

    /// Three copper layers: signal on top, GND below it, signal at the bottom.
    fn board() -> Board {
        let mut b = Board::default();
        for (name, kind, t) in [
            ("Top", LayerKind::Copper, 1.4),
            ("D1", LayerKind::Dielectric, 4.0),
            ("GND", LayerKind::Copper, 1.4),
            ("D2", LayerKind::Dielectric, 4.0),
            ("Bottom", LayerKind::Copper, 1.4),
        ] {
            b.stackup.layers.push(StackLayer {
                name: name.into(),
                altium_id: None,
                kind,
                thickness: Length::from_mil(t),
                dielectric_constant: Some(4.0),
                material: None,
            });
        }
        for n in ["GND", "SIG", "P", "N", "OTHER", "VCC"] {
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
        b.geometry.outline = square(0.0, 0.0, 2000.0, 1000.0);
        b
    }

    #[test]
    fn track_over_a_void_and_across_a_split() {
        let mut b = board();
        // GND plane with a 100 mil slot across x 900..1000.
        b.geometry.polygons.push(plane(
            0,
            1,
            square(0.0, 0.0, 2000.0, 1000.0),
            vec![square(900.0, 100.0, 1000.0, 900.0)],
        ));
        // Over the slot.
        b.geometry
            .tracks
            .push(track(1, 0, (100.0, 500.0), (1900.0, 500.0), 5.0));
        // Beside it, over solid copper.
        b.geometry
            .tracks
            .push(track(1, 0, (100.0, 50.0), (1900.0, 50.0), 5.0));
        let info = NetInfo::new(&b);
        let mut out = Vec::new();
        check(&b, &info, &mut out);
        let rp: Vec<_> = out.iter().filter(|v| v.kind == Kind::ReturnPath).collect();
        assert_eq!(rp.len(), 1, "{:?}", rp);
        assert_eq!(rp[0].items[0], Some(Item::Track(0)));
        assert!((rp[0].found - 100.0).abs() < 6.0, "{}", rp[0].found);
        assert!((rp[0].at[0] - 950.0).abs() < 6.0);

        // Split the plane: GND on the left, VCC on the right.
        b.geometry.polygons = vec![
            plane(0, 1, square(0.0, 0.0, 995.0, 1000.0), vec![]),
            plane(5, 1, square(1005.0, 0.0, 2000.0, 1000.0), vec![]),
        ];
        let mut out = Vec::new();
        check(&b, &NetInfo::new(&b), &mut out);
        let rp: Vec<_> = out.iter().filter(|v| v.kind == Kind::ReturnPath).collect();
        assert_eq!(rp.len(), 2, "{:?}", rp);
        assert!(rp.iter().all(|v| v.found == 0.0 && v.nets[1] == Some(5)));
        assert!(
            describe(&b, rp[0]).contains("from GND to VCC"),
            "{}",
            describe(&b, rp[0])
        );
    }

    #[test]
    fn own_via_clearance_is_not_a_void() {
        let mut b = board();
        // A 40 mil antipad round the track's own via at x 1000.
        let hole: Vec<Point> = (0..16)
            .map(|k| {
                let t = k as f64 * std::f64::consts::TAU / 16.0;
                p(1000.0 + 20.0 * t.cos(), 500.0 + 20.0 * t.sin())
            })
            .collect();
        b.geometry
            .polygons
            .push(plane(0, 1, square(0.0, 0.0, 2000.0, 1000.0), vec![hole]));
        b.geometry
            .tracks
            .push(track(1, 0, (100.0, 500.0), (1000.0, 500.0), 5.0));
        b.geometry.vias.push(Via {
            net: Some(1),
            from_layer: 0,
            to_layer: 2,
            at: p(1000.0, 500.0),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: vec![],
            fab: Fab::default(),
        });
        let mut out = Vec::new();
        check(&b, &NetInfo::new(&b), &mut out);
        assert!(out.iter().all(|v| v.kind != Kind::ReturnPath), "{:?}", out);
    }

    #[test]
    fn pair_nets_held_to_three_w() {
        let mut b = board();
        b.geometry
            .polygons
            .push(plane(0, 1, square(0.0, 0.0, 2000.0, 1000.0), vec![]));
        // The pair, 5 mil apart: its own partner is never reported.
        b.geometry
            .tracks
            .push(track(2, 0, (100.0, 500.0), (1100.0, 500.0), 5.0));
        b.geometry
            .tracks
            .push(track(3, 0, (100.0, 510.0), (1100.0, 510.0), 5.0));
        // Another net 6 mil from P over 400 mil (within 2W = 10 mil).
        b.geometry
            .tracks
            .push(track(4, 0, (300.0, 489.0), (700.0, 489.0), 5.0));
        // And one 12 mil away, outside 3W.
        b.geometry
            .tracks
            .push(track(1, 0, (100.0, 527.0), (1100.0, 527.0), 5.0));
        let mut out = Vec::new();
        check(&b, &NetInfo::new(&b), &mut out);
        let x: Vec<_> = out.iter().filter(|v| v.kind == Kind::Crosstalk).collect();
        assert_eq!(x.len(), 1, "{:?}", x);
        assert_eq!(x[0].nets, [Some(2), Some(4)]);
        assert!((x[0].found - 400.0).abs() < 1.0);
        let text = describe(&b, x[0]);
        assert!(text.contains("P runs 400 mil beside OTHER"), "{text}");

        // A board rule of its own replaces the default.
        b.rules.push(Rule {
            kind: "ParallelSegment".into(),
            name: "Xtalk".into(),
            enabled: true,
            priority: 1,
            scope: vec!["All".into(), "All".into()],
            params: [("GAP", "15mil"), ("LIMIT", "500mil")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
        let mut out = Vec::new();
        check(&b, &NetInfo::new(&b), &mut out);
        let x: Vec<_> = out.iter().filter(|v| v.kind == Kind::Crosstalk).collect();
        // P-N (5 mil, 1000 long) and N-SIG (12 mil, 1000 long) break it.
        assert_eq!(x.len(), 2, "{:?}", x);
        assert!(x.iter().all(|v| v.rule.contains("Xtalk")));
    }
}
