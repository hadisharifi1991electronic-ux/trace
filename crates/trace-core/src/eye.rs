//! Eye diagram of a path from its S-parameters.
//!
//! The path's two-port (from [`crate::pathloss`]) is driven by a matched
//! source and ends in its load (the chip model when one was given, else
//! the port's reference impedance). Its voltage transfer, shaped by a
//! Gaussian edge, gives the response to one bit (the pulse response). A
//! pseudo-random pattern is then built from the pulse response by
//! superposition and folded into an eye two unit intervals wide. The
//! worst case eye (peak distortion) comes straight from the pulse's
//! cursors.

use crate::fdtd::Cx;
use crate::pathloss::{abcd_from_s, Report};
use std::f64::consts::PI;
use std::fmt::Write;

/// Samples per unit interval.
const PER_UI: usize = 32;
/// Samples over an eye trace (2 UI, both ends).
const SAMPLES: usize = 2 * PER_UI + 1;
/// Bits kept for the plot.
const MAX_TRACES: usize = 400;

/// The signal driven into the path.
#[derive(Clone, Debug)]
pub struct EyeSetup {
    /// Bit rate, Gbit/s.
    pub gbps: f64,
    /// Swing the driver puts into a matched load, volts peak to peak
    /// (differential for a pair).
    pub amplitude: f64,
    /// Edge, 20 to 80 %, ps (0: 30 % of a bit).
    pub rise_ps: f64,
    /// PRBS order: 7, 9, 11, 15, 23 or 31.
    pub prbs: u32,
    /// Most bits to fold (a long pattern is cut).
    pub bits: usize,
    /// Transmitter taps and receiver CTLE.
    pub eq: Equaliser,
    /// Nets coupling into the path (from [`aggressors`]); each switches
    /// at the same rate with its own pattern.
    pub aggressors: Vec<Aggressor>,
    /// Send a clock (1010...) instead of the PRBS: a strobe or clock
    /// line. The aggressors still send their PRBS.
    pub clock: bool,
}

/// A net (or pair) running beside the path, and how it couples in.
#[derive(Clone, Debug)]
pub struct Aggressor {
    /// Net name, or "P / N" for a pair.
    pub name: String,
    /// Length run beside the path, mil.
    pub length: f64,
    /// Strongest backward and forward coupling along it.
    pub kb: f64,
    pub kf: f64,
    /// Near-end (driven from the receiver's end) and far-end (driven
    /// from the source's end) coupling per frequency of the report,
    /// relative to the aggressor line's voltage.
    pub next: Vec<Cx>,
    pub fext: Vec<Cx>,
    /// Swing of each aggressor line, volts peak to peak.
    pub swing: f64,
    /// A spot where it couples, mil.
    pub at: [f64; 2],
}

/// What one aggressor does to the eye.
#[derive(Clone, Debug, Default)]
pub struct XtalkHit {
    pub name: String,
    /// "NEXT" (driven from the receiver's end) or "FEXT" (driven
    /// alongside), whichever is worse.
    pub kind: &'static str,
    pub length: f64,
    /// Worst case closure of the eye height, volts.
    pub closure: f64,
    /// Its noise's peak at the receiver, volts.
    pub peak: f64,
    pub at: [f64; 2],
}

/// Equalisation: a 3-tap transmit FFE and a receive CTLE.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Equaliser {
    /// Pre-cursor tap (0 to about -0.25).
    pub pre: f64,
    /// Post-cursor tap, de-emphasis (0 to about -0.4).
    pub post: f64,
    /// CTLE: peaking, dB (DC this far below the peak; 0: none).
    pub ctle_db: f64,
    /// CTLE peak (its first pole), GHz (0: half the bit rate).
    pub ctle_ghz: f64,
}

impl Equaliser {
    /// The main tap: the driver's peak swing is shared by the taps.
    pub fn main(&self) -> f64 {
        1.0 - self.pre.abs() - self.post.abs()
    }

    pub fn is_none(&self) -> bool {
        self.pre == 0.0 && self.post == 0.0 && self.ctle_db <= 0.0
    }

    /// Transmit FFE at `f` Hz for a bit `ui` s long.
    fn ffe(&self, f: f64, ui: f64) -> Cx {
        let w = 2.0 * PI * f * ui;
        Cx::new(self.main(), 0.0)
            .add(Cx::expj(w).scale(self.pre))
            .add(Cx::expj(-w).scale(self.post))
    }

    /// CTLE at `f` Hz: a zero below the peak and two poles; DC is
    /// `ctle_db` down and the peak near 0 dB, as a receiver's CTLE (it
    /// takes away low frequencies rather than adding gain).
    fn ctle(&self, f: f64, gbps: f64) -> Cx {
        if self.ctle_db <= 0.0 {
            return Cx::new(1.0, 0.0);
        }
        let fp = if self.ctle_ghz > 0.0 {
            self.ctle_ghz
        } else {
            gbps / 2.0
        } * 1e9;
        let fz = fp / 10f64.powf(self.ctle_db / 20.0);
        let fp2 = 4.0 * fp;
        Cx::new(1.0, f / fz)
            .div(Cx::new(1.0, f / fp))
            .div(Cx::new(1.0, f / fp2))
            .scale(fz / fp)
    }

    /// One line: "FFE pre -0.05, main 0.80, post -0.15; CTLE 6 dB at 5 GHz".
    pub fn describe(&self, gbps: f64) -> String {
        let mut parts = Vec::new();
        if self.pre != 0.0 || self.post != 0.0 {
            // De-emphasis: the steady level against the transition's.
            let de = if self.main() > 0.0 {
                20.0 * ((self.main() - self.post.abs() - self.pre.abs())
                    / (self.main() + self.post.abs() + self.pre.abs()))
                .abs()
                .max(1e-6)
                .log10()
            } else {
                0.0
            };
            parts.push(format!(
                "FFE pre {:.3}, main {:.3}, post {:.3} ({:.1} dB de-emphasis)",
                self.pre,
                self.main(),
                self.post,
                de
            ));
        }
        if self.ctle_db > 0.0 {
            let fp = if self.ctle_ghz > 0.0 {
                self.ctle_ghz
            } else {
                gbps / 2.0
            };
            parts.push(format!(
                "CTLE {} dB peaking at {} GHz",
                trim(self.ctle_db),
                trim(fp)
            ));
        }
        if parts.is_empty() {
            "no equalisation".into()
        } else {
            parts.join("; ")
        }
    }
}

