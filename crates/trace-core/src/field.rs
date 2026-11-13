//! A 2D field solver for transmission line cross-sections.
//!
//! The cross-section of one track (or a pair) over its reference planes
//! is meshed with a graded rectangular grid, fine at the copper's edges
//! and corners, and Laplace's equation is solved on it by finite volumes
//! (conjugate gradients). Each dielectric slab of the stack-up keeps its
//! own permittivity and loss tangent, and an outer layer's solder mask
//! covers the copper. From the field energy come the capacitance matrix
//! with the dielectric and in air, and from those the impedance, the
//! effective permittivity, the even and odd modes and the coupling. A
//! track can have ground copper beside it on its own layer (coplanar
//! waveguide with ground, as a pour leaves it). The conductor loss comes
//! from Wheeler's incremental inductance rule (the inductance again with
//! every copper face receded a little), tracks and planes together; the
//! dielectric loss weights each slab's loss tangent by the field energy in
//! it.

#![allow(clippy::needless_range_loop)]

use crate::impedance::LayerGeometry;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Mutex;

const E0: f64 = 8.854_187_8e-12;
const C0: f64 = 2.997_924_58e8;
const MU0: f64 = 4e-7 * PI;
const MIL: f64 = 25.4e-6;
/// Copper resistivity, Ω·m.
pub const RHO: f64 = 1.72e-8;

/// A dielectric slab: thickness (mil), permittivity, loss tangent (−1
/// when the stack-up gives none).
pub type Slab = [f64; 3];

/// One track over its planes, solved.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Characteristic impedance, Ω.
    pub z0: f64,
    /// Effective permittivity.
    pub eeff: f64,
    /// Inductance and capacitance per metre (H/m, F/m).
    pub l: f64,
    pub c: f64,
    /// Skin-effect resistance per metre at 1 GHz on smooth copper, tracks
    /// and planes together (Wheeler's incremental inductance rule), Ω/m.
    pub r_ghz: f64,
    /// DC resistance of the track, Ω/m.
    pub r_dc: f64,
    /// Share of the electric energy in dielectric (not air), 0 to 1.
    pub fill: f64,
    /// Loss tangent weighted by the field energy in each slab, where the
    /// slabs give one (others count as 0).
    pub tan_eff: f64,
    /// Share of the electric energy in slabs that give a loss tangent.
    pub fill_given: f64,
}

impl Line {
    /// Delay, ps per inch.
    pub fn ps_per_in(&self) -> f64 {
        self.eeff.sqrt() / C0 * 0.0254 * 1e12
    }

    /// Loss at `f` Hz in dB per inch, with `tan_d` for slabs that give
    /// none and copper roughness `rough_um` (Hammerstad).
    pub fn db_per_in(&self, f: f64, tan_d: f64, rough_um: Option<f64>) -> f64 {
        let k = rough_um.map_or(1.0, |r| crate::sim::loss::rough(f, r));
        let rac = self.r_ghz * (f / 1e9).sqrt() * k;
        let r = rac.hypot(self.r_dc);
        let tan = self.tan_eff + (self.fill - self.fill_given).max(0.0) * tan_d;
        let alpha_c = r / (2.0 * self.z0);
        let alpha_d = PI * f * self.eeff.sqrt() / C0 * tan;
        8.686 * (alpha_c + alpha_d) * 0.0254
    }
}

/// A pair of equal tracks, solved.
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    pub z_odd: f64,
    pub z_even: f64,
    pub e_odd: f64,
    pub e_even: f64,
    /// Differential (2 Z odd) and common mode (Z even / 2) impedance, Ω.
    pub zdiff: f64,
    pub zcomm: f64,
    /// Backward (near-end) and forward (far-end) coupling.
    pub kb: f64,
    pub kf: f64,
    /// Odd mode skin resistance per line at 1 GHz, Ω/m, and its loss
    /// tangent weighted by field energy.
    pub r_odd_ghz: f64,
    pub tan_odd: f64,
    /// The single track alone on the layer.
    pub single: Line,
}

/// The potential over the cross-section, for drawing.
#[derive(Clone, Debug)]
pub struct Picture {
    /// Node coordinates, mil (x across, y up from the lower plane).
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    /// Potential at each node, row by row (y outer), −1 to 1.
    pub phi: Vec<f64>,
    /// Each slab: bottom, top (mil), permittivity.
    pub slabs: Vec<(f64, f64, f64)>,
    /// Copper: x0, x1, y0, y1 (mil); the tracks, then the planes.
    pub copper: Vec<[f64; 4]>,
    /// Number of tracks at the front of `copper`.
    pub tracks: usize,
    /// The upper reference plane's face, mil, for a stripline.
    pub upper: Option<f64>,
}

impl Picture {
    /// The part worth drawing: x0, x1, y0, y1 (mil) around the tracks.
    pub fn view(&self) -> [f64; 4] {
        let tr = &self.copper[..self.tracks];
        let (a, b) = (tr[0][0], tr[tr.len() - 1][1]);
        let y1 = tr[0][3];
        let h = tr[0][2].max(1.0);
        let top = self.upper.unwrap_or(y1 + 2.5 * h);
        let half = ((b - a) / 2.0 + 3.0 * h).max((top + 1.0) * 1.2);
        let mid = (a + b) / 2.0;
        let pl = (h / 20.0).max(0.3);
        [
            mid - half,
            mid + half,
            -pl - 0.5,
            top + if self.upper.is_some() { pl + 0.5 } else { 0.0 },
        ]
    }

    /// Potential at a point, by bilinear interpolation.
    pub fn at(&self, x: f64, y: f64) -> f64 {
        let find = |v: &[f64], q: f64| -> (usize, f64) {
            let i = v.partition_point(|&p| p <= q).clamp(1, v.len() - 1) - 1;
            let f = ((q - v[i]) / (v[i + 1] - v[i])).clamp(0.0, 1.0);
            (i, f)
        };
        let nx = self.xs.len();
        let (i, fx) = find(&self.xs, x);
        let (j, fy) = find(&self.ys, y);
        let p = |i: usize, j: usize| self.phi[j * nx + i];
        (p(i, j) * (1.0 - fx) + p(i + 1, j) * fx) * (1.0 - fy)
            + (p(i, j + 1) * (1.0 - fx) + p(i + 1, j + 1) * fx) * fy
    }

