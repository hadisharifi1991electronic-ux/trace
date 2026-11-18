//! Push-and-shove for the interactive router.
//!
//! New copper (the route being drawn, or a via being placed) pushes the
//! tracks and vias of other nets out of its way, and what those run into
//! is pushed in turn, in a chain, as in Altium's Push and HugNPush modes.
//!
//! Tracks are moved as whole lines: the run of segments of one net on one
//! layer between pads, vias, junctions and width changes. Only the part of
//! a line near what pushes it is routed again, at 0°/45°/90°, on a fine
//! grid so it hugs the copper it goes round, and its ends stay where they
//! are. A line of a differential pair is moved together with its partner,
//! so the pair keeps its gap. A via is moved the least distance that clears
//! what pushes it, and the tracks that end on it follow. Pads, arcs and
//! holes never move: a push that needs them to fails.
//!
//! Everything moved keeps the board clearance and each net's own rule
//! (checked once more at the end), or nothing is moved.

use crate::edit::Edit;
use crate::route::{seg_seg, Item, Obstacles, P};
use crate::walk::{self, Ask, Capsule};
use crate::{Board, LayerRef, Length, Point, Track};
use std::collections::{HashMap, HashSet, VecDeque};

/// Copper that pushes, on copper layers `from..=to`: a capsule from `a` to
/// `b` of radius `r` (a point for a via), carrying one of `nets`.
#[derive(Clone, Debug)]
pub struct Pusher {
    pub from: u8,
    pub to: u8,
    pub a: P,
    pub b: P,
    pub r: f64,
    pub nets: Vec<Option<u32>>,
}

impl Pusher {
    /// A piece of track on one layer.
    pub fn track(layer: u8, a: P, b: P, r: f64, nets: &[Option<u32>]) -> Self {
        Pusher {
            from: layer,
            to: layer,
            a,
            b,
            r,
            nets: nets.to_vec(),
        }
    }
    /// A via of diameter `d` at `c` on layers `from..=to`.
    pub fn via(c: P, d: f64, from: u8, to: u8, nets: &[Option<u32>]) -> Self {
        Pusher {
            from,
            to,
            a: c,
            b: c,
            r: d / 2.0,
            nets: nets.to_vec(),
        }
    }
    fn on(&self, l: u8) -> bool {
        (self.from..=self.to).contains(&l)
    }
}

/// What a push moves: board tracks taken out and their new shapes, and
/// vias moved to new centres.
#[derive(Clone, Debug, Default)]
pub struct Pushed {
    pub remove: Vec<usize>,
    pub add: Vec<Track>,
    pub vias: Vec<(usize, P)>,
}

impl Pushed {
    /// How many tracks (lines of track) and vias moved.
    pub fn counts(&self) -> (usize, usize) {
        (self.remove.len(), self.vias.len())
    }

    /// The edits, for a board that will have `tracks` tracks when they
    /// start (after the route's own are in): removals, then the moved
    /// copies, then the vias.
    pub fn edits(&self, board: &Board, tracks: usize) -> Vec<Edit> {
        let mut out = Vec::new();
        let mut remove = self.remove.clone();
        remove.sort_unstable_by(|a, b| b.cmp(a));
        remove.dedup();
        for &i in &remove {
            out.push(Edit::RemoveTrack {
                index: i,
                track: board.geometry.tracks[i].clone(),
            });
        }
        for (at, t) in (tracks - remove.len()..).zip(&self.add) {
            out.push(Edit::InsertTrack {
                index: at,
                track: t.clone(),
            });
        }
        for &(i, p) in &self.vias {
            let old = board.geometry.vias[i].clone();
            let mut new = old.clone();
            new.at = point(p);
            out.push(Edit::RemoveVia { index: i, via: old });
            out.push(Edit::InsertVia { index: i, via: new });
        }
        out
    }
}

/// Pushes the tracks and vias of other nets out of the way of `pushers`,
/// and what they run into after them. `keep` is more copper that must be
/// kept clear of but does not push (the part of a route already fixed).
/// `None` when something that cannot move is in the way, or the copper
/// cannot all be made to fit.
pub fn push(
    board: &Board,
    obstacles: &Obstacles,
    clearance: f64,
    pushers: &[Pusher],
    keep: &[Pusher],
) -> Option<Pushed> {
    let mut s = State::new(board, obstacles, clearance, pushers, keep);
    s.run(pushers)?;
    s.verify()?;
    Some(s.result())
}

/// Copper layers a via is on.
fn via_span(board: &Board, v: &crate::Via) -> (u8, u8) {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    (
        v.from_layer.min(v.to_layer),
        v.from_layer.max(v.to_layer).min(n - 1),
    )
}

fn via_radius(v: &crate::Via) -> f64 {
    let ring = v.rings.iter().map(|r| r.1.mil()).fold(0.0, f64::max);
    v.diameter.mil().max(ring).max(v.hole.mil()) / 2.0
}

fn point(p: P) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

