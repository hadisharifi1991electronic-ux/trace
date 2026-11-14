//! 3D full-wave solver for via transitions: the finite-difference
//! time-domain (FDTD) method on a graded Yee grid.
//!
//! A signal via (or a pair) carries a trace on one copper layer to a trace
//! on another. Planes are thin perfect conductors with a round antipad
//! around each signal via; return vias join all planes; the barrel runs
//! from the top to the bottom copper (or only between the two layers when
//! backdrilled), so the stub below is kept. The dielectric slabs keep the
//! stack-up's Er (and Df, as a conductivity at mid band). The space around
//! is closed by convolutional PML, so the planes act as an endless board.
//!
//! Each trace ends in a resistor of the reference impedance to its
//! reference plane; the first is driven by a Gaussian pulse. Voltage and
//! current are sampled on both traces some way from the via, turned into
//! power waves, Fourier transformed while stepping, and the reference
//! planes are moved to the via's centre with each trace's delay from the
//! 2D field solver.

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use crate::impedance::LayerGeometry;
use crate::{LayerKind, Stackup};

const C0: f64 = 299_792_458.0;
const EPS0: f64 = 8.854_187_812_8e-12;
const MU0: f64 = 1.256_637_062_12e-6;
const MIL: f64 = 25.4e-6;

/// A complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cx {
    pub re: f64,
    pub im: f64,
}

