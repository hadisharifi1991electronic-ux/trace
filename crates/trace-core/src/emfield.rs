//! Electromagnetic field maps: the fields a 3D solve found, on cut planes
//! through the structure, and the pictures made of them (a dB colour map
//! with a scale bar, as a still SVG page or an animated GIF).
//!
//! The FDTD solve keeps, at each frequency asked for, the phasors of all
//! six field components at every node of the chosen planes (the Yee
//! grid's staggered components averaged onto the nodes). From those come:
//! - |E| (V/m) and |H| (A/m), the peak of the sinusoidal field;
//! - the energy density, time averaged: w = ¼ (ε|E|² + μ|H|²) (J/m³);
//! - the surface current density on copper, |J_s| = |n × H| (A/m), the
//!   tangential H at the metal (on a sheet both faces carry current; this
//!   is the side the plane sits on);
//! - any of them at an instant of the cycle, Re{F e^{jωt}}: stepping the
//!   phase through a cycle animates the wave (as HFSS's field animation).
//!
//! Fields are scaled to 1 W of power incident on port 1 (for a pair, of
//! the differential wave), so maps from different solves compare.

use crate::fdtd::Cx;
use std::fmt::Write;

const EPS0: f64 = 8.854_187_812_8e-12;
const MU0: f64 = 1.256_637_062_12e-6;

/// Which way a plane is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cut {
    /// Horizontal, at depth `z` (mil below the top of the board; air
    /// above is negative).
    Xy { z: f64 },
    /// Vertical along the traces, at `y`.
    Xz { y: f64 },
    /// Vertical across the traces, at `x`.
    Yz { x: f64 },
}

/// One cut plane's fields.
#[derive(Clone, Debug)]
pub struct Plane {
    pub name: String,
    pub cut: Cut,
    /// Node coordinates, mil: `u` across, `v` down the picture (y for a
    /// horizontal plane, depth for a vertical one).
    pub u: Vec<f64>,
    pub v: Vec<f64>,
    /// Phasors per node (row-major: v index × u count + u index), each
    /// component (x, y, z) as (re, im).
    pub e: [Vec<[f32; 2]>; 3],
    pub h: [Vec<[f32; 2]>; 3],
    /// Relative permittivity at each node.
    pub er: Vec<f32>,
    /// Copper at each node.
    pub metal: Vec<bool>,
}

/// The fields of one solve at one frequency.
#[derive(Clone, Debug)]
pub struct FieldMap {
    pub f_ghz: f64,
    pub title: String,
    pub planes: Vec<Plane>,
    /// Outlines to draw over the picture (pads, barrels, traces), per
    /// plane: polylines in (u, v) mil.
    pub outlines: Vec<Vec<Vec<[f64; 2]>>>,
}

/// What to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    E,
    H,
    Energy,
    Current,
}

impl Quantity {
    pub const ALL: [Quantity; 4] = [
        Quantity::E,
        Quantity::H,
        Quantity::Energy,
        Quantity::Current,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Quantity::E => "|E| electric field",
            Quantity::H => "|H| magnetic field",
            Quantity::Energy => "energy density",
            Quantity::Current => "surface current density",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Quantity::E => "E",
            Quantity::H => "H",
            Quantity::Energy => "energy",
            Quantity::Current => "current",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Quantity::E => "V/m",
            Quantity::H | Quantity::Current => "A/m",
            Quantity::Energy => "J/m³",
        }
    }

    /// dB per decade: 20 for field amplitudes, 10 for energy (a power).
    fn db_per_decade(self) -> f64 {
        if self == Quantity::Energy {
            10.0
        } else {
            20.0
        }
    }

    pub fn parse(s: &str) -> Option<Quantity> {
        match s.to_ascii_lowercase().as_str() {
            "e" => Some(Quantity::E),
            "h" => Some(Quantity::H),
            "w" | "energy" | "u" => Some(Quantity::Energy),
            "j" | "current" | "js" => Some(Quantity::Current),
            _ => None,
        }
    }
}

