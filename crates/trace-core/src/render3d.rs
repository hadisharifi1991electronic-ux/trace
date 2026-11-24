//! A realistic 3D picture of a solved structure (a `fem3d::Model`: a via,
//! a board cut-out, a planar window), ray cast on the CPU: copper with its
//! real thickness, plated barrels with their drilled holes, pads, traces
//! and planes with their antipads, see-through laminate, wave ports, an
//! optional section cut, and the solver's fields painted on the cut face
//! (|E|, |H|) or on the copper (surface current), at any phase.

use crate::emfield::{self, Raster};
use crate::fem3d::{FieldGrid, Fill, Mode, Model, Shape};
use faer::c64;

const MIL: f64 = 25.4e-6;
type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V, s: f64) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V) -> V {
    let l = dot(a, a).sqrt().max(1e-30);
    mul(a, 1.0 / l)
}
fn lerp3(a: V, b: V, t: f64) -> V {
    add(mul(a, 1.0 - t), mul(b, t))
}

/// The two axes across `axis`, in increasing order.
fn across(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// What is painted on the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// |E| on the cut face.
    E,
    /// |H| on the cut face.
    H,
    /// Surface current density |n x H| on the copper.
    J,
}

impl Overlay {
    pub fn parse(s: &str) -> Option<Overlay> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "geometry" => Some(Overlay::None),
            "e" => Some(Overlay::E),
            "h" => Some(Overlay::H),
            "j" | "current" => Some(Overlay::J),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Overlay::None => "Geometry",
            Overlay::E => "|E| on the cut",
            Overlay::H => "|H| on the cut",
            Overlay::J => "Surface current |J| on the copper",
        }
    }
}

/// How the picture is taken.
#[derive(Clone, Debug)]
pub struct View {
    /// Turn about the vertical, degrees (0: looking along +y).
    pub az: f64,
    /// Height of the eye above the board's plane, degrees.
    pub el: f64,
    pub zoom: f64,
    /// Shift of the centre, in parts of the model's radius.
    pub pan: [f64; 2],
    pub w: usize,
    pub h: usize,
    /// Section cut: axis and position (mil); the half on the eye's side
    /// is taken away.
    pub cut: Option<(usize, f64)>,
    pub overlay: Overlay,
    /// None: the field's magnitude over the cycle; else the instant at
    /// this phase (degrees).
    pub phase: Option<f64>,
    pub range_db: f64,
    pub shadows: bool,
    /// Rays per pixel along each axis (1 for a quick look, 2 smooth).
    pub samples: usize,
    pub laminate: bool,
    pub title: String,
}