fn pt(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn key(p: P) -> (i64, i64) {
    ((p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64)
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The point of segment a-b nearest `p`, and how far along it is (0..1).
fn foot(p: P, a: P, b: P) -> (P, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 1e-12 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ([a[0] + t * dx, a[1] + t * dy], t)
}

/// How far along a polyline the point nearest `p` is, and that point.
fn project(pts: &[P], p: P) -> (f64, P) {
    let mut best = (f64::MAX, 0.0, p);
    let mut run = 0.0;
    for w in pts.windows(2) {
        let (q, t) = foot(p, w[0], w[1]);
        let len = dist(w[0], w[1]);
        let d = dist(p, q);
        if d < best.0 {
            best = (d, run + t * len, q);
        }
        run += len;
    }
    (best.1, best.2)
}

/// Drops repeated points and points in the middle of a straight run.
fn simplify(pts: Vec<P>) -> Vec<P> {
    let mut out: Vec<P> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last().is_some_and(|&q| dist(p, q) < 1e-6) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let cross = (b[0] - a[0]) * (p[1] - b[1]) - (b[1] - a[1]) * (p[0] - b[0]);
            let dot = (b[0] - a[0]) * (p[0] - b[0]) + (b[1] - a[1]) * (p[1] - b[1]);
            if cross.abs() < 1e-6 * (1.0 + dist(a, b) * dist(b, p)) && dot > 0.0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// A run of track of one net on one layer, moved as one.
#[derive(Clone)]
struct Line {
    tracks: Vec<usize>,
    layer: u8,
    net: Option<u32>,
    w: f64,
    /// Its shape now: the board's until it is moved.
    pts: Vec<P>,
    /// The via each end sits on, if any.
    ends: [Option<usize>; 2],
    moves: u32,
    /// The board's segments, to tell new copper from what was there.
    orig: Vec<(P, P)>,
}

impl Line {
    /// Segments that are not on the board as they are: only these can run
    /// into anything new.
    fn new_segs(&self) -> Vec<(P, P)> {
        self.segs().filter(|&(a, b)| !self.is_old(a, b)).collect()
    }

    /// Whether a-b lies along a segment of the board's (all of it, or
    /// the part left when the line was split).
    fn is_old(&self, a: P, b: P) -> bool {
        self.orig.iter().any(|&(c, d)| {
            let on = |p: P| dist(foot(p, c, d).0, p) < 1e-3;
            on(a) && on(b)
        })
    }
    fn r(&self) -> f64 {
        self.w / 2.0
    }
    fn segs(&self) -> impl Iterator<Item = (P, P)> + '_ {
        self.pts.windows(2).map(|w| (w[0], w[1]))
    }
}

/// Lines are moved at most this many times, and the chain stops after
/// this many lines and vias, so a push never runs away across the board.
const MAX_MOVES: u32 = 4;
const MAX_LINES: usize = 64;
const MAX_VIAS: usize = 32;
const MAX_STEPS: usize = 600;
/// A push is worked out as the cursor moves: one that takes longer than
/// this is given up rather than make the route lag.
const BUDGET: std::time::Duration = std::time::Duration::from_millis(120);

struct State<'a> {
    b: &'a Board,
    obs: &'a Obstacles,
    clearance: f64,
    /// The pushing copper and the copper kept, which never move.
    fixed: Vec<Pusher>,
    lines: Vec<Line>,
    line_of: HashMap<usize, usize>,
    /// Vias moved: new centre and how often.
    vias: HashMap<usize, (P, u32)>,
    /// Track ends by layer and point.
    ends_at: HashMap<(u8, (i64, i64)), Vec<usize>>,
    /// Vias by centre.
    via_at: HashMap<(i64, i64), Vec<usize>>,
    /// Points where a line must stop: pad centres and arc ends, by layer
    /// (`u8::MAX` for every layer).
    stops: HashSet<(u8, (i64, i64))>,
    partner: HashMap<u32, u32>,
    deadline: std::time::Instant,
}

impl<'a> State<'a> {
    fn late(&self) -> bool {
        std::time::Instant::now() > self.deadline
    }

    fn new(
        b: &'a Board,
        obs: &'a Obstacles,
        clearance: f64,
        pushers: &[Pusher],
        keep: &[Pusher],
    ) -> Self {
        let g = &b.geometry;
        let mut ends_at: HashMap<(u8, (i64, i64)), Vec<usize>> = HashMap::new();
        for (i, t) in g.tracks.iter().enumerate() {
            if let LayerRef::Copper(l) = t.layer {
                ends_at.entry((l, key(pt(t.start)))).or_default().push(i);
                ends_at.entry((l, key(pt(t.end)))).or_default().push(i);
            }
        }
        let mut via_at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, v) in g.vias.iter().enumerate() {
            via_at.entry(key(pt(v.at))).or_default().push(i);
        }
        let mut stops = HashSet::new();
        for p in &g.pads {
            let l = match p.layer {
                LayerRef::Copper(l) if p.hole.0 == 0 && p.stack.is_empty() => l,
                _ => u8::MAX,
            };
            stops.insert((l, key(pt(p.at))));
        }
        for a in &g.arcs {
            if let LayerRef::Copper(l) = a.layer {
                let (c, r) = (pt(a.center), a.radius.mil());
                for d in [a.start_deg, a.end_deg] {
                    let t = d.to_radians();
                    stops.insert((l, key([c[0] + r * t.cos(), c[1] + r * t.sin()])));
                }
            }
        }
        let by_name: HashMap<&str, u32> = b
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.as_str(), i as u32))
            .collect();
        let mut partner = HashMap::new();
        for d in &b.diff_pairs {
            if let (Some(&p), Some(&n)) = (
                by_name.get(d.positive.as_str()),
                by_name.get(d.negative.as_str()),
            ) {
                partner.insert(p, n);
                partner.insert(n, p);
            }
        }
        let mut fixed = pushers.to_vec();
        fixed.extend_from_slice(keep);
        State {
            b,
            obs,
            clearance,
            fixed,
            lines: Vec::new(),
            line_of: HashMap::new(),
            vias: HashMap::new(),
            ends_at,
            via_at,
            stops,
            partner,
            deadline: std::time::Instant::now() + BUDGET,
        }
    }

    /// The clearance copper of `net` asks of everything near it.
    fn gap(&self, net: Option<u32>) -> f64 {
        self.obs.net_gap(net, self.clearance)
    }

    /// The nets a line of `net` need not keep clear of: its own, and its
    /// partner's when it is a pair net (the pair gap is its own rule).
    fn friends(&self, net: Option<u32>) -> Vec<Option<u32>> {
        let mut v = vec![net];
        if let Some(p) = net.and_then(|n| self.partner.get(&n)) {
            v.push(Some(*p));
        }
        v
    }

    fn via_pos(&self, v: usize) -> P {
        self.vias
            .get(&v)
            .map(|x| x.0)
            .unwrap_or_else(|| pt(self.b.geometry.vias[v].at))
    }

    fn line_moved(&self, t: usize) -> bool {
        self.line_of
            .get(&t)
            .is_some_and(|&li| self.lines[li].moves > 0)
    }

    /// The line track `t` is part of, found once.
    fn line(&mut self, t: usize) -> usize {
        if let Some(&li) = self.line_of.get(&t) {
            return li;
        }
        let g = &self.b.geometry;
        let first = &g.tracks[t];
        let LayerRef::Copper(layer) = first.layer else {
            unreachable!("only copper tracks are obstacles")
        };
        let (net, width) = (first.net, first.width);
        let mut tracks = vec![t];
        let mut pts = vec![pt(first.start), pt(first.end)];
        // Walk on from the end, then from the start.
        for forward in [true, false] {
            loop {
                if tracks.len() > 400 {
                    break;
                }
                let at = if forward {
                    *pts.last().unwrap()
                } else {
                    pts[0]
                };
                let k = key(at);
                if self.stops.contains(&(layer, k)) || self.stops.contains(&(u8::MAX, k)) {
                    break;
                }
                if self
                    .via_at
                    .get(&k)
                    .is_some_and(|v| v.iter().any(|&i| g.vias[i].net == net))
                {
                    break;
                }
                let here = &self.ends_at[&(layer, k)];
                let same: Vec<usize> = here
                    .iter()
                    .copied()
                    .filter(|&i| g.tracks[i].net == net)
                    .collect();
                if same.len() != 2 {
                    break;
                }
                let cur = if forward {
                    *tracks.last().unwrap()
                } else {
                    tracks[0]
                };
                let Some(&next) = same.iter().find(|&&i| i != cur) else {
                    break;
                };
                let nt = &g.tracks[next];
                if tracks.contains(&next) || nt.width != width || self.line_of.contains_key(&next) {
                    break;
                }
                let (s, e) = (pt(nt.start), pt(nt.end));
                let other = if key(s) == k { e } else { s };
                if forward {
                    tracks.push(next);
                    pts.push(other);
                } else {
                    tracks.insert(0, next);
                    pts.insert(0, other);
                }
            }
        }
        let via_on = |p: P| {
            self.via_at.get(&key(p)).and_then(|v| {
                v.iter().copied().find(|&i| {
                    let (lo, hi) = via_span(self.b, &g.vias[i]);
                    g.vias[i].net == net && (lo..=hi).contains(&layer)
                })
            })
        };
        let ends = [via_on(pts[0]), via_on(*pts.last().unwrap())];
        let li = self.lines.len();
        for &i in &tracks {
            self.line_of.insert(i, li);
        }
        let orig = tracks
            .iter()
            .map(|&i| (pt(g.tracks[i].start), pt(g.tracks[i].end)))
            .collect();
        self.lines.push(Line {
            orig,
            tracks,
            layer,
            net,
            w: width.mil(),
            pts,
            ends,
            moves: 0,
        });
        li
    }

    /// Copper on `layer` that a line of `nets` (needing `need`) must keep
    /// clear of and cannot push: what pushes, what is kept, and everything
    /// moved, as capsules grown by any wider rule of their own nets.
    fn hard(
        &self,
        layer: u8,
        nets: &[Option<u32>],
        need: f64,
        skip_line: &[usize],
    ) -> Vec<Capsule> {
        let mut out = Vec::new();
        let grow = |net: Option<u32>| (self.gap(net) - need).max(0.0);
        for p in &self.fixed {
            if p.on(layer) && !p.nets.iter().any(|n| nets.contains(n)) {
                let g = p.nets.iter().map(|&n| grow(n)).fold(0.0, f64::max);
                out.push((p.a, p.b, p.r + g));
            }
        }
        for (&v, &(c, _)) in &self.vias {
            let via = &self.b.geometry.vias[v];
            let (lo, hi) = via_span(self.b, via);
            if (lo..=hi).contains(&layer) && !nets.contains(&via.net) {
                out.push((c, c, via_radius(via) + grow(via.net)));
            }
        }
        for (li, l) in self.lines.iter().enumerate() {
            if l.moves == 0 || l.layer != layer || nets.contains(&l.net) || skip_line.contains(&li)
            {
                continue;
            }
            let g = grow(l.net);
            out.extend(l.segs().map(|(a, b)| (a, b, l.r() + g)));
        }
        out
    }

    /// What `p` runs into that has to move: unmoved tracks (as lines) and
    /// vias of the board, and moved lines and vias. `None` when something
    /// that cannot move is in the way.
    fn victims(&mut self, p: &Pusher) -> Option<(Vec<usize>, Vec<usize>)> {
        let mut lines = Vec::new();
        let mut vias = Vec::new();
        let need = p
            .nets
            .iter()
            .map(|&n| self.gap(n))
            .fold(self.clearance, f64::max);
        let mut fixed = false;
        let mut tracks = Vec::new();
        for l in p.from..=p.to {
            self.obs.hits(
                Some(l),
                p.a,
                p.b,
                p.r,
                &p.nets,
                need,
                &mut |i, _, _| match i {
                    Item::Track(t) => tracks.push(t),
                    Item::Via(v) => vias.push(v),
                    Item::Fixed => fixed = true,
                },
            );
            for (li, line) in self.lines.iter().enumerate() {
                if line.moves == 0 || line.layer != l || p.nets.contains(&line.net) {
                    continue;
                }
                let need = need.max(self.gap(line.net));
                if line
                    .segs()
                    .any(|(a, b)| seg_seg(a, b, p.a, p.b) - line.r() - p.r < need - 1e-3)
                {
                    lines.push(li);
                }
            }
        }
        if fixed {
            return None;
        }
        for t in tracks {
            if !self.line_moved(t) {
                let li = self.line(t);
                lines.push(li);
            }
        }
        // Moved vias are seen where they are now, not on the board.
        vias.retain(|v| !self.vias.contains_key(v));
        for (&v, &(c, _)) in &self.vias {
            let via = &self.b.geometry.vias[v];
            let (lo, hi) = via_span(self.b, via);
            if hi < p.from || lo > p.to || p.nets.contains(&via.net) {
                continue;
            }
            let need = need.max(self.gap(via.net));
            if seg_seg(c, c, p.a, p.b) - via_radius(via) - p.r < need - 1e-3 {
                vias.push(v);
            }
        }
        lines.sort_unstable();
        lines.dedup();
        vias.sort_unstable();
        vias.dedup();
        Some((lines, vias))
    }

    fn run(&mut self, pushers: &[Pusher]) -> Option<()> {
        let mut queue: VecDeque<Pusher> = pushers.iter().cloned().collect();
        let mut steps = 0;
        while let Some(p) = queue.pop_front() {
            steps += 1;
            if steps > MAX_STEPS || self.late() {
                return None;
            }
            let (lines, vias) = self.victims(&p)?;
            let mut dirty = Vec::new();
            for v in vias {
                let moved = self.push_via(v, &p)?;
                if let Some(np) = moved {
                    let via = &self.b.geometry.vias[v];
                    let (lo, hi) = via_span(self.b, via);
                    queue.push_back(Pusher::via(np, via_radius(via) * 2.0, lo, hi, &[via.net]));
                    dirty.extend(self.lines_on_via(v));
                }
            }
            for li in lines.into_iter().chain(dirty) {
                let moved = self.reroute(li)?;
                for m in moved {
                    let l = &self.lines[m];
                    let nets = self.friends(l.net);
                    for (a, b) in l.new_segs() {
                        queue.push_back(Pusher::track(l.layer, a, b, l.r(), &nets));
                    }
                }
            }
            if self.lines.iter().filter(|l| l.moves > 0).count() > MAX_LINES
                || self.vias.len() > MAX_VIAS
            {
                return None;
            }
        }
        Some(())
    }

    /// Lines that end on via `v` (built from the board if need be).
    fn lines_on_via(&mut self, v: usize) -> Vec<usize> {
        let via = &self.b.geometry.vias[v];
        let (lo, hi) = via_span(self.b, via);
        let k = key(pt(via.at));
        let net = via.net;
        let mut out = Vec::new();
        for l in lo..=hi {
            let ts: Vec<usize> = self
                .ends_at
                .get(&(l, k))
                .map(|v| v.to_vec())
                .unwrap_or_default();
            for t in ts {
                if self.b.geometry.tracks[t].net == net {
                    out.push(self.line(t));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Moves via `v` the least distance that clears `p`, to a spot clear
    /// of everything that cannot move. `Some(None)` when it is already
    /// clear, `None` when there is no such spot.
    fn push_via(&mut self, v: usize, p: &Pusher) -> Option<Option<P>> {
        let via = &self.b.geometry.vias[v];
        let (lo, hi) = via_span(self.b, via);
        let rv = via_radius(via);
        let c = self.via_pos(v);
        let need = p
            .nets
            .iter()
            .map(|&n| self.gap(n))
            .fold(self.gap(via.net), f64::max);
        let (q, _) = foot(c, p.a, p.b);
        let d = dist(c, q);
        let req = p.r + rv + need + 0.05;
        if d >= req - 1e-3 {
            return Some(None);
        }
        let times = self.vias.get(&v).map_or(0, |x| x.1);
        if times >= MAX_MOVES {
            return None;
        }
        let dir = if d > 1e-6 {
            [(c[0] - q[0]) / d, (c[1] - q[1]) / d]
        } else {
            let (dx, dy) = (p.b[0] - p.a[0], p.b[1] - p.a[1]);
            let l = dx.hypot(dy);
            if l > 1e-9 {
                [-dy / l, dx / l]
            } else {
                [1.0, 0.0]
            }
        };
        let nets = [via.net];
        let mut hard: Vec<(u8, Vec<Capsule>)> = Vec::new();
        for l in lo..=hi {
            hard.push((l, self.hard(l, &nets, self.gap(via.net), &[])));
        }
        let fits = |at: P| -> bool {
            if seg_seg(at, at, p.a, p.b) - p.r - rv < need - 1e-3 {
                return false;
            }
            let skip = |i: Item| matches!(i, Item::Track(_) | Item::Via(_));
            for (l, caps) in &hard {
                if !self
                    .obs
                    .clear_items(Some(*l), at, at, rv, &nets, self.gap(via.net), &skip)
                {
                    return false;
                }
                let need = self.gap(via.net);
                if caps
                    .iter()
                    .any(|&(a, b, r)| seg_seg(at, at, a, b) - r - rv < need - 1e-3)
                {
                    return false;
                }
            }
            true
        };
        let mut best: Option<(f64, P)> = None;
        for ang in [0.0f64, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0, 135.0, -135.0] {
            let (s, co) = ang.to_radians().sin_cos();
            let dr = [dir[0] * co - dir[1] * s, dir[0] * s + dir[1] * co];
            for k in 0..8 {
                let far = req + k as f64 * req * 0.25;
                let at = [q[0] + dr[0] * far, q[1] + dr[1] * far];
                let moved = dist(at, c);
                if best.is_some_and(|b| b.0 <= moved) || moved > 4.0 * req {
                    continue;
                }
                if fits(at) {
                    best = Some((moved, at));
                    break;
                }
            }
        }
        let (_, at) = best?;
        self.vias.insert(v, (at, times + 1));
        Some(Some(at))
    }

    /// Routes line `li` (and its pair partner) again around everything
    /// that cannot move, keeping its ends. Returns the lines moved.
    fn reroute(&mut self, li: usize) -> Option<Vec<usize>> {
        if self.lines[li].moves >= MAX_MOVES || self.late() {
            return None;
        }
        // Ends follow moved vias.
        for e in 0..2 {
            if let Some(v) = self.lines[li].ends[e] {
                let at = self.via_pos(v);
                let l = &mut self.lines[li];
                if e == 0 {
                    l.pts[0] = at;
                } else {
                    *l.pts.last_mut().unwrap() = at;
                }
            }
        }
        if let Some(pi) = self.partner_line(li) {
            if let Some(done) = self.reroute_pair(li, pi) {
                return Some(done);
            }
        }
        self.reroute_one(li).map(|()| vec![li])
    }

    /// The stretch of a line (arc length from, to) that has to move: where
    /// it breaks clearance to hard copper or what cannot move, with room
    /// either side; ends on a moved via are always included.
    fn window(&self, li: usize, hard: &[Capsule], need: f64) -> Option<(f64, f64)> {
        let l = &self.lines[li];
        let r = l.r();
        let nets = self.friends(l.net);
        let skip = |i: Item| matches!(i, Item::Track(_) | Item::Via(_));
        let mut cum = vec![0.0];
        for (a, b) in l.segs() {
            cum.push(cum.last().unwrap() + dist(a, b));
        }
        let total = *cum.last().unwrap();
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        let reach = r + need;
        for (i, (a, b)) in l.segs().enumerate() {
            let len = cum[i + 1] - cum[i];
            // A board segment too close to a pad already was drawn that
            // way: only new copper must clear what cannot move.
            if !l.is_old(a, b)
                && !self
                    .obs
                    .clear_items(Some(l.layer), a, b, r, &nets, need, &skip)
            {
                lo = lo.min(cum[i]);
                hi = hi.max(cum[i + 1]);
                continue;
            }
            for &(c, d, rr) in hard {
                if seg_seg(a, b, c, d) - rr - r >= need - 1e-3 {
                    continue;
                }
                let (_, t0) = foot(c, a, b);
                let (_, t1) = foot(d, a, b);
                let ext = rr + reach;
                lo = lo.min(cum[i] + t0.min(t1) * len - ext);
                hi = hi.max(cum[i] + t0.max(t1) * len + ext);
            }
        }
        for e in 0..2 {
            if l.ends[e].is_some_and(|v| self.vias.contains_key(&v)) {
                let m = 4.0 * reach + 10.0;
                if e == 0 {
                    lo = lo.min(0.0);
                    hi = hi.max(m.min(total));
                } else {
                    lo = lo.min((total - m).max(0.0));
                    hi = hi.max(total);
                }
            }
        }
        if lo > hi {
            return None;
        }
        Some((lo.max(0.0), hi.min(total)))
    }

    /// The point at arc length `s` along a line, and the index of the
    /// segment it is on.
    fn at(pts: &[P], s: f64) -> (usize, P) {
        let mut run = 0.0;
        for i in 0..pts.len() - 1 {
            let len = dist(pts[i], pts[i + 1]);
            if run + len >= s - 1e-9 || i + 2 == pts.len() {
                let t = if len > 1e-12 {
                    ((s - run) / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (a, b) = (pts[i], pts[i + 1]);
                return (i, [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
            run += len;
        }
        (0, pts[0])
    }

    /// Whether a point of a line of radius `r` is clear of hard copper and
    /// what cannot move.
    fn spot_clear(
        &self,
        layer: u8,
        at: P,
        r: f64,
        nets: &[Option<u32>],
        need: f64,
        hard: &[Capsule],
    ) -> bool {
        let skip = |i: Item| matches!(i, Item::Track(_) | Item::Via(_));
        self.obs
            .clear_items(Some(layer), at, at, r, nets, need, &skip)
            && hard
                .iter()
                .all(|&(a, b, rr)| seg_seg(at, at, a, b) - rr - r >= need - 1e-3)
    }

    /// Picks the stretch to move, grown until both its ends are clear (or
    /// it reaches the line's ends), as split points (arc lengths).
    fn grow_window(
        &self,
        li: usize,
        hard: &[Capsule],
        need: f64,
        r: f64,
        nets: &[Option<u32>],
        m0: f64,
    ) -> Option<Option<(f64, f64)>> {
        let Some((lo, hi)) = self.window(li, hard, need) else {
            return Some(None);
        };
        let l = &self.lines[li];
        let total: f64 = l.segs().map(|(a, b)| dist(a, b)).sum();
        let mut m = m0;
        for _ in 0..6 {
            let (a, b) = ((lo - m).max(0.0), (hi + m).min(total));
            let ok = |s: f64| {
                let (_, p) = Self::at(&l.pts, s);
                self.spot_clear(l.layer, p, r, nets, need, hard)
            };
            let (ea, eb) = (a <= 1e-9 || ok(a), b >= total - 1e-9 || ok(b));
            if ea && eb {
                // The line's own ends never move: they must be clear.
                if (a <= 1e-9 && !ok(0.0)) || (b >= total - 1e-9 && !ok(total)) {
                    return None;
                }
                return Some(Some((a, b)));
            }
            m *= 2.0;
        }
        None
    }

    /// Splits `pts` at arc lengths `a` < `b`: before, the stretch's ends,
    /// after.
    fn split(pts: &[P], a: f64, b: f64) -> (Vec<P>, P, P, Vec<P>) {
        let (ia, pa) = Self::at(pts, a);
        let (ib, pb) = Self::at(pts, b);
        let mut before: Vec<P> = pts[..=ia].to_vec();
        before.push(pa);
        let mut after = vec![pb];
        after.extend_from_slice(&pts[ib + 1..]);
        (before, pa, pb, after)
    }

    /// Margins tried either side of where a line breaks clearance, least
    /// first, so a moved line leaves its old path as late as it can and
    /// hugs what pushed it.
    fn margins(r: f64, need: f64) -> [f64; 3] {
        [2.0, 3.0 * (r + need) + 10.0, 8.0 * (r + need) + 30.0]
    }

    fn reroute_one(&mut self, li: usize) -> Option<()> {
        let (layer, net, r) = {
            let l = &self.lines[li];
            (l.layer, l.net, l.r())
        };
        let need = self.gap(net);
        let nets = self.friends(net);
        let hard = self.hard(layer, &nets, need, &[li]);
        for m0 in Self::margins(r, need) {
            let Some((a, b)) = self.grow_window(li, &hard, need, r, &nets, m0)? else {
                // Nothing to move after all (its ends followed a via, say).
                self.lines[li].moves += 1;
                return Some(());
            };
            let (before, pa, pb, after) = Self::split(&self.lines[li].pts, a, b);
            let ask = Ask {
                obstacles: self.obs,
                layer,
                r,
                nets: &nets,
                clearance: need,
                extra: &hard,
                skip: &[],
                see_through: true,
                deadline: Some(self.deadline),
            };
            let Some(path) = walk::walkaround(&ask, pa, pb) else {
                continue;
            };
            let mut pts = before;
            pts.extend(path);
            pts.extend(after);
            let l = &mut self.lines[li];
            l.pts = simplify(pts);
            l.moves += 1;
            return Some(());
        }
        None
    }

    /// The partner's line running beside line `li`, for a pair net.
    fn partner_line(&mut self, li: usize) -> Option<usize> {
        let (layer, net, r) = {
            let l = &self.lines[li];
            (l.layer, l.net, l.r())
        };
        let m = *self.partner.get(&net?)?;
        let reach = 4.0 * r + 2.0 * self.gap(net) + 10.0;
        let mut count: HashMap<usize, usize> = HashMap::new();
        let segs: Vec<(P, P)> = self.lines[li].segs().collect();
        let others: Vec<Option<u32>> = vec![net];
        for (a, b) in segs {
            self.obs
                .hits(Some(layer), a, b, r, &others, reach, &mut |i, n, _| {
                    if let (Item::Track(t), true) = (i, n == Some(m)) {
                        *count.entry(t).or_default() += 1;
                    }
                });
        }
        let (&t, _) = count
            .iter()
            .max_by_key(|(&t, &c)| (c, std::cmp::Reverse(t)))?;
        if self.line_moved(t)
            && self
                .line_of
                .get(&t)
                .is_some_and(|&p| self.lines[p].moves >= MAX_MOVES)
        {
            return None;
        }
        let pi = self.line(t);
        (pi != li && self.lines[pi].layer == layer).then_some(pi)
    }

    /// Moves line `li` and its partner `pi` together along a shared centre
    /// line, keeping their spacing. `None` when they do not run as a pair
    /// where they have to move, or there is no way round.
    fn reroute_pair(&mut self, li: usize, pi: usize) -> Option<Vec<usize>> {
        let (layer, net, r) = {
            let l = &self.lines[li];
            (l.layer, l.net, l.r())
        };
        let pr = self.lines[pi].r();
        let need = self.gap(net).max(self.gap(self.lines[pi].net));
        let nets = self.friends(net);
        let hard = self.hard(layer, &nets, need, &[li, pi]);
        // The tracks' mitred corners stick out past the centre's width:
        // by 1/cos 22.5° at a 45° bend, √2 at a right angle.
        for corner in [1.0 / 22.5f64.to_radians().cos(), std::f64::consts::SQRT_2] {
            for m0 in Self::margins(r.max(pr), need) {
                let (a, b) = self.grow_window(li, &hard, need, r, &nets, m0)??;
                let keep = (self.lines[li].pts.clone(), self.lines[pi].pts.clone());
                if let Some(done) = self.pair_window(li, pi, a, b, &hard, need, &nets, corner) {
                    if self.pair_clear(li, pi, need) {
                        return Some(done);
                    }
                    self.lines[li].pts = keep.0;
                    self.lines[pi].pts = keep.1;
                    self.lines[li].moves -= 1;
                    self.lines[pi].moves -= 1;
                }
            }
        }
        None
    }

    /// Moves the stretch `a..b` of line `li` and the partner beside it.
    #[allow(clippy::too_many_arguments)]
    fn pair_window(
        &mut self,
        li: usize,
        pi: usize,
        a: f64,
        b: f64,
        hard: &[Capsule],
        need: f64,
        nets: &[Option<u32>],
        corner: f64,
    ) -> Option<Vec<usize>> {
        let (layer, r) = (self.lines[li].layer, self.lines[li].r());
        let pr = self.lines[pi].r();
        let lp = self.lines[li].pts.clone();
        let pp = self.lines[pi].pts.clone();
        let (before, pa, pb, after) = Self::split(&lp, a, b);
        // Where the partner is beside the stretch's ends.
        let near = |p: P| -> (f64, P) {
            let mut best = (f64::MAX, 0.0, p);
            let mut run = 0.0;
            for w in pp.windows(2) {
                let (q, t) = foot(p, w[0], w[1]);
                let len = dist(w[0], w[1]);
                let d = dist(p, q);
                if d < best.0 {
                    best = (d, run + t * len, q);
                }
                run += len;
            }
            (best.1, best.2)
        };
        let (sa, qa) = near(pa);
        let (sb, qb) = near(pb);
        let span = (dist(pa, qa) + dist(pb, qb)) / 2.0;
        let couple = 3.0 * (r + pr) + 2.0 * need + 10.0;
        if dist(pa, qa) > couple || dist(pb, qb) > couple || span < r + pr {
            return None;
        }
        let (pbefore, _, _, pafter) = if sa <= sb {
            Self::split(&pp, sa, sb)
        } else {
            Self::split(&pp, sb, sa)
        };
        let ca = [(pa[0] + qa[0]) / 2.0, (pa[1] + qa[1]) / 2.0];
        let cb = [(pb[0] + qb[0]) / 2.0, (pb[1] + qb[1]) / 2.0];
        // The centre is as wide as both tracks and the gap between them.
        let half = span / 2.0 * corner + r.max(pr);
        let ask = Ask {
            obstacles: self.obs,
            layer,
            r: half,
            nets,
            clearance: need,
            extra: hard,
            skip: &[],
            see_through: true,
            deadline: Some(self.deadline),
        };
        let centre = walk::walkaround(&ask, ca, cb)?;
        // The way the line runs where the stretch starts and ends: a short
        // lead in and out that way, so the offset tracks meet the old ones
        // in line, then are cut off again.
        let dir_at = |s: f64, back: bool| -> P {
            let (i, _) = Self::at(&lp, s);
            let (u, v) = (lp[i], lp[(i + 1).min(lp.len() - 1)]);
            let l = dist(u, v).max(1e-9);
            let d = [(v[0] - u[0]) / l, (v[1] - u[1]) / l];
            if back {
                [-d[0], -d[1]]
            } else {
                d
            }
        };
        let lead = span.max(1.0);
        let (din, dout) = (dir_at(a, true), dir_at(b, false));
        let mut c = vec![[ca[0] + din[0] * lead, ca[1] + din[1] * lead]];
        c.extend(centre.iter().copied());
        c.push([cb[0] + dout[0] * lead, cb[1] + dout[1] * lead]);
        let c = simplify(c);
        let pieces: Vec<crate::route::Piece> = c
            .windows(2)
            .map(|w| crate::route::Piece::Line(w[0], w[1]))
            .collect();
        if pieces.len() < 2 {
            return None;
        }
        let (c0, c1) = (c[0], c[1]);
        let side = (c1[0] - c0[0]) * (pa[1] - c0[1]) - (c1[1] - c0[1]) * (pa[0] - c0[0]);
        let d = if side >= 0.0 { span / 2.0 } else { -span / 2.0 };
        let chain = |ps: &[crate::route::Piece]| -> Vec<P> {
            let mut v = Vec::new();
            for p in ps {
                for (x, y) in p.chords() {
                    if v.is_empty() {
                        v.push(x);
                    }
                    v.push(y);
                }
            }
            v
        };
        // Each offset track leaves its old path at its first corner and
        // rejoins it at its last: the lead in and out lie on the old track.
        let splice = |old: &[P], new: &[P]| -> Option<Vec<P>> {
            let n = new.len();
            if n < 4 {
                return None;
            }
            let (s1, _) = project(old, new[1]);
            let (s2, _) = project(old, new[n - 2]);
            let mid: Vec<P> = new[1..n - 1].to_vec();
            let (lo, hi, mid) = if s1 <= s2 {
                (s1, s2, mid)
            } else {
                (s2, s1, mid.into_iter().rev().collect())
            };
            let (mut out, _, _, after) = Self::split(old, lo, hi);
            out.extend(mid);
            out.extend(after);
            Some(out)
        };
        let l_pts = splice(&lp, &chain(&crate::route::offset(&pieces, d)))?;
        let p_pts = splice(&pp, &chain(&crate::route::offset(&pieces, -d)))?;
        let _ = (before, after, pbefore, pafter);
        self.lines[li].pts = simplify(l_pts);
        self.lines[pi].pts = simplify(p_pts);
        self.lines[li].moves += 1;
        self.lines[pi].moves += 1;
        Some(vec![li, pi])
    }

    /// Whether the new copper of a moved pair keeps clearance from what
    /// cannot move and from hard copper.
    fn pair_clear(&self, li: usize, pi: usize, need: f64) -> bool {
        let skip = |i: Item| matches!(i, Item::Track(_) | Item::Via(_));
        [li, pi].iter().all(|&i| {
            let l = &self.lines[i];
            let nets = self.friends(l.net);
            let hard = self.hard(l.layer, &nets, need, &[li, pi]);
            l.new_segs().iter().all(|&(a, b)| {
                self.obs
                    .clear_items(Some(l.layer), a, b, l.r(), &nets, need, &skip)
                    && hard
                        .iter()
                        .all(|&(c, d, rr)| seg_seg(a, b, c, d) - rr - l.r() >= need - 1e-3)
            })
        })
    }

    /// Everything moved keeps clearance from everything else, or the push
    /// is given up.
    fn verify(&self) -> Option<()> {
        let skip = |i: Item| match i {
            Item::Track(t) => self.line_moved(t),
            Item::Via(v) => self.vias.contains_key(&v),
            Item::Fixed => false,
        };
        for (li, l) in self.lines.iter().enumerate() {
            if l.moves == 0 {
                continue;
            }
            let nets = self.friends(l.net);
            let need = self.gap(l.net);
            let hard = self.hard(l.layer, &nets, need, &[li]);
            for (a, b) in l.new_segs() {
                if !self
                    .obs
                    .clear_items(Some(l.layer), a, b, l.r(), &nets, need, &skip)
                {
                    return None;
                }
                if hard
                    .iter()
                    .any(|&(c, d, rr)| seg_seg(a, b, c, d) - rr - l.r() < need - 1e-3)
                {
                    return None;
                }
            }
        }
        for (&v, &(c, _)) in &self.vias {
            let via = &self.b.geometry.vias[v];
            let (lo, hi) = via_span(self.b, via);
            let need = self.gap(via.net);
            for l in lo..=hi {
                if !self
                    .obs
                    .clear_items(Some(l), c, c, via_radius(via), &[via.net], need, &skip)
                {
                    return None;
                }
                let others: Vec<Capsule> = self
                    .hard(l, &[via.net], need, &[])
                    .into_iter()
                    .filter(|&(a, b, _)| !(dist(a, c) < 1e-9 && dist(b, c) < 1e-9))
                    .collect();
                if others
                    .iter()
                    .any(|&(a, b, rr)| seg_seg(c, c, a, b) - rr - via_radius(via) < need - 1e-3)
                {
                    return None;
                }
            }
        }
        Some(())
    }

    fn result(&self) -> Pushed {
        let mut out = Pushed::default();
        for l in self.lines.iter().filter(|l| l.moves > 0) {
            out.remove.extend(&l.tracks);
            for (a, b) in l.segs() {
                if dist(a, b) < 1e-6 {
                    continue;
                }
                out.add.push(Track {
                    component: None,
                    layer: LayerRef::Copper(l.layer),
                    net: l.net,
                    start: point(a),
                    end: point(b),
                    width: Length::from_mil(l.w),
                    text: false,
                });
            }
        }
        out.remove.sort_unstable();
        out.remove.dedup();
        let mut vias: Vec<(usize, P)> = self.vias.iter().map(|(&v, &(c, _))| (v, c)).collect();
        vias.sort_by_key(|v| v.0);
        out.vias = vias;
        out
    }
}

#[cfg(test)]
mod tests;