fn mag(c: &[Vec<[f32; 2]>; 3], i: usize, phase: Option<f64>, comps: &[usize]) -> f64 {
    match phase {
        None => comps
            .iter()
            .map(|&k| {
                let [a, b] = c[k][i];
                (a as f64).powi(2) + (b as f64).powi(2)
            })
            .sum::<f64>()
            .sqrt(),
        Some(p) => {
            let (s, co) = p.sin_cos();
            comps
                .iter()
                .map(|&k| {
                    let [a, b] = c[k][i];
                    (a as f64 * co - b as f64 * s).powi(2)
                })
                .sum::<f64>()
                .sqrt()
        }
    }
}

impl Plane {
    /// The quantity at every node (NaN where it has no meaning: current
    /// off the copper). `phase`: None for the peak (or, for energy, the
    /// time average), else the instant ωt in radians.
    pub fn values(&self, q: Quantity, phase: Option<f64>) -> Vec<f64> {
        let n = self.u.len() * self.v.len();
        let all = [0, 1, 2];
        // The copper's own tangential components.
        let tang: &[usize] = match self.cut {
            Cut::Xy { .. } => &[0, 1],
            Cut::Xz { .. } => &[0, 1, 2],
            Cut::Yz { .. } => &[0, 1, 2],
        };
        (0..n)
            .map(|i| match q {
                Quantity::E => mag(&self.e, i, phase, &all),
                Quantity::H => mag(&self.h, i, phase, &all),
                Quantity::Energy => {
                    let e = mag(&self.e, i, phase, &all);
                    let h = mag(&self.h, i, phase, &all);
                    let k = if phase.is_none() { 0.25 } else { 0.5 };
                    k * (EPS0 * self.er[i] as f64 * e * e + MU0 * h * h)
                }
                Quantity::Current => {
                    if self.metal[i] {
                        mag(&self.h, i, phase, tang)
                    } else {
                        f64::NAN
                    }
                }
            })
            .collect()
    }

    /// Whether copper on this plane shows a current at all.
    pub fn has_copper(&self) -> bool {
        self.metal.iter().any(|&m| m)
    }
}

impl FieldMap {
    /// The largest value of a quantity over every plane (peak in time),
    /// the top of a shared colour scale.
    pub fn top(&self, q: Quantity) -> f64 {
        self.planes
            .iter()
            .flat_map(|p| p.values(q, None))
            .filter(|v| v.is_finite())
            .fold(0.0, f64::max)
            * if q == Quantity::Energy { 2.0 } else { 1.0 }
    }
}

// ---------------------------------------------------------------------
// Pictures.

/// An RGB picture.
#[derive(Clone, Debug)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Raster {
    pub fn new(w: usize, h: usize, fill: [u8; 3]) -> Raster {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            rgb.extend_from_slice(&fill);
        }
        Raster { w, h, rgb }
    }

    pub fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn line(&mut self, a: [f64; 2], b: [f64; 2], c: [u8; 3]) {
        let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize).max(1);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            self.put(
                (a[0] + (b[0] - a[0]) * t).round() as i64,
                (a[1] + (b[1] - a[1]) * t).round() as i64,
                c,
            );
        }
    }

    /// Text in the stroke font, `size` px cap height, baseline at `at`.
    pub fn text(&mut self, at: [f64; 2], size: f64, s: &str, c: [u8; 3]) {
        let (lines, _) = crate::font::strokes(s);
        for l in lines {
            for w in l.windows(2) {
                let p = |q: [f64; 2]| [at[0] + q[0] * size, at[1] - q[1] * size];
                self.line(p(w[0]), p(w[1]), c);
            }
        }
    }

    pub fn text_width(size: f64, s: &str) -> f64 {
        crate::font::strokes(s).1 * size
    }

    pub fn fill(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]) {
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                self.put(x as i64, y as i64, c);
            }
        }
    }

    /// Copies `o` in at (x, y).
    pub fn blit(&mut self, o: &Raster, x: usize, y: usize) {
        for r in 0..o.h {
            for c in 0..o.w {
                let i = (r * o.w + c) * 3;
                self.put(
                    (x + c) as i64,
                    (y + r) as i64,
                    [o.rgb[i], o.rgb[i + 1], o.rgb[i + 2]],
                );
            }
        }
    }
}

