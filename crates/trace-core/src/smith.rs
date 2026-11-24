//! Smith chart: reflection coefficients (S11, S22, or a one-port) drawn on
//! the impedance chart, with markers that read out Z, VSWR and return
//! loss. The app draws the same grid; the CLI writes it as SVG.

use crate::fdtd::Cx;
use crate::touchstone::{gamma_to_z, Network};

/// One curve: Γ against frequency.
#[derive(Clone, Debug)]
pub struct Curve {
    pub name: String,
    /// (GHz, Γ).
    pub points: Vec<(f64, Cx)>,
}

/// Γ = S(i,j) (1-based) of a network.
pub fn from_network(n: &Network, i: usize, j: usize, name: &str) -> Result<Curve, String> {
    if i == 0 || j == 0 || i > n.ports || j > n.ports {
        return Err(format!("S{i}{j} is not in a {}-port network", n.ports));
    }
    let k = (i - 1) * n.ports + (j - 1);
    Ok(Curve {
        name: name.to_string(),
        points: n.f.iter().zip(&n.s).map(|(&f, s)| (f, s[k])).collect(),
    })
}

/// Parses "S11", "s22", "11": (i, j).
pub fn parse_param(p: &str) -> Option<(usize, usize)> {
    let d = p.trim().trim_start_matches(['S', 's']);
    let b = d.as_bytes();
    if b.len() == 2 && b.iter().all(u8::is_ascii_digit) {
        Some(((b[0] - b'0') as usize, (b[1] - b'0') as usize))
    } else {
        let (a, c) = d.split_once(',')?;
        Some((a.parse().ok()?, c.parse().ok()?))
    }
}

/// Γ at `f` GHz, linear between points; None outside the curve.
pub fn at(c: &Curve, f: f64) -> Option<Cx> {
    let p = &c.points;
    if p.is_empty() || f < p[0].0 - 1e-9 || f > p[p.len() - 1].0 + 1e-9 {
        return None;
    }
    let k = p.partition_point(|x| x.0 < f).min(p.len() - 1);
    if k == 0 || (p[k].0 - f).abs() < 1e-12 {
        return Some(p[k].1);
    }
    let t = (f - p[k - 1].0) / (p[k].0 - p[k - 1].0);
    Some(p[k - 1].1.scale(1.0 - t).add(p[k].1.scale(t)))
}

/// What a marker reads: impedance, VSWR, return loss.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub f: f64,
    pub gamma: Cx,
    pub z: Cx,
    pub vswr: f64,
    pub rl_db: f64,
}

pub fn read(g: Cx, f: f64, z0: f64) -> Reading {
    let m = g.abs().min(0.999_999);
    Reading {
        f,
        gamma: g,
        z: gamma_to_z(g, z0),
        vswr: (1.0 + m) / (1.0 - m),
        rl_db: -g.db(),
    }
}

impl Reading {
    pub fn text(&self) -> String {
        format!(
            "{:.3} GHz: Z = {:.1} {} j{:.1} Ω, |Γ| {:.3} at {:.0}°, VSWR {:.2}, RL {:.1} dB",
            self.f,
            self.z.re,
            if self.z.im < 0.0 { "-" } else { "+" },
            self.z.im.abs(),
            self.gamma.abs(),
            self.gamma.deg(),
            self.vswr,
            self.rl_db
        )
    }
}

/// Resistance circles and reactance arcs of the chart.
pub const R_LINES: [f64; 6] = [0.0, 0.2, 0.5, 1.0, 2.0, 5.0];
pub const X_LINES: [f64; 5] = [0.2, 0.5, 1.0, 2.0, 5.0];

fn to_gamma(z: Cx) -> Cx {
    let one = Cx::new(1.0, 0.0);
    z.add(one.scale(-1.0)).div(z.add(one))
}