    /// The colour of a potential: blue (−1) through near-black (0) to
    /// amber (+1).
    pub fn color(v: f64) -> [u8; 3] {
        let v = v.clamp(-1.0, 1.0);
        let lerp = |a: [f64; 3], b: [f64; 3], t: f64| {
            [0, 1, 2].map(|k| (a[k] + (b[k] - a[k]) * t).round() as u8)
        };
        let dark = [18.0, 22.0, 38.0];
        if v >= 0.0 {
            lerp(dark, [255.0, 196.0, 64.0], v.powf(0.7))
        } else {
            lerp(dark, [70.0, 150.0, 255.0], (-v).powf(0.7))
        }
    }

    /// The cross-section as SVG: the potential, the slabs and the copper,
    /// `px` wide.
    pub fn svg(&self, px: f64, title: &str) -> String {
        use std::fmt::Write;
        let [x0, x1, y0, y1] = self.view();
        let k = px / (x1 - x0);
        let hpx = (y1 - y0) * k;
        let sx = |x: f64| (x - x0) * k;
        let sy = |y: f64| (y1 - y) * k;
        let mut o = String::new();
        let _ = writeln!(
            o,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {px:.0} {:.0}" font-family="Helvetica, Arial, sans-serif" font-size="12">"#,
            hpx + 24.0
        );
        let _ = writeln!(o, r##"<rect width="100%" height="100%" fill="#10131c"/>"##);
        o.push_str(r#"<g shape-rendering="crispEdges">"#);
        let (cols, rows) = (180usize, ((180.0 * hpx / px).ceil() as usize).max(8));
        let (cw, ch) = (px / cols as f64, hpx / rows as f64);
        for r in 0..rows {
            for c in 0..cols {
                let x = x0 + (c as f64 + 0.5) * cw / k;
                let y = y1 - (r as f64 + 0.5) * ch / k;
                let [cr, cg, cb] = Picture::color(self.at(x, y));
                let _ = write!(
                    o,
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="rgb({cr},{cg},{cb})"/>"#,
                    c as f64 * cw,
                    r as f64 * ch,
                    cw + 0.3,
                    ch + 0.3
                );
            }
        }
        o.push_str("</g>\n");
        for &(a, b, er) in &self.slabs {
            if b <= y1 && b > y0 {
                let _ = writeln!(
                    o,
                    r##"<line x1="0" x2="{px:.0}" y1="{:.1}" y2="{:.1}" stroke="#ffffff" stroke-opacity="0.35" stroke-dasharray="4 3"/>"##,
                    sy(b),
                    sy(b)
                );
            }
            let mid = (a + b) / 2.0;
            if er > 1.0 && mid < y1 && (b - a) * k > 12.0 {
                let _ = writeln!(
                    o,
                    r##"<text x="6" y="{:.1}" fill="#ffffff" fill-opacity="0.8">εr {er:.2}</text>"##,
                    sy(mid) + 4.0
                );
            }
        }
        for c in &self.copper {
            let (a, b) = (c[0].max(x0), c[1].min(x1));
            let _ = writeln!(
                o,
                r##"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="#d9822b" stroke="#ffd29a" stroke-width="0.8"/>"##,
                sx(a),
                sy(c[3].min(y1)),
                (sx(b) - sx(a)).max(0.5),
                ((c[3].min(y1) - c[2].max(y0)) * k).max(1.5)
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="6" y="{:.1}" fill="#e8ecf4">{}</text></svg>"##,
            hpx + 17.0,
            title
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        );
        o
    }
}

/// The cross-section to mesh: everything in mil, y up from the lower
/// reference plane.
struct Section {
    /// Slabs from the lower plane up: bottom, top, er, tan_d, whether
    /// the loss tangent was given.
    slabs: Vec<(f64, f64, f64, f64, bool)>,
    /// Bottom and top of the tracks.
    y0: f64,
    y1: f64,
    /// The upper plane, if any.
    top: Option<f64>,
    /// Solder mask over an outer layer: thickness, er, tan_d, given.
    mask: Option<(f64, f64, f64, bool)>,
    /// Tracks: x0, x1.
    tracks: Vec<(f64, f64)>,
    /// Where ground copper on the track's own layer starts, right of the
    /// tracks (mirrored on the left).
    ground: Option<f64>,
    /// Typical dielectric height, for sizing the mesh.
    h: f64,
}

impl Section {
    fn new(g: &LayerGeometry, w: f64, s: Option<f64>, side: Option<f64>) -> Option<Section> {
        if w.is_nan() || w <= 0.0 {
            return None;
        }
        let t = g.t.max(0.0);
        let fill = |list: &[Slab], h: f64| -> Vec<Slab> {
            if list.is_empty() {
                vec![[h, g.er, g.tan_d.unwrap_or(-1.0)]]
            } else {
                list.to_vec()
            }
        };
        // Reference below the track: for a bottom-layer microstrip, the
        // plane above, turned over.
        let (below, above) = match (g.above, g.below) {
            (Some(a), Some(b)) => (fill(&g.slabs_below, b), Some(fill(&g.slabs_above, a))),
            (None, Some(b)) => (fill(&g.slabs_below, b), None),
            (Some(a), None) => (fill(&g.slabs_above, a), None),
            (None, None) => return None,
        };
        let mut slabs = Vec::new();
        let mut y = 0.0;
        // `below` runs from the track down: lay it from the plane up.
        for sl in below.iter().rev() {
            let (h, er, td) = (sl[0].max(0.0), sl[1].max(1.0), sl[2]);
            slabs.push((y, y + h, er, td.max(0.0), td >= 0.0));
            y += h;
        }
        let y0 = y;
        let y1 = y0 + t;
        let mut top = None;
        let mut mask = None;
        if let Some(above) = &above {
            // The resin around the copper: the mean of the two sides.
            if t > 0.0 {
                let (a, b) = (below[0], above[0]);
                let er = (a[1] + b[1]) / 2.0;
                let td = if a[2] >= 0.0 && b[2] >= 0.0 {
                    (a[2] + b[2]) / 2.0
                } else {
                    -1.0
                };
                slabs.push((y0, y1, er.max(1.0), td.max(0.0), td >= 0.0));
            }
            let mut y = y1;
            for sl in above {
                let (h, er, td) = (sl[0].max(0.0), sl[1].max(1.0), sl[2]);
                slabs.push((y, y + h, er, td.max(0.0), td >= 0.0));
                y += h;
            }
            top = Some(y);
        } else if let Some(m) = g.mask {
            if m[0] > 0.0 {
                mask = Some((m[0], m[1].max(1.0), m[2].max(0.0), m[2] >= 0.0));
            }
        }
        let tracks = match s {
            Some(s) if s > 0.0 => vec![(-s / 2.0 - w, -s / 2.0), (s / 2.0, s / 2.0 + w)],
            Some(_) => return None,
            None => vec![(-w / 2.0, w / 2.0)],
        };
        let h = match top {
            Some(top) => top,
            None => y0,
        }
        .max(0.5);
        let ground = match side {
            Some(gap) if gap > 0.0 => Some(tracks.last().unwrap().1 + gap),
            Some(_) => return None,
            None => None,
        };
        Some(Section {
            slabs,
            y0,
            y1,
            top,
            mask,
            tracks,
            ground,
            h,
        })
    }

    /// Permittivity, loss tangent and whether it was given, at a point
    /// in dielectric or air.
    fn material(&self, x: f64, y: f64) -> (f64, f64, bool) {
        if let Some((tm, er, td, given)) = self.mask {
            let near = self
                .tracks
                .iter()
                .any(|&(a, b)| x > a - tm && x < b + tm && y < self.y1 + tm)
                || self
                    .ground
                    .is_some_and(|gx| x.abs() > gx - tm && y < self.y1 + tm);
            if y >= self.y0 && (y < self.y0 + tm || near) {
                return (er, td, given);
            }
        }
        for &(a, b, er, td, given) in &self.slabs {
            if y >= a && y < b {
                return (er, td, given);
            }
        }
        (1.0, 0.0, true)
    }
}

/// Points from `a` to `b` (both kept), cells `sa` wide at `a` and `sb` at
/// `b`, growing by `grow` but no wider than `max`.
fn grade(a: f64, sa: f64, b: f64, sb: f64, grow: f64, max: f64) -> Vec<f64> {
    let len = b - a;
    if len <= 0.0 {
        return vec![a];
    }
    let size = |x: f64| {
        (sa + (grow - 1.0) * (x - a))
            .min(sb + (grow - 1.0) * (b - x))
            .min(max)
            .max(1e-6)
    };
    let mut xs = vec![a];
    let mut x = a;
    while x < b {
        x += size(x);
        xs.push(x.min(b));
        if xs.len() > 4000 {
            break;
        }
    }
    // Stretch so the last point lands on b, dropping a sliver at the end.
    let n = xs.len();
    if n > 2 && b - xs[n - 2] < 0.3 * size(b) {
        xs.remove(n - 2);
    }
    let last = *xs.last().unwrap();
    xs.iter().map(|&v| a + (v - a) * len / (last - a)).collect()
}

fn axis(mut keys: Vec<(f64, f64)>, grow: f64, max: f64) -> Vec<f64> {
    keys.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut k: Vec<(f64, f64)> = Vec::new();
    for (x, s) in keys {
        match k.last_mut() {
            Some(l) if (x - l.0).abs() < 1e-6 => l.1 = l.1.min(s),
            _ => k.push((x, s)),
        }
    }
    let mut out = vec![k[0].0];
    for w in k.windows(2) {
        let pts = grade(w[0].0, w[0].1, w[1].0, w[1].1, grow, max);
        out.extend_from_slice(&pts[1..]);
    }
    out
}

/// The meshed problem.
struct Mesh {
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Per node: 0 free, 1 ground (planes and the box), 2 + k track k.
    kind: Vec<u8>,
    /// Edges: nodes, coefficient with dielectric, in air, the part of the
    /// coefficient times tan δ, the part in dielectric (er > 1), the part
    /// whose tan δ was given, and the dual face length (mil).
    edges: Vec<Edge>,
}

#[derive(Clone, Copy)]
struct Edge {
    a: u32,
    b: u32,
    k: f64,
    k0: f64,
    ktan: f64,
    kdiel: f64,
    kgiven: f64,
    face: f64,
}

/// Meshes the right half of the cross-section (it is symmetric about
/// x = 0); with `odd`, the middle is held at 0 V, otherwise no field
/// crosses it.
fn mesh(sec: &Section, odd: bool) -> Mesh {
    let h = sec.h;
    let (xl, xr) = (sec.tracks.first().unwrap().0, sec.tracks.last().unwrap().1);
    let w = sec
        .tracks
        .iter()
        .map(|t| t.1 - t.0)
        .fold(f64::MAX, f64::min);
    let gap = if sec.tracks.len() > 1 {
        sec.tracks[1].0 - sec.tracks[0].1
    } else {
        f64::MAX
    };
    let t = sec.y1 - sec.y0;
    let fine = (w / 16.0)
        .min(gap / 8.0)
        .min(sec.y0.max(0.2) / 8.0)
        .min(if t > 0.0 { t / 3.0 } else { f64::MAX })
        .clamp(0.01, 1.0)
        // The field is singular at copper edges: cells there 1/10 of the
        // above keep the impedance within about 0.7 % of exact values.
        * 0.1;
    let span = xr - xl;
    let reach = match sec.top {
        Some(_) => 6.0 * h + 2.0 * w,
        None => 14.0 * h + 3.0 * w,
    };
    let coarse = (h / 6.0).max(fine);
    let _ = xl;
    let first = sec
        .tracks
        .iter()
        .flat_map(|t| [t.0, t.1])
        .filter(|x| *x > 0.0)
        .fold(f64::MAX, f64::min);
    let mut kx = vec![
        (0.0, (first / 6.0).clamp(fine, coarse)),
        (xr + reach, coarse * 4.0),
    ];
    for &(a, b) in &sec.tracks {
        kx.push((a, fine));
        kx.push((b, fine));
        if let Some((tm, ..)) = sec.mask {
            kx.push((a - tm, fine * 2.0));
            kx.push((b + tm, fine * 2.0));
        }
    }
    if let Some(gx) = sec.ground {
        kx.push((gx, fine));
        if let Some((tm, ..)) = sec.mask {
            kx.push((gx - tm, fine * 2.0));
        }
    }
    kx.retain(|k| k.0 >= 0.0);
    let top = sec.top.unwrap_or(sec.y1 + (16.0 * h).max(3.0 * span));
    let mut ky = vec![(0.0, coarse), (top, coarse * 4.0)];
    for &(a, b, ..) in &sec.slabs {
        ky.push((a, coarse.min(fine * 4.0)));
        ky.push((b, coarse.min(fine * 4.0)));
    }
    ky.push((sec.y0, fine));
    ky.push((sec.y1, fine));
    if let Some((tm, ..)) = sec.mask {
        ky.push((sec.y0 + tm, fine * 2.0));
        ky.push((sec.y1 + tm, fine * 2.0));
    }
    if let Some(tp) = sec.top {
        ky.push((tp, coarse));
    }
    ky.retain(|k| k.0 >= 0.0 && k.0 <= top);
    let xs = axis(kx, 1.25, h.max(w));
    let ys = axis(ky, 1.25, (h / 2.0).max(fine));
    let (nx, ny) = (xs.len(), ys.len());
    let eps = 1e-7;
    let mut kind = vec![0u8; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let n = j * nx + i;
            let (x, y) = (xs[i], ys[j]);
            if j == 0 || j == ny - 1 {
                kind[n] = 1;
                continue;
            }
            if odd && i == 0 {
                kind[n] = 1;
            }
            for (k, &(a, b)) in sec.tracks.iter().enumerate() {
                if x >= a - eps && x <= b + eps && y >= sec.y0 - eps && y <= sec.y1 + eps {
                    kind[n] = 2 + k as u8;
                }
            }
            if let Some(gx) = sec.ground {
                if x >= gx - eps && y >= sec.y0 - eps && y <= sec.y1 + eps {
                    kind[n] = 1;
                }
            }
        }
    }
    let mut cell = vec![(1.0, 0.0, false); (nx - 1) * (ny - 1)];
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            cell[j * (nx - 1) + i] =
                sec.material((xs[i] + xs[i + 1]) / 2.0, (ys[j] + ys[j + 1]) / 2.0);
        }
    }
    let mut edges = Vec::with_capacity(2 * nx * ny);
    let mut add = |a: usize, b: usize, len: f64, parts: &[(f64, (f64, f64, bool))]| {
        if kind[a] != 0 && kind[b] != 0 {
            return;
        }
        let mut e = Edge {
            a: a as u32,
            b: b as u32,
            k: 0.0,
            k0: 0.0,
            ktan: 0.0,
            kdiel: 0.0,
            kgiven: 0.0,
            face: 0.0,
        };
        for &(d, (er, td, given)) in parts {
            let g = d / 2.0 / len;
            e.k += er * g;
            e.k0 += g;
            e.ktan += er * td * g;
            if er > 1.0 {
                e.kdiel += er * g;
                if given {
                    e.kgiven += er * g;
                }
            }
            e.face += d / 2.0;
        }
        edges.push(e);
    };
    for j in 0..ny {
        for i in 0..nx {
            let n = j * nx + i;
            if i + 1 < nx {
                let mut parts = Vec::with_capacity(2);
                if j > 0 {
                    parts.push((ys[j] - ys[j - 1], cell[(j - 1) * (nx - 1) + i]));
                }
                if j + 1 < ny {
                    parts.push((ys[j + 1] - ys[j], cell[j * (nx - 1) + i]));
                }
                add(n, n + 1, xs[i + 1] - xs[i], &parts);
            }
            if j + 1 < ny {
                let mut parts = Vec::with_capacity(2);
                if i > 0 {
                    parts.push((xs[i] - xs[i - 1], cell[j * (nx - 1) + i - 1]));
                }
                if i + 1 < nx {
                    parts.push((xs[i + 1] - xs[i], cell[j * (nx - 1) + i]));
                }
                add(n, n + nx, ys[j + 1] - ys[j], &parts);
            }
        }
    }
    Mesh {
        xs,
        ys,
        kind,
        edges,
    }
}