/// Colour map stops (dark violet through red and orange to pale yellow,
/// like "inferno": even steps in lightness, readable in grey).
const STOPS: [[f64; 3]; 9] = [
    [0.0, 0.0, 4.0],
    [31.0, 12.0, 72.0],
    [85.0, 15.0, 109.0],
    [136.0, 34.0, 106.0],
    [186.0, 54.0, 85.0],
    [227.0, 89.0, 51.0],
    [249.0, 140.0, 10.0],
    [249.0, 201.0, 50.0],
    [252.0, 255.0, 164.0],
];

/// Colour for t in 0 to 1.
pub fn colour(t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f64;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f64;
    let a = STOPS[i];
    let b = STOPS[i + 1];
    [
        (a[0] + (b[0] - a[0]) * f).round() as u8,
        (a[1] + (b[1] - a[1]) * f).round() as u8,
        (a[2] + (b[2] - a[2]) * f).round() as u8,
    ]
}

/// Where a value falls on a dB scale `range` dB deep below `top`.
pub fn scale_t(v: f64, top: f64, range: f64, per_decade: f64) -> f64 {
    if v.is_nan() || top.is_nan() || v <= 0.0 || top <= 0.0 {
        return 0.0;
    }
    1.0 + per_decade * (v / top).log10() / range
}

pub const METAL: [u8; 3] = [150, 150, 160];
pub const AIR: [u8; 3] = [24, 24, 30];

/// How a plane is drawn: pixels per mil across and down.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub px_u: f64,
    pub px_v: f64,
    pub w: usize,
    pub h: usize,
}

impl Plane {
    /// The picture's size for about `width` pixels across. Vertical cuts
    /// through a thin board are stretched to at least `min_h` pixels high.
    pub fn frame(&self, width: usize, min_h: usize) -> Frame {
        let su = (self.u[self.u.len() - 1] - self.u[0]).max(1e-6);
        let sv = (self.v[self.v.len() - 1] - self.v[0]).max(1e-6);
        let px = width as f64 / su;
        let mut pv = px;
        if (sv * pv) < min_h as f64 && !matches!(self.cut, Cut::Xy { .. }) {
            pv = min_h as f64 / sv;
        }
        Frame {
            px_u: px,
            px_v: pv,
            w: width,
            h: ((sv * pv).round() as usize).max(8),
        }
    }

    /// The plane in colour: `vals` from `values`, on a dB scale.
    pub fn draw(
        &self,
        q: Quantity,
        vals: &[f64],
        top: f64,
        range_db: f64,
        fr: Frame,
        outlines: &[Vec<[f64; 2]>],
    ) -> Raster {
        let nu = self.u.len();
        // Nearest node for each pixel column and row.
        let nearest = |axis: &[f64], p: f64| -> usize {
            let k = axis.partition_point(|&a| a < p);
            if k == 0 {
                0
            } else if k >= axis.len() {
                axis.len() - 1
            } else if (axis[k] - p) < (p - axis[k - 1]) {
                k
            } else {
                k - 1
            }
        };
        let cols: Vec<usize> = (0..fr.w)
            .map(|x| nearest(&self.u, self.u[0] + (x as f64 + 0.5) / fr.px_u))
            .collect();
        let rows: Vec<usize> = (0..fr.h)
            .map(|y| nearest(&self.v, self.v[0] + (y as f64 + 0.5) / fr.px_v))
            .collect();
        let mut r = Raster::new(fr.w, fr.h, AIR);
        for (y, &j) in rows.iter().enumerate() {
            for (x, &i) in cols.iter().enumerate() {
                let n = j * nu + i;
                let v = vals[n];
                let c = if v.is_nan() {
                    if self.metal[n] {
                        METAL
                    } else if self.er[n] > 1.01 {
                        [40, 44, 40]
                    } else {
                        AIR
                    }
                } else if self.metal[n] && q != Quantity::Current {
                    METAL
                } else {
                    colour(scale_t(v, top, range_db, q.db_per_decade()))
                };
                r.put(x as i64, y as i64, c);
            }
        }
        let to_px = |p: [f64; 2]| [(p[0] - self.u[0]) * fr.px_u, (p[1] - self.v[0]) * fr.px_v];
        for l in outlines {
            for w in l.windows(2) {
                r.line(to_px(w[0]), to_px(w[1]), [220, 220, 230]);
            }
        }
        r
    }
}