impl Default for EyeSetup {
    fn default() -> Self {
        EyeSetup {
            gbps: 2.5,
            amplitude: 0.8,
            rise_ps: 0.0,
            prbs: 7,
            bits: 2047,
            eq: Equaliser::default(),
            aggressors: Vec::new(),
            clock: false,
        }
    }
}

/// A finished eye.
#[derive(Clone, Debug, Default)]
pub struct EyeResult {
    pub gbps: f64,
    /// Bit, ps.
    pub ui_ps: f64,
    /// Edge used, 20 to 80 %, ps.
    pub rise_ps: f64,
    pub amplitude: f64,
    pub prbs: u32,
    pub bits: usize,
    /// Traces over 2 UI ([`SAMPLES`] points each), volts.
    pub traces: Vec<Vec<f32>>,
    /// Decision level, volts.
    pub threshold: f64,
    /// Opening at the centre: lowest one minus highest zero, volts.
    pub height: f64,
    /// Lowest one and highest zero at the centre.
    pub high: f64,
    pub low: f64,
    /// Open time across the threshold, ps (one bit less the jitter).
    pub width_ps: f64,
    /// Spread of the threshold crossings, ps peak to peak.
    pub jitter_ps: f64,
    /// Worst case opening from the cursors (peak distortion), volts.
    pub worst_height: f64,
    /// Pulse response at the main cursor's phase, one per bit, volts for
    /// a one against a zero; `main` is the main cursor.
    pub cursors: Vec<f64>,
    pub main: usize,
    /// Pulse response over a window around its peak: (ns, volts).
    pub pulse: Vec<(f64, f64)>,
    /// Time of the pulse peak after the bit starts, ns.
    pub peak_ns: f64,
    pub notes: Vec<String>,
    /// The equalisation used.
    pub eq: Equaliser,
    /// Crosstalk, worst first; the traces and the measured height, width
    /// and jitter include it.
    pub xtalk: Vec<XtalkHit>,
    /// Worst case height with every aggressor at its worst, volts.
    pub worst_with_xtalk: f64,
    /// The sampling point (the middle of the eye), ns after the bit
    /// starts at the driver: the flight time plus half a bit.
    pub sample_ns: f64,
    /// Threshold crossings into the next bit, ps after the sampling
    /// point: earliest, mean and latest (the open window runs from
    /// the latest less a bit to the earliest).
    pub cross_ps: [f64; 3],
    /// The inner edge of the eye over the 2 UI window: lowest one and
    /// highest zero at each sample, volts.
    pub inner: Vec<(f32, f32)>,
}

impl EyeResult {
    /// Opening as a share of the swing.
    pub fn height_share(&self) -> f64 {
        if self.amplitude > 0.0 {
            self.height / self.amplitude
        } else {
            0.0
        }
    }

    /// Width as a share of a bit.
    pub fn width_share(&self) -> f64 {
        if self.ui_ps > 0.0 {
            self.width_ps / self.ui_ps
        } else {
            0.0
        }
    }

