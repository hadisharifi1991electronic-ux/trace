//! 3D frequency-domain finite-element solver (HFSS-style).
//!
//! The model is a box of space holding dielectric solids, perfect-conductor
//! solids and zero-thickness conductor sheets. It is cut into tetrahedra
//! (each cell of a graded grid that lands on every solid's faces split six
//! ways, Kuhn's split, so neighbours always share whole faces). The
//! electric field is first-order Whitney (Nedelec) edge elements: one
//! unknown per edge, the field's tangential part along it.
//!
//! Each wave port is a rectangle on a face of the box. Its mode comes from a
//! 2D solve on the port's own triangles: quasi-TEM ports (lines) solve
//! Laplace with the signal conductor at 1 V for the field, the capacitance
//! with and without the dielectrics for Er,eff and Z0; waveguide ports take
//! the TE10 mode. The port absorbs that mode and launches it with unit
//! amplitude; the other walls are perfect conductor or a first-order
//! radiation boundary.
//!
//! Adaptive meshing, as HFSS does it: solve at one frequency, estimate each
//! tetrahedron's error (its residual and the jumps of the field across its
//! faces), split the worst share of them (longest-edge bisection, Rivara,
//! which keeps the mesh conforming and the shapes from degrading), solve
//! again, and stop once no S-parameter moves by more than the target ΔS
//! between passes. The final mesh then sweeps the band.

#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::map_entry
)]

use std::collections::HashMap;

use faer::c64;
use faer::linalg::solvers::Solve;
use faer::sparse::{SparseColMat, Triplet};

const C0: f64 = 299_792_458.0;
const ETA0: f64 = 376.730_313_668;
/// Largest ratio between neighbouring starting-grid gaps (thin copper and
/// dielectrics would otherwise grade fine lines across the whole box; the
/// adaptive passes refine where it matters).
const GRADE: f64 = 8.0;
/// Above this many unknowns the iterative solver is used.
const ITER_MIN: usize = 150_000;

/// What a solid is made of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    Dielectric {
        er: f64,
        tand: f64,
    },
    /// Perfect conductor; `net` 0 is ground, others are signals.
    Pec {
        net: u32,
    },
}

/// A solid's shape (metres).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Box {
        lo: [f64; 3],
        hi: [f64; 3],
    },
    /// A cylinder along `axis`, centre `c` in the other two axes (in
    /// increasing axis order), from `lo` to `hi` along the axis.
    Cyl {
        axis: usize,
        c: [f64; 2],
        r: f64,
        lo: f64,
        hi: f64,
    },
}

/// A solid; later solids win where they overlap earlier ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub shape: Shape,
    pub fill: Fill,
}

/// A zero-thickness conductor in the plane `axis` = `at`: a polygon (in
/// the other two axes, increasing order) minus round holes.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub axis: usize,
    pub at: f64,
    pub poly: Vec<[f64; 2]>,
    /// Holes: capsules (both ends, radius); equal ends make a circle.
    pub holes: Vec<([f64; 2], [f64; 2], f64)>,
    /// A round sheet (centre, radius) in place of the polygon.
    pub disc: Option<([f64; 2], f64)>,
    pub net: u32,
}

/// What a face of the box is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wall {
    Pec,
    Radiation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Quasi-TEM line mode, driven on conductor `signal` against all others.
    Tem { signal: u32 },
    /// Odd (differential) quasi-TEM mode of a pair: `plus` at +½ V,
    /// `minus` at −½ V; Z0 is then the differential impedance.
    Diff { plus: u32, minus: u32 },
    /// Rectangular waveguide TE10 over the whole port rectangle.
    Te10,
}

/// A wave port: a rectangle on the box face `axis` (`high` side or low).
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    pub axis: usize,
    pub high: bool,
    pub lo: [f64; 2],
    pub hi: [f64; 2],
    pub mode: Mode,
    /// Move the reference plane this far into the model (m).
    pub deembed: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub lo: [f64; 3],
    pub hi: [f64; 3],
    /// Walls by face: x-, x+, y-, y+, z-, z+.
    pub walls: [Wall; 6],
    pub er: f64,
    pub solids: Vec<Solid>,
    pub sheets: Vec<Sheet>,
    pub ports: Vec<Port>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// Adaptive solution frequency, GHz.
    pub f_adapt: f64,
    /// Sweep frequencies, GHz.
    pub sweep: Vec<f64>,
    pub max_passes: usize,
    pub min_passes: usize,
    pub delta_s: f64,
    /// Share of tetrahedra refined each pass.
    pub refine: f64,
    /// Starting mesh: cells per wavelength (in the densest dielectric).
    pub per_lambda: f64,
    /// Stop refining past this many unknowns.
    pub max_unknowns: usize,
    /// Renormalise to this impedance (None: each port's own Z0).
    pub z_ref: Option<f64>,
    /// Element order: 1 (Whitney) or 2 (HFSS's default).
    pub order: usize,
    /// Interpolating sweep: solve only enough points that the next one
    /// lands within this of the interpolated S (0: solve every point).
    pub interp_tol: f64,
    /// At most this many solved points in an interpolating sweep.
    pub interp_max: usize,
    /// Linear solver: Some(true) the half-memory iterative one, Some(false)
    /// the direct one, None by size (iterative above 150k unknowns).
    pub iterative: Option<bool>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            f_adapt: 10.0,
            sweep: Vec::new(),
            max_passes: 10,
            min_passes: 2,
            delta_s: 0.02,
            refine: 0.3,
            per_lambda: 6.0,
            max_unknowns: 650_000,
            z_ref: Some(50.0),
            order: 2,
            interp_tol: 0.005,
            interp_max: 20,
            iterative: None,
        }
    }
}

/// One adaptive pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub tets: usize,
    pub unknowns: usize,
    /// Largest change of any S-parameter from the pass before (None: first).
    pub delta_s: Option<f64>,
    pub s: Vec<c64>,
    pub seconds: f64,
}

/// A port's mode at one frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortInfo {
    /// Characteristic (TEM) or wave (TE10) impedance, Ω.
    pub z0: f64,
    pub eeff: f64,
    /// Propagation constant, rad/m.
    pub beta: f64,
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub ports: Vec<String>,
    pub passes: Vec<Pass>,
    pub converged: bool,
    /// GHz.
    pub f: Vec<f64>,
    /// Per frequency, S row by row.
    pub s: Vec<Vec<c64>>,
    /// The frequencies actually solved (the rest are interpolated).
    pub solved: Vec<f64>,
    pub info: Vec<Vec<PortInfo>>,
    pub z_ref: Option<f64>,
    pub tets: usize,
    pub unknowns: usize,
}

impl Solution {
    pub fn n(&self) -> usize {
        self.ports.len()
    }
    /// Touchstone (RI) text.
    pub fn touchstone(&self) -> String {
        use std::fmt::Write;
        let n = self.n();
        let z = self
            .z_ref
            .unwrap_or_else(|| self.info.first().map(|i| i[0].z0).unwrap_or(50.0));
        let mut o = String::new();
        let _ = writeln!(
            o,
            "! Trace 3D FEM, {} tetrahedra, {} unknowns",
            self.tets, self.unknowns
        );
        for (i, p) in self.ports.iter().enumerate() {
            let zi = self.info.first().map(|v| v[i].z0).unwrap_or(0.0);
            let _ = writeln!(o, "! port {} {} Z0 {:.2} ohm", i + 1, p, zi);
        }
        let _ = writeln!(o, "# GHz S RI R {}", z);
        for (k, f) in self.f.iter().enumerate() {
            let _ = write!(o, "{}", f);
            // Touchstone 2-port order is S11 S21 S12 S22 (column first).
            let order: Vec<(usize, usize)> = if n == 2 {
                vec![(0, 0), (1, 0), (0, 1), (1, 1)]
            } else {
                (0..n).flat_map(|r| (0..n).map(move |c| (r, c))).collect()
            };
            for (r, c) in order {
                let s = self.s[k][r * n + c];
                let _ = write!(o, " {:.6e} {:.6e}", s.re, s.im);
            }
            o.push('\n');
        }
        o
    }
}

// ---------------------------------------------------------------- mesh

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub nodes: Vec<[f64; 3]>,
    pub tets: Vec<[u32; 4]>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Grid lines along one axis: every break, with gaps cut to at most `h`
/// and neighbours growing by at most 1.5x.
fn axis_lines(mut br: Vec<f64>, h: f64, tol: f64) -> Vec<f64> {
    br.sort_by(|a, b| a.partial_cmp(b).unwrap());
    br.dedup_by(|a, b| (*a - *b).abs() < tol);
    let mut out = vec![br[0]];
    for w in br.windows(2) {
        let n = ((w[1] - w[0]) / h).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0] + (w[1] - w[0]) * k as f64 / n as f64);
        }
    }
    // Grade: split any gap more than 3x a neighbour.
    loop {
        let mut changed = false;
        let mut next = vec![out[0]];
        for i in 0..out.len() - 1 {
            let g = out[i + 1] - out[i];
            let lft = if i > 0 { out[i] - out[i - 1] } else { g };
            let rgt = if i + 2 < out.len() {
                out[i + 2] - out[i + 1]
            } else {
                g
            };
            if g > GRADE * lft.min(rgt) * 1.0001 {
                next.push(out[i] + g / 2.0);
                changed = true;
            }
            next.push(out[i + 1]);
        }
        out = next;
        if !changed {
            break;
        }
    }
    out
}

/// The hard lines, plus the soft ones further than `d` from any line
/// kept so far (hard lines win).
fn merge_lines(hard: &[f64], soft: &[f64], d: f64) -> Vec<f64> {
    let mut all: Vec<(f64, bool)> = hard
        .iter()
        .map(|&x| (x, true))
        .chain(soft.iter().map(|&x| (x, false)))
        .collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut out: Vec<(f64, bool)> = Vec::new();
    for (x, h) in all {
        if h {
            while out.last().is_some_and(|&(y, hy)| !hy && x - y < d) {
                out.pop();
            }
            if out.last().is_some_and(|&(y, _)| x - y < d * 1e-6) {
                out.pop();
            }
            out.push((x, true));
        } else if out.last().is_none_or(|&(y, _)| x - y >= d) {
            out.push((x, false));
        }
    }
    out.into_iter().map(|p| p.0).collect()
}

impl Model {
    fn tol(&self) -> f64 {
        let d = sub(self.hi, self.lo);
        1e-9 * d[0].max(d[1]).max(d[2])
    }

    fn er_max(&self) -> f64 {
        let mut e = self.er;
        for s in &self.solids {
            if let Fill::Dielectric { er, .. } = s.fill {
                e = e.max(er);
            }
        }
        e
    }

    /// The starting mesh: a graded grid through every edge of the model,
    /// cut into tetrahedra. When that would be more than `max_tets`, the
    /// grid lines of small details (pad and hole edges, copper thickness)
    /// are merged until it fits; the box, the layers and the sheets keep
    /// theirs, and adaptive refinement then finds the details again.
    pub fn mesh(&self, f_ghz: f64, per_lambda: f64, per_axis: f64, max_tets: usize) -> Mesh {
        let lambda = C0 / (f_ghz * 1e9) / self.er_max().sqrt();
        let h = lambda / per_lambda;
        let tol = self.tol();
        let mut br: [Vec<f64>; 3] = Default::default();
        // Lines that must stay: the box, sheets and dielectric layers.
        let mut hard: [Vec<f64>; 3] = Default::default();
        for a in 0..3 {
            hard[a].push(self.lo[a]);
            hard[a].push(self.hi[a]);
        }
        for s in &self.solids {
            if let (Fill::Dielectric { .. }, Shape::Box { lo, hi }) = (&s.fill, &s.shape) {
                for a in 0..3 {
                    hard[a].push(lo[a].clamp(self.lo[a], self.hi[a]));
                    hard[a].push(hi[a].clamp(self.lo[a], self.hi[a]));
                }
            }
        }
        for s in &self.sheets {
            hard[s.axis].push(s.at);
        }
        let clamp = |a: usize, v: f64| v.clamp(self.lo[a], self.hi[a]);
        for s in &self.solids {
            match &s.shape {
                Shape::Box { lo, hi } => {
                    for a in 0..3 {
                        br[a].push(clamp(a, lo[a]));
                        br[a].push(clamp(a, hi[a]));
                    }
                }
                Shape::Cyl { axis, c, r, lo, hi } => {
                    br[*axis].push(clamp(*axis, *lo));
                    br[*axis].push(clamp(*axis, *hi));
                    let (u, v) = others(*axis);
                    for (k, a) in [u, v].into_iter().enumerate() {
                        br[a].push(clamp(a, c[k] - r));
                        br[a].push(clamp(a, c[k] + r));
                        // The axis stays, so a barrel is never lost.
                        hard[a].push(clamp(a, c[k]));
                    }
                }
            }
        }
        for s in &self.sheets {
            br[s.axis].push(s.at);
            let (u, v) = others(s.axis);
            // A signal sheet (a trace) keeps its outline: the wave ports
            // take their mode from the mesh's own triangles, which must
            // see the trace's width.
            let keep = s.net != 0;
            let to = if keep { &mut hard } else { &mut br };
            for p in &s.poly {
                to[u].push(clamp(u, p[0]));
                to[v].push(clamp(v, p[1]));
            }
            let caps: Vec<([f64; 2], f64)> = s
                .holes
                .iter()
                .flat_map(|(a, b, r)| [(*a, *r), (*b, *r)])
                .chain(s.disc.iter().copied())
                .collect();
            for (c, r) in &caps {
                hard[u].push(clamp(u, c[0]));
                hard[v].push(clamp(v, c[1]));
                br[u].push(clamp(u, c[0] - r));
                br[u].push(clamp(u, c[0] + r));
                br[v].push(clamp(v, c[1] - r));
                br[v].push(clamp(v, c[1] + r));
            }
        }
        for p in &self.ports {
            let (u, v) = others(p.axis);
            br[u].push(p.lo[0]);
            br[u].push(p.hi[0]);
            br[v].push(p.lo[1]);
            br[v].push(p.hi[1]);
        }
        // Also at least `per_axis` cells across every axis, so small cross
        // sections start with some resolution.
        let ext = (0..3)
            .map(|a| self.hi[a] - self.lo[a])
            .fold(0.0f64, f64::max);
        let r_min = self
            .solids
            .iter()
            .filter_map(|s| match (&s.fill, &s.shape) {
                (Fill::Pec { .. }, Shape::Cyl { r, .. }) => Some(*r),
                _ => None,
            })
            .chain(self.sheets.iter().flat_map(|s| {
                s.holes
                    .iter()
                    .map(|h| h.2)
                    .chain(s.disc.iter().map(|d| d.1))
                    .collect::<Vec<_>>()
            }))
            .fold(ext, f64::min);
        let mut d = 0.0f64;
        loop {
            let g: Vec<Vec<f64>> = (0..3)
                .map(|a| {
                    axis_lines(
                        merge_lines(&hard[a], &br[a], d.max(tol)),
                        h.min((self.hi[a] - self.lo[a]) / per_axis),
                        tol,
                    )
                })
                .collect();
            let tets = 6 * (g[0].len() - 1) * (g[1].len() - 1) * (g[2].len() - 1);
            if std::env::var_os("TRACE_FEM_MESH").is_some() {
                eprintln!(
                    "  start mesh: merge {:.1} um, lines {} x {} x {}, {tets} tetrahedra",
                    d * 1e6,
                    g[0].len(),
                    g[1].len(),
                    g[2].len()
                );
            }
            // Never merge past about a barrel's radius: coarser than that,
            // a via can lose its own hole in the planes.
            if tets <= max_tets || d > ext || d > 1.5 * r_min {
                return kuhn(&g[0], &g[1], &g[2]);
            }
            d = (d * 1.4).max(ext * 1e-3);
        }
    }