/// A vertical scale bar `h` px high: colours, dB ticks, the top value.
fn scale_bar(q: Quantity, top: f64, range: f64, h: usize) -> Raster {
    let w = 150;
    let mut r = Raster::new(w, h, [255, 255, 255]);
    let (y0, y1) = (26usize, h.saturating_sub(10).max(40));
    for y in y0..y1 {
        let t = 1.0 - (y - y0) as f64 / (y1 - y0) as f64;
        r.fill(4, y, 22, y + 1, colour(t));
    }
    let step = if range > 30.0 { 10.0 } else { 5.0 };
    let mut d = 0.0;
    while d <= range + 1e-9 {
        let y = y0 as f64 + d / range * (y1 - y0) as f64;
        r.line([22.0, y], [27.0, y], [0, 0, 0]);
        let lab = if d == 0.0 {
            "0 dB".to_string()
        } else {
            format!("-{d:.0} dB")
        };
        r.text([30.0, y + 4.0], 8.0, &lab, [0, 0, 0]);
        d += step;
    }
    r.text(
        [4.0, 10.0],
        8.0,
        &format!("{} {}", si(top), q.unit()),
        [0, 0, 0],
    );
    r
}

/// A number with an SI prefix, 3 figures.
pub fn si(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let p = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "u"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
    ];
    for (k, s) in p {
        if v.abs() >= k {
            let x = v / k;
            let d = if x.abs() >= 100.0 {
                0
            } else if x.abs() >= 10.0 {
                1
            } else {
                2
            };
            return format!("{x:.d$} {s}", d = d).trim_end().to_string();
        }
    }
    format!("{v:.2e}")
}

// ---------------------------------------------------------------------
// Pages.

