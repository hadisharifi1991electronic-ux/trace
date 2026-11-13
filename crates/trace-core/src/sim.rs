//! Signal integrity simulation: a routed net (or pair) driven by an IBIS
//! buffer into an IBIS receiver, in the time domain, with an eye diagram.
//!
//! The net's copper becomes transmission lines: each track and arc
//! at the impedance and delay the stack-up gives its width and layer (for a
//! pair, the odd-mode impedance, half the differential), and each via
//! barrel as a short line of its own. Branches the route does not take,
//! via stubs past the layers joined (less any backdrill) and pads of other
//! parts, stay on as open stubs. Lines are solved by the method of
//! characteristics (Bergeron); the buffers, their die capacitance and
//! package R, L and C at each step by Newton's method.
//!
//! The driver switches between its pullup and pulldown tables, the
//! switching time set so its edge into the [Ramp] load matches the ramp.
//!
//! Lines lose signal to their copper (DC resistance, skin effect,
//! roughness) and dielectric (loss tangent) through a causal filter per
//! line ([`loss`]). Tracks of other nets running beside the route are
//! driven with patterns of their own, and their near- and far-end
//! crosstalk enters the route's lines ([`xtalk`]). Power noise is not
//! modelled.

pub mod loss;
pub mod xtalk;

use crate::backdrill::faces;
use crate::ibis::{Corner, Model, Rlc};
use crate::impedance::DEFAULT_ER;
use crate::{Board, LayerRef};
pub use loss::{Atten, Loss};
use std::collections::HashMap;

/// Speed of light, inches per second.
const C_IN: f64 = 1.1803e10;

type P = [f64; 2];

/// A transmission line between two nodes.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub a: usize,
    pub b: usize,
    /// Characteristic impedance, ohm.
    pub z: f64,
    /// Delay, seconds.
    pub delay: f64,
    /// Loss along the line.
    pub att: Atten,
}

/// A net's copper as nodes joined by transmission lines.
#[derive(Clone, Debug, Default)]
pub struct Network {
    pub nodes: usize,
    pub lines: Vec<Line>,
    /// Lumped capacitance at nodes (from lines too short to step).
    pub caps: Vec<(usize, f64)>,
    /// Board pad index and its node.
    pub pads: Vec<(usize, usize)>,
    /// Track and arc length, mil, and vias, for the summary.
    pub length: f64,
    pub vias: usize,
    /// Lowest and highest line impedance, ohm.
    pub z_range: (f64, f64),
    /// Board track index and the line it became (its `a` end at the
    /// track's start).
    pub track_lines: Vec<(usize, usize)>,
}

/// Where a line of a network went when it was reduced: the new line and
/// the map from delay along the old line (from its `a` end) to delay
/// along the new one, `off + sign * t`.
#[derive(Clone, Copy, Debug)]
pub struct Moved {
    pub line: usize,
    pub sign: f64,
    pub off: f64,
}

impl Network {
    pub fn node_of_pad(&self, pad: usize) -> Option<usize> {
        self.pads.iter().find(|p| p.0 == pad).map(|p| p.1)
    }
}

struct Uf(Vec<usize>);
impl Uf {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b] = a;
        }
    }
}

/// Builds the network of `net`. With `odd`, tracks running beside the
/// pair partner take half the pair's differential impedance.
pub fn network(board: &Board, net: u32, odd: bool) -> Network {
    let geo = board.layer_geometry();
    let depth = faces(board);
    let mid = |l: u8| depth.get(l as usize).map_or(0.0, |(a, b)| (a + b) / 2.0);
    let er = geo.first().map_or(DEFAULT_ER, |g| g.er);
    let partner = if odd {
        board.pair_partners().get(&net).copied()
    } else {
        None
    };
    let g = &board.geometry;
    let pt = |p: crate::Point| [p.x.mil(), p.y.mil()];
    // Anchors: layer, point, reach.
    let mut anchors: Vec<(u8, P, f64)> = Vec::new();
    // Edges between anchors: z, delay, loss, the track they come from.
    let mut edges: Vec<Edge> = Vec::new();
    let mut pads = Vec::new();
    let mut length = 0.0;
    let delay_of = |l: u8, w: f64, len: f64| -> f64 {
        let ps = geo.get(l as usize).map_or(170.0, |g| g.delay_ps_per_in(w));
        len / 1000.0 * ps * 1e-12
    };
    let z_of = |l: u8, w: f64| -> f64 {
        geo.get(l as usize)
            .map(|g| g.z0(w))
            .filter(|z| z.is_finite() && *z > 1.0)
            .unwrap_or(50.0)
    };
    // A plated barrel through `layers` at `at`: its anchors and edges.
    let barrel = |anchors: &mut Vec<(u8, P, f64)>,
                  edges: &mut Vec<Edge>,
                  layers: &[u8],
                  at: P,
                  d: f64,
                  hole: f64|
     -> Vec<usize> {
        let zv = (60.0 / er.sqrt() * ((d + 20.0) / hole.max(1.0)).ln()).clamp(20.0, 80.0);
        let mut ids = Vec::new();
        for &l in layers {
            anchors.push((l, at, d / 2.0));
            ids.push(anchors.len() - 1);
        }
        for w in ids.windows(2).zip(layers.windows(2)) {
            let ((a, b), (la, lb)) = ((w.0[0], w.0[1]), (w.1[0], w.1[1]));
            let len = (mid(lb) - mid(la)).abs();
            let tau = len / 1000.0 * er.sqrt() / C_IN;
            edges.push((a, b, zv, tau, loss::dielectric(tau), None));
        }
        ids
    };
    for (ti, t) in g.tracks.iter().enumerate() {
        let LayerRef::Copper(l) = t.layer else {
            continue;
        };
        if t.net != Some(net) || t.text {
            continue;
        }
        let (a, b) = (pt(t.start), pt(t.end));
        let w = t.width.mil();
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let z = match partner.and_then(|p| board.pair_gap(t, p)) {
            Some(gap) => geo
                .get(l as usize)
                .map(|g| g.zdiff(w, gap) / 2.0)
                .filter(|z| z.is_finite() && *z > 1.0)
                .unwrap_or_else(|| z_of(l, w)),
            None => z_of(l, w),
        };
        anchors.push((l, a, w / 2.0));
        anchors.push((l, b, w / 2.0));
        let n = anchors.len();
        let tau = delay_of(l, w, len);
        let att = loss::track(geo.get(l as usize), w, len, z, tau);
        edges.push((n - 2, n - 1, z, tau, att, Some(ti)));
        length += len;
    }
    for arc in &g.arcs {
        let LayerRef::Copper(l) = arc.layer else {
            continue;
        };
        if arc.net != Some(net) {
            continue;
        }
        let (c, r) = (pt(arc.center), arc.radius.mil());
        let mut sweep = (arc.end_deg - arc.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let at = |deg: f64| {
            let t = deg.to_radians();
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        };
        let w = arc.width.mil();
        let len = r * sweep.to_radians();
        anchors.push((l, at(arc.start_deg), w / 2.0));
        anchors.push((l, at(arc.start_deg + sweep), w / 2.0));
        let n = anchors.len();
        let (z, tau) = (z_of(l, w), delay_of(l, w, len));
        let att = loss::track(geo.get(l as usize), w, len, z, tau);
        edges.push((n - 2, n - 1, z, tau, att, None));
        length += len;
    }
    let mut vias = 0;
    for v in g.vias.iter().filter(|v| v.net == Some(net)) {
        let (lo, hi) = (
            v.from_layer.min(v.to_layer),
            v.from_layer
                .max(v.to_layer)
                .min(depth.len().max(1) as u8 - 1),
        );
        let layers: Vec<u8> = (lo..=hi).filter(|&l| v.drilled(l).is_none()).collect();
        barrel(
            &mut anchors,
            &mut edges,
            &layers,
            pt(v.at),
            v.diameter.mil(),
            v.hole.mil(),
        );
        vias += 1;
    }
    let copper = depth.len().max(1) as u8;
    for (pi, p) in g.pads.iter().enumerate() {
        if p.net != Some(net) {
            continue;
        }
        let at = pt(p.at);
        let r = p.size_x.mil().max(p.size_y.mil()) / 2.0;
        match p.layer {
            LayerRef::Copper(l) => {
                anchors.push((l, at, r));
                pads.push((pi, anchors.len() - 1));
            }
            LayerRef::AllCopper => {
                let layers: Vec<u8> = (0..copper).collect();
                let ids = barrel(
                    &mut anchors,
                    &mut edges,
                    &layers,
                    at,
                    2.0 * r,
                    p.hole.mil().max(1.0),
                );
                if let Some(&first) = ids.first() {
                    pads.push((pi, first));
                }
            }
            _ => {}
        }
    }
    // Copper that touches on a layer is one node.
    let mut uf = Uf((0..anchors.len()).collect());
    const CELL: f64 = 50.0;
    let mut grid: HashMap<(u8, i64, i64), Vec<usize>> = HashMap::new();
    for (i, a) in anchors.iter().enumerate() {
        let k = (
            a.0,
            (a.1[0] / CELL).floor() as i64,
            (a.1[1] / CELL).floor() as i64,
        );
        grid.entry(k).or_default().push(i);
    }
    for (i, a) in anchors.iter().enumerate() {
        let reach = ((a.2 + 1.0) / CELL).ceil() as i64 + 1;
        let (cx, cy) = (
            (a.1[0] / CELL).floor() as i64,
            (a.1[1] / CELL).floor() as i64,
        );
        for gx in cx - reach..=cx + reach {
            for gy in cy - reach..=cy + reach {
                let Some(list) = grid.get(&(a.0, gx, gy)) else {
                    continue;
                };
                for &j in list {
                    if j <= i {
                        continue;
                    }
                    let b = &anchors[j];
                    let d = (a.1[0] - b.1[0]).hypot(a.1[1] - b.1[1]);
                    if d <= a.2.max(b.2) + 0.01 {
                        uf.join(i, j);
                    }
                }
            }
        }
    }
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let mut node = |uf: &mut Uf, i: usize| {
        let r = uf.find(i);
        let n = ids.len();
        *ids.entry(r).or_insert(n)
    };
    let mut lines = Vec::new();
    let mut track_lines = Vec::new();
    for &(a, b, z, delay, att, track) in &edges {
        let (na, nb) = (node(&mut uf, a), node(&mut uf, b));
        if na != nb {
            if let Some(t) = track {
                track_lines.push((t, lines.len()));
            }
            lines.push(Line {
                a: na,
                b: nb,
                z,
                delay,
                att,
            });
        }
    }
    let pads: Vec<(usize, usize)> = pads
        .into_iter()
        .map(|(pi, a)| (pi, node(&mut uf, a)))
        .collect();
    let z_range = lines.iter().fold((f64::MAX, 0.0_f64), |(lo, hi), l| {
        (lo.min(l.z), hi.max(l.z))
    });
    Network {
        nodes: ids.len(),
        lines,
        caps: Vec::new(),
        pads,
        length,
        vias,
        z_range,
        track_lines,
    }
}

type Edge = (usize, usize, f64, f64, Atten, Option<usize>);

impl Network {
    /// Lines shorter than half a step become capacitance at a node
    /// (their two ends joined): C = delay / Z. Returns the reduced network,
    /// where each old node went, and where each old line went (`None` for
    /// lines that became capacitance).
    fn reduce(&self, dt: f64) -> (Network, Vec<usize>, Vec<Option<Moved>>) {
        // First join lines in series through nodes nothing else touches:
        // their delays and losses add, and the impedance is the
        // delay-weighted mean.
        let mut slots: Vec<Option<Line>> = self.lines.iter().copied().map(Some).collect();
        // Each old line: its slot, and sign and offset into it.
        let mut moved: Vec<(usize, f64, f64)> =
            (0..self.lines.len()).map(|i| (i, 1.0, 0.0)).collect();
        let mut members: Vec<Vec<usize>> = (0..self.lines.len()).map(|i| vec![i]).collect();
        let mut ends: Vec<Vec<usize>> = vec![Vec::new(); self.nodes];
        for (i, l) in self.lines.iter().enumerate() {
            ends[l.a].push(i);
            ends[l.b].push(i);
        }
        let mut keep = vec![false; self.nodes];
        for &(_, n) in &self.pads {
            keep[n] = true;
        }
        for &(n, _) in &self.caps {
            keep[n] = true;
        }
        for node in 0..self.nodes {
            if keep[node] || ends[node].len() != 2 {
                continue;
            }
            let (i, j) = (ends[node][0], ends[node][1]);
            if i == j {
                continue;
            }
            let (Some(x), Some(y)) = (slots[i], slots[j]) else {
                continue;
            };
            let far = |l: &Line| if l.a == node { l.b } else { l.a };
            let (p, q) = (far(&x), far(&y));
            if p == node || q == node {
                continue;
            }
            let d = x.delay + y.delay;
            let z = if d > 0.0 {
                (x.z * x.delay + y.z * y.delay) / d
            } else {
                (x.z + y.z) / 2.0
            };
            // The new line runs p → node → q.
            for &m in &members[i] {
                let (_, sg, off) = moved[m];
                moved[m] = if x.a == p {
                    (i, sg, off)
                } else {
                    (i, -sg, x.delay - off)
                };
            }
            for &m in &members[j] {
                let (_, sg, off) = moved[m];
                moved[m] = if y.a == node {
                    (i, sg, x.delay + off)
                } else {
                    (i, -sg, x.delay + y.delay - off)
                };
            }
            let taken = std::mem::take(&mut members[j]);
            members[i].extend(taken);
            slots[i] = Some(Line {
                a: p,
                b: q,
                z,
                delay: d,
                att: x.att + y.att,
            });
            slots[j] = None;
            // The far end of the dropped line now meets line i.
            for e in ends[q].iter_mut() {
                if *e == j {
                    *e = i;
                }
            }
            ends[node].clear();
        }
        let mut uf = Uf((0..self.nodes).collect());
        let mut caps: Vec<(usize, f64)> = self.caps.clone();
        let mut index = vec![None; slots.len()];
        let mut kept = Vec::new();
        for (si, l) in slots.iter().enumerate() {
            let Some(l) = l else { continue };
            if l.delay < dt / 2.0 {
                uf.join(l.a, l.b);
                caps.push((l.a, l.delay / l.z));
            } else {
                index[si] = Some(kept.len());
                kept.push(*l);
            }
        }
        let mut ids: HashMap<usize, usize> = HashMap::new();
        let mut map = vec![0; self.nodes];
        for (i, m) in map.iter_mut().enumerate() {
            let r = uf.find(i);
            let n = ids.len();
            *m = *ids.entry(r).or_insert(n);
        }
        let lines = kept
            .iter()
            .map(|l| Line {
                a: map[l.a],
                b: map[l.b],
                ..*l
            })
            .collect();
        let caps = caps.into_iter().map(|(n, c)| (map[n], c)).collect();
        let pads = self.pads.iter().map(|&(p, n)| (p, map[n])).collect();
        let lines_moved = moved
            .iter()
            .map(|&(slot, sign, off)| index[slot].map(|line| Moved { line, sign, off }))
            .collect();
        (
            Network {
                nodes: ids.len(),
                lines,
                caps,
                pads,
                ..self.clone()
            },
            map,
            lines_moved,
        )
    }

    /// Whether nodes `a` and `b` are joined by lines.
    pub fn joined(&self, a: usize, b: usize) -> bool {
        let mut uf = Uf((0..self.nodes).collect());
        for l in &self.lines {
            uf.join(l.a, l.b);
        }
        uf.find(a) == uf.find(b)
    }

    /// The shortest delay between two nodes, seconds, and the loss along
    /// that path.
    pub fn path(&self, a: usize, b: usize) -> Option<(f64, Atten)> {
        let mut adj: Vec<Vec<(usize, f64, Atten)>> = vec![Vec::new(); self.nodes];
        for l in &self.lines {
            adj[l.a].push((l.b, l.delay, l.att));
            adj[l.b].push((l.a, l.delay, l.att));
        }
        let mut best = vec![(f64::MAX, Atten::default()); self.nodes];
        best[a].0 = 0.0;
        let mut todo = vec![a];
        while let Some(i) = todo.pop() {
            for &(j, d, att) in &adj[i] {
                if best[i].0 + d < best[j].0 - 1e-18 {
                    best[j] = (best[i].0 + d, best[i].1 + att);
                    todo.push(j);
                }
            }
        }
        (best[b].0 < f64::MAX).then_some(best[b])
    }

    /// The shortest delay between two nodes, seconds.
    pub fn delay_between(&self, a: usize, b: usize) -> Option<f64> {
        self.path(a, b).map(|p| p.0)
    }
}

/// A termination at the receiver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Term {
    None,
    /// To a voltage at each receiver pin.
    ToRef {
        r: f64,
        v: f64,
    },
    /// Across the pair.
    Across(f64),
}