    /// Material at a point: (complex Er, PEC net).
    fn at(&self, p: [f64; 3]) -> (c64, Option<u32>) {
        let mut eps = c64::new(self.er, 0.0);
        let mut pec = None;
        for s in &self.solids {
            if inside(&s.shape, p, 0.0) {
                match s.fill {
                    Fill::Dielectric { er, tand } => {
                        eps = c64::new(er, -er * tand);
                        pec = None;
                    }
                    Fill::Pec { net } => pec = Some(net),
                }
            }
        }
        (eps, pec)
    }

    /// The conductor a point lies on (for port modes): sheets, solids
    /// (touching counts), and PEC walls (ground).
    fn conductor_at(&self, p: [f64; 3], tol: f64, port_face: usize) -> Option<u32> {
        for s in &self.sheets {
            if on_sheet(s, p, tol) {
                return Some(s.net);
            }
        }
        let mut net = None;
        for s in &self.solids {
            if inside(&s.shape, p, tol) {
                match s.fill {
                    Fill::Pec { net: n } => net = Some(n),
                    Fill::Dielectric { .. } => {}
                }
            }
        }
        if net.is_some() {
            return net;
        }
        for a in 0..3 {
            for (side, b) in [(0, self.lo[a]), (1, self.hi[a])] {
                if 2 * a + side != port_face
                    && (p[a] - b).abs() < tol
                    && self.walls[2 * a + side] == Wall::Pec
                {
                    return Some(0);
                }
            }
        }
        None
    }
}

fn others(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn inside(s: &Shape, p: [f64; 3], tol: f64) -> bool {
    match s {
        Shape::Box { lo, hi } => (0..3).all(|a| p[a] >= lo[a] - tol && p[a] <= hi[a] + tol),
        Shape::Cyl { axis, c, r, lo, hi } => {
            let (u, v) = others(*axis);
            let d = (p[u] - c[0]).hypot(p[v] - c[1]);
            p[*axis] >= lo - tol && p[*axis] <= hi + tol && d <= r + tol
        }
    }
}

fn point_in_poly(v: &[[f64; 2]], p: [f64; 2], tol: f64) -> bool {
    if crate::fdtd::point_in(v, p) {
        return true;
    }
    (0..v.len()).any(|i| crate::fdtd::seg_dist(p, v[i], v[(i + 1) % v.len()]) <= tol)
}

fn on_sheet(s: &Sheet, p: [f64; 3], tol: f64) -> bool {
    if (p[s.axis] - s.at).abs() > tol {
        return false;
    }
    let (u, v) = others(s.axis);
    let q = [p[u], p[v]];
    if let Some((c, r)) = s.disc {
        return (q[0] - c[0]).hypot(q[1] - c[1]) <= r + tol;
    }
    point_in_poly(&s.poly, q, tol)
        && s.holes
            .iter()
            .all(|(a, b, r)| crate::fdtd::seg_dist(q, *a, *b) >= r - tol)
}

/// Kuhn's split of a graded grid into tetrahedra.
fn kuhn(x: &[f64], y: &[f64], z: &[f64]) -> Mesh {
    let (nx, ny, nz) = (x.len(), y.len(), z.len());
    let id = |i: usize, j: usize, k: usize| (i + nx * (j + ny * k)) as u32;
    let mut nodes = Vec::with_capacity(nx * ny * nz);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                nodes.push([x[i], y[j], z[k]]);
            }
        }
    }
    let mut tets = Vec::new();
    let perms = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for k in 0..nz - 1 {
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                for p in perms {
                    let mut c = [i, j, k];
                    let mut t = [id(c[0], c[1], c[2]); 4];
                    for (s, &a) in p.iter().enumerate() {
                        c[a] += 1;
                        t[s + 1] = id(c[0], c[1], c[2]);
                    }
                    tets.push(t);
                }
            }
        }
    }
    Mesh { nodes, tets }
}

fn ekey(a: u32, b: u32) -> (u32, u32) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

const LOCAL_EDGES: [(usize, usize); 6] = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];

impl Mesh {
    fn len2(&self, e: (u32, u32)) -> f64 {
        let d = sub(self.nodes[e.1 as usize], self.nodes[e.0 as usize]);
        dot(d, d)
    }

    fn longest(&self, t: &[u32; 4]) -> (u32, u32) {
        let mut best = ekey(t[0], t[1]);
        let mut bl = self.len2(best);
        for (a, b) in LOCAL_EDGES {
            let e = ekey(t[a], t[b]);
            let l = self.len2(e);
            if l > bl || (l == bl && e < best) {
                best = e;
                bl = l;
            }
        }
        best
    }

    /// Longest-edge bisection of the marked tetrahedra, then of every
    /// tetrahedron left with a split edge, until the mesh conforms.
    pub fn refine(&mut self, marked: &[bool], edges: &[(u32, u32)]) {
        let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
        for &e in edges {
            let e = ekey(e.0, e.1);
            if !mids.contains_key(&e) {
                let p = scale(add(self.nodes[e.0 as usize], self.nodes[e.1 as usize]), 0.5);
                self.nodes.push(p);
                mids.insert(e, (self.nodes.len() - 1) as u32);
            }
        }
        let mid = |m: &mut Mesh, e: (u32, u32), mids: &mut HashMap<(u32, u32), u32>| -> u32 {
            *mids.entry(e).or_insert_with(|| {
                let p = scale(add(m.nodes[e.0 as usize], m.nodes[e.1 as usize]), 0.5);
                m.nodes.push(p);
                (m.nodes.len() - 1) as u32
            })
        };
        let tets = std::mem::take(&mut self.tets);
        let mut cur = Vec::with_capacity(tets.len() * 2);
        for (i, t) in tets.into_iter().enumerate() {
            if marked.get(i).copied().unwrap_or(false) {
                let e = self.longest(&t);
                let m = mid(self, e, &mut mids);
                let (a, b) = split(t, e, m);
                cur.push(a);
                cur.push(b);
            } else {
                cur.push(t);
            }
        }
        loop {
            let mut changed = false;
            let mut next = Vec::with_capacity(cur.len() + cur.len() / 8);
            for t in cur {
                let hanging = LOCAL_EDGES
                    .iter()
                    .any(|&(a, b)| mids.contains_key(&ekey(t[a], t[b])));
                if hanging {
                    let e = self.longest(&t);
                    let m = mid(self, e, &mut mids);
                    let (a, b) = split(t, e, m);
                    next.push(a);
                    next.push(b);
                    changed = true;
                } else {
                    next.push(t);
                }
            }
            cur = next;
            if !changed {
                break;
            }
        }
        self.tets = cur;
    }
}

fn split(t: [u32; 4], e: (u32, u32), m: u32) -> ([u32; 4], [u32; 4]) {
    let mut a = t;
    let mut b = t;
    for k in 0..4 {
        if t[k] == e.1 {
            a[k] = m;
        }
        if t[k] == e.0 {
            b[k] = m;
        }
    }
    (a, b)
}

// ---------------------------------------------------------------- FEM

struct Geo {
    /// Gradients of the barycentric coordinates.
    g: [[f64; 3]; 4],
    vol: f64,
}

fn tet_geo(p: [[f64; 3]; 4]) -> Geo {
    let a = sub(p[1], p[0]);
    let b = sub(p[2], p[0]);
    let c = sub(p[3], p[0]);
    let det = dot(a, cross(b, c));
    // Rows of the inverse of [a b c] (columns).
    let r1 = scale(cross(b, c), 1.0 / det);
    let r2 = scale(cross(c, a), 1.0 / det);
    let r3 = scale(cross(a, b), 1.0 / det);
    let r0 = scale(add(add(r1, r2), r3), -1.0);
    Geo {
        g: [r0, r1, r2, r3],
        vol: det.abs() / 6.0,
    }
}

/// Whitney function of local edge (i, j) at barycentric point `l`.
fn whitney(g: &[[f64; 3]], l: &[f64], i: usize, j: usize) -> [f64; 3] {
    sub(scale(g[j], l[i]), scale(g[i], l[j]))
}

/// Local faces of a sorted tetrahedron (node positions, increasing).
const LOCAL_FACES: [(usize, usize, usize); 4] = [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)];

/// What a basis function hangs on: kind (0 Whitney, 1 edge gradient,
/// 2 and 3 the two face functions) and its global nodes.
type Key = (u8, [u32; 3]);

/// The basis functions' keys for a sorted tetrahedron.
fn keys(s: &[u32; 4], order: usize) -> Vec<Key> {
    let mut k = Vec::with_capacity(20);
    for (a, b) in LOCAL_EDGES {
        k.push((0, [s[a], s[b], u32::MAX]));
    }
    if order >= 2 {
        for (a, b) in LOCAL_EDGES {
            k.push((1, [s[a], s[b], u32::MAX]));
        }
        for (a, b, c) in LOCAL_FACES {
            k.push((2, [s[a], s[b], s[c]]));
            k.push((3, [s[a], s[b], s[c]]));
        }
    }
    k
}

/// Basis values and curls at barycentric point `l` (same order as `keys`).
/// Order 2 is the hierarchical mixed-order second-order set (Webb):
/// Whitney, the gradients of λiλj, and on each face (i, j, k) λk·Nij and
/// λi·Njk; 20 functions, full second order in the curl-conforming sense.
fn basis(g: &[[f64; 3]; 4], l: &[f64; 4], order: usize, out: &mut Vec<([f64; 3], [f64; 3])>) {
    out.clear();
    for (i, j) in LOCAL_EDGES {
        out.push((whitney(g, l, i, j), scale(cross(g[i], g[j]), 2.0)));
    }
    if order >= 2 {
        for (i, j) in LOCAL_EDGES {
            out.push((add(scale(g[j], l[i]), scale(g[i], l[j])), [0.0; 3]));
        }
        let face_fn = |k: usize, i: usize, j: usize| -> ([f64; 3], [f64; 3]) {
            let n = whitney(g, l, i, j);
            let c = add(cross(g[k], n), scale(cross(g[i], g[j]), 2.0 * l[k]));
            (scale(n, l[k]), c)
        };
        for (a, b, c) in LOCAL_FACES {
            out.push(face_fn(c, a, b));
            out.push(face_fn(a, b, c));
        }
    }
}

/// True when the function with key `k` has a tangential part on the face
/// with (sorted) nodes `f`.
fn on_face(k: &Key, f: &[u32; 3]) -> bool {
    match k.0 {
        0 | 1 => f.contains(&k.1[0]) && f.contains(&k.1[1]),
        _ => k.1 == *f,
    }
}

const KA: f64 = 0.585_410_196_624_968_5;
const KB: f64 = 0.138_196_601_125_010_5;
/// Degree-2 tetrahedron rule: (barycentrics, weight share).
const Q_TET2: [([f64; 4], f64); 4] = [
    ([KA, KB, KB, KB], 0.25),
    ([KB, KA, KB, KB], 0.25),
    ([KB, KB, KA, KB], 0.25),
    ([KB, KB, KB, KA], 0.25),
];

/// Degree-4 tetrahedron rule (Keast, 11 points).
fn q_tet4() -> Vec<([f64; 4], f64)> {
    let mut q = vec![([0.25; 4], -0.013_155_555_555_555_6 * 6.0)];
    let (a, b) = (0.071_428_571_428_571_4, 0.785_714_285_714_286);
    for k in 0..4 {
        let mut l = [a; 4];
        l[k] = b;
        q.push((l, 0.007_622_222_222_222_22 * 6.0));
    }
    let (c, d) = (0.399_403_576_166_799, 0.100_596_423_833_201);
    for (i, j) in LOCAL_EDGES {
        let mut l = [d; 4];
        l[i] = c;
        l[j] = c;
        q.push((l, 0.024_888_888_888_888_9 * 6.0));
    }
    q
}

/// Degree-4 triangle rule (Dunavant, 6 points).
const Q_TRI4: [([f64; 3], f64); 6] = [
    (
        [
            0.108_103_018_168_070,
            0.445_948_490_915_965,
            0.445_948_490_915_965,
        ],
        0.223_381_589_678_011,
    ),
    (
        [
            0.445_948_490_915_965,
            0.108_103_018_168_070,
            0.445_948_490_915_965,
        ],
        0.223_381_589_678_011,
    ),
    (
        [
            0.445_948_490_915_965,
            0.445_948_490_915_965,
            0.108_103_018_168_070,
        ],
        0.223_381_589_678_011,
    ),
    (
        [
            0.816_847_572_980_459,
            0.091_576_213_509_771,
            0.091_576_213_509_771,
        ],
        0.109_951_743_655_322,
    ),
    (
        [
            0.091_576_213_509_771,
            0.816_847_572_980_459,
            0.091_576_213_509_771,
        ],
        0.109_951_743_655_322,
    ),
    (
        [
            0.091_576_213_509_771,
            0.091_576_213_509_771,
            0.816_847_572_980_459,
        ],
        0.109_951_743_655_322,
    ),
];

/// A boundary face: its sorted nodes, the tetrahedron behind it and which
/// of the tetrahedron's (sorted) nodes they are.
#[derive(Clone, Copy)]
struct BFace {
    f: [u32; 3],
    tet: usize,
    local: [usize; 3],
}

struct Assembled {
    /// Each tet's node indices, sorted (so local edges point up in index).
    tets: Vec<[u32; 4]>,
    eps: Vec<c64>,
    /// Global unknown per local basis function, None where it is PEC.
    dof: Vec<Vec<Option<u32>>>,
    n: usize,
    order: usize,
    ports: Vec<PortMesh>,
    /// Radiation faces and their wavenumber.
    rad: Vec<(BFace, f64)>,
}

