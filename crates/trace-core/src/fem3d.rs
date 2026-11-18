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

#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity, clippy::map_entry)]

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
    pub fn mesh(&self, f_ghz: f64, per_lambda: f64) -> Mesh {
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
            for (c, r) in &s.holes {
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
            .map(|a| axis_lines(br[a].clone(), h.min((self.hi[a] - self.lo[a]) / 12.0), tol))
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

const KA: f64 = 0.585_410_196_624_968_5;
const KB: f64 = 0.138_196_601_125_010_5;
const QUAD4: [[f64; 4]; 4] = [
    [KA, KB, KB, KB],
    [KB, KA, KB, KB],
    [KB, KB, KA, KB],
    [KB, KB, KB, KA],
];

struct Assembled {
    /// Each tet's node indices, sorted (so local edges point up in index).
    tets: Vec<[u32; 4]>,
    eps: Vec<c64>,
    /// Global edge per local edge, None where the edge is PEC.
    dof: Vec<[Option<u32>; 6]>,
    n: usize,
    ports: Vec<PortMesh>,
    /// Radiation faces: (tet, local face nodes).
    rad: Vec<([u32; 3], c64)>,
    edge_dof: HashMap<(u32, u32), u32>,
}

struct PortMesh {
    tris: Vec<[u32; 3]>,
    /// The mode's tangential field at each triangle's three edge midpoints.
    e: Vec<[[f64; 3]; 3]>,
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
/// Midpoint rule (exact to degree 2): barycentrics of the edge midpoints.
const TRI_Q: [[f64; 3]; 3] = [[0.5, 0.5, 0.0], [0.5, 0.0, 0.5], [0.0, 0.5, 0.5]];

fn assemble_mesh(model: &Model, mesh: &Mesh, f_ghz: f64) -> Result<Assembled, String> {
    let tol = model.tol();
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let mut tets = Vec::with_capacity(mesh.tets.len());
    let mut eps = Vec::with_capacity(mesh.tets.len());
    let mut pec_edge: HashMap<(u32, u32), bool> = HashMap::new();
    let mut is_pec_tet = Vec::with_capacity(mesh.tets.len());
    for t in &mesh.tets {
        let mut s = *t;
        s.sort_unstable();
        let c = scale(
            add(
                add(mesh.nodes[s[0] as usize], mesh.nodes[s[1] as usize]),
                add(mesh.nodes[s[2] as usize], mesh.nodes[s[3] as usize]),
            ),
            0.25,
        );
        let (e, pec) = model.at(c);
        tets.push(s);
        eps.push(e);
        is_pec_tet.push(pec.is_some());
    }
    // PEC edges: in conductor solids, on sheets, on PEC walls.
    for (ti, s) in tets.iter().enumerate() {
        for (a, b) in LOCAL_EDGES {
            let e = (s[a], s[b]);
            let entry = pec_edge.entry(e).or_insert(false);
            if is_pec_tet[ti] {
                *entry = true;
            }
        }
    }
    for (e, v) in pec_edge.iter_mut() {
        if *v {
            continue;
        }
        let p = mesh.nodes[e.0 as usize];
        let q = mesh.nodes[e.1 as usize];
        let m = scale(add(p, q), 0.5);
        for s in &model.sheets {
            if (p[s.axis] - s.at).abs() < tol
                && (q[s.axis] - s.at).abs() < tol
                && on_sheet(s, m, tol)
            {
                *v = true;
                break;
            }
        }
        if *v {
            continue;
        }
        for a in 0..3 {
            for (side, b) in [(0, model.lo[a]), (1, model.hi[a])] {
                if (p[a] - b).abs() < tol && (q[a] - b).abs() < tol {
                    let in_port = model.ports.iter().any(|pt| {
                        pt.axis == a && pt.high == (side == 1) && {
                            let (u, w) = others(a);
                            m[u] > pt.lo[0] + tol
                                && m[u] < pt.hi[0] - tol
                                && m[w] > pt.lo[1] + tol
                                && m[w] < pt.hi[1] - tol
                        }
                    });
                    if model.walls[2 * a + side] == Wall::Pec && !in_port {
                        *v = true;
                    }
                }
            }
        }
    }
    let mut edge_dof: HashMap<(u32, u32), u32> = HashMap::new();
    let mut dof = Vec::with_capacity(tets.len());
    for s in &tets {
        let mut d = [None; 6];
        for (k, (a, b)) in LOCAL_EDGES.iter().enumerate() {
            let e = (s[*a], s[*b]);
            if !pec_edge[&e] {
                let n = edge_dof.len() as u32;
                d[k] = Some(*edge_dof.entry(e).or_insert(n));
            }
        }
        dof.push(d);
    }
    let n = edge_dof.len();
    // Boundary faces.
    let mut faces: HashMap<[u32; 3], (usize, u8)> = HashMap::new();
    for (ti, s) in tets.iter().enumerate() {
        for skip in 0..4 {
            let f: Vec<u32> = (0..4).filter(|&k| k != skip).map(|k| s[k]).collect();
            let key = [f[0], f[1], f[2]];
            let ent = faces.entry(key).or_insert((ti, 0));
            ent.1 += 1;
        }
    }
    let mut port_tris: Vec<Vec<([u32; 3], usize)>> = vec![Vec::new(); model.ports.len()];
    let mut rad = Vec::new();
    for (f, (ti, cnt)) in &faces {
        if *cnt != 1 {
            continue;
        }
        let p = [
            mesh.nodes[f[0] as usize],
            mesh.nodes[f[1] as usize],
            mesh.nodes[f[2] as usize],
        ];
        let c = scale(add(add(p[0], p[1]), p[2]), 1.0 / 3.0);
        let mut done = false;
        for a in 0..3 {
            for (side, b) in [(0, model.lo[a]), (1, model.hi[a])] {
                if done || !p.iter().all(|q| (q[a] - b).abs() < tol) {
                    continue;
                }
                done = true;
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
                    port_tris[pi].push((*f, *ti));
                } else if model.walls[2 * a + side] == Wall::Radiation && !is_pec_tet[*ti] {
                    rad.push((*f, k0 * eps[*ti].re.sqrt() * c64::new(1.0, 0.0)));
                }
            }
        }
    }
    let mut ports = Vec::new();
    for (pi, pt) in model.ports.iter().enumerate() {
        if port_tris[pi].is_empty() {
            return Err(format!("port {} has no mesh faces", pt.name));
        }
        ports.push(port_mode(model, mesh, pt, &port_tris[pi], &eps, k0, tol)?);
    }
    Ok(Assembled {
        tets,
        eps,
        dof,
        n,
        ports,
        rad,
        edge_dof,
    })
}

/// Solve the port's mode on its triangles.
fn port_mode(
    model: &Model,
    mesh: &Mesh,
    pt: &Port,
    tris: &[([u32; 3], usize)],
    eps: &[c64],
    k0: f64,
    tol: f64,
) -> Result<PortMesh, String> {
    let (u, w) = others(pt.axis);
    let tri_list: Vec<[u32; 3]> = tris.iter().map(|t| t.0).collect();
    let tri_eps: Vec<f64> = tris.iter().map(|t| eps[t.1].re).collect();
    match pt.mode {
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
            let mut ee = 0.0;
            for t in &tri_list {
                let p = [
                    mesh.nodes[t[0] as usize],
                    mesh.nodes[t[1] as usize],
                    mesh.nodes[t[2] as usize],
                ];
                let (_, area, _) = tri_geo(p);
                let mut v = [[0.0; 3]; 3];
                for (q, l) in TRI_Q.iter().enumerate() {
                    let x = add(add(scale(p[0], l[0]), scale(p[1], l[1])), scale(p[2], l[2]));
                    let s = (kc * (x[along] - w0)).sin();
                    v[q][dir] = s;
                    ee += area / 3.0 * s * s;
                }
                e.push(v);
            }
            Ok(PortMesh {
                tris: tri_list,
                e,
                info: PortInfo {
                    z0: zte,
                    eeff: (beta / k0).powi(2),
                    beta,
                },
                norm: (ee / zte).sqrt(),
                ee,
                err: vec![0.0; tris.len()],
            })
        }
        Mode::Tem { signal } => {
            // The 2D solve runs on the port's triangles each cut into
            // SUB x SUB, so Z0 and the mode come out finer than the 3D mesh.
            let run = |sub_n: usize| -> Result<(f64, f64, Vec<[f64; 3]>), String> {
                let q = 1.0 / (tol * 10.0);
                let mut fidx: HashMap<[i64; 3], usize> = HashMap::new();
                let mut fnodes: Vec<[f64; 3]> = Vec::new();
                let mut ftris: Vec<([usize; 3], usize)> = Vec::new();
                for (ti, t) in tri_list.iter().enumerate() {
                    let p = [
                        mesh.nodes[t[0] as usize],
                        mesh.nodes[t[1] as usize],
                        mesh.nodes[t[2] as usize],
                    ];
                    let mut node = |i: usize, j: usize| -> usize {
                        let (a, b) = (i as f64 / sub_n as f64, j as f64 / sub_n as f64);
                        let x = add(
                            add(scale(p[0], 1.0 - a - b), scale(p[1], a)),
                            scale(p[2], b),
                        );
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
                    for i in 0..sub_n {
                        for j in 0..sub_n - i {
                            let (n0, n1, n2) = (node(i, j), node(i + 1, j), node(i, j + 1));
                            ftris.push(([n0, n1, n2], ti));
                            if i + j + 1 < sub_n {
                                let n3 = node(i + 1, j + 1);
                                ftris.push(([n1, n3, n2], ti));
                            }
                        }
                    }
                }
                let nn = fnodes.len();
                let mut fixed: Vec<Option<f64>> = vec![None; nn];
                for (i, p) in fnodes.iter().enumerate() {
                    let p = *p;
                    let on_rim = (p[u] - pt.lo[0]).abs() < tol
                        || (p[u] - pt.hi[0]).abs() < tol
                        || (p[w] - pt.lo[1]).abs() < tol
                        || (p[w] - pt.hi[1]).abs() < tol;
                    match model.conductor_at(p, tol, 2 * pt.axis + pt.high as usize) {
                        Some(n) if n == signal => fixed[i] = Some(1.0),
                        Some(_) => fixed[i] = Some(0.0),
                        None if on_rim => fixed[i] = Some(0.0),
                        None => {}
                    }
                }
                if !fixed.contains(&Some(1.0)) {
                    return Err::<(f64, f64, Vec<[f64; 3]>), String>(format!(
                        "port {}: the signal conductor does not cross the port",
                        pt.name
                    ));
                }
                let fgeo: Vec<(f64, [[f64; 3]; 3])> = ftris
                    .iter()
                    .map(|(t, _)| {
                        let (_, area, g) = tri_geo([fnodes[t[0]], fnodes[t[1]], fnodes[t[2]]]);
                        (area, g)
                    })
                    .collect();
                let free: Vec<Option<usize>> = {
                    let mut c = 0;
                    fixed
                        .iter()
                        .map(|f| {
                            if f.is_none() {
                                c += 1;
                                Some(c - 1)
                            } else {
                                None
                            }
                        })
                        .collect()
                };
                let nf = free.iter().filter(|f| f.is_some()).count();
                let solve = |with_eps: bool| -> (Vec<f64>, f64) {
                    let mut kt: Vec<Triplet<usize, usize, f64>> = Vec::new();
                    let mut rhs = vec![0.0; nf];
                    for (k, (t, parent)) in ftris.iter().enumerate() {
                        let (area, g) = fgeo[k];
                        let e = if with_eps { tri_eps[*parent] } else { 1.0 };
                        for i in 0..3 {
                            let Some(fi) = free[t[i]] else { continue };
                            for j in 0..3 {
                                let kk = e * area * dot(g[i], g[j]);
                                match free[t[j]] {
                                    Some(fj) => kt.push(Triplet::new(fi, fj, kk)),
                                    None => rhs[fi] -= kk * fixed[t[j]].unwrap(),
                                }
                            }
                        }
                    }
                    let mut phi: Vec<f64> = fixed.iter().map(|f| f.unwrap_or(0.0)).collect();
                    if nf > 0 {
                        let a =
                            SparseColMat::<usize, f64>::try_new_from_triplets(nf, nf, &kt).unwrap();
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
                    for (k, (t, parent)) in ftris.iter().enumerate() {
                        let (area, g) = fgeo[k];
                        let e = if with_eps { tri_eps[*parent] } else { 1.0 };
                        let mut gr = [0.0; 3];
                        for i in 0..3 {
                            gr = add(gr, scale(g[i], phi[t[i]]));
                        }
                        c += e * area * dot(gr, gr);
                    }
                    (phi, c)
                };
                let (phi, c) = solve(true);
                let (_, c0) = solve(false);
                let mut sum = vec![[0.0; 3]; tri_list.len()];
                for (k, (t, parent)) in ftris.iter().enumerate() {
                    let (area, g) = fgeo[k];
                    for i in 0..3 {
                        sum[*parent] = add(sum[*parent], scale(g[i], -phi[t[i]] * area));
                    }
                }
                Ok((c, c0, sum))
            };
            // Z0 and Er,eff from the fine solve; the mode's shape from the
            // 3D mesh's own triangles, so the 3D field can match it exactly
            // (a finer shape would leak power as an unmatched remainder).
            let fine = if tri_list.len() < 3000 { 4 } else { 2 };
            let (c, c0, _) = run(fine)?;
            let (_, _, sum) = run(1)?;
            let eeff = c / c0;
            let z0 = ETA0 / (c * c0).sqrt();
            let beta = k0 * eeff.sqrt();
            let mut e = Vec::new();
            let mut ee = 0.0;
            for (ti, t) in tri_list.iter().enumerate() {
                let p = [
                    mesh.nodes[t[0] as usize],
                    mesh.nodes[t[1] as usize],
                    mesh.nodes[t[2] as usize],
                ];
                let (_, area, _) = tri_geo(p);
                let gr = scale(sum[ti], 1.0 / area);
                ee += area * dot(gr, gr);
                e.push([gr; 3]);
            }
            // 2D error: jumps of the normal flux across triangle sides.
            let mut sides: HashMap<(u32, u32), Vec<(usize, f64)>> = HashMap::new();
            for (ti, t) in tri_list.iter().enumerate() {
                let p = [
                    mesh.nodes[t[0] as usize],
                    mesh.nodes[t[1] as usize],
                    mesh.nodes[t[2] as usize],
                ];
                let d = scale(e[ti][0], tri_eps[ti]);
                for (a, b) in TRI_EDGES {
                    let k = ekey(t[a], t[b]);
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
            let mut err = vec![0.0; tri_list.len()];
            for v in sides.values() {
                if v.len() == 2 {
                    // The two normals are opposite as the triangles share
                    // one orientation of the face plane, so a sum is a jump.
                    let j = (v[0].1 + v[1].1).powi(2);
                    err[v[0].0] += j;
                    err[v[1].0] += j;
                }
            }
            Ok(PortMesh {
                tris: tri_list,
                e,
                info: PortInfo { z0, eeff, beta },
                norm: (1.0 / z0).sqrt(),
                ee,
                err,
            })
        }
    }
}

fn czero() -> c64 {
    c64::new(0.0, 0.0)
}

/// Solve at one frequency; per excitation, the edge values, and the S matrix.
fn solve_at(
    model: &Model,
    mesh: &Mesh,
    f_ghz: f64,
) -> Result<(Assembled, Vec<Vec<c64>>, Vec<c64>), String> {
    let asm = assemble_mesh(model, mesh, f_ghz)?;
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let n = asm.n;
    if n == 0 {
        return Err("no unknowns".into());
    }
    let mut trip: Vec<Triplet<usize, usize, c64>> = Vec::with_capacity(asm.tets.len() * 36);
    for (ti, s) in asm.tets.iter().enumerate() {
        let p = [
            mesh.nodes[s[0] as usize],
            mesh.nodes[s[1] as usize],
            mesh.nodes[s[2] as usize],
            mesh.nodes[s[3] as usize],
        ];
        let geo = tet_geo(p);
        let mut curl = [[0.0; 3]; 6];
        for (k, (i, j)) in LOCAL_EDGES.iter().enumerate() {
            curl[k] = scale(cross(geo.g[*i], geo.g[*j]), 2.0);
        }
        let mut mass = [[0.0; 6]; 6];
        for l in &QUAD4 {
            let nv: Vec<[f64; 3]> = LOCAL_EDGES
                .iter()
                .map(|(i, j)| whitney(&geo.g, l, *i, *j))
                .collect();
            for a in 0..6 {
                for b in 0..6 {
                    mass[a][b] += geo.vol / 4.0 * dot(nv[a], nv[b]);
                }
            }
        }
        let ke = asm.eps[ti] * (k0 * k0);
        for a in 0..6 {
            let Some(da) = asm.dof[ti][a] else { continue };
            for b in 0..6 {
                let Some(db) = asm.dof[ti][b] else { continue };
                let v = c64::new(geo.vol * dot(curl[a], curl[b]), 0.0) - ke * mass[a][b];
                trip.push(Triplet::new(da as usize, db as usize, v));
            }
        }
    }
    let face_mass = |p: [[f64; 3]; 3]| -> (f64, [[f64; 3]; 3], [[[f64; 3]; 3]; 3]) {
        let (_, area, g) = tri_geo(p);
        // N at each quadrature point for each of the three edges.
        let mut nq = [[[0.0; 3]; 3]; 3];
        for (q, l) in TRI_Q.iter().enumerate() {
            for (k, (i, j)) in TRI_EDGES.iter().enumerate() {
                nq[q][k] = whitney(&g, l, *i, *j);
            }
        }
        (area, [[0.0; 3]; 3], nq)
    };
    let add_face = |trip: &mut Vec<Triplet<usize, usize, c64>>,
                    f: [u32; 3],
                    coef: c64,
                    edge_dof: &HashMap<(u32, u32), u32>| {
        let p = [
            mesh.nodes[f[0] as usize],
            mesh.nodes[f[1] as usize],
            mesh.nodes[f[2] as usize],
        ];
        let (area, _, nq) = face_mass(p);
        let d: Vec<Option<u32>> = TRI_EDGES
            .iter()
            .map(|(i, j)| edge_dof.get(&(f[*i], f[*j])).copied())
            .collect();
        for a in 0..3 {
            let Some(da) = d[a] else { continue };
            for b in 0..3 {
                let Some(db) = d[b] else { continue };
                let mut m = 0.0;
                for q in 0..3 {
                    m += area / 3.0 * dot(nq[q][a], nq[q][b]);
                }
                trip.push(Triplet::new(da as usize, db as usize, coef * m));
            }
        }
    };
    for (f, k) in &asm.rad {
        add_face(&mut trip, *f, c64::new(0.0, k.re), &asm.edge_dof);
    }
    let np = asm.ports.len();
    let mut rhs = faer::Mat::<c64>::zeros(n, np);
    for (pi, pm) in asm.ports.iter().enumerate() {
        let jb = c64::new(0.0, pm.info.beta);
        for (ti, f) in pm.tris.iter().enumerate() {
            add_face(&mut trip, *f, jb, &asm.edge_dof);
            let p = [
                mesh.nodes[f[0] as usize],
                mesh.nodes[f[1] as usize],
                mesh.nodes[f[2] as usize],
            ];
            let (area, _, nq) = face_mass(p);
            for (k, (i, j)) in TRI_EDGES.iter().enumerate() {
                if let Some(d) = asm.edge_dof.get(&(f[*i], f[*j])) {
                    let mut b = 0.0;
                    for q in 0..3 {
                        b += area / 3.0 * dot(nq[q][k], pm.e[ti][q]);
                    }
                    rhs[(*d as usize, pi)] += jb * 2.0 * b;
                }
            }
        }
    }
    let a = SparseColMat::<usize, c64>::try_new_from_triplets(n, n, &trip)
        .map_err(|e| format!("{e:?}"))?;
    let lu = a
        .sp_lu()
        .map_err(|e| format!("factorisation failed: {e:?}"))?;
    let x = lu.solve(&rhs);
    let sols: Vec<Vec<c64>> = (0..np)
        .map(|c| (0..n).map(|r| x[(r, c)]).collect())
        .collect();
    // Mode amplitudes at each port for each excitation.
    let mut s = vec![czero(); np * np];
    for (pe, sol) in sols.iter().enumerate() {
        for (pq, pm) in asm.ports.iter().enumerate() {
            let mut g = czero();
            for (ti, f) in pm.tris.iter().enumerate() {
                let p = [
                    mesh.nodes[f[0] as usize],
                    mesh.nodes[f[1] as usize],
                    mesh.nodes[f[2] as usize],
                ];
                let (area, _, nq) = face_mass(p);
                for (k, (i, j)) in TRI_EDGES.iter().enumerate() {
                    if let Some(d) = asm.edge_dof.get(&(f[*i], f[*j])) {
                        let mut b = 0.0;
                        for q in 0..3 {
                            b += area / 3.0 * dot(nq[q][k], pm.e[ti][q]);
                        }
                        g += sol[*d as usize] * b;
                    }
                }
            }
            g /= pm.ee;
            if pq == pe {
                g -= c64::new(1.0, 0.0);
            }
            let mut v = g * (pm.norm / asm.ports[pe].norm);
            // De-embed: move both reference planes inwards.
            let shift = model.ports[pq].deembed * pm.info.beta
                + model.ports[pe].deembed * asm.ports[pe].info.beta;
            v *= c64::new(shift.cos(), shift.sin());
            s[pq * np + pe] = v;
        }
    }
    Ok((asm, sols, s))
}

/// Per-tetrahedron error indicator from the solutions.
fn estimate(
    model: &Model,
    mesh: &Mesh,
    asm: &Assembled,
    sols: &[Vec<c64>],
    f_ghz: f64,
) -> Vec<f64> {
    let k0 = 2.0 * std::f64::consts::PI * f_ghz * 1e9 / C0;
    let nt = asm.tets.len();
    let mut eta = vec![0.0; nt];
    let geos: Vec<Geo> = asm
        .tets
        .iter()
        .map(|s| {
            tet_geo([
                mesh.nodes[s[0] as usize],
                mesh.nodes[s[1] as usize],
                mesh.nodes[s[2] as usize],
                mesh.nodes[s[3] as usize],
            ])
        })
        .collect();
    let mut faces: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
    for (ti, s) in asm.tets.iter().enumerate() {
        for skip in 0..4 {
            let f: Vec<u32> = (0..4).filter(|&k| k != skip).map(|k| s[k]).collect();
            faces.entry([f[0], f[1], f[2]]).or_default().push(ti);
        }
    }
    let field = |ti: usize, sol: &[c64], l: [f64; 4]| -> ([c64; 3], [c64; 3]) {
        let g = &geos[ti];
        let mut e = [czero(); 3];
        let mut c = [czero(); 3];
        for (k, (i, j)) in LOCAL_EDGES.iter().enumerate() {
            let Some(d) = asm.dof[ti][k] else { continue };
            let x = sol[d as usize];
            let w = whitney(&g.g, &l, *i, *j);
            let cu = scale(cross(g.g[*i], g.g[*j]), 2.0);
            for a in 0..3 {
                e[a] += x * w[a];
                c[a] += x * cu[a];
            }
        }
        (e, c)
    };
    let _ = model;
    for sol in sols {
        for ti in 0..nt {
            let (e, _) = field(ti, sol, [0.25; 4]);
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
            let p = [
                mesh.nodes[f[0] as usize],
                mesh.nodes[f[1] as usize],
                mesh.nodes[f[2] as usize],
            ];
            let (n, area, _) = tri_geo(p);
            let mut jump_t = 0.0;
            let mut jump_n = czero();
            let mut cs = Vec::new();
            for (side, &ti) in ts.iter().enumerate() {
                let s = &asm.tets[ti];
                let mut l = [0.0; 4];
                for k in 0..4 {
                    if f.contains(&s[k]) {
                        l[k] = 1.0 / 3.0;
                    }
                }
                let (e, c) = field(ti, sol, l);
                let sign = if side == 0 { 1.0 } else { -1.0 };
                let en = (e[0] * n[0] + e[1] * n[1] + e[2] * n[2]) * asm.eps[ti];
                jump_n += en * sign;
                cs.push(c);
            }
            let d = [
                cs[0][0] - cs[1][0],
                cs[0][1] - cs[1][1],
                cs[0][2] - cs[1][2],
            ];
            // |n x d|²
            let nx = [
                d[1] * n[2] - d[2] * n[1],
                d[2] * n[0] - d[0] * n[2],
                d[0] * n[1] - d[1] * n[0],
            ];
            jump_t += nx.iter().map(|v| v.norm_sqr()).sum::<f64>();
            let v = area.sqrt() * area * (jump_t + k0 * k0 * jump_n.norm_sqr()) * 0.5;
            eta[ts[0]] += v;
            eta[ts[1]] += v;
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
    let mut mesh = model.mesh(setup.f_adapt, setup.per_lambda);
    let mut passes: Vec<Pass> = Vec::new();
    let mut converged = false;
    let mut last_unknowns;
    loop {
        let t0 = std::time::Instant::now();
        let (asm, sols, s) = solve_at(model, &mesh, setup.f_adapt)?;
        last_unknowns = asm.n;
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
            if d < setup.delta_s && passes.len() >= setup.min_passes {
                converged = true;
                break;
            }
        }
        if passes.len() >= setup.max_passes || asm.n * 2 > setup.max_unknowns {
            break;
        }
        let eta = estimate(model, &mesh, &asm, &sols, setup.f_adapt);
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
                    let t = pm.tris[i];
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
        let (asm, _, s) = solve_at(model, &mesh, f)?;
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
            max_unknowns: 60_000,
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
        assert!((z - 56.5).abs() < 0.07 * 56.5, "Z0 {z}");
        assert!(r.s[0][0].norm_sqr() + r.s[0][2].norm_sqr() > 0.98, "power");
        assert!(r.s[0][0].norm() < 0.08);
        assert!((r.s[0][2].norm() - 1.0).abs() < 0.03);
    }
}