    /// Lines for a report.
    pub fn summary(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "Eye at {} Gbit/s (bit {:.1} ps), PRBS{} over {} bits, swing {:.3} V, edge {:.1} ps (20-80 %)",
            trim(self.gbps),
            self.ui_ps,
            self.prbs,
            self.bits,
            self.amplitude,
            self.rise_ps
        );
        let _ = writeln!(
            o,
            "  height {:.1} mV ({:.0} % of the swing), width {:.1} ps ({:.0} % of a bit), jitter {:.1} ps p-p",
            self.height * 1e3,
            100.0 * self.height_share(),
            self.width_ps,
            100.0 * self.width_share(),
            self.jitter_ps
        );
        let _ = writeln!(
            o,
            "  worst case height (peak distortion) {:.1} mV; sampled {:.3} ns after the bit starts",
            self.worst_height * 1e3,
            self.peak_ns
        );
        let main = self.cursors.get(self.main).copied().unwrap_or(0.0);
        if main.abs() > 0.0 {
            let pick = |k: isize| {
                let i = self.main as isize + k;
                if i >= 0 {
                    self.cursors.get(i as usize).map(|v| v / main)
                } else {
                    None
                }
            };
            let mut line = format!("  cursors (of the main {:.1} mV):", main * 1e3);
            for k in [-2isize, -1, 1, 2, 3] {
                if let Some(v) = pick(k) {
                    let _ = write!(line, " {k:+}: {:+.3}", v);
                }
            }
            let _ = writeln!(o, "{line}");
        }
        if !self.eq.is_none() {
            let _ = writeln!(o, "  with {}", self.eq.describe(self.gbps));
        }
        if !self.xtalk.is_empty() {
            let total: f64 = self.xtalk.iter().map(|x| x.closure).sum();
            let _ = writeln!(
                o,
                "  crosstalk from {} net(s): closes the worst case by {:.1} mV to {:.1} mV",
                self.xtalk.len(),
                total * 1e3,
                self.worst_with_xtalk * 1e3
            );
            for x in self.xtalk.iter().take(8) {
                let _ = writeln!(
                    o,
                    "    {:<28} {} {:>6.0} mil beside, closure {:>6.1} mV, peak {:>6.1} mV",
                    x.name,
                    x.kind,
                    x.length,
                    x.closure * 1e3,
                    x.peak * 1e3
                );
            }
        }
        for n in &self.notes {
            let _ = writeln!(o, "  note: {n}");
        }
        o
    }

    /// The eye and the pulse response as an SVG picture.
    pub fn svg(&self, title: &str) -> String {
        let (w, h) = (900.0, 640.0);
        let (l, r) = (70.0, 24.0);
        let mut o = String::new();
        let _ = writeln!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="Helvetica, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#ffffff"/>"##
        );
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="24" font-size="14" fill="#111">{}</text>"##,
            esc(title)
        );
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="42" fill="#444">height {:.1} mV · width {:.1} ps · jitter {:.1} ps · worst case {:.1} mV</text>"##,
            self.height * 1e3,
            self.width_ps,
            self.jitter_ps,
            self.worst_height * 1e3
        );
        // Eye panel.
        let (t, b) = (60.0, 400.0);
        let vmax = self
            .traces
            .iter()
            .flatten()
            .map(|v| (*v as f64).abs())
            .fold(self.amplitude / 2.0, f64::max)
            * 1.08;
        let x = |i: f64| l + (w - l - r) * i / (SAMPLES - 1) as f64;
        let y = |v: f64| t + (b - t) * (0.5 - v / (2.0 * vmax));
        grid_v(&mut o, l, w - r, vmax, &y);
        for k in 0..=4 {
            let xi = x(k as f64 * PER_UI as f64 / 2.0);
            let ps = (k as f64 / 2.0 - 1.0) * self.ui_ps;
            let _ = writeln!(
                o,
                r##"<line x1="{xi:.1}" x2="{xi:.1}" y1="{t}" y2="{b}" stroke="#e3e6ec"/><text x="{xi:.1}" y="{}" text-anchor="middle" fill="#555">{}</text>"##,
                b + 16.0,
                trim((ps * 10.0).round() / 10.0)
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="{}" y="{}" text-anchor="middle" fill="#555">ps from the centre of the bit</text>"##,
            (l + w - r) / 2.0,
            b + 32.0
        );
        for tr in &self.traces {
            let mut d = String::new();
            for (i, v) in tr.iter().enumerate() {
                let _ = write!(
                    d,
                    "{}{:.1},{:.1}",
                    if i == 0 { "M" } else { "L" },
                    x(i as f64),
                    y(*v as f64)
                );
            }
            let _ = writeln!(
                o,
                r##"<path d="{d}" fill="none" stroke="#1f5fbf" stroke-opacity="0.18" stroke-width="1"/>"##
            );
        }
        // The opening at the centre.
        if self.height > 0.0 {
            let xc = x(PER_UI as f64);
            let _ = writeln!(
                o,
                r##"<line x1="{xc:.1}" x2="{xc:.1}" y1="{:.1}" y2="{:.1}" stroke="#d4380d" stroke-width="2"/>"##,
                y(self.high),
                y(self.low)
            );
        }
        // Pulse panel.
        let (t2, b2) = (460.0, 600.0);
        if let (Some(first), Some(last)) = (self.pulse.first(), self.pulse.last()) {
            let (t0, t1) = (first.0, last.0.max(first.0 + 1e-6));
            let pmax = self.pulse.iter().map(|p| p.1.abs()).fold(1e-6, f64::max) * 1.1;
            let xp = |tt: f64| l + (w - l - r) * (tt - t0) / (t1 - t0);
            let yp = |v: f64| t2 + (b2 - t2) * (0.5 - v / (2.0 * pmax));
            grid_v(&mut o, l, w - r, pmax, &yp);
            let mut d = String::new();
            for (i, (tt, v)) in self.pulse.iter().enumerate() {
                let _ = write!(
                    d,
                    "{}{:.1},{:.1}",
                    if i == 0 { "M" } else { "L" },
                    xp(*tt),
                    yp(*v)
                );
            }
            let _ = writeln!(
                o,
                r##"<path d="{d}" fill="none" stroke="#111" stroke-width="1.5"/>"##
            );
            // Cursor marks.
            let ui = self.ui_ps * 1e-3;
            for (k, v) in self.cursors.iter().enumerate() {
                let tt = self.peak_ns + ui / 2.0 + (k as f64 - self.main as f64) * ui;
                if tt < t0 || tt > t1 {
                    continue;
                }
                let _ = writeln!(
                    o,
                    r##"<circle cx="{:.1}" cy="{:.1}" r="2.5" fill="{}"/>"##,
                    xp(tt),
                    yp(*v),
                    if k == self.main { "#d4380d" } else { "#1f5fbf" }
                );
            }
            let _ = writeln!(
                o,
                r##"<text x="{l}" y="{}" fill="#555">pulse response, {} to {} ns (dots: the cursors)</text>"##,
                b2 + 18.0,
                trim((t0 * 1e3).round() / 1e3),
                trim((t1 * 1e3).round() / 1e3)
            );
        }
        o.push_str("</svg>\n");
        o
    }
}

fn grid_v(o: &mut String, x0: f64, x1: f64, vmax: f64, y: &dyn Fn(f64) -> f64) {
    let steps = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0,
    ];
    let step = steps.into_iter().find(|s| vmax / s <= 4.0).unwrap_or(10.0);
    let mut v = -(vmax / step).floor() * step;
    while v <= vmax + 1e-12 {
        let _ = writeln!(
            o,
            r##"<line x1="{x0}" x2="{x1}" y1="{:.1}" y2="{:.1}" stroke="{}"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{} mV</text>"##,
            y(v),
            y(v),
            if v.abs() < 1e-12 {
                "#b8bfcc"
            } else {
                "#e3e6ec"
            },
            x0 - 6.0,
            y(v) + 4.0,
            trim((v * 1e3).round())
        );
        v += step;
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// Voltage at the load for a matched source, per frequency, relative to
/// what the source puts into a matched load (1 for a perfect path).
pub fn transfer(rep: &Report) -> Vec<Cx> {
    let [z1, z2] = rep.z_ref;
    let zs = Cx::new(z1, 0.0);
    rep.s
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let m = abcd_from_s(s, z1, z2);
            let zl = rep.load_z.get(k).copied().unwrap_or(Cx::new(z2, 0.0));
            // 2 ZL / (A ZL + B + C Zs ZL + D Zs).
            let den = m[0]
                .mul(zl)
                .add(m[1])
                .add(m[2].mul(zs).mul(zl))
                .add(m[3].mul(zs));
            zl.scale(2.0).div(den)
        })
        .collect()
}

/// Bits of a PRBS pattern (one period, at most `max` bits).
pub fn prbs(order: u32, max: usize) -> Vec<bool> {
    let (n, m) = match order {
        9 => (9, 5),
        11 => (11, 9),
        15 => (15, 14),
        23 => (23, 18),
        31 => (31, 28),
        _ => (7, 6),
    };
    let period = (1u64 << n) - 1;
    let count = (period as usize).min(max.max(16));
    let mut state: u32 = (1u32 << n) - 1;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let bit = ((state >> (n - 1)) ^ (state >> (m - 1))) & 1;
        state = ((state << 1) | bit) & ((1u32 << n) - 1);
        out.push(bit == 1);
    }
    out
}

/// One bit's response: `p(t)` for a pulse of height 1 and one bit long
/// starting at 0, from the band-limited spectrum.
struct Pulse {
    /// Frequencies (Hz) with their weighted spectrum; `dc` the zero term.
    f: Vec<f64>,
    spec: Vec<Cx>,
    dc: f64,
    df: f64,
}