struct PortMesh {
    tris: Vec<BFace>,
    /// The mode's tangential field at each triangle's quadrature points,
    /// and the shape of n x H (for a quasi-TEM line: the field without
    /// the dielectrics; for a waveguide the same as e).
    e: Vec<[[f64; 3]; 6]>,
    e0: Vec<[[f64; 3]; 6]>,
    /// ∫ e·e0.
    ee0: f64,
    info: PortInfo,
    /// sqrt(2 x power) for unit mode amplitude.
    norm: f64,
    /// Relative gap between the port Z0 on the mesh's own triangles and
    /// on the fine 2D solve (0 for a waveguide).
    gap: f64,
    /// ∫ e·e.
    ee: f64,
    /// The 2D solve's error at each triangle.
    err: Vec<f64>,
}

/// Triangle: unit normal (from the vertex order), area and surface
/// gradients of the barycentrics.
fn tri_geo(p: [[f64; 3]; 3]) -> ([f64; 3], f64, [[f64; 3]; 3]) {
    let nn = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let a2 = dot(nn, nn).sqrt();
    let n = scale(nn, 1.0 / a2);
    let mut g = [[0.0; 3]; 3];
    for i in 0..3 {
        g[i] = scale(cross(n, sub(p[(i + 2) % 3], p[(i + 1) % 3])), 1.0 / a2);
    }
    (n, a2 / 2.0, g)
}

const TRI_EDGES: [(usize, usize); 3] = [(0, 1), (0, 2), (1, 2)];

fn face_nodes(mesh: &Mesh, f: &[u32; 3]) -> [[f64; 3]; 3] {
    [
        mesh.nodes[f[0] as usize],
        mesh.nodes[f[1] as usize],
        mesh.nodes[f[2] as usize],
    ]
}

fn tet_nodes(mesh: &Mesh, s: &[u32; 4]) -> [[f64; 3]; 4] {
    [
        mesh.nodes[s[0] as usize],
        mesh.nodes[s[1] as usize],
        mesh.nodes[s[2] as usize],
        mesh.nodes[s[3] as usize],
    ]
}

fn assemble_mesh(
    model: &Model,
    mesh: &Mesh,
    f_ghz: f64,
    order: usize,
) -> Result<Assembled, String> {
    let tol = model.tol();
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let mut tets = Vec::with_capacity(mesh.tets.len());
    let mut eps = Vec::with_capacity(mesh.tets.len());
    let mut is_pec_tet = Vec::with_capacity(mesh.tets.len());
    for t in &mesh.tets {
        let mut s = *t;
        s.sort_unstable();
        let p = tet_nodes(mesh, &s);
        let c = scale(add(add(p[0], p[1]), add(p[2], p[3])), 0.25);
        let (e, pec) = model.at(c);
        tets.push(s);
        eps.push(e);
        is_pec_tet.push(pec);
    }
    // The PEC solid (net) each node lies in or on, if any.
    let node_pec: Vec<Option<u32>> = mesh
        .nodes
        .iter()
        .map(|&p| {
            let mut net = None;
            for s in &model.solids {
                if let Fill::Pec { net: n } = s.fill {
                    if inside(&s.shape, p, tol) {
                        net = Some(n);
                    }
                }
            }
            net
        })
        .collect();
    let in_port = |a: usize, side: usize, m: [f64; 3]| {
        model.ports.iter().any(|pt| {
            pt.axis == a && pt.high == (side == 1) && {
                let (u, w) = others(a);
                m[u] > pt.lo[0] + tol
                    && m[u] < pt.hi[0] - tol
                    && m[w] > pt.lo[1] + tol
                    && m[w] < pt.hi[1] - tol
            }
        })
    };
    // Is a set of points (an edge or a face) on a conductor surface: a
    // sheet or a PEC wall (outside the ports)? `m` is its centre.
    // Every point and the centre must be on it, so an edge from a via
    // inside a plane's hole to the plane beyond never shorts the two.
    let on_conductor = |pts: &[[f64; 3]], m: [f64; 3]| -> bool {
        for s in &model.sheets {
            if pts.iter().all(|p| (p[s.axis] - s.at).abs() < tol)
                && on_sheet(s, m, tol)
                && pts.iter().all(|&p| on_sheet(s, p, tol))
            {
                return true;
            }
        }
        for a in 0..3 {
            for (side, b) in [(0, model.lo[a]), (1, model.hi[a])] {
                if pts.iter().all(|p| (p[a] - b).abs() < tol)
                    && model.walls[2 * a + side] == Wall::Pec
                    && !in_port(a, side, m)
                {
                    return true;
                }
            }
        }
        false
    };
    let mut pec: HashMap<Key, bool> = HashMap::new();
    for (ti, s) in tets.iter().enumerate() {
        for k in keys(s, order) {
            let e = pec.entry(k).or_insert(false);
            // Inside a PEC solid: the keys whose nodes are all in it.
            if let Some(n) = is_pec_tet[ti] {
                if k.1
                    .iter()
                    .filter(|&&v| v != u32::MAX)
                    .all(|&v| node_pec[v as usize] == Some(n))
                {
                    *e = true;
                }
            }
        }
    }
    // Edge and face geometry tests, shared by the keys on the same nodes.
    let mut geo_pec: HashMap<[u32; 3], bool> = HashMap::new();
    for (k, v) in pec.iter_mut() {
        if *v {
            continue;
        }
        let nodes = k.1;
        *v = *geo_pec.entry(nodes).or_insert_with(|| {
            let pts: Vec<[f64; 3]> = nodes
                .iter()
                .filter(|&&n| n != u32::MAX)
                .map(|&n| mesh.nodes[n as usize])
                .collect();
            let m = scale(
                pts.iter().fold([0.0; 3], |a, p| add(a, *p)),
                1.0 / pts.len() as f64,
            );
            on_conductor(&pts, m)
        });
    }
    let mut key_dof: HashMap<Key, u32> = HashMap::new();
    let mut dof = Vec::with_capacity(tets.len());
    for s in &tets {
        let d: Vec<Option<u32>> = keys(s, order)
            .into_iter()
            .map(|k| {
                if pec[&k] {
                    None
                } else {
                    let n = key_dof.len() as u32;
                    Some(*key_dof.entry(k).or_insert(n))
                }
            })
            .collect();
        dof.push(d);
    }
    let n = key_dof.len();
    // Boundary faces.
    let mut faces: HashMap<[u32; 3], (BFace, u8)> = HashMap::new();
    for (ti, s) in tets.iter().enumerate() {
        for (a, b, c) in LOCAL_FACES {
            let bf = BFace {
                f: [s[a], s[b], s[c]],
                tet: ti,
                local: [a, b, c],
            };
            faces.entry(bf.f).or_insert((bf, 0)).1 += 1;
        }
    }
    let mut port_tris: Vec<Vec<BFace>> = vec![Vec::new(); model.ports.len()];
    let mut rad = Vec::new();
    let mut bfaces: Vec<&(BFace, u8)> = faces.values().filter(|v| v.1 == 1).collect();
    bfaces.sort_by_key(|v| v.0.f);
    for (bf, _) in bfaces {
        let p = face_nodes(mesh, &bf.f);
        let c = scale(add(add(p[0], p[1]), p[2]), 1.0 / 3.0);
        'axes: for a in 0..3 {
            for (side, b) in [(0, model.lo[a]), (1, model.hi[a])] {
                if !p.iter().all(|q| (q[a] - b).abs() < tol) {
                    continue;
                }
                let (u, w) = others(a);
                let port = model.ports.iter().position(|pt| {
                    pt.axis == a
                        && pt.high == (side == 1)
                        && c[u] > pt.lo[0]
                        && c[u] < pt.hi[0]
                        && c[w] > pt.lo[1]
                        && c[w] < pt.hi[1]
                });
                if let Some(pi) = port {
                    port_tris[pi].push(*bf);
                } else if model.walls[2 * a + side] == Wall::Radiation
                    && is_pec_tet[bf.tet].is_none()
                {
                    rad.push((*bf, k0 * eps[bf.tet].re.sqrt()));
                }
                break 'axes;
            }
        }
    }
    let mut ports = Vec::new();
    for (pi, pt) in model.ports.iter().enumerate() {
        if port_tris[pi].is_empty() {
            return Err(format!("port {} has no mesh faces", pt.name));
        }
        ports.push(port_mode(
            model,
            mesh,
            pt,
            &port_tris[pi],
            &eps,
            k0,
            tol,
            order,
        )?);
    }
    Ok(Assembled {
        tets,
        eps,
        dof,
        n,
        order,
        ports,
        rad,
    })
}

/// The 2D potential problem on a port: Lagrange elements of order `p`
/// (1 or 2) on the port triangles each cut `sub_n` x `sub_n`.
struct Port2d {
    c: f64,
    c0: f64,
    /// −∇φ at each parent triangle's quadrature points (sub_n = 1 only),
    /// with the dielectrics and without (the magnetic field's shape).
    e: Vec<[[f64; 3]; 6]>,
    e0: Vec<[[f64; 3]; 6]>,
}

fn port_2d(
    model: &Model,
    mesh: &Mesh,
    pt: &Port,
    tris: &[BFace],
    tri_eps: &[f64],
    signal: (u32, Option<u32>),
    tol: f64,
    sub_n: usize,
    p_order: usize,
) -> Result<Port2d, String> {
    let _ = others(pt.axis);
    let q = 1.0 / (tol * 10.0);
    let mut fidx: HashMap<[i64; 3], usize> = HashMap::new();
    let mut fnodes: Vec<[f64; 3]> = Vec::new();
    let mut node_at = |x: [f64; 3]| -> usize {
        let key = [
            (x[0] * q).round() as i64,
            (x[1] * q).round() as i64,
            (x[2] * q).round() as i64,
        ];
        *fidx.entry(key).or_insert_with(|| {
            fnodes.push(x);
            fnodes.len() - 1
        })
    };
    // Sub-triangles: (corner points, parent).
    let mut ftris: Vec<([[f64; 3]; 3], usize)> = Vec::new();
    for (ti, bf) in tris.iter().enumerate() {
        let p = face_nodes(mesh, &bf.f);
        let pt_at = |i: usize, j: usize| -> [f64; 3] {
            let (a, b) = (i as f64 / sub_n as f64, j as f64 / sub_n as f64);
            add(
                add(scale(p[0], 1.0 - a - b), scale(p[1], a)),
                scale(p[2], b),
            )
        };
        for i in 0..sub_n {
            for j in 0..sub_n - i {
                ftris.push(([pt_at(i, j), pt_at(i + 1, j), pt_at(i, j + 1)], ti));
                if i + j + 1 < sub_n {
                    ftris.push(([pt_at(i + 1, j), pt_at(i + 1, j + 1), pt_at(i, j + 1)], ti));
                }
            }
        }
    }
    // Element nodes: corners, then (P2) side midpoints in TRI_EDGES order.
    let elems: Vec<Vec<usize>> = ftris
        .iter()
        .map(|(c, _)| {
            let mut v: Vec<usize> = c.iter().map(|x| node_at(*x)).collect();
            if p_order == 2 {
                for (a, b) in TRI_EDGES {
                    v.push(node_at(scale(add(c[a], c[b]), 0.5)));
                }
            }
            v
        })
        .collect();
    let nn = fnodes.len();
    let mut fixed: Vec<Option<f64>> = vec![None; nn];
    let vp = if signal.1.is_some() { 0.5 } else { 1.0 };
    for (i, p) in fnodes.iter().enumerate() {
        let p = *p;
        match model.conductor_at(p, tol, 2 * pt.axis + pt.high as usize) {
            Some(n) if n == signal.0 => fixed[i] = Some(vp),
            Some(n) if Some(n) == signal.1 => fixed[i] = Some(-vp),
            Some(_) => fixed[i] = Some(0.0),
            // The rim follows the wall beside it: grounded on a PEC wall
            // (conductor_at above), free beside a radiation wall.
            None => {}
        }
    }
    if !fixed.contains(&Some(vp)) || (signal.1.is_some() && !fixed.contains(&Some(-vp))) {
        return Err(format!(
            "port {}: the signal conductor does not cross the port",
            pt.name
        ));
    }
    let mut nf = 0;
    let free: Vec<Option<usize>> = fixed
        .iter()
        .map(|f| {
            f.is_none().then(|| {
                nf += 1;
                nf - 1
            })
        })
        .collect();
    // Shape gradients at barycentric point l.
    let grads = |g: &[[f64; 3]; 3], l: &[f64; 3]| -> Vec<[f64; 3]> {
        if p_order == 1 {
            g.to_vec()
        } else {
            let mut v: Vec<[f64; 3]> = (0..3).map(|i| scale(g[i], 4.0 * l[i] - 1.0)).collect();
            for (a, b) in TRI_EDGES {
                v.push(scale(add(scale(g[a], l[b]), scale(g[b], l[a])), 4.0));
            }
            v
        }
    };
    let geo: Vec<(f64, [[f64; 3]; 3])> = ftris
        .iter()
        .map(|(c, _)| {
            let (_, a, g) = tri_geo(*c);
            (a, g)
        })
        .collect();
    let solve = |with_eps: bool| -> (Vec<f64>, f64) {
        let mut kt: Vec<Triplet<usize, usize, f64>> = Vec::new();
        let mut rhs = vec![0.0; nf];
        for (k, el) in elems.iter().enumerate() {
            let (area, g) = geo[k];
            let e = if with_eps { tri_eps[ftris[k].1] } else { 1.0 };
            let nl = el.len();
            let mut m = vec![0.0; nl * nl];
            for (l, wq) in &Q_TRI4 {
                let gr = grads(&g, l);
                for i in 0..nl {
                    for j in 0..nl {
                        m[i * nl + j] += wq * area * e * dot(gr[i], gr[j]);
                    }
                }
            }
            for i in 0..nl {
                let Some(fi) = free[el[i]] else { continue };
                for j in 0..nl {
                    match free[el[j]] {
                        Some(fj) => kt.push(Triplet::new(fi, fj, m[i * nl + j])),
                        None => rhs[fi] -= m[i * nl + j] * fixed[el[j]].unwrap(),
                    }
                }
            }
        }
        let mut phi: Vec<f64> = fixed.iter().map(|f| f.unwrap_or(0.0)).collect();
        if nf > 0 {
            let a = SparseColMat::<usize, f64>::try_new_from_triplets(nf, nf, &kt).unwrap();
            let lu = a.sp_lu().unwrap();
            let mut x = faer::Mat::<f64>::zeros(nf, 1);
            for i in 0..nf {
                x[(i, 0)] = rhs[i];
            }
            let x = lu.solve(&x);
            for (gi, f) in free.iter().enumerate() {
                if let Some(fi) = f {
                    phi[gi] = x[(*fi, 0)];
                }
            }
        }
        let mut c = 0.0;
        for (k, el) in elems.iter().enumerate() {
            let (area, g) = geo[k];
            let e = if with_eps { tri_eps[ftris[k].1] } else { 1.0 };
            for (l, wq) in &Q_TRI4 {
                let gr = grads(&g, l);
                let mut v = [0.0; 3];
                for i in 0..el.len() {
                    v = add(v, scale(gr[i], phi[el[i]]));
                }
                c += wq * area * e * dot(v, v);
            }
        }
        (phi, c)
    };
    let (phi, c) = solve(true);
    let (phi0, c0) = solve(false);
    let mut e = Vec::new();
    let mut e0 = Vec::new();
    if sub_n == 1 {
        for (k, el) in elems.iter().enumerate() {
            // The parent's own vertex order, so the quadrature points line
            // up with the 3D integrals.
            let (_, g) = geo[k];
            let mut v = [[0.0; 3]; 6];
            let mut v0 = [[0.0; 3]; 6];
            for (qi, (l, _)) in Q_TRI4.iter().enumerate() {
                let gr = grads(&g, l);
                for i in 0..el.len() {
                    v[qi] = add(v[qi], scale(gr[i], -phi[el[i]]));
                    v0[qi] = add(v0[qi], scale(gr[i], -phi0[el[i]]));
                }
            }
            e.push(v);
            e0.push(v0);
        }
    }
    Ok(Port2d { c, c0, e, e0 })
}

