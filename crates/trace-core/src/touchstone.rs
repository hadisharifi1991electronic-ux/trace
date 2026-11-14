//! Touchstone (.s1p, .s2p) files: reading them, and cascading two-ports
//! (vias, lines) in front of a load, such as a chip's port model.

use crate::fdtd::Cx;

/// An N-port network against frequency.
#[derive(Clone, Debug)]
pub struct Network {
    /// GHz.
    pub f: Vec<f64>,
    pub ports: usize,
    /// Reference impedance, Ω.
    pub z_ref: f64,
    /// Per frequency, the S matrix row by row (S11, S12, ..., S21, ...).
    pub s: Vec<Vec<Cx>>,
    /// The file's comment lines.
    pub comments: Vec<String>,
}

/// Reads a Touchstone 1.x file of `ports` ports (from the file name's
/// .sNp, or 1 or 2 when that is missing).
pub fn parse(text: &str, ports: usize) -> Result<Network, String> {
    let mut unit = 1e9;
    let mut fmt = "MA".to_string();
    let mut z_ref = 50.0;
    let mut nums: Vec<f64> = Vec::new();
    let mut comments = Vec::new();
    for line in text.lines() {
        let (data, comment) = line.split_once('!').unwrap_or((line, ""));
        if !comment.trim().is_empty() {
            comments.push(comment.trim().to_string());
        }
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if let Some(opt) = data.strip_prefix('#') {
            let w: Vec<String> = opt.split_whitespace().map(|w| w.to_uppercase()).collect();
            let mut i = 0;
            while i < w.len() {
                match w[i].as_str() {
                    "HZ" => unit = 1.0,
                    "KHZ" => unit = 1e3,
                    "MHZ" => unit = 1e6,
                    "GHZ" => unit = 1e9,
                    "MA" | "DB" | "RI" => fmt = w[i].clone(),
                    "R" => {
                        if let Some(r) = w.get(i + 1).and_then(|r| r.parse().ok()) {
                            z_ref = r;
                            i += 1;
                        }
                    }
                    "S" => {}
                    p => return Err(format!("only S-parameters are read, not {p}")),
                }
                i += 1;
            }
            continue;
        }
        if data.starts_with('[') {
            return Err("Touchstone 2.0 keywords are not read".into());
        }
        for t in data.split_whitespace() {
            nums.push(t.parse().map_err(|_| format!("not a number: {t}"))?);
        }
    }
    let per = 1 + 2 * ports * ports;
    if nums.is_empty() || !nums.len().is_multiple_of(per) {
        return Err(format!(
            "{} numbers do not make whole {ports}-port rows",
            nums.len()
        ));
    }
    let mut f = Vec::new();
    let mut s = Vec::new();
    for row in nums.chunks(per) {
        f.push(row[0] * unit / 1e9);
        let mut m: Vec<Cx> = row[1..]
            .chunks(2)
            .map(|p| match fmt.as_str() {
                "RI" => Cx::new(p[0], p[1]),
                "DB" => Cx::expj(p[1].to_radians()).scale(10f64.powf(p[0] / 20.0)),
                _ => Cx::expj(p[1].to_radians()).scale(p[0]),
            })
            .collect();
        // A 2-port lists S11 S21 S12 S22; keep rows (S11 S12 S21 S22).
        if ports == 2 {
            m.swap(1, 2);
        }
        s.push(m);
    }
    Ok(Network {
        f,
        ports,
        z_ref,
        s,
        comments,
    })
}

/// Reads a file, taking the port count from its .sNp name.
pub fn read(path: &std::path::Path) -> Result<Network, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ports = ext
        .strip_prefix('s')
        .and_then(|e| e.strip_suffix('p'))
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    parse(&text, ports)
}

impl Network {
    /// The S matrix at `f` GHz, linear in real and imaginary parts
    /// between points; None outside the file's band.
    pub fn at(&self, f: f64) -> Option<Vec<Cx>> {
        let n = self.f.len();
        if n == 0 || f < self.f[0] - 1e-9 || f > self.f[n - 1] + 1e-9 {
            return None;
        }
        let k = self.f.partition_point(|&x| x < f).min(n - 1);
        if k == 0 || (self.f[k] - f).abs() < 1e-12 {
            return Some(self.s[k].clone());
        }
        let t = (f - self.f[k - 1]) / (self.f[k] - self.f[k - 1]);
        Some(
            self.s[k - 1]
                .iter()
                .zip(&self.s[k])
                .map(|(a, b)| a.scale(1.0 - t).add(b.scale(t)))
                .collect(),
        )
    }

    /// A one-port's impedance at `f`, Ω.
    pub fn impedance(&self, f: f64) -> Option<Cx> {
        let g = *self.at(f)?.first()?;
        Some(gamma_to_z(g, self.z_ref))
    }
}

