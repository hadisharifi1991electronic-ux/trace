//! Bus (multi-trace) routing: several nets routed side by side along one
//! centre line at a set spacing, and their lengths matched afterwards.
//! Units are mil.
//!
//! Each net is a lane: lanes are ordered across the route as their start
//! pads are, so tracks never cross. A lane is the centre line shifted
//! sideways; it joins its pad with a 45° jog where the pads sit wider or
//! narrower than the lanes, as a fan-out does.

use crate::edit::Edit;
use crate::route::{self, seg_seg, Obstacles, Piece, P};

/// A pair meander tried: its skew, the two legs and their lengths.
type PairTry = (f64, Vec<Piece>, Vec<Piece>, f64, f64);
use crate::{Board, LayerRef, Length, Point, Track};

/// Orders bus starts (pad centre, net) across the bus: along the line the
/// pads lie on (their principal axis), so neighbouring pads get
/// neighbouring lanes.
pub fn order(starts: &mut [(P, u32)]) {
    let n = starts.len().max(1) as f64;
    let c = starts
        .iter()
        .fold([0.0, 0.0], |s, (p, _)| [s[0] + p[0] / n, s[1] + p[1] / n]);
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for (p, _) in starts.iter() {
        let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
        xx += dx * dx;
        xy += dx * dy;
        yy += dy * dy;
    }
    let t = 0.5 * (2.0 * xy).atan2(xx - yy);
    let axis = [t.cos(), t.sin()];
    starts.sort_by(|a, b| {
        let ka = (a.0[0] - c[0]) * axis[0] + (a.0[1] - c[1]) * axis[1];
        let kb = (b.0[0] - c[0]) * axis[0] + (b.0[1] - c[1]) * axis[1];
        ka.total_cmp(&kb)
    });
}

/// Where lane `i` of `n` sits across the bus, in pitches from the centre
/// line: -(n-1)/2 .. (n-1)/2.
pub fn slot(i: usize, n: usize) -> f64 {
    i as f64 - (n as f64 - 1.0) / 2.0
}