impl Default for View {
    fn default() -> View {
        View {
            az: -32.0,
            el: 24.0,
            zoom: 1.25,
            pan: [0.0, 0.0],
            w: 1200,
            h: 800,
            cut: None,
            overlay: Overlay::None,
            phase: None,
            range_db: 40.0,
            shadows: true,
            samples: 2,
            laminate: true,
            title: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum Prim {
    Box {
        lo: V,
        hi: V,
    },
    /// A plated barrel: copper between radii `ri` and `r` (ri 0: filled).
    Tube {
        axis: usize,
        c: [f64; 2],
        r: f64,
        ri: f64,
        lo: f64,
        hi: f64,
    },
    /// A copper sheet with thickness: between z0 and z1 along `axis`.
    Slab {
        axis: usize,
        z0: f64,
        z1: f64,
        poly: Vec<[f64; 2]>,
        holes: Vec<([f64; 2], [f64; 2], f64)>,
        disc: Option<([f64; 2], f64)>,
    },
}

#[derive(Clone, Debug)]
struct Item {
    prim: Prim,
    lo: V,
    hi: V,
}

struct Diel {
    lo: V,
    hi: V,
    tint: V,
}

struct Port {
    axis: usize,
    at: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    name: String,
    tint: V,
}

#[derive(Clone, Copy)]
enum Node {
    Leaf { lo: V, hi: V, start: u32, end: u32 },
    Inner { lo: V, hi: V, left: u32, right: u32 },
}

/// The structure, in mil, ready to ray cast.
pub struct Scene {
    items: Vec<Item>,
    nodes: Vec<Node>,
    diel: Vec<Diel>,
    ports: Vec<Port>,
    pub lo: V,
    pub hi: V,
    /// A signal barrel's centre (x, y), mil, for the default cut.
    barrel: Option<[f64; 2]>,
}

fn in_poly(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let n = v.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (v[i], v[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn seg_dist2(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + d[0] * t - p[0], a[1] + d[1] * t - p[1]];
    q[0] * q[0] + q[1] * q[1]
}

/// Ray (2D) against a circle: both roots.
fn circle_roots(o: [f64; 2], d: [f64; 2], c: [f64; 2], r: f64) -> Option<(f64, f64)> {
    let a = d[0] * d[0] + d[1] * d[1];
    if a < 1e-24 {
        return None;
    }
    let oc = [o[0] - c[0], o[1] - c[1]];
    let b = oc[0] * d[0] + oc[1] * d[1];
    let cc = oc[0] * oc[0] + oc[1] * oc[1] - r * r;
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    Some(((-b - s) / a, (-b + s) / a))
}

/// Ray (2D) against segment a-b: ray parameter.
fn seg_hit(o: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = d[0] * e[1] - d[1] * e[0];
    if den.abs() < 1e-18 {
        return None;
    }
    let w = [a[0] - o[0], a[1] - o[1]];
    let t = (w[0] * e[1] - w[1] * e[0]) / den;
    let s = (w[0] * d[1] - w[1] * d[0]) / den;
    (0.0..=1.0).contains(&s).then_some(t)
}

fn box_range(lo: V, hi: V, o: V, inv: V) -> (f64, f64) {
    let mut t0 = f64::MIN;
    let mut t1 = f64::MAX;
    for a in 0..3 {
        let mut ta = (lo[a] - o[a]) * inv[a];
        let mut tb = (hi[a] - o[a]) * inv[a];
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        if ta.is_nan() || tb.is_nan() {
            if o[a] < lo[a] || o[a] > hi[a] {
                return (1.0, 0.0);
            }
            continue;
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
    }
    (t0, t1)
}

impl Prim {
    fn region(&self, q: [f64; 2]) -> bool {
        let Prim::Slab {
            poly, holes, disc, ..
        } = self
        else {
            return false;
        };
        let inside = match disc {
            Some((c, r)) => (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) <= r * r,
            None => in_poly(poly, q),
        };
        inside && !holes.iter().any(|(a, b, r)| seg_dist2(*a, *b, q) < r * r)
    }

    fn contains(&self, p: V) -> bool {
        match self {
            Prim::Box { lo, hi } => (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]),
            Prim::Tube {
                axis,
                c,
                r,
                ri,
                lo,
                hi,
            } => {
                let (u, v) = across(*axis);
                let rho2 = (p[u] - c[0]).powi(2) + (p[v] - c[1]).powi(2);
                p[*axis] >= *lo && p[*axis] <= *hi && rho2 <= r * r && rho2 >= ri * ri
            }
            Prim::Slab { axis, z0, z1, .. } => {
                let (u, v) = across(*axis);
                p[*axis] >= *z0 && p[*axis] <= *z1 && self.region([p[u], p[v]])
            }
        }
    }

    /// Nearest hit past `t0` and before `t1`: (t, normal).
    fn hit(&self, o: V, d: V, inv: V, t0: f64, t1: f64) -> Option<(f64, V)> {
        let mut best: Option<(f64, V)> = None;
        let mut take = |t: f64, n: V| {
            if t > t0 && t < t1 && best.is_none_or(|b| t < b.0) {
                best = Some((t, n));
            }
        };
        match self {
            Prim::Box { lo, hi } => {
                for a in 0..3 {
                    for (w, s) in [(lo[a], -1.0), (hi[a], 1.0)] {
                        let t = (w - o[a]) * inv[a];
                        if !t.is_finite() {
                            continue;
                        }
                        let p = add(o, mul(d, t));
                        let (u, v) = across(a);
                        if p[u] >= lo[u] && p[u] <= hi[u] && p[v] >= lo[v] && p[v] <= hi[v] {
                            let mut n = [0.0; 3];
                            n[a] = s;
                            take(t, n);
                        }
                    }
                }
            }
            Prim::Tube {
                axis,
                c,
                r,
                ri,
                lo,
                hi,
            } => {
                let ax = *axis;
                let (u, v) = across(ax);
                let o2 = [o[u], o[v]];
                let d2 = [d[u], d[v]];
                let radial = |t: f64, inward: bool| {
                    let p = add(o, mul(d, t));
                    let mut n = [0.0; 3];
                    n[u] = p[u] - c[0];
                    n[v] = p[v] - c[1];
                    let n = norm(n);
                    if inward {
                        mul(n, -1.0)
                    } else {
                        n
                    }
                };
                for (rad, inward) in [(*r, false), (*ri, true)] {
                    if rad <= 0.0 {
                        continue;
                    }
                    if let Some((ta, tb)) = circle_roots(o2, d2, *c, rad) {
                        for t in [ta, tb] {
                            let w = o[ax] + d[ax] * t;
                            if w >= *lo && w <= *hi {
                                take(t, radial(t, inward));
                            }
                        }
                    }
                }
                for (w, s) in [(*lo, -1.0), (*hi, 1.0)] {
                    let t = (w - o[ax]) * inv[ax];
                    if !t.is_finite() {
                        continue;
                    }
                    let p = add(o, mul(d, t));
                    let rho2 = (p[u] - c[0]).powi(2) + (p[v] - c[1]).powi(2);
                    if rho2 <= r * r && rho2 >= ri * ri {
                        let mut n = [0.0; 3];
                        n[ax] = s;
                        take(t, n);
                    }
                }
            }
            Prim::Slab {
                axis,
                z0,
                z1,
                poly,
                holes,
                disc,
            } => {
                let ax = *axis;
                let (u, v) = across(ax);
                for (w, s) in [(*z0, -1.0), (*z1, 1.0)] {
                    let t = (w - o[ax]) * inv[ax];
                    if !t.is_finite() || t <= t0 || t >= t1 {
                        continue;
                    }
                    let p = add(o, mul(d, t));
                    if self.region([p[u], p[v]]) {
                        let mut n = [0.0; 3];
                        n[ax] = s;
                        take(t, n);
                    }
                }
                let o2 = [o[u], o[v]];
                let d2 = [d[u], d[v]];
                let in_z = |t: f64| {
                    let w = o[ax] + d[ax] * t;
                    w >= *z0 && w <= *z1
                };
                let wall = |_t: f64, n2: [f64; 2]| {
                    let mut n = [0.0; 3];
                    n[u] = n2[0];
                    n[v] = n2[1];
                    norm(n)
                };
                // Outline walls.
                match disc {
                    Some((c, r)) => {
                        if let Some((ta, tb)) = circle_roots(o2, d2, *c, *r) {
                            for t in [ta, tb] {
                                if in_z(t) {
                                    let q = [o2[0] + d2[0] * t, o2[1] + d2[1] * t];
                                    if !holes
                                        .iter()
                                        .any(|(a, b, rr)| seg_dist2(*a, *b, q) < rr * rr)
                                    {
                                        take(t, wall(t, [q[0] - c[0], q[1] - c[1]]));
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        let n = poly.len();
                        for i in 0..n {
                            let a = poly[i];
                            let b = poly[(i + 1) % n];
                            if let Some(t) = seg_hit(o2, d2, a, b) {
                                if t > t0 && t < t1 && in_z(t) {
                                    take(t, wall(t, [b[1] - a[1], a[0] - b[0]]));
                                }
                            }
                        }
                    }
                }
                // Antipad and cut walls (the copper is outside them).
                let inside_outline = |q: [f64; 2]| match disc {
                    Some((c, r)) => (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) <= r * r,
                    None => in_poly(poly, q),
                };
                for (hi_, (a, b, r)) in holes.iter().enumerate() {
                    let mut cands: Vec<(f64, [f64; 2])> = Vec::new();
                    for c in [a, b] {
                        if let Some((ta, tb)) = circle_roots(o2, d2, *c, *r) {
                            for t in [ta, tb] {
                                let q = [o2[0] + d2[0] * t, o2[1] + d2[1] * t];
                                cands.push((t, [c[0] - q[0], c[1] - q[1]]));
                            }
                        }
                    }
                    let e = [b[0] - a[0], b[1] - a[1]];
                    let el = (e[0] * e[0] + e[1] * e[1]).sqrt();
                    if el > 1e-9 {
                        let nn = [-e[1] / el * r, e[0] / el * r];
                        for sgn in [1.0, -1.0] {
                            let off = [nn[0] * sgn, nn[1] * sgn];
                            let pa = [a[0] + off[0], a[1] + off[1]];
                            let pb = [b[0] + off[0], b[1] + off[1]];
                            if let Some(t) = seg_hit(o2, d2, pa, pb) {
                                cands.push((t, [-off[0], -off[1]]));
                            }
                        }
                    }
                    for (t, n2) in cands {
                        if t <= t0 || t >= t1 || !in_z(t) {
                            continue;
                        }
                        let q = [o2[0] + d2[0] * t, o2[1] + d2[1] * t];
                        if !inside_outline(q) {
                            continue;
                        }
                        // On this hole's rim, not inside another hole.
                        let d_own = seg_dist2(*a, *b, q).sqrt();
                        if (d_own - r).abs() > 1e-6 * (1.0 + r) {
                            continue;
                        }
                        if holes
                            .iter()
                            .enumerate()
                            .any(|(k, (a2, b2, r2))| k != hi_ && seg_dist2(*a2, *b2, q) < r2 * r2)
                        {
                            continue;
                        }
                        take(t, wall(t, n2));
                    }
                }
            }
        }
        best
    }
}

impl Scene {
    /// `copper` is the drawn thickness of sheets (mil), `plating` the
    /// barrel wall (mil).
    pub fn new(model: &Model, copper: f64, plating: f64) -> Scene {
        let m = |x: f64| x / MIL;
        let mv = |p: [f64; 3]| [m(p[0]), m(p[1]), m(p[2])];
        let mut items = Vec::new();
        let mut diel = Vec::new();
        let mut barrel = None;
        for s in &model.solids {
            match (&s.shape, s.fill) {
                (Shape::Box { lo, hi }, Fill::Dielectric { er, .. }) => {
                    if er > 1.05 {
                        // Laminates read amber; the higher the
                        // permittivity, the deeper the tone.
                        let k = ((er - 2.0) / 3.0).clamp(0.0, 1.0);
                        let tint = lerp3([0.80, 0.70, 0.42], [0.55, 0.42, 0.18], k);
                        diel.push(Diel {
                            lo: mv(*lo),
                            hi: mv(*hi),
                            tint,
                        });
                    }
                }
                (Shape::Box { lo, hi }, Fill::Pec { .. }) => items.push(Item {
                    prim: Prim::Box {
                        lo: mv(*lo),
                        hi: mv(*hi),
                    },
                    lo: mv(*lo),
                    hi: mv(*hi),
                }),
                (Shape::Cyl { axis, c, r, lo, hi }, Fill::Pec { net }) => {
                    let r = m(*r);
                    let ri = if r > 2.5 * plating { r - plating } else { 0.0 };
                    let c = [m(c[0]), m(c[1])];
                    if net != 0 && barrel.is_none() && *axis == 2 {
                        barrel = Some(c);
                    }
                    let (u, v) = across(*axis);
                    let mut blo = [0.0; 3];
                    let mut bhi = [0.0; 3];
                    blo[*axis] = m(*lo);
                    bhi[*axis] = m(*hi);
                    blo[u] = c[0] - r;
                    bhi[u] = c[0] + r;
                    blo[v] = c[1] - r;
                    bhi[v] = c[1] + r;
                    items.push(Item {
                        prim: Prim::Tube {
                            axis: *axis,
                            c,
                            r,
                            ri,
                            lo: m(*lo),
                            hi: m(*hi),
                        },
                        lo: blo,
                        hi: bhi,
                    });
                }
                _ => {}
            }
        }
        let box_lo = mv(model.lo);
        let box_hi = mv(model.hi);
        for sh in &model.sheets {
            let at = m(sh.at);
            // Keep the copper inside the box (a plane on a wall).
            let mut z0 = at - copper / 2.0;
            let mut z1 = at + copper / 2.0;
            if z0 < box_lo[sh.axis] {
                z0 = box_lo[sh.axis];
                z1 = z0 + copper;
            }
            if z1 > box_hi[sh.axis] {
                z1 = box_hi[sh.axis];
                z0 = z1 - copper;
            }
            let poly: Vec<[f64; 2]> = sh.poly.iter().map(|p| [m(p[0]), m(p[1])]).collect();
            let holes: Vec<_> = sh
                .holes
                .iter()
                .map(|(a, b, r)| ([m(a[0]), m(a[1])], [m(b[0]), m(b[1])], m(*r)))
                .collect();
            let disc = sh.disc.map(|(c, r)| ([m(c[0]), m(c[1])], m(r)));
            let (u, v) = across(sh.axis);
            let (mut l2, mut h2) = ([f64::MAX; 2], [f64::MIN; 2]);
            match disc {
                Some((c, r)) => {
                    l2 = [c[0] - r, c[1] - r];
                    h2 = [c[0] + r, c[1] + r];
                }
                None => {
                    for p in &poly {
                        for k in 0..2 {
                            l2[k] = l2[k].min(p[k]);
                            h2[k] = h2[k].max(p[k]);
                        }
                    }
                }
            }
            if l2[0] > h2[0] {
                continue;
            }
            let mut lo = [0.0; 3];
            let mut hi = [0.0; 3];
            lo[sh.axis] = z0;
            hi[sh.axis] = z1;
            lo[u] = l2[0];
            hi[u] = h2[0];
            lo[v] = l2[1];
            hi[v] = h2[1];
            items.push(Item {
                prim: Prim::Slab {
                    axis: sh.axis,
                    z0,
                    z1,
                    poly,
                    holes,
                    disc,
                },
                lo,
                hi,
            });
        }
        let ports = model
            .ports
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let at = if p.high {
                    box_hi[p.axis]
                } else {
                    box_lo[p.axis]
                };
                let diff = matches!(p.mode, Mode::Diff { .. });
                Port {
                    axis: p.axis,
                    at,
                    lo: [m(p.lo[0]), m(p.lo[1])],
                    hi: [m(p.hi[0]), m(p.hi[1])],
                    name: format!("P{}", k + 1),
                    tint: if diff {
                        [0.85, 0.25, 0.75]
                    } else {
                        [0.95, 0.22, 0.18]
                    },
                }
            })
            .collect();
        let mut sc = Scene {
            items,
            nodes: Vec::new(),
            diel,
            ports,
            lo: box_lo,
            hi: box_hi,
            barrel,
        };
        sc.build_bvh();
        sc
    }

    pub fn centre(&self) -> V {
        mul(add(self.lo, self.hi), 0.5)
    }

    pub fn radius(&self) -> f64 {
        dot(sub(self.hi, self.lo), sub(self.hi, self.lo)).sqrt() / 2.0
    }

    /// A cut through the first signal barrel (else the middle), across y.
    pub fn default_cut(&self) -> (usize, f64) {
        (1, self.barrel.map_or(self.centre()[1], |c| c[1]))
    }

    fn build_bvh(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut order: Vec<usize> = (0..n).collect();
        let mut nodes = Vec::new();
        fn bounds(items: &[Item], ix: &[usize]) -> (V, V) {
            let mut lo = [f64::MAX; 3];
            let mut hi = [f64::MIN; 3];
            for &i in ix {
                for a in 0..3 {
                    lo[a] = lo[a].min(items[i].lo[a]);
                    hi[a] = hi[a].max(items[i].hi[a]);
                }
            }
            // A thin pad around zero-width boxes.
            for a in 0..3 {
                lo[a] -= 1e-6;
                hi[a] += 1e-6;
            }
            (lo, hi)
        }
        fn build(items: &[Item], ix: &mut [usize], off: usize, nodes: &mut Vec<Node>) -> u32 {
            let (lo, hi) = bounds(items, ix);
            let me = nodes.len();
            if ix.len() <= 4 {
                nodes.push(Node::Leaf {
                    lo,
                    hi,
                    start: off as u32,
                    end: (off + ix.len()) as u32,
                });
                return me as u32;
            }
            nodes.push(Node::Leaf {
                lo,
                hi,
                start: 0,
                end: 0,
            });
            let ext = sub(hi, lo);
            let a = (0..3).max_by(|&i, &j| ext[i].total_cmp(&ext[j])).unwrap();
            ix.sort_by(|&p, &q| {
                (items[p].lo[a] + items[p].hi[a]).total_cmp(&(items[q].lo[a] + items[q].hi[a]))
            });
            let mid = ix.len() / 2;
            let (l, r) = ix.split_at_mut(mid);
            let left = build(items, l, off, nodes);
            let right = build(items, r, off + mid, nodes);
            nodes[me] = Node::Inner {
                lo,
                hi,
                left,
                right,
            };
            me as u32
        }
        build(&self.items, &mut order, 0, &mut nodes);
        let items = order.iter().map(|&i| self.items[i].clone()).collect();
        self.items = items;
        self.nodes = nodes;
    }

    /// The nearest copper hit in (t0, t1).
    fn trace(&self, o: V, d: V, t0: f64, t1: f64) -> Option<(f64, V)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut best: Option<(f64, V)> = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let lim = best.map_or(t1, |b| b.0);
            let node = self.nodes[ni as usize];
            let (lo, hi) = match node {
                Node::Leaf { lo, hi, .. } | Node::Inner { lo, hi, .. } => (lo, hi),
            };
            let (a, b) = box_range(lo, hi, o, inv);
            if a > b || b < t0 || a > lim {
                continue;
            }
            match node {
                Node::Leaf { start, end, .. } => {
                    for it in &self.items[start as usize..end as usize] {
                        if let Some(h) =
                            it.prim
                                .hit(o, d, inv, t0, lim.min(best.map_or(t1, |b| b.0)))
                        {
                            if best.is_none_or(|b| h.0 < b.0) {
                                best = Some(h);
                            }
                        }
                    }
                }
                Node::Inner { left, right, .. } => {
                    stack.push(left);
                    stack.push(right);
                }
            }
        }
        best
    }

    fn in_copper(&self, p: V) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            let (lo, hi) = match node {
                Node::Leaf { lo, hi, .. } | Node::Inner { lo, hi, .. } => (lo, hi),
            };
            if (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a]) {
                continue;
            }
            match node {
                Node::Leaf { start, end, .. } => {
                    if self.items[start as usize..end as usize]
                        .iter()
                        .any(|it| it.prim.contains(p))
                    {
                        return true;
                    }
                }
                Node::Inner { left, right, .. } => {
                    stack.push(left);
                    stack.push(right);
                }
            }
        }
        false
    }

    fn laminate_at(&self, p: V) -> Option<V> {
        self.diel
            .iter()
            .rev()
            .find(|d| (0..3).all(|a| p[a] >= d.lo[a] - 1e-9 && p[a] <= d.hi[a] + 1e-9))
            .map(|d| d.tint)
    }
}

/// Field magnitudes to paint, from a grid.
struct Paint<'a> {
    g: &'a FieldGrid,
    rot: c64,
    instant: bool,
    top_e: f64,
    top_h: f64,
    range: f64,
}

fn mag(v: &[c64; 3], rot: c64, instant: bool) -> f64 {
    if instant {
        v.iter().map(|z| (z * rot).re.powi(2)).sum::<f64>().sqrt()
    } else {
        v.iter().map(|z| z.norm_sqr()).sum::<f64>().sqrt()
    }
}

/// A high percentile of a field's magnitude over the grid (the peak sits
/// on sharp copper edges and would wash the rest out).
fn top_of(g: &FieldGrid, h: bool) -> f64 {
    let src = if h { &g.hf } else { &g.e };
    let step = (src.len() / 200_000).max(1);
    let mut v: Vec<f64> = src
        .iter()
        .step_by(step)
        .map(|x| (0..6).map(|k| (x[k] as f64).powi(2)).sum::<f64>().sqrt())
        .filter(|x| *x > 0.0)
        .collect();
    if v.is_empty() {
        return 1.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 * 0.995) as usize).min(v.len() - 1)]
}