/// Potentials with track k at `v[k]` and ground at 0, solving with the
/// coefficients `air` or with the dielectric: conjugate gradients with a
/// symmetric Gauss-Seidel preconditioner.
fn solve(m: &Mesh, v: &[f64], air: bool) -> Vec<f64> {
    let n = m.kind.len();
    let fixed = |i: usize| match m.kind[i] {
        0 => None,
        1 => Some(0.0),
        k => Some(v[(k - 2) as usize]),
    };
    let mut phi: Vec<f64> = (0..n).map(|i| fixed(i).unwrap_or(0.0)).collect();
    // Unknowns are the free nodes, numbered in node order.
    let mut id = vec![u32::MAX; n];
    let mut nodes = Vec::new();
    for i in 0..n {
        if m.kind[i] == 0 {
            id[i] = nodes.len() as u32;
            nodes.push(i);
        }
    }
    let u = nodes.len();
    let mut diag = vec![0.0; u];
    let mut rhs = vec![0.0; u];
    let mut nb: Vec<Vec<(u32, f64)>> = vec![Vec::new(); u];
    for e in &m.edges {
        let (a, b) = (e.a as usize, e.b as usize);
        let k = if air { e.k0 } else { e.k };
        match (fixed(a), fixed(b)) {
            (None, None) => {
                let (ia, ib) = (id[a] as usize, id[b] as usize);
                diag[ia] += k;
                diag[ib] += k;
                nb[ia].push((ib as u32, k));
                nb[ib].push((ia as u32, k));
            }
            (None, Some(vb)) => {
                diag[id[a] as usize] += k;
                rhs[id[a] as usize] += k * vb;
            }
            (Some(va), None) => {
                diag[id[b] as usize] += k;
                rhs[id[b] as usize] += k * va;
            }
            _ => {}
        }
    }
    // Compressed rows: neighbours below each node, then above.
    let mut start = vec![0usize; u + 1];
    let mut mid = vec![0usize; u];
    let mut col: Vec<u32> = Vec::new();
    let mut val: Vec<f64> = Vec::new();
    for i in 0..u {
        nb[i].sort_by_key(|e| e.0);
        start[i] = col.len();
        mid[i] = col.len() + nb[i].iter().filter(|e| (e.0 as usize) < i).count();
        for &(j, k) in &nb[i] {
            col.push(j);
            val.push(k);
        }
    }
    start[u] = col.len();
    drop(nb);
    let inv: Vec<f64> = diag.iter().map(|d| 1.0 / d).collect();
    let apply = |x: &[f64], y: &mut [f64]| {
        for i in 0..u {
            let mut s = diag[i] * x[i];
            for e in start[i]..start[i + 1] {
                s -= val[e] * x[col[e] as usize];
            }
            y[i] = s;
        }
    };
    // z = M⁻¹ r with M = (D − L) D⁻¹ (D − U).
    let pre = |r: &[f64], z: &mut [f64]| {
        for i in 0..u {
            let mut s = r[i];
            for e in start[i]..mid[i] {
                s += val[e] * z[col[e] as usize];
            }
            z[i] = s * inv[i];
        }
        for i in (0..u).rev() {
            let mut s = 0.0;
            for e in mid[i]..start[i + 1] {
                s += val[e] * z[col[e] as usize];
            }
            z[i] += s * inv[i];
        }
    };
    let mut x = vec![0.0; u];
    let mut r = rhs.clone();
    let mut z = vec![0.0; u];
    pre(&r, &mut z);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let norm0 = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut q = vec![0.0; u];
    for _ in 0..20_000 {
        apply(&p, &mut q);
        let pq: f64 = p.iter().zip(&q).map(|(a, b)| a * b).sum();
        if pq <= 0.0 {
            break;
        }
        let alpha = rz / pq;
        let mut rn = 0.0;
        for i in 0..u {
            x[i] += alpha * p[i];
            r[i] -= alpha * q[i];
            rn += r[i] * r[i];
        }
        if rn.sqrt() < 1e-10 * norm0 {
            break;
        }
        pre(&r, &mut z);
        let rz2: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz2 / rz;
        rz = rz2;
        for i in 0..u {
            p[i] = z[i] + beta * p[i];
        }
    }
    for (k, &i) in nodes.iter().enumerate() {
        phi[i] = x[k];
    }
    phi
}