/// What to simulate.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub corner: Corner,
    /// Bits per second.
    pub rate: f64,
    /// Bits of the pseudo-random (PRBS7) pattern.
    pub bits: usize,
    pub term: Term,
    /// Track losses; `None` for lossless lines.
    pub loss: Option<Loss>,
    /// Drive the nets running beside the route and add their crosstalk.
    pub crosstalk: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            corner: Corner::Typ,
            rate: 1e9,
            bits: 160,
            term: Term::None,
            loss: Some(Loss::default()),
            crosstalk: true,
        }
    }
}

/// A net whose crosstalk the simulation added.
#[derive(Clone, Debug)]
pub struct Aggressor {
    pub net: String,
    /// Pad it was driven from (designator.pin).
    pub from: String,
    /// Length it runs beside the route, mil.
    pub coupled: f64,
    /// Strongest backward (near-end) coupling over that length.
    pub kb: f64,
    /// Whether it was driven inverted, as the negative of a pair.
    pub inverted: bool,
}

/// One buffer end: its model and package.
#[derive(Clone, Copy)]
pub struct End<'a> {
    pub model: &'a Model,
    pub pkg: Rlc,
}

/// A recorded voltage.
#[derive(Clone, Debug)]
pub struct Probe {
    pub name: String,
    pub v: Vec<f32>,
}

/// The eye: traces two unit intervals long, the eye centred on the middle.
#[derive(Clone, Debug, Default)]
pub struct Eye {
    pub ui: f64,
    /// Samples per trace (over 2 UI).
    pub samples: usize,
    pub traces: Vec<Vec<f32>>,
    /// Lowest one minus highest zero at the centre, volts.
    pub height: f64,
    /// Open time around the centre, seconds.
    pub width: f64,
    /// Lowest one and highest zero at the centre.
    pub high: f64,
    pub low: f64,
    pub threshold: f64,
}

/// A finished simulation.
#[derive(Clone, Debug, Default)]
pub struct Run {
    /// Time step of the probes, seconds.
    pub dt: f64,
    /// Time of the first bit.
    pub start: f64,
    pub probes: Vec<Probe>,
    pub eye: Eye,
    /// Delay from driver to receiver along the route, seconds.
    pub delay: f64,
    /// The switching time the driver was given, rising and falling.
    pub switch: (f64, f64),
    pub bits: Vec<bool>,
    pub notes: Vec<String>,
    /// Loss from driver to receiver at the Nyquist frequency (half the bit
    /// rate), dB; 0 when lossless.
    pub loss_db: f64,
    /// Nets whose crosstalk is in the waveforms.
    pub aggressors: Vec<Aggressor>,
    /// The eye without crosstalk, when there was any.
    pub quiet: Option<Eye>,
    /// Largest crosstalk at the receiver (with minus without), volts.
    pub noise: f64,
}

/// PRBS7 (x^7 + x^6 + 1).
pub fn prbs7(n: usize) -> Vec<bool> {
    let mut s: u8 = 0x7f;
    (0..n)
        .map(|_| {
            let b = ((s >> 6) ^ (s >> 5)) & 1;
            s = ((s << 1) | b) & 0x7f;
            b == 1
        })
        .collect()
}

/// A line as the solver steps it: its whole steps of delay, and the
/// filter its loss gives (one tap of 1 when lossless).
struct Stepped {
    line: Line,
    k: usize,
    taps: Vec<f64>,
    /// One pole after the taps: decay per step, first value.
    tail: (f64, f64),
}

impl Stepped {
    fn gain(&self) -> f64 {
        let (a, b) = self.tail;
        self.taps.iter().sum::<f64>() + if b != 0.0 { b / (1.0 - a) } else { 0.0 }
    }

    /// Length of its ring buffers.
    fn ring(&self) -> usize {
        self.k + self.taps.len() + 1
    }
}

/// The circuit the solver steps: nodes, lines, and elements.
struct Circuit<'a> {
    n: usize,
    lines: Vec<Stepped>,
    /// Shunt capacitors: node, farads.
    caps: Vec<(usize, f64)>,
    /// Shunt resistors: node, ohm, to volts.
    shunts: Vec<(usize, f64, f64)>,
    /// Series resistors and inductors: from, to, value; R true or L false.
    series: Vec<(usize, usize, f64, bool)>,
    /// Buffers: node, model, driven (index into the drive list) or not.
    buffers: Vec<(usize, &'a Model, Option<usize>)>,
}

struct Solved {
    v: Vec<Vec<f32>>,
    /// For each line asked for, the wave it launched at its `a` and `b`
    /// ends each step (f = 2v/Z − h).
    waves: Vec<(Vec<f32>, Vec<f32>)>,
}

/// A starting state: node voltages, the current into each line at its
/// two ends, and through each inductor.
struct Init {
    v: Vec<f64>,
    line_i: Vec<(f64, f64)>,
    ind_i: Vec<f64>,
}