fn srgb(c: V) -> [u8; 3] {
    c.map(|x| (x.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8)
}
fn lin(c: [u8; 3]) -> V {
    c.map(|x| (x as f64 / 255.0).powf(2.2))
}

/// Copper's colour (linear; about sRGB 232, 142, 88).
const COPPER: V = [0.80, 0.27, 0.10];

struct Cam {
    eye: V,
    f: V,
    r: V,
    u: V,
    tan: f64,
    aspect: f64,
}

impl Cam {
    fn new(sc: &Scene, v: &View) -> Cam {
        let (az, el) = (v.az.to_radians(), v.el.to_radians());
        let back = [el.cos() * az.sin(), -el.cos() * az.cos(), el.sin()];
        let rad = sc.radius();
        let r0 = norm(cross(mul(back, -1.0), [0.0, 0.0, 1.0]));
        let u0 = cross(r0, mul(back, -1.0));
        let target = add(
            sc.centre(),
            add(mul(r0, v.pan[0] * rad), mul(u0, v.pan[1] * rad)),
        );
        let tan = (15f64).to_radians().tan();
        let dist = rad / tan * 1.08 / v.zoom.max(0.05);
        let eye = add(target, mul(back, dist));
        Cam {
            eye,
            f: mul(back, -1.0),
            r: r0,
            u: u0,
            tan,
            aspect: v.w as f64 / v.h.max(1) as f64,
        }
    }
    /// Pixel (x right, y down, in 0..1) to a ray direction.
    fn ray(&self, sx: f64, sy: f64) -> V {
        let x = (2.0 * sx - 1.0) * self.tan * self.aspect;
        let y = (1.0 - 2.0 * sy) * self.tan;
        norm(add(self.f, add(mul(self.r, x), mul(self.u, y))))
    }
    /// A point to the picture (pixels), None behind the eye.
    fn project(&self, p: V, w: usize, h: usize) -> Option<[f64; 2]> {
        let q = sub(p, self.eye);
        let z = dot(q, self.f);
        if z <= 1e-9 {
            return None;
        }
        let x = dot(q, self.r) / z / (self.tan * self.aspect);
        let y = dot(q, self.u) / z / self.tan;
        Some([(x + 1.0) / 2.0 * w as f64, (1.0 - y) / 2.0 * h as f64])
    }
}

fn background(sy: f64) -> V {
    lerp3([0.030, 0.034, 0.048], [0.11, 0.12, 0.15], sy)
}

/// Copper's colour at a surface: metal with a sky to reflect, a key and a
/// fill light, optionally darkened by a shadow.
fn shade_metal(base: V, n: V, view: V, key: V, fill: V, lit: f64) -> V {
    let n = if dot(n, view) > 0.0 { mul(n, -1.0) } else { n };
    let v = mul(view, -1.0);
    let diff = dot(n, key).max(0.0) * lit;
    let diff2 = dot(n, fill).max(0.0);
    let hv = norm(add(key, v));
    let spec = dot(n, hv).max(0.0).powf(48.0) * lit;
    let refl = sub(view, mul(n, 2.0 * dot(view, n)));
    let sky = (0.5 + 0.5 * refl[2]).clamp(0.0, 1.0);
    let env = 0.10 + 0.55 * sky * sky;
    let fres = (1.0 - dot(n, v).max(0.0)).powi(5);
    let f0 = lerp3(base, [1.0, 1.0, 1.0], fres * 0.6);
    let mut c = [0.0; 3];
    for k in 0..3 {
        c[k] =
            f0[k] * (0.06 + 0.55 * diff + 0.12 * diff2 + env * 0.55) + spec * (0.6 * f0[k] + 0.4);
    }
    c
}

fn shade_matte(base: V, n: V, view: V, key: V, fill: V) -> V {
    let n = if dot(n, view) > 0.0 { mul(n, -1.0) } else { n };
    let diff = dot(n, key).max(0.0);
    let diff2 = dot(n, fill).max(0.0);
    mul(base, 0.30 + 0.60 * diff + 0.15 * diff2)
}

/// One ray's colour (linear).
fn cast(sc: &Scene, paint: Option<&Paint>, v: &View, cam: &Cam, d: V, sy: f64) -> V {
    let o = cam.eye;
    let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
    let key = norm(add(
        add(mul(cam.u, 1.2), mul(cam.r, -0.7)),
        mul(cam.f, -0.8),
    ));
    let key = norm(add(key, [0.0, 0.0, 0.6]));
    let fill = norm(add(add(mul(cam.u, 0.3), mul(cam.r, 0.9)), mul(cam.f, -0.5)));
    let (bt0, bt1) = box_range(sc.lo, sc.hi, o, inv);
    let bg = background(sy);
    if bt0 > bt1 || bt1 < 0.0 {
        return bg;
    }
    let mut t0 = bt0.max(0.0);
    let t1 = bt1;
    let mut cut_t = None;
    if let Some((ax, at)) = v.cut {
        // The eye's side is taken away.
        let tp = (at - o[ax]) * inv[ax];
        let eye_low = o[ax] < at;
        let toward = if eye_low { d[ax] > 0.0 } else { d[ax] < 0.0 };
        if !toward {
            return bg;
        }
        if tp > t0 {
            t0 = tp;
            if tp < t1 {
                cut_t = Some(tp);
            }
        }
    }
    let eps = 1e-6 * sc.radius();
    let mut col: V;
    let mut t_end;
    // The cut face first.
    let mut done = false;
    col = bg;
    t_end = t1;
    if let (Some(tc), Some((ax, _))) = (cut_t, v.cut) {
        let p = add(o, mul(d, tc));
        let mut n = [0.0; 3];
        n[ax] = if o[ax] < p[ax] { -1.0 } else { 1.0 };
        if sc.in_copper(p) {
            let c = shade_metal(COPPER, n, d, key, fill, 1.0);
            // A polished section reads a little paler.
            col = lerp3(c, [0.90, 0.45, 0.25], 0.15);
            t_end = tc;
            done = true;
        } else if let Some(pt) = paint.filter(|_| matches!(v.overlay, Overlay::E | Overlay::H)) {
            let (e, h) = pt.g.at(mul(p, MIL));
            let (val, top) = if v.overlay == Overlay::E {
                (mag(&e, pt.rot, pt.instant), pt.top_e)
            } else {
                (mag(&h, pt.rot, pt.instant), pt.top_h)
            };
            let tt = emfield::scale_t(val, top, pt.range, 20.0);
            let heat = lin(emfield::colour(tt));
            let lam = sc.laminate_at(p).unwrap_or([0.05, 0.05, 0.06]);
            col = lerp3(mul(lam, 0.35), heat, if tt > 0.0 { 0.92 } else { 0.0 });
            t_end = tc;
            done = true;
        }
    }
    if !done {
        if let Some((th, n)) = sc.trace(o, d, t0 + eps, t1) {
            let p = add(o, mul(d, th));
            let nf = if dot(n, d) > 0.0 { mul(n, -1.0) } else { n };
            let lit = if v.shadows
                && sc
                    .trace(add(p, mul(nf, eps * 20.0)), key, 0.0, f64::MAX)
                    .is_some()
            {
                0.25
            } else {
                1.0
            };
            let mut base = COPPER;
            if let (Some(pt), Overlay::J) = (paint, v.overlay) {
                let q = add(p, mul(nf, pt.g.h / MIL * 0.9));
                let (_, h) = pt.g.at(mul(q, MIL));
                // Tangential H: the surface current.
                let hn: c64 = h[0] * nf[0] + h[1] * nf[1] + h[2] * nf[2];
                let ht = [h[0] - hn * nf[0], h[1] - hn * nf[1], h[2] - hn * nf[2]];
                let tt = emfield::scale_t(mag(&ht, pt.rot, pt.instant), pt.top_h, pt.range, 20.0);
                base = lerp3(mul(COPPER, 0.25), lin(emfield::colour(tt)), 0.9);
                let c = shade_matte(base, n, d, key, fill);
                col = add(
                    mul(c, 1.25),
                    mul(shade_metal([0.2; 3], n, d, key, fill, lit), 0.25),
                );
            } else {
                col = shade_metal(base, n, d, key, fill, lit);
            }
            t_end = th;
        }
    }
    // See-through laminate and the ports in front, far to near.
    let mut layers: Vec<(f64, V, f64)> = Vec::new();
    if v.laminate {
        let thick = 0.3 * sc.radius();
        for dl in &sc.diel {
            let (a, b) = box_range(dl.lo, dl.hi, o, inv);
            let a = a.max(t0);
            let b = b.min(t_end);
            if b > a {
                let alpha = 1.0 - (-(b - a) / thick).exp();
                // A faint sheen where the ray enters the board.
                layers.push((a, dl.tint, (alpha * 0.9).min(0.8)));
            }
        }
    }
    for p in &sc.ports {
        let t = (p.at - o[p.axis]) * inv[p.axis];
        if t > t0 - eps && t < t_end {
            let q = add(o, mul(d, t));
            let (u, w) = across(p.axis);
            if q[u] >= p.lo[0] && q[u] <= p.hi[0] && q[w] >= p.lo[1] && q[w] <= p.hi[1] {
                layers.push((t, p.tint, 0.16));
            }
        }
    }
    layers.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, c, a) in layers {
        let lam = shade_matte(c, mul(d, -1.0), d, key, fill);
        col = lerp3(col, lam, a);
    }
    col
}