/// Which way round the lanes go (+1 or -1): lane `i` is shifted by
/// `side * slot(i, n) * pitch` to the left of the centre line, chosen so
/// the lanes are in the same order across the route as `from` (their
/// pads or vias). `start` is where the centre line begins.
pub fn side(centre: &[Piece], start: P, from: &[P]) -> f64 {
    let Some((a, b)) = centre.iter().find_map(|p| p.chords().first().copied()) else {
        return 1.0;
    };
    let d = [b[0] - a[0], b[1] - a[1]];
    let n = from.len();
    let sum: f64 = from
        .iter()
        .enumerate()
        .map(|(i, f)| slot(i, n) * (d[0] * (f[1] - start[1]) - d[1] * (f[0] - start[0])))
        .sum();
    if sum > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// One lane: the centre line shifted `k` to the left, joined to `from` and
/// (when the route has landed) to `to`. With `fan`, the joins are 45° jogs
/// where the straight piece they meet is long enough; otherwise straight
/// lines from the pad to the lane.
pub fn lane(centre: &[Piece], k: f64, from: P, to: Option<P>, fan: bool) -> Vec<Piece> {
    let mut off = route::offset(centre, k);
    if off.is_empty() {
        return Vec::new();
    }
    if fan {
        if let Some(Piece::Line(a, b)) = off.first().copied() {
            if let Some(j) = jog(from, a, b) {
                off[0] = Piece::Line(j, b);
            }
        }
        if let (Some(t), Some(Piece::Line(a, b))) = (to, off.last().copied()) {
            if let Some(j) = jog(t, b, a) {
                let last = off.len() - 1;
                off[last] = Piece::Line(a, j);
            }
        }
    }
    let mut out = Vec::with_capacity(off.len() + 2);
    out.push(Piece::Line(from, off[0].start()));
    let end = off[off.len() - 1].end();
    out.extend(off);
    if let Some(t) = to {
        out.push(Piece::Line(end, t));
    }
    out.retain(|p| p.length() > 1e-6);
    out
}

/// The point on the straight a→b (leaving a) that `p` reaches with a 45°
/// jog, when it lies on that straight short of b.
fn jog(p: P, a: P, b: P) -> Option<P> {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len < 1e-6 {
        return None;
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let n = [-u[1], u[0]];
    let (dx, dy) = (a[0] - p[0], a[1] - p[1]);
    let across = (dx * n[0] + dy * n[1]).abs();
    let ahead = dx * u[0] + dy * u[1];
    let t = across - ahead;
    if across < 1e-6 || t <= 0.0 || t >= len - 1e-6 {
        return None;
    }
    Some([a[0] + u[0] * t, a[1] + u[1] * t])
}

/// What matching a group's lengths did for one net.
#[derive(Clone, Debug)]
pub struct Tuned {
    pub net: u32,
    pub before: f64,
    pub after: f64,
}

/// The result of [`match_lengths`]: the edits, each net's length before
/// and after, and the length matched to.
#[derive(Clone, Debug, Default)]
pub struct Matched {
    pub edits: Vec<Edit>,
    pub nets: Vec<Tuned>,
    pub target: f64,
    /// Differential pairs tuned as pairs (positive, negative): both legs
    /// meander together and the pair's mean length is matched.
    pub pairs: Vec<(u32, u32)>,
}

impl Matched {
    /// Nets left more than `tol` short of the target.
    pub fn short(&self, tol: f64) -> impl Iterator<Item = &Tuned> {
        self.nets
            .iter()
            .filter(move |t| self.length_of(t.net) < self.target - tol)
    }

    /// A net's length after matching, a pair leg's being its pair's mean.
    pub fn length_of(&self, net: u32) -> f64 {
        let after = |n: u32| {
            self.nets
                .iter()
                .find(|t| t.net == n)
                .map_or(0.0, |t| t.after)
        };
        match self.pairs.iter().find(|(p, n)| *p == net || *n == net) {
            Some(&(p, n)) => (after(p) + after(n)) / 2.0,
            None => after(net),
        }
    }
}

/// Brings every net of `nets` to `target` (or, with `None`, to the longest
/// of them) with meanders on its straight tracks, longest track first.
/// A meander must keep `clearance` from other copper and from the
/// meanders placed for the other nets; where a full one does not fit, a
/// lower one is tried, then the other side, then the next track. `amp` is
/// the tallest meander, `space` the step along the track (`None`: four
/// track widths).
pub fn match_lengths(
    board: &Board,
    nets: &[u32],
    target: Option<f64>,
    amp: f64,
    space: Option<f64>,
    clearance: f64,
) -> Matched {
    let pairs = pairs_in(board, nets);
    let paired: std::collections::HashSet<u32> = pairs.iter().flat_map(|&(p, n)| [p, n]).collect();
    let len_of = |n: u32| board.net_length(n).mil();
    let target = target.unwrap_or_else(|| {
        nets.iter()
            .filter(|n| !paired.contains(n))
            .map(|&n| len_of(n))
            .chain(pairs.iter().map(|&(p, n)| (len_of(p) + len_of(n)) / 2.0))
            .fold(0.0, f64::max)
    });
    let obstacles = Obstacles::new(board);
    let g = &board.geometry;
    // New copper so far: layer, ends, half width, net.
    let mut placed: Vec<(u8, P, P, f64, u32)> = Vec::new();
    // Tracks replaced and their meanders.
    let mut swaps: Vec<(usize, Vec<Piece>)> = Vec::new();
    let mut report = Vec::new();
    for &(p, n) in &pairs {
        let (lp, ln) = (len_of(p), len_of(n));
        let added = tune_pair(
            board,
            &obstacles,
            (p, n),
            target - (lp + ln) / 2.0,
            (lp - ln, amp, space, clearance),
            &mut placed,
            &mut swaps,
        );
        report.push(Tuned {
            net: p,
            before: lp,
            after: lp + added.0,
        });
        report.push(Tuned {
            net: n,
            before: ln,
            after: ln + added.1,
        });
    }
    let lengths: Vec<f64> = nets.iter().map(|&n| len_of(n)).collect();
    for (&net, &before) in nets.iter().zip(&lengths) {
        if paired.contains(&net) {
            continue;
        }
        let mut need = target - before;
        let mut tracks: Vec<(usize, u8, P, P, f64)> = g
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.net == Some(net) && !t.text && t.component.is_none())
            .filter_map(|(i, t)| match t.layer {
                LayerRef::Copper(l) => Some((
                    i,
                    l,
                    [t.start.x.mil(), t.start.y.mil()],
                    [t.end.x.mil(), t.end.y.mil()],
                    t.width.mil(),
                )),
                _ => None,
            })
            .collect();
        let len = |a: P, b: P| (b[0] - a[0]).hypot(b[1] - a[1]);
        tracks.sort_by(|x, y| len(y.2, y.3).total_cmp(&len(x.2, x.3)));
        for (ti, layer, a, b, w) in tracks {
            if need <= 0.5 {
                break;
            }
            let step = space.unwrap_or(4.0 * w).max(w + clearance);
            // Copper the track is already closer to than the clearance (a
            // pair's partner at the pair gap): a meander may stay as close
            // to it, no closer.
            let base = obstacles.check_track(layer, a, b, w, Some(net), clearance);
            let skip = [Some(net), base.as_ref().and_then(|h| h.net)];
            let floor = base.as_ref().map_or(clearance, |h| h.gap - 0.01);
            let mut best: Option<(Vec<Piece>, f64)> = None;
            'search: for k in [1.0, 0.7, 0.5, 0.35, 0.25] {
                let h = amp * k;
                if h < 2.0 * w {
                    break;
                }
                for flip in [false, true] {
                    let (pieces, added) = route::meander(a, b, need, h, step, flip);
                    if added <= 0.0 {
                        continue;
                    }
                    let clear = pieces.iter().all(|p| {
                        let (s, e) = (p.start(), p.end());
                        let own = obstacles.check_track(layer, s, e, w, Some(net), clearance);
                        own.is_none_or(|h| h.net == skip[1] && h.gap >= floor)
                            && obstacles
                                .check_pair_track(layer, s, e, w, &skip, clearance)
                                .is_none()
                            && placed.iter().all(|&(l, c, d, r, n)| {
                                l != layer
                                    || n == net
                                    || seg_seg(s, e, c, d) - r - w / 2.0 >= clearance - 1e-3
                            })
                    });
                    if clear {
                        best = Some((pieces, added));
                        break 'search;
                    }
                }
            }
            if let Some((pieces, added)) = best {
                for p in &pieces {
                    placed.push((layer, p.start(), p.end(), w / 2.0, net));
                }
                need -= added;
                swaps.push((ti, pieces));
            }
        }
        report.push(Tuned {
            net,
            before,
            after: target - need.max(0.0),
        });
    }
    // Remove the replaced tracks from the highest index down, then add the
    // meanders at the end.
    swaps.sort_by_key(|x| std::cmp::Reverse(x.0));
    let mut edits = Vec::new();
    for (ti, _) in &swaps {
        edits.push(Edit::RemoveTrack {
            index: *ti,
            track: g.tracks[*ti].clone(),
        });
    }
    let mut at = g.tracks.len() - swaps.len();
    let pt = |p: P| Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    };
    for (ti, pieces) in &swaps {
        let old = &g.tracks[*ti];
        for p in pieces {
            edits.push(Edit::InsertTrack {
                index: at,
                track: Track {
                    start: pt(p.start()),
                    end: pt(p.end()),
                    ..old.clone()
                },
            });
            at += 1;
        }
    }
    Matched {
        edits,
        nets: report,
        target,
        pairs,
    }
}

