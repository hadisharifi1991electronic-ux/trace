//! Track losses for the simulator: conductor loss (DC resistance, skin
//! effect and copper roughness) and dielectric loss (loss tangent), as a
//! causal filter per transmission line.
//!
//! A line's attenuation is kept in three parts that add along a route:
//! `dc` nepers from the DC resistance, `skin` nepers at 1 GHz from smooth
//! copper's skin effect (growing as √f), and `diel` nepers per GHz for a
//! loss tangent of one. The filter is the minimum-phase response of
//! exp(−α(f)), found through the cepstrum, so it is causal and carries the
//! dispersion that goes with the loss; the lossless delay stays in the
//! line's step count.

use crate::impedance::LayerGeometry;
use std::f64::consts::PI;

/// Resistivity of copper, ohm metre.
const RHO: f64 = 1.72e-8;
const MU0: f64 = 4.0e-7 * PI;
const MIL: f64 = 25.4e-6;
/// Share of the impulse response given to the one-pole tail instead of
/// taps (the slow end of the skin effect).
const TAIL: f64 = 1e-2;

/// The board materials the loss comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loss {
    /// Dielectric loss tangent (Df).
    pub tan_d: f64,
    /// Copper surface roughness, RMS, micrometres.
    pub rough_um: f64,
    /// Use each layer's own Df from the stack-up where it gives one
    /// (`tan_d` for the rest); otherwise `tan_d` everywhere.
    pub stackup_df: bool,
    /// The same for roughness.
    pub stackup_rough: bool,
}

impl Default for Loss {
    /// The stack-up's materials; standard FR-4 and ordinary
    /// electro-deposited foil where it gives none.
    fn default() -> Self {
        Loss {
            tan_d: 0.02,
            rough_um: 1.0,
            stackup_df: true,
            stackup_rough: true,
        }
    }
}

/// A line's attenuation, in parts that add in series.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Atten {
    /// Nepers from DC resistance.
    pub dc: f64,
    /// Nepers at 1 GHz from the skin effect, smooth copper.
    pub skin: f64,
    /// The part of `skin` on copper with a roughness of its own, and that
    /// part times the roughness (µm), for the mean.
    pub skin_own: f64,
    pub skin_rough: f64,
    /// Nepers per GHz for a loss tangent of 1.
    pub diel: f64,
    /// The part of `diel` in dielectric with a loss tangent of its own,
    /// and that part times its loss tangent (nepers per GHz).
    pub diel_own: f64,
    pub diel_lost: f64,
}

impl std::ops::Add for Atten {
    type Output = Atten;
    fn add(self, o: Atten) -> Atten {
        Atten {
            dc: self.dc + o.dc,
            skin: self.skin + o.skin,
            skin_own: self.skin_own + o.skin_own,
            skin_rough: self.skin_rough + o.skin_rough,
            diel: self.diel + o.diel,
            diel_own: self.diel_own + o.diel_own,
            diel_lost: self.diel_lost + o.diel_lost,
        }
    }
}

/// Hammerstad's roughness factor at frequency `f`.
pub fn rough(f: f64, rough_um: f64) -> f64 {
    if rough_um <= 0.0 || f <= 0.0 {
        return 1.0;
    }
    let delta = (RHO / (PI * f * MU0)).sqrt();
    let r = rough_um * 1e-6 / delta;
    1.0 + 2.0 / PI * (1.4 * r * r).atan()
}

impl Atten {
    /// Attenuation at `f` hertz, nepers.
    pub fn at(&self, f: f64, loss: &Loss) -> f64 {
        let k = rough(f, loss.rough_um);
        let skin = if loss.stackup_rough && self.skin_own > 0.0 {
            let own = rough(f, self.skin_rough / self.skin_own);
            self.skin_own * own + (self.skin - self.skin_own) * k
        } else {
            self.skin * k
        };
        let diel = if loss.stackup_df {
            self.diel_lost + (self.diel - self.diel_own) * loss.tan_d
        } else {
            self.diel * loss.tan_d
        };
        let ac = skin * (f.max(0.0) / 1e9).sqrt();
        (self.dc * self.dc + ac * ac).sqrt() + diel * f / 1e9
    }

    pub fn is_zero(&self) -> bool {
        self.dc == 0.0 && self.skin == 0.0 && self.diel == 0.0
    }
}