/// The chart's grid as polylines in the Γ plane (unit circle), each with
/// its normalised value: resistance circles, then reactance arcs (±x),
/// then the real axis.
pub fn grid() -> Vec<(f64, Vec<[f64; 2]>)> {
    let n = 160;
    let mut out = Vec::new();
    for &r in &R_LINES {
        let pts = (0..=n)
            .map(|k| {
                let th = -std::f64::consts::FRAC_PI_2 * 0.9999
                    + std::f64::consts::PI * 0.9999 * k as f64 / n as f64;
                let g = to_gamma(Cx::new(r, th.tan()));
                [g.re, g.im]
            })
            .collect();
        out.push((r, pts));
    }
    for &x in &X_LINES {
        for sx in [x, -x] {
            let pts = (0..=n)
                .map(|k| {
                    let th = std::f64::consts::FRAC_PI_2 * 0.9999 * k as f64 / n as f64;
                    let g = to_gamma(Cx::new(th.tan(), sx));
                    [g.re, g.im]
                })
                .collect();
            out.push((sx, pts));
        }
    }
    out.push((0.0, vec![[-1.0, 0.0], [1.0, 0.0]]));
    out
}

/// Where a grid value's label goes (in the Γ plane): resistance labels on
/// the real axis, reactance labels just inside the rim.
pub fn labels() -> Vec<([f64; 2], String)> {
    let mut out = Vec::new();
    for &r in &R_LINES[1..] {
        let g = to_gamma(Cx::new(r, 0.0));
        out.push(([g.re, g.im], fmt_val(r)));
    }
    for &x in &X_LINES {
        for sx in [x, -x] {
            let g = to_gamma(Cx::new(0.0, sx)).scale(1.06);
            out.push((
                [g.re, g.im],
                format!("{}{}", if sx < 0.0 { "-" } else { "+" }, fmt_val(x)),
            ));
        }
    }
    out
}