/// The picture.
pub fn render(sc: &Scene, fields: Option<&FieldGrid>, v: &View) -> Raster {
    let cam = Cam::new(sc, v);
    let paint = fields.map(|g| {
        let ph = v.phase.unwrap_or(0.0).to_radians();
        Paint {
            g,
            rot: c64::new(ph.cos(), ph.sin()),
            instant: v.phase.is_some(),
            top_e: top_of(g, false),
            top_h: top_of(g, true),
            range: v.range_db,
        }
    });
    let s = v.samples.clamp(1, 4);
    let (w, h) = (v.w.max(16), v.h.max(16));
    let mut lin_px = vec![[0f32; 3]; w * h];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let rows_per = h.div_ceil(threads * 4).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(&mut lin_px);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let r0 = next.fetch_add(rows_per, std::sync::atomic::Ordering::Relaxed);
                if r0 >= h {
                    break;
                }
                let r1 = (r0 + rows_per).min(h);
                let mut buf = vec![[0f32; 3]; (r1 - r0) * w];
                for y in r0..r1 {
                    for x in 0..w {
                        let mut acc = [0.0; 3];
                        for j in 0..s {
                            for i in 0..s {
                                let sx = (x as f64 + (i as f64 + 0.5) / s as f64) / w as f64;
                                let sy = (y as f64 + (j as f64 + 0.5) / s as f64) / h as f64;
                                let c = cast(sc, paint.as_ref(), v, &cam, cam.ray(sx, sy), sy);
                                acc = add(acc, c);
                            }
                        }
                        let k = 1.0 / (s * s) as f64;
                        buf[(y - r0) * w + x] = [
                            (acc[0] * k) as f32,
                            (acc[1] * k) as f32,
                            (acc[2] * k) as f32,
                        ];
                    }
                }
                let mut o = out.lock().unwrap();
                o[r0 * w..r1 * w].copy_from_slice(&buf);
            });
        }
    });
    let mut r = Raster::new(w, h, [0, 0, 0]);
    for (i, c) in lin_px.iter().enumerate() {
        // A soft tone curve so bright copper does not clip.
        let tone = |x: f32| {
            let x = x as f64 * 1.1;
            x / (1.0 + 0.25 * x)
        };
        let px = srgb([tone(c[0]), tone(c[1]), tone(c[2])]);
        r.rgb[i * 3..i * 3 + 3].copy_from_slice(&px);
    }
    decorate(&mut r, sc, &cam, v, paint.as_ref());
    r
}