/// Sums over the edges of a and b's potential differences, weighted by
/// `f(edge)`.
fn energy(m: &Mesh, a: &[f64], b: &[f64], f: impl Fn(&Edge) -> f64) -> f64 {
    m.edges
        .iter()
        .map(|e| {
            let (i, j) = (e.a as usize, e.b as usize);
            f(e) * (a[i] - a[j]) * (b[i] - b[j])
        })
        .sum()
}

/// How far Wheeler's rule moves the copper's faces, mil.
const RECEDE: f64 = 0.05;

/// The same layer with every copper face moved `d` mil into its metal,
/// for Wheeler's incremental inductance rule (in air, so the slabs go).
fn receded(g: &LayerGeometry, d: f64) -> LayerGeometry {
    LayerGeometry {
        t: (g.t - 2.0 * d).max(0.0),
        above: g.above.map(|a| a + 2.0 * d),
        below: g.below.map(|b| b + 2.0 * d),
        slabs_above: Vec::new(),
        slabs_below: Vec::new(),
        mask: None,
        ..g.clone()
    }
}

/// Skin resistance at 1 GHz from Wheeler's rule: Rs/µ0 · dL/dn, with L
/// before and after the faces recede `RECEDE`.
fn wheeler(l: f64, l_receded: f64) -> f64 {
    let rs = (PI * 1e9 * MU0 * RHO).sqrt();
    (rs / MU0 * (l_receded - l) / (RECEDE * MIL)).max(0.0)
}