/// Extra waves arriving at lines' ends from crosstalk, volts per step:
/// line index, at `a`, at `b`.
type Inject = Vec<(usize, Vec<f32>, Vec<f32>)>;

impl<'a> Circuit<'a> {
    /// Steps the circuit for `steps` of `dt`, the drivers' pullup share
    /// given by `ku(driver, step)`, recording the voltages of `record` and
    /// the waves of the lines `waves`.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        dt: f64,
        steps: usize,
        corner: Corner,
        ku: &dyn Fn(usize, usize) -> f64,
        record: &[usize],
        init: Option<&Init>,
        waves: &[usize],
        inject: &Inject,
    ) -> Solved {
        let n = self.n;
        // Linear conductance to ground per node, and history sources.
        let mut g = vec![0.0; n];
        for s in &self.lines {
            g[s.line.a] += 1.0 / s.line.z;
            g[s.line.b] += 1.0 / s.line.z;
        }
        for &(node, c) in &self.caps {
            g[node] += 2.0 * c / dt;
        }
        for &(node, r, _) in &self.shunts {
            g[node] += 1.0 / r;
        }
        let sg: Vec<f64> = self
            .series
            .iter()
            .map(|&(_, _, x, r)| if r { 1.0 / x } else { dt / (2.0 * x) })
            .collect();
        // Clusters: nodes joined by series elements, or holding a buffer,
        // solved together by Newton; the rest by division.
        let mut uf = Uf((0..n).collect());
        for &(a, b, _, _) in &self.series {
            uf.join(a, b);
        }
        let mut in_cluster = vec![usize::MAX; n];
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut root_of: HashMap<usize, usize> = HashMap::new();
        let mut mark = |i: usize, uf: &mut Uf, clusters: &mut Vec<Vec<usize>>| {
            let r = uf.find(i);
            let c = *root_of.entry(r).or_insert_with(|| {
                clusters.push(Vec::new());
                clusters.len() - 1
            });
            if in_cluster[i] == usize::MAX {
                in_cluster[i] = c;
                clusters[c].push(i);
            }
        };
        for &(a, b, _, _) in &self.series {
            mark(a, &mut uf, &mut clusters);
            mark(b, &mut uf, &mut clusters);
        }
        for &(node, _, _) in &self.buffers {
            mark(node, &mut uf, &mut clusters);
        }
        // Each cluster's members: series elements and buffers inside it.
        let local: Vec<HashMap<usize, usize>> = clusters
            .iter()
            .map(|c| c.iter().enumerate().map(|(k, &i)| (i, k)).collect())
            .collect();
        let mut v = vec![0.0; n];
        let mut cap_i = vec![0.0; self.caps.len()];
        let mut cap_v = vec![0.0; self.caps.len()];
        let mut ind_i = vec![0.0; self.series.len()];
        let mut ind_h = vec![0.0; self.series.len()];
        // Ring buffers of launched waves, long enough for delay and filter.
        let mut fa: Vec<Vec<f64>> = self.lines.iter().map(|s| vec![0.0; s.ring()]).collect();
        let mut fb = fa.clone();
        // The tails' states at each end.
        let mut tails = vec![(0.0, 0.0); self.lines.len()];
        if let Some(init) = init {
            v.copy_from_slice(&init.v);
            for (li, s) in self.lines.iter().enumerate() {
                let (ia, ib) = init.line_i[li];
                let l = &s.line;
                let (wa, wb) = (v[l.a] / l.z + ia, v[l.b] / l.z + ib);
                fa[li].iter_mut().for_each(|x| *x = wa);
                fb[li].iter_mut().for_each(|x| *x = wb);
                let (a, b) = s.tail;
                if b != 0.0 {
                    tails[li] = (b * wb / (1.0 - a), b * wa / (1.0 - a));
                }
            }
            for (ci, &(node, _)) in self.caps.iter().enumerate() {
                cap_v[ci] = v[node];
            }
            ind_i.copy_from_slice(&init.ind_i);
        }
        let mut extra: Vec<Option<(&[f32], &[f32])>> = vec![None; self.lines.len()];
        for (li, a, b) in inject {
            extra[*li] = Some((a, b));
        }
        let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(steps); record.len()];
        let mut wave_out: Vec<(Vec<f32>, Vec<f32>)> = waves
            .iter()
            .map(|_| (Vec::with_capacity(steps), Vec::with_capacity(steps)))
            .collect();
        let mut src = vec![0.0; n];
        let mut hist = vec![(0.0, 0.0); self.lines.len()];
        for step in 0..steps {
            src.iter_mut().for_each(|s| *s = 0.0);
            for (li, s) in self.lines.iter().enumerate() {
                let len = s.ring() as isize;
                let (mut ha, mut hb) = (0.0, 0.0);
                for (m, &t) in s.taps.iter().enumerate() {
                    let idx = (step as isize - s.k as isize - m as isize).rem_euclid(len) as usize;
                    ha += t * fb[li][idx];
                    hb += t * fa[li][idx];
                }
                let (a, b) = s.tail;
                if b != 0.0 {
                    let idx = (step as isize - s.k as isize - s.taps.len() as isize).rem_euclid(len)
                        as usize;
                    let (ya, yb) = tails[li];
                    let (ya, yb) = (a * ya + b * fb[li][idx], a * yb + b * fa[li][idx]);
                    tails[li] = (ya, yb);
                    ha += ya;
                    hb += yb;
                }
                if let Some((ea, eb)) = extra[li] {
                    ha += 2.0 * ea.get(step).copied().unwrap_or(0.0) as f64 / s.line.z;
                    hb += 2.0 * eb.get(step).copied().unwrap_or(0.0) as f64 / s.line.z;
                }
                hist[li] = (ha, hb);
                src[s.line.a] += ha;
                src[s.line.b] += hb;
            }
            for (ci, &(node, c)) in self.caps.iter().enumerate() {
                let gc = 2.0 * c / dt;
                src[node] += gc * cap_v[ci] + cap_i[ci];
            }
            for &(node, r, vr) in &self.shunts {
                src[node] += vr / r;
            }
            for (si, &(a, b, _, is_r)) in self.series.iter().enumerate() {
                if !is_r {
                    // i(a→b) = G (va - vb) + h, h = i_prev + G (va - vb)_prev.
                    let h = ind_i[si] + sg[si] * (v[a] - v[b]);
                    ind_h[si] = h;
                    src[a] -= h;
                    src[b] += h;
                }
            }
            // Plain nodes.
            for i in 0..n {
                if in_cluster[i] == usize::MAX {
                    v[i] = if g[i] > 0.0 { src[i] / g[i] } else { 0.0 };
                }
            }
            // Clusters.
            for (c, members) in clusters.iter().enumerate() {
                let m = members.len();
                let mut a = vec![0.0; m * m];
                for (k, &i) in members.iter().enumerate() {
                    a[k * m + k] += g[i];
                }
                for (si, &(p, q, _, _)) in self.series.iter().enumerate() {
                    if in_cluster[p] != c {
                        continue;
                    }
                    let (kp, kq) = (local[c][&p], local[c][&q]);
                    let x = sg[si];
                    a[kp * m + kp] += x;
                    a[kq * m + kq] += x;
                    a[kp * m + kq] -= x;
                    a[kq * m + kp] -= x;
                }
                let mut x: Vec<f64> = members.iter().map(|&i| v[i]).collect();
                for _ in 0..30 {
                    let mut jac = a.clone();
                    let mut r: Vec<f64> = (0..m)
                        .map(|k| (0..m).map(|j| a[k * m + j] * x[j]).sum::<f64>() - src[members[k]])
                        .collect();
                    for &(node, model, drv) in &self.buffers {
                        if in_cluster[node] != c {
                            continue;
                        }
                        let k = local[c][&node];
                        let (u, d) = match drv {
                            Some(di) => {
                                let u = ku(di, step);
                                (u, 1.0 - u)
                            }
                            None => (0.0, 0.0),
                        };
                        r[k] += model.current(x[k], u, d, corner);
                        jac[k * m + k] += model.conductance(x[k], u, d, corner).max(0.0);
                    }
                    let dx = solve(&mut jac, &mut r, m);
                    let mut worst: f64 = 0.0;
                    for k in 0..m {
                        // Damped: no more than a volt a step.
                        let s = dx[k].clamp(-1.0, 1.0);
                        x[k] -= s;
                        worst = worst.max(s.abs());
                    }
                    if worst < 1e-7 {
                        break;
                    }
                }
                for (k, &i) in members.iter().enumerate() {
                    v[i] = x[k];
                }
            }
            // Histories.
            for (li, s) in self.lines.iter().enumerate() {
                let idx = step % s.ring();
                let (ha, hb) = hist[li];
                fa[li][idx] = 2.0 * v[s.line.a] / s.line.z - ha;
                fb[li][idx] = 2.0 * v[s.line.b] / s.line.z - hb;
            }
            for (o, &li) in wave_out.iter_mut().zip(waves) {
                let idx = step % self.lines[li].ring();
                o.0.push(fa[li][idx] as f32);
                o.1.push(fb[li][idx] as f32);
            }
            for (ci, &(node, c)) in self.caps.iter().enumerate() {
                let gc = 2.0 * c / dt;
                let j = gc * cap_v[ci] + cap_i[ci];
                cap_i[ci] = gc * v[node] - j;
                cap_v[ci] = v[node];
            }
            for (si, &(a, b, _, is_r)) in self.series.iter().enumerate() {
                if !is_r {
                    ind_i[si] = sg[si] * (v[a] - v[b]) + ind_h[si];
                }
            }
            for (o, &node) in out.iter_mut().zip(record) {
                o.push(v[node] as f32);
            }
        }
        Solved {
            v: out,
            waves: wave_out,
        }
    }
}