/// The differential pairs both of whose nets are in `nets`.
pub fn pairs_in(board: &Board, nets: &[u32]) -> Vec<(u32, u32)> {
    let id = |name: &str| {
        board
            .nets
            .iter()
            .position(|x| x.name == name)
            .map(|i| i as u32)
            .filter(|i| nets.contains(i))
    };
    board
        .diff_pairs
        .iter()
        .filter_map(|d| Some((id(&d.positive)?, id(&d.negative)?)))
        .collect()
}

/// Two tracks of a pair running side by side: the positive's index, the
/// negative's, the layer, the run along the positive track (from its
/// start, mil), the positive's direction, its start, and the signed
/// distance across to the negative (left positive).
struct Coupled {
    ip: usize,
    jn: usize,
    layer: u8,
    span: (f64, f64),
    u: P,
    a: P,
    d: f64,
}

fn coupled(board: &Board, p: u32, n: u32, used: &std::collections::HashSet<usize>) -> Vec<Coupled> {
    let g = &board.geometry;
    let line = |i: usize| {
        let t = &g.tracks[i];
        let l = match t.layer {
            LayerRef::Copper(l) => l,
            _ => return None,
        };
        (t.component.is_none() && !t.text).then_some((
            l,
            [t.start.x.mil(), t.start.y.mil()],
            [t.end.x.mil(), t.end.y.mil()],
            t.width.mil(),
        ))
    };
    let of = |net: u32| -> Vec<usize> {
        (0..g.tracks.len())
            .filter(|&i| g.tracks[i].net == Some(net) && !used.contains(&i))
            .collect()
    };
    let (ps, ns) = (of(p), of(n));
    let mut out = Vec::new();
    for &i in &ps {
        let Some((lp, a, b, wp)) = line(i) else {
            continue;
        };
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1.0 {
            continue;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let nrm = [-u[1], u[0]];
        let mut best: Option<Coupled> = None;
        for &j in &ns {
            let Some((ln, c, e, wn)) = line(j) else {
                continue;
            };
            if ln != lp {
                continue;
            }
            let along = |q: P| (q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1];
            let across = |q: P| (q[0] - a[0]) * nrm[0] + (q[1] - a[1]) * nrm[1];
            let (dc, de) = (across(c), across(e));
            // Parallel, and as close as a pair runs.
            if (dc - de).abs() > 0.05
                || dc.abs() < (wp + wn) / 2.0
                || dc.abs() > 3.0 * (wp + wn) + 20.0
            {
                continue;
            }
            let (t0, t1) = (along(c).min(along(e)), along(c).max(along(e)));
            let span = (t0.max(0.0), t1.min(len));
            if span.1 - span.0 < 1.0 {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|b| span.1 - span.0 > b.span.1 - b.span.0)
            {
                best = Some(Coupled {
                    ip: i,
                    jn: j,
                    layer: lp,
                    span,
                    u,
                    a,
                    d: dc,
                });
            }
        }
        out.extend(best);
    }
    out.sort_by(|x, y| (y.span.1 - y.span.0).total_cmp(&(x.span.1 - x.span.0)));
    out
}

/// Meanders a pair's coupled runs together to add `need` mil to its mean
/// length, keeping the legs at their gap and choosing the side that best
/// keeps (or improves) the pair's skew. Returns the length added to each
/// leg. `opts` is (skew now, positive minus negative; tallest meander;
/// step along the track; clearance).
fn tune_pair(
    board: &Board,
    obstacles: &Obstacles,
    (p, n): (u32, u32),
    mut need: f64,
    (mut skew, amp, space, clearance): (f64, f64, Option<f64>, f64),
    placed: &mut Vec<(u8, P, P, f64, u32)>,
    swaps: &mut Vec<(usize, Vec<Piece>)>,
) -> (f64, f64) {
    let g = &board.geometry;
    let mut used = std::collections::HashSet::new();
    let mut added = (0.0, 0.0);
    let skip = [Some(p), Some(n)];
    let pt = |c: &Coupled, s: f64, across: f64| {
        let nrm = [-c.u[1], c.u[0]];
        [
            c.a[0] + c.u[0] * s + nrm[0] * across,
            c.a[1] + c.u[1] * s + nrm[1] * across,
        ]
    };
    while need > 0.5 {
        let runs = coupled(board, p, n, &used);
        let mut done = false;
        for c in &runs {
            let (wp, wn) = (g.tracks[c.ip].width.mil(), g.tracks[c.jn].width.mil());
            let w = wp.max(wn);
            // Bumps wide enough that the inner leg's stays open.
            let step = space
                .unwrap_or(4.0 * w)
                .max(c.d.abs() + 2.0 * w + clearance);
            let (c0, c1) = (pt(c, c.span.0, c.d / 2.0), pt(c, c.span.1, c.d / 2.0));
            let mut best: Option<PairTry> = None;
            for k in [1.0, 0.7, 0.5, 0.35, 0.25] {
                let h = amp * k;
                if h < 2.0 * w + c.d.abs() {
                    break;
                }
                for flip in [false, true] {
                    let (centre, add) = route::meander(c0, c1, need, h, step, flip);
                    if add <= 0.0 {
                        continue;
                    }
                    // The positive runs at 0 across, the negative at d.
                    let lp = route::offset(&centre, -c.d / 2.0);
                    let ln = route::offset(&centre, c.d / 2.0);
                    let run = c.span.1 - c.span.0;
                    let sum = |v: &[Piece]| v.iter().map(Piece::length).sum::<f64>();
                    let (ap, an) = (sum(&lp) - run, sum(&ln) - run);
                    let clear = |leg: &[Piece], wl: f64, net: u32| {
                        leg.iter().all(|q| {
                            let (s, e) = (q.start(), q.end());
                            obstacles
                                .check_pair_track(c.layer, s, e, wl, &skip, clearance)
                                .is_none()
                                && placed.iter().all(|&(l, a, b, r, other)| {
                                    l != c.layer
                                        || other == net
                                        || other == p
                                        || other == n
                                        || seg_seg(s, e, a, b) - r - wl / 2.0 >= clearance - 1e-3
                                })
                        })
                    };
                    if !(clear(&lp, wp, p) && clear(&ln, wn, n)) {
                        continue;
                    }
                    let new_skew = skew + ap - an;
                    if best
                        .as_ref()
                        .is_none_or(|b| new_skew.abs() < (skew + b.3 - b.4).abs())
                    {
                        best = Some((add, lp, ln, ap, an));
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            let Some((_, lp, ln, ap, an)) = best else {
                continue;
            };
            // Whole new tracks: the untouched ends, then the legs.
            let tp = &g.tracks[c.ip];
            let tn = &g.tracks[c.jn];
            let ps = [tp.start.x.mil(), tp.start.y.mil()];
            let pe = [tp.end.x.mil(), tp.end.y.mil()];
            let mut newp = Vec::new();
            if c.span.0 > 1e-6 {
                newp.push(Piece::Line(ps, pt(c, c.span.0, 0.0)));
            }
            newp.extend(lp.iter().copied());
            if pt(c, c.span.1, 0.0) != pe {
                newp.push(Piece::Line(pt(c, c.span.1, 0.0), pe));
            }
            let along = |q: P| (q[0] - c.a[0]) * c.u[0] + (q[1] - c.a[1]) * c.u[1];
            let (ns, ne) = {
                let s = [tn.start.x.mil(), tn.start.y.mil()];
                let e = [tn.end.x.mil(), tn.end.y.mil()];
                if along(s) <= along(e) {
                    (s, e)
                } else {
                    (e, s)
                }
            };
            let mut newn = Vec::new();
            if along(ns) < c.span.0 - 1e-6 {
                newn.push(Piece::Line(ns, pt(c, c.span.0, c.d)));
            }
            newn.extend(ln.iter().copied());
            if along(ne) > c.span.1 + 1e-6 {
                newn.push(Piece::Line(pt(c, c.span.1, c.d), ne));
            }
            for q in &lp {
                placed.push((c.layer, q.start(), q.end(), wp / 2.0, p));
            }
            for q in &ln {
                placed.push((c.layer, q.start(), q.end(), wn / 2.0, n));
            }
            used.insert(c.ip);
            used.insert(c.jn);
            swaps.push((c.ip, newp));
            swaps.push((c.jn, newn));
            added.0 += ap;
            added.1 += an;
            skew += ap - an;
            need -= (ap + an) / 2.0;
            done = true;
            break;
        }
        if !done {
            break;
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerKind, Net, StackLayer};

    #[test]
    fn lanes_keep_pad_order_and_pitch() {
        // Pads in a column at x = 0, routed to the right: the top pad gets
        // the top lane.
        let mut s = vec![([0.0, 40.0], 2), ([0.0, -40.0], 0), ([0.0, 0.0], 1)];
        order(&mut s);
        let nets: Vec<u32> = s.iter().map(|x| x.1).collect();
        assert!(nets == vec![0, 1, 2] || nets == vec![2, 1, 0], "{nets:?}");
        let from: Vec<P> = s.iter().map(|x| x.0).collect();
        let centre = vec![Piece::Line([0.0, 0.0], [500.0, 0.0])];
        let sd = side(&centre, [0.0, 0.0], &from);
        let pitch = 10.0;
        let lanes: Vec<Vec<Piece>> = (0..3)
            .map(|i| lane(&centre, sd * slot(i, 3) * pitch, from[i], None, true))
            .collect();
        for (i, l) in lanes.iter().enumerate() {
            // Ends on its lane, in pad order.
            let end = l.last().unwrap().end();
            let want = slot(i, 3) * pitch * if from[0][1] < 0.0 { 1.0 } else { -1.0 };
            assert!((end[1] - want).abs() < 1e-6, "lane {i} ends at {end:?}");
            // The jog in is 45°.
            let j = l[0];
            let (a, b) = (j.start(), j.end());
            if (a[1] - b[1]).abs() > 1e-6 {
                assert!(((b[0] - a[0]).abs() - (b[1] - a[1]).abs()).abs() < 1e-6);
            }
        }
    }

    fn board() -> Board {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "L1".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length::from_mil(1.4),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
        b.nets = (0..3)
            .map(|i| Net {
                name: format!("D{i}"),
                ..Default::default()
            })
            .collect();
        let t = |net: u32, y: f64, x1: f64| Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(net),
            start: Point {
                x: Length::from_mil(0.0),
                y: Length::from_mil(y),
            },
            end: Point {
                x: Length::from_mil(x1),
                y: Length::from_mil(y),
            },
            width: Length::from_mil(5.0),
            text: false,
        };
        b.geometry.tracks = vec![t(0, 0.0, 1000.0), t(1, 200.0, 1200.0), t(2, 400.0, 1100.0)];
        crate::connectivity::analyse(&mut b);
        b
    }

    #[test]
    fn group_lengths_meet_the_longest() {
        let mut b = board();
        let m = match_lengths(&b, &[0, 1, 2], None, 40.0, None, 5.0);
        assert!((m.target - 1200.0).abs() < 1e-6);
        assert_eq!(m.short(1.0).count(), 0, "{:?}", m.nets);
        let mut h = crate::edit::History::default();
        h.apply(&mut b, "tune", m.edits);
        crate::connectivity::analyse(&mut b);
        for n in 0..3 {
            let l = b.net_length(n).mil();
            assert!((l - 1200.0).abs() < 1.0, "net {n}: {l}");
        }
    }

    #[test]
    fn meanders_keep_clear_of_a_close_neighbour() {
        // Net 0 has a neighbour 20 mil away: a 40 mil meander towards it
        // would hit, so it goes the other way or lower.
        let mut b = board();
        b.geometry.tracks[1].start.y = Length::from_mil(20.0);
        b.geometry.tracks[1].end.y = Length::from_mil(20.0);
        crate::connectivity::analyse(&mut b);
        let m = match_lengths(&b, &[0], Some(1100.0), 40.0, None, 5.0);
        let obs = Obstacles::new(&b);
        let mut checked = 0;
        for e in &m.edits {
            if let Edit::InsertTrack { track, .. } = e {
                let p = |q: Point| [q.x.mil(), q.y.mil()];
                assert!(obs
                    .check_track(0, p(track.start), p(track.end), 5.0, Some(0), 5.0)
                    .is_none());
                checked += 1;
            }
        }
        assert!(checked > 0);
        assert_eq!(m.short(1.0).count(), 0);
    }

    #[test]
    fn pairs_meander_together() {
        let mut b = board();
        // Nets 0 and 1 become a pair 8 mil apart (centre to centre, 5 mil
        // tracks), 1000 mil long; net 2 is 1200 long and far away.
        b.diff_pairs.push(crate::DiffPair {
            name: "D".into(),
            positive: "D0".into(),
            negative: "D1".into(),
        });
        for (i, y) in [(0usize, 0.0), (1, 8.0)] {
            let t = &mut b.geometry.tracks[i];
            t.start = Point::mil(0.0, y);
            t.end = Point::mil(1000.0, y);
        }
        b.geometry.tracks[2].start = Point::mil(0.0, 400.0);
        b.geometry.tracks[2].end = Point::mil(1200.0, 400.0);
        crate::connectivity::analyse(&mut b);
        let m = match_lengths(&b, &[0, 1, 2], None, 60.0, None, 5.0);
        assert_eq!(m.pairs, vec![(0, 1)]);
        assert!((m.target - 1200.0).abs() < 1e-6);
        assert_eq!(m.short(2.0).count(), 0, "{:?}", m.nets);
        let mut h = crate::edit::History::default();
        h.apply(&mut b, "tune", m.edits);
        crate::connectivity::analyse(&mut b);
        let (p, n) = (b.net_length(0).mil(), b.net_length(1).mil());
        assert!(((p + n) / 2.0 - 1200.0).abs() < 2.0, "{p} {n}");
        assert!((p - n).abs() < 10.0, "skew {}", p - n);
        // The legs keep their gap: no piece of one comes nearer the other
        // than the pair did (8 mil centre to centre).
        let legs = |net: u32| -> Vec<(P, P)> {
            b.geometry
                .tracks
                .iter()
                .filter(|t| t.net == Some(net))
                .map(|t| {
                    (
                        [t.start.x.mil(), t.start.y.mil()],
                        [t.end.x.mil(), t.end.y.mil()],
                    )
                })
                .collect()
        };
        for (a0, a1) in legs(0) {
            for &(b0, b1) in &legs(1) {
                assert!(seg_seg(a0, a1, b0, b1) > 8.0 - 0.01);
            }
        }
    }
}