impl FieldMap {
    /// A still page: for each quantity asked, every plane on one shared
    /// dB scale `range` deep.
    pub fn svg(&self, quantities: &[Quantity], range: f64) -> String {
        let width = 520usize;
        let mut body = String::new();
        let mut y = 74.0;
        let mut page_w: f64 = 2.0 * (width as f64 + 40.0) + 190.0;
        for &q in quantities {
            let top = self.top(q);
            if top.is_nan() || top <= 0.0 {
                continue;
            }
            let shown: Vec<usize> = (0..self.planes.len())
                .filter(|&i| q != Quantity::Current || self.planes[i].has_copper())
                .collect();
            if shown.is_empty() {
                continue;
            }
            let _ = writeln!(
                body,
                r#"<text x="20" y="{:.0}" font-size="16" font-weight="bold">{} ({}), {} dB scale</text>"#,
                y + 4.0,
                q.label(),
                q.unit(),
                range
            );
            y += 18.0;
            let y_sec = y;
            let mut col = 0;
            let mut row_h: f64 = 0.0;
            for &pi in &shown {
                let p = &self.planes[pi];
                let fr = p.frame(width, 150);
                let vals = p.values(q, None);
                let img = p.draw(
                    q,
                    &vals,
                    if q == Quantity::Energy {
                        top / 2.0
                    } else {
                        top
                    },
                    range,
                    fr,
                    &self.outlines[pi],
                );
                let x = 20.0 + col as f64 * (width as f64 + 40.0);
                let peak = vals
                    .iter()
                    .filter(|v| v.is_finite())
                    .fold(0.0f64, |a, &b| a.max(b));
                let _ = writeln!(
                    body,
                    r#"<text x="{x:.0}" y="{:.0}" font-size="12">{} (peak {} {})</text>"#,
                    y + 12.0,
                    esc(&p.name),
                    si(peak),
                    q.unit()
                );
                let _ = writeln!(
                    body,
                    r#"<image x="{x:.0}" y="{:.0}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                    y + 16.0,
                    img.w,
                    img.h,
                    base64(&png(&img))
                );
                // Axis: the span in mil.
                let su = p.u[p.u.len() - 1] - p.u[0];
                let _ = writeln!(
                    body,
                    r#"<text x="{x:.0}" y="{:.0}" font-size="10" fill="gray">{:.0} mil across{}</text>"#,
                    y + 16.0 + img.h as f64 + 12.0,
                    su,
                    if fr.px_v > fr.px_u * 1.01 {
                        format!(", depth drawn {:.1}× taller", fr.px_v / fr.px_u)
                    } else {
                        String::new()
                    }
                );
                row_h = row_h.max(img.h as f64 + 36.0);
                col += 1;
                if col == 2 {
                    col = 0;
                    y += row_h;
                    row_h = 0.0;
                }
            }
            if col != 0 {
                y += row_h;
            }
            let bar = scale_bar(q, top, range, ((y - y_sec).max(160.0) as usize).min(400));
            let bx = 20.0 + 2.0 * (width as f64 + 40.0);
            let _ = writeln!(
                body,
                r#"<image x="{bx:.0}" y="{:.0}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                y_sec + 16.0,
                bar.w,
                bar.h,
                base64(&png(&bar))
            );
            page_w = page_w.max(bx + bar.w as f64 + 20.0);
            y += 20.0;
        }
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_w:.0}" height="{:.0}" font-family="sans-serif">
<rect width="100%" height="100%" fill="white"/>
<text x="20" y="28" font-size="18" font-weight="bold">{} at {:.3} GHz</text>
<text x="20" y="48" font-size="11" fill="gray">Peak of the sinusoidal field for 1 W incident on port 1; energy time averaged; copper grey (its current in the current maps).</text>
{body}</svg>
"#,
            y + 10.0,
            esc(&self.title),
            self.f_ghz
        )
    }

    /// One plane's field through a cycle, `frames` pictures, as an
    /// animated GIF with its scale bar and a label.
    pub fn gif(&self, plane: usize, q: Quantity, range: f64, frames: usize) -> Vec<u8> {
        let p = &self.planes[plane];
        let fr = p.frame(640, 200);
        // Peak over the cycle: the phasor's magnitude (twice the average
        // for energy).
        let top = p
            .values(q, None)
            .iter()
            .filter(|v| v.is_finite())
            .fold(0.0f64, |a, &b| a.max(b))
            * if q == Quantity::Energy { 2.0 } else { 1.0 };
        let bar = scale_bar(q, top, range, fr.h.max(160));
        let (w, h) = (fr.w + bar.w + 10, fr.h.max(bar.h) + 30);
        let frames = frames.max(2);
        let pics: Vec<Raster> = (0..frames)
            .map(|k| {
                let ph = std::f64::consts::TAU * k as f64 / frames as f64;
                let vals = p.values(q, Some(ph));
                let img = p.draw(q, &vals, top, range, fr, &self.outlines[plane]);
                let mut page = Raster::new(w, h, [255, 255, 255]);
                page.text(
                    [6.0, 18.0],
                    11.0,
                    &format!(
                        "{} {}  {:.2} GHz  {} deg",
                        p.name,
                        q.short(),
                        self.f_ghz,
                        (360.0 * k as f64 / frames as f64).round()
                    ),
                    [0, 0, 0],
                );
                page.blit(&img, 0, 28);
                page.blit(&bar, fr.w + 10, 28);
                page
            })
            .collect();
        gif(&pics, 8)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------
// Encoders.

/// PNG (8-bit RGB).
pub fn png(r: &Raster) -> Vec<u8> {
    use std::io::Write as _;
    let mut raw = Vec::with_capacity((r.w * 3 + 1) * r.h);
    for y in 0..r.h {
        raw.push(0);
        raw.extend_from_slice(&r.rgb[y * r.w * 3..(y + 1) * r.w * 3]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    let data = z.finish().unwrap_or_default();
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let chunk = |out: &mut Vec<u8>, kind: &[u8], d: &[u8]| {
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        let mut c = Vec::with_capacity(d.len() + 4);
        c.extend_from_slice(kind);
        c.extend_from_slice(d);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(r.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(r.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

pub fn base64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(d.len() * 4 / 3 + 4);
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// The palette an animation uses: 240 map colours, then fixed ones.
fn palette() -> Vec<[u8; 3]> {
    let mut p: Vec<[u8; 3]> = (0..240).map(|k| colour(k as f64 / 239.0)).collect();
    p.extend_from_slice(&[
        METAL,
        AIR,
        [40, 44, 40],
        [255, 255, 255],
        [0, 0, 0],
        [220, 220, 230],
    ]);
    while p.len() < 256 {
        p.push([128, 128, 128]);
    }
    p
}

/// Index of the palette colour nearest `c`.
fn nearest_index(
    pal: &[[u8; 3]],
    c: [u8; 3],
    cache: &mut std::collections::HashMap<[u8; 3], u8>,
) -> u8 {
    if let Some(&i) = cache.get(&c) {
        return i;
    }
    let mut best = (u32::MAX, 0u8);
    for (i, p) in pal.iter().enumerate() {
        let d = (0..3)
            .map(|k| (p[k] as i32 - c[k] as i32).pow(2) as u32)
            .sum::<u32>();
        if d < best.0 {
            best = (d, i as u8);
        }
    }
    cache.insert(c, best.1);
    best.1
}

/// An animated GIF looping forever, `delay` hundredths of a second per
/// frame. All frames the same size.
pub fn gif(frames: &[Raster], delay: u16) -> Vec<u8> {
    gif_with(frames, delay, &palette())
}

/// An animated GIF with a palette made for its own pictures (median cut
/// over all frames): for shaded scenes, where the map's palette bands.
pub fn gif_adaptive(frames: &[Raster], delay: u16) -> Vec<u8> {
    let mut px: Vec<[u8; 3]> = Vec::new();
    for f in frames {
        for c in f.rgb.chunks(3).step_by(5) {
            px.push([c[0], c[1], c[2]]);
        }
    }
    let mut boxes: Vec<Vec<[u8; 3]>> = vec![px];
    while boxes.len() < 256 {
        // Split the box with the widest channel spread (weighted by size).
        let mut best = None;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            for ch in 0..3 {
                let (lo, hi) = b
                    .iter()
                    .fold((255u8, 0u8), |(l, h), c| (l.min(c[ch]), h.max(c[ch])));
                let score = (hi - lo) as f64 * (b.len() as f64).sqrt();
                if hi > lo && best.is_none_or(|(_, _, s)| score > s) {
                    best = Some((i, ch, score));
                }
            }
        }
        let Some((i, ch, _)) = best else { break };
        let mut b = boxes.swap_remove(i);
        b.sort_unstable_by_key(|c| c[ch]);
        let other = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(other);
    }
    let mut pal: Vec<[u8; 3]> = boxes
        .iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let n = b.len() as u64;
            let s = b.iter().fold([0u64; 3], |a, c| {
                [a[0] + c[0] as u64, a[1] + c[1] as u64, a[2] + c[2] as u64]
            });
            [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
        })
        .collect();
    while pal.len() < 256 {
        pal.push([0, 0, 0]);
    }
    gif_with(frames, delay, &pal)
}

fn gif_with(frames: &[Raster], delay: u16, pal: &[[u8; 3]]) -> Vec<u8> {
    let (w, h) = (frames[0].w as u16, frames[0].h as u16);
    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&[0xf7, 0, 0]);
    for c in pal {
        out.extend_from_slice(c);
    }
    // Loop forever.
    out.extend_from_slice(&[0x21, 0xff, 0x0b]);
    out.extend_from_slice(b"NETSCAPE2.0");
    out.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
    let mut cache = std::collections::HashMap::new();
    for f in frames {
        out.extend_from_slice(&[0x21, 0xf9, 0x04, 0x04]);
        out.extend_from_slice(&delay.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.push(0x2c);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.push(0);
        let idx: Vec<u8> = f
            .rgb
            .chunks(3)
            .map(|c| nearest_index(pal, [c[0], c[1], c[2]], &mut cache))
            .collect();
        out.push(8);
        let data = lzw(&idx, 8);
        for b in data.chunks(255) {
            out.push(b.len() as u8);
            out.extend_from_slice(b);
        }
        out.push(0);
    }
    out.push(0x3b);
    out
}

/// GIF's LZW, variable code width up to 12 bits.
fn lzw(data: &[u8], min: u8) -> Vec<u8> {
    let clear = 1u16 << min;
    let eoi = clear + 1;
    let mut dict: std::collections::HashMap<(u16, u8), u16> = std::collections::HashMap::new();
    let mut next = eoi + 1;
    let mut width = min as u32 + 1;
    let mut out = Vec::new();
    let (mut acc, mut nbits) = (0u32, 0u32);
    let mut emit = |code: u16, width: u32, out: &mut Vec<u8>| {
        acc |= (code as u32) << nbits;
        nbits += width;
        while nbits >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            nbits -= 8;
        }
    };
    emit(clear, width, &mut out);
    let mut it = data.iter();
    let Some(&first) = it.next() else {
        emit(eoi, width, &mut out);
        if nbits > 0 {
            out.push(acc as u8);
        }
        return out;
    };
    let mut cur = first as u16;
    for &b in it {
        if let Some(&c) = dict.get(&(cur, b)) {
            cur = c;
            continue;
        }
        emit(cur, width, &mut out);
        if next < 4096 {
            dict.insert((cur, b), next);
            next += 1;
            if next > (1 << width) && width < 12 {
                width += 1;
            }
        } else {
            emit(clear, width, &mut out);
            dict.clear();
            next = eoi + 1;
            width = min as u32 + 1;
        }
        cur = b as u16;
    }
    emit(cur, width, &mut out);
    emit(eoi, width, &mut out);
    if nbits > 0 {
        out.push(acc as u8);
    }
    out
}

/// A phasor from the solver's complex type.
pub fn pack(c: Cx) -> [f32; 2] {
    [c.re as f32, c.im as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzw_round_trips_through_a_decoder() {
        // Decode with a straightforward GIF LZW decoder.
        let data: Vec<u8> = (0..5000).map(|i| ((i * 7 / 13) % 251) as u8).collect();
        let enc = lzw(&data, 8);
        let mut bits = 0u64;
        let mut nb = 0u32;
        let mut pos = 0;
        let mut width = 9u32;
        let mut dict: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
        dict.push(vec![]);
        dict.push(vec![]);
        let mut prev: Option<Vec<u8>> = None;
        let mut out = Vec::new();
        loop {
            while nb < width && pos < enc.len() {
                bits |= (enc[pos] as u64) << nb;
                nb += 8;
                pos += 1;
            }
            if nb < width {
                break;
            }
            let code = (bits & ((1 << width) - 1)) as usize;
            bits >>= width;
            nb -= width;
            if code == 256 {
                dict.truncate(258);
                width = 9;
                prev = None;
                continue;
            }
            if code == 257 {
                break;
            }
            let entry = if code < dict.len() {
                dict[code].clone()
            } else {
                let mut p = prev.clone().unwrap();
                p.push(p[0]);
                p
            };
            out.extend_from_slice(&entry);
            if let Some(mut p) = prev {
                p.push(entry[0]);
                dict.push(p);
                if dict.len() == (1 << width) && width < 12 {
                    width += 1;
                }
            }
            prev = Some(entry);
        }
        assert_eq!(out, data);
    }

    #[test]
    fn colour_scale_ends() {
        assert_eq!(colour(0.0), [0, 0, 4]);
        assert_eq!(colour(1.0), [252, 255, 164]);
        assert!((scale_t(0.01, 1.0, 40.0, 20.0) - 0.0).abs() < 1e-9);
        assert!((scale_t(1.0, 1.0, 40.0, 20.0) - 1.0).abs() < 1e-9);
    }
}