impl Pulse {
    fn at(&self, t: f64) -> f64 {
        let mut sum = self.dc;
        for (f, p) in self.f.iter().zip(&self.spec) {
            let e = Cx::expj(2.0 * PI * f * t);
            sum += 2.0 * (p.re * e.re - p.im * e.im);
        }
        sum * self.df
    }

    /// `n` samples from `t0`, `step` apart (phasors turned, no sines in
    /// the loop).
    fn grid(&self, t0: f64, step: f64, n: usize) -> Vec<f64> {
        let mut out = vec![self.dc * self.df; n];
        for (f, p) in self.f.iter().zip(&self.spec) {
            let rot = Cx::expj(2.0 * PI * f * step);
            let mut z = p.mul(Cx::expj(2.0 * PI * f * t0)).scale(2.0 * self.df);
            for o in out.iter_mut() {
                *o += z.re;
                z = z.mul(rot);
            }
        }
        out
    }
}

/// Equalisation chosen for a path.
#[derive(Clone, Debug)]
pub struct EqChoice {
    pub eq: Equaliser,
    /// Worst case eye height (peak distortion), volts, without and with
    /// the equaliser.
    pub before: f64,
    pub after: f64,
    pub tries: usize,
}

/// Worst case height (peak distortion) when sampling a pulse at `c`;
/// `q` holds `per` points a bit over one period.
fn pd_at(q: &[f64], per: usize, c: usize) -> f64 {
    let n = q.len();
    let mut isi = 0.0;
    for k in 1..n / per {
        isi += q[(c + k * per) % n].abs();
    }
    q[c] - isi
}

/// Worst case eye area: the opening summed over the sampling phases of
/// one bit around the pulse's peak (closed eyes compare by their least
/// closed phase, below every open one).
fn pd_area(q: &[f64], per: usize) -> f64 {
    let n = q.len() / per * per;
    let q = &q[..n];
    let Some((imax, _)) = q.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
        return f64::MIN;
    };
    let mut area = 0.0;
    let mut most = f64::MIN;
    for o in -(per as isize)..=(per as isize) {
        let c = (imax as isize + o).rem_euclid(n as isize) as usize;
        let h = pd_at(q, per, c);
        area += h.max(0.0);
        most = most.max(h);
    }
    if area > 0.0 {
        area
    } else {
        most - 1e3
    }
}

/// The sampling point near the pulse's peak whose worst case opening
/// holds best over a quarter bit either side (an eye open in both
/// directions), with that score and the opening at the point itself.
fn best_phase(q: &[f64], per: usize) -> (usize, f64, f64) {
    let n = q.len() / per * per;
    let q = &q[..n];
    let Some((imax, _)) = q.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
        return (0, 0.0, 0.0);
    };
    let at = |c: isize| (c).rem_euclid(n as isize) as usize;
    let side = (per / 4) as isize;
    let mut best = (imax, f64::MIN, 0.0);
    for o in -(per as isize / 2)..=(per as isize / 2) {
        let c = imax as isize + o;
        let centre = pd_at(q, per, at(c));
        let score = centre
            .min(pd_at(q, per, at(c - side)))
            .min(pd_at(q, per, at(c + side)));
        if score > best.1 {
            best = (at(c), score, centre);
        }
    }
    best
}

/// Picks the transmit taps and CTLE that open the worst case eye most.
pub fn optimise(
    rep: &Report,
    setup: &EyeSetup,
    use_ffe: bool,
    use_ctle: bool,
) -> Result<EqChoice, String> {
    const PER: usize = 8;
    let ui = 1e-9 / setup.gbps.max(1e-6);
    let step = ui / PER as f64;
    let mut plain = setup.clone();
    plain.eq = Equaliser::default();
    let mut ctles = vec![(0.0, 0.0)];
    if use_ctle {
        for db in 1..=18 {
            for k in [0.5, 0.75, 1.0] {
                ctles.push((db as f64, k * setup.gbps / 2.0));
            }
        }
    }
    let mut taps = vec![(0.0, 0.0)];
    if use_ffe {
        taps.clear();
        for i in 0..=6 {
            for j in 0..=14 {
                taps.push((0.0 - 0.025 * i as f64, 0.0 - 0.025 * j as f64));
            }
        }
    }
    let mut before = 0.0;
    let mut best = (f64::MIN, Equaliser::default(), 0.0);
    let mut tries = 0;
    for (db, ghz) in ctles {
        crate::cancel::check()?;
        let mut s2 = plain.clone();
        s2.eq = Equaliser {
            ctle_db: db,
            ctle_ghz: ghz,
            ..Default::default()
        };
        let (pulse, ..) = spectrum(rep, &s2)?;
        let n = ((1.0 / pulse.df / step) as usize) / PER * PER;
        if n < 4 * PER {
            return Err(
                "the sweep's step is too coarse for this bit rate: raise the points".into(),
            );
        }
        let p = pulse.grid(0.0, step, n);
        let mut q = vec![0.0; n];
        for &(pre, post) in &taps {
            let main = 1.0 - pre.abs() - post.abs();
            for (i, v) in q.iter_mut().enumerate() {
                *v = main * p[i] + pre * p[(i + PER) % n] + post * p[(i + n - PER) % n];
            }
            let (_, _, centre) = best_phase(&q, PER);
            let score = pd_area(&q, PER);
            tries += 1;
            if db == 0.0 && pre == 0.0 && post == 0.0 {
                before = centre * setup.amplitude;
            }
            if score > best.0 + 1e-9 {
                best = (
                    score,
                    Equaliser {
                        pre,
                        post,
                        ctle_db: db,
                        ctle_ghz: ghz,
                    },
                    centre * setup.amplitude,
                );
            }
        }
    }
    Ok(EqChoice {
        eq: best.1,
        before: before.max(0.0),
        after: best.2.max(0.0),
        tries,
    })
}

