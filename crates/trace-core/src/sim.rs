//! Signal integrity simulation: a routed net (or pair) driven by an IBIS
//! buffer into an IBIS receiver, in the time domain, with an eye diagram.
//!
//! The net's copper becomes lossless transmission lines: each track and arc
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
//! Losses, coupling to other nets and power noise are not modelled.

use crate::backdrill::faces;
use crate::ibis::{Corner, Model, Rlc};
use crate::impedance::DEFAULT_ER;
use crate::{Board, LayerRef};
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
    // Edges between anchors: z, delay.
    let mut edges: Vec<(usize, usize, f64, f64)> = Vec::new();
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
                  edges: &mut Vec<(usize, usize, f64, f64)>,
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
            edges.push((a, b, zv, len / 1000.0 * er.sqrt() / C_IN));
        }
        ids
    };
    for t in &g.tracks {
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
        edges.push((n - 2, n - 1, z, delay_of(l, w, len)));
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
        edges.push((n - 2, n - 1, z_of(l, w), delay_of(l, w, len)));
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
    for &(a, b, z, delay) in &edges {
        let (na, nb) = (node(&mut uf, a), node(&mut uf, b));
        if na != nb {
            lines.push(Line {
                a: na,
                b: nb,
                z,
                delay,
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
    }
}

impl Network {
    /// Lines shorter than half a step become capacitance at a node
    /// (their two ends joined): C = delay / Z. Returns the reduced network
    /// and where each old node went.
    fn reduce(&self, dt: f64) -> (Network, Vec<usize>) {
        // First join lines in series through nodes nothing else touches:
        // their delays add, and the impedance is the delay-weighted mean.
        let mut slots: Vec<Option<Line>> = self.lines.iter().copied().map(Some).collect();
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
            slots[i] = Some(Line {
                a: p,
                b: q,
                z,
                delay: d,
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
        let merged: Vec<Line> = slots.into_iter().flatten().collect();
        let mut uf = Uf((0..self.nodes).collect());
        let mut caps: Vec<(usize, f64)> = self.caps.clone();
        for l in &merged {
            if l.delay < dt / 2.0 {
                uf.join(l.a, l.b);
                caps.push((l.a, l.delay / l.z));
            }
        }
        let mut ids: HashMap<usize, usize> = HashMap::new();
        let mut map = vec![0; self.nodes];
        for (i, m) in map.iter_mut().enumerate() {
            let r = uf.find(i);
            let n = ids.len();
            *m = *ids.entry(r).or_insert(n);
        }
        let lines = merged
            .iter()
            .filter(|l| l.delay >= dt / 2.0)
            .map(|l| Line {
                a: map[l.a],
                b: map[l.b],
                ..*l
            })
            .collect();
        let caps = caps.into_iter().map(|(n, c)| (map[n], c)).collect();
        let pads = self.pads.iter().map(|&(p, n)| (p, map[n])).collect();
        (
            Network {
                nodes: ids.len(),
                lines,
                caps,
                pads,
                ..self.clone()
            },
            map,
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

    /// The shortest delay between two nodes, seconds.
    pub fn delay_between(&self, a: usize, b: usize) -> Option<f64> {
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); self.nodes];
        for l in &self.lines {
            adj[l.a].push((l.b, l.delay));
            adj[l.b].push((l.a, l.delay));
        }
        let mut best = vec![f64::MAX; self.nodes];
        best[a] = 0.0;
        let mut todo = vec![a];
        while let Some(i) = todo.pop() {
            for &(j, d) in &adj[i] {
                if best[i] + d < best[j] - 1e-18 {
                    best[j] = best[i] + d;
                    todo.push(j);
                }
            }
        }
        (best[b] < f64::MAX).then_some(best[b])
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
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            corner: Corner::Typ,
            rate: 1e9,
            bits: 160,
            term: Term::None,
        }
    }
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

/// The circuit the solver steps: nodes, lines, and elements.
struct Circuit<'a> {
    n: usize,
    lines: Vec<(Line, usize)>,
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
}

/// A starting state: node voltages, the current into each line at its
/// `a` end (out at `b`), and through each inductor.
struct Init {
    v: Vec<f64>,
    line_i: Vec<f64>,
    ind_i: Vec<f64>,
}

impl<'a> Circuit<'a> {
    /// Steps the circuit for `steps` of `dt`, the drivers' pullup share
    /// given by `ku(driver, step)`, recording the voltages of `record`.
    fn run(
        &self,
        dt: f64,
        steps: usize,
        corner: Corner,
        ku: &dyn Fn(usize, usize) -> f64,
        record: &[usize],
        init: Option<&Init>,
    ) -> Solved {
        let n = self.n;
        // Linear conductance to ground per node, and history sources.
        let mut g = vec![0.0; n];
        for (l, _) in &self.lines {
            g[l.a] += 1.0 / l.z;
            g[l.b] += 1.0 / l.z;
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
        let mut fa: Vec<Vec<f64>> = self.lines.iter().map(|(_, k)| vec![0.0; *k]).collect();
        let mut fb = fa.clone();
        if let Some(init) = init {
            v.copy_from_slice(&init.v);
            for (li, (l, _)) in self.lines.iter().enumerate() {
                let i = init.line_i[li];
                fa[li].iter_mut().for_each(|x| *x = v[l.a] / l.z + i);
                fb[li].iter_mut().for_each(|x| *x = v[l.b] / l.z - i);
            }
            for (ci, &(node, _)) in self.caps.iter().enumerate() {
                cap_v[ci] = v[node];
            }
            ind_i.copy_from_slice(&init.ind_i);
        }
        let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(steps); record.len()];
        let mut src = vec![0.0; n];
        let mut hist = vec![(0.0, 0.0); self.lines.len()];
        for step in 0..steps {
            src.iter_mut().for_each(|s| *s = 0.0);
            for (li, (l, k)) in self.lines.iter().enumerate() {
                let idx = step % k;
                let (ha, hb) = (fb[li][idx], fa[li][idx]);
                hist[li] = (ha, hb);
                src[l.a] += ha;
                src[l.b] += hb;
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
            for (li, (l, k)) in self.lines.iter().enumerate() {
                let idx = step % k;
                let (ha, hb) = hist[li];
                fa[li][idx] = 2.0 * v[l.a] / l.z - ha;
                fb[li][idx] = 2.0 * v[l.b] / l.z - hb;
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
        Solved { v: out }
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
        let v = &circuit.run(dt, steps, corner, &ku, &[0], None).v[0];
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

/// Simulates the route from the driver's pad to the receiver's (and, for a
/// pair, the partner's pads driven the other way) with the PRBS7 pattern.
pub fn simulate(
    board: &Board,
    pins: Pins,
    driver: End,
    receiver: End,
    setup: &Setup,
) -> Result<Run, String> {
    let pad_net = |p: usize| -> Result<u32, String> {
        let pad = board
            .geometry
            .pads
            .get(p)
            .ok_or_else(|| format!("no pad {p}"))?;
        pad.net.ok_or_else(|| "the pad has no net".to_string())
    };
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
    let mut notes = Vec::new();
    // Each net's network, reduced, its nodes offset into the circuit.
    let mut n = 0;
    let mut lines = Vec::new();
    let mut caps = Vec::new();
    let mut at: Vec<(usize, usize)> = Vec::new();
    let mut delay: f64 = 0.0;
    for &(dp, rp) in &ends {
        let net = pad_net(dp)?;
        if pad_net(rp)? != net {
            return Err(format!(
                "the driver and receiver pads are on different nets ({} and {})",
                board.nets[net as usize].name,
                board.nets[pad_net(rp)? as usize].name
            ));
        }
        let full = network(board, net, diff);
        let (nw, _) = full.reduce(dt);
        let (Some(a), Some(b)) = (nw.node_of_pad(dp), nw.node_of_pad(rp)) else {
            return Err("pad not on a copper layer".into());
        };
        if a != b && !nw.joined(a, b) {
            return Err(format!(
                "{} is not routed from the driver to the receiver",
                board.nets[net as usize].name
            ));
        }
        delay = delay.max(nw.delay_between(a, b).unwrap_or(0.0));
        if nw.pads.len() > 2 {
            notes.push(format!(
                "{} has {} other pads; they load the net as open stubs",
                board.nets[net as usize].name,
                nw.pads.len() - 2
            ));
        }
        for l in &nw.lines {
            let k = ((l.delay / dt).round() as usize).max(1);
            lines.push((
                Line {
                    a: l.a + n,
                    b: l.b + n,
                    ..*l
                },
                k,
            ));
        }
        caps.extend(nw.caps.iter().map(|&(i, c)| (i + n, c)));
        at.push((a + n, b + n));
        n += nw.nodes;
    }
    // Buffers, dies and packages.
    let mut shunts = Vec::new();
    let mut series = Vec::new();
    let mut buffers = Vec::new();
    let mut dies = Vec::new();
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
            dies.push(die);
        }
    }
    match setup.term {
        Term::None => {}
        Term::ToRef { r, v } => {
            for &d in &dies {
                shunts.push((d, r.max(1e-3), v));
            }
        }
        Term::Across(r) => {
            if dies.len() == 2 {
                series.push((dies[0], dies[1], r.max(1e-3), true));
            } else {
                notes.push("a termination across the pair needs a pair".into());
            }
        }
    }
    let circuit = Circuit {
        n,
        lines,
        caps,
        shunts,
        series,
        buffers,
    };
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
    // The drivers' pullup share at each step, slewing toward each bit.
    let mut ku: Vec<Vec<f64>> = Vec::new();
    for k in 0..at.len() {
        let want = |b: bool| if b ^ (k == 1) { 1.0 } else { 0.0 };
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
        ku.push(list);
    }
    let mut record = Vec::new();
    for (k, &(a, _)) in at.iter().enumerate() {
        record.push(a);
        record.push(dies[k]);
    }
    let init = dc(&circuit, &at, &dies, setup.term, corner, &|d| ku[d][0]);
    let solved = circuit.run(dt, steps, corner, &|d, s| ku[d][s], &record, Some(&init));
    let probes: Vec<Probe> = if diff {
        let d = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a - b).collect::<Vec<f32>>();
        vec![
            Probe {
                name: "Driver pins, P − N".into(),
                v: d(&solved.v[0], &solved.v[2]),
            },
            Probe {
                name: "Receiver die, P − N".into(),
                v: d(&solved.v[1], &solved.v[3]),
            },
            Probe {
                name: "Receiver die, P".into(),
                v: solved.v[1].clone(),
            },
            Probe {
                name: "Receiver die, N".into(),
                v: solved.v[3].clone(),
            },
        ]
    } else {
        vec![
            Probe {
                name: "Driver pin".into(),
                v: solved.v[0].clone(),
            },
            Probe {
                name: "Receiver die".into(),
                v: solved.v[1].clone(),
            },
        ]
    };
    let threshold = if diff {
        0.0
    } else {
        let (lo, hi) = receiver.model.thresholds(corner);
        (lo + hi) / 2.0
    };
    let eye = eye(&probes[1].v, dt, start, ui, &bits, threshold);
    Ok(Run {
        dt,
        start: start as f64 * dt,
        probes,
        eye,
        delay,
        switch: (t_r, t_f),
        bits,
        notes,
    })
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

/// The steady state before the first edge: the lines are shorts, the
/// capacitors open and the inductors shorts, so each net is one voltage
/// between its driver's and receiver's dies. Solved by Newton's method,
/// then spread over the circuit with the current along each route.
fn dc(
    c: &Circuit<'_>,
    at: &[(usize, usize)],
    dies: &[usize],
    term: Term,
    corner: Corner,
    ku: &dyn Fn(usize) -> f64,
) -> Init {
    // Package resistance between each die and its net, by buffer.
    let r_of = |die: usize| -> f64 {
        c.series
            .iter()
            .filter(|s| s.3 && (s.0 == die || s.1 == die))
            // Not the termination across the pair's receiver dies.
            .filter(|s| !(dies.contains(&s.0) && dies.contains(&s.1)))
            .map(|s| s.2)
            .sum::<f64>()
            .max(1e-3)
    };
    let drivers: Vec<(usize, &Model, f64)> = c
        .buffers
        .iter()
        .filter_map(|&(n, m, d)| d.map(|k| (n, m, ku(k))))
        .collect();
    let rx: Vec<(usize, &Model)> = c
        .buffers
        .iter()
        .filter(|b| b.2.is_none())
        .map(|&(n, m, _)| (n, m))
        .collect();
    let k = at.len();
    // x = [net, driver die, receiver die] per net.
    let resid = |x: &[f64]| -> Vec<f64> {
        let mut r = vec![0.0; 3 * k];
        for j in 0..k {
            let (vn, vd, vr) = (x[3 * j], x[3 * j + 1], x[3 * j + 2]);
            let (dn, dm, u) = drivers[j];
            let (rn, rm) = rx[j];
            let (rd, rr) = (r_of(dn), r_of(rn));
            r[3 * j] = (vn - vd) / rd + (vn - vr) / rr;
            r[3 * j + 1] = dm.current(vd, u, 1.0 - u, corner) + (vd - vn) / rd;
            r[3 * j + 2] = rm.current(vr, 0.0, 0.0, corner) + (vr - vn) / rr;
            match term {
                Term::None => {}
                Term::ToRef { r: t, v } => r[3 * j + 2] += (vr - v) / t.max(1e-3),
                Term::Across(t) if k == 2 => {
                    r[3 * j + 2] += (vr - x[3 * (1 - j) + 2]) / t.max(1e-3)
                }
                Term::Across(_) => {}
            }
        }
        r
    };
    let mut x: Vec<f64> = (0..k)
        .flat_map(|j| {
            let (_, m, u) = drivers[j];
            let v = if u > 0.5 {
                m.vcc(corner)
            } else {
                m.vss(corner)
            };
            [v, v, v]
        })
        .collect();
    let m = 3 * k;
    for _ in 0..60 {
        let r0 = resid(&x);
        let mut jac = vec![0.0; m * m];
        for col in 0..m {
            let mut y = x.clone();
            y[col] += 1e-6;
            let r1 = resid(&y);
            for row in 0..m {
                jac[row * m + col] = (r1[row] - r0[row]) / 1e-6;
            }
        }
        let mut rhs = r0.clone();
        let dx = solve(&mut jac, &mut rhs, m);
        let mut worst: f64 = 0.0;
        for i in 0..m {
            let s = dx[i].clamp(-0.5, 0.5);
            x[i] -= s;
            worst = worst.max(s.abs());
        }
        if worst < 1e-9 {
            break;
        }
    }
    // Spread: every node of a net at its voltage, dies at theirs.
    let mut v = vec![0.0; c.n];
    let mut uf = Uf((0..c.n).collect());
    for (l, _) in &c.lines {
        uf.join(l.a, l.b);
    }
    for s in c.series.iter().filter(|s| !s.3) {
        uf.join(s.0, s.1);
    }
    for (j, &(a, _)) in at.iter().enumerate() {
        let root = uf.find(a);
        for (i, vi) in v.iter_mut().enumerate() {
            if uf.find(i) == root {
                *vi = x[3 * j];
            }
        }
        v[drivers[j].0] = x[3 * j + 1];
        v[dies[j]] = x[3 * j + 2];
    }
    // The current from driver to receiver along the route's lines.
    let mut line_i = vec![0.0; c.lines.len()];
    let mut ind_i = vec![0.0; c.series.len()];
    for (j, &(a, b)) in at.iter().enumerate() {
        let i_net = (x[3 * j + 1] - x[3 * j]) / r_of(drivers[j].0);
        // Breadth-first from the driver pad to the receiver pad.
        let mut from: Vec<Option<(usize, usize)>> = vec![None; c.n];
        let mut seen = vec![false; c.n];
        seen[a] = true;
        let mut queue = std::collections::VecDeque::from([a]);
        while let Some(n) = queue.pop_front() {
            if n == b {
                break;
            }
            for (li, (l, _)) in c.lines.iter().enumerate() {
                let other = if l.a == n {
                    l.b
                } else if l.b == n {
                    l.a
                } else {
                    continue;
                };
                if !seen[other] {
                    seen[other] = true;
                    from[other] = Some((li, n));
                    queue.push_back(other);
                }
            }
        }
        let mut n = b;
        while let Some((li, prev)) = from[n] {
            // Current flows prev → n.
            line_i[li] = if c.lines[li].0.a == prev {
                i_net
            } else {
                -i_net
            };
            n = prev;
        }
        for (si, s) in c.series.iter().enumerate() {
            if s.3 {
                continue;
            }
            // Driver side: mid → pad carries the current out; receiver
            // side: pad → mid carries it in (mid → pad is negative).
            let into_net = uf.find(s.1) == uf.find(a);
            let driver_side = c
                .series
                .iter()
                .any(|t| t.3 && t.1 == s.0 && t.0 == drivers[j].0);
            if into_net {
                ind_i[si] = if driver_side { i_net } else { -i_net };
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
        format!("Route delay {:.1} ps", run.delay * 1e12),
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