/// Solves `a x = b` (row-major m×m) by Gaussian elimination with pivoting.
fn solve(a: &mut [f64], b: &mut [f64], m: usize) -> Vec<f64> {
    for col in 0..m {
        let piv = (col..m)
            .max_by(|&i, &j| a[i * m + col].abs().total_cmp(&a[j * m + col].abs()))
            .unwrap_or(col);
        if piv != col {
            for k in 0..m {
                a.swap(col * m + k, piv * m + k);
            }
            b.swap(col, piv);
        }
        let d = a[col * m + col];
        if d.abs() < 1e-30 {
            continue;
        }
        for row in col + 1..m {
            let f = a[row * m + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..m {
                a[row * m + k] -= f * a[col * m + k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; m];
    for row in (0..m).rev() {
        let mut s = b[row];
        for k in row + 1..m {
            s -= a[row * m + k] * x[k];
        }
        let d = a[row * m + row];
        x[row] = if d.abs() < 1e-30 { 0.0 } else { s / d };
    }
    x
}

/// The fixture a driver's [Ramp] is measured into: towards ground for a
/// rising edge and the supply for a falling one (towards the supply both
/// ways for open-drain, ground for open-source).
fn fixture(model: &Model, corner: Corner, rising: bool) -> f64 {
    if model.pullup.is_empty() {
        model.vcc(corner).max(model.power_clamp_vcc(corner))
    } else if model.pulldown.is_empty() || rising {
        model.vss(corner)
    } else {
        model.vcc(corner)
    }
}

/// How long the driver takes to hand over from pulldown to pullup (or
/// back) so that its 20–80 % edge into the ramp's load matches [Ramp].
/// Models without a ramp get 300 ps.
pub fn switch_time(model: &Model, corner: Corner, rising: bool) -> f64 {
    let Some(ramp) = model.ramp else {
        return 0.3e-9;
    };
    let target = if rising { ramp.rise.1 } else { ramp.fall.1 }.at(corner);
    if target.is_nan() || target <= 0.0 {
        return 0.3e-9;
    }
    let r_load = ramp.r_load.max(1.0);
    let c = model.c_comp.at(corner).max(0.0);
    let vref = fixture(model, corner, rising);
    let edge = |tsw: f64| -> f64 {
        let settle = 10.0 * r_load * c + target;
        let total = settle + 2.0 * tsw + 6.0 * target + 10.0 * r_load * c;
        let steps = 4000;
        let dt = total / steps as f64;
        let start = (settle / dt) as usize;
        let circuit = Circuit {
            n: 1,
            lines: Vec::new(),
            caps: if c > 0.0 { vec![(0, c)] } else { Vec::new() },
            shunts: vec![(0, r_load, vref)],
            series: Vec::new(),
            buffers: vec![(0, model, Some(0))],
        };
        let ku = |_: usize, step: usize| {
            let k = (step.saturating_sub(start) as f64 * dt / tsw).min(1.0);
            if rising {
                k
            } else {
                1.0 - k
            }
        };
        let v = &circuit
            .run(dt, steps, corner, &ku, &[0], None, &[], &Vec::new())
            .v[0];
        let (v0, v1) = (v[start.saturating_sub(1)] as f64, v[steps - 1] as f64);
        let at = |f: f64| {
            let lvl = v0 + f * (v1 - v0);
            v[start..]
                .iter()
                .position(|&x| {
                    if v1 > v0 {
                        x as f64 >= lvl
                    } else {
                        x as f64 <= lvl
                    }
                })
                .map_or(total, |i| i as f64 * dt)
        };
        at(0.8) - at(0.2)
    };
    // The edge grows with the switching time: bisect on a log scale.
    let (mut lo, mut hi) = ((target * 0.01).ln(), (target * 20.0).ln());
    for _ in 0..30 {
        let m = (lo + hi) / 2.0;
        if edge(m.exp()) < target {
            lo = m;
        } else {
            hi = m;
        }
    }
    ((lo + hi) / 2.0).exp()
}

/// Board pads the simulation drives and receives at; the `_n` pads for a
/// pair.
#[derive(Clone, Copy, Debug)]
pub struct Pins {
    pub driver: usize,
    pub receiver: usize,
    pub driver_n: Option<usize>,
    pub receiver_n: Option<usize>,
}

/// A net of a built circuit: where its nodes and lines start, and where
/// its tracks' lines went.
struct Part {
    net: u32,
    line0: usize,
    /// Board track → (delay of its line before reduction, where that line
    /// went).
    tracks: HashMap<usize, (f64, Moved)>,
}

/// A circuit built from nets of the board, with buffers at their ends.
struct Built<'a> {
    c: Circuit<'a>,
    /// Driver pad node and receiver pad node per net.
    at: Vec<(usize, usize)>,
    /// Receiver die nodes per net.
    rx_dies: Vec<usize>,
    parts: Vec<Part>,
    delay: f64,
    /// Loss along the route, the worst net.
    att: Atten,
    notes: Vec<String>,
}

fn pad_net(board: &Board, p: usize) -> Result<u32, String> {
    let pad = board
        .geometry
        .pads
        .get(p)
        .ok_or_else(|| format!("no pad {p}"))?;
    pad.net.ok_or_else(|| "the pad has no net".to_string())
}

/// Builds the circuit of the nets from each `(driver pad, receiver pad)`,
/// the first driven by drive 0, the next by drive 1.
#[allow(clippy::too_many_arguments)]
fn build<'a>(
    board: &Board,
    ends: &[(usize, usize)],
    odd: bool,
    driver: End<'a>,
    receiver: End<'a>,
    term: Term,
    corner: Corner,
    dt: f64,
    loss: Option<&Loss>,
) -> Result<Built<'a>, String> {
    let mut notes = Vec::new();
    let mut n = 0;
    let mut lines = Vec::new();
    let mut caps = Vec::new();
    let mut at: Vec<(usize, usize)> = Vec::new();
    let mut parts = Vec::new();
    let mut delay: f64 = 0.0;
    let mut att = Atten::default();
    for &(dp, rp) in ends {
        let net = pad_net(board, dp)?;
        if pad_net(board, rp)? != net {
            return Err(format!(
                "the driver and receiver pads are on different nets ({} and {})",
                board.nets[net as usize].name,
                board.nets[pad_net(board, rp)? as usize].name
            ));
        }
        let full = network(board, net, odd);
        let (nw, _, moved) = full.reduce(dt);
        let (Some(a), Some(b)) = (nw.node_of_pad(dp), nw.node_of_pad(rp)) else {
            return Err("pad not on a copper layer".into());
        };
        if a != b && !nw.joined(a, b) {
            return Err(format!(
                "{} is not routed from the driver to the receiver",
                board.nets[net as usize].name
            ));
        }
        if let Some((d, path)) = nw.path(a, b) {
            delay = delay.max(d);
            if path.dc + path.skin + path.diel > att.dc + att.skin + att.diel {
                att = path;
            }
        }
        if nw.pads.len() > 2 {
            notes.push(format!(
                "{} has {} other pads; they load the net as open stubs",
                board.nets[net as usize].name,
                nw.pads.len() - 2
            ));
        }
        let tracks = full
            .track_lines
            .iter()
            .filter_map(|&(t, li)| moved[li].map(|m| (t, (full.lines[li].delay, m))))
            .collect();
        parts.push(Part {
            net,
            line0: lines.len(),
            tracks,
        });
        for l in &nw.lines {
            let k = ((l.delay / dt).round() as usize).max(1);
            let f = match loss {
                Some(loss) => loss::filter(&l.att, loss, dt),
                None => loss::Filter::lossless(),
            };
            lines.push(Stepped {
                line: Line {
                    a: l.a + n,
                    b: l.b + n,
                    ..*l
                },
                k,
                taps: f.taps,
                tail: f.tail,
            });
        }
        caps.extend(nw.caps.iter().map(|&(i, c)| (i + n, c)));
        at.push((a + n, b + n));
        n += nw.nodes;
    }
    // Buffers, dies and packages.
    let mut shunts = Vec::new();
    let mut series = Vec::new();
    let mut buffers = Vec::new();
    let mut rx_dies = Vec::new();
    let mut hook: Vec<(End, usize, Option<usize>)> = Vec::new();
    for (k, &(a, b)) in at.iter().enumerate() {
        hook.push((driver, a, Some(k)));
        hook.push((receiver, b, None));
    }
    for (end, pad, drive) in hook {
        let die = n;
        n += 1;
        let r = end.pkg.r.max(1e-3);
        if end.pkg.l > 1e-13 {
            let mid = n;
            n += 1;
            series.push((die, mid, r, true));
            series.push((mid, pad, end.pkg.l, false));
        } else {
            series.push((die, pad, r, true));
        }
        if end.pkg.c > 0.0 {
            caps.push((pad, end.pkg.c));
        }
        let cc = end.model.c_comp.at(corner);
        if cc > 0.0 {
            caps.push((die, cc));
        }
        buffers.push((die, end.model, drive));
        if drive.is_none() {
            rx_dies.push(die);
        }
    }
    match term {
        Term::None => {}
        Term::ToRef { r, v } => {
            for &d in &rx_dies {
                shunts.push((d, r.max(1e-3), v));
            }
        }
        Term::Across(r) => {
            if rx_dies.len() == 2 {
                series.push((rx_dies[0], rx_dies[1], r.max(1e-3), true));
            } else {
                notes.push("a termination across the pair needs a pair".into());
            }
        }
    }
    Ok(Built {
        c: Circuit {
            n,
            lines,
            caps,
            shunts,
            series,
            buffers,
        },
        at,
        rx_dies,
        parts,
        delay,
        att,
        notes,
    })
}

/// A driver's pullup share at each step: `bits` after a settling time of
/// `start` steps on the first bit, slewing at `t_r` and `t_f`.
#[allow(clippy::too_many_arguments)]
fn drive(
    bits: &[bool],
    invert: bool,
    start: usize,
    per_bit: f64,
    steps: usize,
    dt: f64,
    t_r: f64,
    t_f: f64,
) -> Vec<f64> {
    let want = |b: bool| if b ^ invert { 1.0 } else { 0.0 };
    let mut cur = want(bits[0]);
    let mut list = Vec::with_capacity(steps);
    for s in 0..steps {
        let bit = if s < start {
            bits[0]
        } else {
            bits[(((s - start) as f64 / per_bit) as usize).min(bits.len() - 1)]
        };
        let w = want(bit);
        if w > cur {
            cur = (cur + dt / t_r).min(w);
        } else if w < cur {
            cur = (cur - dt / t_f).max(w);
        }
        list.push(cur);
    }
    list
}

/// Simulates the route from the driver's pad to the receiver's (and, for a
/// pair, the partner's pads driven the other way) with the PRBS7 pattern.
pub fn simulate(
    board: &Board,
    pins: Pins,
    driver: End,
    receiver: End,
    setup: &Setup,
) -> Result<Run, String> {
    if !driver.model.kind.drives() {
        return Err(format!(
            "model {} is a {:?}, which cannot drive",
            driver.model.name, driver.model.kind
        ));
    }
    let diff = pins.driver_n.is_some() && pins.receiver_n.is_some();
    let mut ends = vec![(pins.driver, pins.receiver)];
    if let (Some(a), Some(b)) = (pins.driver_n, pins.receiver_n) {
        ends.push((a, b));
    }
    let corner = setup.corner;
    let t_r = switch_time(driver.model, corner, true);
    let t_f = switch_time(driver.model, corner, false);
    let ui = 1.0 / setup.rate.max(1.0);
    let dt = (t_r.min(t_f) / 25.0).min(ui / 40.0).clamp(0.2e-12, 5e-12);
    let loss = setup.loss.as_ref();
    let victim = build(
        board, &ends, diff, driver, receiver, setup.term, corner, dt, loss,
    )?;
    let mut notes = victim.notes.clone();
    // Settle with the first bit, then the pattern.
    let bits = prbs7(setup.bits.max(16));
    let settle = (2.0 * t_r.max(t_f)).max(0.5e-9);
    let start = (settle / dt).ceil() as usize;
    let per_bit = ui / dt;
    let steps = start + (bits.len() as f64 * per_bit).ceil() as usize;
    if steps > 3_000_000 {
        return Err(format!(
            "{} steps of {:.2} ps: lower the bit count or raise the rate",
            steps,
            dt * 1e12
        ));
    }
    let ku: Vec<Vec<f64>> = (0..victim.at.len())
        .map(|k| drive(&bits, k == 1, start, per_bit, steps, dt, t_r, t_f))
        .collect();
    let mut record = Vec::new();
    for (k, &(a, _)) in victim.at.iter().enumerate() {
        record.push(a);
        record.push(victim.rx_dies[k]);
    }
    let init = dc(&victim.c, corner, &|d| ku[d][0]);
    let ku_at = |d: usize, s: usize| ku[d][s];
    let (inject, aggressors) = if setup.crosstalk {
        crosstalk(
            board, &victim, &pins, driver, receiver, setup, dt, start, per_bit, steps, t_r, t_f,
            &mut notes,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let quiet = victim.c.run(
        dt,
        steps,
        corner,
        &ku_at,
        &record,
        Some(&init),
        &[],
        &Vec::new(),
    );
    let solved = if inject.is_empty() {
        None
    } else {
        Some(victim.c.run(
            dt,
            steps,
            corner,
            &ku_at,
            &record,
            Some(&init),
            &[],
            &inject,
        ))
    };
    let probes_of = |v: &[Vec<f32>]| -> Vec<Probe> {
        if diff {
            let d =
                |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a - b).collect::<Vec<f32>>();
            vec![
                Probe {
                    name: "Driver pins, P − N".into(),
                    v: d(&v[0], &v[2]),
                },
                Probe {
                    name: "Receiver die, P − N".into(),
                    v: d(&v[1], &v[3]),
                },
                Probe {
                    name: "Receiver die, P".into(),
                    v: v[1].clone(),
                },
                Probe {
                    name: "Receiver die, N".into(),
                    v: v[3].clone(),
                },
            ]
        } else {
            vec![
                Probe {
                    name: "Driver pin".into(),
                    v: v[0].clone(),
                },
                Probe {
                    name: "Receiver die".into(),
                    v: v[1].clone(),
                },
            ]
        }
    };
    let threshold = if diff {
        0.0
    } else {
        let (lo, hi) = receiver.model.thresholds(corner);
        (lo + hi) / 2.0
    };
    let quiet_probes = probes_of(&quiet.v);
    let quiet_eye = eye(&quiet_probes[1].v, dt, start, ui, &bits, threshold);
    let (probes, eye_, quiet, noise) = match solved {
        Some(sv) => {
            let probes = probes_of(&sv.v);
            let e = eye(&probes[1].v, dt, start, ui, &bits, threshold);
            let noise = probes[1].v[start.min(probes[1].v.len())..]
                .iter()
                .zip(&quiet_probes[1].v[start.min(quiet_probes[1].v.len())..])
                .map(|(a, b)| (a - b).abs() as f64)
                .fold(0.0, f64::max);
            (probes, e, Some(quiet_eye), noise)
        }
        None => (quiet_probes, quiet_eye, None, 0.0),
    };
    let nyquist = setup.rate / 2.0;
    let loss_db = loss.map_or(0.0, |l| victim.att.at(nyquist, l) * 8.686);
    Ok(Run {
        dt,
        start: start as f64 * dt,
        probes,
        eye: eye_,
        delay: victim.delay,
        switch: (t_r, t_f),
        bits,
        notes,
        loss_db,
        aggressors,
        quiet,
        noise,
    })
}

/// Finds the nets beside the route, drives each from its pad nearest the
/// route's driver into its farthest pad (with the route's driver and
/// receiver models, pairs driven against each other), and turns their
/// waves into crosstalk arriving at the route's lines.
#[allow(clippy::too_many_arguments)]
fn crosstalk(
    board: &Board,
    victim: &Built,
    pins: &Pins,
    driver: End,
    receiver: End,
    setup: &Setup,
    dt: f64,
    start: usize,
    per_bit: f64,
    steps: usize,
    t_r: f64,
    t_f: f64,
    notes: &mut Vec<String>,
) -> (Inject, Vec<Aggressor>) {
    let victims: Vec<u32> = victim.parts.iter().map(|p| p.net).collect();
    let raw = xtalk::find(board, &victims);
    if raw.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // The strongest few nets.
    let mut score: HashMap<u32, (f64, f64, f64)> = HashMap::new();
    for r in &raw {
        let e = score.entry(r.aggressor).or_insert((0.0, 0.0, 0.0));
        e.0 += r.kb.abs() * r.len;
        e.1 += r.len;
        e.2 = e.2.max(r.kb);
    }
    let mut nets: Vec<(u32, (f64, f64, f64))> = score.into_iter().collect();
    nets.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let mut dropped = 0;
    if nets.len() > 8 {
        dropped = nets.len() - 8;
        nets.truncate(8);
    }
    let g = &board.geometry;
    let from = g.pads.get(pins.driver).map(|p| p.at);
    let partners = board.pair_partners();
    let negatives: std::collections::HashSet<u32> = board
        .diff_pairs
        .iter()
        .filter_map(|d| board.nets.iter().position(|n| n.name == d.negative))
        .map(|i| i as u32)
        .collect();
    let chosen: Vec<u32> = nets.iter().map(|n| n.0).collect();
    let corner = setup.corner;
    let mut inj: HashMap<usize, (Vec<f32>, Vec<f32>)> = HashMap::new();
    let mut out = Vec::new();
    for (gi, &(net, (_, coupled, kb))) in nets.iter().enumerate() {
        let name = board.nets[net as usize].name.clone();
        let pads: Vec<usize> = g
            .pads
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.net == Some(net) && matches!(p.layer, LayerRef::Copper(_) | LayerRef::AllCopper)
            })
            .map(|(i, _)| i)
            .collect();
        if pads.len() < 2 {
            notes.push(format!(
                "{name} runs beside the route but has no two pads to drive"
            ));
            continue;
        }
        let d2 = |p: usize, q: crate::Point| {
            let a = g.pads[p].at;
            (a.x.mil() - q.x.mil()).powi(2) + (a.y.mil() - q.y.mil()).powi(2)
        };
        let dp = match from {
            Some(f) => *pads
                .iter()
                .min_by(|&&a, &&b| d2(a, f).total_cmp(&d2(b, f)))
                .unwrap(),
            None => pads[0],
        };
        let far = g.pads[dp].at;
        let rp = *pads
            .iter()
            .filter(|&&p| p != dp)
            .max_by(|&&a, &&b| d2(a, far).total_cmp(&d2(b, far)))
            .unwrap();
        // Pairs: the negative runs inverted on the positive's pattern.
        let partner = partners.get(&net).copied().filter(|p| chosen.contains(p));
        let inverted = partner.is_some() && negatives.contains(&net);
        let group = match partner {
            Some(p) if inverted => chosen.iter().position(|&c| c == p).unwrap_or(gi),
            _ => gi,
        };
        let term = match setup.term {
            Term::ToRef { r, v } => Term::ToRef { r, v },
            Term::Across(r) if partner.is_some() => Term::ToRef {
                r: r / 2.0,
                v: (driver.model.vcc(corner) + driver.model.vss(corner)) / 2.0,
            },
            _ => Term::None,
        };
        let none = Rlc::default();
        let built = match build(
            board,
            &[(dp, rp)],
            false,
            End {
                model: driver.model,
                pkg: none,
            },
            End {
                model: receiver.model,
                pkg: none,
            },
            term,
            corner,
            dt,
            setup.loss.as_ref(),
        ) {
            Ok(b) => b,
            Err(e) => {
                notes.push(format!("{name} not driven: {e}"));
                continue;
            }
        };
        let shift = 23 * (group + 1);
        let all = prbs7(setup.bits.max(16) + shift);
        let bits = &all[shift..];
        let ku = drive(bits, inverted, start, per_bit, steps, dt, t_r, t_f);
        let part = &built.parts[0];
        // The aggressor lines that couple, and where the victim's are.
        let mine: Vec<&xtalk::Raw> = raw.iter().filter(|r| r.aggressor == net).collect();
        let mut wanted: Vec<usize> = Vec::new();
        let mut couples = Vec::new();
        for r in mine {
            let Some(vp) = victim.parts.iter().find(|p| p.net == r.victim) else {
                continue;
            };
            let (Some(&(vd, vm)), Some(&(ad, am))) =
                (vp.tracks.get(&r.vtrack), part.tracks.get(&r.atrack))
            else {
                continue;
            };
            let vl = vp.line0 + vm.line;
            let al = part.line0 + am.line;
            let pos = |m: Moved, d: f64, x: f64| m.off + m.sign * x * d;
            let (t1, t2) = (pos(vm, vd, r.x.0), pos(vm, vd, r.x.1));
            let (u1, u2) = (pos(am, ad, r.r.0), pos(am, ad, r.r.1));
            // In the victim's direction.
            let (t1, t2, u1, u2) = if t1 <= t2 {
                (t1, t2, u1, u2)
            } else {
                (t2, t1, u2, u1)
            };
            if !wanted.contains(&al) {
                wanted.push(al);
            }
            couples.push((vl, t1, t2, al, u1, u2, r.kb, r.kf));
        }
        if couples.is_empty() {
            continue;
        }
        let init = dc(&built.c, corner, &|_| ku[0]);
        let solved = built.c.run(
            dt,
            steps,
            corner,
            &|_, s| ku[s],
            &[],
            Some(&init),
            &wanted,
            &Vec::new(),
        );
        let dl = |x: f64| (x / dt).round().max(0.0) as usize;
        for (vl, t1, t2, al, u1, u2, kb, kf) in couples {
            let wi = wanted.iter().position(|&w| w == al).unwrap();
            let (fa, fb) = &solved.waves[wi];
            let za = built.c.lines[al].line.z;
            let ta = built.c.lines[al].line.delay;
            let tv = victim.c.lines[vl].line.delay;
            // The aggressor's waves heading the victim's + and − ways,
            // as series with the delay to the stretch's entry.
            let (plus, plus_d, minus, minus_d) = if u2 >= u1 {
                (fa, u1, fb, ta - u2)
            } else {
                (fb, ta - u1, fa, u2)
            };
            let w = |f: &Vec<f32>, s: isize| -> f64 {
                let i = s.clamp(0, f.len() as isize - 1) as usize;
                f[i] as f64 * za / 2.0
            };
            let span = (t2 - t1).max(0.0);
            let e = inj
                .entry(vl)
                .or_insert_with(|| (vec![0.0; steps], vec![0.0; steps]));
            let (pd, md) = (dl(plus_d) as isize, dl(minus_d) as isize);
            let (d_t1, d_2span) = (dl(t1) as isize, dl(2.0 * span) as isize);
            let d_end = dl(tv - t2) as isize;
            let d_span = dl(span) as isize;
            for s in 0..steps as isize {
                // Near end, from the wave heading +: back to a.
                let p0 = s - d_t1 - pd;
                let next_a = kb * (w(plus, p0) - w(plus, p0 - d_2span));
                // Far end, at b.
                let pf = s - d_span - d_end - pd;
                let fext_b = kf * span * (w(plus, pf) - w(plus, pf - 1)) / dt;
                // From the wave heading −: near end at b, far end at a.
                let m0 = s - d_end - md;
                let next_b = kb * (w(minus, m0) - w(minus, m0 - d_2span));
                let mf = s - d_span - d_t1 - md;
                let fext_a = kf * span * (w(minus, mf) - w(minus, mf - 1)) / dt;
                e.0[s as usize] += (next_a + fext_a) as f32;
                e.1[s as usize] += (next_b + fext_b) as f32;
            }
        }
        out.push(Aggressor {
            net: name,
            from: pad_label(board, dp),
            coupled,
            kb,
            inverted,
        });
    }
    if dropped > 0 {
        notes.push(format!(
            "{dropped} more nets run beside the route more weakly; left out"
        ));
    }
    (inj.into_iter().map(|(l, (a, b))| (l, a, b)).collect(), out)
}