/// The attenuation of a track `w` mil wide and `len` mil long on layer
/// `g`, at impedance `z` with delay `tau` seconds.
pub fn track(g: Option<&LayerGeometry>, w: f64, len: f64, z: f64, tau: f64) -> Atten {
    let t = g.map_or(1.4, |g| g.t).max(0.1) * MIL;
    let wm = w.max(0.5) * MIL;
    let l = len * MIL;
    let z = z.max(1.0);
    // Skin resistance per square at 1 GHz.
    let rs = (PI * 1e9 * MU0 * RHO).sqrt();
    // Current on the faces towards the planes, and spread in each plane
    // over about the width plus 2πh.
    let (geom, q) = match g.map(|g| (g.above, g.below, g.er)) {
        Some((Some(h1), Some(h2), _)) => {
            let h = (h1 + h2) / 2.0 * MIL;
            (1.0 / (2.0 * wm) + 1.0 / (2.0 * (wm + 2.0 * PI * h)), 1.0)
        }
        Some((None, Some(h), er)) | Some((Some(h), None, er)) => {
            let hm = h * MIL;
            let ee = crate::impedance::microstrip_eeff(w, h, er);
            // Share of the field in the dielectric.
            let q = if er > 1.0 {
                er * (ee - 1.0) / (ee * (er - 1.0))
            } else {
                1.0
            };
            (1.0 / wm + 1.0 / (wm + 2.0 * PI * hm), q.clamp(0.0, 1.0))
        }
        _ => (1.0 / wm, 1.0),
    };
    let skin = rs * geom * l / (2.0 * z);
    let rough = g.and_then(|g| g.rough_um).filter(|r| *r >= 0.0);
    let diel = PI * 1e9 * tau * q;
    let df = g.and_then(|g| g.tan_d).filter(|d| *d >= 0.0);
    Atten {
        dc: RHO / (wm * t) * l / (2.0 * z),
        skin,
        skin_own: rough.map_or(0.0, |_| skin),
        skin_rough: rough.map_or(0.0, |r| skin * r),
        diel,
        diel_own: df.map_or(0.0, |_| diel),
        diel_lost: df.map_or(0.0, |d| diel * d),
    }
}

/// The attenuation of a track `len` mil long solved by the field solver
/// (`line`), at impedance `z` with delay `tau` seconds, its copper
/// `rough_um` rough where known.
pub fn track_field(
    line: &crate::field::Line,
    len: f64,
    z: f64,
    tau: f64,
    rough_um: Option<f64>,
    df_given: bool,
) -> Atten {
    let l = len * MIL;
    let z = z.max(1.0);
    let skin = line.r_ghz * l / (2.0 * z);
    let rough = rough_um.filter(|r| *r >= 0.0);
    let diel = PI * 1e9 * tau * line.fill;
    Atten {
        dc: line.r_dc * l / (2.0 * z),
        skin,
        skin_own: rough.map_or(0.0, |_| skin),
        skin_rough: rough.map_or(0.0, |r| skin * r),
        diel,
        diel_own: if df_given {
            PI * 1e9 * tau * line.fill_given
        } else {
            0.0
        },
        diel_lost: if df_given {
            PI * 1e9 * tau * line.tan_eff
        } else {
            0.0
        },
    }
}

/// Dielectric loss only (a via barrel), delay `tau`, in a dielectric of
/// loss tangent `df` where known.
pub fn dielectric(tau: f64, df: Option<f64>) -> Atten {
    let diel = PI * 1e9 * tau;
    Atten {
        diel,
        diel_own: df.map_or(0.0, |_| diel),
        diel_lost: df.map_or(0.0, |d| diel * d),
        ..Default::default()
    }
}

/// In-place radix-2 FFT of (re, im); `inverse` divides by n.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
        }
        len <<= 1;
    }
    if inverse {
        let k = 1.0 / n as f64;
        re.iter_mut().for_each(|x| *x *= k);
        im.iter_mut().for_each(|x| *x *= k);
    }
}

/// A line's loss as the solver applies it: taps one step apart, then a
/// tail that decays by `tail.0` each step from `tail.1` (the slow end of
/// the skin effect, as one pole).
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub taps: Vec<f64>,
    pub tail: (f64, f64),
}

impl Filter {
    pub fn lossless() -> Filter {
        Filter {
            taps: vec![1.0],
            tail: (0.0, 0.0),
        }
    }

    /// The DC gain.
    pub fn gain(&self) -> f64 {
        let (a, b) = self.tail;
        self.taps.iter().sum::<f64>() + if b != 0.0 { b / (1.0 - a) } else { 0.0 }
    }
}