/// Solve the port's mode on its triangles.
fn port_mode(
    model: &Model,
    mesh: &Mesh,
    pt: &Port,
    tris: &[BFace],
    eps: &[c64],
    k0: f64,
    tol: f64,
    order: usize,
) -> Result<PortMesh, String> {
    let (u, w) = others(pt.axis);
    let tri_eps: Vec<f64> = tris.iter().map(|t| eps[t.tet].re).collect();
    // How far the mesh's own port Z0 is from the fine 2D solve's.
    let mut gap = 0.0;
    let (e, e0, info, norm) = match pt.mode {
        Mode::Te10 => {
            let (a, b) = (pt.hi[0] - pt.lo[0], pt.hi[1] - pt.lo[1]);
            // The wide side carries the sine; E points across the narrow side.
            let (wide, along, w0, dir) = if a >= b {
                (a, u, pt.lo[0], w)
            } else {
                (b, w, pt.lo[1], u)
            };
            let er = tri_eps.iter().cloned().fold(0.0, f64::max);
            let kc = std::f64::consts::PI / wide;
            let b2 = k0 * k0 * er - kc * kc;
            if b2 <= 0.0 {
                return Err(format!(
                    "port {}: TE10 is cut off at this frequency",
                    pt.name
                ));
            }
            let beta = b2.sqrt();
            let zte = ETA0 * k0 / beta;
            let mut e = Vec::new();
            for t in tris {
                let p = face_nodes(mesh, &t.f);
                let mut v = [[0.0; 3]; 6];
                for (q, (l, _)) in Q_TRI4.iter().enumerate() {
                    let x = add(add(scale(p[0], l[0]), scale(p[1], l[1])), scale(p[2], l[2]));
                    v[q][dir] = (kc * (x[along] - w0)).sin();
                }
                e.push(v);
            }
            (
                e.clone(),
                e,
                PortInfo {
                    z0: zte,
                    eeff: (beta / k0).powi(2),
                    beta,
                },
                None,
            )
        }
        Mode::Tem { .. } | Mode::Diff { .. } => {
            let signal = match pt.mode {
                Mode::Diff { plus, minus } => (plus, Some(minus)),
                Mode::Tem { signal } => (signal, None),
                Mode::Te10 => unreachable!(),
            };
            // Z0 and Er,eff from a fine second-order solve; the mode's shape
            // on the 3D mesh's own triangles at the 3D order, so the 3D
            // field can match it exactly (a finer shape would leak power).
            let fine = if tris.len() < 2000 {
                6
            } else if tris.len() < 8000 {
                3
            } else {
                2
            };
            let f = port_2d(model, mesh, pt, tris, &tri_eps, signal, tol, fine, 2)?;
            let shape = port_2d(
                model,
                mesh,
                pt,
                tris,
                &tri_eps,
                signal,
                tol,
                1,
                order.min(2),
            )?;
            // Z0 from the fine solve; the gap to the mesh's own triangles
            // tells the solver to refine a coarse port face first.
            let zf = ETA0 / (f.c * f.c0).sqrt();
            let zs = ETA0 / (shape.c * shape.c0).sqrt();
            gap = (zs - zf).abs() / zf;
            let eeff = f.c / f.c0;
            let z0 = zf;
            (
                shape.e,
                shape.e0,
                PortInfo {
                    z0,
                    eeff,
                    beta: k0 * eeff.sqrt(),
                },
                Some(z0),
            )
        }
    };
    let mut ee = 0.0;
    let mut ee0 = 0.0;
    for (ti, t) in tris.iter().enumerate() {
        let (_, area, _) = tri_geo(face_nodes(mesh, &t.f));
        for (q, (_, wq)) in Q_TRI4.iter().enumerate() {
            ee += wq * area * dot(e[ti][q], e[ti][q]);
            ee0 += wq * area * dot(e[ti][q], e0[ti][q]);
        }
    }
    let norm = match norm {
        Some(z0) => (1.0 / z0).sqrt(),
        None => (ee / info.z0).sqrt(),
    };
    // 2D error: jumps of the normal flux (mean field) across triangle sides.
    let mut sides: HashMap<(u32, u32), Vec<(usize, f64)>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        let p = face_nodes(mesh, &t.f);
        let mean = scale(e[ti].iter().fold([0.0; 3], |a, v| add(a, *v)), 1.0 / 6.0);
        let d = scale(mean, tri_eps[ti]);
        for (a, b) in TRI_EDGES {
            let k = ekey(t.f[a], t.f[b]);
            let o = p[3 - a - b];
            let side = sub(mesh.nodes[k.1 as usize], mesh.nodes[k.0 as usize]);
            let len = dot(side, side).sqrt();
            let r = sub(o, mesh.nodes[k.0 as usize]);
            let r = sub(r, scale(side, dot(r, side) / (len * len)));
            // Outward normal of this side, in the plane.
            let m = scale(r, -1.0 / dot(r, r).sqrt());
            sides.entry(k).or_default().push((ti, dot(d, m) * len));
        }
    }
    let mut err = vec![0.0; tris.len()];
    for v in sides.values() {
        if v.len() == 2 {
            let j = (v[0].1 + v[1].1).powi(2);
            err[v[0].0] += j;
            err[v[1].0] += j;
        }
    }
    Ok(PortMesh {
        tris: tris.to_vec(),
        e,
        e0,
        ee0,
        info,
        norm,
        ee,
        err,
        gap,
    })
}

fn czero() -> c64 {
    c64::new(0.0, 0.0)
}

/// The tangential traces of the tetrahedron's basis functions on a
/// boundary face at its quadrature points: for each quadrature point, the
/// (local basis index, tangential value) of the functions living there.
fn face_traces(mesh: &Mesh, asm: &Assembled, bf: &BFace) -> (f64, Vec<Vec<(usize, [f64; 3])>>) {
    let s = &asm.tets[bf.tet];
    let geo = tet_geo(tet_nodes(mesh, s));
    let (n, area, _) = tri_geo(face_nodes(mesh, &bf.f));
    let ks = keys(s, asm.order);
    let live: Vec<usize> = (0..ks.len()).filter(|&i| on_face(&ks[i], &bf.f)).collect();
    let mut out = Vec::with_capacity(6);
    let mut b = Vec::new();
    for (lq, _) in &Q_TRI4 {
        let mut l = [0.0; 4];
        for k in 0..3 {
            l[bf.local[k]] = lq[k];
        }
        basis(&geo.g, &l, asm.order, &mut b);
        out.push(
            live.iter()
                .map(|&i| {
                    let v = b[i].0;
                    (i, sub(v, scale(n, dot(v, n))))
                })
                .collect(),
        );
    }
    (area, out)
}

/// Solve at one frequency; per excitation, the unknowns, and the S matrix.
fn solve_at(
    model: &Model,
    mesh: &Mesh,
    f_ghz: f64,
    order: usize,
    iterative: Option<bool>,
    pre: Option<(f64, &mut Option<Precond>)>,
) -> Result<(Assembled, Vec<Vec<c64>>, Vec<c64>), String> {
    let tm = std::time::Instant::now();
    let tlog = |what: &str| {
        if std::env::var_os("TRACE_FEM_TIME").is_some() {
            eprintln!("  t {what}: {:.1} s", tm.elapsed().as_secs_f64());
        }
    };
    let asm = assemble_mesh(model, mesh, f_ghz, order)?;
    tlog("assemble_mesh");
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let n = asm.n;
    if n == 0 {
        return Err("no unknowns".into());
    }
    let quad: Vec<([f64; 4], f64)> = if order >= 2 {
        q_tet4()
    } else {
        Q_TET2.to_vec()
    };
    let nb = if order >= 2 { 20 } else { 6 };
    let iter = iterative.unwrap_or(n > ITER_MIN);
    let mut trip: Vec<Triplet<usize, usize, c64>> =
        Vec::with_capacity(asm.tets.len() * nb * nb / 2);
    // The iterative solver's factor is taken at f_pre: keep ε·mass.
    let mut emass: Vec<[f32; 2]> = Vec::new();
    let mut b = Vec::new();
    let mut stiff = vec![0.0; nb * nb];
    let mut mass = vec![0.0; nb * nb];
    for (ti, s) in asm.tets.iter().enumerate() {
        let geo = tet_geo(tet_nodes(mesh, s));
        stiff.iter_mut().for_each(|v| *v = 0.0);
        mass.iter_mut().for_each(|v| *v = 0.0);
        for (l, wq) in &quad {
            basis(&geo.g, l, order, &mut b);
            let w = wq * geo.vol;
            for a in 0..nb {
                for c in 0..nb {
                    stiff[a * nb + c] += w * dot(b[a].1, b[c].1);
                    mass[a * nb + c] += w * dot(b[a].0, b[c].0);
                }
            }
        }
        let ke = asm.eps[ti] * (k0 * k0);
        let d = &asm.dof[ti];
        for a in 0..nb {
            let Some(da) = d[a] else { continue };
            for c in 0..nb {
                let Some(dc) = d[c] else { continue };
                let v = c64::new(stiff[a * nb + c], 0.0) - ke * mass[a * nb + c];
                trip.push(Triplet::new(da as usize, dc as usize, v));
                if iter {
                    let em = asm.eps[ti] * mass[a * nb + c];
                    emass.push([em.re as f32, em.im as f32]);
                }
            }
        }
    }
    let add_face = |trip: &mut Vec<Triplet<usize, usize, c64>>, bf: &BFace, coef: c64| {
        let (area, tr) = face_traces(mesh, &asm, bf);
        let d = &asm.dof[bf.tet];
        for (q, (_, wq)) in Q_TRI4.iter().enumerate() {
            for (ia, va) in &tr[q] {
                let Some(da) = d[*ia] else { continue };
                for (ic, vc) in &tr[q] {
                    let Some(dc) = d[*ic] else { continue };
                    trip.push(Triplet::new(
                        da as usize,
                        dc as usize,
                        coef * (wq * area * dot(*va, *vc)),
                    ));
                }
            }
        }
    };
    for (bf, k) in &asm.rad {
        add_face(&mut trip, bf, c64::new(0.0, *k));
    }
    // Ports. Each absorbs everything locally (jβ on its face, which is
    // exact for the mode in a uniform fill) and, as a rank-2 correction
    // solved by Woodbury's identity, swaps that for the exact modal
    // condition on the mode itself: n x curl E = -2jβ e0 + jβ V e0 with
    // V = ∫E·e0 / ∫e·e0, e0 the magnetic shape. On microstrip, where the
    // dielectric makes the electric and magnetic shapes differ, the plain
    // local condition loses a few percent of the power.
    let np = asm.ports.len();
    let mut be = vec![vec![0.0f64; n]; np];
    let mut b0 = vec![vec![0.0f64; n]; np];
    for (pi, pm) in asm.ports.iter().enumerate() {
        let jb = c64::new(0.0, pm.info.beta);
        for (ti, bf) in pm.tris.iter().enumerate() {
            add_face(&mut trip, bf, jb);
            let (area, tr) = face_traces(mesh, &asm, bf);
            let d = &asm.dof[bf.tet];
            for (q, (_, wq)) in Q_TRI4.iter().enumerate() {
                for (ia, va) in &tr[q] {
                    if let Some(da) = d[*ia] {
                        be[pi][da as usize] += wq * area * dot(*va, pm.e[ti][q]);
                        b0[pi][da as usize] += wq * area * dot(*va, pm.e0[ti][q]);
                    }
                }
            }
        }
    }
    // Columns: [b0 of each port, be of each port].
    let m2 = 2 * np;
    let mut cols = faer::Mat::<c64>::zeros(n, m2);
    for p in 0..np {
        for r in 0..n {
            cols[(r, p)] = c64::new(b0[p][r], 0.0);
            cols[(r, np + p)] = c64::new(be[p][r], 0.0);
        }
    }
    tlog("matrix");
    let y = if iter {
        let (fp, cache) = match pre {
            Some((fp, c)) => (fp, Some(c)),
            None => (f_ghz, None),
        };
        let kp = 2.0 * std::f64::consts::PI * fp * 1e9 / C0;
        let shift = c64::new(k0 * k0 - kp * kp, 0.0);
        iter_solve(
            n,
            trip,
            &cols,
            &node_gradients(&asm),
            &emass,
            shift,
            fp,
            cache,
        )?
    } else {
        sym_solve(n, trip, &cols)?
    };
    tlog("solve");
    // A = A0 + U C Uᵀ, U = [b0 | be], C = diag(jβ/ee0, -jβ/ee).
    let mut cinv_d = vec![czero(); m2];
    for (p, pm) in asm.ports.iter().enumerate() {
        let jb = c64::new(0.0, pm.info.beta);
        cinv_d[p] = c64::new(pm.ee0, 0.0) / jb;
        cinv_d[np + p] = -c64::new(pm.ee, 0.0) / jb;
    }
    let ucol = |k: usize| -> &Vec<f64> {
        if k < np {
            &b0[k]
        } else {
            &be[k - np]
        }
    };
    // S = C⁻¹ + Uᵀ A0⁻¹ U.
    let mut sm = vec![czero(); m2 * m2];
    for i in 0..m2 {
        let u = ucol(i);
        for j in 0..m2 {
            let mut acc = czero();
            for r in 0..n {
                if u[r] != 0.0 {
                    acc += y[j][r] * u[r];
                }
            }
            sm[i * m2 + j] = acc;
        }
        sm[i * m2 + i] += cinv_d[i];
    }
    let sinv = cinv(&sm, m2);
    // Excitation p: r = 2jβ b0_p, so A0⁻¹ r = 2jβ y[p]; x = z − Y S⁻¹ Uᵀ z.
    let mut sols = Vec::with_capacity(np);
    for (p, pm) in asm.ports.iter().enumerate() {
        let k2 = c64::new(0.0, 2.0 * pm.info.beta);
        let z: Vec<c64> = y[p].iter().map(|v| *v * k2).collect();
        let utz: Vec<c64> = (0..m2)
            .map(|i| {
                let u = ucol(i);
                let mut acc = czero();
                for r in 0..n {
                    if u[r] != 0.0 {
                        acc += z[r] * u[r];
                    }
                }
                acc
            })
            .collect();
        let mut coef = vec![czero(); m2];
        for i in 0..m2 {
            for j in 0..m2 {
                coef[i] += sinv[i * m2 + j] * utz[j];
            }
        }
        let mut x = z;
        for j in 0..m2 {
            if coef[j] != czero() {
                for r in 0..n {
                    x[r] -= y[j][r] * coef[j];
                }
            }
        }
        sols.push(x);
    }
    // Mode voltages at each port for each excitation.
    let mut s = vec![czero(); np * np];
    for (pq, pm) in asm.ports.iter().enumerate() {
        for pe in 0..np {
            let mut g = czero();
            for r in 0..n {
                if b0[pq][r] != 0.0 {
                    g += sols[pe][r] * b0[pq][r];
                }
            }
            let mut v = g / pm.ee0;
            if pq == pe {
                v -= c64::new(1.0, 0.0);
            }
            let mut v = v * (pm.norm / asm.ports[pe].norm);
            // De-embed: move both reference planes inwards.
            let shift = model.ports[pq].deembed * pm.info.beta
                + model.ports[pe].deembed * asm.ports[pe].info.beta;
            v *= c64::new(shift.cos(), shift.sin());
            s[pq * np + pe] = v;
        }
    }
    Ok((asm, sols, s))
}

