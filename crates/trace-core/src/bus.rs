//! Bus (multi-trace) routing: several nets routed side by side along one
//! centre line at a set spacing, and their lengths matched afterwards.
//! Units are mil.
//!
//! Each net is a lane: lanes are ordered across the route as their start
//! pads are, so tracks never cross. A lane is the centre line shifted
//! sideways; it joins its pad with a 45° jog where the pads sit wider or
//! narrower than the lanes, as a fan-out does.

use crate::edit::Edit;
use crate::route::{self, seg_seg, Obstacles, Piece, TuneShape, P};

/// A pair meander tried: its skew, the two legs and their lengths.
type PairTry = (f64, Vec<Piece>, Vec<Piece>, f64, f64);
/// Tracks to replace, each with the pieces that take its place.
pub type Swaps = Vec<(usize, Vec<Piece>)>;
use crate::{Arc, Board, LayerRef, Length, Point, Track};

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
pub fn match_lengths(board: &Board, nets: &[u32], target: Option<f64>, o: &TuneOpts) -> Matched {
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
            lp - ln,
            o,
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
    for &net in nets {
        if paired.contains(&net) {
            continue;
        }
        let before = len_of(net);
        let added = tune_net(
            board,
            &obstacles,
            net,
            target - before,
            o,
            None,
            &mut placed,
            &mut swaps,
        );
        report.push(Tuned {
            net,
            before,
            after: before + added,
        });
    }
    Matched {
        edits: swap_edits(board, &swaps),
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

/// Tuning settings: tallest bump, step along the track (`None`: four
/// track widths), clearance to other copper, and the bump shape.
#[derive(Clone, Copy, Debug)]
pub struct TuneOpts {
    pub amp: f64,
    pub space: Option<f64>,
    pub clearance: f64,
    pub shape: TuneShape,
}

/// A pair run meandered: the new positive and negative tracks (the run's
/// untouched ends included), the legs alone, and the length each gains.
struct PairRun {
    newp: Vec<Piece>,
    newn: Vec<Piece>,
    lp: Vec<Piece>,
    ln: Vec<Piece>,
    added: (f64, f64),
}

/// Meanders the coupled run `c` of pair (p, n) to add `need` to the pair's
/// mean length, trying the sides in `flips` and lower bumps where full ones
/// do not fit; of those that keep clear, the one that leaves the least skew
/// (`skew` now, positive minus negative) wins.
#[allow(clippy::too_many_arguments)]
fn pair_run(
    board: &Board,
    obstacles: &Obstacles,
    c: &Coupled,
    (p, n): (u32, u32),
    need: f64,
    skew: f64,
    o: &TuneOpts,
    placed: &[(u8, P, P, f64, u32)],
    flips: &[bool],
) -> Option<PairRun> {
    let g = &board.geometry;
    let skip = [Some(p), Some(n)];
    let pt = |s: f64, across: f64| {
        let nrm = [-c.u[1], c.u[0]];
        [
            c.a[0] + c.u[0] * s + nrm[0] * across,
            c.a[1] + c.u[1] * s + nrm[1] * across,
        ]
    };
    let (wp, wn) = (g.tracks[c.ip].width.mil(), g.tracks[c.jn].width.mil());
    let w = wp.max(wn);
    // Bumps wide enough that the inner leg's stays open.
    let step = o
        .space
        .unwrap_or(4.0 * w)
        .max(c.d.abs() + 2.0 * w + o.clearance);
    let (c0, c1) = (pt(c.span.0, c.d / 2.0), pt(c.span.1, c.d / 2.0));
    let mut best: Option<PairTry> = None;
    for k in [1.0, 0.7, 0.5, 0.35, 0.25] {
        let h = o.amp * k;
        if h < 2.0 * w + c.d.abs() {
            break;
        }
        for &flip in flips {
            let (centre, add) = route::tune_pattern(c0, c1, need, h, step, flip, o.shape);
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
                leg.iter().flat_map(Piece::chords).all(|(s, e)| {
                    obstacles
                        .check_pair_track(c.layer, s, e, wl, &skip, o.clearance)
                        .is_none()
                        && placed.iter().all(|&(l, a, b, r, other)| {
                            l != c.layer
                                || other == net
                                || other == p
                                || other == n
                                || seg_seg(s, e, a, b) - r - wl / 2.0 >= o.clearance - 1e-3
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
    let (_, lp, ln, ap, an) = best?;
    // Whole new tracks: the untouched ends, then the legs.
    let tp = &g.tracks[c.ip];
    let tn = &g.tracks[c.jn];
    let ps = [tp.start.x.mil(), tp.start.y.mil()];
    let pe = [tp.end.x.mil(), tp.end.y.mil()];
    let mut newp = Vec::new();
    if c.span.0 > 1e-6 {
        newp.push(Piece::Line(ps, pt(c.span.0, 0.0)));
    }
    newp.extend(lp.iter().copied());
    if pt(c.span.1, 0.0) != pe {
        newp.push(Piece::Line(pt(c.span.1, 0.0), pe));
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
        newn.push(Piece::Line(ns, pt(c.span.0, c.d)));
    }
    newn.extend(ln.iter().copied());
    if along(ne) > c.span.1 + 1e-6 {
        newn.push(Piece::Line(pt(c.span.1, c.d), ne));
    }
    Some(PairRun {
        newp,
        newn,
        lp,
        ln,
        added: (ap, an),
    })
}

/// Meanders a pair's coupled runs together to add `need` mil to its mean
/// length, keeping the legs at their gap and choosing the side that best
/// keeps (or improves) the pair's skew (`skew` now, positive minus
/// negative). Returns the length added to each leg.
#[allow(clippy::too_many_arguments)]
fn tune_pair(
    board: &Board,
    obstacles: &Obstacles,
    (p, n): (u32, u32),
    mut need: f64,
    mut skew: f64,
    o: &TuneOpts,
    placed: &mut Vec<(u8, P, P, f64, u32)>,
    swaps: &mut Vec<(usize, Vec<Piece>)>,
) -> (f64, f64) {
    let g = &board.geometry;
    let mut used = std::collections::HashSet::new();
    let mut added = (0.0, 0.0);
    while need > 0.5 {
        let runs = coupled(board, p, n, &used);
        let mut done = false;
        for c in &runs {
            let Some(r) = pair_run(
                board,
                obstacles,
                c,
                (p, n),
                need,
                skew,
                o,
                placed,
                &[false, true],
            ) else {
                continue;
            };
            let (wp, wn) = (g.tracks[c.ip].width.mil(), g.tracks[c.jn].width.mil());
            for (s, e) in r.lp.iter().flat_map(Piece::chords) {
                placed.push((c.layer, s, e, wp / 2.0, p));
            }
            for (s, e) in r.ln.iter().flat_map(Piece::chords) {
                placed.push((c.layer, s, e, wn / 2.0, n));
            }
            used.insert(c.ip);
            used.insert(c.jn);
            swaps.push((c.ip, r.newp));
            swaps.push((c.jn, r.newn));
            let (ap, an) = r.added;
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

/// Interactive pair tuning: meanders the coupled run of pair (p, n) that
/// track `track` (of either leg) belongs to, on the side `flip`, to add
/// `need` to the pair's mean length. Returns the tracks replaced with
/// their new pieces, and the length each leg gains.
pub fn tune_pair_at(
    board: &Board,
    obstacles: &Obstacles,
    (p, n): (u32, u32),
    track: usize,
    need: f64,
    flip: bool,
    o: &TuneOpts,
) -> Option<(Swaps, (f64, f64))> {
    let runs = coupled(board, p, n, &Default::default());
    let c = runs.iter().find(|c| c.ip == track || c.jn == track)?;
    let skew = board.net_length(p).mil() - board.net_length(n).mil();
    let r = pair_run(board, obstacles, c, (p, n), need, skew, o, &[], &[flip])?;
    Some((vec![(c.ip, r.newp), (c.jn, r.newn)], r.added))
}

/// Which side tuning bumps on track `track` go to keep away from the
/// tracks of net `partner` beside it: the `flip` for [`route::tune_pattern`],
/// or `None` when no partner track runs alongside.
pub fn away_from(board: &Board, track: usize, partner: u32) -> Option<bool> {
    let g = &board.geometry;
    let t = g.tracks.get(track)?;
    let (a, b) = (
        [t.start.x.mil(), t.start.y.mil()],
        [t.end.x.mil(), t.end.y.mil()],
    );
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len < 1e-6 {
        return None;
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let nrm = [-u[1], u[0]];
    let reach = 3.0 * t.width.mil() + 40.0;
    let mut best: Option<(f64, f64)> = None;
    for o in g
        .tracks
        .iter()
        .filter(|o| o.net == Some(partner) && o.layer == t.layer)
    {
        for q in [
            [o.start.x.mil(), o.start.y.mil()],
            [o.end.x.mil(), o.end.y.mil()],
        ] {
            let along = (q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1];
            let across = (q[0] - a[0]) * nrm[0] + (q[1] - a[1]) * nrm[1];
            let off = (-along).max(along - len).max(0.0);
            let d = across.abs() + off;
            if d < reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, across));
            }
        }
    }
    // Unflipped bumps go to the left (positive across).
    best.map(|(_, across)| across > 0.0)
}

/// Adds up to `need` mil to `net` with bumps on its straight tracks,
/// longest first, keeping clear of other copper and of `placed`. With
/// `partner` (a pair leg being phase tuned), bumps go away from the
/// partner first. Returns the length added.
#[allow(clippy::too_many_arguments)]
fn tune_net(
    board: &Board,
    obstacles: &Obstacles,
    net: u32,
    mut need: f64,
    o: &TuneOpts,
    partner: Option<u32>,
    placed: &mut Vec<(u8, P, P, f64, u32)>,
    swaps: &mut Vec<(usize, Vec<Piece>)>,
) -> f64 {
    let g = &board.geometry;
    let start = need;
    let taken: std::collections::HashSet<usize> = swaps.iter().map(|s| s.0).collect();
    let mut tracks: Vec<(usize, u8, P, P, f64)> = g
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            t.net == Some(net) && !t.text && t.component.is_none() && !taken.contains(i)
        })
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
        let step = o.space.unwrap_or(4.0 * w).max(w + o.clearance);
        // Copper the track is already closer to than the clearance (a
        // pair's partner at the pair gap): a meander may stay as close
        // to it, no closer.
        let base = obstacles.check_track(layer, a, b, w, Some(net), o.clearance);
        let skip = [Some(net), base.as_ref().and_then(|h| h.net)];
        let floor = base.as_ref().map_or(o.clearance, |h| h.gap - 0.01);
        let away = partner.and_then(|q| away_from(board, ti, q));
        let flips = match away {
            Some(f) => [f, !f],
            None => [false, true],
        };
        let mut best: Option<(Vec<Piece>, f64)> = None;
        'search: for k in [1.0, 0.7, 0.5, 0.35, 0.25] {
            let h = o.amp * k;
            if h < 2.0 * w {
                break;
            }
            for flip in flips {
                let (pieces, added) = route::tune_pattern(a, b, need, h, step, flip, o.shape);
                if added <= 0.0 {
                    continue;
                }
                let clear = pieces.iter().flat_map(Piece::chords).all(|(s, e)| {
                    let own = obstacles.check_track(layer, s, e, w, Some(net), o.clearance);
                    own.is_none_or(|h| h.net == skip[1] && h.gap >= floor)
                        && obstacles
                            .check_pair_track(layer, s, e, w, &skip, o.clearance)
                            .is_none()
                        && placed.iter().all(|&(l, c, d, r, n)| {
                            l != layer
                                || n == net
                                || seg_seg(s, e, c, d) - r - w / 2.0 >= o.clearance - 1e-3
                        })
                });
                if clear {
                    best = Some((pieces, added));
                    break 'search;
                }
            }
        }
        if let Some((pieces, added)) = best {
            for (s, e) in pieces.iter().flat_map(Piece::chords) {
                placed.push((layer, s, e, w / 2.0, net));
            }
            need -= added;
            swaps.push((ti, pieces));
        }
    }
    start - need
}

/// The first place where `pieces`, put in for net `net`'s straight track
/// a-b of width `w`, come closer to other copper than `clearance`. Copper
/// the track was already closer to (a pair partner at its gap) may stay
/// as close, no closer.
#[allow(clippy::too_many_arguments)]
pub fn bumps_hit(
    obstacles: &Obstacles,
    layer: u8,
    net: u32,
    (a, b): (P, P),
    w: f64,
    pieces: &[Piece],
    clearance: f64,
) -> Option<route::Hit> {
    let base = obstacles.check_track(layer, a, b, w, Some(net), clearance);
    let skip = [Some(net), base.as_ref().and_then(|h| h.net)];
    let floor = base.as_ref().map_or(clearance, |h| h.gap - 0.01);
    // Stretches still on the old track are where it was already.
    let on_track = |q: P| route::point_seg(q, a, b) < 1e-6;
    for (s, e) in pieces.iter().flat_map(Piece::chords) {
        if on_track(s) && on_track(e) {
            continue;
        }
        if let Some(h) = obstacles.check_track(layer, s, e, w, Some(net), clearance) {
            if !(h.net == skip[1] && h.gap >= floor) {
                return Some(h);
            }
        }
        if let Some(h) = obstacles.check_pair_track(layer, s, e, w, &skip, clearance) {
            return Some(h);
        }
    }
    None
}

/// Edits that replace each track of `swaps` with its new pieces (tracks,
/// and arcs for round corners) on the same layer, net and width: removals
/// from the highest index down, then the new copper at the end.
pub fn swap_edits(board: &Board, swaps: &[(usize, Vec<Piece>)]) -> Vec<Edit> {
    let g = &board.geometry;
    let mut swaps: Vec<&(usize, Vec<Piece>)> = swaps.iter().collect();
    swaps.sort_by_key(|x| std::cmp::Reverse(x.0));
    let mut edits = Vec::new();
    for (ti, _) in &swaps {
        edits.push(Edit::RemoveTrack {
            index: *ti,
            track: g.tracks[*ti].clone(),
        });
    }
    let mut at = g.tracks.len() - swaps.len();
    let mut ai = g.arcs.len();
    let pt = |p: P| Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    };
    for (ti, pieces) in &swaps {
        let old = &g.tracks[*ti];
        for p in pieces {
            match *p {
                Piece::Line(s, e) => {
                    edits.push(Edit::InsertTrack {
                        index: at,
                        track: Track {
                            start: pt(s),
                            end: pt(e),
                            ..old.clone()
                        },
                    });
                    at += 1;
                }
                Piece::Arc {
                    c,
                    r,
                    start_deg,
                    end_deg,
                    ..
                } => {
                    edits.push(Edit::InsertArc {
                        index: ai,
                        arc: Arc {
                            component: None,
                            layer: old.layer.clone(),
                            net: old.net,
                            center: pt(c),
                            radius: Length::from_mil(r),
                            start_deg,
                            end_deg,
                            width: old.width,
                        },
                    });
                    ai += 1;
                }
            }
        }
    }
    edits
}

/// Phase (skew) tuning of a differential pair: bumps on the shorter leg,
/// on the side away from its partner, until both legs are the same
/// length. Bumps keep `clearance` from other copper and never come closer
/// to the partner than the pair's gap.
pub fn phase_match(board: &Board, (p, n): (u32, u32), o: &TuneOpts) -> Matched {
    let (lp, ln) = (board.net_length(p).mil(), board.net_length(n).mil());
    let (short, long) = if lp < ln { (p, n) } else { (n, p) };
    let obstacles = Obstacles::new(board);
    let mut placed = Vec::new();
    let mut swaps = Vec::new();
    let added = tune_net(
        board,
        &obstacles,
        short,
        (lp - ln).abs(),
        o,
        Some(long),
        &mut placed,
        &mut swaps,
    );
    let before = |x: u32| board.net_length(x).mil();
    Matched {
        edits: swap_edits(board, &swaps),
        nets: vec![
            Tuned {
                net: p,
                before: lp,
                after: before(p) + if short == p { added } else { 0.0 },
            },
            Tuned {
                net: n,
                before: ln,
                after: before(n) + if short == n { added } else { 0.0 },
            },
        ],
        target: lp.max(ln),
        pairs: Vec::new(),
    }
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

    fn opts(amp: f64, clearance: f64) -> TuneOpts {
        TuneOpts {
            amp,
            space: None,
            clearance,
            shape: TuneShape::default(),
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
        let m = match_lengths(&b, &[0, 1, 2], None, &opts(40.0, 5.0));
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
        let m = match_lengths(&b, &[0], Some(1100.0), &opts(40.0, 5.0));
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
        let m = match_lengths(&b, &[0, 1, 2], None, &opts(60.0, 5.0));
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

    #[test]
    fn phase_tuning_bumps_the_short_leg_away_from_its_partner() {
        let mut b = board();
        b.diff_pairs.push(crate::DiffPair {
            name: "D".into(),
            positive: "D0".into(),
            negative: "D1".into(),
        });
        // D0 is 30 mil shorter than D1; the pair runs 8 mil apart.
        for (i, y, x1) in [(0usize, 0.0, 970.0), (1, 8.0, 1000.0)] {
            let t = &mut b.geometry.tracks[i];
            t.start = Point::mil(0.0, y);
            t.end = Point::mil(x1, y);
        }
        b.geometry.tracks[2].start = Point::mil(0.0, 400.0);
        b.geometry.tracks[2].end = Point::mil(1200.0, 400.0);
        crate::connectivity::analyse(&mut b);
        assert_eq!(away_from(&b, 0, 1), Some(true));
        for style in route::TuneStyle::ALL {
            let mut o = opts(25.0, 5.0);
            o.shape.style = style;
            o.shape.round = style == route::TuneStyle::Trombone;
            let m = phase_match(&b, (0, 1), &o);
            let mut c = b.clone();
            let mut h = crate::edit::History::default();
            h.apply(&mut c, "phase", m.edits);
            crate::connectivity::analyse(&mut c);
            let (p, n) = (c.net_length(0).mil(), c.net_length(1).mil());
            assert!((p - n).abs() < 1.0, "{style:?}: skew {}", p - n);
            assert!((n - 1000.0).abs() < 1e-6);
            // Every bump is below the pair (away from D1 at y = 8).
            for t in c.geometry.tracks.iter().filter(|t| t.net == Some(0)) {
                assert!(
                    t.start.y.mil() <= 1e-6 && t.end.y.mil() <= 1e-6,
                    "{style:?}"
                );
            }
        }
    }
}