/// Builds the eye of a path.
/// The band-limited spectrum of one bit through the path (with the
/// equaliser), its bit time and notes on the sweep.
fn spectrum(rep: &Report, setup: &EyeSetup) -> Result<(Pulse, f64, Vec<String>), String> {
    if rep.f.len() < 4 || setup.gbps <= 0.0 {
        return Err("no sweep or no bit rate".into());
    }
    let ui = 1e-9 / setup.gbps;
    let rise = if setup.rise_ps > 0.0 {
        setup.rise_ps * 1e-12
    } else {
        0.3 * ui
    };
    let sigma = rise / 1.683;
    let h = transfer(rep);
    let df = rep.f[0] * 1e9;
    let fmax = rep.f.last().copied().unwrap_or(0.0) * 1e9;
    let mut notes = Vec::new();
    // Even spacing from zero is what the series needs.
    let even = rep
        .f
        .iter()
        .enumerate()
        .all(|(k, f)| (f * 1e9 - df * (k + 1) as f64).abs() < 1e-3 * df);
    if !even {
        return Err("the sweep is not evenly spaced from zero".into());
    }
    let pulse = shape(setup, df, fmax, &h, true);
    // Content cut by the top of the sweep.
    let g_top = (-2.0 * PI * PI * sigma * sigma * fmax * fmax).exp();
    if g_top > 0.05 {
        let need = ((100f64).ln() / (2.0 * PI * PI * sigma * sigma)).sqrt();
        notes.push(format!(
            "the sweep stops at {} GHz, where the edge still has {:.0} % of its content: sweep to {} GHz or more for this edge",
            trim(fmax * 1e-9),
            100.0 * g_top,
            trim((need * 1e-9 * 2.0).ceil() / 2.0)
        ));
    }
    Ok((pulse, rise, notes))
}

/// One bit with the edge, through `h` (per frequency, `df` apart) and
/// the receiver's CTLE, and the transmit taps when `ffe`.
fn shape(setup: &EyeSetup, df: f64, fmax: f64, h: &[Cx], ffe: bool) -> Pulse {
    let ui = 1e-9 / setup.gbps;
    let rise = if setup.rise_ps > 0.0 {
        setup.rise_ps * 1e-12
    } else {
        0.3 * ui
    };
    let sigma = rise / 1.683;
    let eq = setup.eq;
    let taps = if ffe {
        eq.main() + eq.pre + eq.post
    } else {
        1.0
    };
    // The zero term from the first two points.
    let h0 = (2.0 * h[0].re - h[1].re) * taps * eq.ctle(0.0, setup.gbps).re;
    let taper_from = 0.8 * fmax;
    let mut spec = Vec::with_capacity(h.len());
    for (k, hk) in h.iter().enumerate() {
        let f = df * (k + 1) as f64;
        let g = (-2.0 * PI * PI * sigma * sigma * f * f).exp();
        let x = PI * f * ui;
        let sinc = x.sin() / x;
        let taper = if f > taper_from {
            0.5 * (1.0 + (PI * (f - taper_from) / (fmax - taper_from)).cos())
        } else {
            1.0
        };
        let p = Cx::expj(-PI * f * ui).scale(ui * sinc * g * taper);
        let mut v = hk.mul(p).mul(eq.ctle(f, setup.gbps));
        if ffe {
            v = v.mul(eq.ffe(f, ui));
        }
        spec.push(v);
    }
    Pulse {
        f: (1..=h.len()).map(|k| df * k as f64).collect(),
        spec,
        dc: h0 * ui,
        df,
    }
}

/// Nets running beside the path's traces on the same layer (within four
/// dielectric heights), grouped by net or pair, with their near- and
/// far-end coupling into the path's receiver. The noise is taken where
/// it couples, without the loss on its way to the receiver (a bound).
pub fn aggressors(board: &crate::Board, rep: &Report, amplitude: f64) -> Vec<Aggressor> {
    use std::collections::{HashMap, HashSet};
    const C0: f64 = 2.998e8;
    let mut victims: Vec<u32> = Vec::new();
    let mut sign: HashMap<u32, f64> = HashMap::new();
    let mut diff = false;
    for &(p, n) in &rep.victims {
        victims.push(p);
        sign.insert(p, 1.0);
        if let Some(n) = n {
            victims.push(n);
            sign.insert(n, -1.0);
            diff = true;
        }
    }
    if victims.is_empty() || rep.f.is_empty() {
        return Vec::new();
    }
    // Aggressor pairs on the board (and by name, P/N): their negative
    // lines count opposite.
    let mut pairs = board.diff_pairs.clone();
    pairs.extend(crate::zcheck::name_pairs(board));
    let id = |s: &str| {
        board
            .nets
            .iter()
            .position(|n| n.name == s)
            .map(|i| i as u32)
    };
    let mut partner: HashMap<u32, u32> = HashMap::new();
    for p in &pairs {
        if let (Some(a), Some(b)) = (id(&p.positive), id(&p.negative)) {
            partner.insert(a, b);
            partner.insert(b, a);
        }
    }
    let positive: HashSet<u32> = pairs
        .iter()
        .filter_map(|p| {
            board
                .nets
                .iter()
                .position(|n| n.name == p.positive)
                .map(|i| i as u32)
        })
        .collect();
    let name = |n: u32| {
        board
            .nets
            .get(n as usize)
            .map_or(format!("net {n}"), |x| x.name.clone())
    };
    let geo = board.layer_geometry();
    let raws = crate::sim::xtalk::find(board, &victims, false);
    let f: Vec<f64> = rep.f.iter().map(|g| g * 1e9).collect();
    let mut groups: HashMap<u32, Aggressor> = HashMap::new();
    let mut runs: HashMap<(u32, u32, u32), f64> = HashMap::new();
    for r in &raws {
        let (key, asign) = match partner.get(&r.aggressor) {
            Some(&o) if positive.contains(&o) => (o, -1.0),
            _ => (r.aggressor, 1.0),
        };
        let vs = sign.get(&r.victim).copied().unwrap_or(1.0);
        let t = &board.geometry.tracks[r.vtrack];
        let er = match &t.layer {
            crate::LayerRef::Copper(l) => geo.get(*l as usize).map_or(4.0, |lg| {
                if lg.above.is_some() && lg.below.is_some() {
                    lg.er
                } else {
                    // Microstrip: about halfway between air and board.
                    (lg.er + 1.0) / 2.0 + 0.1 * lg.er
                }
            }),
            _ => 4.0,
        };
        let tau = r.len * 25.4e-6 * er.sqrt() / C0;
        let k = vs * asign;
        let a = groups.entry(key).or_insert_with(|| Aggressor {
            name: match partner.get(&key) {
                Some(&o) if positive.contains(&key) => format!("{} / {}", name(key), name(o)),
                _ => name(key),
            },
            length: 0.0,
            kb: 0.0,
            kf: 0.0,
            next: vec![Cx::default(); f.len()],
            fext: vec![Cx::default(); f.len()],
            swing: if diff { amplitude / 2.0 } else { amplitude },
            at: [
                (t.start.x.mil() + t.end.x.mil()) / 2.0,
                (t.start.y.mil() + t.end.y.mil()) / 2.0,
            ],
        });
        // Length beside the path: the longest of the runs beside each
        // victim line by each aggressor line (a pair's lines run together).
        *runs.entry((key, r.victim, r.aggressor)).or_insert(0.0) += r.len;
        a.length = a.length.max(runs[&(key, r.victim, r.aggressor)]);
        a.kb = a.kb.max(r.kb.abs());
        a.kf = a.kf.max(r.kf.abs());
        for (i, &fq) in f.iter().enumerate() {
            let w = 2.0 * PI * fq * tau;
            // NEXT: Kb (1 - e^(-j2wt)); FEXT: Kf jwt e^(-jwt).
            let next = Cx::new(1.0, 0.0)
                .add(Cx::expj(-2.0 * w).scale(-1.0))
                .scale(r.kb * k);
            let fext = Cx::new(0.0, w).mul(Cx::expj(-w)).scale(r.kf * k);
            a.next[i] = a.next[i].add(next);
            a.fext[i] = a.fext[i].add(fext);
        }
    }
    let mut out: Vec<Aggressor> = groups.into_values().collect();
    out.sort_by(|a, b| (b.kb * b.length).total_cmp(&(a.kb * a.length)));
    out
}