/// The factor's number type: f64, or f32 for half the memory (then
/// polished by GMRES).
trait Real: faer::traits::RealField + Copy {
    fn of(x: f64) -> Self;
    fn to(self) -> f64;
}

impl Real for f64 {
    fn of(x: f64) -> Self {
        x
    }
    fn to(self) -> f64 {
        self
    }
}

impl Real for f32 {
    fn of(x: f64) -> Self {
        x as f32
    }
    fn to(self) -> f64 {
        self as f64
    }
}

/// A factorised complex symmetric matrix, kept for many solves. It is
/// factorised as the real symmetric indefinite system [[Re, Im], [Im, −Re]]
/// (unknowns Re x and −Im x side by side), with a minimum-degree ordering
/// and supernodal Bunch-Kaufman LBLᵀ: about half the memory of a general
/// LU with column ordering on FEM matrices.
struct SymFactor<T: Real> {
    n: usize,
    sym: faer::sparse::linalg::cholesky::SymbolicCholesky<usize>,
    lv: Vec<T>,
    sub: Vec<T>,
    pf: Vec<usize>,
    pi: Vec<usize>,
}

impl<T: Real> SymFactor<T> {
    /// From (row, col, value) entries, both triangles given (duplicates
    /// add up).
    fn new(n: usize, trip: &[Triplet<usize, usize, c64>]) -> Result<SymFactor<T>, String> {
        use faer::dyn_stack::{MemBuffer, MemStack};
        use faer::sparse::linalg::cholesky::{
            factorize_symbolic_cholesky, CholeskySymbolicParams, SymmetricOrdering,
        };
        let mut rt: Vec<Triplet<usize, usize, T>> = Vec::with_capacity(trip.len() * 2);
        for t in trip {
            let (i, j, v) = (t.row, t.col, t.val);
            if i < j {
                continue;
            }
            if i > j {
                rt.push(Triplet::new(2 * i, 2 * j, T::of(v.re)));
                rt.push(Triplet::new(2 * i, 2 * j + 1, T::of(v.im)));
                rt.push(Triplet::new(2 * i + 1, 2 * j, T::of(v.im)));
                rt.push(Triplet::new(2 * i + 1, 2 * j + 1, T::of(-v.re)));
            } else {
                rt.push(Triplet::new(2 * i, 2 * i, T::of(v.re)));
                rt.push(Triplet::new(2 * i + 1, 2 * i, T::of(v.im)));
                rt.push(Triplet::new(2 * i + 1, 2 * i + 1, T::of(-v.re)));
            }
        }
        let m = 2 * n;
        let a = SparseColMat::<usize, T>::try_new_from_triplets(m, m, &rt)
            .map_err(|e| format!("{e:?}"))?;
        drop(rt);
        let sym = factorize_symbolic_cholesky(
            a.symbolic(),
            faer::Side::Lower,
            SymmetricOrdering::Amd,
            CholeskySymbolicParams::default(),
        )
        .map_err(|e| format!("symbolic factorisation failed: {e:?}"))?;
        let par = faer::Par::rayon(0);
        let mut mem = MemBuffer::try_new(
            sym.factorize_numeric_intranode_lblt_scratch::<T>(par, Default::default()),
        )
        .map_err(|_| "out of memory for the factorisation".to_string())?;
        let stack = MemStack::new(&mut mem);
        let mut lv = Vec::new();
        lv.try_reserve_exact(sym.len_val()).map_err(|_| {
            format!(
                "out of memory: the factor needs {:.1} GB",
                sym.len_val() as f64 * std::mem::size_of::<T>() as f64 * 1e-9
            )
        })?;
        lv.resize(sym.len_val(), T::of(0.0));
        let mut sub = vec![T::of(0.0); m];
        let mut pf = vec![0usize; m];
        let mut pi = vec![0usize; m];
        sym.factorize_numeric_intranode_lblt(
            &mut lv,
            &mut sub,
            &mut pf,
            &mut pi,
            a.as_ref(),
            faer::Side::Lower,
            par,
            stack,
            Default::default(),
        );
        Ok(SymFactor {
            n,
            sym,
            lv,
            sub,
            pf,
            pi,
        })
    }

    /// Solves for each column of `x` in place.
    fn solve(&self, x: &mut faer::Mat<c64>) {
        use faer::dyn_stack::{MemBuffer, MemStack};
        use faer::sparse::linalg::cholesky::IntranodeLbltRef;
        let (n, k) = (self.n, x.ncols());
        let par = faer::Par::rayon(0);
        let mut mem = MemBuffer::new(self.sym.solve_in_place_scratch::<T>(k, par));
        let stack = MemStack::new(&mut mem);
        let perm = faer::perm::PermRef::new_checked(&self.pf, &self.pi, 2 * n);
        let lblt = IntranodeLbltRef::new(&self.sym, &self.lv, &self.sub, perm);
        let mut y = faer::Mat::<T>::from_fn(2 * n, k, |r, c| {
            let v = x[(r / 2, c)];
            T::of(if r % 2 == 0 { v.re } else { v.im })
        });
        lblt.solve_in_place_with_conj(faer::Conj::No, y.as_mut(), par, stack);
        for c in 0..k {
            for r in 0..n {
                x[(r, c)] = c64::new(y[(2 * r, c)].to(), -y[(2 * r + 1, c)].to());
            }
        }
    }
}

/// Solves the complex symmetric system (`trip`, both triangles given) for
/// each column of `rhs`, directly.
fn sym_solve(
    n: usize,
    trip: Vec<Triplet<usize, usize, c64>>,
    rhs: &faer::Mat<c64>,
) -> Result<Vec<Vec<c64>>, String> {
    let f = SymFactor::<f64>::new(n, &trip)?;
    drop(trip);
    let mut x = rhs.clone();
    f.solve(&mut x);
    Ok((0..x.ncols())
        .map(|c| (0..n).map(|r| x[(r, c)]).collect())
        .collect())
}

/// A sparse matrix by rows.
struct Csr {
    ptr: Vec<usize>,
    col: Vec<u32>,
    val: Vec<c64>,
}

impl Csr {
    /// From entries (duplicates add up).
    fn new(n: usize, mut trip: Vec<Triplet<usize, usize, c64>>) -> Csr {
        trip.sort_unstable_by_key(|t| (t.row, t.col));
        let mut ptr = vec![0usize; n + 1];
        let mut col: Vec<u32> = Vec::with_capacity(trip.len() / 2);
        let mut val: Vec<c64> = Vec::with_capacity(trip.len() / 2);
        let mut last = (usize::MAX, usize::MAX);
        for t in &trip {
            if (t.row, t.col) == last {
                *val.last_mut().unwrap() += t.val;
            } else {
                col.push(t.col as u32);
                val.push(t.val);
                ptr[t.row + 1] += 1;
                last = (t.row, t.col);
            }
        }
        for i in 0..n {
            ptr[i + 1] += ptr[i];
        }
        Csr { ptr, col, val }
    }

    fn n(&self) -> usize {
        self.ptr.len() - 1
    }

    /// y = A x, rows split over the threads.
    fn mul(&self, x: &[c64], y: &mut [c64]) {
        let threads = std::thread::available_parallelism().map_or(4, |t| t.get());
        let chunk = self.n().div_ceil(threads).max(1);
        std::thread::scope(|sc| {
            for (k, ys) in y.chunks_mut(chunk).enumerate() {
                sc.spawn(move || {
                    for (i, yi) in ys.iter_mut().enumerate() {
                        let r = k * chunk + i;
                        let mut acc = czero();
                        for p in self.ptr[r]..self.ptr[r + 1] {
                            acc += self.val[p] * x[self.col[p] as usize];
                        }
                        *yi = acc;
                    }
                });
            }
        });
    }
}

/// Right-preconditioned restarted flexible GMRES for A x = b, for several
/// right-hand sides in step, so the preconditioner works on all of them
/// at once (`m` maps a batch of vectors to M⁻¹ of each). To a relative
/// residual of `tol`; returns each system's iterations, or an error.
fn gmres(
    a: &Csr,
    m: &dyn Fn(&[&[c64]]) -> Vec<Vec<c64>>,
    bs: &[Vec<c64>],
    xs: &mut [Vec<c64>],
    tol: f64,
    max_it: usize,
) -> Result<Vec<usize>, String> {
    const RESTART: usize = 25;
    struct St {
        v: Vec<Vec<c64>>,
        zs: Vec<Vec<c64>>,
        h: Vec<Vec<c64>>,
        cs: Vec<c64>,
        sn: Vec<c64>,
        g: Vec<c64>,
        k: usize,
        it: usize,
        bn: f64,
        done: bool,
        open: bool,
    }
    let n = a.n();
    let norm = |v: &[c64]| v.iter().map(|z| z.norm_sqr()).sum::<f64>().sqrt();
    let dotc = |u: &[c64], v: &[c64]| -> c64 { u.iter().zip(v).map(|(p, q)| p.conj() * q).sum() };
    let mut st: Vec<St> = bs
        .iter()
        .map(|b| St {
            v: Vec::new(),
            zs: Vec::new(),
            h: Vec::new(),
            cs: Vec::new(),
            sn: Vec::new(),
            g: Vec::new(),
            k: 0,
            it: 0,
            bn: norm(b).max(1e-300),
            done: false,
            open: false,
        })
        .collect();
    let mut tmp = vec![czero(); n];
    let mut w = vec![czero(); n];
    loop {
        // Restart: true residuals.
        for (i, s) in st.iter_mut().enumerate() {
            if s.done {
                continue;
            }
            a.mul(&xs[i], &mut tmp);
            let r: Vec<c64> = bs[i].iter().zip(&tmp).map(|(p, q)| p - q).collect();
            let beta = norm(&r);
            if beta / s.bn < tol {
                s.done = true;
                continue;
            }
            if s.it >= max_it {
                return Err(format!(
                    "iterative solver: residual {:.1e} after {} iterations",
                    beta / s.bn,
                    s.it
                ));
            }
            s.v = vec![r.iter().map(|q| q / beta).collect()];
            s.zs.clear();
            s.h = vec![vec![czero(); RESTART]; RESTART + 1];
            s.cs = vec![czero(); RESTART];
            s.sn = vec![czero(); RESTART];
            s.g = vec![czero(); RESTART + 1];
            s.g[0] = c64::new(beta, 0.0);
            s.k = 0;
            s.open = true;
        }
        if st.iter().all(|s| s.done) {
            return Ok(st.iter().map(|s| s.it).collect());
        }
        while st.iter().any(|s| s.open) {
            let act: Vec<usize> = (0..st.len()).filter(|&i| st[i].open).collect();
            let zb = {
                let vs: Vec<&[c64]> = act.iter().map(|&i| &st[i].v[st[i].k][..]).collect();
                m(&vs)
            };
            for (&i, z) in act.iter().zip(zb) {
                let s = &mut st[i];
                let k = s.k;
                a.mul(&z, &mut w);
                s.zs.push(z);
                for j in 0..=k {
                    let hj = dotc(&s.v[j], &w);
                    s.h[j][k] = hj;
                    for (wi, vi) in w.iter_mut().zip(&s.v[j]) {
                        *wi -= hj * vi;
                    }
                }
                let wn = norm(&w);
                s.h[k + 1][k] = c64::new(wn, 0.0);
                for j in 0..k {
                    let t = s.cs[j].conj() * s.h[j][k] + s.sn[j].conj() * s.h[j + 1][k];
                    s.h[j + 1][k] = -s.sn[j] * s.h[j][k] + s.cs[j] * s.h[j + 1][k];
                    s.h[j][k] = t;
                }
                let (p, q) = (s.h[k][k], s.h[k + 1][k]);
                let d = (p.norm_sqr() + q.norm_sqr()).sqrt().max(1e-300);
                s.cs[k] = p / d;
                s.sn[k] = q / d;
                s.h[k][k] = c64::new(d, 0.0);
                s.h[k + 1][k] = czero();
                s.g[k + 1] = -s.sn[k] * s.g[k];
                s.g[k] = s.cs[k].conj() * s.g[k];
                s.it += 1;
                s.k += 1;
                if s.it.is_multiple_of(25) && std::env::var_os("TRACE_FEM_MESH").is_some() {
                    eprintln!("  gmres {}: residual {:.1e}", s.it, s.g[s.k].norm() / s.bn);
                }
                if s.k < RESTART && s.it < max_it && s.g[s.k].norm() / s.bn >= tol && wn > 1e-300 {
                    s.v.push(w.iter().map(|q| q / wn).collect());
                    continue;
                }
                // End of the cycle: x += Z y.
                s.open = false;
                let kk = s.k;
                let mut y = vec![czero(); kk];
                for r in (0..kk).rev() {
                    let mut acc = s.g[r];
                    for j in r + 1..kk {
                        acc -= s.h[r][j] * y[j];
                    }
                    y[r] = acc / s.h[r][r];
                }
                for (j, yj) in y.iter().enumerate() {
                    for (xi, zi) in xs[i].iter_mut().zip(&s.zs[j]) {
                        *xi += yj * zi;
                    }
                }
                s.v.clear();
                s.zs.clear();
            }
        }
    }
}