/// Inductance per metre of the air solution `phi0` on a half mesh,
/// counting `halves` of it (2 for a whole line, 1 for one line of a
/// pair's mode).
fn inductance(m: &Mesh, phi0: &[f64], halves: f64) -> f64 {
    1.0 / (C0 * C0 * halves * E0 * energy(m, phi0, phi0, |e| e.k0))
}

/// The line from its half-mesh solutions; `l_receded` is its inductance
/// with the copper's faces receded.
fn line_of(m: &Mesh, phi: &[f64], phi0: &[f64], w: f64, t: f64, l_receded: f64) -> Line {
    let c = 2.0 * E0 * energy(m, phi, phi, |e| e.k);
    let c0 = 2.0 * E0 * energy(m, phi0, phi0, |e| e.k0);
    let w_all = energy(m, phi, phi, |e| e.k);
    let tan = energy(m, phi, phi, |e| e.ktan) / w_all;
    let fill = energy(m, phi, phi, |e| e.kdiel) / w_all;
    let fill_given = energy(m, phi, phi, |e| e.kgiven) / w_all;
    let r_ghz = wheeler(1.0 / (C0 * C0 * c0), l_receded);
    let z0 = 1.0 / (C0 * (c * c0).sqrt());
    Line {
        z0,
        eeff: c / c0,
        l: 1.0 / (C0 * C0 * c0),
        c,
        r_ghz,
        r_dc: if t > 0.0 {
            RHO / (w * t * MIL * MIL)
        } else {
            0.0
        },
        fill,
        tan_eff: tan,
        fill_given,
    }
}

fn single_uncached(g: &LayerGeometry, w: f64, side: Option<f64>) -> Option<(Line, Picture)> {
    let sec = Section::new(g, w, None, side)?;
    let m = mesh(&sec, false);
    let phi = solve(&m, &[1.0], false);
    let phi0 = solve(&m, &[1.0], true);
    let gr = receded(g, RECEDE);
    let wr = (w - 2.0 * RECEDE).max(w / 2.0);
    let mr = mesh(
        &Section::new(&gr, wr, None, side.map(|g| g + (w - wr) / 2.0 + RECEDE))?,
        false,
    );
    let lr = inductance(&mr, &solve(&mr, &[1.0], true), 2.0);
    let line = line_of(&m, &phi, &phi0, w, g.t, lr);
    line.z0
        .is_finite()
        .then(|| (line, picture(&sec, &m, phi, false)))
}