/// An aggressor's noise at the receiver for one of its bits: per cursor,
/// samples over the 2 UI window around the victim's sampling point
/// (lined up at its worst phase), with its worst case closure and peak.
struct Noise {
    rows: Vec<(isize, Vec<f64>)>,
    hit: XtalkHit,
    /// Volts per symbol (half the line's swing).
    half: f64,
}

fn noise(setup: &EyeSetup, df: f64, fmax: f64, a: &Aggressor, t_sample: f64) -> Noise {
    let ui = 1e-9 / setup.gbps;
    let dt = ui / PER_UI as f64;
    let period = 1.0 / df;
    let total = (period / dt) as usize / PER_UI * PER_UI;
    let mut best: Option<(f64, &'static str, Vec<f64>)> = None;
    for (kind, c) in [("NEXT", &a.next), ("FEXT", &a.fext)] {
        let pulse = shape(setup, df, fmax, c, false);
        let x = pulse.grid(0.0, dt, total);
        // Worst phase: the most summed noise over the bits.
        let bits = total / PER_UI;
        let (mut worst, mut phase) = (0.0, 0);
        for ph in 0..PER_UI {
            let sum: f64 = (0..bits).map(|k| x[ph + k * PER_UI].abs()).sum();
            if sum > worst {
                worst = sum;
                phase = ph;
            }
        }
        if best.as_ref().is_none_or(|b| worst > b.0) {
            // Rotate so the worst phase sits at index 0.
            let mut r = x.clone();
            r.rotate_left(phase);
            best = Some((worst, kind, r));
        }
    }
    let half = a.swing / 2.0;
    let Some((sum, kind, x)) = best else {
        return Noise {
            rows: Vec::new(),
            hit: XtalkHit::default(),
            half,
        };
    };
    let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs())) * half;
    // Rows over the window: x(worst + k UI + (s - PER_UI) dt), kept while
    // they matter.
    let n = x.len();
    let bits = n / PER_UI;
    let top = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let mut rows = Vec::new();
    for k in 0..bits {
        let row: Vec<f64> = (0..SAMPLES)
            .map(|s| x[(k * PER_UI + n + s - PER_UI) % n])
            .collect();
        if row.iter().any(|v| v.abs() > 1e-3 * top) {
            rows.push((k as isize, row));
        }
    }
    let _ = t_sample;
    Noise {
        rows,
        hit: XtalkHit {
            name: a.name.clone(),
            kind,
            length: a.length,
            // Both rails move in: 2 × half × sum.
            closure: 2.0 * half * sum,
            peak,
            at: a.at,
        },
        half,
    }
}