/// The gradient of each free node's hat function, as Whitney unknowns
/// (∇λm is the sum of the Whitney functions of the edges at m, +1 where
/// the edge points into m), then (second order) each edge-gradient
/// function. Nodes touching a PEC edge are left out.
fn node_gradients(asm: &Assembled) -> Vec<Vec<(u32, f64)>> {
    let nn = asm
        .tets
        .iter()
        .flatten()
        .map(|&v| v as usize + 1)
        .max()
        .unwrap_or(0);
    let mut cols: Vec<Vec<(u32, f64)>> = vec![Vec::new(); nn];
    let mut bad = vec![false; nn];
    for (t, d) in asm.tets.iter().zip(&asm.dof) {
        for (k, &(i, j)) in LOCAL_EDGES.iter().enumerate() {
            let (a, b) = (t[i] as usize, t[j] as usize);
            match d[k] {
                Some(e) => {
                    cols[a].push((e, -1.0));
                    cols[b].push((e, 1.0));
                }
                None => {
                    bad[a] = true;
                    bad[b] = true;
                }
            }
        }
    }
    let mut out: Vec<Vec<(u32, f64)>> = cols
        .into_iter()
        .zip(bad)
        .filter(|(c, b)| !b && !c.is_empty())
        .map(|(mut c, _)| {
            c.sort_unstable_by_key(|x| x.0);
            c.dedup_by_key(|x| x.0);
            c
        })
        .collect();
    // Second order: the edge functions ∇(λiλj) are gradients already.
    if asm.order >= 2 {
        let mut seen = vec![false; asm.n];
        for d in &asm.dof {
            for e in d[6..12].iter().flatten() {
                if !std::mem::replace(&mut seen[*e as usize], true) {
                    out.push(vec![(*e, 1.0)]);
                }
            }
        }
    }
    out
}

/// The iterative solver's single-precision factor, kept between sweep
/// points that share its frequency.
pub struct Precond {
    f: f64,
    n: usize,
    d: Vec<f64>,
    m: SymFactor<f32>,
}

/// Floater-Hormann rational interpolation (degree up to 3) of y over the
/// sorted nodes x, at t.
fn fh_weights(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let d = 3.min(n.saturating_sub(1));
    (0..n)
        .map(|k| {
            let mut w = 0.0;
            for i in k.saturating_sub(d)..=k.min(n - 1 - d) {
                let mut p = 1.0;
                for j in i..=i + d {
                    if j != k {
                        p /= (x[k] - x[j]).abs();
                    }
                }
                w += p;
            }
            if (k as i64 - d as i64) % 2 != 0 {
                -w
            } else {
                w
            }
        })
        .collect()
}

/// S at `f` from the solved points (rational interpolation; exact at a
/// solved point).
fn interp_s(solved: &[(f64, Vec<c64>, Vec<PortInfo>)], f: f64) -> Vec<c64> {
    if let Some(p) = solved.iter().find(|p| (p.0 - f).abs() < 1e-12) {
        return p.1.clone();
    }
    let x: Vec<f64> = solved.iter().map(|p| p.0).collect();
    let w = fh_weights(&x);
    let m = solved[0].1.len();
    let mut num = vec![czero(); m];
    let mut den = 0.0;
    for ((xk, sk, _), wk) in solved.iter().zip(&w) {
        let c = wk / (f - xk);
        den += c;
        for (o, v) in num.iter_mut().zip(sk) {
            *o += v * c;
        }
    }
    num.iter().map(|v| v / den).collect()
}

/// Port modes at `f`: linear between the solved points.
fn interp_info(solved: &[(f64, Vec<c64>, Vec<PortInfo>)], f: f64) -> Vec<PortInfo> {
    let k = solved
        .partition_point(|p| p.0 < f)
        .clamp(1, solved.len().max(2) - 1);
    if solved.len() == 1 {
        return solved[0].2.clone();
    }
    let (a, b) = (&solved[k - 1], &solved[k]);
    let t = ((f - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    a.2.iter()
        .zip(&b.2)
        .map(|(p, q)| PortInfo {
            z0: p.z0 + (q.z0 - p.z0) * t,
            eeff: p.eeff + (q.eeff - p.eeff) * t,
            beta: p.beta + (q.beta - p.beta) * t,
        })
        .collect()
}

/// The frequency whose factor preconditions a sweep point at `f`: the
/// adaptive frequency, or for points below 40% of it (where single
/// precision loses too much of the static field), 40% of it.
/// TRACE_FEM_FPRE sets the share; TRACE_FEM_ANCHOR=0 factors every point.
fn anchor(f: f64, f_adapt: f64) -> f64 {
    let env = |k: &str, d: f64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let lo = env("TRACE_FEM_FPRE", 0.4) * f_adapt;
    if env("TRACE_FEM_ANCHOR", 1.0) == 0.0 {
        return f.max(lo);
    }
    if f >= lo {
        f_adapt
    } else {
        lo
    }
}

/// Solves with the factor in single precision (half the memory of the
/// direct solve), polished to double precision by preconditioned GMRES.
/// `grad` (node gradients) adds an exact correction on the gradient
/// fields, which single precision loses at low frequencies (there they
/// are k² smaller than the rest of the matrix).
fn iter_solve(
    n: usize,
    trip: Vec<Triplet<usize, usize, c64>>,
    rhs: &faer::Mat<c64>,
    grad: &[Vec<(u32, f64)>],
    emass: &[[f32; 2]],
    shift: c64,
    f_pre: f64,
    cache: Option<&mut Option<Precond>>,
) -> Result<Vec<Vec<c64>>, String> {
    let mut local = None;
    let slot = cache.unwrap_or(&mut local);
    if slot
        .as_ref()
        .is_some_and(|p| p.n != n || (p.f - f_pre).abs() > 1e-9)
    {
        *slot = None;
    }
    let mut trip = trip;
    if slot.is_none() {
        // Scale to a unit diagonal first (D A D): the basis functions
        // differ widely in size, and single precision keeps far more of
        // the small-eigenvalue (low-frequency, gradient) part of a
        // balanced matrix.
        let mut diag = vec![0.0f64; n];
        for t in &trip {
            if t.row == t.col {
                diag[t.row] += t.val.norm();
            }
        }
        let d: Vec<f64> = diag
            .iter()
            .map(|&v| if v > 1e-300 { 1.0 / v.sqrt() } else { 1.0 })
            .collect();
        // The factor: of A + shift·εM (A at the preconditioner's
        // frequency, where single precision still holds the gradient
        // fields).
        // In place: scale and shift, factor, then take the shift off.
        let em = |i: usize| {
            emass
                .get(i)
                .map_or(czero(), |m| c64::new(m[0] as f64, m[1] as f64))
        };
        for (i, t) in trip.iter_mut().enumerate() {
            t.val = (t.val + shift * em(i)) * (d[t.row] * d[t.col]);
        }
        let m = SymFactor::<f32>::new(n, &trip)?;
        for (i, t) in trip.iter_mut().enumerate() {
            t.val -= shift * em(i) * (d[t.row] * d[t.col]);
        }
        *slot = Some(Precond { f: f_pre, n, d, m });
    } else if let Some(pc) = slot.as_ref() {
        for t in trip.iter_mut() {
            t.val *= pc.d[t.row] * pc.d[t.col];
        }
    }
    let Some(pc) = slot.as_ref() else {
        return Err("no preconditioner".into());
    };
    let (d, m) = (&pc.d, &pc.m);
    let a = Csr::new(n, trip);
    // Gradient space in the scaled unknowns: G' = D⁻¹ G; S = G'ᵀ A' G'.
    let g: Vec<Vec<(u32, f64)>> = grad
        .iter()
        .map(|c| c.iter().map(|&(e, sg)| (e, sg / d[e as usize])).collect())
        .collect();
    let mut owner: Vec<[(u32, f64); 2]> = vec![[(u32::MAX, 0.0); 2]; n];
    for (m, c) in g.iter().enumerate() {
        for &(e, w) in c {
            let o = &mut owner[e as usize];
            let k = if o[0].0 == u32::MAX { 0 } else { 1 };
            o[k] = (m as u32, w);
        }
    }
    let mut st: Vec<Triplet<usize, usize, c64>> = Vec::new();
    for r in 0..n {
        for &(mr, wr) in &owner[r] {
            if mr == u32::MAX {
                continue;
            }
            for p in a.ptr[r]..a.ptr[r + 1] {
                for &(mc, wc) in &owner[a.col[p] as usize] {
                    if mc != u32::MAX {
                        st.push(Triplet::new(mr as usize, mc as usize, a.val[p] * (wr * wc)));
                    }
                }
            }
        }
    }
    drop(owner);
    let ng = g.len();
    let tm = std::time::Instant::now();
    let tlog = |what: &str| {
        if std::env::var_os("TRACE_FEM_TIME").is_some() {
            eprintln!("  t   {what}: {:.1} s", tm.elapsed().as_secs_f64());
        }
    };
    tlog("factor+csr+coarse matrix");
    let sf = if ng > 0 {
        Some(SymFactor::<f64>::new(ng, &st)?)
    } else {
        None
    };
    drop(st);
    // G S⁻¹ Gᵀ on a batch, added into zs.
    let coarse = |rs: &[&[c64]], zs: &mut [Vec<c64>]| {
        let Some(sf) = &sf else { return };
        let mut q = faer::Mat::<c64>::from_fn(ng, rs.len(), |m, c| {
            g[m].iter().map(|&(e, w)| rs[c][e as usize] * w).sum()
        });
        sf.solve(&mut q);
        for (c, z) in zs.iter_mut().enumerate() {
            for (m, col) in g.iter().enumerate() {
                let qm = q[(m, c)];
                for &(e, w) in col {
                    z[e as usize] += qm * w;
                }
            }
        }
    };
    let resid = |rs: &[&[c64]], zs: &[Vec<c64>]| -> Vec<Vec<c64>> {
        rs.iter()
            .zip(zs)
            .map(|(r, z)| {
                let mut az = vec![czero(); n];
                a.mul(z, &mut az);
                r.iter().zip(&az).map(|(p, q)| p - q).collect()
            })
            .collect()
    };
    // z = G S⁻¹ Gᵀ r, then the single-precision factor on what is left,
    // then the gradient part once more.
    let pre = |rs: &[&[c64]]| -> Vec<Vec<c64>> {
        let mut zs = vec![vec![czero(); n]; rs.len()];
        coarse(rs, &mut zs);
        let r2 = resid(rs, &zs);
        let mut x = faer::Mat::<c64>::from_fn(n, rs.len(), |r, c| r2[c][r]);
        drop(r2);
        m.solve(&mut x);
        for (c, z) in zs.iter_mut().enumerate() {
            for (r, zr) in z.iter_mut().enumerate() {
                *zr += x[(r, c)];
            }
        }
        drop(x);
        let r3 = resid(rs, &zs);
        let r3r: Vec<&[c64]> = r3.iter().map(|v| &v[..]).collect();
        coarse(&r3r, &mut zs);
        zs
    };
    let bs: Vec<Vec<c64>> = (0..rhs.ncols())
        .map(|c| (0..n).map(|r| rhs[(r, c)] * d[r]).collect())
        .collect();
    let mut ys = vec![vec![czero(); n]; bs.len()];
    tlog("before gmres");
    let its = gmres(&a, &pre, &bs, &mut ys, 1e-9, 300)?;
    tlog("gmres");
    if std::env::var_os("TRACE_FEM_MESH").is_some() {
        eprintln!("  gmres: {its:?} iterations");
    }
    let out = ys
        .into_iter()
        .map(|y| y.iter().zip(d).map(|(v, s)| v * s).collect())
        .collect();
    Ok(out)
}

/// Field and curl in tetrahedron `ti` at barycentric point `l`.
fn field_at(
    asm: &Assembled,
    geo: &Geo,
    ti: usize,
    sol: &[c64],
    l: &[f64; 4],
    b: &mut Vec<([f64; 3], [f64; 3])>,
) -> ([c64; 3], [c64; 3]) {
    basis(&geo.g, l, asm.order, b);
    let mut e = [czero(); 3];
    let mut c = [czero(); 3];
    for (k, d) in asm.dof[ti].iter().enumerate() {
        let Some(d) = d else { continue };
        let x = sol[*d as usize];
        for a in 0..3 {
            e[a] += x * b[k].0[a];
            c[a] += x * b[k].1[a];
        }
    }
    (e, c)
}

/// Per-tetrahedron error indicator from the solutions.
fn estimate(mesh: &Mesh, asm: &Assembled, sols: &[Vec<c64>], f_ghz: f64) -> Vec<f64> {
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let nt = asm.tets.len();
    let mut eta = vec![0.0; nt];
    let geos: Vec<Geo> = asm
        .tets
        .iter()
        .map(|s| tet_geo(tet_nodes(mesh, s)))
        .collect();
    let mut faces: HashMap<[u32; 3], Vec<(usize, [usize; 3])>> = HashMap::new();
    for (ti, s) in asm.tets.iter().enumerate() {
        for (a, b, c) in LOCAL_FACES {
            faces
                .entry([s[a], s[b], s[c]])
                .or_default()
                .push((ti, [a, b, c]));
        }
    }
    let mut b = Vec::new();
    for sol in sols {
        for ti in 0..nt {
            let (e, _) = field_at(asm, &geos[ti], ti, sol, &[0.25; 4], &mut b);
            let h2 = geos[ti].vol.powf(2.0 / 3.0);
            let r: f64 = e
                .iter()
                .map(|v| (*v * asm.eps[ti] * (k0 * k0)).norm_sqr())
                .sum();
            eta[ti] += h2 * geos[ti].vol * r;
        }
        for (f, ts) in &faces {
            if ts.len() != 2 {
                continue;
            }
            let (n, area, _) = tri_geo(face_nodes(mesh, f));
            let mut jump_n = czero();
            let mut cs = Vec::new();
            for (side, (ti, loc)) in ts.iter().enumerate() {
                let mut l = [0.0; 4];
                for k in loc {
                    l[*k] = 1.0 / 3.0;
                }
                let (e, c) = field_at(asm, &geos[*ti], *ti, sol, &l, &mut b);
                let sign = if side == 0 { 1.0 } else { -1.0 };
                jump_n += (e[0] * n[0] + e[1] * n[1] + e[2] * n[2]) * asm.eps[*ti] * sign;
                cs.push(c);
            }
            let d = [
                cs[0][0] - cs[1][0],
                cs[0][1] - cs[1][1],
                cs[0][2] - cs[1][2],
            ];
            let nx = [
                d[1] * n[2] - d[2] * n[1],
                d[2] * n[0] - d[0] * n[2],
                d[0] * n[1] - d[1] * n[0],
            ];
            let jump_t: f64 = nx.iter().map(|v| v.norm_sqr()).sum();
            let v = area.sqrt() * area * (jump_t + k0 * k0 * jump_n.norm_sqr()) * 0.5;
            eta[ts[0].0] += v;
            eta[ts[1].0] += v;
        }
    }
    eta
}

fn max_delta(a: &[c64], b: &[c64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x - *y).norm())
        .fold(0.0, f64::max)
}

/// Renormalise S from each port's own Z0 to `z`.
pub fn renormalise(s: &[c64], z0: &[f64], z: f64) -> Vec<c64> {
    let n = z0.len();
    let id = |i: usize, j: usize| if i == j { c64::new(1.0, 0.0) } else { czero() };
    // Z = D (I - S)^-1 (I + S) D
    let ims: Vec<c64> = (0..n * n).map(|k| id(k / n, k % n) - s[k]).collect();
    let ips: Vec<c64> = (0..n * n).map(|k| id(k / n, k % n) + s[k]).collect();
    let inv = cinv(&ims, n);
    let mut zm = matmul(&inv, &ips, n);
    for i in 0..n {
        for j in 0..n {
            zm[i * n + j] *= (z0[i] * z0[j]).sqrt();
        }
    }
    let zm_m: Vec<c64> = (0..n * n).map(|k| zm[k] - id(k / n, k % n) * z).collect();
    let zm_p: Vec<c64> = (0..n * n).map(|k| zm[k] + id(k / n, k % n) * z).collect();
    matmul(&zm_m, &cinv(&zm_p, n), n)
}

fn matmul(a: &[c64], b: &[c64], n: usize) -> Vec<c64> {
    let mut o = vec![czero(); n * n];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                o[i * n + j] += a[i * n + k] * b[k * n + j];
            }
        }
    }
    o
}