/// Box edges, port names, the axes, a scale bar, the title and colour bar.
fn decorate(r: &mut Raster, sc: &Scene, cam: &Cam, v: &View, paint: Option<&Paint>) {
    let (w, h) = (r.w, r.h);
    let grey = [92, 98, 112];
    let (lo, hi) = (sc.lo, sc.hi);
    let corner = |k: usize| {
        [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    for a in 0..8usize {
        for bit in [1usize, 2, 4] {
            let b = a | bit;
            if b == a {
                continue;
            }
            if let (Some(p), Some(q)) = (cam.project(corner(a), w, h), cam.project(corner(b), w, h))
            {
                dashed(r, p, q, grey);
            }
        }
    }
    let size = (h as f64 / 60.0).max(8.0);
    for p in &sc.ports {
        let (u, ww) = across(p.axis);
        let mut c = [0.0; 3];
        c[p.axis] = p.at;
        c[u] = (p.lo[0] + p.hi[0]) / 2.0;
        c[ww] = p.hi[1];
        if let Some(s) = cam.project(c, w, h) {
            let tw = Raster::text_width(size, &p.name);
            r.text(
                [s[0] - tw / 2.0, s[1] - 6.0],
                size,
                &p.name,
                [255, 120, 110],
            );
        }
    }
    // Axes, lower left.
    let o2 = [size * 3.5, h as f64 - size * 3.5];
    for (a, c, n) in [
        (0usize, [235, 90, 80], "x"),
        (1, [110, 210, 110], "y"),
        (2, [100, 150, 255], "z"),
    ] {
        let mut e = [0.0; 3];
        e[a] = 1.0;
        let sx = dot(e, cam.r);
        let sy = -dot(e, cam.u);
        let l = size * 2.4;
        let q = [o2[0] + sx * l, o2[1] + sy * l];
        thick_line(r, o2, q, c);
        r.text([q[0] + 3.0, q[1] + size * 0.4], size * 0.8, n, c);
    }
    // Scale bar: a round number of mil about a sixth of the width.
    let c0 = sc.centre();
    if let (Some(p), Some(q)) = (cam.project(c0, w, h), cam.project(add(c0, cam.r), w, h)) {
        let px_per_mil = (q[0] - p[0]).hypot(q[1] - p[1]);
        if px_per_mil > 0.0 {
            let want = w as f64 / 6.0 / px_per_mil;
            let mag = 10f64.powf(want.log10().floor());
            let nice = [1.0, 2.0, 5.0, 10.0]
                .iter()
                .map(|k| k * mag)
                .rfind(|x| *x <= want)
                .unwrap_or(mag);
            let len = nice * px_per_mil;
            let x0 = size * 8.0;
            let y0 = h as f64 - size * 1.6;
            thick_line(r, [x0, y0], [x0 + len, y0], [220, 220, 225]);
            r.line([x0, y0 - 4.0], [x0, y0 + 4.0], [220, 220, 225]);
            r.line([x0 + len, y0 - 4.0], [x0 + len, y0 + 4.0], [220, 220, 225]);
            let txt = format!("{} mil", emfield::si(nice).trim());
            r.text(
                [x0 + len + 8.0, y0 + size * 0.45],
                size * 0.9,
                &txt,
                [220, 220, 225],
            );
        }
    }
    if !v.title.is_empty() {
        r.text(
            [size * 1.2, size * 2.2],
            size * 1.1,
            &v.title,
            [235, 235, 240],
        );
    }
    let sub_t = match (paint, v.overlay) {
        (Some(pt), o) if o != Overlay::None => {
            let ph = match v.phase {
                Some(p) => format!("phase {:.0} deg", p.rem_euclid(360.0)),
                None => "magnitude".into(),
            };
            format!(
                "{} at {:.2} GHz, {} driven, {}",
                o.label(),
                pt.g.f_ghz,
                pt.g.port,
                ph
            )
        }
        _ => "Copper, laminate and wave ports".into(),
    };
    r.text(
        [size * 1.2, size * 4.0],
        size * 0.85,
        &sub_t,
        [190, 192, 200],
    );
    if let (Some(pt), true) = (paint, v.overlay != Overlay::None) {
        let x0 = w - (size * 6.5) as usize;
        let y0 = (size * 6.0) as usize;
        let bh = h / 2;
        let bw = (size * 1.2) as usize;
        for k in 0..bh {
            let t = 1.0 - k as f64 / bh as f64;
            r.fill(x0, y0 + k, x0 + bw, y0 + k + 1, emfield::colour(t));
        }
        let steps = (pt.range / 10.0).round().max(1.0) as usize;
        for i in 0..=steps {
            let db = -(i as f64) * pt.range / steps as f64;
            let y = y0 as f64 + bh as f64 * (i as f64 / steps as f64);
            r.line([x0 as f64 - 3.0, y], [x0 as f64, y], [220, 220, 225]);
            r.text(
                [x0 as f64 + bw as f64 + 5.0, y + size * 0.35],
                size * 0.75,
                &format!("{db:.0}"),
                [220, 220, 225],
            );
        }
        r.text(
            [x0 as f64 - size, y0 as f64 - size * 0.9],
            size * 0.75,
            "dB",
            [220, 220, 225],
        );
    }
}

fn dashed(r: &mut Raster, a: [f64; 2], b: [f64; 2], c: [u8; 3]) {
    let l = (b[0] - a[0]).hypot(b[1] - a[1]);
    let n = (l / 6.0).ceil().max(1.0) as usize;
    for k in (0..n).step_by(2) {
        let t0 = k as f64 / n as f64;
        let t1 = ((k + 1) as f64 / n as f64).min(1.0);
        r.line(
            [a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0],
            [a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1],
            c,
        );
    }
}

fn thick_line(r: &mut Raster, a: [f64; 2], b: [f64; 2], c: [u8; 3]) {
    for o in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]] {
        r.line([a[0] + o[0], a[1] + o[1]], [b[0] + o[0], b[1] + o[1]], c);
    }
}

