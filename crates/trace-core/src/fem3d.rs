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
    pub holes: Vec<([f64; 2], f64)>,
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
            max_unknowns: 400_000,
            z_ref: Some(50.0),
            order: 2,
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
            if g > 3.0 * lft.min(rgt) * 1.0001 {
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

    /// The starting mesh.
    pub fn mesh(&self, f_ghz: f64, per_lambda: f64, per_axis: f64) -> Mesh {
        let lambda = C0 / (f_ghz * 1e9) / self.er_max().sqrt();
        let h = lambda / per_lambda;
        let tol = self.tol();
        let mut br: [Vec<f64>; 3] = Default::default();
        for a in 0..3 {
            br[a].push(self.lo[a]);
            br[a].push(self.hi[a]);
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
                        br[a].push(clamp(a, c[k]));
                    }
                }
            }
        }
        for s in &self.sheets {
            br[s.axis].push(s.at);
            let (u, v) = others(s.axis);
            for p in &s.poly {
                br[u].push(clamp(u, p[0]));
                br[v].push(clamp(v, p[1]));
            }
            for (c, r) in s.holes.iter().chain(s.disc.iter()) {
                br[u].push(clamp(u, c[0]));
                br[v].push(clamp(v, c[1]));
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
        // Also at least a dozen cells across every axis, so small cross
        // sections start with some resolution.
        let g: Vec<Vec<f64>> = (0..3)
            .map(|a| {
                axis_lines(
                    br[a].clone(),
                    h.min((self.hi[a] - self.lo[a]) / per_axis),
                    tol,
                )
            })
            .collect();
        kuhn(&g[0], &g[1], &g[2])
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
            .all(|(c, r)| (q[0] - c[0]).hypot(q[1] - c[1]) >= r - tol)
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
        is_pec_tet.push(pec.is_some());
    }
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
    let on_conductor = |pts: &[[f64; 3]], m: [f64; 3]| -> bool {
        for s in &model.sheets {
            if pts.iter().all(|p| (p[s.axis] - s.at).abs() < tol) && on_sheet(s, m, tol) {
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
            if is_pec_tet[ti] {
                *e = true;
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
                } else if model.walls[2 * a + side] == Wall::Radiation && !is_pec_tet[bf.tet] {
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
    signal: u32,
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
    for (i, p) in fnodes.iter().enumerate() {
        let p = *p;
        match model.conductor_at(p, tol, 2 * pt.axis + pt.high as usize) {
            Some(n) if n == signal => fixed[i] = Some(1.0),
            Some(_) => fixed[i] = Some(0.0),
            // The rim follows the wall beside it: grounded on a PEC wall
            // (conductor_at above), free beside a radiation wall.
            None => {}
        }
    }
    if !fixed.contains(&Some(1.0)) {
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
        Mode::Tem { signal } => {
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
            let eeff = f.c / f.c0;
            let z0 = ETA0 / (f.c * f.c0).sqrt();
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
) -> Result<(Assembled, Vec<Vec<c64>>, Vec<c64>), String> {
    let asm = assemble_mesh(model, mesh, f_ghz, order)?;
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
    let mut trip: Vec<Triplet<usize, usize, c64>> =
        Vec::with_capacity(asm.tets.len() * nb * nb / 2);
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
    let y = sym_solve(n, trip, &cols)?;
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

/// Solves the complex symmetric system (`trip`, both triangles given) for
/// each column of `rhs`. It is factorised as the real symmetric indefinite
/// system [[Re, Im], [Im, −Re]] (unknowns Re x and −Im x side by side),
/// with a minimum-degree ordering and supernodal Bunch-Kaufman LBLᵀ: about
/// half the memory of a general LU with column ordering on FEM matrices.
fn sym_solve(
    n: usize,
    trip: Vec<Triplet<usize, usize, c64>>,
    rhs: &faer::Mat<c64>,
) -> Result<Vec<Vec<c64>>, String> {
    use faer::dyn_stack::{MemBuffer, MemStack, StackReq};
    use faer::sparse::linalg::cholesky::{
        factorize_symbolic_cholesky, CholeskySymbolicParams, SymmetricOrdering,
    };
    let mut rt: Vec<Triplet<usize, usize, f64>> = Vec::with_capacity(trip.len() * 2);
    for t in &trip {
        let (i, j, v) = (t.row, t.col, t.val);
        if i < j {
            continue;
        }
        if i > j {
            rt.push(Triplet::new(2 * i, 2 * j, v.re));
            rt.push(Triplet::new(2 * i, 2 * j + 1, v.im));
            rt.push(Triplet::new(2 * i + 1, 2 * j, v.im));
            rt.push(Triplet::new(2 * i + 1, 2 * j + 1, -v.re));
        } else {
            rt.push(Triplet::new(2 * i, 2 * i, v.re));
            rt.push(Triplet::new(2 * i + 1, 2 * i, v.im));
            rt.push(Triplet::new(2 * i + 1, 2 * i + 1, -v.re));
        }
    }
    drop(trip);
    let m = 2 * n;
    let a = SparseColMat::<usize, f64>::try_new_from_triplets(m, m, &rt)
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
    let k = rhs.ncols();
    let mut mem = MemBuffer::try_new(StackReq::any_of(&[
        sym.factorize_numeric_intranode_lblt_scratch::<f64>(par, Default::default()),
        sym.solve_in_place_scratch::<f64>(k, par),
    ]))
    .map_err(|_| "out of memory for the factorisation".to_string())?;
    let stack = MemStack::new(&mut mem);
    let mut lv = Vec::new();
    lv.try_reserve_exact(sym.len_val()).map_err(|_| {
        format!(
            "out of memory: the factor needs {:.1} GB",
            sym.len_val() as f64 * 8e-9
        )
    })?;
    lv.resize(sym.len_val(), 0.0f64);
    let mut sub = vec![0.0f64; m];
    let mut pf = vec![0usize; m];
    let mut pi = vec![0usize; m];
    let lblt = sym.factorize_numeric_intranode_lblt(
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
    let mut x = faer::Mat::<f64>::zeros(m, k);
    for c in 0..k {
        for r in 0..n {
            x[(2 * r, c)] = rhs[(r, c)].re;
            x[(2 * r + 1, c)] = rhs[(r, c)].im;
        }
    }
    lblt.solve_in_place_with_conj(faer::Conj::No, x.as_mut(), par, stack);
    Ok((0..k)
        .map(|c| {
            (0..n)
                .map(|r| c64::new(x[(2 * r, c)], -x[(2 * r + 1, c)]))
                .collect()
        })
        .collect())
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
    let mut mesh = if setup.order >= 2 {
        model.mesh(setup.f_adapt, setup.per_lambda / 2.0, 5.0)
    } else {
        model.mesh(setup.f_adapt, setup.per_lambda, 12.0)
    };
    let mut passes: Vec<Pass> = Vec::new();
    let mut converged = false;
    let mut last_unknowns;
    let mut last_z0: Vec<f64> = Vec::new();
    loop {
        let t0 = std::time::Instant::now();
        let (asm, sols, s) = solve_at(model, &mesh, setup.f_adapt, setup.order)?;
        last_unknowns = asm.n;
        let z0: Vec<f64> = asm.ports.iter().map(|p| p.info.z0).collect();
        // Converge on the S-parameters the user gets (renormalised), and
        // on the ports' Z0 to within 1% (HFSS's port accuracy).
        let s = match setup.z_ref {
            Some(z) => renormalise(&s, &z0, z),
            None => s,
        };
        let z_moved = last_z0
            .iter()
            .zip(&z0)
            .any(|(a, b)| (a - b).abs() > 0.01 * b);
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
        if passes.len() >= setup.max_passes || asm.n * 8 / 5 > setup.max_unknowns {
            break;
        }
        let eta = estimate(&mesh, &asm, &sols, setup.f_adapt);
        let mut order: Vec<usize> = (0..eta.len()).collect();
        order.sort_by(|a, b| eta[*b].partial_cmp(&eta[*a]).unwrap());
        let take = ((eta.len() as f64) * setup.refine).ceil() as usize;
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
        mesh.refine(&marked, &edges);
    }
    let sweep = if setup.sweep.is_empty() {
        vec![setup.f_adapt]
    } else {
        setup.sweep.clone()
    };
    let mut out_s = Vec::new();
    let mut info = Vec::new();
    for (k, &f) in sweep.iter().enumerate() {
        let (asm, _, s) = solve_at(model, &mesh, f, setup.order)?;
        let z0: Vec<f64> = asm.ports.iter().map(|p| p.info.z0).collect();
        let s = match setup.z_ref {
            Some(z) => renormalise(&s, &z0, z),
            None => s,
        };
        out_s.push(s);
        info.push(asm.ports.iter().map(|p| p.info).collect());
        if !progress(&format!("sweep {}/{}: {f} GHz", k + 1, sweep.len())) {
            return Err("stopped".into());
        }
    }
    Ok(Solution {
        ports: model.ports.iter().map(|p| p.name.clone()).collect(),
        passes,
        converged,
        f: sweep,
        s: out_s,
        info,
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
    if setup.pair.is_some() {
        return Err("the 3D FEM via model is single-ended for now".into());
    }
    let stackup = crate::fdtd::materials(stackup, setup.er, setup.df);
    let m = crate::fdtd::model(&stackup, setup)?;
    let z = |l: usize| -m.st.sheet[l] * MIL;
    let nl = m.nl;
    let h_max = m.st.slabs.iter().map(|s| s.1 - s.0).fold(0.0, f64::max);
    let reach = m.w[0].max(m.w[1]) / 2.0 + 5.0 * h_max;
    let half_w = (2.5 * m.ra).max(setup.return_dist + 2.0 * m.rp).max(reach) * MIL;
    let xa = m.port_at * MIL;
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
    for s in &m.st.slabs {
        solids.push(Solid {
            shape: Shape::Box {
                lo: [lo[0], lo[1], -s.1 * MIL],
                hi: [hi[0], hi[1], -s.0 * MIL],
            },
            fill: Fill::Dielectric {
                er: s.2,
                tand: if setup.lossy { s.3 } else { 0.0 },
            },
        });
    }
    let sig: Vec<[f64; 2]> = m.sig.iter().map(|c| [c[0] * MIL, c[1] * MIL]).collect();
    for c in sig.iter().filter(|_| !m.thru) {
        solids.push(Solid {
            shape: Shape::Cyl {
                axis: 2,
                c: *c,
                r: m.rd * MIL,
                lo: z(m.via_bot),
                hi: z(m.via_top),
            },
            fill: Fill::Pec { net: 1 },
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
                holes: sig.iter().map(|c| (*c, m.ra * MIL)).collect(),
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
        for c in &sig {
            sheets.push(Sheet {
                axis: 2,
                at: z(l),
                poly: Vec::new(),
                holes: Vec::new(),
                disc: Some((*c, m.rp * MIL)),
                net: 1,
            });
        }
    }
    let c = sig[0];
    for (t, &l) in [m.from, m.to].iter().enumerate() {
        let hw = m.w[t] / 2.0 * MIL;
        let (x0, x1) = if t == 0 { (lo[0], c[0]) } else { (c[0], hi[0]) };
        sheets.push(Sheet {
            axis: 2,
            at: z(l),
            poly: rect(x0, x1, c[1] - hw, c[1] + hw),
            holes: Vec::new(),
            disc: None,
            net: 1,
        });
    }
    let port = |name: &str, high: bool| Port {
        name: name.into(),
        axis: 0,
        high,
        lo: [lo[1], lo[2]],
        hi: [hi[1], hi[2]],
        mode: Mode::Tem { signal: 1 },
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
}