/// A pad as designator.pin.
pub fn pad_label(board: &Board, pad: usize) -> String {
    let Some(p) = board.geometry.pads.get(pad) else {
        return format!("pad {pad}");
    };
    match p.component.and_then(|c| board.components.get(c as usize)) {
        Some(c) => format!("{}.{}", c.designator, p.name),
        None => format!("pad {}", p.name),
    }
}

/// Folds the receiver's voltage into an eye two unit intervals wide,
/// centred on each bit (found from the first edge's crossing).
pub fn eye(v: &[f32], dt: f64, start: usize, ui: f64, bits: &[bool], threshold: f64) -> Eye {
    let per_bit = ui / dt;
    // The first edge of the pattern and when the receiver crosses for it.
    let Some(k1) = (1..bits.len()).find(|&k| bits[k] != bits[k - 1]) else {
        return Eye::default();
    };
    let edge = start + (k1 as f64 * per_bit) as usize;
    let rising = bits[k1];
    let cross = v[edge.min(v.len())..].iter().position(|&x| {
        if rising {
            x as f64 >= threshold
        } else {
            (x as f64) < threshold
        }
    });
    let Some(cross) = cross else {
        return Eye {
            ui,
            threshold,
            ..Default::default()
        };
    };
    let lag = cross as f64 * dt;
    // Trace samples: at most 200 over the 2 UI.
    let span = (2.0 * per_bit).round() as usize;
    let stride = span.div_ceil(200).max(1);
    let samples = span / stride + 1;
    let mut traces = Vec::new();
    let mut ones: Vec<Vec<f32>> = Vec::new();
    let mut zeros: Vec<Vec<f32>> = Vec::new();
    for (k, &bit) in bits.iter().enumerate().skip(4) {
        let centre = start as f64 * dt + k as f64 * ui + lag + ui / 2.0;
        let first = ((centre - ui) / dt).round();
        if first < 0.0 {
            continue;
        }
        let first = first as usize;
        if first + span >= v.len() {
            break;
        }
        let t: Vec<f32> = (0..samples).map(|i| v[first + i * stride]).collect();
        if bit {
            ones.push(t.clone());
        } else {
            zeros.push(t.clone());
        }
        traces.push(t);
    }
    let open = |i: usize| -> (f64, f64) {
        let hi = ones.iter().map(|t| t[i] as f64).fold(f64::MAX, f64::min);
        let lo = zeros.iter().map(|t| t[i] as f64).fold(f64::MIN, f64::max);
        (hi, lo)
    };
    let mid = samples / 2;
    let (high, low) = if ones.is_empty() || zeros.is_empty() {
        (0.0, 0.0)
    } else {
        open(mid)
    };
    let mut width = 0.0;
    if !ones.is_empty() && !zeros.is_empty() && high > low {
        let ok = |i: usize| {
            let (h, l) = open(i);
            h > threshold && l < threshold && h > l
        };
        let (mut a, mut b) = (mid, mid);
        while a > 0 && ok(a - 1) {
            a -= 1;
        }
        while b + 1 < samples && ok(b + 1) {
            b += 1;
        }
        if ok(mid) {
            width = (b - a) as f64 * stride as f64 * dt;
        }
    }
    Eye {
        ui,
        samples,
        traces,
        height: (high - low).max(0.0),
        width,
        high,
        low,
        threshold,
    }
}