#[allow(clippy::should_implement_trait)]
impl Cx {
    pub fn new(re: f64, im: f64) -> Cx {
        Cx { re, im }
    }
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn db(self) -> f64 {
        20.0 * self.abs().max(1e-12).log10()
    }
    pub fn deg(self) -> f64 {
        self.im.atan2(self.re).to_degrees()
    }
    pub fn mul(self, o: Cx) -> Cx {
        Cx::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    pub fn div(self, o: Cx) -> Cx {
        let d = o.re * o.re + o.im * o.im;
        Cx::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    pub fn add(self, o: Cx) -> Cx {
        Cx::new(self.re + o.re, self.im + o.im)
    }
    pub fn scale(self, k: f64) -> Cx {
        Cx::new(self.re * k, self.im * k)
    }
    pub fn expj(phi: f64) -> Cx {
        Cx::new(phi.cos(), phi.sin())
    }
}

/// The via transition to solve. Lengths in mil.
#[derive(Clone, Debug)]
pub struct ViaSetup {
    /// Copper layers (0 = top) of the incoming and outgoing traces.
    pub from: u8,
    pub to: u8,
    pub drill: f64,
    pub pad: f64,
    /// Diameter of the clearance hole in the planes.
    pub antipad: f64,
    /// For a pair, one oval clearance around both vias (the antipad's
    /// width, stretched along the pair) instead of a round one each.
    pub antipad_oval: bool,
    /// Trace widths on the two layers (None: 50 Ω from the field solver).
    pub w_from: Option<f64>,
    pub w_to: Option<f64>,
    /// A differential pair: distance between the two vias' centres.
    pub pair: Option<f64>,
    /// Return (ground) vias: how many, and their distance from the signal
    /// via's centre (for a pair, from each via, beside the pair).
    pub returns: usize,
    pub return_dist: f64,
    /// Keep the barrel only between `from` and `to` (no stub).
    pub backdrill: bool,
    /// Highest frequency, GHz.
    pub fmax_ghz: f64,
    pub points: usize,
    /// Smallest cell, mil (0: chosen from the drill and the dielectrics).
    pub cell: f64,
    /// Use the stack-up's Df, as a conductivity fixed at mid band (which
    /// overstates the loss at low frequencies; off by default, as the
    /// via's own dielectric loss is small beside the traces').
    pub lossy: bool,
}

impl ViaSetup {
    pub fn new(from: u8, to: u8) -> ViaSetup {
        ViaSetup {
            from,
            to,
            drill: 8.0,
            pad: 18.0,
            antipad: 30.0,
            antipad_oval: false,
            w_from: None,
            w_to: None,
            pair: None,
            returns: 2,
            return_dist: 30.0,
            backdrill: false,
            fmax_ghz: 40.0,
            points: 400,
            cell: 0.0,
            lossy: false,
        }
    }
}

/// The solved transition.
#[derive(Clone, Debug)]
pub struct ViaResult {
    /// Frequencies, GHz.
    pub f: Vec<f64>,
    /// Return and insertion loss with reference planes at the via
    /// (for a pair, the differential Sdd11 and Sdd21).
    pub s11: Vec<Cx>,
    pub s21: Vec<Cx>,
    /// Reference impedance, Ω (50, or 100 for a pair).
    pub z_ref: f64,
    /// Trace widths used and their impedance (single-ended), mil and Ω.
    pub w: [f64; 2],
    pub z0: [f64; 2],
    /// Each trace's impedance as the 3D grid measures it (from its two
    /// probes, at a quarter of the band; differential for a pair), Ω.
    pub z_line: [f64; 2],
    /// Length of the stub below the outgoing layer, mil.
    pub stub: f64,
    pub cells: usize,
    pub steps: usize,
    pub notes: Vec<String>,
}

impl ViaResult {
    /// The deepest dip in |S21| (a stub resonance): GHz and dB, when it
    /// falls more than 3 dB below the trend around it.
    pub fn notch(&self) -> Option<(f64, f64)> {
        let db: Vec<f64> = self.s21.iter().map(|s| s.db()).collect();
        let n = db.len();
        let mut best: Option<(usize, f64)> = None;
        for i in 2..n.saturating_sub(2) {
            if db[i] <= db[i - 1] && db[i] <= db[i + 1] {
                // Depth below the higher of the shoulders within ±25 %.
                let span = (n / 8).max(3);
                let lo = db[i.saturating_sub(span)..i]
                    .iter()
                    .cloned()
                    .fold(f64::MIN, f64::max);
                let hi = db[i + 1..(i + span).min(n)]
                    .iter()
                    .cloned()
                    .fold(f64::MIN, f64::max);
                let depth = lo.min(hi) - db[i];
                if depth > 3.0 && best.is_none_or(|b| depth > b.1) {
                    best = Some((i, depth));
                }
            }
        }
        // Between samples: the vertex of a parabola through the three
        // points around the lowest.
        best.map(|(i, _)| {
            let (a, b, c) = (db[i - 1], db[i], db[i + 1]);
            let den = a - 2.0 * b + c;
            let t = if den.abs() > 1e-12 {
                (0.5 * (a - c) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let df = if t >= 0.0 {
                self.f[i + 1] - self.f[i]
            } else {
                self.f[i] - self.f[i - 1]
            };
            (self.f[i] + t * df, b - 0.25 * (a - c) * t)
        })
    }

    /// The S-parameters as a Touchstone 2-port file (magnitude and angle).
    pub fn touchstone(&self, title: &str) -> String {
        use std::fmt::Write;
        let mut o = String::new();
        let _ = writeln!(o, "! {title}");
        let _ = writeln!(
            o,
            "! Trace 3D FDTD via solver; reference planes at the via centre"
        );
        if self.z_ref > 60.0 {
            let _ = writeln!(o, "! Differential (mixed-mode Sdd) as a 2-port");
        }
        let _ = writeln!(o, "# GHz S MA R {}", self.z_ref);
        for (k, f) in self.f.iter().enumerate() {
            let (a, b) = (self.s11[k], self.s21[k]);
            let _ = writeln!(
                o,
                "{f:.6} {:.6e} {:.3} {:.6e} {:.3} {:.6e} {:.3} {:.6e} {:.3}",
                a.abs(),
                a.deg(),
                b.abs(),
                b.deg(),
                b.abs(),
                b.deg(),
                a.abs(),
                a.deg()
            );
        }
        o
    }

    /// |S11| and |S21| in dB against frequency, as SVG.
    pub fn svg(&self, title: &str) -> String {
        use std::fmt::Write;
        let (w, h, l, r, t, b) = (900.0, 520.0, 60.0, 20.0, 40.0, 50.0);
        let fmax = self.f.last().copied().unwrap_or(1.0);
        let min_db = self
            .s11
            .iter()
            .chain(&self.s21)
            .map(|s| s.db())
            .fold(0.0f64, f64::min)
            .max(-60.0);
        let lo = (min_db / 10.0).floor() * 10.0;
        let x = |f: f64| l + (w - l - r) * f / fmax;
        let y = |d: f64| t + (h - t - b) * (d.min(0.0) / lo);
        let mut o = String::new();
        let _ = writeln!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="Helvetica, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#ffffff"/>"##
        );
        let mut d = 0.0;
        while d >= lo - 1e-9 {
            let _ = writeln!(
                o,
                r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#e3e6ec"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{d:.0}</text>"##,
                w - r,
                y(d),
                y(d),
                l - 6.0,
                y(d) + 4.0
            );
            d -= 10.0;
        }
        let step = [1.0, 2.0, 5.0, 10.0, 20.0]
            .into_iter()
            .find(|s| fmax / s <= 10.0)
            .unwrap_or(50.0);
        let mut f = 0.0;
        while f <= fmax + 1e-9 {
            let _ = writeln!(
                o,
                r##"<line x1="{:.1}" x2="{:.1}" y1="{t}" y2="{}" stroke="#e3e6ec"/><text x="{:.1}" y="{}" text-anchor="middle" fill="#555">{f:.0}</text>"##,
                x(f),
                x(f),
                h - b,
                x(f),
                h - b + 16.0
            );
            f += step;
        }
        for (s, color, name) in [
            (&self.s21, "#1f6feb", "insertion |S21|"),
            (&self.s11, "#d9480f", "return |S11|"),
        ] {
            let pts: Vec<String> = self
                .f
                .iter()
                .zip(s.iter())
                .map(|(f, v)| format!("{:.1},{:.1}", x(*f), y(v.db().max(lo))))
                .collect();
            let _ = writeln!(
                o,
                r##"<polyline points="{}" fill="none" stroke="{color}" stroke-width="1.8"/>"##,
                pts.join(" ")
            );
            let ly = if name.starts_with("ins") {
                t + 14.0
            } else {
                t + 30.0
            };
            let _ = writeln!(
                o,
                r##"<line x1="{}" x2="{}" y1="{ly}" y2="{ly}" stroke="{color}" stroke-width="2"/><text x="{}" y="{}" fill="#222">{name}</text>"##,
                w - r - 170.0,
                w - r - 150.0,
                w - r - 144.0,
                ly + 4.0
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="24" font-size="14" fill="#111">{}</text><text x="{}" y="{}" text-anchor="middle" fill="#555">GHz</text><text x="16" y="{}" fill="#555" transform="rotate(-90 16 {})">dB</text></svg>"##,
            title.replace('&', "&amp;").replace('<', "&lt;"),
            (l + w - r) / 2.0,
            h - 12.0,
            (t + h - b) / 2.0,
            (t + h - b) / 2.0
        );
        o
    }
}

/// The stack-up along z: copper sheets (z of each copper layer, mil, from
/// the top, going down) and the dielectric slabs between them.
struct Stack {
    /// z of each copper layer (0 at the top copper, growing downwards).
    sheet: Vec<f64>,
    /// Slabs: top z, bottom z, Er, Df.
    slabs: Vec<(f64, f64, f64, f64)>,
    names: Vec<String>,
}

fn stack(stackup: &Stackup) -> Stack {
    let mut z = 0.0;
    let mut sheet: Vec<f64> = Vec::new();
    let mut slabs: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut names = Vec::new();
    let mut last_copper = false;
    let mut pending = 0.0; // copper thickness waiting for the slab below
    for l in &stackup.layers {
        match l.kind {
            LayerKind::Copper => {
                sheet.push(z);
                names.push(l.name.clone());
                pending = l.thickness.mil().max(0.0);
                last_copper = true;
            }
            LayerKind::Dielectric if !sheet.is_empty() => {
                let t = l.thickness.mil().max(0.0);
                if t <= 0.0 {
                    continue;
                }
                let er = l.dielectric_constant.filter(|e| *e >= 1.0).unwrap_or(4.2);
                let df = l.loss_tangent.unwrap_or(0.0).max(0.0);
                if last_copper || slabs.is_empty() {
                    // A new gap: the copper's thickness joins it.
                    let t = t + pending;
                    slabs.push((z, z + t, er, df));
                    z += t;
                } else {
                    let s = slabs.last_mut().unwrap();
                    let (t0, t1) = (s.1 - s.0, t);
                    s.2 = (s.2 * t0 + er * t1) / (t0 + t1);
                    s.3 = (s.3 * t0 + df * t1) / (t0 + t1);
                    s.1 += t;
                    z += t;
                }
                pending = 0.0;
                last_copper = false;
            }
            _ => {}
        }
    }
    // Copper with no dielectric after it ends the board.
    sheet.truncate(slabs.len() + 1);
    names.truncate(sheet.len());
    Stack {
        sheet,
        slabs,
        names,
    }
}

/// Grid lines from `lo` to `hi`: uniform `d0` over [`a`, `b`] (with a line
/// at `anchor`), growing by `ratio` up to `dmax` outside it.
fn mesh_axis(
    lo: f64,
    hi: f64,
    a: f64,
    b: f64,
    anchor: f64,
    d0: f64,
    dmax: f64,
    ratio: f64,
) -> Vec<f64> {
    let n0 = ((a - anchor) / d0).floor() as i64;
    let n1 = ((b - anchor) / d0).ceil() as i64;
    let mut core: Vec<f64> = (n0..=n1).map(|n| anchor + n as f64 * d0).collect();
    let mut left = Vec::new();
    let (mut x, mut d) = (core[0], d0);
    while x > lo {
        d = (d * ratio).min(dmax);
        x -= d;
        left.push(x.max(lo));
        if x <= lo {
            break;
        }
    }
    let mut right = Vec::new();
    let (mut x, mut d) = (*core.last().unwrap(), d0);
    while x < hi {
        d = (d * ratio).min(dmax);
        x += d;
        right.push(x.min(hi));
        if x >= hi {
            break;
        }
    }
    left.reverse();
    // Drop a sliver left at an end.
    let mut v = left;
    v.append(&mut core);
    v.append(&mut right);
    v.dedup_by(|a, b| (*a - *b).abs() < d0 * 0.3);
    v
}

/// Adds `n` PML cells to both ends of an axis, each as wide as the end cell.
fn with_pml(mut v: Vec<f64>, n: usize) -> Vec<f64> {
    let d0 = v[1] - v[0];
    let d1 = v[v.len() - 1] - v[v.len() - 2];
    let first = v[0];
    let last = *v.last().unwrap();
    let mut out: Vec<f64> = (1..=n).rev().map(|k| first - k as f64 * d0).collect();
    out.append(&mut v);
    out.extend((1..=n).map(|k| last + k as f64 * d1));
    out
}

/// CPML coefficients (b, c) along one axis at `pos`.
fn cpml(
    pos: &[f64],
    inner: [f64; 2],
    outer: [f64; 2],
    dt: f64,
    d_cell: [f64; 2],
) -> (Vec<f32>, Vec<f32>) {
    let m = 3.0;
    let eta = (MU0 / EPS0).sqrt();
    let mut b = vec![0.0f32; pos.len()];
    let mut c = vec![0.0f32; pos.len()];
    for (i, &p) in pos.iter().enumerate() {
        let (rho, dcell) = if p < inner[0] {
            ((inner[0] - p) / (inner[0] - outer[0]), d_cell[0])
        } else if p > inner[1] {
            ((p - inner[1]) / (outer[1] - inner[1]), d_cell[1])
        } else {
            continue;
        };
        let rho = rho.clamp(0.0, 1.0);
        let smax = 0.8 * (m + 1.0) / (eta * dcell * MIL * 2.0f64.sqrt());
        let sigma = smax * rho.powf(m);
        let alpha = 0.05 * (1.0 - rho);
        let bb = (-(sigma + alpha) * dt / EPS0).exp();
        let cc = if sigma + alpha > 0.0 {
            sigma / (sigma + alpha) * (bb - 1.0)
        } else {
            0.0
        };
        b[i] = bb as f32;
        c[i] = cc as f32;
    }
    (b, c)
}

struct Lumped {
    /// Ez edges in series, each with (index, alpha, gamma, delta).
    edges: Vec<(usize, f32, f32, f32)>,
    /// Source voltage share per edge (0 for a load), sign included.
    drive: f64,
}

/// A trace's sampling place: Ez column under (or above) it, and the
/// current loop around it.
struct Probe {
    /// V: (index, dz in m, sign) of Ez edges from the plane to the trace.
    v: Vec<Vec<(usize, f64)>>,
    /// I: (index, length in m, sign) of H components around the trace.
    i: Vec<(usize, f64, u8)>,
}

/// Solves a via transition on `stackup`.
pub fn solve(stackup: &Stackup, setup: &ViaSetup) -> Result<ViaResult, String> {
    solve_with(stackup, setup, &|_| {})
}

/// As `solve`, reporting progress (0 to 1).
pub fn solve_with(
    stackup: &Stackup,
    setup: &ViaSetup,
    progress: &dyn Fn(f64),
) -> Result<ViaResult, String> {
    let clock = std::time::Instant::now();
    let st = stack(stackup);
    let nl = st.sheet.len();
    if nl < 2 {
        return Err("the stack-up needs two or more copper layers".into());
    }
    let (from, to) = (setup.from as usize, setup.to as usize);
    if from >= nl || to >= nl {
        return Err(format!("layers must be in 0 to {}", nl - 1));
    }
    // The same layer twice: a plain trace, no via (to check the solver).
    let thru = from == to;
    let geo = crate::impedance::layer_geometry(stackup);
    let gfrom = geo
        .iter()
        .find(|g| g.copper as usize == from)
        .ok_or("no geometry for the first layer")?;
    let gto = geo
        .iter()
        .find(|g| g.copper as usize == to)
        .ok_or("no geometry for the second layer")?;
    let _ = (gfrom, gto);
    let w_of =
        |g: &LayerGeometry, w: Option<f64>| w.or_else(|| g.width_for_field(50.0)).unwrap_or(5.0);
    let mut notes = Vec::new();
    // Planes: copper layers named like planes, else every other layer.
    let plane_name = |n: &str| {
        let u = n.to_ascii_uppercase();
        [
            "GND", "GROUND", "PWR", "POWER", "VCC", "VDD", "VSS", "PLANE",
        ]
        .iter()
        .any(|k| u.contains(k))
    };
    let named: Vec<bool> = (0..nl)
        .map(|i| plane_name(&st.names[i]) && i != from && i != to)
        .collect();
    let planes: Vec<bool> = if named.iter().any(|&p| p) {
        named
    } else {
        (0..nl).map(|i| i != from && i != to).collect()
    };
    // Reference plane for each trace: nearest below, else above.
    let refs = |l: usize| -> (Option<usize>, Option<usize>) {
        (
            (l + 1..nl).find(|&k| planes[k]),
            (0..l).rev().find(|&k| planes[k]),
        )
    };
    let (r_from, r_to) = (refs(from), refs(to));
    // Each trace's cross-section against the planes the solver uses (signal
    // layers between are only dielectric to it).
    let ref_geo = |l: usize, r: (Option<usize>, Option<usize>)| -> LayerGeometry {
        let mut g = geo
            .iter()
            .find(|g| g.copper as usize == l)
            .cloned()
            .unwrap_or_else(|| geo[0].clone());
        let er_of = |m: usize, below: bool| {
            geo.iter()
                .find(|g| g.copper as usize == m)
                .and_then(|g| {
                    if below {
                        g.slabs_below.first()
                    } else {
                        g.slabs_above.first()
                    }
                    .copied()
                })
                .map_or(g.er, |s| s[1])
        };
        if let Some(p) = r.0 {
            let mut sl = Vec::new();
            for m in l..p {
                if let Some(gm) = geo.iter().find(|g| g.copper as usize == m) {
                    if m > l && gm.t > 0.0 {
                        sl.push([gm.t, er_of(m, true), -1.0]);
                    }
                    sl.extend(gm.slabs_below.iter().copied());
                }
            }
            if !sl.is_empty() {
                g.below = Some(sl.iter().map(|s| s[0]).sum());
                g.slabs_below = sl;
            }
        }
        if let Some(p) = r.1 {
            let mut sl = Vec::new();
            for m in (p + 1..=l).rev() {
                if let Some(gm) = geo.iter().find(|g| g.copper as usize == m) {
                    if m < l && gm.t > 0.0 {
                        sl.push([gm.t, er_of(m, false), -1.0]);
                    }
                    sl.extend(gm.slabs_above.iter().copied());
                }
            }
            if !sl.is_empty() {
                g.above = Some(sl.iter().map(|s| s[0]).sum());
                g.slabs_above = sl;
            }
        }
        g
    };
    let (g1, g2) = (ref_geo(from, r_from), ref_geo(to, r_to));
    let w = [w_of(&g1, setup.w_from), w_of(&g2, setup.w_to)];
    let lines = [
        crate::field::single(&g1, w[0]),
        crate::field::single(&g2, w[1]),
    ];
    let z0 = [
        lines[0].as_ref().map_or(50.0, |l| l.z0),
        lines[1].as_ref().map_or(50.0, |l| l.z0),
    ];
    let eeff = [
        lines[0].as_ref().map_or(4.0, |l| l.eeff),
        lines[1].as_ref().map_or(4.0, |l| l.eeff),
    ];
    if r_from.0.is_none() && r_from.1.is_none() || r_to.0.is_none() && r_to.1.is_none() {
        return Err("a trace layer has no plane above or below it".into());
    }
    // The barrel's extent.
    let (via_top, via_bot) = if setup.backdrill {
        (from.min(to), from.max(to))
    } else {
        (0, nl - 1)
    };
    let stub =
        st.sheet[via_bot] - st.sheet[from.max(to)] + (st.sheet[from.min(to)] - st.sheet[via_top]);
    let (rd, rp, ra) = (
        setup.drill / 2.0,
        setup.pad.max(setup.drill) / 2.0,
        setup.antipad.max(setup.pad) / 2.0,
    );
    // Signal vias and return vias, mil.
    let sig: Vec<[f64; 2]> = match setup.pair {
        Some(p) => vec![[0.0, p / 2.0], [0.0, -p / 2.0]],
        None => vec![[0.0, 0.0]],
    };
    let rdist = setup.return_dist.max(ra + rd + 1.0);
    // Return vias stay off the traces, which leave each via along ±x on
    // their layers but which a through return via would short: first
    // straight out from the via (±y), then on each side at the shallowest
    // angle that clears the widest trace.
    let wtr = w[0].max(w[1]) / 2.0 + rp + 1.0;
    let side = (wtr / rdist).clamp(-1.0, 1.0).asin();
    let tilted = side < std::f64::consts::FRAC_PI_2 * 0.9;
    let at = |c: [f64; 2], a: f64| [c[0] + rdist * a.cos(), c[1] + rdist * a.sin()];
    let half = std::f64::consts::FRAC_PI_2;
    let mut spots: Vec<[f64; 2]> = Vec::new();
    match setup.pair {
        Some(p) => {
            let (up, dn) = ([0.0, p / 2.0], [0.0, -p / 2.0]);
            spots.push(at(up, half));
            spots.push(at(dn, -half));
            if tilted {
                // Point-symmetric pairs, so the pair stays balanced.
                spots.push(at(up, side));
                spots.push(at(dn, std::f64::consts::PI + side));
                spots.push(at(up, std::f64::consts::PI - side));
                spots.push(at(dn, -side));
            }
        }
        None => {
            let c = [0.0, 0.0];
            spots.push(at(c, half));
            spots.push(at(c, -half));
            if tilted {
                spots.push(at(c, side));
                spots.push(at(c, std::f64::consts::PI + side));
                spots.push(at(c, std::f64::consts::PI - side));
                spots.push(at(c, -side));
            }
        }
    }
    if setup.returns > spots.len() {
        return Err(format!(
            "{} return vias do not fit at {rdist:.1} mil without touching the traces; \
             at most {} (move them further out for more)",
            setup.returns,
            spots.len()
        ));
    }
    let ret: Vec<[f64; 2]> = spots.into_iter().take(setup.returns).collect();
    // Each return via must clear every signal via's antipad (or the oval
    // one) and the other returns.
    for (k, r) in ret.iter().enumerate() {
        let near_sig = sig
            .iter()
            .any(|c| (r[0] - c[0]).hypot(r[1] - c[1]) < ra + rd + 1.0 - 1e-9)
            || (setup.antipad_oval
                && sig.len() == 2
                && seg_dist(*r, sig[0], sig[1]) < ra + rd + 1.0 - 1e-9);
        let near_ret = ret[..k]
            .iter()
            .any(|q| (r[0] - q[0]).hypot(r[1] - q[1]) < 2.0 * rp);
        if near_sig || near_ret {
            return Err(format!(
                "the return vias at {rdist:.1} mil overlap the antipad or each other"
            ));
        }
    }
    // Cell sizes: fine enough for the drill, the traces and the thinnest
    // slab; coarse cells no more than a twentieth of a wavelength.
    let hmin = st.slabs.iter().map(|s| s.1 - s.0).fold(f64::MAX, f64::min);
    let er_max = st.slabs.iter().map(|s| s.2).fold(1.0, f64::max);
    let mut d0 = if setup.cell > 0.0 {
        setup.cell
    } else {
        // A quarter of the drill: a stub's notch then lands within about
        // 2% of where finer grids converge (a third gave about 7%).
        (setup.drill / 4.0)
            .min(w[0].min(w[1]) / 2.0)
            .min(hmin / 2.0)
            .clamp(0.5, 3.0)
    };
    // Make the trace widths whole numbers of cells.
    // A thin metal edge on a grid line acts as if it reached about a third
    // of a cell further (the one-third rule), so metal is drawn a third of
    // a cell inside its true edges: traces, pads and barrels shrink by
    // that, antipads grow. The narrower trace spans whole cells plus that.
    let wmin = w[0].min(w[1]);
    d0 = wmin / ((wmin / d0 - 2.0 / 3.0).round().max(1.0) + 2.0 / 3.0);
    let edge = d0 / 3.0;
    let lambda = C0 / (setup.fmax_ghz * 1e9) / er_max.sqrt() / MIL;
    let dmax = (lambda / 20.0).max(d0);
    let hbig = st.slabs.iter().map(|s| s.1 - s.0).fold(0.0, f64::max);
    let core = ra.max(rdist + rd).max(rp) + 2.0 * d0;
    let half_pair = setup.pair.map_or(0.0, |p| p / 2.0);
    // The traces' fields reach a few times their height over the plane.
    let h_ref = [(from, r_from), (to, r_to)]
        .iter()
        .flat_map(|&(l, r)| {
            let sh = &st.sheet;
            [r.0, r.1]
                .into_iter()
                .flatten()
                .map(move |p| (sh[p] - sh[l]).abs())
        })
        .fold(0.0f64, f64::max);
    let probe_at = core + (3.0 * h_ref).max(10.0);
    // The resistors' near field (and, over a microstrip, what they
    // radiate) must die down before the probes.
    let outer = [from, to].iter().any(|&l| l == 0 || l == nl - 1);
    let port_at = probe_at
        + if outer {
            (10.0 * h_ref).max(50.0)
        } else {
            (5.0 * h_ref).max(25.0)
        };
    let xspan = port_at + 3.0 * d0.max(h_ref * 0.5);
    let yspan = core + half_pair + (4.0 * h_ref).max(25.0);
    let board = st.sheet[nl - 1];
    // A microstrip's field reaches well up into the air.
    let air = if outer {
        (5.0 * h_ref).max(25.0)
    } else {
        (2.0 * h_ref).max(10.0)
    };
    let _ = hbig;
    // Along the traces, cells small enough to sample the waves finely.
    let x = with_pml(
        mesh_axis(
            -xspan,
            xspan,
            -core,
            core,
            0.0,
            d0,
            (lambda / 40.0).max(d0),
            1.25,
        ),
        8,
    );
    // Fine across the traces too (they run along x at the vias' y).
    let y = with_pml(
        mesh_axis(
            -yspan,
            yspan,
            -core - half_pair,
            core + half_pair,
            -(w[0] / 2.0 - edge) + half_pair,
            d0,
            dmax,
            1.25,
        ),
        8,
    );
    let mut zl: Vec<f64> = vec![-air];
    {
        // Air above, graded.
        let mut zz = Vec::new();
        let mut d = d0;
        let mut p = 0.0;
        while p > -air {
            p -= d;
            zz.push(p.max(-air));
            d = (d * 1.3).min(dmax);
        }
        zz.pop();
        zz.reverse();
        zl.extend(zz);
    }
    for (k, s) in st.slabs.iter().enumerate() {
        let t = s.1 - s.0;
        let n = ((t / (d0 * 1.2)).ceil() as usize).max(2);
        if k == 0 {
            zl.push(s.0);
        }
        for m in 1..=n {
            zl.push(s.0 + t * m as f64 / n as f64);
        }
    }
    {
        let mut d = d0;
        let mut p = board;
        loop {
            d = (d * 1.3).min(dmax);
            p += d;
            if p >= board + air {
                zl.push(board + air);
                break;
            }
            zl.push(p);
        }
    }
    zl.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let z = with_pml(zl, 8);
    let (nx, ny, nz) = (x.len() - 1, y.len() - 1, z.len() - 1);
    let (sx, sy) = ((ny + 1) * (nz + 1), nz + 1);
    let total = (nx + 1) * sx;
    if total > 12_000_000 {
        return Err(format!(
            "the grid would have {total} cells: use a larger cell size"
        ));
    }
    let id = |i: usize, j: usize, k: usize| i * sx + j * sy + k;
    let near = |v: &[f64], p: f64| -> usize {
        let mut best = 0;
        for (i, &q) in v.iter().enumerate() {
            if (q - p).abs() < (v[best] - p).abs() {
                best = i;
            }
        }
        best
    };
    let ksheet: Vec<usize> = st.sheet.iter().map(|&s| near(&z, s)).collect();
    // Cell and dual sizes, in metres.
    let prim = |v: &[f64]| -> Vec<f64> { v.windows(2).map(|p| (p[1] - p[0]) * MIL).collect() };
    let dual = |v: &[f64]| -> Vec<f64> {
        let n = v.len();
        (0..n)
            .map(|i| {
                let a = if i > 0 { v[i] - v[i - 1] } else { v[1] - v[0] };
                let b = if i + 1 < n {
                    v[i + 1] - v[i]
                } else {
                    v[n - 1] - v[n - 2]
                };
                (a + b) / 2.0 * MIL
            })
            .collect()
    };
    let (dx, dy, dz) = (prim(&x), prim(&y), prim(&z));
    let (ddx, ddy, ddz) = (dual(&x), dual(&y), dual(&z));
    let dmin = |v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min);
    let dt = 0.95
        / (C0
            * (1.0 / dmin(&dx).powi(2) + 1.0 / dmin(&dy).powi(2) + 1.0 / dmin(&dz).powi(2)).sqrt());
    // Material per cell (Er, conductivity) from the slab the cell's
    // centre is in.
    let fc = setup.fmax_ghz * 0.5e9;
    let cell_mat: Vec<(f64, f64)> = (0..nz)
        .map(|k| {
            let zc = (z[k] + z[k + 1]) / 2.0;
            st.slabs
                .iter()
                .find(|s| zc >= s.0 && zc <= s.1)
                .map_or((1.0, 0.0), |s| {
                    let sigma = if setup.lossy {
                        2.0 * std::f64::consts::PI * fc * EPS0 * s.2 * s.3
                    } else {
                        0.0
                    };
                    (s.2, sigma)
                })
        })
        .collect();
    // E-field coefficients: an edge along z sits in one cell's material; an
    // edge in the x-y plane on the boundary of two slabs takes the mean.
    let edge_mat = |k: usize, along_z: bool| -> (f64, f64) {
        if along_z {
            cell_mat[k.min(nz - 1)]
        } else {
            let a = cell_mat[k.saturating_sub(1).min(nz - 1)];
            let b = cell_mat[k.min(nz - 1)];
            ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
        }
    };
    let coef = |er: f64, sig: f64| -> (f32, f32) {
        let e = EPS0 * er;
        let q = sig * dt / (2.0 * e);
        (((1.0 - q) / (1.0 + q)) as f32, (dt / e / (1.0 + q)) as f32)
    };
    let mut cax = vec![0.0f32; total];
    let mut cbx = vec![0.0f32; total];
    let mut cay = vec![0.0f32; total];
    let mut cby = vec![0.0f32; total];
    let mut caz = vec![0.0f32; total];
    let mut cbz = vec![0.0f32; total];
    for k in 0..=nz {
        let (a, b) = coef(edge_mat(k, false).0, edge_mat(k, false).1);
        let (az, bz) = coef(edge_mat(k, true).0, edge_mat(k, true).1);
        for i in 0..=nx {
            for j in 0..=ny {
                let c = id(i, j, k);
                cax[c] = a;
                cbx[c] = b;
                cay[c] = a;
                cby[c] = b;
                caz[c] = az;
                cbz[c] = bz;
            }
        }
    }
    // Metal: edges set to zero coefficients.
    let pec = |v: &mut Vec<f32>, w: &mut Vec<f32>, c: usize| {
        v[c] = 0.0;
        w[c] = 0.0;
    };
    let xm = |i: usize| (x[i] + x[i + 1]) / 2.0;
    let ym = |j: usize| (y[j] + y[j + 1]) / 2.0;
    let inside = |p: [f64; 2], cs: &[[f64; 2]], r: f64| {
        cs.iter()
            .any(|c| (p[0] - c[0]).hypot(p[1] - c[1]) <= r + 1e-9)
    };
    // Planes with antipads.
    let oval = setup.antipad_oval && sig.len() == 2;
    let clear = |p: [f64; 2]| {
        if oval {
            seg_dist(p, sig[0], sig[1]) <= ra + edge + 1e-9
        } else {
            inside(p, &sig, ra + edge)
        }
    };
    for (l, &is_plane) in planes.iter().enumerate() {
        if !is_plane {
            continue;
        }
        let k = ksheet[l];
        for i in 0..=nx {
            for j in 0..=ny {
                if i < nx && (thru || !clear([xm(i), y[j]])) {
                    pec(&mut cax, &mut cbx, id(i, j, k));
                }
                if j < ny && (thru || !clear([x[i], ym(j)])) {
                    pec(&mut cay, &mut cby, id(i, j, k));
                }
            }
        }
    }
    // Pads on the barrel's ends and the trace layers.
    let mut pad_layers = vec![via_top, via_bot, from, to];
    pad_layers.sort();
    pad_layers.dedup();
    for &l in pad_layers.iter().filter(|_| !thru) {
        let k = ksheet[l];
        for i in 0..=nx {
            for j in 0..=ny {
                if i < nx && inside([xm(i), y[j]], &sig, rp - edge) {
                    pec(&mut cax, &mut cbx, id(i, j, k));
                }
                if j < ny && inside([x[i], ym(j)], &sig, rp - edge) {
                    pec(&mut cay, &mut cby, id(i, j, k));
                }
            }
        }
    }
    // Barrels (solid): signal vias over their extent, return vias through.
    let barrel = |cs: &[[f64; 2]],
                  k0: usize,
                  k1: usize,
                  cax: &mut Vec<f32>,
                  cbx: &mut Vec<f32>,
                  cay: &mut Vec<f32>,
                  cby: &mut Vec<f32>,
                  caz: &mut Vec<f32>,
                  cbz: &mut Vec<f32>| {
        let rd = (rd - edge).max(0.0);
        for i in 0..=nx {
            for j in 0..=ny {
                for k in k0..=k1 {
                    if k < k1 && inside([x[i], y[j]], cs, rd) {
                        pec(caz, cbz, id(i, j, k));
                    }
                    if i < nx && inside([xm(i), y[j]], cs, rd) {
                        pec(cax, cbx, id(i, j, k));
                    }
                    if j < ny && inside([x[i], ym(j)], cs, rd) {
                        pec(cay, cby, id(i, j, k));
                    }
                }
            }
        }
    };
    if !thru {
        barrel(
            &sig,
            ksheet[via_top],
            ksheet[via_bot],
            &mut cax,
            &mut cbx,
            &mut cay,
            &mut cby,
            &mut caz,
            &mut cbz,
        );
    }
    if !ret.is_empty() && !thru {
        barrel(
            &ret,
            ksheet[0],
            ksheet[nl - 1],
            &mut cax,
            &mut cbx,
            &mut cay,
            &mut cby,
            &mut caz,
            &mut cbz,
        );
    }
    // Traces: the first runs from -x to the via, the second from the via
    // to +x; each at its via's y.
    let x_port = [near(&x, -port_at), near(&x, port_at)];
    // Probes: near the via on each trace (0, 1), and further out (2, 3)
    // to measure each trace's own impedance and propagation.
    let gap = (port_at - probe_at) / 3.0;
    let x_probe = [
        near(&x, -probe_at),
        near(&x, probe_at),
        near(&x, -probe_at - gap),
        near(&x, probe_at + gap),
    ];
    let mut trace_rows: Vec<[Vec<usize>; 2]> = Vec::new(); // per signal via: j nodes of each trace
    for c in &sig {
        let mut rows: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
        for (t, &l) in [from, to].iter().enumerate() {
            let yc = c[1];
            let js: Vec<usize> = (0..=ny)
                .filter(|&j| (y[j] - yc).abs() <= w[t] / 2.0 - edge + d0 * 0.01)
                .collect();
            let js = if js.is_empty() {
                vec![near(&y, yc)]
            } else {
                js
            };
            let k = ksheet[l];
            let (i0, i1) = if t == 0 {
                (x_port[0], near(&x, 0.0))
            } else {
                (near(&x, 0.0), x_port[1])
            };
            for i in i0..=i1 {
                for &j in &js {
                    if i < i1 {
                        pec(&mut cax, &mut cbx, id(i, j, k));
                    }
                    if j < *js.last().unwrap() {
                        pec(&mut cay, &mut cby, id(i, j, k));
                    }
                }
            }
            rows[t] = js;
        }
        trace_rows.push(rows);
    }
    // Ports: resistors from each trace's end to its reference plane(s),
    // the first driven.
    let z_ref = 50.0;
    let mut lumped: Vec<Lumped> = Vec::new();
    for (si, rows) in trace_rows.iter().enumerate() {
        for t in 0..2 {
            let l = [from, to][t];
            let r = [r_from, r_to][t];
            let refs: Vec<usize> = [r.0, r.1].into_iter().flatten().collect();
            let i = x_port[t];
            let j = rows[t][rows[t].len() / 2];
            for &p in &refs {
                let (k0, k1) = (ksheet[l].min(ksheet[p]), ksheet[l].max(ksheet[p]));
                let n = (k1 - k0).max(1);
                let r_e = z_ref * refs.len() as f64 / n as f64;
                let a = ddx[i] * ddy[j];
                let mut edges = Vec::new();
                for k in k0..k1 {
                    let c = id(i, j, k);
                    let e = EPS0 * cell_mat[k].0;
                    let beta = dt * dz[k] / (2.0 * r_e * e * a);
                    edges.push((
                        c,
                        ((1.0 - beta) / (1.0 + beta)) as f32,
                        (1.0 / (1.0 + beta)) as f32,
                        (dt / (r_e * e * a * (1.0 + beta))) as f32,
                    ));
                    caz[c] = 1.0;
                    cbz[c] = cbz[c].max((dt / e) as f32);
                }
                // The trace is above the plane when the plane is lower
                // (bigger z): the source pushes the trace up in voltage.
                let sign = if ksheet[p] > ksheet[l] { 1.0 } else { -1.0 };
                let polarity = if si == 0 { 1.0 } else { -1.0 };
                let drive = if t == 0 {
                    sign * polarity / n as f64
                } else {
                    0.0
                };
                lumped.push(Lumped { edges, drive });
            }
        }
    }
    // Probes.
    let mut probes: Vec<[Probe; 4]> = Vec::new();
    for rows in &trace_rows {
        let mk = |p: usize| -> Probe {
            let t = p % 2;
            let l = [from, to][t];
            let r = [r_from, r_to][t];
            let i = x_probe[p];
            let js = &rows[t];
            let jc = js[js.len() / 2];
            let mut v = Vec::new();
            for ii in [i, i + 1] {
                let mut col = Vec::new();
                if let Some(p) = r.0 {
                    // Plane below: V = sum of Ez dz from the trace down.
                    for k in ksheet[l]..ksheet[p] {
                        col.push((id(ii, jc, k), dz[k]));
                    }
                } else if let Some(p) = r.1 {
                    for k in ksheet[p]..ksheet[l] {
                        col.push((id(ii, jc, k), -dz[k]));
                    }
                }
                v.push(col);
            }
            // Current loop at x = i + ½ around the trace.
            let k = ksheet[l];
            let (jl, jr) = (js[0] - 1, *js.last().unwrap());
            let mut cur = Vec::new();
            for j in jl + 1..=jr {
                cur.push((id(i, j, k - 1), ddy[j], 0u8)); // Hy below, +
                cur.push((id(i, j, k), ddy[j], 1u8)); // Hy above, -
            }
            cur.push((id(i, jr, k), ddz[k], 2u8)); // Hz right, +
            cur.push((id(i, jl, k), ddz[k], 3u8)); // Hz left, -
            Probe { v, i: cur }
        };
        probes.push([mk(0), mk(1), mk(2), mk(3)]);
    }
    // CPML.
    let pml = 8;
    let cp = |v: &[f64], half: bool| -> (Vec<f32>, Vec<f32>) {
        let n = v.len();
        let pos: Vec<f64> = if half {
            v.windows(2).map(|p| (p[0] + p[1]) / 2.0).collect()
        } else {
            v.to_vec()
        };
        cpml(
            &pos,
            [v[pml], v[n - 1 - pml]],
            [v[0], v[n - 1]],
            dt,
            [v[1] - v[0], v[n - 1] - v[n - 2]],
        )
    };
    let (bex, cex) = cp(&x, false);
    let (bey, cey) = cp(&y, false);
    let (bez, cez) = cp(&z, false);
    let (bhx, chx) = cp(&x, true);
    let (bhy, chy) = cp(&y, true);
    let (bhz, chz) = cp(&z, true);
    let inv = |v: &[f64]| -> Vec<f32> { v.iter().map(|d| (1.0 / d) as f32).collect() };
    let (ipx, ipy, ipz) = (inv(&dx), inv(&dy), inv(&dz));
    let (idx, idy, idz) = (inv(&ddx), inv(&ddy), inv(&ddz));
    let ch = (dt / MU0) as f32;
    // Excitation: a Gaussian down 20 dB at fmax.
    let tau = (10.0f64.ln()).sqrt() / (std::f64::consts::PI * setup.fmax_ghz * 1e9);
    let t0 = 4.5 * tau;
    let vs = |t: f64| (-((t - t0) / tau).powi(2)).exp();
    // Frequencies and running transforms.
    let nf = setup.points.max(10);
    let freqs: Vec<f64> = (1..=nf)
        .map(|k| setup.fmax_ghz * 1e9 * k as f64 / nf as f64)
        .collect();
    // Per port side (2), per signal via: V and I transforms.
    let ns = sig.len();
    let mut fv = vec![vec![Cx::default(); nf]; 4 * ns];
    let mut fi = vec![vec![Cx::default(); nf]; 4 * ns];
    let rot: Vec<Cx> = freqs
        .iter()
        .map(|f| Cx::expj(-2.0 * std::f64::consts::PI * f * dt))
        .collect();
    let mut ph_e: Vec<Cx> = vec![Cx::new(1.0, 0.0); nf];
    let mut ph_h: Vec<Cx> = freqs
        .iter()
        .map(|f| Cx::expj(-2.0 * std::f64::consts::PI * f * dt * 0.5))
        .collect();
    // Fields.
    let mut ex = vec![0.0f32; total];
    let mut ey = vec![0.0f32; total];
    let mut ez = vec![0.0f32; total];
    let mut hx = vec![0.0f32; total];
    let mut hy = vec![0.0f32; total];
    let mut hz = vec![0.0f32; total];
    let mut pe: Vec<Vec<f32>> = (0..6).map(|_| vec![0.0f32; total]).collect();
    let mut ph: Vec<Vec<f32>> = (0..6).map(|_| vec![0.0f32; total]).collect();
    // Steps: until the fields have died away, at most enough for the
    // slowest wave to cross the board twenty times.
    let span = (x[nx] - x[0]).max(y[ny] - y[0]) * MIL * er_max.sqrt() / C0;
    let max_steps = (((t0 * 2.0 + 20.0 * span) / dt) as usize).clamp(2000, 400_000);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let chunk = nx.div_ceil(threads).max(1);
    let mut steps = 0;
    let mut peak = 0.0f64;
    let mut quiet = 0usize;
    let mut old: Vec<f32> = Vec::new();
    let pml_of = |c: &[f32]| -> Vec<usize> { (0..c.len()).filter(|&i| c[i] != 0.0).collect() };
    let k3 = Kernel {
        nx,
        ny,
        nz,
        sx,
        sy,
        ch,
        cax: &cax,
        cbx: &cbx,
        cay: &cay,
        cby: &cby,
        caz: &caz,
        cbz: &cbz,
        ipx: &ipx,
        ipy: &ipy,
        ipz: &ipz,
        idx: &idx,
        idy: &idy,
        idz: &idz,
        be: [&bex, &bey, &bez],
        ce: [&cex, &cey, &cez],
        bh: [&bhx, &bhy, &bhz],
        chh: [&chx, &chy, &chz],
        pml_e: [pml_of(&cex), pml_of(&cey), pml_of(&cez)],
        pml_h: [pml_of(&chx), pml_of(&chy), pml_of(&chz)],
    };
    if std::env::var("FDTD_DEBUG").is_ok() {
        eprintln!(
            "setup {:.2} s, grid {nx}x{ny}x{nz}",
            clock.elapsed().as_secs_f64()
        );
    }
    let mut t_h = 0.0;
    let mut t_e = 0.0;
    for n in 0..max_steps {
        steps = n + 1;
        // H update.
        let tq = std::time::Instant::now();
        update_h(
            &k3, &mut hx, &mut hy, &mut hz, &mut ph, &ex, &ey, &ez, chunk,
        );
        t_h += tq.elapsed().as_secs_f64();
        // Sample I (at time (n + ½) dt).
        for (s, pr) in probes.iter().enumerate() {
            for t in 0..4 {
                let mut cur = 0.0;
                for &(c, len, kind) in &pr[t].i {
                    cur += match kind {
                        0 => hy[c] as f64 * len,
                        1 => -(hy[c] as f64) * len,
                        2 => hz[c] as f64 * len,
                        _ => -(hz[c] as f64) * len,
                    };
                }
                // The loop as laid out runs clockwise seen from +x.
                let cur = -cur;
                let q = &mut fi[t * ns + s];
                for f in 0..nf {
                    q[f] = q[f].add(ph_h[f].scale(cur));
                }
            }
        }
        // E update.
        old.clear();
        for lp in &lumped {
            for e in &lp.edges {
                old.push(ez[e.0]);
            }
        }
        let tq = std::time::Instant::now();
        update_e(
            &k3, &mut ex, &mut ey, &mut ez, &mut pe, &hx, &hy, &hz, chunk,
        );
        t_e += tq.elapsed().as_secs_f64();
        // Resistors and the source (at time (n + ½) dt).
        let v_now = vs((n as f64 + 0.5) * dt);
        let mut q = 0;
        for lp in &lumped {
            for &(c, alpha, gamma, delta) in &lp.edges {
                let e0 = old[q];
                q += 1;
                let curl = ez[c] - e0;
                ez[c] = alpha * e0 + gamma * curl - delta * (lp.drive * v_now) as f32;
            }
        }
        // Sample V (at time (n + 1) dt).
        let mut vmax = 0.0f64;
        for (s, pr) in probes.iter().enumerate() {
            for t in 0..4 {
                let mut vv = 0.0;
                for col in &pr[t].v {
                    let mut sum = 0.0;
                    for &(c, d) in col {
                        sum += ez[c] as f64 * d;
                    }
                    vv += sum / pr[t].v.len() as f64;
                }
                // V of the trace over the plane is −∫E·dl from the plane.
                let vv = -vv;
                vmax = vmax.max(vv.abs());
                let q = &mut fv[t * ns + s];
                for f in 0..nf {
                    q[f] = q[f].add(ph_e[f].mul(rot[f]).scale(vv));
                }
            }
        }
        for f in 0..nf {
            ph_e[f] = ph_e[f].mul(rot[f]);
            ph_h[f] = ph_h[f].mul(rot[f]);
        }
        peak = peak.max(vmax);
        if (n as f64) * dt > 3.0 * t0 && vmax < peak * 1e-3 {
            quiet += 1;
            if quiet > 1000 {
                break;
            }
        } else {
            quiet = 0;
        }
        if n % 200 == 0 {
            progress(n as f64 / max_steps as f64);
        }
    }
    if std::env::var("FDTD_DEBUG").is_ok() {
        eprintln!(
            "H {t_h:.2} s, E {t_e:.2} s, all {:.2} s",
            clock.elapsed().as_secs_f64()
        );
    }
    if steps == max_steps {
        notes.push("stopped before the fields had died away: the result may ring".into());
    }
    // Power waves, each trace normalised to its own impedance as the
    // grid sees it (from the two probes on it), then moved to the via
    // with the trace's own propagation.
    let zr = if ns == 2 { 2.0 * z_ref } else { z_ref };
    let pair_vi = |t: usize, f: usize| -> (Cx, Cx) {
        if ns == 1 {
            (fv[t][f], fi[t][f])
        } else {
            // Differential: Vd = Vp − Vn, Id = (Ip − In) / 2.
            (
                fv[t * ns][f].add(fv[t * ns + 1][f].scale(-1.0)),
                fi[t * ns][f].add(fi[t * ns + 1][f].scale(-1.0)).scale(0.5),
            )
        }
    };
    let xh = |i: usize| (x[i] + x[i + 1]) / 2.0 * MIL;
    let mut s11 = Vec::with_capacity(nf);
    let mut s21 = Vec::with_capacity(nf);
    let mut zline = [0.0f64; 2];
    let z_design = (z0[0] * z0[1]).sqrt() * if ns == 2 { 2.0 } else { 1.0 };
    for f in 0..nf {
        let mut zc = [Cx::new(zr, 0.0); 2];
        let mut gam = [Cx::default(); 2];
        for t in 0..2 {
            // Far and near probe: (P, Q) in the direction of +x.
            let (p, q) = if t == 0 { (2, 0) } else { (1, 3) };
            let (v1, i1) = pair_vi(p, f);
            let (v2, i2) = pair_vi(q, f);
            let len = (xh(x_probe[q]) - xh(x_probe[p])).abs();
            let a = v1.mul(i1).add(v2.mul(i2)).div(v1.mul(i2).add(v2.mul(i1)));
            let b = v1.add(a.mul(v2).scale(-1.0)).div(i2);
            let c = i1.add(a.mul(i2).scale(-1.0)).div(v2);
            let z = csqrt(b.div(c));
            let z = if z.re < 0.0 { z.scale(-1.0) } else { z };
            // γ·len = acosh(A).
            let g = clog(a.add(csqrt(a.mul(a).add(Cx::new(-1.0, 0.0)))));
            let g = Cx::new(g.re.abs(), g.im.abs()).scale(1.0 / len);
            if z.re.is_finite() && z.re > zr * 0.3 && z.re < zr * 3.0 {
                zc[t] = z;
            }
            if g.im.is_finite() {
                gam[t] = g;
            }
            if f == nf / 4 {
                zline[t] = zc[t].re;
            }
            if std::env::var("FDTD_DEBUG").is_ok() && f % 10 == 0 {
                eprintln!(
                    "f {:.1} line {t} Zc {:.2}{:+.2}j gamma*len {:.4}{:+.4}j len {:.1} mil",
                    freqs[f] / 1e9,
                    z.re,
                    z.im,
                    g.re * len,
                    g.im * len,
                    len / MIL
                );
            }
        }
        // The traces' own waves, moved to the via's centre, give the
        // voltage and current there on each side.
        let (d1, d2) = (xh(x_probe[0]).abs(), xh(x_probe[1]).abs());
        // Fall back on the field solver's delay where the probes could
        // not tell (very low frequencies).
        let gamma = |t: usize, e: f64| {
            if gam[t].im > 0.0 {
                gam[t]
            } else {
                Cx::new(0.0, 2.0 * std::f64::consts::PI * freqs[f] * e.sqrt() / C0)
            }
        };
        let ex = |g: Cx, d: f64| {
            let e = g.scale(d);
            Cx::expj(e.im).scale(e.re.exp())
        };
        let at_via = |t: usize, d: f64, toward: f64| -> (Cx, Cx) {
            let (v, i) = pair_vi(t, f);
            let z = zc[t];
            let fwd = v.add(i.mul(z)).scale(0.5);
            let back = v.add(i.mul(z).scale(-1.0)).scale(0.5);
            let g = gamma(t, eeff[t]);
            // Forward waves run +x: the via is +d1 from probe 0, −d2 from
            // probe 1.
            let fwd = fwd.mul(ex(g, -toward * d));
            let back = back.mul(ex(g, toward * d));
            (fwd.add(back), fwd.add(back.scale(-1.0)).div(z))
        };
        let (v1, i1) = at_via(0, d1, 1.0);
        let (v2, i2) = at_via(1, d2, -1.0);
        // Reference: the grid's own version of the design impedance.
        let zg = csqrt(zc[0].mul(zc[1])).scale(zr / z_design);
        let wave = |v: Cx, i: Cx, sgn: f64| v.add(i.mul(zg).scale(sgn)).div(csqrt(zg).scale(2.0));
        let a1 = wave(v1, i1, 1.0);
        let b1 = wave(v1, i1, -1.0);
        let b2 = wave(v2, i2, 1.0);
        // The far resistor is not quite the reference, so a little comes
        // back (a2). Taking S22 = S11 and S12 = S21 for that small wave:
        // b1 = S11 a1 + S21 a2, b2 = S21 a1 + S11 a2.
        let a2 = wave(v2, i2, -1.0);
        let det = a1.mul(a1).add(a2.mul(a2).scale(-1.0));
        let (m11, m21) = if det.abs() > 1e-30 {
            (
                b1.mul(a1).add(b2.mul(a2).scale(-1.0)).div(det),
                b2.mul(a1).add(b1.mul(a2).scale(-1.0)).div(det),
            )
        } else {
            (b1.div(a1), b2.div(a1))
        };
        s11.push(m11);
        s21.push(m21);
    }
    if std::env::var("FDTD_DEBUG").is_ok() {
        eprintln!("grid line impedance {zline:?}");
    }
    if ns == 2 {
        notes.push(format!(
            "the pair's traces enter the vias at the vias' pitch (uncoupled), {:.1} Ω each",
            (z0[0] * z0[1]).sqrt()
        ));
    }
    Ok(ViaResult {
        f: freqs.iter().map(|f| f / 1e9).collect(),
        s11,
        s21,
        z_ref: zr,
        w,
        z0,
        z_line: zline,
        stub,
        cells: nx * ny * nz,
        steps,
        notes,
    })
}

/// Grid sizes and update coefficients for the field updates.
struct Kernel<'a> {
    nx: usize,
    ny: usize,
    nz: usize,
    sx: usize,
    sy: usize,
    ch: f32,
    cax: &'a [f32],
    cbx: &'a [f32],
    cay: &'a [f32],
    cby: &'a [f32],
    caz: &'a [f32],
    cbz: &'a [f32],
    ipx: &'a [f32],
    ipy: &'a [f32],
    ipz: &'a [f32],
    idx: &'a [f32],
    idy: &'a [f32],
    idz: &'a [f32],
    /// CPML b and c per axis, at nodes (E) and half nodes (H).
    be: [&'a [f32]; 3],
    ce: [&'a [f32]; 3],
    bh: [&'a [f32]; 3],
    chh: [&'a [f32]; 3],
    /// Indices along each axis that lie in the PML.
    pml_e: [Vec<usize>; 3],
    pml_h: [Vec<usize>; 3],
}

/// Runs `f(i0, i1, parts)` on chunks of i-planes in parallel, `parts`
/// holding the chunk of each array in `arrays`.
fn par_chunks<F>(arrays: Vec<&mut [f32]>, sx: usize, chunk: usize, f: F)
where
    F: Fn(usize, Vec<&mut [f32]>) + Sync,
{
    let mut per: Vec<Vec<&mut [f32]>> = Vec::new();
    for arr in arrays {
        for (ci, part) in arr.chunks_mut(sx * chunk).enumerate() {
            if per.len() <= ci {
                per.push(Vec::new());
            }
            per[ci].push(part);
        }
    }
    let f = &f;
    std::thread::scope(|s| {
        for (ci, parts) in per.into_iter().enumerate() {
            s.spawn(move || f(ci * chunk, parts));
        }
    });
}

/// H from the curl of E (Faraday), with CPML.
fn update_h(
    k: &Kernel,
    hx: &mut [f32],
    hy: &mut [f32],
    hz: &mut [f32],
    ph: &mut [Vec<f32>],
    ex: &[f32],
    ey: &[f32],
    ez: &[f32],
    chunk: usize,
) {
    let (nx, ny, nz, sx, sy, ch) = (k.nx, k.ny, k.nz, k.sx, k.sy, k.ch);
    let mut arrays: Vec<&mut [f32]> = vec![hx, hy, hz];
    for p in ph.iter_mut() {
        arrays.push(p);
    }
    par_chunks(arrays, sx, chunk, |i0, mut p| {
        let i1 = (i0 + chunk).min(nx + 1);
        let (hx, rest) = p.split_at_mut(1);
        let (hy, rest) = rest.split_at_mut(1);
        let (hz, ps) = rest.split_at_mut(1);
        let (hx, hy, hz) = (&mut *hx[0], &mut *hy[0], &mut *hz[0]);
        for i in i0..i1 {
            let li = (i - i0) * sx;
            for j in 0..=ny {
                let c0 = i * sx + j * sy;
                let l0 = li + j * sy;
                // Hx (i, j+½, k+½)
                if i > 0 && i < nx && j < ny {
                    let h = &mut hx[l0..l0 + nz];
                    let (ez0, ez1) = (&ez[c0..c0 + nz], &ez[c0 + sy..c0 + sy + nz]);
                    let (ey0, ey1) = (&ey[c0..c0 + nz], &ey[c0 + 1..c0 + 1 + nz]);
                    let iy = k.ipy[j];
                    let ipz = &k.ipz[..nz];
                    for q in 0..nz {
                        h[q] -= ch * ((ez1[q] - ez0[q]) * iy - (ey1[q] - ey0[q]) * ipz[q]);
                    }
                }
                // Hy (i+½, j, k+½)
                if i < nx && j > 0 && j < ny {
                    let h = &mut hy[l0..l0 + nz];
                    let (ex0, ex1) = (&ex[c0..c0 + nz], &ex[c0 + 1..c0 + 1 + nz]);
                    let (ez0, ez1) = (&ez[c0..c0 + nz], &ez[c0 + sx..c0 + sx + nz]);
                    let ix = k.ipx[i];
                    let ipz = &k.ipz[..nz];
                    for q in 0..nz {
                        h[q] -= ch * ((ex1[q] - ex0[q]) * ipz[q] - (ez1[q] - ez0[q]) * ix);
                    }
                }
                // Hz (i+½, j+½, k), k in 1..nz
                if i < nx && j < ny {
                    let h = &mut hz[l0 + 1..l0 + nz];
                    let (ey0, ey1) = (&ey[c0 + 1..c0 + nz], &ey[c0 + sx + 1..c0 + sx + nz]);
                    let (ex0, ex1) = (&ex[c0 + 1..c0 + nz], &ex[c0 + sy + 1..c0 + sy + nz]);
                    let (ix, iy) = (k.ipx[i], k.ipy[j]);
                    for q in 0..nz - 1 {
                        h[q] -= ch * ((ey1[q] - ey0[q]) * ix - (ex1[q] - ex0[q]) * iy);
                    }
                }
            }
        }
        // CPML: the extra terms where an axis is in the PML.
        // ps: 0 Hx·y, 1 Hx·z, 2 Hy·z, 3 Hy·x, 4 Hz·x, 5 Hz·y.
        let (b, c) = (&k.bh, &k.chh);
        let (px, py, pz) = (&k.pml_h[0], &k.pml_h[1], &k.pml_h[2]);
        let chunk_i = |lo: usize, hi: usize| (i0.max(lo))..(i1.min(hi));
        let in_chunk = |i: &&usize| **i >= i0 && **i < i1;
        let mut run = |which: usize,
                       is: &mut dyn Iterator<Item = usize>,
                       js: &[usize],
                       ks: &[usize],
                       f: &dyn Fn(usize, usize, usize) -> (f32, usize, f32)| {
            let mut v: Vec<usize> = Vec::new();
            v.extend(is);
            for &i in &v {
                for &j in js {
                    for &q in ks {
                        let c0 = i * sx + j * sy + q;
                        let l = c0 - i0 * sx;
                        let (d, ax_at, sign) = f(i, j, q);
                        let ax = ax_at >> 32;
                        let at = ax_at & 0xffff_ffff;
                        let val = b[ax][at] * ps[which][l] + c[ax][at] * d;
                        ps[which][l] = val;
                        match which {
                            0 | 1 => hx[l] -= ch * sign * val,
                            2 | 3 => hy[l] -= ch * sign * val,
                            _ => hz[l] -= ch * sign * val,
                        }
                    }
                }
            }
        };
        let all_j: Vec<usize> = (0..ny).collect();
        let inner_j: Vec<usize> = (1..ny).collect();
        let all_k: Vec<usize> = (0..nz).collect();
        let inner_k: Vec<usize> = (1..nz).collect();
        let py_n: Vec<usize> = py.iter().copied().filter(|&j| j < ny).collect();
        let py_in: Vec<usize> = py.iter().copied().filter(|&j| j > 0 && j < ny).collect();
        let pz_n: Vec<usize> = pz.iter().copied().filter(|&q| q < nz).collect();
        let at = |ax: usize, n: usize| (ax << 32) | n;
        run(0, &mut chunk_i(1, nx), &py_n, &all_k, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((ez[c0 + sy] - ez[c0]) * k.ipy[j], at(1, j), 1.0)
        });
        run(1, &mut chunk_i(1, nx), &all_j, &pz_n, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((ey[c0 + 1] - ey[c0]) * k.ipz[q], at(2, q), -1.0)
        });
        run(2, &mut chunk_i(0, nx), &inner_j, &pz_n, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((ex[c0 + 1] - ex[c0]) * k.ipz[q], at(2, q), 1.0)
        });
        run(
            3,
            &mut px.iter().filter(in_chunk).copied().filter(|&i| i < nx),
            &inner_j,
            &all_k,
            &|i, j, q| {
                let c0 = i * sx + j * sy + q;
                ((ez[c0 + sx] - ez[c0]) * k.ipx[i], at(0, i), -1.0)
            },
        );
        run(
            4,
            &mut px.iter().filter(in_chunk).copied().filter(|&i| i < nx),
            &all_j,
            &inner_k,
            &|i, j, q| {
                let c0 = i * sx + j * sy + q;
                ((ey[c0 + sx] - ey[c0]) * k.ipx[i], at(0, i), 1.0)
            },
        );
        run(5, &mut chunk_i(0, nx), &py_n, &inner_k, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((ex[c0 + sy] - ex[c0]) * k.ipy[j], at(1, j), -1.0)
        });
        let _ = &py_in;
    });
}

/// E from the curl of H (Ampère), with CPML.
fn update_e(
    k: &Kernel,
    ex: &mut [f32],
    ey: &mut [f32],
    ez: &mut [f32],
    pe: &mut [Vec<f32>],
    hx: &[f32],
    hy: &[f32],
    hz: &[f32],
    chunk: usize,
) {
    let (nx, ny, nz, sx, sy) = (k.nx, k.ny, k.nz, k.sx, k.sy);
    let mut arrays: Vec<&mut [f32]> = vec![ex, ey, ez];
    for p in pe.iter_mut() {
        arrays.push(p);
    }
    par_chunks(arrays, sx, chunk, |i0, mut p| {
        let i1 = (i0 + chunk).min(nx + 1);
        let (ex, rest) = p.split_at_mut(1);
        let (ey, rest) = rest.split_at_mut(1);
        let (ez, ps) = rest.split_at_mut(1);
        let (ex, ey, ez) = (&mut *ex[0], &mut *ey[0], &mut *ez[0]);
        for i in i0..i1 {
            let li = (i - i0) * sx;
            for j in 0..=ny {
                let c0 = i * sx + j * sy;
                let l0 = li + j * sy;
                // Ex (i+½, j, k), k in 1..nz
                if i < nx && j > 0 && j < ny {
                    let e = &mut ex[l0 + 1..l0 + nz];
                    let (ca, cb) = (&k.cax[c0 + 1..c0 + nz], &k.cbx[c0 + 1..c0 + nz]);
                    let (hz0, hzm) = (&hz[c0 + 1..c0 + nz], &hz[c0 - sy + 1..c0 - sy + nz]);
                    let (hy0, hym) = (&hy[c0 + 1..c0 + nz], &hy[c0..c0 + nz - 1]);
                    let iy = k.idy[j];
                    let idz = &k.idz[1..nz];
                    for q in 0..nz - 1 {
                        e[q] = ca[q] * e[q]
                            + cb[q] * ((hz0[q] - hzm[q]) * iy - (hy0[q] - hym[q]) * idz[q]);
                    }
                }
                // Ey (i, j+½, k), k in 1..nz
                if i > 0 && i < nx && j < ny {
                    let e = &mut ey[l0 + 1..l0 + nz];
                    let (ca, cb) = (&k.cay[c0 + 1..c0 + nz], &k.cby[c0 + 1..c0 + nz]);
                    let (hx0, hxm) = (&hx[c0 + 1..c0 + nz], &hx[c0..c0 + nz - 1]);
                    let (hz0, hzm) = (&hz[c0 + 1..c0 + nz], &hz[c0 - sx + 1..c0 - sx + nz]);
                    let ix = k.idx[i];
                    let idz = &k.idz[1..nz];
                    for q in 0..nz - 1 {
                        e[q] = ca[q] * e[q]
                            + cb[q] * ((hx0[q] - hxm[q]) * idz[q] - (hz0[q] - hzm[q]) * ix);
                    }
                }
                // Ez (i, j, k+½), k in 0..nz
                if i > 0 && i < nx && j > 0 && j < ny {
                    let e = &mut ez[l0..l0 + nz];
                    let (ca, cb) = (&k.caz[c0..c0 + nz], &k.cbz[c0..c0 + nz]);
                    let (hy0, hym) = (&hy[c0..c0 + nz], &hy[c0 - sx..c0 - sx + nz]);
                    let (hx0, hxm) = (&hx[c0..c0 + nz], &hx[c0 - sy..c0 - sy + nz]);
                    let (ix, iy) = (k.idx[i], k.idy[j]);
                    for q in 0..nz {
                        e[q] = ca[q] * e[q]
                            + cb[q] * ((hy0[q] - hym[q]) * ix - (hx0[q] - hxm[q]) * iy);
                    }
                }
            }
        }
        // CPML terms. ps: 0 Ex·y, 1 Ex·z, 2 Ey·z, 3 Ey·x, 4 Ez·x, 5 Ez·y.
        let (b, c) = (&k.be, &k.ce);
        let (px, py, pz) = (&k.pml_e[0], &k.pml_e[1], &k.pml_e[2]);
        let chunk_i = |lo: usize, hi: usize| (i0.max(lo))..(i1.min(hi));
        let in_chunk = |i: &&usize| **i >= i0 && **i < i1;
        let mut run = |which: usize,
                       is: &mut dyn Iterator<Item = usize>,
                       js: &[usize],
                       ks: &[usize],
                       f: &dyn Fn(usize, usize, usize) -> (f32, usize, f32)| {
            let mut v: Vec<usize> = Vec::new();
            v.extend(is);
            for &i in &v {
                for &j in js {
                    for &q in ks {
                        let c0 = i * sx + j * sy + q;
                        let l = c0 - i0 * sx;
                        let (d, ax_at, sign) = f(i, j, q);
                        let ax = ax_at >> 32;
                        let at = ax_at & 0xffff_ffff;
                        let val = b[ax][at] * ps[which][l] + c[ax][at] * d;
                        ps[which][l] = val;
                        match which {
                            0 | 1 => ex[l] += k.cbx[c0] * sign * val,
                            2 | 3 => ey[l] += k.cby[c0] * sign * val,
                            _ => ez[l] += k.cbz[c0] * sign * val,
                        }
                    }
                }
            }
        };
        let all_j: Vec<usize> = (0..ny).collect();
        let inner_j: Vec<usize> = (1..ny).collect();
        let all_k: Vec<usize> = (0..nz).collect();
        let inner_k: Vec<usize> = (1..nz).collect();
        let py_n: Vec<usize> = py.iter().copied().filter(|&j| j < ny).collect();
        let py_in: Vec<usize> = py.iter().copied().filter(|&j| j > 0 && j < ny).collect();
        let pz_in: Vec<usize> = pz.iter().copied().filter(|&q| q > 0 && q < nz).collect();
        let at = |ax: usize, n: usize| (ax << 32) | n;
        run(0, &mut chunk_i(0, nx), &py_in, &inner_k, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((hz[c0] - hz[c0 - sy]) * k.idy[j], at(1, j), 1.0)
        });
        run(1, &mut chunk_i(0, nx), &inner_j, &pz_in, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((hy[c0] - hy[c0 - 1]) * k.idz[q], at(2, q), -1.0)
        });
        run(2, &mut chunk_i(1, nx), &all_j, &pz_in, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((hx[c0] - hx[c0 - 1]) * k.idz[q], at(2, q), 1.0)
        });
        run(
            3,
            &mut px
                .iter()
                .filter(in_chunk)
                .copied()
                .filter(|&i| i > 0 && i < nx),
            &all_j,
            &inner_k,
            &|i, j, q| {
                let c0 = i * sx + j * sy + q;
                ((hz[c0] - hz[c0 - sx]) * k.idx[i], at(0, i), -1.0)
            },
        );
        run(
            4,
            &mut px
                .iter()
                .filter(in_chunk)
                .copied()
                .filter(|&i| i > 0 && i < nx),
            &inner_j,
            &all_k,
            &|i, j, q| {
                let c0 = i * sx + j * sy + q;
                ((hy[c0] - hy[c0 - sx]) * k.idx[i], at(0, i), 1.0)
            },
        );
        run(5, &mut chunk_i(1, nx), &py_in, &all_k, &|i, j, q| {
            let c0 = i * sx + j * sy + q;
            ((hx[c0] - hx[c0 - sy]) * k.idy[j], at(1, j), -1.0)
        });
        let _ = &py_n;
    });
}