fn pair_uncached(g: &LayerGeometry, w: f64, s: f64, side: Option<f64>) -> Option<(Pair, Picture)> {
    let sec = Section::new(g, w, Some(s), side)?;
    // Odd and even modes on the right half: the right track at 1 V, the
    // middle at 0 V or left free.
    let (mo, me) = (mesh(&sec, true), mesh(&sec, false));
    let po = solve(&mo, &[-1.0, 1.0], false);
    let pe = solve(&me, &[1.0, 1.0], false);
    let po0 = solve(&mo, &[-1.0, 1.0], true);
    let pe0 = solve(&me, &[1.0, 1.0], true);
    // A mode's energy over both halves, per line: the half's.
    let co = E0 * energy(&mo, &po, &po, |e| e.k);
    let ce = E0 * energy(&me, &pe, &pe, |e| e.k);
    let co0 = E0 * energy(&mo, &po0, &po0, |e| e.k0);
    let ce0 = E0 * energy(&me, &pe0, &pe0, |e| e.k0);
    let z_odd = 1.0 / (C0 * (co * co0).sqrt());
    let z_even = 1.0 / (C0 * (ce * ce0).sqrt());
    let (e_odd, e_even) = (co / co0, ce / ce0);
    let (kb, kf) = crate::sim::xtalk::coefficients(z_even, z_odd, e_even, e_odd);
    // Odd mode skin loss per line, by Wheeler's rule.
    let gr = receded(g, RECEDE);
    let mr = mesh(
        &Section::new(
            &gr,
            (w - 2.0 * RECEDE).max(w / 2.0),
            Some(s + 2.0 * RECEDE),
            side.map(|g| g + 2.0 * RECEDE),
        )?,
        true,
    );
    let r_odd_ghz = wheeler(
        inductance(&mo, &po0, 1.0),
        inductance(&mr, &solve(&mr, &[-1.0, 1.0], true), 1.0),
    );
    let wo = energy(&mo, &po, &po, |e| e.k);
    let tan_odd = energy(&mo, &po, &po, |e| e.ktan) / wo;
    let single = single_with(g, w, side)?;
    let p = Pair {
        z_odd,
        z_even,
        e_odd,
        e_even,
        zdiff: 2.0 * z_odd,
        zcomm: z_even / 2.0,
        kb,
        kf,
        r_odd_ghz,
        tan_odd,
        single,
    };
    (p.z_odd.is_finite() && p.z_even.is_finite()).then(|| (p, picture(&sec, &mo, po, true)))
}

/// The whole cross-section from the right half, mirrored (the potential
/// turned over too when `odd`).
fn picture(sec: &Section, m: &Mesh, phi: Vec<f64>, odd: bool) -> Picture {
    let nx = m.xs.len();
    let mut xs: Vec<f64> = m.xs[1..].iter().rev().map(|x| -x).collect();
    xs.extend_from_slice(&m.xs);
    let sign = if odd { -1.0 } else { 1.0 };
    let mut full = Vec::with_capacity(xs.len() * m.ys.len());
    for row in phi.chunks(nx) {
        full.extend(row[1..].iter().rev().map(|v| sign * v));
        full.extend_from_slice(row);
    }
    let phi = full;
    let (x0, x1) = (xs[0], *xs.last().unwrap());
    let ytop = *m.ys.last().unwrap();
    let mut copper: Vec<[f64; 4]> = sec
        .tracks
        .iter()
        .map(|&(a, b)| [a, b, sec.y0, sec.y1])
        .collect();
    if let Some(gx) = sec.ground {
        copper.push([-x1, -gx, sec.y0, sec.y1.max(sec.y0 + 0.2)]);
        copper.push([gx, x1, sec.y0, sec.y1.max(sec.y0 + 0.2)]);
    }
    let pl = (sec.h / 20.0).max(0.3);
    copper.push([x0, x1, -pl, 0.0]);
    if let Some(t) = sec.top {
        copper.push([x0, x1, t, t + pl]);
    }
    let mut slabs: Vec<(f64, f64, f64)> = sec.slabs.iter().map(|s| (s.0, s.1, s.2)).collect();
    if sec.top.is_none() {
        if let Some((tm, er, ..)) = sec.mask {
            slabs.push((sec.y0, sec.y0 + tm, er));
        }
        slabs.push((sec.y0, ytop, 1.0));
    }
    Picture {
        xs,
        ys: m.ys.clone(),
        phi,
        slabs,
        copper,
        tracks: sec.tracks.len(),
        upper: sec.top,
    }
}

fn key(g: &LayerGeometry, w: f64, s: f64, side: Option<f64>) -> String {
    format!(
        "{:.4}|{:?}|{:?}|{:.4}|{:?}|{:?}|{:?}|{:?}|{:.4}|{:.4}|{:?}",
        g.t,
        g.above,
        g.below,
        g.er,
        g.tan_d,
        g.slabs_above,
        g.slabs_below,
        g.mask,
        w,
        s,
        side.map(|x| (x * 1e4).round())
    )
}

type Cache<T> = Mutex<HashMap<String, Option<(T, std::sync::Arc<Picture>)>>>;
static SINGLE: std::sync::OnceLock<Cache<Line>> = std::sync::OnceLock::new();
static PAIRS: std::sync::OnceLock<Cache<Pair>> = std::sync::OnceLock::new();

fn cached<T: Clone>(
    cache: &std::sync::OnceLock<Cache<T>>,
    k: String,
    f: impl FnOnce() -> Option<(T, Picture)>,
) -> Option<(T, std::sync::Arc<Picture>)> {
    let c = cache.get_or_init(Default::default);
    if let Some(v) = c.lock().ok().and_then(|m| m.get(&k).cloned()) {
        return v;
    }
    let v = f().map(|(t, p)| (t, std::sync::Arc::new(p)));
    if let Ok(mut m) = c.lock() {
        if m.len() > 20_000 {
            m.clear();
        }
        m.insert(k, v.clone());
    }
    v
}