fn fmt_val(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// Curve colours (also used by the app).
pub const COLOURS: [&str; 6] = [
    "#2b6cb0", "#c05621", "#2f855a", "#9b2c2c", "#6b46c1", "#b7791f",
];

/// The chart as an SVG page: grid, curves, markers at `markers` GHz on
/// every curve, and a table of what the markers read.
pub fn svg(curves: &[Curve], markers: &[f64], z0: f64, title: &str) -> String {
    let size = 560.0;
    let r = 240.0;
    let (cx, cy) = (size / 2.0, 72.0 + r);
    let px = |g: [f64; 2]| (cx + g[0] * r, cy - g[1] * r);
    let mut readings: Vec<(usize, Reading)> = Vec::new();
    for (k, c) in curves.iter().enumerate() {
        for &f in markers {
            if let Some(g) = at(c, f) {
                readings.push((k, read(g, f, z0)));
            }
        }
    }
    let table_h = 22.0 * (readings.len() + curves.len()) as f64 + 20.0;
    let h = cy + r + 40.0 + table_h;
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{size}' height='{h:.0}' \
         viewBox='0 0 {size} {h:.0}' font-family='sans-serif' font-size='11'>\n\
         <rect width='100%' height='100%' fill='white'/>\n\
         <text x='{cx}' y='24' font-size='15' text-anchor='middle'>{}</text>\n\
         <text x='{cx}' y='40' fill='#666' text-anchor='middle'>Impedance Smith chart, Z0 = {z0} Ω</text>\n",
        esc(title)
    );
    s += &format!(
        "<circle cx='{cx}' cy='{cy}' r='{r}' fill='#fafafa' stroke='#444' stroke-width='1.2'/>\n"
    );
    for (v, line) in grid() {
        let d: Vec<String> = line
            .iter()
            .map(|&g| {
                let (x, y) = px(g);
                format!("{x:.1},{y:.1}")
            })
            .collect();
        let strong = (v.abs() - 1.0).abs() < 1e-9;
        s += &format!(
            "<polyline points='{}' fill='none' stroke='{}' stroke-width='{}'/>\n",
            d.join(" "),
            if strong { "#999" } else { "#ccc" },
            if strong { 1.0 } else { 0.7 }
        );
    }
    for (g, t) in labels() {
        let (x, y) = px(g);
        s += &format!(
            "<text x='{:.1}' y='{:.1}' fill='#888' font-size='9' text-anchor='middle'>{t}</text>\n",
            x,
            y - 3.0
        );
    }
    for (k, c) in curves.iter().enumerate() {
        let col = COLOURS[k % COLOURS.len()];
        let d: Vec<String> = c
            .points
            .iter()
            .map(|p| {
                let (x, y) = px([p.1.re, p.1.im]);
                format!("{x:.1},{y:.1}")
            })
            .collect();
        s += &format!(
            "<polyline points='{}' fill='none' stroke='{col}' stroke-width='2'/>\n",
            d.join(" ")
        );
        if let (Some(a), Some(b)) = (c.points.first(), c.points.last()) {
            for (p, lab) in [(a, "start"), (b, "end")] {
                let (x, y) = px([p.1.re, p.1.im]);
                s += &format!(
                    "<circle cx='{x:.1}' cy='{y:.1}' r='2.5' fill='{col}'/>\
                     <text x='{:.1}' y='{:.1}' fill='{col}' font-size='9'>{lab} {:.2} GHz</text>\n",
                    x + 4.0,
                    y + 10.0,
                    p.0
                );
            }
        }
    }
    for (n, (k, rd)) in readings.iter().enumerate() {
        let col = COLOURS[k % COLOURS.len()];
        let (x, y) = px([rd.gamma.re, rd.gamma.im]);
        s += &format!(
            "<path d='M{:.1},{:.1} l5,-9 l-10,0 z' fill='{col}' stroke='white' stroke-width='0.6'/>\
             <text x='{:.1}' y='{:.1}' font-size='10' font-weight='bold' fill='{col}'>m{}</text>\n",
            x,
            y,
            x + 5.0,
            y - 10.0,
            n + 1
        );
    }
    let mut ty = cy + r + 34.0;
    for (k, c) in curves.iter().enumerate() {
        let col = COLOURS[k % COLOURS.len()];
        s += &format!(
            "<rect x='20' y='{:.1}' width='14' height='4' fill='{col}'/>\
             <text x='40' y='{:.1}'>{}</text>\n",
            ty - 5.0,
            ty,
            esc(&c.name)
        );
        ty += 22.0;
    }
    for (n, (k, rd)) in readings.iter().enumerate() {
        let col = COLOURS[k % COLOURS.len()];
        s += &format!(
            "<text x='20' y='{ty:.1}' fill='{col}'>m{} {}</text>\n",
            n + 1,
            esc(&rd.text())
        );
        ty += 22.0;
    }
    s += "</svg>\n";
    s
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matched_load_sits_in_the_centre() {
        let rd = read(Cx::new(0.0, 0.0), 1.0, 50.0);
        assert!((rd.z.re - 50.0).abs() < 1e-9 && rd.z.im.abs() < 1e-9);
        assert!((rd.vswr - 1.0).abs() < 1e-9);
        // 100 Ω on 50: Γ = 1/3, VSWR 2.
        let g = crate::touchstone::z_to_gamma(Cx::new(100.0, 0.0), 50.0);
        let rd = read(g, 1.0, 50.0);
        assert!((rd.vswr - 2.0).abs() < 1e-9);
        assert!((rd.rl_db - 9.542).abs() < 1e-3);
    }

    #[test]
    fn grid_stays_on_the_chart() {
        for (_, line) in grid() {
            for p in line {
                assert!(p[0].hypot(p[1]) <= 1.0 + 1e-6);
            }
        }
        assert_eq!(parse_param("S21"), Some((2, 1)));
        assert_eq!(parse_param("s11"), Some((1, 1)));
    }
}