fn csqrt(z: Cx) -> Cx {
    let r = z.abs().sqrt();
    let a = z.im.atan2(z.re) / 2.0;
    Cx::new(r * a.cos(), r * a.sin())
}

fn clog(z: Cx) -> Cx {
    Cx::new(z.abs().ln(), z.im.atan2(z.re))
}

/// The board's commonest through via, as (drill, pad, antipad) in mil;
/// the antipad measured from the plane clearances around such vias where
/// the board has pours, else the pad plus 12 mil.
pub fn board_via(board: &crate::Board) -> Option<(f64, f64, f64)> {
    use std::collections::HashMap;
    let g = &board.geometry;
    let key = |v: &crate::Via| {
        (
            (v.hole.mil() * 10.0).round() as i64,
            (v.diameter.mil() * 10.0).round() as i64,
        )
    };
    let mut count: HashMap<(i64, i64), usize> = HashMap::new();
    for v in &g.vias {
        *count.entry(key(v)).or_default() += 1;
    }
    let (&(h, d), _) = count.iter().max_by_key(|(k, n)| (**n, k.0))?;
    let (drill, pad) = (h as f64 / 10.0, d as f64 / 10.0);
    // Clearance holes: a hole in another net's poured copper that holds
    // the via's centre; its nearest edge gives the radius.
    let mut radii: Vec<f64> = Vec::new();
    for v in g.vias.iter().filter(|v| key(v) == (h, d)).take(200) {
        let c = [v.at.x.mil(), v.at.y.mil()];
        for p in g
            .polygons
            .iter()
            .filter(|p| p.net.is_some() && p.net != v.net)
        {
            for hole in &p.holes {
                let pts: Vec<[f64; 2]> = hole.iter().map(|q| [q.x.mil(), q.y.mil()]).collect();
                if pts.len() < 3 || !point_in(&pts, c) {
                    continue;
                }
                let r = pts
                    .iter()
                    .zip(pts.iter().cycle().skip(1))
                    .map(|(a, b)| seg_dist(c, *a, *b))
                    .fold(f64::MAX, f64::min);
                if r > pad / 2.0 && r < pad * 3.0 {
                    radii.push(r);
                }
            }
        }
        if radii.len() > 50 {
            break;
        }
    }
    radii.sort_by(f64::total_cmp);
    let antipad = radii.get(radii.len() / 2).map_or(pad + 12.0, |r| 2.0 * r);
    Some((drill, pad, antipad))
}