/// The steady state before the first edge, with each driver held at
/// `ku(driver)`. Capacitors are open; inductors and lossless lines are
/// shorts, so the nodes they join are one voltage; a lossy line is the
/// two-port its DC gain g gives (Y11 = (1 + g²)/(1 − g²)/Z, Y12 =
/// −2g/(1 − g²)/Z). Solved by Newton's method; the currents through the
/// shorts then follow from the currents leaving each node.
fn dc(c: &Circuit<'_>, corner: Corner, ku: &dyn Fn(usize) -> f64) -> Init {
    let n = c.n;
    let gains: Vec<f64> = c.lines.iter().map(|s| s.gain()).collect();
    let short = |li: usize| 1.0 - gains[li] < 1e-9;
    let mut uf = Uf((0..n).collect());
    for (li, s) in c.lines.iter().enumerate() {
        if short(li) {
            uf.join(s.line.a, s.line.b);
        }
    }
    for &(a, b, _, is_r) in &c.series {
        if !is_r {
            uf.join(a, b);
        }
    }
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let mut sid = vec![0; n];
    for (i, x) in sid.iter_mut().enumerate() {
        let r = uf.find(i);
        let k = ids.len();
        *x = *ids.entry(r).or_insert(k);
    }
    let m = ids.len();
    // The linear part: G x = b.
    let mut gm = vec![0.0; m * m];
    let mut bv = vec![0.0; m];
    let stamp = |gm: &mut Vec<f64>, p: usize, q: usize, y: f64| {
        let (p, q) = (sid[p], sid[q]);
        gm[p * m + p] += y;
        gm[q * m + q] += y;
        gm[p * m + q] -= y;
        gm[q * m + p] -= y;
    };
    let ys: Vec<(f64, f64)> = c
        .lines
        .iter()
        .enumerate()
        .map(|(li, s)| {
            if short(li) {
                return (0.0, 0.0);
            }
            let (g, y) = (gains[li].clamp(0.0, 1.0), 1.0 / s.line.z);
            let d = 1.0 - g * g;
            (y * (1.0 + g * g) / d, -2.0 * g * y / d)
        })
        .collect();
    for (li, s) in c.lines.iter().enumerate() {
        if short(li) {
            continue;
        }
        let (y11, y12) = ys[li];
        let (p, q) = (sid[s.line.a], sid[s.line.b]);
        gm[p * m + p] += y11;
        gm[q * m + q] += y11;
        gm[p * m + q] += y12;
        gm[q * m + p] += y12;
    }
    for &(a, b, r, is_r) in &c.series {
        if is_r {
            stamp(&mut gm, a, b, 1.0 / r);
        }
    }
    for &(node, r, vr) in &c.shunts {
        let k = sid[node];
        gm[k * m + k] += 1.0 / r;
        bv[k] += vr / r;
    }
    for k in 0..m {
        gm[k * m + k] += 1e-12;
    }
    let level =
        |model: &Model, u: f64| model.vss(corner) + u * (model.vcc(corner) - model.vss(corner));
    let guess = c
        .buffers
        .iter()
        .find_map(|&(_, model, d)| d.map(|d| level(model, ku(d))))
        .unwrap_or(0.0);
    let mut x = vec![guess; m];
    let drive = |d: Option<usize>| match d {
        Some(d) => (ku(d), 1.0 - ku(d)),
        None => (0.0, 0.0),
    };
    for _ in 0..200 {
        let mut jac = gm.clone();
        let mut r: Vec<f64> = (0..m)
            .map(|k| (0..m).map(|j| gm[k * m + j] * x[j]).sum::<f64>() - bv[k])
            .collect();
        for &(node, model, d) in &c.buffers {
            let k = sid[node];
            let (u, dn) = drive(d);
            r[k] += model.current(x[k], u, dn, corner);
            jac[k * m + k] += model.conductance(x[k], u, dn, corner).max(0.0);
        }
        let dx = solve(&mut jac, &mut r, m);
        let mut worst: f64 = 0.0;
        for k in 0..m {
            let s = dx[k].clamp(-0.5, 0.5);
            x[k] -= s;
            worst = worst.max(s.abs());
        }
        if worst < 1e-10 {
            break;
        }
    }
    let v: Vec<f64> = (0..n).map(|i| x[sid[i]]).collect();
    // Currents leaving each node through everything but the shorts.
    let mut leave = vec![0.0; n];
    let mut line_i = vec![(0.0, 0.0); c.lines.len()];
    for (li, s) in c.lines.iter().enumerate() {
        if short(li) {
            continue;
        }
        let (y11, y12) = ys[li];
        let (va, vb) = (v[s.line.a], v[s.line.b]);
        let (ia, ib) = (y11 * va + y12 * vb, y11 * vb + y12 * va);
        line_i[li] = (ia, ib);
        leave[s.line.a] += ia;
        leave[s.line.b] += ib;
    }
    for &(a, b, r, is_r) in &c.series {
        if is_r {
            let i = (v[a] - v[b]) / r;
            leave[a] += i;
            leave[b] -= i;
        }
    }
    for &(node, r, vr) in &c.shunts {
        leave[node] += (v[node] - vr) / r;
    }
    for &(node, model, d) in &c.buffers {
        let (u, dn) = drive(d);
        leave[node] += model.current(v[node], u, dn, corner);
    }
    // The shorts: a spanning tree of each group carries what its far side
    // sends out; edges closing loops carry nothing.
    // Edge: node, node, line (true) or inductor, index.
    let mut adj: Vec<Vec<(usize, bool, usize)>> = vec![Vec::new(); n];
    for (li, s) in c.lines.iter().enumerate() {
        if short(li) {
            adj[s.line.a].push((s.line.b, true, li));
            adj[s.line.b].push((s.line.a, true, li));
        }
    }
    for (si, &(a, b, _, is_r)) in c.series.iter().enumerate() {
        if !is_r {
            adj[a].push((b, false, si));
            adj[b].push((a, false, si));
        }
    }
    let mut ind_i = vec![0.0; c.series.len()];
    let mut seen = vec![false; n];
    let mut sub = leave.clone();
    for root in 0..n {
        if seen[root] || adj[root].is_empty() {
            continue;
        }
        seen[root] = true;
        // Breadth-first order with each node's parent edge.
        let mut order = vec![(root, None::<(usize, bool, usize)>)];
        let mut head = 0;
        while head < order.len() {
            let i = order[head].0;
            head += 1;
            for &(j, is_line, e) in &adj[i] {
                if !seen[j] {
                    seen[j] = true;
                    order.push((j, Some((i, is_line, e))));
                }
            }
        }
        for &(child, up) in order.iter().rev() {
            let Some((parent, is_line, e)) = up else {
                continue;
            };
            // Flow from child to parent.
            let f = -sub[child];
            sub[parent] += sub[child];
            if is_line {
                let l = &c.lines[e].line;
                line_i[e] = if l.a == child { (f, -f) } else { (-f, f) };
            } else {
                let (a, _, _, _) = c.series[e];
                ind_i[e] = if a == child { f } else { -f };
            }
        }
    }
    Init { v, line_i, ind_i }
}