fn cinv(a: &[c64], n: usize) -> Vec<c64> {
    let mut m = a.to_vec();
    let mut inv: Vec<c64> = (0..n * n)
        .map(|k| {
            if k / n == k % n {
                c64::new(1.0, 0.0)
            } else {
                czero()
            }
        })
        .collect();
    for c in 0..n {
        let p = (c..n)
            .max_by(|&x, &y| {
                m[x * n + c]
                    .norm()
                    .partial_cmp(&m[y * n + c].norm())
                    .unwrap()
            })
            .unwrap();
        for k in 0..n {
            m.swap(c * n + k, p * n + k);
            inv.swap(c * n + k, p * n + k);
        }
        let d = m[c * n + c];
        for k in 0..n {
            m[c * n + k] /= d;
            inv[c * n + k] /= d;
        }
        for r in 0..n {
            if r != c {
                let f = m[r * n + c];
                for k in 0..n {
                    let (mv, iv) = (m[c * n + k], inv[c * n + k]);
                    m[r * n + k] -= f * mv;
                    inv[r * n + k] -= f * iv;
                }
            }
        }
    }
    inv
}

/// Adaptive solve, then the sweep. `progress` gets a line per step and
/// returns false to stop.
pub fn solve(
    model: &Model,
    setup: &Setup,
    mut progress: impl FnMut(&str) -> bool,
) -> Result<Solution, String> {
    // Second-order elements start from a coarser mesh, as HFSS does.
    // Start at no more than a third of the unknowns allowed, so at least
    // two refinements fit (about 5.5 unknowns per tetrahedron at order 2,
    // 1.2 at order 1).
    let per_tet = if setup.order >= 2 { 5.5 } else { 1.2 };
    let max_tets = ((setup.max_unknowns as f64 / 3.0) / per_tet) as usize;
    let mut mesh = if setup.order >= 2 {
        model.mesh(setup.f_adapt, setup.per_lambda / 2.0, 5.0, max_tets)
    } else {
        model.mesh(setup.f_adapt, setup.per_lambda, 12.0, max_tets)
    };
    // Coarse port faces first: refine the tetrahedra on any port whose
    // mode on the mesh's own triangles is off the fine 2D solve by more
    // than 2%, so the ports start right (their power and Z0 hang on it).
    for _ in 0..4 {
        let asm = assemble_mesh(model, &mesh, setup.f_adapt, setup.order)?;
        let marked = vec![false; mesh.tets.len()];
        let mut edges = Vec::new();
        let mut any = false;
        for pm in asm.ports.iter().filter(|p| p.gap > 0.02) {
            any = true;
            // Each port triangle's longest side, so the face really gets
            // finer.
            for t in &pm.tris {
                let f = t.f;
                let mut best = (ekey(f[0], f[1]), -1.0);
                for (a, b) in TRI_EDGES {
                    let e = ekey(f[a], f[b]);
                    let l = mesh.len2(e);
                    if l > best.1 {
                        best = (e, l);
                    }
                }
                edges.push(best.0);
            }
        }
        let per = asm.n as f64 / mesh.tets.len().max(1) as f64;
        if !any {
            break;
        }
        let before = mesh.clone();
        mesh.refine(&marked, &edges);
        if mesh.tets.len() as f64 * per * 1.7 > setup.max_unknowns as f64 {
            mesh = before;
            break;
        }
        if !progress(&format!(
            "ports: refined to {} tetrahedra (port Z0 on the mesh off by more than 2%)",
            mesh.tets.len()
        )) {
            return Err("stopped".into());
        }
    }
    let mut passes: Vec<Pass> = Vec::new();
    let mut converged = false;
    let mut last_unknowns;
    let mut last_z0: Vec<f64> = Vec::new();
    // The iterative solver's factor; the last pass's is the final mesh's,
    // so the sweep starts with it.
    let mut cache: Option<Precond> = None;
    loop {
        let t0 = std::time::Instant::now();
        let (asm, sols, s) = solve_at(
            model,
            &mesh,
            setup.f_adapt,
            setup.order,
            setup.iterative,
            Some((setup.f_adapt, &mut cache)),
        )?;
        last_unknowns = asm.n;
        let z0: Vec<f64> = asm.ports.iter().map(|p| p.info.z0).collect();
        // Converge on the S-parameters the user gets (renormalised), and
        // on the ports' Z0 to within 0.5% (HFSS's port accuracy).
        let s = match setup.z_ref {
            Some(z) => renormalise(&s, &z0, z),
            None => s,
        };
        let z_moved = last_z0
            .iter()
            .zip(&z0)
            .any(|(a, b)| (a - b).abs() > 0.005 * b);
        last_z0 = z0;
        let delta = passes.last().map(|p| max_delta(&p.s, &s));
        passes.push(Pass {
            tets: mesh.tets.len(),
            unknowns: asm.n,
            delta_s: delta,
            s: s.clone(),
            seconds: t0.elapsed().as_secs_f64(),
        });
        let line = format!(
            "pass {}: {} tetrahedra, {} unknowns, dS {}, {:.1} s",
            passes.len(),
            mesh.tets.len(),
            asm.n,
            delta
                .map(|d| format!("{d:.4}"))
                .unwrap_or_else(|| "-".into()),
            t0.elapsed().as_secs_f64()
        );
        if !progress(&line) {
            return Err("stopped".into());
        }
        if let Some(d) = delta {
            if d < setup.delta_s && !z_moved && passes.len() >= setup.min_passes {
                converged = true;
                break;
            }
        }
        // Refining a share q of the tetrahedra grows the mesh about
        // 1 + 2.3 q (closure included); near the limit refine fewer, so a
        // last pass fits.
        let room = setup.max_unknowns as f64 / asm.n as f64;
        if passes.len() >= setup.max_passes || room < 1.15 {
            break;
        }
        let frac = setup.refine.min((0.92 * room - 1.0) / 2.3).max(0.05);
        let eta = estimate(&mesh, &asm, &sols, setup.f_adapt);
        let mut order: Vec<usize> = (0..eta.len()).collect();
        order.sort_by(|a, b| eta[*b].partial_cmp(&eta[*a]).unwrap());
        let take = ((eta.len() as f64) * frac).ceil() as usize;
        let mut marked = vec![false; eta.len()];
        let mut edges = Vec::new();
        for &i in order.iter().take(take) {
            marked[i] = true;
        }
        // Ports: refine their own worst triangles too, as their 2D solve
        // sets the mode and Z0.
        for pm in &asm.ports {
            let mut o: Vec<usize> = (0..pm.err.len()).collect();
            o.sort_by(|a, b| pm.err[*b].partial_cmp(&pm.err[*a]).unwrap());
            for &i in o
                .iter()
                .take(((o.len() as f64) * setup.refine).ceil() as usize)
            {
                if pm.err[i] > 0.0 {
                    // Split the triangle's own longest side, so the port's
                    // 2D mesh really gets finer.
                    let t = pm.tris[i].f;
                    let mut best = (ekey(t[0], t[1]), -1.0);
                    for (a, b) in TRI_EDGES {
                        let e = ekey(t[a], t[b]);
                        let l = mesh.len2(e);
                        if l > best.1 {
                            best = (e, l);
                        }
                    }
                    edges.push(best.0);
                }
            }
        }
        // The estimator sees sorted tets in the same order as the mesh.
        let before = mesh.clone();
        let per = asm.n as f64 / before.tets.len().max(1) as f64;
        mesh.refine(&marked, &edges);
        // Over the limit: stop on the last solved mesh (the sweep uses it).
        if mesh.tets.len() as f64 * per > setup.max_unknowns as f64 {
            mesh = before;
            break;
        }
    }
    let sweep = if setup.sweep.is_empty() {
        vec![setup.f_adapt]
    } else {
        setup.sweep.clone()
    };
    // One single-precision factor serves the sweep points near its
    // frequency (the iterative solver only); the points that share one are
    // solved together, the adaptive frequency's first (the last pass left
    // its factor).
    let mut solved: Vec<(f64, Vec<c64>, Vec<PortInfo>)> = Vec::new();
    let mut solve_f = |f: f64,
                       solved: &mut Vec<(f64, Vec<c64>, Vec<PortInfo>)>,
                       progress: &mut dyn FnMut(&str) -> bool|
     -> Result<(), String> {
        let (asm, _, s) = solve_at(
            model,
            &mesh,
            f,
            setup.order,
            setup.iterative,
            Some((anchor(f, setup.f_adapt), &mut cache)),
        )?;
        let z0: Vec<f64> = asm.ports.iter().map(|p| p.info.z0).collect();
        let s = match setup.z_ref {
            Some(z) => renormalise(&s, &z0, z),
            None => s,
        };
        solved.push((f, s, asm.ports.iter().map(|p| p.info).collect()));
        solved.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !progress(&format!(
            "sweep: {f:.4} GHz solved ({} so far)",
            solved.len()
        )) {
            return Err("stopped".into());
        }
        Ok(())
    };
    let lo = anchor(0.0, setup.f_adapt);
    let (high, low): (Vec<f64>, Vec<f64>) = sweep.iter().partition(|&&f| f >= lo);
    if setup.interp_tol > 0.0 && sweep.len() > 4 {
        // Interpolating sweep: solve the ends of each band, then the middle
        // of the widest gap, until two in a row land within interp_tol of
        // what the solved points predict.
        for band in [&high, &low] {
            if band.is_empty() {
                continue;
            }
            let (f0, f1) = band
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), &f| (a.min(f), b.max(f)));
            let mut pts = vec![f1, f0, 0.5 * (f0 + f1)];
            pts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            for f in pts {
                solve_f(f, &mut solved, &mut progress)?;
            }
            let mut good = 0;
            while good < 2 && solved.len() < setup.interp_max {
                let inb: Vec<f64> = solved
                    .iter()
                    .map(|p| p.0)
                    .filter(|&f| f >= f0 - 1e-9 && f <= f1 + 1e-9)
                    .collect();
                let Some((ga, gb)) = inb
                    .windows(2)
                    .map(|w| (w[0], w[1]))
                    .max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0)))
                else {
                    break;
                };
                let f = 0.5 * (ga + gb);
                let guess = interp_s(&solved, f);
                solve_f(f, &mut solved, &mut progress)?;
                let got = &solved.iter().find(|p| (p.0 - f).abs() < 1e-12).unwrap().1;
                let err = guess
                    .iter()
                    .zip(got)
                    .map(|(a, b)| (a - b).norm())
                    .fold(0.0, f64::max);
                good = if err < setup.interp_tol { good + 1 } else { 0 };
            }
        }
    } else {
        for f in high.iter().rev().chain(low.iter().rev()) {
            solve_f(*f, &mut solved, &mut progress)?;
        }
    }
    let out_s: Vec<Vec<c64>> = sweep.iter().map(|&f| interp_s(&solved, f)).collect();
    let info: Vec<Vec<PortInfo>> = sweep.iter().map(|&f| interp_info(&solved, f)).collect();
    let solved_f: Vec<f64> = solved.iter().map(|p| p.0).collect();
    Ok(Solution {
        ports: model.ports.iter().map(|p| p.name.clone()).collect(),
        passes,
        converged,
        f: sweep,
        s: out_s,
        info,
        solved: solved_f,
        z_ref: setup.z_ref,
        tets: mesh.tets.len(),
        unknowns: last_unknowns,
    })
}

// ---------------------------------------------------------------- models

const MM: f64 = 1e-3;
const MIL: f64 = 25.4e-6;