fn point_in(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
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

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Quarter-wave resonance of a via stub of `len` mil in Er `er`, GHz.
pub fn stub_resonance_ghz(len: f64, er: f64) -> f64 {
    C0 / (4.0 * len * MIL * er.sqrt()) / 1e9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Length, StackLayer};

    /// A board of equal cores, Er 3.6, no loss; `kinds` names each copper
    /// layer: 'S' signal, 'G' plane.
    fn board_of(kinds: &str, core_mil: f64) -> Stackup {
        let mut s = Stackup::default();
        for (i, k) in kinds.chars().enumerate() {
            if i > 0 {
                s.layers.push(StackLayer {
                    name: format!("D{i}"),
                    altium_id: None,
                    kind: LayerKind::Dielectric,
                    thickness: Length::from_mil(core_mil),
                    dielectric_constant: Some(3.6),
                    loss_tangent: None,
                    roughness_um: None,
                    material: None,
                });
            }
            s.layers.push(StackLayer {
                name: if k == 'G' {
                    format!("GND{i}")
                } else {
                    format!("SIG{i}")
                },
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(0.0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        s
    }

    fn board(n: usize, core_mil: f64) -> Stackup {
        let kinds: String = (0..n).map(|i| if i % 2 == 1 { 'G' } else { 'S' }).collect();
        board_of(&kinds, core_mil)
    }

    fn quick(from: u8, to: u8, fmax: f64) -> ViaSetup {
        let mut v = ViaSetup::new(from, to);
        v.fmax_ghz = fmax;
        v.points = 40;
        v
    }

    #[test]
    fn plain_trace_passes_everything() {
        // The same layer twice: no via, the probes and the de-embedding
        // alone. It should be a perfect line.
        let r = solve(&board_of("SGGS", 5.0), &quick(0, 0, 20.0)).unwrap();
        for k in 0..r.f.len() {
            assert!(
                r.s11[k].db() < -35.0,
                "{} GHz S11 {}",
                r.f[k],
                r.s11[k].db()
            );
            assert!(
                r.s21[k].db().abs() < 0.1,
                "{} GHz S21 {}",
                r.f[k],
                r.s21[k].db()
            );
            assert!(
                r.s21[k].deg().abs() < 2.0,
                "{} GHz S21 phase {}",
                r.f[k],
                r.s21[k].deg()
            );
        }
    }

    #[test]
    fn via_is_passive_and_short_at_low_frequency() {
        let r = solve(&board_of("SGGS", 5.0), &quick(0, 3, 20.0)).unwrap();
        for k in 0..r.f.len() {
            let p = r.s11[k].abs().powi(2) + r.s21[k].abs().powi(2);
            assert!(p < 1.05 && p > 0.9, "{} GHz |S|² {p}", r.f[k]);
        }
        // A short through-via is nearly transparent at 1 GHz and its
        // reflection grows with frequency.
        let k1 = r.f.iter().position(|f| *f >= 1.0).unwrap();
        assert!(r.s11[k1].db() < -30.0, "S11 at 1 GHz {}", r.s11[k1].db());
        assert!(r.s11[r.f.len() - 1].db() > r.s11[k1].db() + 10.0);
        assert!((r.stub).abs() < 1e-9);
    }

    #[test]
    fn stub_resonates_and_backdrill_removes_it() {
        // Top to layer 2 with the barrel left down to layer 7: a 60 mil
        // stub. A quarter-wave stub notches |S21|; the planes the stub
        // passes load it, so the notch falls below the bare
        // c / (4 L √Er), but not by half.
        let b = board_of("SGSGGGGS", 12.0);
        let big = |v: &mut ViaSetup| {
            v.drill = 12.0;
            v.pad = 24.0;
            v.antipad = 40.0;
            v.return_dist = 45.0;
            v.cell = 4.0;
        };
        let mut v = quick(0, 2, 30.0);
        big(&mut v);
        let t = std::time::Instant::now();
        let r = solve(&b, &v).unwrap();
        eprintln!(
            "stub: {} cells, {} steps, {:.1} s",
            r.cells,
            r.steps,
            t.elapsed().as_secs_f64()
        );
        assert!((r.stub - 60.0).abs() < 1e-6, "stub {}", r.stub);
        let bare = stub_resonance_ghz(60.0, 3.6);
        let (f, db) = r.notch().expect("a stub notch");
        assert!(
            f < bare && f > 0.5 * bare,
            "notch {f} GHz, bare quarter wave {bare}"
        );
        assert!(db < -6.0, "notch only {db} dB");
        v.backdrill = true;
        let r = solve(&b, &v).unwrap();
        assert!(r.notch().is_none(), "{:?}", r.notch());
        let k = r.f.iter().position(|x| *x >= f).unwrap();
        assert!(
            r.s21[k].db() > -1.5,
            "backdrilled S21 at {f} GHz {}",
            r.s21[k].db()
        );
    }

    #[test]
    fn stack_positions() {
        let s = stack(&board(4, 10.0));
        assert_eq!(s.sheet.len(), 4);
        assert!((s.sheet[3] - 30.0).abs() < 1e-9, "{:?}", s.sheet);
        assert_eq!(s.slabs.len(), 3);
    }

    #[test]
    fn mesh_is_graded() {
        let m = mesh_axis(-100.0, 100.0, -10.0, 10.0, 0.0, 1.0, 8.0, 1.25);
        assert!(m.contains(&0.0));
        assert!(m.windows(2).all(|p| p[1] > p[0]));
        let d: Vec<f64> = m.windows(2).map(|p| p[1] - p[0]).collect();
        assert!(d.iter().all(|&x| x <= 8.0 + 1e-9));
        assert!((m[0] + 100.0).abs() < 1e-9 && (m[m.len() - 1] - 100.0).abs() < 1e-9);
    }
}