/// A track `w` mil wide on layer `g`, solved (and kept, so asking again
/// is free).
pub fn single(g: &LayerGeometry, w: f64) -> Option<Line> {
    single_with(g, w, None)
}

/// A track with ground copper `side` mil from each edge on its own layer
/// (coplanar waveguide with ground), or none.
pub fn single_with(g: &LayerGeometry, w: f64, side: Option<f64>) -> Option<Line> {
    single_picture(g, w, side).map(|v| v.0)
}

/// As [`single_with`], with the potential for drawing.
pub fn single_picture(
    g: &LayerGeometry,
    w: f64,
    side: Option<f64>,
) -> Option<(Line, std::sync::Arc<Picture>)> {
    cached(&SINGLE, key(g, w, 0.0, side), || {
        single_uncached(g, w, side)
    })
}

/// A track with pour copper `sides[0]` mil from one edge and `sides[1]`
/// from the other (either may be missing): the symmetric solutions with
/// each side's gap averaged, which holds while the two sides' fields
/// overlap little.
pub fn single_sides(g: &LayerGeometry, w: f64, sides: [Option<f64>; 2]) -> Option<Line> {
    match sides {
        [None, None] => single(g, w),
        [Some(a), Some(b)] if (a - b).abs() < 0.05 => single_with(g, w, Some(a)),
        [a, b] => Some(blend(&single_with(g, w, a)?, &single_with(g, w, b)?)),
    }
}

/// One gap for a symmetric solve from two sides' gaps: their mean when
/// both are there, none otherwise.
pub fn both(sides: [Option<f64>; 2]) -> Option<f64> {
    match sides {
        [Some(a), Some(b)] => Some(((a + b) / 2.0 * 20.0).round() / 20.0),
        _ => None,
    }
}

/// The line between two others, as each side of a track takes after one:
/// capacitances (with dielectric and in air) and losses averaged.
fn blend(a: &Line, b: &Line) -> Line {
    let c0 = |l: &Line| 1.0 / (C0 * C0 * l.l);
    let (c, c_air) = ((a.c + b.c) / 2.0, (c0(a) + c0(b)) / 2.0);
    let mean = |x: f64, y: f64| (x + y) / 2.0;
    Line {
        z0: 1.0 / (C0 * (c * c_air).sqrt()),
        eeff: c / c_air,
        l: 1.0 / (C0 * C0 * c_air),
        c,
        r_ghz: mean(a.r_ghz, b.r_ghz),
        r_dc: a.r_dc,
        fill: mean(a.fill, b.fill),
        tan_eff: mean(a.tan_eff, b.tan_eff),
        fill_given: mean(a.fill_given, b.fill_given),
    }
}

/// Two tracks `w` mil wide with edge gap `s` mil on layer `g`, solved.
pub fn pair(g: &LayerGeometry, w: f64, s: f64) -> Option<Pair> {
    pair_with(g, w, s, None)
}

/// A pair with ground copper `side` mil outside each track.
pub fn pair_with(g: &LayerGeometry, w: f64, s: f64, side: Option<f64>) -> Option<Pair> {
    pair_picture(g, w, s, side).map(|v| v.0)
}

/// As [`pair_with`], with the potential of the odd mode for drawing.
pub fn pair_picture(
    g: &LayerGeometry,
    w: f64,
    s: f64,
    side: Option<f64>,
) -> Option<(Pair, std::sync::Arc<Picture>)> {
    cached(&PAIRS, key(g, w, s, side), || pair_uncached(g, w, s, side))
}