/// Builds the eye of a path.
pub fn eye(rep: &Report, setup: &EyeSetup) -> Result<EyeResult, String> {
    let (pulse, rise, mut notes) = spectrum(rep, setup)?;
    let ui = 1e-9 / setup.gbps;
    let df = pulse.df;
    // The pulse over one period of the series, to find its peak.
    let period = 1.0 / df;
    let dt = ui / PER_UI as f64;
    let total = (period / dt).floor() as usize;
    if total < 4 * PER_UI {
        return Err(format!(
            "the sweep's step ({} MHz) is too coarse for {} Gbit/s: raise the points",
            trim(df * 1e-6),
            trim(setup.gbps)
        ));
    }
    let coarse: Vec<f64> = pulse.grid(0.0, dt, total);
    let (ipk, &vpk) = coarse
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap_or((0, &0.0));
    if vpk <= 1e-9 {
        return Err("the path passes no signal at this bit rate".into());
    }
    // Sampling phase: the one near the peak with the widest worst case
    // opening (an equalised pulse overshoots, so its peak is not always
    // the best place).
    let _ = ipk;
    let ipk = best_phase(&coarse, PER_UI).0;
    let t_peak = ipk as f64 * dt;
    // Cursors: one per bit over the period, a few before the main one.
    let n_cur = total / PER_UI;
    let pre = (n_cur / 4).min(8);
    let at = |k: isize, s: usize| -> f64 {
        // Bit k from the main one, sample s of the 2 UI window.
        t_peak + k as f64 * ui + (s as f64 - PER_UI as f64) * dt
    };
    // Each cursor's samples over the 2 UI window, kept while they matter.
    let mut rows: Vec<(isize, Vec<f64>)> = Vec::new();
    for k in -(pre as isize)..(n_cur as isize - pre as isize) {
        let row = pulse.grid(at(k, 0), dt, SAMPLES);
        rows.push((k, row));
    }
    let keep = 1e-4 * vpk;
    rows.retain(|(k, row)| *k == 0 || row.iter().any(|v| v.abs() > keep));
    // The tail at the end of the period (it would wrap into the start).
    // (Leaving out the edge's own spread before t = 0, which wraps to
    // the end of the period.)
    let lead = (3.0 * rise / 1.683 / dt).ceil() as usize;
    let tail = coarse
        .iter()
        .take(total.saturating_sub(lead))
        .skip(total.saturating_sub(PER_UI * 2 + lead))
        .map(|v| v.abs())
        .fold(0.0, f64::max);
    let late = coarse
        .iter()
        .take(ipk.saturating_sub(pre * PER_UI))
        .map(|v| v.abs())
        .fold(0.0, f64::max);
    if tail.max(late) > 0.02 * vpk {
        notes.push(format!(
            "the pulse has not settled within the sweep's period ({} ns): raise the points for a finer step",
            trim((period * 1e9 * 10.0).round() / 10.0)
        ));
    }
    // Pulse response for a one against a zero is twice p (symbols ±1),
    // scaled by half the swing: p × amplitude.
    let amp = setup.amplitude;
    let cursors: Vec<f64> = rows.iter().map(|(_, r)| amp * r[PER_UI]).collect();
    let main = rows.iter().position(|(k, _)| *k == 0).unwrap_or(0);
    let p0 = cursors[main];
    let isi: f64 = cursors
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != main)
        .map(|(_, v)| v.abs())
        .sum();
    let worst_height = (p0 - isi).max(0.0);
    // Crosstalk: each aggressor at its worst phase on the sampling point.
    let fmax = rep.f.last().copied().unwrap_or(0.0) * 1e9;
    let noises: Vec<Noise> = setup
        .aggressors
        .iter()
        .take(12)
        .map(|a| noise(setup, df, fmax, a, t_peak))
        .filter(|n| n.hit.closure > 1e-5)
        .collect();
    let closure: f64 = noises.iter().map(|n| n.hit.closure).sum();
    if setup.aggressors.len() > 12 {
        notes.push(format!(
            "crosstalk from the 12 most coupled of {} nets beside the path",
            setup.aggressors.len()
        ));
    }
    // The pattern by superposition, periodic over its length.
    let random = prbs(setup.prbs, setup.bits);
    let asym: Vec<f64> = random.iter().map(|&b| if b { 1.0 } else { -1.0 }).collect();
    let na = asym.len() as isize;
    let bits = if setup.clock {
        (0..(random.len() / 2 * 2).max(128))
            .map(|k| k % 2 == 0)
            .collect()
    } else {
        random
    };
    let nb = bits.len() as isize;
    let sym: Vec<f64> = bits.iter().map(|&b| if b { 1.0 } else { -1.0 }).collect();
    let half = amp / 2.0;
    let mut ones_min = vec![f64::MAX; SAMPLES];
    let mut zeros_max = vec![f64::MIN; SAMPLES];
    let mut traces = Vec::new();
    let mut crossings: Vec<f64> = Vec::new();
    let every = (bits.len() / MAX_TRACES).max(1);
    let mut wave = vec![0.0f64; SAMPLES];
    for n in 0..nb {
        wave.iter_mut().for_each(|v| *v = 0.0);
        for (k, row) in &rows {
            let a = sym[(n - k).rem_euclid(nb) as usize];
            for (w, r) in wave.iter_mut().zip(row) {
                *w += a * r;
            }
        }
        wave.iter_mut().for_each(|v| *v *= half);
        // Each aggressor sends the pattern from its own starting point.
        for (i, nz) in noises.iter().enumerate() {
            let shift = 1 + 37 * (i as isize + 1);
            for (k, row) in &nz.rows {
                let a = asym[(n - k + shift).rem_euclid(na) as usize] * nz.half;
                for (w, r) in wave.iter_mut().zip(row) {
                    *w += a * r;
                }
            }
        }
        let one = bits[n as usize];
        for (s, v) in wave.iter().enumerate() {
            if one {
                ones_min[s] = ones_min[s].min(*v);
            } else {
                zeros_max[s] = zeros_max[s].max(*v);
            }
        }
        // Crossing towards the next bit (second half of the window).
        let next = bits[((n + 1) % nb) as usize];
        if next != one {
            for s in PER_UI..SAMPLES - 1 {
                let (a, b) = (wave[s], wave[s + 1]);
                if (a >= 0.0) != (b >= 0.0) {
                    let frac = a / (a - b);
                    crossings.push((s as f64 + frac - PER_UI as f64) * dt);
                    break;
                }
            }
        }
        if (n as usize).is_multiple_of(every) && traces.len() < MAX_TRACES {
            traces.push(wave.iter().map(|&v| v as f32).collect());
        }
    }
    let high = ones_min[PER_UI];
    let low = zeros_max[PER_UI];
    let height = (high - low).max(0.0);
    let cross_ps = if crossings.is_empty() {
        [ui * 0.5e12; 3]
    } else {
        let lo = crossings.iter().copied().fold(f64::MAX, f64::min);
        let hi = crossings.iter().copied().fold(f64::MIN, f64::max);
        let mean = crossings.iter().sum::<f64>() / crossings.len() as f64;
        [lo * 1e12, mean * 1e12, hi * 1e12]
    };
    let inner: Vec<(f32, f32)> = ones_min
        .iter()
        .zip(&zeros_max)
        .map(|(&a, &b)| (a as f32, b as f32))
        .collect();
    let jitter = if crossings.len() > 1 {
        let lo = crossings.iter().copied().fold(f64::MAX, f64::min);
        let hi = crossings.iter().copied().fold(f64::MIN, f64::max);
        hi - lo
    } else {
        0.0
    };
    let width = if height > 0.0 && !crossings.is_empty() {
        (ui - jitter).max(0.0)
    } else {
        0.0
    };
    let [z1, z2] = rep.z_ref;
    if (z1 - z2).abs() > 1.0 && rep.load_z.is_empty() {
        notes.push(format!(
            "the path goes from {} to {} ohm: even a perfect path scales the voltage by {:.2} (the square root of their ratio)",
            trim(z1),
            trim(z2),
            (z2 / z1).sqrt()
        ));
    }
    if height <= 0.0 {
        notes.push("the eye is closed at the centre".into());
    }
    let wraps = (1u64 << setup.prbs.clamp(7, 31)) - 1;
    if !setup.clock && (bits.len() as u64) < wraps {
        notes.push(format!(
            "PRBS{} cut to its first {} bits",
            setup.prbs,
            bits.len()
        ));
    }
    // The pulse around its peak for the plot: a few bits before, more after.
    let big = |r: &&(isize, Vec<f64>)| r.1.iter().any(|v| v.abs() > 0.01 * vpk);
    let first = rows.iter().find(big).map_or(0, |r| r.0).clamp(-4, 0) - 1;
    let last = rows.iter().rev().find(big).map_or(0, |r| r.0).clamp(0, 21) + 3;
    let mut pulse_pts = Vec::new();
    let span = ((last - first + 2) as usize) * PER_UI;
    let step = (span / 600).max(1);
    let mut i = 0;
    while i <= span {
        let t = t_peak + (first as f64 - 1.0) * ui + i as f64 * dt;
        pulse_pts.push((t * 1e9, amp * pulse.at(t)));
        i += step;
    }
    Ok(EyeResult {
        gbps: setup.gbps,
        ui_ps: ui * 1e12,
        rise_ps: rise * 1e12,
        amplitude: amp,
        prbs: setup.prbs,
        bits: bits.len(),
        traces,
        threshold: 0.0,
        height,
        high,
        low,
        width_ps: width * 1e12,
        jitter_ps: jitter * 1e12,
        worst_height,
        cursors,
        main,
        pulse: pulse_pts,
        peak_ns: (t_peak - ui / 2.0) * 1e9,
        notes,
        eq: setup.eq,
        worst_with_xtalk: (worst_height - closure).max(0.0),
        sample_ns: t_peak * 1e9,
        cross_ps,
        inner,
        xtalk: {
            let mut x: Vec<XtalkHit> = noises.into_iter().map(|n| n.hit).collect();
            x.sort_by(|a, b| b.closure.total_cmp(&a.closure));
            x
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_report(z0: f64, len_ns: f64, loss_db_per_ghz: f64, ghz: f64, n: usize) -> Report {
        let f: Vec<f64> = (1..=n).map(|k| ghz * k as f64 / n as f64).collect();
        let s = f
            .iter()
            .map(|&fg| {
                let a = loss_db_per_ghz * fg / 8.686;
                let b = 2.0 * PI * fg * len_ns;
                // Matched line: S21 = e^(-a - jb).
                let s21 = Cx::expj(-b).scale((-a).exp());
                [Cx::default(), s21, s21, Cx::default()]
            })
            .collect();
        Report {
            f,
            s,
            z_ref: [z0, z0],
            ..Default::default()
        }
    }

    #[test]
    fn prbs7_has_its_period() {
        let b = prbs(7, 10_000);
        assert_eq!(b.len(), 127);
        assert_eq!(b.iter().filter(|x| **x).count(), 64);
    }

    #[test]
    fn a_short_clean_line_gives_an_open_eye() {
        let rep = line_report(50.0, 1.0, 0.0, 20.0, 400);
        let e = eye(&rep, &EyeSetup::default()).unwrap();
        // Nearly the whole swing and bit, peak near 1 ns + rise.
        assert!(e.height > 0.95 * 0.8, "{}", e.summary());
        assert!(e.width_ps > 0.9 * 400.0, "{}", e.summary());
        assert!((e.peak_ns - 1.0).abs() < 0.1, "{}", e.summary());
        assert!(e.worst_height > 0.9 * 0.8);
    }

    #[test]
    fn a_lossy_line_closes_the_eye() {
        let clean = eye(
            &line_report(50.0, 1.0, 0.0, 20.0, 400),
            &EyeSetup {
                gbps: 10.0,
                ..Default::default()
            },
        )
        .unwrap();
        let lossy = eye(
            &line_report(50.0, 1.0, 3.0, 20.0, 400),
            &EyeSetup {
                gbps: 10.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            lossy.height < 0.8 * clean.height,
            "{}{}",
            clean.summary(),
            lossy.summary()
        );
        assert!(lossy.jitter_ps > clean.jitter_ps);
        // The worst case bounds the pattern's eye from below.
        assert!(lossy.worst_height <= lossy.height + 1e-6);
    }

    #[test]
    fn near_end_crosstalk_of_a_long_run_closes_by_twice_kb() {
        let rep = line_report(50.0, 1.0, 0.0, 20.0, 400);
        let tau = 1e-9;
        let kb = 0.05;
        let next: Vec<Cx> = rep
            .f
            .iter()
            .map(|g| {
                let w = 2.0 * PI * g * 1e9 * tau;
                Cx::new(1.0, 0.0)
                    .add(Cx::expj(-2.0 * w).scale(-1.0))
                    .scale(kb)
            })
            .collect();
        let a = Aggressor {
            name: "A".into(),
            length: 6000.0,
            kb,
            kf: 0.0,
            fext: vec![Cx::default(); next.len()],
            next,
            swing: 0.8,
            at: [0.0, 0.0],
        };
        let setup = EyeSetup {
            aggressors: vec![a],
            ..Default::default()
        };
        let e = eye(&rep, &setup).unwrap();
        let x = &e.xtalk[0];
        // A pulse in, +Kb then -Kb 2 tau later: summed |cursors| 2 Kb,
        // both rails: 2 x 0.4 V x 2 Kb.
        assert_eq!(x.kind, "NEXT");
        assert!((x.closure - 0.08).abs() < 0.012, "{x:?}");
        assert!((x.peak - 0.02).abs() < 0.004, "{x:?}");
        assert!(
            e.height < 0.8 - 0.05 && e.height > 0.8 - 0.1,
            "{}",
            e.summary()
        );
    }

    #[test]
    fn equalising_a_lossy_line_opens_the_eye() {
        let rep = line_report(50.0, 1.0, 4.0, 20.0, 400);
        let setup = EyeSetup {
            gbps: 10.0,
            ..Default::default()
        };
        let plain = eye(&rep, &setup).unwrap();
        let c = optimise(&rep, &setup, true, true).unwrap();
        assert!(c.after > c.before + 0.05, "{c:?}");
        assert!(
            (c.before - plain.worst_height).abs() < 0.03,
            "{c:?} {}",
            plain.summary()
        );
        let eq = eye(&rep, &EyeSetup { eq: c.eq, ..setup }).unwrap();
        assert!(
            eq.worst_height > plain.worst_height + 0.03,
            "{}{}",
            plain.summary(),
            eq.summary()
        );
        assert!(
            eq.height > plain.height,
            "{}{}",
            plain.summary(),
            eq.summary()
        );
    }
}