/// An air-filled rectangular waveguide `a` x `b` (mm), `len` long (mm),
/// optionally with a dielectric slab of `er` filling `slab` mm in the middle.
pub fn waveguide(a: f64, b: f64, len: f64, slab: Option<(f64, f64)>) -> Model {
    let mut solids = Vec::new();
    if let Some((er, l)) = slab {
        let z0 = (len - l) / 2.0;
        solids.push(Solid {
            shape: Shape::Box {
                lo: [0.0, 0.0, z0 * MM],
                hi: [a * MM, b * MM, (z0 + l) * MM],
            },
            fill: Fill::Dielectric { er, tand: 0.0 },
        });
    }
    let port = |name: &str, high: bool| Port {
        name: name.into(),
        axis: 2,
        high,
        lo: [0.0, 0.0],
        hi: [a * MM, b * MM],
        mode: Mode::Te10,
        deembed: 0.0,
    };
    Model {
        lo: [0.0; 3],
        hi: [a * MM, b * MM, len * MM],
        walls: [Wall::Pec; 6],
        er: 1.0,
        solids,
        sheets: Vec::new(),
        ports: vec![port("P1", false), port("P2", true)],
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Microstrip,
    Stripline,
}

/// A straight line `len` mil long: width `w`, dielectric `h` (microstrip:
/// substrate height; stripline: the full plane-to-plane spacing with the
/// strip in the middle), all in mil. The strip is a zero-thickness sheet.
/// Microstrip has air above to 5h with a PEC lid (as a wave port needs a
/// closed outline) and radiation side walls are not used: the box is
/// shielded, wide enough (w + 10h) that the walls barely touch the field.
pub fn line(kind: LineKind, w: f64, h: f64, er: f64, tand: f64, len: f64) -> Model {
    let width = w + 10.0 * h;
    let (top, strip_z) = match kind {
        LineKind::Microstrip => (6.0 * h, h),
        LineKind::Stripline => (h, h / 2.0),
    };
    let x0 = (width - w) / 2.0;
    let mut solids = vec![Solid {
        shape: Shape::Box {
            lo: [0.0, 0.0, 0.0],
            hi: [width * MIL, len * MIL, h * MIL],
        },
        fill: Fill::Dielectric { er, tand },
    }];
    if kind == LineKind::Microstrip {
        solids.push(Solid {
            shape: Shape::Box {
                lo: [0.0, 0.0, h * MIL],
                hi: [width * MIL, len * MIL, top * MIL],
            },
            fill: Fill::Dielectric { er: 1.0, tand: 0.0 },
        });
    }
    let sheet = Sheet {
        axis: 2,
        at: strip_z * MIL,
        poly: vec![
            [x0 * MIL, 0.0],
            [(x0 + w) * MIL, 0.0],
            [(x0 + w) * MIL, len * MIL],
            [x0 * MIL, len * MIL],
        ],
        holes: Vec::new(),
        disc: None,
        net: 1,
    };
    let port = |name: &str, high: bool| Port {
        name: name.into(),
        axis: 1,
        high,
        lo: [0.0, 0.0],
        hi: [width * MIL, top * MIL],
        mode: Mode::Tem { signal: 1 },
        deembed: 0.0,
    };
    Model {
        lo: [0.0; 3],
        hi: [width * MIL, len * MIL, top * MIL],
        walls: [Wall::Pec; 6],
        er: 1.0,
        solids,
        sheets: vec![sheet],
        ports: vec![port("P1", false), port("P2", true)],
    }
}

/// A via transition from the stack-up, the same geometry the FDTD solver
/// and openEMS get: planes with round antipads, pads, the barrel (and its
/// stub), return vias, and the two traces running out to wave ports at
/// either end, with the reference planes moved back to the via. Copper is
/// perfect conductor; side walls radiate (the board goes on).
pub fn via(stackup: &crate::Stackup, setup: &crate::fdtd::ViaSetup) -> Result<Model, String> {
    let stackup = crate::fdtd::materials(stackup, setup.er, setup.df);
    let m = crate::fdtd::model(&stackup, setup)?;
    let z = |l: usize| -m.st.sheet[l] * MIL;
    let nl = m.nl;
    let h_max = m.st.slabs.iter().map(|s| s.1 - s.0).fold(0.0, f64::max);
    let reach = m.w[0].max(m.w[1]) / 2.0 + 5.0 * h_max;
    let half_w = (2.5 * m.ra).max(setup.return_dist + 2.0 * m.rp).max(reach) * MIL;
    // Wave ports need only clear the via's fringing field (they take the
    // line's own mode), so they sit closer than the FDTD's sources do;
    // a shorter box is a much smaller mesh.
    let xa = m.port_at.min(m.ra.max(m.rp) + reach + 2.0 * h_max) * MIL;
    // Air beyond an outer copper that is not a plane (a trace or a pad
    // sits there, and a wall on it would short them).
    let outer = |l: usize| !m.planes[l];
    let air_top = if outer(0) {
        5.0 * (m.st.slabs[0].1 - m.st.slabs[0].0) * MIL
    } else {
        0.0
    };
    let last = m.st.slabs.len() - 1;
    let air_bot = if outer(nl - 1) {
        5.0 * (m.st.slabs[last].1 - m.st.slabs[last].0) * MIL
    } else {
        0.0
    };
    let lo = [-xa, -half_w, z(nl - 1) - air_bot];
    let hi = [xa, half_w, z(0) + air_top];
    let mut solids = Vec::new();
    // The top slab holds the top copper's thickness too (the stack-up's
    // convention); beside a top trace that is air, not laminate.
    let t_top = if outer(0) {
        stackup
            .copper_layers()
            .next()
            .map(|c| c.thickness.mil())
            .unwrap_or(0.0)
    } else {
        0.0
    };
    for (i, s) in m.st.slabs.iter().enumerate() {
        let top = if i == 0 {
            s.0 + t_top.min(s.1 - s.0)
        } else {
            s.0
        };
        solids.push(Solid {
            shape: Shape::Box {
                lo: [lo[0], lo[1], -s.1 * MIL],
                hi: [hi[0], hi[1], -top * MIL],
            },
            fill: Fill::Dielectric {
                er: s.2,
                tand: if setup.lossy { s.3 } else { 0.0 },
            },
        });
    }
    let sig: Vec<[f64; 2]> = m.sig.iter().map(|c| [c[0] * MIL, c[1] * MIL]).collect();
    // Each signal via its own net (a pair: 1 and 2).
    for (k, c) in sig.iter().enumerate().filter(|_| !m.thru) {
        solids.push(Solid {
            shape: Shape::Cyl {
                axis: 2,
                c: *c,
                r: m.rd * MIL,
                lo: z(m.via_bot),
                hi: z(m.via_top),
            },
            fill: Fill::Pec { net: 1 + k as u32 },
        });
    }
    for c in &m.ret {
        solids.push(Solid {
            shape: Shape::Cyl {
                axis: 2,
                c: [c[0] * MIL, c[1] * MIL],
                r: m.rd * MIL,
                lo: z(nl - 1),
                hi: z(0),
            },
            fill: Fill::Pec { net: 0 },
        });
    }
    let mut sheets = Vec::new();
    let rect = |x0: f64, x1: f64, y0: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    for l in 0..nl {
        if m.planes[l] {
            sheets.push(Sheet {
                axis: 2,
                at: z(l),
                poly: rect(lo[0], hi[0], lo[1], hi[1]),
                holes: if setup.antipad_oval && sig.len() == 2 {
                    vec![(sig[0], sig[1], m.ra * MIL)]
                } else {
                    sig.iter().map(|c| (*c, *c, m.ra * MIL)).collect()
                },
                disc: None,
                net: 0,
            });
        }
    }
    let mut pads = if m.thru {
        Vec::new()
    } else {
        vec![m.via_top, m.via_bot, m.from, m.to]
    };
    pads.sort();
    pads.dedup();
    for &l in &pads {
        for (k, c) in sig.iter().enumerate() {
            sheets.push(Sheet {
                axis: 2,
                at: z(l),
                poly: Vec::new(),
                holes: Vec::new(),
                disc: Some((*c, m.rp * MIL)),
                net: 1 + k as u32,
            });
        }
    }
    for (k, c) in sig.iter().enumerate() {
        let net = 1 + k as u32;
        for (t, &l) in [m.from, m.to].iter().enumerate() {
            let hw = m.w[t] / 2.0 * MIL;
            let (x0, x1) = if t == 0 { (lo[0], c[0]) } else { (c[0], hi[0]) };
            sheets.push(Sheet {
                axis: 2,
                at: z(l),
                poly: rect(x0, x1, c[1] - hw, c[1] + hw),
                holes: Vec::new(),
                disc: None,
                net,
            });
            // The copper's own thickness, below the sheet (where the
            // stack-up puts it: in the slab under an inner or top layer,
            // outside the board under the bottom one).
            let t_cu = stackup
                .copper_layers()
                .nth(l)
                .map(|c| c.thickness.mil())
                .unwrap_or(0.0)
                * MIL;
            if t_cu > 0.0 {
                solids.push(Solid {
                    shape: Shape::Box {
                        lo: [x0, c[1] - hw, z(l) - t_cu],
                        hi: [x1, c[1] + hw, z(l)],
                    },
                    fill: Fill::Pec { net },
                });
            }
        }
    }
    let c = [
        sig.iter().map(|c| c[0]).sum::<f64>() / sig.len() as f64,
        0.0,
    ];
    let mode = if sig.len() == 2 {
        Mode::Diff { plus: 1, minus: 2 }
    } else {
        Mode::Tem { signal: 1 }
    };
    let port = |name: &str, high: bool| Port {
        name: name.into(),
        axis: 0,
        high,
        lo: [lo[1], lo[2]],
        hi: [hi[1], hi[2]],
        mode,
        deembed: if high { xa - c[0] } else { xa + c[0] },
    };
    // Lids over the air are perfect conductor (as in `line`): a radiation
    // wall that close soaks up the traces' fringing field.
    let wall_z = |_air: f64| Wall::Pec;
    Ok(Model {
        lo,
        hi,
        walls: [
            Wall::Pec,
            Wall::Pec,
            Wall::Radiation,
            Wall::Radiation,
            wall_z(air_bot),
            wall_z(air_top),
        ],
        er: 1.0,
        solids,
        sheets,
        ports: vec![port("P1", false), port("P2", true)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refine_keeps_mesh_conforming() {
        let mut m = kuhn(&[0.0, 1.0, 2.0], &[0.0, 1.0], &[0.0, 1.0]);
        let v0: f64 = m
            .tets
            .iter()
            .map(|t| tet_geo(t.map(|i| m.nodes[i as usize])).vol)
            .sum();
        let mut marked = vec![false; m.tets.len()];
        marked[0] = true;
        marked[7] = true;
        m.refine(&marked, &[]);
        m.refine(&[true; 3], &[(0, 1)]);
        let v1: f64 = m
            .tets
            .iter()
            .map(|t| tet_geo(t.map(|i| m.nodes[i as usize])).vol)
            .sum();
        assert!((v0 - v1).abs() < 1e-12);
        // Every interior face is shared by exactly two tets, boundary by one,
        // and no edge has a node at its midpoint (no hanging nodes).
        let mut faces: HashMap<[u32; 3], usize> = HashMap::new();
        for t in &m.tets {
            let mut s = *t;
            s.sort_unstable();
            for skip in 0..4 {
                let f: Vec<u32> = (0..4).filter(|&k| k != skip).map(|k| s[k]).collect();
                *faces.entry([f[0], f[1], f[2]]).or_default() += 1;
            }
        }
        let area: f64 = faces
            .iter()
            .filter(|(_, c)| **c == 1)
            .map(|(f, _)| {
                tri_geo([
                    m.nodes[f[0] as usize],
                    m.nodes[f[1] as usize],
                    m.nodes[f[2] as usize],
                ])
                .1
            })
            .sum();
        assert!((area - 10.0).abs() < 1e-9, "boundary area {area}");
        assert!(faces.values().all(|c| *c <= 2));
    }

    #[test]
    fn empty_waveguide_matches_analytic() {
        // WR-90 at 10 GHz, 20 mm long.
        let m = waveguide(22.86, 10.16, 20.0, None);
        let setup = Setup {
            f_adapt: 10.0,
            max_passes: 6,
            max_unknowns: 50_000,
            z_ref: None,
            ..Setup::default()
        };
        let r = solve(&m, &setup, |l| {
            eprintln!("{l}");
            true
        })
        .unwrap();
        let s = &r.s[0];
        let k0 = 2.0 * std::f64::consts::PI * 10e9 / C0;
        let beta = (k0 * k0 - (std::f64::consts::PI / 22.86e-3).powi(2)).sqrt();
        let want = c64::new((beta * 0.02).cos(), -(beta * 0.02).sin());
        eprintln!("S11 {:?} S21 {:?} want {:?}", s[0], s[2], want);
        assert!(s[0].norm() < 0.05, "S11 {}", s[0].norm());
        assert!((s[2] - want).norm() < 0.05, "S21 {:?}", s[2]);
    }

    #[test]
    fn stripline_impedance() {
        // 50-ohm-ish stripline: w 8 mil in 20 mil Er 4 (Cohn ~ 47 ohm).
        let m = line(LineKind::Stripline, 8.0, 20.0, 4.0, 0.0, 200.0);
        let setup = Setup {
            f_adapt: 5.0,
            max_passes: 10,
            max_unknowns: 50_000,
            z_ref: None,
            ..Setup::default()
        };
        let r = solve(&m, &setup, |l| {
            eprintln!("{l}");
            true
        })
        .unwrap();
        let z = r.info[0][0].z0;
        let zc = crate::impedance::stripline(8.0, 0.0, 20.0, 4.0);
        eprintln!(
            "Z0 fem {z:.2} closed form {zc:.2} eeff {:.3} S11 {:.4} S21 {:.4}",
            r.info[0][0].eeff,
            r.s[0][0].norm(),
            r.s[0][2].norm()
        );
        assert!((r.info[0][0].eeff - 4.0).abs() < 0.05);
        // The exact zero-thickness value is 56.5 ohm; the edge singularity
        // keeps a first-order mesh a little low.
        assert!((z - 56.5).abs() < 0.04 * 56.5, "Z0 {z}");
        assert!(r.s[0][0].norm_sqr() + r.s[0][2].norm_sqr() > 0.98, "power");
        assert!(r.s[0][0].norm() < 0.08);
        assert!((r.s[0][2].norm() - 1.0).abs() < 0.03);
    }

    #[test]
    fn microstrip_modal_port_conserves_power() {
        // Inhomogeneous fill: the modal port keeps a lossless line lossless.
        let m = line(LineKind::Microstrip, 10.0, 5.0, 4.0, 0.0, 200.0);
        let setup = Setup {
            f_adapt: 5.0,
            max_unknowns: 40_000,
            z_ref: None,
            ..Setup::default()
        };
        let r = solve(&m, &setup, |_| true).unwrap();
        let p = r.s[0][0].norm_sqr() + r.s[0][2].norm_sqr();
        assert!((p - 1.0).abs() < 0.005, "power {p}");
        assert!(r.s[0][0].norm() < 0.02, "S11 {}", r.s[0][0].norm());
    }

    #[test]
    fn iterative_solver_matches_direct() {
        let m = line(LineKind::Microstrip, 10.0, 5.0, 4.0, 0.0, 200.0);
        let run = |it: bool| {
            let setup = Setup {
                f_adapt: 5.0,
                sweep: vec![1.0, 5.0],
                max_unknowns: 40_000,
                z_ref: None,
                iterative: Some(it),
                ..Setup::default()
            };
            solve(&m, &setup, |_| true).unwrap()
        };
        let (d, i) = (run(false), run(true));
        assert_eq!(d.unknowns, i.unknowns);
        for (a, b) in d.s.iter().zip(&i.s) {
            for (x, y) in a.iter().zip(b) {
                assert!((*x - *y).norm() < 1e-6, "{x} vs {y}");
            }
        }
    }

    #[test]
    fn sweep_interpolation_is_exact_for_cubics() {
        let f = |x: f64| c64::new(1.0 + x - 0.3 * x * x + 0.02 * x * x * x, 0.5 * x);
        let info = PortInfo {
            z0: 50.0,
            eeff: 1.0,
            beta: 1.0,
        };
        let solved: Vec<(f64, Vec<c64>, Vec<PortInfo>)> = [0.5, 1.0, 2.5, 4.0, 7.0, 9.0]
            .iter()
            .map(|&x| (x, vec![f(x)], vec![info]))
            .collect();
        for t in [0.7, 3.3, 5.0, 8.9] {
            let g = interp_s(&solved, t)[0];
            assert!((g - f(t)).norm() < 1e-9, "{t}: {g} vs {}", f(t));
        }
    }
}