/// A part's pad, by designator and pin name.
pub fn find_pad(board: &Board, designator: &str, pin: &str) -> Option<usize> {
    let c = board
        .components
        .iter()
        .position(|c| c.designator.eq_ignore_ascii_case(designator))? as u32;
    board
        .geometry
        .pads
        .iter()
        .position(|p| p.component == Some(c) && p.name.eq_ignore_ascii_case(pin))
}

/// The pad of the pair partner's net on the same part as `pad`.
pub fn partner_pad(board: &Board, pad: usize) -> Option<usize> {
    let p = board.geometry.pads.get(pad)?;
    let other = *board.pair_partners().get(&p.net?)?;
    board
        .geometry
        .pads
        .iter()
        .position(|q| q.component == p.component && q.net == Some(other))
}

/// The model and package parasitics an IBIS file gives a pin: `name` when
/// given, else the model its [Pin] table names, else the only model that
/// fits (`drive` for a driver).
pub fn pin_model<'a>(
    ibis: &'a crate::ibis::Ibis,
    pin: &str,
    name: Option<&str>,
    drive: bool,
) -> Result<(&'a Model, Rlc), String> {
    let entry = ibis
        .components
        .iter()
        .find_map(|c| c.pin(pin).map(|p| (c, p)));
    let pkg = entry
        .map(|(c, p)| c.rlc(p))
        .or_else(|| ibis.components.first().map(|c| c.package))
        .unwrap_or_default();
    if let Some(n) = name {
        return ibis
            .model(n)
            .map(|m| (m, pkg))
            .ok_or_else(|| format!("no model {n} in the IBIS file"));
    }
    if let Some(m) = entry.and_then(|(_, p)| ibis.model(&p.model)) {
        return Ok((m, pkg));
    }
    let fits: Vec<&Model> = ibis
        .models
        .iter()
        .filter(|m| !drive || m.kind.drives())
        .collect();
    match fits.as_slice() {
        [m] => Ok((m, pkg)),
        [] => Err("no model in the IBIS file fits".into()),
        _ => Err(format!(
            "pin {pin} is not in the IBIS file's [Pin] table: name a model ({})",
            fits.iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// A picture of a run: the first bits of each probe over time, and the eye.
pub fn svg(run: &Run, title: &str) -> String {
    let (w, h, pad) = (900.0, 300.0, 50.0);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{}\" font-family=\"sans-serif\" font-size=\"12\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#16181d\"/>\
         <text x=\"{pad}\" y=\"24\" fill=\"#e8e8e8\" font-size=\"15\">{}</text>",
        2.0 * h + 120.0,
        xml(title)
    );
    let colours = ["#ffd060", "#5ec8ff", "#ff7aa8", "#9be27a"];
    // Waveforms: 16 bits from the start.
    let n = ((16.0 * run.eye.ui / run.dt) as usize).max(2);
    let first = (run.start / run.dt) as usize;
    let span: Vec<&[f32]> = run
        .probes
        .iter()
        .map(|p| &p.v[first.min(p.v.len())..(first + n).min(p.v.len())])
        .collect();
    let (lo, hi) = span
        .iter()
        .flat_map(|s| s.iter())
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let (lo, hi) = (lo as f64 - 0.1, hi as f64 + 0.1);
    let top = 40.0;
    let y = |v: f64, top: f64, lo: f64, hi: f64| top + h - (v - lo) / (hi - lo) * h;
    out += &frame(
        pad,
        top,
        w - 2.0 * pad,
        h,
        lo,
        hi,
        16.0 * run.eye.ui,
        "ns",
        1e9,
    );
    for (k, s) in span.iter().enumerate() {
        let step = (s.len() / 1500).max(1);
        let pts: Vec<String> = s
            .iter()
            .enumerate()
            .step_by(step)
            .map(|(i, &v)| {
                format!(
                    "{:.1},{:.1}",
                    pad + i as f64 / n as f64 * (w - 2.0 * pad),
                    y(v as f64, top, lo, hi)
                )
            })
            .collect();
        out += &format!(
            "<polyline fill=\"none\" stroke=\"{}\" stroke-width=\"1.2\" points=\"{}\"/>\
             <text x=\"{}\" y=\"{}\" fill=\"{}\">{}</text>",
            colours[k % 4],
            pts.join(" "),
            pad + 10.0 + 200.0 * k as f64,
            top + 14.0,
            colours[k % 4],
            xml(&run.probes[k].name)
        );
    }
    // Eye.
    let e = &run.eye;
    let top = top + h + 40.0;
    let (lo, hi) = e
        .traces
        .iter()
        .flatten()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let (lo, hi) = (lo as f64 - 0.1, hi as f64 + 0.1);
    let ew = h * 1.6;
    out += &frame(pad, top, ew, h, lo, hi, 2.0 * e.ui, "ps", 1e12);
    for t in &e.traces {
        let pts: Vec<String> = t
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                format!(
                    "{:.1},{:.1}",
                    pad + i as f64 / (e.samples.max(2) - 1) as f64 * ew,
                    y(v as f64, top, lo, hi)
                )
            })
            .collect();
        out += &format!(
            "<polyline fill=\"none\" stroke=\"#5ec8ff\" stroke-opacity=\"0.25\" stroke-width=\"1\" points=\"{}\"/>",
            pts.join(" ")
        );
    }
    let text = [
        format!(
            "Eye height {:.3} V, width {:.1} ps ({:.0} % UI)",
            e.height,
            e.width * 1e12,
            e.width / e.ui * 100.0
        ),
        format!(
            "Unit interval {:.1} ps, threshold {:.2} V",
            e.ui * 1e12,
            e.threshold
        ),
        format!(
            "Route delay {:.1} ps, loss {:.2} dB at {:.2} GHz",
            run.delay * 1e12,
            run.loss_db,
            0.5 / e.ui / 1e9
        ),
        match &run.quiet {
            Some(q) => format!(
                "Crosstalk {:.1} mV peak from {} nets (quiet eye {:.3} V)",
                run.noise * 1e3,
                run.aggressors.len(),
                q.height
            ),
            None => "No crosstalk added".to_string(),
        },
        format!(
            "Driver edges {:.0} / {:.0} ps (switching)",
            run.switch.0 * 1e12,
            run.switch.1 * 1e12
        ),
    ];
    for (i, t) in text.iter().chain(run.notes.iter()).enumerate() {
        out += &format!(
            "<text x=\"{}\" y=\"{}\" fill=\"#e8e8e8\">{}</text>",
            pad + ew + 40.0,
            top + 20.0 + 20.0 * i as f64,
            xml(t)
        );
    }
    out += "</svg>";
    out
}

#[allow(clippy::too_many_arguments)]
fn frame(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    lo: f64,
    hi: f64,
    span: f64,
    unit: &str,
    k: f64,
) -> String {
    let mut s = format!(
        "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"none\" stroke=\"#555\"/>"
    );
    for i in 0..=4 {
        let v = lo + (hi - lo) * i as f64 / 4.0;
        let yy = y + h - h * i as f64 / 4.0;
        s += &format!(
            "<line x1=\"{x}\" x2=\"{}\" y1=\"{yy:.1}\" y2=\"{yy:.1}\" stroke=\"#333\"/>\
             <text x=\"{}\" y=\"{:.1}\" fill=\"#999\" text-anchor=\"end\">{v:.2} V</text>",
            x + w,
            x - 4.0,
            yy + 4.0
        );
        let t = span * i as f64 / 4.0 * k;
        let xx = x + w * i as f64 / 4.0;
        s += &format!(
            "<text x=\"{xx:.1}\" y=\"{}\" fill=\"#999\" text-anchor=\"middle\">{t:.1} {unit}</text>",
            y + h + 15.0
        );
    }
    s
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ibis;

    /// A board with one straight 50 ohm-ish track between two pads.
    fn board(len: f64) -> Board {
        use crate::{LayerKind, Length, Net, Pad, PadShape, Point, StackLayer, Track};
        let mut b = Board::default();
        let layer = |name: &str, kind, t: f64, er: Option<f64>| StackLayer {
            name: name.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(t),
            dielectric_constant: er,
            material: None,
        };
        b.stackup.layers = vec![
            layer("Top", LayerKind::Copper, 1.4, None),
            layer("D1", LayerKind::Dielectric, 5.5, Some(4.2)),
            layer("Bottom", LayerKind::Copper, 1.4, None),
        ];
        b.nets = vec![Net {
            name: "S".into(),
            ..Default::default()
        }];
        let p = |x: f64| Point {
            x: Length::from_mil(x),
            y: Length::from_mil(0.0),
        };
        let pad = |x: f64, name: &str| Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: None,
            name: name.into(),
            at: p(x),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        b.geometry.pads = vec![pad(0.0, "1"), pad(len, "2")];
        let w = b.layer_geometry()[0].width_for(50.0).unwrap();
        b.geometry.tracks = vec![Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: p(0.0),
            end: p(len),
            width: Length::from_mil(w),
            text: false,
        }];
        b
    }

    #[test]
    fn pair_across_100_ohm_swings_as_the_divider_says() {
        use crate::{DiffPair, Length, Net};
        let mut b = board(2000.0);
        // A partner 10 mil away (edge to edge), pads beside.
        let mut t = b.geometry.tracks[0].clone();
        let off = Length::from_mil(10.0 + t.width.mil());
        t.net = Some(1);
        t.start.y = off;
        t.end.y = off;
        b.geometry.tracks.push(t);
        for i in 0..2 {
            let mut p = b.geometry.pads[i].clone();
            p.net = Some(1);
            p.at.y = off;
            p.size_y = Length::from_mil(8.0);
            p.size_x = Length::from_mil(8.0);
            b.geometry.pads.push(p);
            b.geometry.pads[i].size_y = Length::from_mil(8.0);
            b.geometry.pads[i].size_x = Length::from_mil(8.0);
        }
        b.nets.push(Net {
            name: "S_N".into(),
            ..Default::default()
        });
        b.nets[0].name = "S_P".into();
        b.diff_pairs.push(DiffPair {
            name: "S".into(),
            positive: "S_P".into(),
            negative: "S_N".into(),
        });
        let f = ibis::parse(ibis::SAMPLE).unwrap();
        let (drv, rcv) = (f.model("drv33").unwrap(), f.model("rcv33").unwrap());
        let none = Rlc::default();
        let pins = Pins {
            driver: 0,
            receiver: 1,
            driver_n: Some(2),
            receiver_n: Some(3),
        };
        let setup = Setup {
            rate: 200e6,
            bits: 32,
            term: Term::Across(100.0),
            ..Default::default()
        };
        let run = simulate(
            &b,
            pins,
            End {
                model: drv,
                pkg: none,
            },
            End {
                model: rcv,
                pkg: none,
            },
            &setup,
        )
        .unwrap();
        // 3.3 V over 25 + 100 + 25 ohm: 2.2 V across the termination.
        assert!((run.eye.high - 2.2).abs() < 0.15, "{}", run.eye.high);
        assert!((run.eye.low + 2.2).abs() < 0.15, "{}", run.eye.low);
        // Before the first edge, no ringing: the start is the DC state.
        let first = &run.probes[1].v[..(run.start / run.dt) as usize];
        let (lo, hi) = first
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi - lo < 1e-3, "{lo} {hi}");
    }

    #[test]
    fn stripline_neighbour_adds_its_near_end_crosstalk() {
        use crate::{LayerKind, Length, Net, Pad, PadShape, Point, StackLayer, Track};
        let mut b = Board::default();
        let layer = |name: &str, kind, t: f64, er: Option<f64>| StackLayer {
            name: name.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(t),
            dielectric_constant: er,
            material: None,
        };
        b.stackup.layers = vec![
            layer("Top", LayerKind::Copper, 1.4, None),
            layer("D1", LayerKind::Dielectric, 5.0, Some(4.2)),
            layer("In1", LayerKind::Copper, 0.7, None),
            layer("D2", LayerKind::Dielectric, 5.0, Some(4.2)),
            layer("Bottom", LayerKind::Copper, 1.4, None),
        ];
        b.nets = vec![
            Net {
                name: "V".into(),
                ..Default::default()
            },
            Net {
                name: "A".into(),
                ..Default::default()
            },
        ];
        let geo = b.layer_geometry();
        let w = geo[1].width_for(50.0).unwrap();
        let gap = w;
        let p = |x: f64, y: f64| Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        };
        let pad = |x: f64, y: f64, net: u32| Pad {
            layer: LayerRef::Copper(1),
            net: Some(net),
            component: None,
            name: "1".into(),
            at: p(x, y),
            size_x: Length::from_mil(w),
            size_y: Length::from_mil(w),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        let len = 3000.0;
        let off = w + gap;
        b.geometry.pads = vec![
            pad(0.0, 0.0, 0),
            pad(len, 0.0, 0),
            pad(0.0, off, 1),
            pad(len, off, 1),
        ];
        let track = |y: f64, net: u32| Track {
            component: None,
            layer: LayerRef::Copper(1),
            net: Some(net),
            start: p(0.0, y),
            end: p(len, y),
            width: Length::from_mil(w),
            text: false,
        };
        b.geometry.tracks = vec![track(0.0, 0), track(off, 1)];
        let f = ibis::parse(ibis::SAMPLE).unwrap();
        let (drv, rcv) = (f.model("drv33").unwrap(), f.model("rcv33").unwrap());
        let none = Rlc::default();
        let pins = Pins {
            driver: 0,
            receiver: 1,
            driver_n: None,
            receiver_n: None,
        };
        let setup = Setup {
            rate: 200e6,
            bits: 32,
            term: Term::ToRef { r: 50.0, v: 1.65 },
            loss: None,
            crosstalk: true,
            ..Default::default()
        };
        let run = simulate(
            &b,
            pins,
            End {
                model: drv,
                pkg: none,
            },
            End {
                model: rcv,
                pkg: none,
            },
            &setup,
        )
        .unwrap();
        assert_eq!(run.aggressors.len(), 1);
        let (ze, zo, ee, eo) = xtalk::modes(&geo[1], w, gap).unwrap();
        let (kb, kf) = xtalk::coefficients(ze, zo, ee, eo);
        assert!(kf.abs() < 1e-9);
        // The aggressor launches 2.2 V edges (25 ohm into 50); its
        // near-end crosstalk, Kb of that, reaches the victim's driver,
        // which sends back -1/3 of it to the matched receiver: a plateau
        // as long as the coupled stretch's round trip (the capacitance at
        // the driver adds a spike at its front).
        let want = kb * 2.2 / 3.0;
        let mut q = setup;
        q.crosstalk = false;
        let calm = simulate(
            &b,
            pins,
            End {
                model: drv,
                pkg: none,
            },
            End {
                model: rcv,
                pkg: none,
            },
            &q,
        )
        .unwrap();
        let mut d: Vec<f64> = run.probes[1]
            .v
            .iter()
            .zip(&calm.probes[1].v)
            .map(|(a, c)| (a - c).abs() as f64)
            .filter(|x| *x > want / 2.0)
            .collect();
        d.sort_by(f64::total_cmp);
        let level = d[d.len() / 2];
        assert!((level - want).abs() < 0.15 * want, "{level} against {want}");
        assert!(run.noise < 2.0 * want, "{}", run.noise);
        let quiet = run.quiet.as_ref().unwrap();
        assert!(run.eye.height <= quiet.height + 1e-6);
    }

    #[test]
    fn reduced_lines_keep_where_tracks_were() {
        // One route as two tracks meeting in the middle, the second drawn
        // backwards: one line after reduction, with both tracks mapped
        // onto it the right way round.
        let mut b = board(3000.0);
        let mut t = b.geometry.tracks[0].clone();
        b.geometry.tracks[0].end.x = crate::Length::from_mil(1500.0);
        t.start.x = crate::Length::from_mil(3000.0);
        t.end.x = crate::Length::from_mil(1500.0);
        b.geometry.tracks.push(t);
        let full = network(&b, 0, false);
        let (nw, nodes, moved) = full.reduce(1e-12);
        assert_eq!(nw.lines.len(), 1);
        let l = nw.lines[0];
        let pad0 = nodes[full.node_of_pad(0).unwrap()];
        let at = |track: usize, x: f64| {
            let &(_, li) = full.track_lines.iter().find(|t| t.0 == track).unwrap();
            let m = moved[li].unwrap();
            m.off + m.sign * x * full.lines[li].delay
        };
        // Distance (as delay) from the line's a end of each track's start.
        let from_pad0 = |d: f64| if l.a == pad0 { d } else { l.delay - d };
        assert!((from_pad0(at(0, 0.0))).abs() < 1e-15);
        assert!((from_pad0(at(0, 1.0)) - l.delay / 2.0).abs() < 1e-14);
        assert!((from_pad0(at(1, 0.0)) - l.delay).abs() < 1e-14);
        assert!((from_pad0(at(1, 1.0)) - l.delay / 2.0).abs() < 1e-14);
    }

    #[test]
    fn loss_closes_a_fast_eye_but_keeps_the_levels() {
        let f = ibis::parse(ibis::SAMPLE).unwrap();
        let (drv, rcv) = (f.model("drv33").unwrap(), f.model("rcv33").unwrap());
        let b = board(10000.0);
        let pins = Pins {
            driver: 0,
            receiver: 1,
            driver_n: None,
            receiver_n: None,
        };
        let none = Rlc::default();
        let run = |rate: f64, loss: Option<Loss>| {
            let setup = Setup {
                rate,
                bits: 64,
                term: Term::ToRef { r: 50.0, v: 0.0 },
                loss,
                crosstalk: false,
                ..Default::default()
            };
            simulate(
                &b,
                pins,
                End {
                    model: drv,
                    pkg: none,
                },
                End {
                    model: rcv,
                    pkg: none,
                },
                &setup,
            )
            .unwrap()
        };
        let lossy = Some(Loss::default());
        // Ten inches of FR-4 microstrip: a couple of dB at 1 GHz.
        let fast = run(2e9, lossy);
        assert!(fast.loss_db > 1.0 && fast.loss_db < 6.0, "{}", fast.loss_db);
        let ideal = run(2e9, None);
        assert!(fast.eye.height < ideal.eye.height - 0.05);
        // Slowly, the ones settle near 2.2 V either way.
        let slow = run(50e6, lossy);
        assert!((slow.eye.high - 2.2).abs() < 0.1, "{}", slow.eye.high);
    }

    #[test]
    fn prbs7_repeats_every_127() {
        let b = prbs7(254);
        assert_eq!(&b[..127], &b[127..]);
        assert_eq!(b.iter().filter(|x| **x).count(), 128);
    }

    #[test]
    fn network_of_a_track_is_one_line() {
        let b = board(3000.0);
        let nw = network(&b, 0, false);
        assert_eq!(nw.lines.len(), 1);
        let l = nw.lines[0];
        assert!((l.z - 50.0).abs() < 0.5, "{}", l.z);
        // 3 inches of microstrip: about 430 ps.
        assert!(l.delay > 380e-12 && l.delay < 480e-12, "{}", l.delay);
        let (a, b2) = (nw.node_of_pad(0).unwrap(), nw.node_of_pad(1).unwrap());
        assert!(nw.joined(a, b2));
    }

    #[test]
    fn switching_time_reproduces_the_ramp() {
        let f = ibis::parse(ibis::SAMPLE).unwrap();
        let m = f.model("drv33").unwrap();
        let t = switch_time(m, Corner::Typ, true);
        assert!(t > 0.05e-9 && t < 2e-9, "{t}");
    }

    #[test]
    fn open_line_doubles_and_matched_line_does_not() {
        let f = ibis::parse(ibis::SAMPLE).unwrap();
        let drv = f.model("drv33").unwrap();
        let rcv = f.model("rcv33").unwrap();
        let b = board(3000.0);
        let pins = Pins {
            driver: 0,
            receiver: 1,
            driver_n: None,
            receiver_n: None,
        };
        let none = Rlc::default();
        let run = |term| {
            let setup = Setup {
                rate: 200e6,
                bits: 32,
                term,
                ..Default::default()
            };
            simulate(
                &b,
                pins,
                End {
                    model: drv,
                    pkg: none,
                },
                End {
                    model: rcv,
                    pkg: none,
                },
                &setup,
            )
            .unwrap()
        };
        // The driver is 25 ohm (132 mA at 3.3 V): into 50 ohm it launches
        // 2/3 of 3.3 V; an open end doubles it to about 4.4 V, which the
        // clamps hold near 3.3 V + a diode. Matched (50 ohm to ground at
        // the end), the receiver settles to 3.3 * 50 / 75 = 2.2 V.
        let open = run(Term::None);
        let rx = &open.probes[1].v;
        let peak = rx.iter().cloned().fold(f32::MIN, f32::max);
        assert!(peak > 3.5, "{peak}");
        assert!(open.delay > 380e-12 && open.delay < 480e-12);
        let matched = run(Term::ToRef { r: 50.0, v: 0.0 });
        let rx = &matched.probes[1].v;
        let peak = rx.iter().cloned().fold(f32::MIN, f32::max);
        assert!((peak - 2.2).abs() < 0.15, "{peak}");
        // An open eye at 200 Mb/s.
        assert!(matched.eye.height > 1.5, "{:?}", matched.eye.height);
        assert!(matched.eye.width > 0.6 / 200e6, "{}", matched.eye.width);
    }
}