pub fn gamma_to_z(g: Cx, z0: f64) -> Cx {
    let one = Cx::new(1.0, 0.0);
    one.add(g).div(one.add(g.scale(-1.0))).scale(z0)
}

pub fn z_to_gamma(z: Cx, z0: f64) -> Cx {
    let r = Cx::new(z0, 0.0);
    z.add(r.scale(-1.0)).div(z.add(r))
}

/// The reflection into port 1 of a two-port (S row by row, reference
/// `z_ref`) ending in a load of impedance `zl`.
pub fn input_impedance(s: &[Cx], z_ref: f64, zl: Cx) -> Cx {
    let gl = z_to_gamma(zl, z_ref);
    let one = Cx::new(1.0, 0.0);
    // Γin = S11 + S12 S21 ΓL / (1 − S22 ΓL)
    let gin = s[0].add(
        s[1].mul(s[2])
            .mul(gl)
            .div(one.add(s[3].mul(gl).scale(-1.0))),
    );
    gamma_to_z(gin, z_ref)
}

/// A uniform line `len` mil long of impedance `zc` and effective
/// permittivity `eeff`, with `db_per_in` of loss at `f` GHz, as an S
/// matrix in `z_ref`.
pub fn line(zc: f64, eeff: f64, len: f64, db_per_in: f64, f: f64, z_ref: f64) -> Vec<Cx> {
    let beta = 2.0 * std::f64::consts::PI * f * 1e9 * eeff.sqrt() / 2.997_924_58e8;
    let l = len * 25.4e-6;
    let alpha = db_per_in / 8.686 / 0.0254;
    // ABCD of the line, then S.
    let gl = Cx::new(alpha * l, beta * l);
    let (e, em) = (cexp(gl), cexp(gl.scale(-1.0)));
    let ch = e.add(em).scale(0.5);
    let sh = e.add(em.scale(-1.0)).scale(0.5);
    let z = Cx::new(zc, 0.0);
    let (a, b, c, d) = (ch, sh.mul(z), sh.div(z), ch);
    let r = Cx::new(z_ref, 0.0);
    let den = a.add(b.div(r)).add(c.mul(r)).add(d);
    let s11 = a
        .add(b.div(r))
        .add(c.mul(r).scale(-1.0))
        .add(d.scale(-1.0))
        .div(den);
    let s21 = Cx::new(2.0, 0.0).div(den);
    let s22 = a
        .scale(-1.0)
        .add(b.div(r))
        .add(c.mul(r).scale(-1.0))
        .add(d)
        .div(den);
    vec![s11, s21, s21, s22]
}

fn cexp(z: Cx) -> Cx {
    Cx::expj(z.im).scale(z.re.exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_s1p_and_an_s2p() {
        let n = parse("! x\n# hz S ma R 50\n0 0.5 180\n1e9 0.5 90\n2e9 0.5 0\n", 1).unwrap();
        assert_eq!(n.f, vec![0.0, 1.0, 2.0]);
        let z = n.impedance(0.0).unwrap();
        assert!((z.re - 50.0 / 3.0).abs() < 1e-6 && z.im.abs() < 1e-6);
        let z = n.impedance(2.0).unwrap();
        assert!((z.re - 150.0).abs() < 1e-6);
        assert!(n.at(3.0).is_none());
        let m = parse(
            "# GHz S DB R 100\n1 -20 0 -1 90 -1 90 -30 0\n2 -20 0 -1 90 -1 90 -30 0\n",
            2,
        )
        .unwrap();
        assert_eq!(m.z_ref, 100.0);
        let s = m.at(1.5).unwrap();
        assert!((s[1].abs() - 10f64.powf(-1.0 / 20.0)).abs() < 1e-9);
        assert!((s[3].db() + 30.0).abs() < 1e-6);
    }

    #[test]
    fn matched_line_passes_the_load_and_a_quarter_wave_inverts_it() {
        // A matched line passes any load through unchanged.
        let zl = Cx::new(30.0, 10.0);
        let s = line(50.0, 4.0, 1000.0, 0.0, 2.0, 50.0);
        let zin = input_impedance(&s, 50.0, zl);
        let g0 = z_to_gamma(zl, 50.0).abs();
        assert!((z_to_gamma(zin, 50.0).abs() - g0).abs() < 1e-9);
        // A quarter-wave 50 Ω line turns 25 Ω into 100 Ω.
        let f = 3.0;
        let quarter = 2.997_924_58e8 / (f * 1e9) / 2.0 / 4.0 / 25.4e-6;
        let s = line(50.0, 4.0, quarter, 0.0, f, 75.0);
        let zin = input_impedance(&s, 75.0, Cx::new(25.0, 0.0));
        assert!(
            (zin.re - 100.0).abs() < 1e-6 && zin.im.abs() < 1e-6,
            "{zin:?}"
        );
    }
}