/// Finds x in [lo, hi] where `f` (falling as x grows) meets `target`,
/// by secant steps on the log of x, bracketed.
pub fn solve_for(f: impl Fn(f64) -> Option<f64>, target: f64, lo: f64, hi: f64) -> Option<f64> {
    let (mut a, mut b) = (lo.ln(), hi.ln());
    let (mut fa, mut fb) = (f(lo)? - target, f(hi)? - target);
    if fa.signum() == fb.signum() {
        return None;
    }
    for _ in 0..40 {
        // Illinois-style false position.
        let c = (a * fb - b * fa) / (fb - fa);
        let fc = f(c.exp())? - target;
        if fc.abs() < 1e-3 || (b - a).abs() < 1e-5 {
            return Some(c.exp());
        }
        if fc.signum() == fb.signum() {
            fa /= 2.0;
        } else {
            a = b;
            fa = fb;
        }
        b = c;
        fb = fc;
    }
    Some(((a + b) / 2.0).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::impedance::LayerGeometry;

    fn layer(above: Option<f64>, below: Option<f64>, t: f64) -> LayerGeometry {
        LayerGeometry {
            copper: 1,
            name: "L".into(),
            t,
            above,
            below,
            er: 4.2,
            tan_d: Some(0.02),
            rough_um: None,
            slabs_above: Vec::new(),
            slabs_below: Vec::new(),
            mask: None,
        }
    }

    /// Exact impedance of a thin centred stripline (Cohn).
    fn thin_stripline(w: f64, b: f64, er: f64) -> f64 {
        let k = 1.0 / (PI * w / (2.0 * b)).cosh();
        let kp = (1.0 - k * k).sqrt();
        // K(k)/K(k') by the AGM.
        let agm = |mut a: f64, mut g: f64| {
            for _ in 0..30 {
                let (a2, g2) = ((a + g) / 2.0, (a * g).sqrt());
                a = a2;
                g = g2;
            }
            a
        };
        // K(k)/K(k') = AGM(1, k)/AGM(1, k').
        let ratio = agm(1.0, k) / agm(1.0, kp);
        30.0 * PI / er.sqrt() * ratio
    }

    #[test]
    fn thin_stripline_matches_the_exact_value() {
        for (w, b) in [(5.0, 10.0), (10.0, 20.0), (3.0, 16.0)] {
            let g = layer(Some(b / 2.0), Some(b / 2.0), 0.0);
            let z = single(&g, w).unwrap().z0;
            let exact = thin_stripline(w, b, 4.2);
            assert!((z - exact).abs() < 0.01 * exact, "{w} {b}: {z} vs {exact}");
            let l = single(&g, w).unwrap();
            assert!((l.eeff - 4.2).abs() < 1e-6);
            assert!((l.ps_per_in() - 173.6).abs() < 1.0, "{}", l.ps_per_in());
        }
    }

    #[test]
    fn thin_microstrip_matches_hammerstad_and_jensen() {
        for (w, h) in [(10.0, 5.0), (5.0, 5.0), (20.0, 10.0)] {
            let g = layer(None, Some(h), 0.0);
            let l = single(&g, w).unwrap();
            let z = crate::impedance::microstrip(w, 0.0, h, 4.2);
            let ee = crate::impedance::microstrip_eeff(w, h, 4.2);
            assert!((l.z0 - z).abs() < 0.02 * z, "{w} {h}: {} vs {z}", l.z0);
            assert!((l.eeff - ee).abs() < 0.03 * ee, "{} vs {ee}", l.eeff);
        }
    }

    #[test]
    fn thin_coupled_stripline_matches_cohn() {
        let g = layer(Some(5.0), Some(5.0), 0.0);
        let p = pair(&g, 5.0, 5.0).unwrap();
        let (ze, zo, ..) = crate::sim::xtalk::modes(&g, 5.0, 5.0).unwrap();
        assert!((p.z_odd - zo).abs() < 0.02 * zo, "{} {zo}", p.z_odd);
        assert!((p.z_even - ze).abs() < 0.02 * ze, "{} {ze}", p.z_even);
        assert!(p.kf.abs() < 1e-3 && p.kb > 0.02, "{} {}", p.kf, p.kb);
    }

    #[test]
    fn microstrip_pair_has_negative_far_end_coupling() {
        let g = layer(None, Some(4.0), 1.4);
        let p = pair(&g, 5.0, 5.0).unwrap();
        assert!(
            p.kf < 0.0 && p.e_odd < p.e_even,
            "{} {} {}",
            p.kf,
            p.e_odd,
            p.e_even
        );
        assert!(p.zdiff > 60.0 && p.zdiff < 2.0 * p.single.z0);
    }

    #[test]
    fn thickness_and_mask_lower_the_impedance() {
        let thin = single(&layer(None, Some(5.0), 0.0), 8.0).unwrap().z0;
        let thick = single(&layer(None, Some(5.0), 2.0), 8.0).unwrap().z0;
        let mut g = layer(None, Some(5.0), 2.0);
        g.mask = Some([0.8, 3.5, 0.02]);
        let masked = single(&g, 8.0).unwrap().z0;
        assert!(thick < thin && masked < thick, "{thin} {thick} {masked}");
        assert!(thin - masked < 0.15 * thin);
    }

    #[test]
    fn losses_are_in_the_expected_range() {
        // 5 mil stripline, 5 mil to each plane, 1 GHz: copper about
        // 0.05-0.08 dB/in, FR-4 dielectric about 0.14 dB/in.
        let g = layer(Some(5.0), Some(5.0), 1.4);
        let l = single(&g, 5.0).unwrap();
        let rs = (PI * 1e9 * MU0 * RHO).sqrt();
        let geom = rs * (1.0 / (2.0 * 5.0) + 1.0 / (2.0 * (5.0 + 2.0 * PI * 5.0))) / MIL;
        assert!(
            l.r_ghz > 0.6 * geom && l.r_ghz < 1.6 * geom,
            "{} {geom}",
            l.r_ghz
        );
        assert!((l.tan_eff - 0.02).abs() < 1e-9 && (l.fill - 1.0).abs() < 1e-9);
        let db = l.db_per_in(1e9, 0.0, None);
        assert!(db > 0.15 && db < 0.3, "{db}");
    }

    /// Coplanar waveguide without a lower plane far below approaches the
    /// conformal-mapping value; with ground beside it the impedance falls.
    #[test]
    fn side_ground_lowers_the_impedance_as_cpw_formulas_say() {
        // CPWG, w 10, gap 5, h 10 mil, er 4.2, thin: conformal mapping
        // (Wadell) gives about 52 Ω.
        let g = layer(None, Some(10.0), 0.0);
        let cpw = single_with(&g, 10.0, Some(5.0)).unwrap();
        let ms = single(&g, 10.0).unwrap();
        let k = 10.0 / 20.0;
        let k3 = (PI * 10.0 / 40.0).tanh() / (PI * 20.0 / 40.0).tanh();
        let kp = |k: f64| (1.0 - k * k).sqrt();
        let agm = |mut a: f64, mut b: f64| {
            for _ in 0..30 {
                let (a2, b2) = ((a + b) / 2.0, (a * b).sqrt());
                a = a2;
                b = b2;
            }
            a
        };
        // K(k)/K(k') = AGM(1, k)/AGM(1, k').
        let r = |k: f64| agm(1.0, k) / agm(1.0, kp(k));
        let ee = (1.0 + 4.2 * r(k3) / r(k)) / (1.0 + r(k3) / r(k));
        let z = 60.0 * PI / ee.sqrt() / (r(k) + r(k3));
        assert!(cpw.z0 < ms.z0, "{} {}", cpw.z0, ms.z0);
        assert!((cpw.z0 - z).abs() < 0.03 * z, "{} vs {z}", cpw.z0);
        assert!((cpw.eeff - ee).abs() < 0.04 * ee, "{} vs {ee}", cpw.eeff);
    }

    #[test]
    fn pour_on_one_side_lands_between_none_and_both() {
        let g = layer(None, Some(10.0), 1.4);
        let none = single(&g, 10.0).unwrap().z0;
        let both = single_with(&g, 10.0, Some(5.0)).unwrap().z0;
        let one = single_sides(&g, 10.0, [Some(5.0), None]).unwrap().z0;
        assert!(both < one && one < none, "{both} {one} {none}");
    }

    #[test]
    fn solve_for_finds_the_width() {
        let g = layer(Some(5.0), Some(5.0), 1.4);
        let w = solve_for(|w| single(&g, w).map(|l| l.z0), 50.0, 0.5, 100.0).unwrap();
        let z = single(&g, w).unwrap().z0;
        assert!((z - 50.0).abs() < 0.1, "{w} {z}");
    }
}