/// The causal impulse response, one step of `dt` apart, of a line with
/// attenuation `a` (its lossless delay left out).
pub fn filter(a: &Atten, loss: &Loss, dt: f64) -> Filter {
    if a.is_zero() {
        return Filter::lossless();
    }
    // A window long enough for the response to die away.
    let mut n = 1024;
    while (n as f64) * dt < 40e-9 && n < 65536 {
        n *= 2;
    }
    let mut re: Vec<f64> = (0..n)
        .map(|k| {
            let f = k.min(n - k) as f64 / (n as f64 * dt);
            (-a.at(f, loss)).max(-40.0)
        })
        .collect();
    let mut im = vec![0.0; n];
    // Cepstrum of the log magnitude, folded to make it minimum phase.
    fft(&mut re, &mut im, true);
    for k in 1..n / 2 {
        re[k] *= 2.0;
        im[k] *= 2.0;
    }
    for k in n / 2 + 1..n {
        re[k] = 0.0;
        im[k] = 0.0;
    }
    fft(&mut re, &mut im, false);
    for k in 0..n {
        let m = re[k].exp();
        let (s, c) = im[k].sin_cos();
        re[k] = m * c;
        im[k] = m * s;
    }
    fft(&mut re, &mut im, true);
    // The DC gain, exactly.
    let gain = (-a.dc).exp();
    let h = &re[..n / 2];
    // Taps for all but the last percent of the response (at most 512),
    // the rest as one decaying pole with the same sum and mean delay.
    let total: f64 = h.iter().map(|x| x.abs()).sum();
    let mut acc = 0.0;
    let mut keep = h.len();
    for (k, x) in h.iter().enumerate() {
        acc += x.abs();
        if acc >= total * (1.0 - TAIL) {
            keep = k + 1;
            break;
        }
    }
    let keep = keep.min(512);
    let taps: Vec<f64> = h[..keep].to_vec();
    let rest = gain - taps.iter().sum::<f64>();
    let rest_h = &h[keep..];
    let mass: f64 = rest_h.iter().sum();
    let tail = if rest.abs() < 1e-9 || mass.abs() < 1e-12 {
        (0.0, 0.0)
    } else {
        let mean = (rest_h
            .iter()
            .enumerate()
            .map(|(k, x)| k as f64 * x)
            .sum::<f64>()
            / mass)
            .max(0.0);
        let a = mean / (1.0 + mean);
        (a, rest * (1.0 - a))
    };
    let mut f = Filter { taps, tail };
    if f.tail.1 == 0.0 {
        // No tail: scale the taps to the DC gain.
        let kept: f64 = f.taps.iter().sum();
        if kept.abs() > 1e-12 {
            let k = gain / kept;
            f.taps.iter_mut().for_each(|x| *x *= k);
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_dc_gain_and_spreads_with_loss() {
        let loss = Loss::default();
        let a = Atten {
            dc: 0.01,
            skin: 0.2,
            diel: 0.3,
            ..Default::default()
        };
        let f = filter(&a, &loss, 2e-12);
        let sum = f.gain();
        // The taps and the tail, as one response.
        let mut h = f.taps.clone();
        let mut y = f.tail.1;
        for _ in 0..20000 {
            h.push(y);
            y *= f.tail.0;
        }
        assert!((sum - (-0.01f64).exp()).abs() < 1e-3, "{sum}");
        // Causal and smeared: no single tap holds it all.
        assert!(h.iter().all(|x| x.is_finite()));
        assert!(h[0] < 0.5 * sum, "{}", h[0]);
        // Its magnitude at 1 GHz matches exp(-a).
        let (mut re, mut im) = (0.0, 0.0);
        for (k, x) in h.iter().enumerate() {
            let w = -2.0 * PI * 1e9 * k as f64 * 2e-12;
            re += x * w.cos();
            im += x * w.sin();
        }
        let want = (-a.at(1e9, &loss)).exp();
        assert!(((re * re + im * im).sqrt() - want).abs() < 0.01 * want + 1e-3);
    }

    #[test]
    fn the_stackup_df_counts_where_it_is_given() {
        let mut g = LayerGeometry {
            copper: 1,
            name: "In1".into(),
            t: 1.4,
            above: Some(5.0),
            below: Some(5.0),
            er: 3.3,
            tan_d: Some(0.003),
            rough_um: Some(0.4),
            slabs_above: Vec::new(),
            slabs_below: Vec::new(),
            mask: None,
        };
        let a = track(Some(&g), 5.0, 1000.0, 50.0, 150e-12);
        let fr4 = Loss {
            stackup_df: false,
            stackup_rough: false,
            ..Loss::default()
        };
        let own = Loss::default();
        // Low-loss laminate and smooth copper lose less than FR-4 values.
        assert!(a.at(5e9, &own) < 0.5 * a.at(5e9, &fr4));
        // A layer without its own values takes the defaults either way.
        g.tan_d = None;
        g.rough_um = None;
        let b = track(Some(&g), 5.0, 1000.0, 50.0, 150e-12);
        assert!((b.at(5e9, &own) - b.at(5e9, &fr4)).abs() < 1e-12);
        // Losses still add in series.
        let s = a + b;
        assert!(
            (s.at(5e9, &own) - a.at(5e9, &own) - b.at(5e9, &own)).abs() < 0.05 * s.at(5e9, &own)
        );
    }

    #[test]
    fn a_five_mil_stripline_loses_what_tables_say() {
        // 5 mil, 1 oz, 50 ohm stripline in FR-4 (Df 0.02): about
        // 0.1 dB/inch at 1 GHz from copper and 0.1 from the dielectric.
        let g = LayerGeometry {
            copper: 2,
            name: "In1".into(),
            t: 1.4,
            above: Some(5.0),
            below: Some(5.0),
            er: 4.2,
            tan_d: None,
            rough_um: None,
            slabs_above: Vec::new(),
            slabs_below: Vec::new(),
            mask: None,
        };
        let tau = 4.2f64.sqrt() * 0.0254 / 3e8;
        let a = track(Some(&g), 5.0, 1000.0, 50.0, tau);
        let db = a.at(1e9, &Loss::default()) * 8.686;
        assert!(db > 0.12 && db < 0.35, "{db}");
    }
}