/// Pictures for an animation: the field through one cycle (with a field
/// overlay), else one turn around the structure.
pub fn frames(sc: &Scene, fields: Option<&FieldGrid>, v: &View, n: usize) -> Vec<Raster> {
    let n = n.max(2);
    (0..n)
        .map(|k| {
            let mut vv = v.clone();
            if fields.is_some() && v.overlay != Overlay::None {
                vv.phase = Some(360.0 * k as f64 / n as f64);
            } else {
                vv.az = v.az + 360.0 * k as f64 / n as f64;
            }
            render(sc, fields, &vv)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fem3d;

    #[test]
    fn a_via_draws_copper_laminate_and_its_hole() {
        let model = fem3d::line(fem3d::LineKind::Microstrip, 10.0, 5.0, 4.0, 0.0, 30.0);
        let sc = Scene::new(&model, 1.4, 1.0);
        assert!(!sc.items.is_empty());
        let v = View {
            w: 120,
            h: 80,
            samples: 1,
            shadows: true,
            ..View::default()
        };
        let r = render(&sc, None, &v);
        assert_eq!(r.rgb.len(), 120 * 80 * 3);
        // Something copper-coloured (red over blue) is in the picture.
        let coppery = r
            .rgb
            .chunks(3)
            .filter(|c| c[0] as i32 > c[2] as i32 + 40 && c[0] > 90)
            .count();
        assert!(coppery > 50, "{coppery}");
        let mut vc = v.clone();
        vc.cut = Some(sc.default_cut());
        let rc = render(&sc, None, &vc);
        assert_eq!(rc.w, 120);
    }
}
