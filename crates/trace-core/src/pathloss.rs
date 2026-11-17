//! Whole-path insertion and return loss: the routed path between two pins,
//! every trace section (2D field solver, with the pour beside it), every
//! via (3D solver, or a coaxial estimate), and every part on the way
//! (series capacitors, resistors, inductors, beads and baluns, and shunt
//! parts hanging off it), cascaded into one two-port.
//!
//! Parts take a vendor Touchstone file when one is given (by designator or
//! value), and otherwise a model from their value and case size.

use crate::fdtd::{self, Cx};
use crate::field;
use crate::impedance::{side_reach, LayerGeometry, SideGround};
use crate::pdn::{self, Package};
use crate::touchstone::Network;
use crate::{Board, LayerRef, Stackup};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::fmt::Write;

const C0: f64 = 299_792_458.0;
const MIL: f64 = 25.4e-6;
const TAU: f64 = 2.0 * std::f64::consts::PI;
/// Nets with more pads than this are rails or grounds: AC ground for the
/// path, never followed.
const MAX_PADS: usize = 40;
/// An open end's impedance, Ω.
const OPEN_Z: f64 = 1e12;

/// What to analyse.
#[derive(Clone, Debug)]
pub struct Options {
    /// Start: a pin ("J503.1", "U101.A5"), a part with one signal pin
    /// ("J503"), or a net name.
    pub from: String,
    /// End: as `from`; None takes the farthest pin the path reaches
    /// (a chip's, where there is one).
    pub to: Option<String>,
    /// Highest frequency, GHz, and how many points up to it.
    pub ghz: f64,
    pub points: usize,
    /// Solve each kind of via in 3D (a minute or so each); otherwise a
    /// coaxial estimate from its drill, antipad and barrel.
    pub vias_3d: bool,
    /// Vendor models: a designator or value pattern (* and ?) and its
    /// Touchstone file (.s2p for a two-pin part, .s1p for a shunt part,
    /// .s3p for a balun: port 1 unbalanced, 2 and 3 balanced).
    pub models: Vec<(String, Network)>,
    /// A one-port at the end (a chip's input, such as a vendor's .s1p):
    /// the return loss into the path with it as the load.
    pub load: Option<Network>,
    /// Loss tangent where the stack-up gives none.
    pub tan_d: f64,
    /// Lowest frequency of interest, GHz: the worst return loss is taken
    /// from here up (a DC block's corner lies below the band).
    pub fmin: f64,
    /// Fitted parts (designator patterns) taken as empty sites, for the
    /// tuner to choose again.
    pub free: Vec<String>,
    /// The tuner may put a capacitor or an inductor in any site (else C
    /// sites take capacitors and L sites inductors).
    pub any_part: bool,
    /// Of footprints stacked on the same pads, the ones fitted
    /// (designator patterns).
    pub fit: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            from: String::new(),
            to: None,
            ghz: 6.0,
            points: 200,
            vias_3d: true,
            models: Vec::new(),
            load: None,
            tan_d: 0.02,
            fmin: 0.0,
            free: Vec::new(),
            any_part: false,
            fit: Vec::new(),
        }
    }
}

/// One section of the path, in order.
#[derive(Clone, Debug)]
pub struct Section {
    pub what: String,
    /// Trace, via, part, or shunt part.
    pub kind: &'static str,
    pub net: String,
    /// Length, mil (traces; a via's barrel).
    pub length: f64,
    /// Its impedance, Ω (differential in a pair), where it has one.
    pub z: Option<f64>,
    pub diff: bool,
    /// Its own loss at the top frequency, dB (a trace's attenuation; for
    /// others |S21| in the path's reference impedance).
    pub loss_db: f64,
    /// Insertion loss from the start to the end of this section, at the
    /// top frequency, dB.
    pub total_db: f64,
    /// Where it is, mil.
    pub at: [f64; 2],
    /// How it was modelled.
    pub model: String,
}

/// The result.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub start: String,
    pub end: String,
    /// GHz.
    pub f: Vec<f64>,
    /// S11, S12, S21, S22 per frequency.
    pub s: Vec<[Cx; 4]>,
    /// Port reference impedances (50 single-ended, 100 for a pair).
    pub z_ref: [f64; 2],
    /// S11 with the load at the end, when one was given.
    pub s11_loaded: Vec<Cx>,
    /// The load's impedance per frequency, when one was given.
    pub load_z: Vec<Cx>,
    /// Nets the path's traces run on, in order, each with its pair's
    /// negative line where the run is differential.
    pub victims: Vec<(u32, Option<u32>)>,
    pub sections: Vec<Section>,
    /// Trace length along the path, mil.
    pub length: f64,
    pub notes: Vec<String>,
    /// Lowest frequency of interest, GHz (for the worst return loss).
    pub fmin: f64,
    /// Where the load's model is valid, GHz (its stated range, else its
    /// points).
    pub load_band: Option<(f64, f64)>,
}

impl Report {
    /// Insertion loss at the top frequency, dB (positive).
    pub fn il_db(&self) -> f64 {
        self.s.last().map_or(0.0, |s| -s[2].db())
    }

    /// Insertion loss at `f` GHz, dB, linear between points.
    pub fn il_at(&self, f: f64) -> Option<f64> {
        let k = self.f.partition_point(|&x| x < f);
        let k = k.min(self.f.len().checked_sub(1)?);
        Some(-self.s[k][2].db())
    }

    /// The worst (highest) return loss in the band of interest, dB, and
    /// where.
    pub fn worst_rl(&self) -> Option<(f64, f64)> {
        self.f
            .iter()
            .zip(&self.s)
            .filter(|(f, _)| **f >= self.fmin - 1e-12)
            .map(|(f, s)| (s[0].db(), *f))
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The worst return loss into the path with the load at the end,
    /// within the band of interest and the load's valid range.
    pub fn worst_loaded(&self) -> Option<(f64, f64)> {
        let (lo, hi) = self.load_band.unwrap_or((0.0, f64::MAX));
        self.f
            .iter()
            .zip(&self.s11_loaded)
            .filter(|(f, _)| **f >= self.fmin.max(lo) - 1e-12 && **f <= hi + 1e-12)
            .map(|(f, s)| (s.db(), *f))
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    pub fn summary(&self) -> String {
        let top = self.f.last().copied().unwrap_or(0.0);
        let mut o = format!(
            "{} to {}: {:.2} in of trace, {} section(s); insertion loss {:.3} dB at {top} GHz",
            self.start,
            self.end,
            self.length / 1000.0,
            self.sections.len(),
            self.il_db()
        );
        if let Some((rl, f)) = self.worst_rl() {
            let _ = write!(o, ", return loss worst {rl:.1} dB at {f:.2} GHz");
            if self.fmin > 0.0 {
                let _ = write!(o, " (from {} GHz)", trim_num(self.fmin));
            }
        }
        o
    }

    /// A Touchstone file: version 1 when both ports share a reference,
    /// version 2 (with [Reference]) when they do not.
    pub fn touchstone(&self) -> String {
        let mut o = String::new();
        let same = (self.z_ref[0] - self.z_ref[1]).abs() < 1e-9;
        if !same {
            let _ = writeln!(o, "[Version] 2.0");
        }
        let _ = writeln!(o, "! Trace whole-path S-parameters: {}", self.summary());
        if self.z_ref.iter().any(|z| *z > 60.0) {
            let _ = writeln!(
                o,
                "! A differential port is the mixed-mode Sdd port (reference 100 ohm)"
            );
        }
        let _ = writeln!(o, "# GHz S MA R {}", self.z_ref[0]);
        if !same {
            let _ = writeln!(o, "[Number of Ports] 2");
            let _ = writeln!(o, "[Two-Port Data Order] 21_12");
            let _ = writeln!(o, "[Reference] {} {}", self.z_ref[0], self.z_ref[1]);
            let _ = writeln!(o, "[Network Data]");
        }
        for (f, s) in self.f.iter().zip(&self.s) {
            // Order S11 S21 S12 S22 for a 2-port.
            let _ = write!(o, "{f:.6}");
            for v in [s[0], s[2], s[1], s[3]] {
                let _ = write!(o, " {:.6e} {:.3}", v.abs(), v.deg());
            }
            o.push('\n');
        }
        if !same {
            let _ = writeln!(o, "[End]");
        }
        o
    }

    pub fn csv(&self) -> String {
        let top = self.f.last().copied().unwrap_or(0.0);
        let mut o = format!(
            "section,kind,net,length_mil,z_ohm,diff,own_loss_db_at_{top}ghz,total_il_db_at_{top}ghz,x_mil,y_mil,model\n"
        );
        for s in &self.sections {
            let _ = writeln!(
                o,
                "\"{}\",{},\"{}\",{:.1},{},{},{:.4},{:.4},{:.1},{:.1},\"{}\"",
                s.what.replace('"', "'"),
                s.kind,
                s.net,
                s.length,
                s.z.map_or(String::new(), |z| format!("{z:.1}")),
                s.diff,
                s.loss_db,
                s.total_db,
                s.at[0],
                s.at[1],
                s.model.replace('"', "'")
            );
        }
        o
    }

    /// Insertion loss (upper panel) and return loss (lower) against
    /// frequency, as SVG.
    pub fn svg(&self, title: &str) -> String {
        let (w, h) = (900.0, 620.0);
        let (l, r) = (64.0, 24.0);
        let fmax = self.f.last().copied().unwrap_or(1.0).max(1e-9);
        let x = |f: f64| l + (w - l - r) * f / fmax;
        let il: Vec<f64> = self.s.iter().map(|s| s[2].db()).collect();
        let rl: Vec<f64> = self.s.iter().map(|s| s[0].db()).collect();
        let loaded: Vec<f64> = self.s11_loaded.iter().map(|s| s.db()).collect();
        let nice = |v: f64| {
            let steps = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0];
            let s = steps.into_iter().find(|s| v / s <= 8.0).unwrap_or(50.0);
            (((v / s).ceil()).max(1.0) * s, s)
        };
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
        // Panels: (top, bottom, data, lowest dB, label, colour).
        let il_lo = il.iter().copied().fold(0.0f64, f64::min);
        let (il_span, il_step) = nice(-il_lo.max(-60.0));
        let rl_lo = rl
            .iter()
            .chain(&loaded)
            .copied()
            .fold(0.0f64, f64::min)
            .max(-60.0);
        let (rl_span, rl_step) = nice(-rl_lo);
        let panels: [(f64, f64, f64, f64, &str); 2] = [
            (44.0, 300.0, il_span, il_step, "insertion |S21|, dB"),
            (340.0, 580.0, rl_span, rl_step, "return |S11|, dB"),
        ];
        for (k, (t, b, span, step, name)) in panels.iter().enumerate() {
            let y = |d: f64| t + (b - t) * (-d).clamp(0.0, *span) / span;
            let mut d = 0.0;
            while d <= span + 1e-9 {
                let _ = writeln!(
                    o,
                    r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#e3e6ec"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{}</text>"##,
                    w - r,
                    y(-d),
                    y(-d),
                    l - 6.0,
                    y(-d) + 4.0,
                    trim_num(-d)
                );
                d += step;
            }
            let fstep = [0.5, 1.0, 2.0, 5.0, 10.0, 20.0]
                .into_iter()
                .find(|s| fmax / s <= 12.0)
                .unwrap_or(50.0);
            let mut f = 0.0;
            while f <= fmax + 1e-9 {
                let _ = writeln!(
                    o,
                    r##"<line x1="{:.1}" x2="{:.1}" y1="{t}" y2="{b}" stroke="#e3e6ec"/><text x="{:.1}" y="{}" text-anchor="middle" fill="#555">{}</text>"##,
                    x(f),
                    x(f),
                    x(f),
                    b + 16.0,
                    trim_num(f)
                );
                f += fstep;
            }
            let series: Vec<(&Vec<f64>, &str, &str)> = if k == 0 {
                vec![(&il, "#1f6feb", "path")]
            } else if loaded.is_empty() {
                vec![(&rl, "#d9480f", "path, 50/100 ohm ends")]
            } else {
                vec![
                    (&rl, "#d9480f", "path, 50/100 ohm ends"),
                    (&loaded, "#7048e8", "into the path, with the load"),
                ]
            };
            for (i, (data, colour, label)) in series.iter().enumerate() {
                let pts: Vec<String> = self
                    .f
                    .iter()
                    .zip(data.iter())
                    .map(|(f, v)| format!("{:.1},{:.1}", x(*f), y(*v)))
                    .collect();
                let _ = writeln!(
                    o,
                    r##"<polyline points="{}" fill="none" stroke="{colour}" stroke-width="1.8"/>"##,
                    pts.join(" ")
                );
                let ly = t + 14.0 + 16.0 * i as f64;
                let _ = writeln!(
                    o,
                    r##"<line x1="{}" x2="{}" y1="{ly}" y2="{ly}" stroke="{colour}" stroke-width="2"/><text x="{}" y="{}" fill="#222">{}</text>"##,
                    w - r - 230.0,
                    w - r - 210.0,
                    w - r - 204.0,
                    ly + 4.0,
                    esc(label)
                );
            }
            let _ = writeln!(
                o,
                r##"<text x="18" y="{:.1}" fill="#555" transform="rotate(-90 18 {:.1})" text-anchor="middle">{name}</text>"##,
                (t + b) / 2.0,
                (t + b) / 2.0
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="{}" y="{}" text-anchor="middle" fill="#555">GHz</text></svg>"##,
            (l + w - r) / 2.0,
            h - 8.0
        );
        o
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn trim_num(v: f64) -> String {
    let v = if v.abs() < 1e-9 { 0.0 } else { v };
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map_or(s.clone(), |t| t.to_string())
}

// ---------------------------------------------------------------------
// Two-port algebra.

type Abcd = [Cx; 4];

fn c(re: f64) -> Cx {
    Cx::new(re, 0.0)
}

fn sub(a: Cx, b: Cx) -> Cx {
    a.add(b.scale(-1.0))
}

const IDENT: Abcd = [
    Cx { re: 1.0, im: 0.0 },
    Cx { re: 0.0, im: 0.0 },
    Cx { re: 0.0, im: 0.0 },
    Cx { re: 1.0, im: 0.0 },
];

fn mul(a: &Abcd, b: &Abcd) -> Abcd {
    [
        a[0].mul(b[0]).add(a[1].mul(b[2])),
        a[0].mul(b[1]).add(a[1].mul(b[3])),
        a[2].mul(b[0]).add(a[3].mul(b[2])),
        a[2].mul(b[1]).add(a[3].mul(b[3])),
    ]
}

fn series(z: Cx) -> Abcd {
    [c(1.0), z, c(0.0), c(1.0)]
}

fn shunt(y: Cx) -> Abcd {
    [c(1.0), c(0.0), y, c(1.0)]
}

/// The same two-port in each line of a pair, seen by the differential
/// mode (Vd = 2V, Id = I).
fn to_diff(m: &Abcd) -> Abcd {
    [m[0], m[1].scale(2.0), m[2].scale(0.5), m[3]]
}

fn cexp(z: Cx) -> Cx {
    Cx::expj(z.im).scale(z.re.exp())
}

/// A uniform line: impedance `zc`, `eeff`, `len` mil, `alpha` Np/m, at
/// `f` GHz.
fn line(zc: f64, eeff: f64, len: f64, alpha: f64, f: f64) -> Abcd {
    let beta = TAU * f * 1e9 * eeff.max(1.0).sqrt() / C0;
    let l = len * MIL;
    let gl = Cx::new(alpha * l, beta * l);
    let (e, em) = (cexp(gl), cexp(gl.scale(-1.0)));
    let ch = e.add(em).scale(0.5);
    let sh = sub(e, em).scale(0.5);
    let z = c(zc);
    [ch, sh.mul(z), sh.div(z), ch]
}

/// An open stub's input admittance: tanh(γl)/Zc.
fn open_stub(zc: f64, eeff: f64, len: f64, alpha: f64, f: f64) -> Cx {
    let m = line(zc, eeff, len, alpha, f);
    // Zin of an open end = A / C.
    m[2].div(m[0])
}

/// S (S11, S12, S21, S22, port references `z1`, `z2`) to ABCD.
pub fn abcd_from_s(s: &[Cx; 4], z1: f64, z2: f64) -> [Cx; 4] {
    let one = c(1.0);
    let (s11, s12, s21, s22) = (s[0], s[1], s[2], s[3]);
    let p = s12.mul(s21);
    let d = s21.scale(2.0);
    let (r1, r2) = (z1.sqrt(), z2.sqrt());
    [
        one.add(s11).mul(sub(one, s22)).add(p).div(d).scale(r1 / r2),
        one.add(s11)
            .mul(one.add(s22))
            .add(p.scale(-1.0))
            .div(d)
            .scale(r1 * r2),
        sub(one, s11)
            .mul(sub(one, s22))
            .add(p.scale(-1.0))
            .div(d)
            .scale(1.0 / (r1 * r2)),
        sub(one, s11).mul(one.add(s22)).add(p).div(d).scale(r2 / r1),
    ]
}

/// ABCD to S (S11, S12, S21, S22) with port references `z1`, `z2`.
pub fn s_from_abcd(m: &[Cx; 4], z1: f64, z2: f64) -> [Cx; 4] {
    let (a, b, cc, d) = (m[0], m[1], m[2], m[3]);
    let (za, zb) = (c(z1), c(z2));
    let den = a.mul(zb).add(b).add(cc.mul(za).mul(zb)).add(d.mul(za));
    let k = (z1 * z2).sqrt();
    let det = sub(a.mul(d), b.mul(cc));
    [
        sub(a.mul(zb).add(b), cc.mul(za).mul(zb).add(d.mul(za))).div(den),
        det.scale(2.0 * k).div(den),
        c(2.0 * k).div(den),
        sub(b.add(d.mul(za)), a.mul(zb).add(cc.mul(za).mul(zb))).div(den),
    ]
}

/// Input impedance of a two-port ending in `zl`.
fn z_in(m: &Abcd, zl: Cx) -> Cx {
    m[0].mul(zl).add(m[1]).div(m[2].mul(zl).add(m[3]))
}

/// Where a model is valid, GHz: its "Valid Frequency Range: 1 GHz to
/// 6 GHz" comment, else the span of its points.
fn valid_band(n: &Network) -> Option<(f64, f64)> {
    for c in &n.comments {
        let low = c.to_ascii_lowercase();
        let Some(i) = low.find("valid frequency range") else {
            continue;
        };
        let rest = &low[i..];
        let nums: Vec<(f64, f64)> = rest
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
            .collect::<Vec<_>>()
            .windows(2)
            .filter_map(|w| {
                let v: f64 = w[0].parse().ok()?;
                let mul = match w[1] {
                    "ghz" => 1.0,
                    "mhz" => 1e-3,
                    "khz" => 1e-6,
                    "hz" => 1e-9,
                    _ => return None,
                };
                Some((v * mul, mul))
            })
            .collect();
        if nums.len() >= 2 {
            return Some((nums[0].0, nums[1].0));
        }
    }
    Some((*n.f.first()?, *n.f.last()?))
}

/// A network's S at `f`, held at its first or last point outside its band.
fn at_clamped(n: &Network, f: f64) -> Option<Vec<Cx>> {
    let lo = *n.f.first()?;
    let hi = *n.f.last()?;
    n.at(f.clamp(lo, hi))
}

// ---------------------------------------------------------------------
// Part models.

/// A part's own model.
#[derive(Clone, Debug)]
pub enum PartModel {
    /// R, L and C in series (C None: no capacitor), with `cp` farads
    /// across the whole.
    Rlc {
        r: f64,
        l: f64,
        c: Option<f64>,
        cp: f64,
    },
    /// A ferrite bead: its impedance at 100 MHz (R across L across C).
    Bead { z100: f64 },
    /// A balun as an ideal transformer: balanced over unbalanced
    /// impedance ratio.
    Ideal { ratio: f64 },
    /// A vendor file.
    File(Network),
}

impl PartModel {
    /// A two-terminal model's impedance at `f` GHz.
    fn z(&self, f: f64) -> Option<Cx> {
        let w = TAU * f * 1e9;
        match self {
            PartModel::Rlc { r, l, c: cap, cp } => {
                let mut z = Cx::new(*r, w * l);
                if let Some(cap) = cap {
                    z = z.add(Cx::new(0.0, -1.0 / (w * cap)));
                }
                if *cp > 0.0 {
                    let y = c(1.0).div(z).add(Cx::new(0.0, w * cp));
                    z = c(1.0).div(y);
                }
                Some(z)
            }
            PartModel::Bead { z100 } => {
                let l = z100 / (TAU * 100e6);
                let cp = 0.3e-12;
                let y = Cx::new(1.0 / z100, w * cp).add(Cx::new(0.0, -1.0 / (w * l)));
                Some(c(1.0).div(y))
            }
            PartModel::File(n) if n.ports == 1 => Some(crate::touchstone::gamma_to_z(
                *at_clamped(n, f)?.first()?,
                n.z_ref,
            )),
            _ => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            PartModel::Rlc { r, l, c: cap, cp } => {
                let mut s = Vec::new();
                if let Some(cap) = cap {
                    s.push(pdn::farads_text(*cap));
                }
                if *r > 0.0 {
                    s.push(format!("{} Ω", sig(*r)));
                }
                if *l > 0.0 {
                    s.push(format!("{:.2} nH", l * 1e9));
                }
                if *cp > 0.0 {
                    s.push(format!("{:.2} pF across", cp * 1e12));
                }
                s.join(" + ")
            }
            PartModel::Bead { z100 } => format!("bead {z100:.0} Ω at 100 MHz"),
            PartModel::Ideal { ratio } => format!("ideal 1:{ratio:.2} (no model given)"),
            PartModel::File(n) => {
                format!(
                    "vendor model, {} port(s), {:.2}-{:.2} GHz",
                    n.ports,
                    n.f.first().unwrap_or(&0.0),
                    n.f.last().unwrap_or(&0.0)
                )
            }
        }
    }
}

fn sig(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 1.0 {
        format!("{v:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        format!("{v:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// Henries from a value such as 10nH, 2.2uH, 4n7, 100 nH.
pub fn parse_henries(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase().replace(['µ', 'μ'], "u");
    let b = t.as_bytes();
    for i in 0..b.len() {
        if !b[i].is_ascii_digit()
            || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.'))
        {
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
            j += 1;
        }
        let num = &t[i..j];
        let mut k = j;
        while k < b.len() && b[k] == b' ' {
            k += 1;
        }
        let mul = match b.get(k) {
            Some(b'p') => 1e-12,
            Some(b'n') => 1e-9,
            Some(b'u') => 1e-6,
            Some(b'm') => 1e-3,
            _ => continue,
        };
        // 4n7: the prefix is the decimal point.
        let mut e = k + 1;
        let mut frac = String::new();
        while e < b.len() && b[e].is_ascii_digit() {
            frac.push(b[e] as char);
            e += 1;
        }
        let henry = b.get(e) == Some(&b'h');
        if henry || !frac.is_empty() {
            let text = if frac.is_empty() {
                num.to_string()
            } else {
                format!("{num}.{frac}")
            };
            if let Ok(v) = text.parse::<f64>() {
                return Some(v * mul);
            }
        }
    }
    None
}

/// Impedance ratio of a balun from its value or part number: "50:100",
/// "1:2" (taken as impedances), else 2 (50 Ω to 100 Ω).
fn balun_ratio(text: &str) -> f64 {
    for tok in text.split(|c: char| c.is_whitespace() || c == ',' || c == '/' || c == '_') {
        if let Some((a, b)) = tok.split_once(':') {
            let num = |x: &str| {
                x.trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
                    .parse::<f64>()
                    .ok()
            };
            if let (Some(a), Some(b)) = (num(a), num(b)) {
                if a > 0.0 && b > 0.0 {
                    return b / a;
                }
            }
        }
    }
    2.0
}

fn prefix(des: &str) -> String {
    des.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn is_balun(board: &Board, comp: usize) -> bool {
    let p = &board.components[comp];
    let text = format!(
        "{} {} {}",
        p.param(&["Value", "Comment", "Device Type", "Part Number"])
            .unwrap_or(""),
        p.footprint,
        p.library_ref.as_deref().unwrap_or("")
    )
    .to_ascii_uppercase();
    let pre = prefix(&p.designator);
    pre == "T"
        || pre == "BAL"
        || pre == "TR"
        || text.contains("BALUN")
        || text.contains("XFMR")
        || text.contains("TRANSFORMER")
}

/// A fitted part the tuner may choose again.
fn freed(opt: &Options, des: &str) -> bool {
    let d = des.to_ascii_uppercase();
    opt.free
        .iter()
        .any(|p| crate::constraints::glob(&p.to_ascii_uppercase(), &d))
}

/// A value that says the part is not chosen or not fitted: TBD, DNI,
/// DNP, NC, N/A, empty.
pub(crate) fn placeholder(value: &str) -> bool {
    let v = value.trim().to_ascii_uppercase();
    v.is_empty()
        || [
            "TBD", "DNI", "DNP", "DNF", "NC", "N/C", "N/A", "NF", "NOPOP", "NO POP", "OPEN", "NP",
            "?",
        ]
        .iter()
        .any(|k| {
            // "TBD0201" is a placeholder; "NC7WZ16" (a logic part) is not.
            let next = |c: char| c.is_ascii_alphabetic() || k.len() <= 2 && c.is_ascii_digit();
            v == *k || v.starts_with(k) && !v[k.len()..].starts_with(next)
        })
}

/// The model of a part: a vendor file whose pattern matches its
/// designator or value, else one from its value and case; None for a
/// part not fitted (in the variant, or valued TBD/DNI) or whose value
/// says nothing (a capacitor without farads).
fn part_model(
    board: &Board,
    comp: usize,
    pads: &[usize],
    opt: &Options,
    want_ports: usize,
) -> Option<PartModel> {
    if !crate::variant::fitted(board, comp) {
        return None;
    }
    if freed(opt, &board.components[comp].designator) {
        return None;
    }
    let p = &board.components[comp];
    let value = p
        .param(&["Value", "Comment", "Device Type"])
        .unwrap_or("")
        .to_string();
    let pn = p
        .param(&[
            "Part Number",
            "Manufacturer Part Number",
            "MPN",
            "PartNumber",
        ])
        .unwrap_or("")
        .to_string();
    for (pat, net) in &opt.models {
        let pu = pat.to_ascii_uppercase();
        let hit = [&p.designator, &value, &pn]
            .iter()
            .any(|t| !t.is_empty() && crate::constraints::glob(&pu, &t.to_ascii_uppercase()));
        if hit && net.ports == want_ports {
            return Some(PartModel::File(net.clone()));
        }
    }
    if want_ports == 3 {
        return Some(PartModel::Ideal {
            ratio: balun_ratio(&format!("{value} {pn} {}", p.footprint)),
        });
    }
    if placeholder(&value) {
        return None;
    }
    let g = &board.geometry;
    let pitch = if pads.len() >= 2 {
        let (a, b) = (&g.pads[pads[0]].at, &g.pads[pads[1]].at);
        (a.x.mil() - b.x.mil()).hypot(a.y.mil() - b.y.mil())
    } else {
        40.0
    };
    let pack = Package::from_pitch(pitch);
    let pre = prefix(&p.designator);
    let up = format!("{value} {}", p.footprint).to_ascii_uppercase();
    let bead = matches!(pre.as_str(), "FB" | "E" | "B" | "BD" | "FER")
        || up.contains("BEAD")
        || up.contains("BLM")
        || up.contains("FERRITE");
    if bead {
        let f = pdn::Feed {
            part: comp,
            designator: p.designator.clone(),
            net: 0,
            net_name: String::new(),
            bead: true,
            value: value.clone(),
        };
        let z100 = match pdn::bead_model(&f) {
            pdn::BeadModel::Bead { z100, .. } => z100,
            pdn::BeadModel::Inductor(_) => 220.0,
        };
        return Some(PartModel::Bead { z100 });
    }
    if pre == "C" || (pre != "R" && pre != "L" && pdn::parse_farads(&value).is_some()) {
        let farads = pdn::parse_farads(&value)?;
        let m = pdn::CapModel::mlcc(farads, pack);
        // Small RF values have a far lower ESR than the decoupling rule.
        let esr = if farads < 1e-9 { 0.15 } else { m.esr };
        return Some(PartModel::Rlc {
            r: esr,
            l: m.esl,
            c: Some(farads),
            cp: 0.0,
        });
    }
    if pre == "L" {
        // A 0 Ω link on an inductor footprint is common.
        let h = match parse_henries(&value) {
            Some(h) => h,
            None if pdn::parse_ohms(&value).is_some_and(|r| r < 1.0) => 0.0,
            None => return None,
        };
        return Some(PartModel::Rlc {
            r: if h > 0.0 { 0.2 } else { 0.01 },
            l: if h > 0.0 { h } else { pack.esl() * 0.8 },
            c: None,
            cp: if h > 0.0 { 0.1e-12 } else { 0.0 },
        });
    }
    if pre == "D" {
        // An ESD diode or similar off the line: its capacitance.
        return Some(PartModel::Rlc {
            r: 0.5,
            l: 0.3e-9,
            c: Some(0.5e-12),
            cp: 0.0,
        });
    }
    // A resistor (0 Ω links too).
    let r = pdn::parse_ohms(&value)?;
    Some(PartModel::Rlc {
        r,
        l: pack.esl() * 0.8,
        c: None,
        cp: if r > 10.0 { 0.03e-12 } else { 0.0 },
    })
}

// ---------------------------------------------------------------------
// The copper graph.

#[derive(Clone, Debug)]
enum Ek {
    Track {
        track: usize,
        net: u32,
        layer: u8,
        width: f64,
        len: f64,
        a: [f64; 2],
        b: [f64; 2],
    },
    Arc {
        net: u32,
        layer: u8,
        width: f64,
        len: f64,
        mid: [f64; 2],
    },
    Via {
        via: usize,
        net: u32,
        la: u8,
        lb: u8,
    },
    Part {
        comp: usize,
        pa: usize,
        pb: usize,
        balun: bool,
    },
}

#[derive(Clone, Debug)]
struct Edge {
    a: usize,
    b: usize,
    cost: f64,
    kind: Ek,
}

struct Graph {
    n: usize,
    adj: Vec<Vec<usize>>,
    edges: Vec<Edge>,
    /// Pad index -> node.
    pad_node: HashMap<usize, usize>,
}

struct Uf(Vec<usize>);

impl Uf {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

fn mil(p: &crate::Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Whether `q` lies on the pad's copper (with `margin` mil).
fn in_pad(p: &crate::Pad, q: [f64; 2], margin: f64) -> bool {
    if p.outline.len() >= 3 {
        let pts: Vec<[f64; 2]> = p.outline.iter().map(mil).collect();
        if fdtd::point_in(&pts, q) {
            return true;
        }
    }
    let c = mil(&p.at);
    let (sx, sy) = (p.size_x.mil() / 2.0 + margin, p.size_y.mil() / 2.0 + margin);
    let (s, co) = (-p.rotation_deg).to_radians().sin_cos();
    let (dx, dy) = (q[0] - c[0], q[1] - c[1]);
    let (u, v) = (dx * co - dy * s, dx * s + dy * co);
    match p.shape {
        crate::PadShape::Round if (sx - sy).abs() < 1e-6 => u.hypot(v) <= sx,
        _ => u.abs() <= sx && v.abs() <= sy,
    }
}

fn pad_on(p: &crate::Pad, layer: u8) -> bool {
    match p.layer {
        LayerRef::Copper(l) => l == layer || p.hole.0 > 0,
        LayerRef::AllCopper => true,
        _ => false,
    }
}

/// The graph of copper of `nets`, and part edges between them.
fn build(board: &Board, nets: &HashSet<u32>, ground: &[bool]) -> Graph {
    let g = &board.geometry;
    // Raw points: (layer or None for every layer, position, reach).
    struct Pt {
        layer: Option<u8>,
        at: [f64; 2],
        net: u32,
    }
    let mut pts: Vec<Pt> = Vec::new();
    let mut edges: Vec<(usize, usize, f64, Ek)> = Vec::new();
    // Tracks, split where another end or a via lands on them.
    let tracks: Vec<usize> = (0..g.tracks.len())
        .filter(|&i| {
            let t = &g.tracks[i];
            !t.text
                && t.net.is_some_and(|n| nets.contains(&n))
                && matches!(t.layer, LayerRef::Copper(_))
        })
        .collect();
    let arcs: Vec<usize> = (0..g.arcs.len())
        .filter(|&i| {
            let a = &g.arcs[i];
            a.net.is_some_and(|n| nets.contains(&n)) && matches!(a.layer, LayerRef::Copper(_))
        })
        .collect();
    let vias: Vec<usize> = (0..g.vias.len())
        .filter(|&i| g.vias[i].net.is_some_and(|n| nets.contains(&n)))
        .collect();
    let pads: Vec<usize> = (0..g.pads.len())
        .filter(|&i| g.pads[i].net.is_some_and(|n| nets.contains(&n)))
        .collect();
    let arc_ends = |i: usize| -> ([f64; 2], [f64; 2], f64) {
        let a = &g.arcs[i];
        let cc = mil(&a.center);
        let r = a.radius.mil();
        let p = |d: f64| {
            let (s, co) = d.to_radians().sin_cos();
            [cc[0] + r * co, cc[1] + r * s]
        };
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep < 1e-9 {
            sweep = 360.0;
        }
        (p(a.start_deg), p(a.end_deg), r * sweep.to_radians())
    };
    // End points by layer, for splitting.
    // (point, how far it reaches): ends of wires and vias, and pads,
    // which a track may run through rather than end in.
    type Reach = Vec<([f64; 2], f64)>;
    let mut ends: HashMap<(u32, u8), Reach> = HashMap::new();
    for &i in &tracks {
        let t = &g.tracks[i];
        let LayerRef::Copper(l) = t.layer else {
            continue;
        };
        let e = ends.entry((t.net.unwrap_or(0), l)).or_default();
        e.push((mil(&t.start), 0.05));
        e.push((mil(&t.end), 0.05));
    }
    let layers = board.stackup.copper_depths().len().max(1) as u8;
    for &i in &pads {
        let p = &g.pads[i];
        let reach = p.size_x.mil().min(p.size_y.mil()) / 2.0;
        for l in 0..layers {
            if pad_on(p, l) {
                ends.entry((p.net.unwrap_or(0), l))
                    .or_default()
                    .push((mil(&p.at), reach));
            }
        }
    }
    for &i in &arcs {
        let a = &g.arcs[i];
        let LayerRef::Copper(l) = a.layer else {
            continue;
        };
        let (s, e, _) = arc_ends(i);
        let v = ends.entry((a.net.unwrap_or(0), l)).or_default();
        v.push((s, 0.05));
        v.push((e, 0.05));
    }
    for &i in &vias {
        let v = &g.vias[i];
        for l in v.from_layer..=v.to_layer {
            ends.entry((v.net.unwrap_or(0), l))
                .or_default()
                .push((mil(&v.at), 0.05));
        }
    }
    for &i in &tracks {
        let t = &g.tracks[i];
        let LayerRef::Copper(l) = t.layer else {
            continue;
        };
        let net = t.net.unwrap_or(0);
        let (a, b) = (mil(&t.start), mil(&t.end));
        let hw = t.width.mil() / 2.0;
        let len = dist(a, b);
        let mut cuts: Vec<f64> = vec![0.0, 1.0];
        if len > 1e-6 {
            for (q, reach) in ends.get(&(net, l)).into_iter().flatten() {
                let t_ =
                    ((q[0] - a[0]) * (b[0] - a[0]) + (q[1] - a[1]) * (b[1] - a[1])) / (len * len);
                if t_ * len <= hw || (1.0 - t_) * len <= hw {
                    continue;
                }
                let p = [a[0] + t_ * (b[0] - a[0]), a[1] + t_ * (b[1] - a[1])];
                if dist(p, *q) <= hw + reach {
                    cuts.push(t_);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() * len < 0.01);
        for w in cuts.windows(2) {
            let pa = [a[0] + w[0] * (b[0] - a[0]), a[1] + w[0] * (b[1] - a[1])];
            let pb = [a[0] + w[1] * (b[0] - a[0]), a[1] + w[1] * (b[1] - a[1])];
            let ia = pts.len();
            pts.push(Pt {
                layer: Some(l),
                at: pa,
                net,
            });
            pts.push(Pt {
                layer: Some(l),
                at: pb,
                net,
            });
            let seg = dist(pa, pb);
            edges.push((
                ia,
                ia + 1,
                seg,
                Ek::Track {
                    track: i,
                    net,
                    layer: l,
                    width: t.width.mil(),
                    len: seg,
                    a: pa,
                    b: pb,
                },
            ));
        }
    }
    for &i in &arcs {
        let a = &g.arcs[i];
        let LayerRef::Copper(l) = a.layer else {
            continue;
        };
        let net = a.net.unwrap_or(0);
        let (s, e, len) = arc_ends(i);
        let ia = pts.len();
        pts.push(Pt {
            layer: Some(l),
            at: s,
            net,
        });
        pts.push(Pt {
            layer: Some(l),
            at: e,
            net,
        });
        let cc = mil(&a.center);
        let mid_deg = a.start_deg + (a.end_deg - a.start_deg).rem_euclid(360.0) / 2.0;
        let (sn, co) = mid_deg.to_radians().sin_cos();
        let r = a.radius.mil();
        edges.push((
            ia,
            ia + 1,
            len,
            Ek::Arc {
                net,
                layer: l,
                width: a.width.mil(),
                len,
                mid: [cc[0] + r * co, cc[1] + r * sn],
            },
        ));
    }
    let wires = pts.len();
    // Vias: a point per layer of their span, joined pairwise.
    let mut via_pts: Vec<(usize, Vec<(u8, usize)>)> = Vec::new();
    for &i in &vias {
        let v = &g.vias[i];
        let net = v.net.unwrap_or(0);
        let mut mine = Vec::new();
        for l in v.from_layer..=v.to_layer {
            mine.push((l, pts.len()));
            pts.push(Pt {
                layer: Some(l),
                at: mil(&v.at),
                net,
            });
        }
        via_pts.push((i, mine));
    }
    let mut pad_pt: HashMap<usize, usize> = HashMap::new();
    for &i in &pads {
        let p = &g.pads[i];
        let layer = match p.layer {
            LayerRef::Copper(l) if p.hole.0 == 0 => Some(l),
            _ => None,
        };
        pad_pt.insert(i, pts.len());
        pts.push(Pt {
            layer,
            at: mil(&p.at),
            net: p.net.unwrap_or(0),
        });
    }
    let mut uf = Uf((0..pts.len()).collect());
    // Wire ends meeting.
    let mut by_cell: HashMap<(u32, u8, i64, i64), Vec<usize>> = HashMap::new();
    let cell = 20.0;
    for (i, p) in pts.iter().enumerate().take(wires) {
        let l = p.layer.unwrap_or(0);
        by_cell
            .entry((
                p.net,
                l,
                (p.at[0] / cell).floor() as i64,
                (p.at[1] / cell).floor() as i64,
            ))
            .or_default()
            .push(i);
    }
    let reach_of = |i: usize| -> f64 {
        // Half the width of the wire that owns point i.
        let e = &edges[i / 2];
        match e.3 {
            Ek::Track { width, .. } | Ek::Arc { width, .. } => width / 2.0,
            _ => 0.5,
        }
    };
    for i in 0..wires {
        let p = &pts[i];
        let l = p.layer.unwrap_or(0);
        let (cx, cy) = (
            (p.at[0] / cell).floor() as i64,
            (p.at[1] / cell).floor() as i64,
        );
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(list) = by_cell.get(&(p.net, l, cx + dx, cy + dy)) {
                    for &j in list {
                        if j <= i || j / 2 == i / 2 {
                            continue;
                        }
                        let tol = reach_of(i).min(reach_of(j)).max(0.2);
                        if dist(p.at, pts[j].at) <= tol {
                            uf.join(i, j);
                        }
                    }
                }
            }
        }
    }
    // Wire ends on vias and pads.
    for (vi, mine) in &via_pts {
        let v = &g.vias[*vi];
        let r = v.diameter.mil() / 2.0 + 0.05;
        let c0 = mil(&v.at);
        for &(l, k) in mine {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let key = (
                        v.net.unwrap_or(0),
                        l,
                        (c0[0] / cell).floor() as i64 + dx,
                        (c0[1] / cell).floor() as i64 + dy,
                    );
                    if let Some(list) = by_cell.get(&key) {
                        for &j in list {
                            if dist(pts[j].at, c0) <= r {
                                uf.join(j, k);
                            }
                        }
                    }
                }
            }
            // Via in pad.
            for &pi in &pads {
                let p = &g.pads[pi];
                if p.net == v.net && pad_on(p, l) && in_pad(p, c0, 0.0) {
                    uf.join(k, pad_pt[&pi]);
                }
            }
        }
    }
    for &pi in &pads {
        let p = &g.pads[pi];
        let c0 = mil(&p.at);
        let half = p.size_x.mil().max(p.size_y.mil()) / 2.0 + cell;
        for (i, q) in pts.iter().enumerate().take(wires) {
            if q.net != p.net.unwrap_or(u32::MAX) || dist(q.at, c0) > half {
                continue;
            }
            if pad_on(p, q.layer.unwrap_or(0)) && in_pad(p, q.at, reach_of(i).max(0.05)) {
                uf.join(i, pad_pt[&pi]);
            }
        }
    }
    // Nodes.
    let mut node_of: HashMap<usize, usize> = HashMap::new();
    let mut n = 0;
    let mut node = |uf: &mut Uf, i: usize| -> usize {
        let r = uf.find(i);
        *node_of.entry(r).or_insert_with(|| {
            n += 1;
            n - 1
        })
    };
    let mut out: Vec<Edge> = Vec::new();
    for (a, b, cost, kind) in edges {
        let (na, nb) = (node(&mut uf, a), node(&mut uf, b));
        if na != nb {
            out.push(Edge {
                a: na,
                b: nb,
                cost,
                kind,
            });
        }
    }
    let st = &board.stackup;
    let depth = st.copper_depths();
    for (vi, mine) in &via_pts {
        for x in 0..mine.len() {
            for y in x + 1..mine.len() {
                let (la, ka) = mine[x];
                let (lb, kb) = mine[y];
                let (na, nb) = (node(&mut uf, ka), node(&mut uf, kb));
                if na == nb {
                    continue;
                }
                let h = match (depth.get(la as usize), depth.get(lb as usize)) {
                    (Some(a), Some(b)) => (a.mil() - b.mil()).abs(),
                    _ => 10.0 * (lb as f64 - la as f64).abs(),
                };
                out.push(Edge {
                    a: na,
                    b: nb,
                    cost: h,
                    kind: Ek::Via {
                        via: *vi,
                        net: g.vias[*vi].net.unwrap_or(0),
                        la,
                        lb,
                    },
                });
            }
        }
    }
    let mut pad_node = HashMap::new();
    for &pi in &pads {
        let k = node(&mut uf, pad_pt[&pi]);
        pad_node.insert(pi, k);
    }
    // Parts between the nets: two-pin parts, and baluns from their
    // unbalanced pins to their balanced ones.
    let mut by_comp: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(c) = p.component {
            by_comp.entry(c as usize).or_default().push(i);
        }
    }
    for (comp, list) in &by_comp {
        let signal: Vec<usize> = list
            .iter()
            .copied()
            .filter(|&i| {
                g.pads[i]
                    .net
                    .is_some_and(|n| ground.get(n as usize) != Some(&true))
            })
            .collect();
        let inside: Vec<usize> = signal
            .iter()
            .copied()
            .filter(|&i| g.pads[i].net.is_some_and(|n| nets.contains(&n)))
            .collect();
        if inside.len() < 2 {
            continue;
        }
        let balun = list.len() >= 3 && is_balun(board, *comp);
        if list.len() == 2 || balun {
            for x in 0..inside.len() {
                for y in x + 1..inside.len() {
                    let (pa, pb) = (inside[x], inside[y]);
                    if g.pads[pa].net == g.pads[pb].net {
                        continue;
                    }
                    out.push(Edge {
                        a: pad_node[&pa],
                        b: pad_node[&pb],
                        cost: 20.0,
                        kind: Ek::Part {
                            comp: *comp,
                            pa,
                            pb,
                            balun,
                        },
                    });
                }
            }
        }
    }
    let mut adj = vec![Vec::new(); n];
    for (k, e) in out.iter().enumerate() {
        adj[e.a].push(k);
        adj[e.b].push(k);
    }
    Graph {
        n,
        adj,
        edges: out,
        pad_node,
    }
}

/// Shortest paths from `sources` along edges `ok` allows: distance and
/// the edge each node was reached by.
fn dijkstra(
    gr: &Graph,
    sources: &[usize],
    ok: &dyn Fn(&Edge) -> bool,
) -> (Vec<f64>, Vec<Option<usize>>) {
    let mut d = vec![f64::INFINITY; gr.n];
    let mut by = vec![None; gr.n];
    let mut heap = BinaryHeap::new();
    for &s in sources {
        d[s] = 0.0;
        heap.push(Reverse((0u64, s)));
    }
    while let Some(Reverse((k, u))) = heap.pop() {
        let du = k as f64 / 1000.0;
        if du > d[u] + 1e-9 {
            continue;
        }
        for &e in &gr.adj[u] {
            let ed = &gr.edges[e];
            if !ok(ed) {
                continue;
            }
            let v = if ed.a == u { ed.b } else { ed.a };
            let nd = du + ed.cost;
            if nd + 1e-9 < d[v] {
                d[v] = nd;
                by[v] = Some(e);
                heap.push(Reverse(((nd * 1000.0) as u64, v)));
            }
        }
    }
    (d, by)
}

/// Edges from the source set to `t`, in order, with the node each starts at.
fn walk_back(gr: &Graph, by: &[Option<usize>], t: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut u = t;
    let mut guard = 0;
    while let Some(e) = by[u] {
        let ed = &gr.edges[e];
        let prev = if ed.a == u { ed.b } else { ed.a };
        out.push((e, prev));
        u = prev;
        guard += 1;
        if guard > gr.n {
            break;
        }
    }
    out.reverse();
    out
}

// ---------------------------------------------------------------------
// Elements of the path.

#[derive(Clone, Debug)]
enum El {
    Line {
        net: u32,
        layer: u8,
        width: f64,
        /// Edge gap to the partner (a coupled pair run).
        gap: Option<f64>,
        /// Gaps to pour on each side, mil.
        sides: [Option<f64>; 2],
        len: f64,
        at: [f64; 2],
        tracks: Vec<usize>,
    },
    Via {
        via: usize,
        net: u32,
        from: u8,
        to: u8,
    },
    Part {
        comp: usize,
        pa: usize,
        pb: usize,
        balun: bool,
    },
    Shunt {
        comp: usize,
        pad: usize,
        /// Across the pair (P to N) rather than to ground.
        across: bool,
        stub: Vec<(El, bool)>,
    },
}

/// What one element is, once modelled, at every frequency.
#[derive(Clone)]
struct Built {
    sec: Section,
    /// ABCD per frequency (in the element's mode).
    m: Vec<Abcd>,
    /// Port references this element changes to (a balun) or None.
    mode_after: Option<bool>,
    /// An empty part site (TBD, DNI) a matching part may go in.
    site: Option<Site>,
}

/// An empty part site on the path.
#[derive(Clone, Debug)]
struct Site {
    /// In series with the line (else to ground, or across the pair).
    series: bool,
    across: bool,
    diff: bool,
    pack: Package,
    /// The designators of the site (one, or alternates on the same pads).
    names: Vec<String>,
    /// Capacitors, inductors, or either, may go here.
    caps: bool,
    inductors: bool,
    /// For a shunt site: the stub to it, per frequency.
    stub: Vec<Abcd>,
}

struct Ctx<'a> {
    board: &'a Board,
    stackup: &'a Stackup,
    geo: Vec<LayerGeometry>,
    opt: &'a Options,
    f: Vec<f64>,
    partner: HashMap<u32, u32>,
    ground: Vec<bool>,
    vias: HashMap<String, Result<fdtd::ViaResult, String>>,
    notes: Vec<String>,
    progress: &'a dyn Fn(f64, &str),
    via_kinds: usize,
    via_done: usize,
}

fn net_name(board: &Board, n: u32) -> String {
    board
        .nets
        .get(n as usize)
        .map_or(format!("net {n}"), |x| x.name.clone())
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

enum Lm {
    Single(field::Line),
    Coupled(field::Pair),
}

impl Ctx<'_> {
    fn z_ref(diff: bool) -> f64 {
        if diff {
            100.0
        } else {
            50.0
        }
    }

    fn td(&self, layer: u8) -> f64 {
        self.geo
            .get(layer as usize)
            .and_then(|g| g.tan_d)
            .unwrap_or(self.opt.tan_d)
    }

    /// Model a trace section.
    fn line(&mut self, el: &El, diff: bool) -> Option<Built> {
        let El::Line {
            net,
            layer,
            width,
            gap,
            sides,
            len,
            at,
            ..
        } = el
        else {
            return None;
        };
        let g = self.geo.get(*layer as usize)?.clone();
        let td = self.td(*layer);
        let rough = g.rough_um;
        let lm = match (diff, gap) {
            (true, Some(s)) => {
                let s = (s * 20.0).round().max(1.0) / 20.0;
                field::pair_with(&g, *width, s, field::both(*sides)).map(Lm::Coupled)
            }
            _ => field::single_sides(&g, *width, *sides).map(Lm::Single),
        };
        let Some(lm) = lm else {
            self.notes.push(format!(
                "{} on {}: no field solution (stack-up?)",
                net_name(self.board, *net),
                g.name
            ));
            return None;
        };
        let (zc, eeff) = match &lm {
            Lm::Single(l) => (if diff { 2.0 * l.z0 } else { l.z0 }, l.eeff),
            Lm::Coupled(p) => (p.zdiff, p.e_odd),
        };
        let alpha = |fh: f64| -> f64 {
            match &lm {
                Lm::Single(l) => l.db_per_in(fh, td, rough) / 8.686 / 0.0254,
                Lm::Coupled(p) => {
                    let k = rough.map_or(1.0, |r| crate::sim::loss::rough(fh, r));
                    let rac = p.r_odd_ghz * (fh / 1e9).sqrt() * k;
                    let r = rac.hypot(p.single.r_dc);
                    let tan = p.tan_odd + (p.single.fill - p.single.fill_given).max(0.0) * td;
                    r / (2.0 * p.z_odd) + std::f64::consts::PI * fh * p.e_odd.sqrt() / C0 * tan
                }
            }
        };
        let m: Vec<Abcd> = self
            .f
            .iter()
            .map(|&f| line(zc, eeff, *len, alpha(f * 1e9), f))
            .collect();
        let top = *self.f.last()?;
        let own = 8.686 * alpha(top * 1e9) * len * MIL;
        let what = match (diff, gap) {
            (true, Some(s)) => format!("{} {:.2}/{:.2} mil, {:.0} mil", g.name, width, s, len),
            (true, None) => format!("{} {:.2} mil uncoupled, {:.0} mil", g.name, width, len),
            _ => format!("{} {:.2} mil, {:.0} mil", g.name, width, len),
        };
        let side_txt = match sides {
            [Some(a), Some(b)] => format!(", pour {a:.1}/{b:.1} mil beside"),
            [Some(a), None] | [None, Some(a)] => format!(", pour {a:.1} mil on one side"),
            _ => String::new(),
        };
        Some(Built {
            sec: Section {
                what,
                kind: "trace",
                net: net_name(self.board, *net),
                length: *len,
                z: Some(zc),
                diff,
                loss_db: own,
                total_db: 0.0,
                at: *at,
                model: format!(
                    "2D field solver, Er eff {eeff:.2}, Df {td}{}{side_txt}",
                    rough.map_or(String::new(), |r| format!(", rough {r} µm"))
                ),
            },
            m,
            mode_after: None,
            site: None,
        })
    }

    fn via(&mut self, el: &El, diff: bool, quick_only: bool) -> Option<Built> {
        let El::Via { via, net, from, to } = el else {
            return None;
        };
        let board = self.board;
        let v = &board.geometry.vias[*via];
        let (lo, hi) = ((*from).min(*to), (*from).max(*to));
        let mate = if diff {
            self.partner
                .get(net)
                .and_then(|p| crate::zcheck::mate_via(board, v, *p))
        } else {
            None
        };
        let diff_via = diff && mate.is_some();
        if diff && mate.is_none() {
            self.notes.push(format!(
                "{}: via at ({:.0}, {:.0}) has no partner via beside it; taken single-ended",
                net_name(board, *net),
                v.at.x.mil(),
                v.at.y.mil()
            ));
        }
        let mut s = crate::zcheck::build_via(board, v, lo, hi, mate, &self.ground);
        // Widths of the traces on the two layers.
        let joined = crate::zcheck::joined_layers(board, v);
        let w_on = |l: u8| joined.iter().find(|j| j.0 == l).and_then(|j| j.1);
        s.w_from = w_on(lo);
        s.w_to = w_on(hi);
        // No stub when the via ends at the outgoing layer.
        s.backdrill = v.to_layer <= hi;
        s.fmax_ghz = self.opt.ghz.max(1.0);
        s.points = 120;
        let barrel = fdtd::barrel_mil(self.stackup, lo, hi).unwrap_or(0.0);
        let at = mil(&v.at);
        let zr = Self::z_ref(diff_via);
        let (er, df) = fdtd::materials_between(self.stackup, lo, hi).unwrap_or((4.0, 0.02));
        let quick = |f: f64| -> Abcd {
            // A coaxial line: barrel inside the antipad, then the stub
            // below as an open line.
            let z = 60.0 / er.sqrt() * (s.antipad / s.drill.max(1.0)).ln().max(0.1);
            let z = if diff_via { 2.0 * z } else { z };
            let alpha = std::f64::consts::PI * f * 1e9 * er.sqrt() / C0 * df;
            let mut m = line(z, er, barrel, alpha, f);
            let below = if s.backdrill {
                0.0
            } else {
                fdtd::barrel_mil(self.stackup, hi, v.to_layer).unwrap_or(0.0)
            };
            if below > 0.5 {
                m = mul(&m, &shunt(open_stub(z, er, below, alpha, f)));
            }
            m
        };
        let mut model = String::new();
        let m: Vec<Abcd> = if self.opt.vias_3d && !quick_only {
            let key = format!(
                "{} {} {} {} {} {} {:?} {} {} {:?} {:?} {}",
                s.from,
                s.to,
                (s.drill * 2.0).round(),
                (s.pad * 2.0).round(),
                (s.antipad * 2.0).round(),
                s.antipad_oval,
                s.pair.map(|p| (p * 2.0).round()),
                s.returns,
                (s.return_dist / 5.0).round(),
                s.w_from.map(|w| (w * 4.0).round()),
                s.w_to.map(|w| (w * 4.0).round()),
                s.backdrill
            );
            if !self.vias.contains_key(&key) {
                let what = format!(
                    "via {} of about {}",
                    self.via_done + 1,
                    self.via_kinds.max(1)
                );
                let (k, n) = (self.via_done as f64, self.via_kinds.max(1) as f64);
                let prog = self.progress;
                let r = fdtd::solve_with(self.stackup, &s, &|p| {
                    prog(0.1 + 0.8 * ((k + p) / n).min(1.0), &what)
                });
                self.via_done += 1;
                self.vias.insert(key.clone(), r);
            }
            match &self.vias[&key] {
                Ok(r) => {
                    model = format!(
                        "3D solver{}, drill {:.1} pad {:.1} antipad {:.1}{} mil, {} return via(s){}",
                        if diff_via { " (pair)" } else { "" },
                        s.drill,
                        s.pad,
                        s.antipad,
                        if s.antipad_oval { " oval" } else { "" },
                        s.returns,
                        r.z_via.map_or(String::new(), |z| format!(", Z via {z:.0} Ω"))
                    );
                    let zr = r.z_ref;
                    self.f
                        .iter()
                        .map(|&f| {
                            let k =
                                r.f.partition_point(|&x| x < f)
                                    .min(r.f.len().saturating_sub(1));
                            let (a, b) = if k == 0 || r.f.len() < 2 {
                                (r.s11[k], r.s21[k])
                            } else {
                                let t = ((f - r.f[k - 1]) / (r.f[k] - r.f[k - 1])).clamp(0.0, 1.0);
                                (
                                    r.s11[k - 1].scale(1.0 - t).add(r.s11[k].scale(t)),
                                    r.s21[k - 1].scale(1.0 - t).add(r.s21[k].scale(t)),
                                )
                            };
                            abcd_from_s(&[a, b, b, a], zr, zr)
                        })
                        .collect()
                }
                Err(e) => {
                    self.notes
                        .push(format!("via not solved in 3D ({e}); coaxial estimate used"));
                    self.f.iter().map(|&f| quick(f)).collect()
                }
            }
        } else {
            Vec::new()
        };
        let m = if m.is_empty() {
            let z = 60.0 / er.sqrt() * (s.antipad / s.drill.max(1.0)).ln().max(0.1);
            model = format!(
                "coaxial estimate, drill {:.1} antipad {:.1} mil, about {:.0} Ω",
                s.drill,
                s.antipad,
                if diff_via { 2.0 * z } else { z }
            );
            self.f.iter().map(|&f| quick(f)).collect()
        } else {
            m
        };
        // A pair via in a single-ended path, or the reverse, cannot be;
        // the via's own mode is the path's.
        let m: Vec<Abcd> = if diff && !diff_via {
            m.iter().map(to_diff).collect()
        } else {
            m
        };
        let top = m.last().copied().unwrap_or(IDENT);
        let own = -s_from_abcd(&top, zr, zr)[2].db();
        let names = |l: u8| {
            self.geo
                .get(l as usize)
                .map_or(format!("L{}", l + 1), |g| g.name.clone())
        };
        Some(Built {
            sec: Section {
                what: format!("via {} to {}", names(*from), names(*to)),
                kind: "via",
                net: net_name(board, *net),
                length: barrel,
                z: None,
                diff,
                loss_db: own,
                total_db: 0.0,
                at,
                model,
            },
            m,
            mode_after: None,
            site: None,
        })
    }

    fn part(&mut self, el: &El, diff: bool, next_diff: bool) -> Option<Built> {
        let El::Part {
            comp,
            pa,
            pb,
            balun,
        } = el
        else {
            return None;
        };
        let board = self.board;
        let g = &board.geometry;
        let des = board.components[*comp].designator.clone();
        let value = board.components[*comp]
            .param(&["Value", "Comment", "Device Type"])
            .unwrap_or("")
            .to_string();
        let at = mil(&g.pads[*pa].at);
        let mid = {
            let b = mil(&g.pads[*pb].at);
            [(at[0] + b[0]) / 2.0, (at[1] + b[1]) / 2.0]
        };
        if *balun && diff != next_diff {
            let pm = part_model(board, *comp, &[*pa, *pb], self.opt, 3)
                .unwrap_or(PartModel::Ideal { ratio: 2.0 });
            // From the unbalanced side (SE) to the balanced one, or back.
            let se_first = !diff;
            let m: Vec<Abcd> = self
                .f
                .iter()
                .map(|&f| match &pm {
                    PartModel::File(n) => {
                        let s = at_clamped(n, f).unwrap_or_default();
                        if s.len() < 9 {
                            return IDENT;
                        }
                        let q = |i: usize, j: usize| s[3 * i + j];
                        let r2 = std::f64::consts::FRAC_1_SQRT_2;
                        // Mixed mode: port 1 single-ended, 2 differential.
                        let s11 = q(0, 0);
                        let s1d = sub(q(0, 1), q(0, 2)).scale(r2);
                        let sd1 = sub(q(1, 0), q(2, 0)).scale(r2);
                        let sdd = sub(q(1, 1), q(1, 2)).add(sub(q(2, 2), q(2, 1))).scale(0.5);
                        let z = n.z_ref;
                        if se_first {
                            abcd_from_s(&[s11, s1d, sd1, sdd], z, 2.0 * z)
                        } else {
                            abcd_from_s(&[sdd, sd1, s1d, s11], 2.0 * z, z)
                        }
                    }
                    PartModel::Ideal { ratio } => {
                        let n_ = (1.0 / ratio).sqrt();
                        if se_first {
                            [c(n_), c(0.0), c(0.0), c(1.0 / n_)]
                        } else {
                            [c(1.0 / n_), c(0.0), c(0.0), c(n_)]
                        }
                    }
                    _ => IDENT,
                })
                .collect();
            if matches!(pm, PartModel::Ideal { .. }) {
                self.notes.push(format!(
                    "{des}: balun taken as ideal (no loss); give its .s3p (port 1 unbalanced, 2 and 3 balanced) for its real loss"
                ));
            }
            let top = m.last().copied().unwrap_or(IDENT);
            let (z1, z2) = (Self::z_ref(diff), Self::z_ref(next_diff));
            let own = (-s_from_abcd(&top, z1, z2)[2].db()).max(0.0);
            return Some(Built {
                sec: Section {
                    what: format!("{des} {value}"),
                    kind: "balun",
                    net: format!(
                        "{} > {}",
                        net_name(board, g.pads[*pa].net.unwrap_or(0)),
                        net_name(board, g.pads[*pb].net.unwrap_or(0))
                    ),
                    length: 0.0,
                    z: None,
                    diff,
                    loss_db: own,
                    total_db: 0.0,
                    at: mid,
                    model: pm.describe(),
                },
                m,
                mode_after: Some(next_diff),
                site: None,
            });
        }
        // A series part (in each line of a pair), with any other part on
        // the same two nets beside it (alternate footprints: a DC block
        // beside a TBD inductor).
        let (na, nb) = (g.pads[*pa].net, g.pads[*pb].net);
        let mut group: Vec<(usize, usize, usize)> = vec![(*comp, *pa, *pb)];
        for (ci, _) in board.components.iter().enumerate() {
            if ci == *comp {
                continue;
            }
            let pads: Vec<usize> = (0..g.pads.len())
                .filter(|&i| g.pads[i].component == Some(ci as u32))
                .collect();
            if pads.len() != 2 {
                continue;
            }
            let (x, y) = (pads[0], pads[1]);
            if g.pads[x].net == na && g.pads[y].net == nb {
                group.push((ci, x, y));
            } else if g.pads[x].net == nb && g.pads[y].net == na {
                group.push((ci, y, x));
            }
        }
        let mut used: Vec<(usize, usize, usize, PartModel)> = Vec::new();
        let mut unfitted: Vec<String> = Vec::new();
        for &(ci, x, y) in &group {
            let pm = part_model(board, ci, &[x, y], self.opt, 2);
            let pm = match pm {
                Some(PartModel::File(n)) if n.ports != 2 => None,
                other => other,
            };
            let pc = &board.components[ci];
            match pm {
                Some(pm) => used.push((ci, x, y, pm)),
                None if freed(self.opt, &pc.designator) => {}
                None => unfitted.push(format!(
                    "{} {}",
                    pc.designator,
                    pc.param(&["Value", "Comment", "Device Type"])
                        .unwrap_or("(no value)")
                )),
            }
        }
        // Footprints stacked on the same pads are alternatives: only one
        // is fitted. Take the one --fit names, else a capacitor, then an
        // inductor, then a resistor.
        let stacked = |a: &(usize, usize, usize, PartModel),
                       b: &(usize, usize, usize, PartModel)| {
            let near = |x: usize, y: usize| {
                let (p, q) = (&g.pads[x], &g.pads[y]);
                let r = 0.5
                    * p.size_x
                        .mil()
                        .max(p.size_y.mil())
                        .max(q.size_x.mil())
                        .max(q.size_y.mil());
                p.layer == q.layer && dist(mil(&p.at), mil(&q.at)) < r
            };
            near(a.1, b.1) && near(a.2, b.2)
        };
        let rank = |u: &(usize, usize, usize, PartModel)| {
            let d = board.components[u.0].designator.to_ascii_uppercase();
            let forced = self
                .opt
                .fit
                .iter()
                .any(|p| crate::constraints::glob(&p.to_ascii_uppercase(), &d));
            let kind = match prefix(&d).as_str() {
                "C" => 0,
                "L" => 1,
                "R" => 2,
                _ => 3,
            };
            (!forced, kind)
        };
        let mut kept: Vec<(usize, usize, usize, PartModel)> = Vec::new();
        let mut left: Vec<(usize, usize, usize, PartModel)> = used;
        left.sort_by_key(|u| rank(u));
        for u in left {
            if let Some(k) = kept.iter().position(|k| stacked(k, &u)) {
                self.notes.push(format!(
                    "{} and {} are alternative footprints on the same pads: took {} (--fit to choose)",
                    board.components[kept[k].0].designator,
                    board.components[u.0].designator,
                    board.components[kept[k].0].designator
                ));
                continue;
            }
            kept.push(u);
        }
        let used = kept;
        if !unfitted.is_empty() {
            self.notes.push(format!(
                "not fitted or no value, left out: {}{}",
                unfitted.join(", "),
                if used.is_empty() {
                    "; nothing else joins these nets, so taken as a 0 Ω link"
                } else {
                    ""
                }
            ));
        }
        let m: Vec<Abcd> = self
            .f
            .iter()
            .map(|&f| {
                // Parts side by side: their Y matrices add.
                let mut y = [c(0.0); 4];
                for (_, x, _, pm) in &used {
                    let m = match pm {
                        PartModel::File(n) => {
                            let s = at_clamped(n, f).unwrap_or_default();
                            // Port 1 of a vendor file is the part's pad "1".
                            let flip = g.pads[*x].name != "1";
                            if s.len() < 4 {
                                IDENT
                            } else if flip {
                                abcd_from_s(&[s[3], s[2], s[1], s[0]], n.z_ref, n.z_ref)
                            } else {
                                abcd_from_s(&[s[0], s[1], s[2], s[3]], n.z_ref, n.z_ref)
                            }
                        }
                        other => series(other.z(f).unwrap_or(c(0.0))),
                    };
                    if used.len() == 1 {
                        return m;
                    }
                    let yb = c(1.0).div(m[1]);
                    let det = sub(m[0].mul(m[3]), m[1].mul(m[2]));
                    let my = [
                        m[3].mul(yb),
                        det.mul(yb).scale(-1.0),
                        yb.scale(-1.0),
                        m[0].mul(yb),
                    ];
                    for k in 0..4 {
                        y[k] = y[k].add(my[k]);
                    }
                }
                if used.is_empty() {
                    return series(c(0.0));
                }
                // Back to ABCD.
                let inv = c(-1.0).div(y[2]);
                let dy = sub(y[0].mul(y[3]), y[1].mul(y[2]));
                [y[3].mul(inv), inv, dy.mul(inv), y[0].mul(inv)]
            })
            .map(|m| if diff { to_diff(&m) } else { m })
            .collect();
        let zr = Self::z_ref(diff);
        let top = m.last().copied().unwrap_or(IDENT);
        let own = -s_from_abcd(&top, zr, zr)[2].db();
        let name = |ci: usize| {
            let pc = &board.components[ci];
            format!(
                "{} {}",
                pc.designator,
                pc.param(&["Value", "Comment", "Device Type"]).unwrap_or("")
            )
        };
        let site = used.is_empty().then(|| {
            let names: Vec<String> = group
                .iter()
                .map(|u| board.components[u.0].designator.clone())
                .collect();
            let pre: Vec<String> = names.iter().map(|n| prefix(n)).collect();
            let (x, y) = (&g.pads[*pa].at, &g.pads[*pb].at);
            let pitch = (x.x.mil() - y.x.mil()).hypot(x.y.mil() - y.y.mil());
            Site {
                series: true,
                across: false,
                diff,
                pack: Package::from_pitch(pitch),
                caps: self.opt.any_part || pre.iter().any(|p| p != "L"),
                inductors: self.opt.any_part || pre.iter().any(|p| p != "C"),
                names,
                stub: Vec::new(),
            }
        });
        let what = if used.is_empty() {
            format!("{des} {value} (no value: 0 Ω link)")
        } else {
            used.iter()
                .map(|u| name(u.0))
                .collect::<Vec<_>>()
                .join(" ‖ ")
        };
        let model = if used.is_empty() {
            "not fitted or no value".to_string()
        } else {
            used.iter()
                .map(|u| u.3.describe())
                .collect::<Vec<_>>()
                .join(" ‖ ")
        };
        Some(Built {
            sec: Section {
                what,
                kind: "part",
                net: format!(
                    "{} > {}",
                    net_name(board, na.unwrap_or(0)),
                    net_name(board, nb.unwrap_or(0))
                ),
                length: 0.0,
                z: None,
                diff,
                loss_db: own,
                total_db: 0.0,
                at: mid,
                model,
            },
            m,
            mode_after: None,
            site,
        })
    }

    fn shunt(&mut self, el: &El, diff: bool) -> Option<Built> {
        let El::Shunt {
            comp,
            pad,
            across,
            stub,
        } = el
        else {
            return None;
        };
        let board = self.board;
        let g = &board.geometry;
        let des = board.components[*comp].designator.clone();
        let value = board.components[*comp]
            .param(&["Value", "Comment", "Device Type"])
            .unwrap_or("")
            .to_string();
        let pads: Vec<usize> = g
            .pads
            .iter()
            .enumerate()
            .filter(|(_, p)| p.component == Some(*comp as u32))
            .map(|(i, _)| i)
            .collect();
        let pm: Option<PartModel> = match part_model(board, *comp, &pads, self.opt, 1) {
            Some(PartModel::File(n)) => Some(PartModel::File(n)),
            _ => match part_model(board, *comp, &pads, self.opt, 2) {
                Some(PartModel::File(n)) if n.ports != 1 => return None,
                Some(pm) => Some(pm),
                None if freed(self.opt, &des) => None,
                None => {
                    self.notes.push(format!(
                        "{des} {} to {}: not fitted or no value, left off (its pad and stub stay)",
                        if value.is_empty() {
                            "(no value)"
                        } else {
                            value.as_str()
                        },
                        if *across { "the other line" } else { "ground" }
                    ));
                    None
                }
            },
        };
        let mut built: Vec<Built> = Vec::new();
        let mut stub_len = 0.0;
        for (e, d) in stub {
            let b = match e {
                El::Line { len, .. } => {
                    stub_len += len;
                    self.line(e, *d)
                }
                // Stubs take the quick via model.
                El::Via { .. } => self.via(e, *d, true),
                _ => None,
            };
            if let Some(b) = b {
                built.push(b);
            }
        }
        let chains: Vec<Abcd> = (0..self.f.len())
            .map(|k| {
                let mut chain = IDENT;
                for b in &built {
                    chain = mul(&chain, &b.m[k]);
                }
                chain
            })
            .collect();
        let m: Vec<Abcd> = (0..self.f.len())
            .map(|k| {
                let f = self.f[k];
                let chain = chains[k];
                let zl = pm
                    .as_ref()
                    .and_then(|pm| pm.z(f))
                    .filter(|z| z.re.is_finite() && z.im.is_finite())
                    .unwrap_or(Cx::new(OPEN_Z, 0.0));
                // To ground from one line of a pair: twice its impedance
                // for the differential mode; across the pair: itself.
                let zl = if diff && !*across { zl.scale(2.0) } else { zl };
                let zin = z_in(&chain, zl);
                shunt(c(1.0).div(zin))
            })
            .collect();
        let zr = Self::z_ref(diff);
        let top = m.last().copied().unwrap_or(IDENT);
        let own = -s_from_abcd(&top, zr, zr)[2].db();
        let at = mil(&g.pads[*pad].at);
        let site = pm.is_none().then(|| {
            let pitch = if pads.len() >= 2 {
                let (x, y) = (&g.pads[pads[0]].at, &g.pads[pads[1]].at);
                (x.x.mil() - y.x.mil()).hypot(x.y.mil() - y.y.mil())
            } else {
                40.0
            };
            let pre = prefix(&des);
            Site {
                series: false,
                across: *across,
                diff,
                pack: Package::from_pitch(pitch),
                caps: self.opt.any_part || pre != "L",
                inductors: self.opt.any_part || pre != "C",
                names: vec![des.clone()],
                stub: chains,
            }
        });
        Some(Built {
            sec: Section {
                what: format!(
                    "{des} {value} {}{}",
                    if *across {
                        "across the pair"
                    } else {
                        "to ground"
                    },
                    if stub_len > 0.5 {
                        format!(", {stub_len:.0} mil stub")
                    } else {
                        String::new()
                    }
                ),
                kind: "shunt",
                net: net_name(board, g.pads[*pad].net.unwrap_or(0)),
                length: stub_len,
                z: None,
                diff,
                loss_db: own,
                total_db: 0.0,
                at,
                model: pm
                    .as_ref()
                    .map_or("not fitted: an open pad".to_string(), |p| p.describe()),
            },
            m,
            mode_after: None,
            site,
        })
    }
}

/// Nets that continue a pair through matching two-pin parts on both its
/// lines (AC capacitors, series resistors), up to a balun or a chip, are
/// a pair too: the one on the positive line's side positive. Returns how
/// many it added.
fn implicit_pairs(
    board: &Board,
    ground: &[bool],
    pads_per_net: &[usize],
    partner: &mut HashMap<u32, u32>,
    positive: &mut HashSet<u32>,
) -> usize {
    let g = &board.geometry;
    let mut by_comp: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(c) = p.component {
            by_comp.entry(c).or_default().push(i);
        }
    }
    // Net -> (part prefix, net on its other pad).
    let mut via_part: HashMap<u32, Vec<(String, u32)>> = HashMap::new();
    for (c, list) in &by_comp {
        if list.len() != 2 {
            continue;
        }
        let (Some(a), Some(b)) = (g.pads[list[0]].net, g.pads[list[1]].net) else {
            continue;
        };
        let ok = |n: u32| {
            ground.get(n as usize) != Some(&true)
                && pads_per_net.get(n as usize).copied().unwrap_or(0) <= MAX_PADS
        };
        if a == b || !ok(a) || !ok(b) {
            continue;
        }
        let pre = prefix(&board.components[*c as usize].designator);
        via_part.entry(a).or_default().push((pre.clone(), b));
        via_part.entry(b).or_default().push((pre, a));
    }
    let mut added = 0;
    for _round in 0..4 {
        let pairs: Vec<(u32, u32)> = positive
            .iter()
            .filter_map(|&p| partner.get(&p).map(|&n| (p, n)))
            .collect();
        let mut new: Vec<(u32, u32)> = Vec::new();
        for (p, n) in pairs {
            let (Some(ps), Some(ns)) = (via_part.get(&p), via_part.get(&n)) else {
                continue;
            };
            for (pre, a) in ps {
                let found: Vec<u32> = ns
                    .iter()
                    .filter(|(q, b)| q == pre && b != a && *b != p && *a != n)
                    .map(|x| x.1)
                    .filter(|b| !partner.contains_key(b))
                    .collect();
                if found.len() == 1 && !partner.contains_key(a) {
                    new.push((*a, found[0]));
                }
            }
        }
        if new.is_empty() {
            break;
        }
        for (a, b) in new {
            if partner.contains_key(&a) || partner.contains_key(&b) {
                continue;
            }
            partner.insert(a, b);
            partner.insert(b, a);
            positive.insert(a);
            added += 1;
        }
    }
    added
}

/// A pin from its name: "J503.1", "U101.A5", "J503-1", or a part with one
/// signal pin.
fn find_pins(board: &Board, spec: &str, ground: &[bool], one: bool) -> Result<Vec<usize>, String> {
    let spec = spec.trim();
    let g = &board.geometry;
    let split = spec
        .rsplit_once('.')
        .or_else(|| spec.rsplit_once('-'))
        .or_else(|| spec.rsplit_once(':'));
    let find_comp = |d: &str| {
        board
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
    };
    let (comp, pin) = match split.and_then(|(d, p)| find_comp(d).map(|c| (c, Some(p)))) {
        Some(x) => x,
        None => match find_comp(spec) {
            Some(c) => (c, None),
            None => {
                // A net name: its pads.
                if let Some(n) = board
                    .nets
                    .iter()
                    .position(|n| n.name.eq_ignore_ascii_case(spec))
                {
                    let mut pads: Vec<usize> = (0..g.pads.len())
                        .filter(|&i| g.pads[i].net == Some(n as u32))
                        .collect();
                    if pads.is_empty() {
                        return Err(format!("net {spec} has no pads"));
                    }
                    if one {
                        // Start at a connector's pin, else a chip's.
                        let rank = |i: &usize| {
                            let c = g.pads[*i]
                                .component
                                .and_then(|c| board.components.get(c as usize));
                            let pre = c.map_or(String::new(), |c| prefix(&c.designator));
                            let pins = g
                                .pads
                                .iter()
                                .filter(|p| {
                                    p.component.is_some() && p.component == g.pads[*i].component
                                })
                                .count();
                            let conn =
                                matches!(pre.as_str(), "J" | "P" | "X" | "CN" | "CON" | "SMA");
                            (Reverse(conn), Reverse(pins >= 4), *i)
                        };
                        pads.sort_by_key(rank);
                        pads.truncate(1);
                    }
                    return Ok(pads);
                }
                return Err(format!("no part, pin or net named {spec}"));
            }
        },
    };
    let pads: Vec<usize> = (0..g.pads.len())
        .filter(|&i| {
            let p = &g.pads[i];
            p.component == Some(comp as u32)
                && pin.is_none_or(|n| p.name.eq_ignore_ascii_case(n))
                && p.net.is_some_and(|n| ground.get(n as usize) != Some(&true))
        })
        .collect();
    if pads.is_empty() {
        return Err(match pin {
            Some(p) => format!(
                "{} has no signal pin {p}",
                board.components[comp].designator
            ),
            None => format!("{} has no signal pins", board.components[comp].designator),
        });
    }
    Ok(pads)
}

fn pin_name(board: &Board, pad: usize) -> String {
    let p = &board.geometry.pads[pad];
    let des = p
        .component
        .and_then(|c| board.components.get(c as usize))
        .map_or("?".to_string(), |c| c.designator.clone());
    format!("{des}.{}", p.name)
}

/// Finds the routed path and cascades it.
pub fn run(
    board: &Board,
    stackup: &Stackup,
    opt: &Options,
    progress: &dyn Fn(f64, &str),
) -> Result<Report, String> {
    let p = prepare(board, stackup, opt, progress)?;
    progress(0.95, "cascading");
    let r = finish(&p, &p.built);
    progress(1.0, "done");
    Ok(r)
}

/// Finds the routed path and models each section.
fn prepare(
    board: &Board,
    stackup: &Stackup,
    opt: &Options,
    progress: &dyn Fn(f64, &str),
) -> Result<Prepared, String> {
    progress(0.0, "finding the path");
    let geo = crate::impedance::layer_geometry(stackup);
    if geo.iter().all(|g| g.above.is_none() && g.below.is_none()) {
        return Err(
            "the stack-up has no dielectric thicknesses; give one (Impedance window, or --stackup file)".into(),
        );
    }
    let (own, by_name) = crate::zcheck::with_name_pairs(board, &crate::zcheck::Goal::default());
    let board = own.as_ref().unwrap_or(board);
    let ground: Vec<bool> = board
        .nets
        .iter()
        .map(|n| crate::zcheck::is_ground(&n.name))
        .collect();
    let mut partner = board.pair_partners();
    let mut positive: HashSet<u32> = board
        .diff_pairs
        .iter()
        .filter_map(|p| {
            board
                .nets
                .iter()
                .position(|n| n.name == p.positive)
                .map(|i| i as u32)
        })
        .collect();
    let g = &board.geometry;
    let mut pads_per_net = vec![0usize; board.nets.len()];
    for p in &g.pads {
        if let Some(n) = p.net {
            if let Some(c) = pads_per_net.get_mut(n as usize) {
                *c += 1;
            }
        }
    }
    let implied = implicit_pairs(board, &ground, &pads_per_net, &mut partner, &mut positive);
    let followable = |n: u32| {
        ground.get(n as usize) != Some(&true)
            && pads_per_net.get(n as usize).copied().unwrap_or(0) <= MAX_PADS
            && pdn::volts_in_name(&net_name(board, n)).is_none()
    };
    // The positive line of a pair stands for both; a pin on the negative
    // one moves to the same part's pin on the positive.
    let to_positive = |pads: Vec<usize>| -> Vec<usize> {
        pads.into_iter()
            .map(|i| {
                let p = &g.pads[i];
                let Some(n) = p.net else { return i };
                match partner.get(&n) {
                    Some(&pn) if !positive.contains(&n) => (0..g.pads.len())
                        .find(|&j| {
                            g.pads[j].component == p.component
                                && g.pads[j].net == Some(pn)
                                && p.component.is_some()
                        })
                        .unwrap_or(i),
                    _ => i,
                }
            })
            .collect()
    };
    let starts = to_positive(find_pins(board, &opt.from, &ground, true)?);
    let ends = match &opt.to {
        Some(t) => Some(to_positive(find_pins(board, t, &ground, false)?)),
        None => None,
    };
    // Nets the path may follow: from the start's, through parts.
    let negatives: HashSet<u32> = partner
        .keys()
        .copied()
        .filter(|n| !positive.contains(n))
        .collect();
    let mut nets: HashSet<u32> = HashSet::new();
    let mut todo: Vec<u32> = starts.iter().filter_map(|&i| g.pads[i].net).collect();
    if let Some(e) = &ends {
        todo.extend(e.iter().filter_map(|&i| g.pads[i].net));
    }
    let mut by_comp: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(c) = p.component {
            by_comp.entry(c).or_default().push(i);
        }
    }
    while let Some(n) = todo.pop() {
        if nets.len() >= 24 || !nets.insert(n) {
            continue;
        }
        for p in g.pads.iter().filter(|p| p.net == Some(n)) {
            let Some(c) = p.component else { continue };
            let list = &by_comp[&c];
            let passes = list.len() == 2 || (list.len() >= 3 && is_balun(board, c as usize));
            if !passes {
                continue;
            }
            for &q in list {
                if let Some(m) = g.pads[q].net {
                    if m != n && followable(m) && !negatives.contains(&m) && !nets.contains(&m) {
                        todo.push(m);
                    }
                }
            }
        }
    }
    let gr = build(board, &nets, &ground);
    let src: Vec<usize> = starts
        .iter()
        .filter_map(|i| gr.pad_node.get(i).copied())
        .collect();
    if src.is_empty() {
        return Err("the start pin has no copper in the graph".into());
    }
    let (d, by) = dijkstra(&gr, &src, &|_| true);
    let start_comp = g.pads[starts[0]].component;
    let end_pad = match &ends {
        Some(e) => e
            .iter()
            .filter_map(|&i| gr.pad_node.get(&i).map(|&k| (i, d[k])))
            .filter(|x| x.1.is_finite())
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
            .ok_or_else(|| {
                format!(
                    "no routed path from {} to {}",
                    opt.from,
                    opt.to.as_deref().unwrap_or("")
                )
            })?,
        None => {
            // The farthest pin reached, a chip's (4 pins or more) first.
            let mut best: Option<(bool, f64, usize)> = None;
            for (&pi, &k) in &gr.pad_node {
                let p = &g.pads[pi];
                if !d[k].is_finite() || p.component == start_comp {
                    continue;
                }
                let chip = p.component.and_then(|c| by_comp.get(&c)).is_some_and(|l| {
                    l.len() >= 4 && !is_balun(board, p.component.unwrap_or(0) as usize)
                });
                let cand = (chip, d[k], pi);
                if best.is_none_or(|b| (cand.0, cand.1) > (b.0, b.1)) {
                    best = Some(cand);
                }
            }
            best.map(|b| b.2)
                .ok_or("the start pin has no routed copper".to_string())?
        }
    };
    let route = walk_back(&gr, &by, gr.pad_node[&end_pad]);
    if route.is_empty() {
        return Err("start and end are the same copper".into());
    }
    let start_node = route[0].1;
    let start_pad = starts
        .iter()
        .copied()
        .find(|i| gr.pad_node.get(i) == Some(&start_node))
        .unwrap_or(starts[0]);
    let mut rep = Report {
        start: pin_name(board, start_pad),
        end: pin_name(board, end_pad),
        ..Default::default()
    };
    if !by_name.is_empty() {
        rep.notes.push(format!(
            "{} pair(s) found by name (the board defines none for them)",
            by_name.len()
        ));
    }
    if implied > 0 {
        rep.notes.push(format!(
            "{implied} pair(s) of unnamed nets taken as pairs: they follow a pair through matching parts on both lines"
        ));
    }
    let paired = |n: u32| partner.contains_key(&n) && positive.contains(&n);
    // Elements in order, each with its mode.
    let route_nodes: Vec<usize> = std::iter::once(start_node)
        .chain(route.iter().map(|(e, from)| {
            let ed = &gr.edges[*e];
            if ed.a == *from {
                ed.b
            } else {
                ed.a
            }
        }))
        .collect();
    let mut diff = g.pads[start_pad].net.is_some_and(paired);
    let mut items: Vec<(El, bool)> = Vec::new();
    let side = SideGround::new(board);
    let mut mode_notes: Vec<String> = Vec::new();
    let mut used_parts: HashSet<usize> = HashSet::new();
    let to_el = |e: &Edge, from: usize, diff: bool, side: &SideGround| -> Option<El> {
        match &e.kind {
            Ek::Track {
                track,
                net,
                layer,
                width,
                len,
                a,
                b,
            } => {
                let t = &g.tracks[*track];
                let gap = if diff {
                    partner.get(net).and_then(|p| board.pair_gap(t, *p))
                } else {
                    None
                };
                let sides = geo
                    .get(*layer as usize)
                    .map_or([None, None], |gg| side.sides(t, side_reach(gg)));
                Some(El::Line {
                    net: *net,
                    layer: *layer,
                    width: *width,
                    gap,
                    sides,
                    len: *len,
                    at: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
                    tracks: vec![*track],
                })
            }
            Ek::Arc {
                net,
                layer,
                width,
                len,
                mid,
            } => Some(El::Line {
                net: *net,
                layer: *layer,
                width: *width,
                gap: None,
                sides: [None, None],
                len: *len,
                at: *mid,
                tracks: Vec::new(),
            }),
            Ek::Via { via, net, la, lb } => {
                let (f, t) = if gr.edges.is_empty() || e.a == from {
                    (*la, *lb)
                } else {
                    (*lb, *la)
                };
                let _ = from;
                Some(El::Via {
                    via: *via,
                    net: *net,
                    from: f,
                    to: t,
                })
            }
            Ek::Part {
                comp,
                pa,
                pb,
                balun,
            } => {
                let (pa, pb) = if gr.pad_node.get(pa) == Some(&from) {
                    (*pa, *pb)
                } else {
                    (*pb, *pa)
                };
                Some(El::Part {
                    comp: *comp,
                    pa,
                    pb,
                    balun: *balun,
                })
            }
        }
    };
    // Via direction: the edge's own a/b is not the layer order, so set
    // it from the node the walk enters.
    let mut node_items: Vec<usize> = vec![0]; // items before route node k
    for (k, (e, from)) in route.iter().enumerate() {
        let ed = &gr.edges[*e];
        let mut el = to_el(ed, *from, diff, &side);
        if let (Some(El::Via { from: f, to: t, .. }), Ek::Via { la, lb, .. }) = (&mut el, &ed.kind)
        {
            // The via edge runs la..lb between its layer nodes; find which
            // end the walk entered by the layer of the previous element.
            let prev_layer = items.iter().rev().find_map(|(x, _)| match x {
                El::Line { layer, .. } => Some(*layer),
                El::Via { to, .. } => Some(*to),
                _ => None,
            });
            let next_layer = route[k + 1..]
                .iter()
                .find_map(|(e2, _)| match &gr.edges[*e2].kind {
                    Ek::Track { layer, .. } | Ek::Arc { layer, .. } => Some(*layer),
                    Ek::Via { .. } => None,
                    Ek::Part { .. } => None,
                });
            let (a, b) = match (prev_layer, next_layer) {
                (Some(p), _) if p == *lb => (*lb, *la),
                (Some(p), _) if p == *la => (*la, *lb),
                (_, Some(nx)) if nx == *la => (*lb, *la),
                _ => (*la, *lb),
            };
            *f = a;
            *t = b;
        }
        let mut next_diff = diff;
        if let Ek::Part {
            pb,
            balun,
            comp,
            pa,
        } = &ed.kind
        {
            let to_pad = if gr.pad_node.get(pa) == Some(from) {
                *pb
            } else {
                *pa
            };
            let nn = g.pads[to_pad].net.unwrap_or(0);
            used_parts.insert(*comp);
            next_diff = if *balun {
                paired(nn)
            } else {
                let p = paired(nn);
                if p && !diff {
                    mode_notes.push(format!(
                        "{} is a pair's line, reached single-ended through {}: taken single-ended",
                        net_name(board, nn),
                        board.components[*comp].designator
                    ));
                }
                p && diff
            };
        }
        if let Some(el) = el {
            items.push((el, diff));
        }
        diff = next_diff;
        node_items.push(items.len());
    }
    rep.notes.extend(mode_notes);
    // Shunt parts off the path: two-pin parts with a pad on a net of the
    // path and the other to ground (or a rail, AC ground), or across the
    // pair.
    let route_nets: HashSet<u32> = route
        .iter()
        .filter_map(|(e, _)| match &gr.edges[*e].kind {
            Ek::Track { net, .. } | Ek::Arc { net, .. } | Ek::Via { net, .. } => Some(*net),
            Ek::Part { .. } => None,
        })
        .chain(g.pads[start_pad].net)
        .chain(g.pads[end_pad].net)
        .collect();
    let on_route: HashMap<usize, usize> = route_nodes
        .iter()
        .enumerate()
        .map(|(k, &n)| (n, k))
        .collect();
    let mut inserts: Vec<(usize, El)> = Vec::new(); // (route node index, shunt)
    let route_pads: HashSet<usize> = [start_pad, end_pad].into_iter().collect();
    for (&c, list) in &by_comp {
        if list.len() != 2 || used_parts.contains(&(c as usize)) {
            continue;
        }
        for (x, y) in [(list[0], list[1]), (list[1], list[0])] {
            let (Some(na), nb) = (g.pads[x].net, g.pads[y].net) else {
                continue;
            };
            if !route_nets.contains(&na) || route_pads.contains(&x) {
                continue;
            }
            let across = nb.is_some_and(|b| partner.get(&na) == Some(&b));
            let to_gnd =
                nb.is_none_or(|b| !nets.contains(&b) || ground.get(b as usize) == Some(&true));
            if !(across || to_gnd) {
                continue;
            }
            let Some(&pn) = gr.pad_node.get(&x) else {
                continue;
            };
            // The stub from the route to the part's pad.
            let srcs: Vec<usize> = route_nodes.clone();
            let (sd, sby) = dijkstra(&gr, &srcs, &|e: &Edge| match &e.kind {
                Ek::Track { net, .. } | Ek::Arc { net, .. } | Ek::Via { net, .. } => *net == na,
                Ek::Part { .. } => false,
            });
            if !sd[pn].is_finite() {
                continue;
            }
            let path = walk_back(&gr, &sby, pn);
            let attach = path.first().map_or(pn, |p| p.1);
            let Some(&k) = on_route.get(&attach) else {
                continue;
            };
            // The mode at that point of the route.
            let mode = items
                .get(node_items[k].saturating_sub(1))
                .map_or(diff, |x| x.1);
            let stub: Vec<(El, bool)> = path
                .iter()
                .filter_map(|(e, from)| {
                    to_el(&gr.edges[*e], *from, mode, &side).map(|el| (el, mode))
                })
                .collect();
            inserts.push((
                k,
                El::Shunt {
                    comp: c as usize,
                    pad: x,
                    across,
                    stub,
                },
            ));
            break;
        }
    }
    // Put shunts in place (after the items up to their route node).
    inserts.sort_by_key(|x| Reverse(x.0));
    for (k, el) in inserts {
        let at = node_items[k];
        let mode = if at == 0 {
            items.first().is_some_and(|x| x.1)
        } else {
            items[at - 1].1
        };
        let mode = match (&el, at) {
            (_, 0) => items.first().map_or(mode, |x| x.1),
            _ => mode,
        };
        items.insert(at, (el, mode));
    }
    // Merge trace pieces into sections: same layer, width, coupling.
    let mut merged: Vec<(El, bool)> = Vec::new();
    for (el, d) in items {
        // One via, crossed layer by layer: one section.
        if let (
            Some((
                El::Via {
                    via: v0, to: t0, ..
                },
                d0,
            )),
            El::Via { via, from, to, .. },
        ) = (merged.last_mut(), &el)
        {
            if *v0 == *via && *t0 == *from && *d0 == d {
                *t0 = *to;
                continue;
            }
        }
        if let (
            Some((
                El::Line {
                    net: n0,
                    layer: l0,
                    width: w0,
                    gap: g0,
                    len: len0,
                    tracks: t0,
                    sides: s0,
                    ..
                },
                d0,
            )),
            El::Line {
                net,
                layer,
                width,
                gap,
                len,
                tracks,
                sides,
                ..
            },
        ) = (merged.last_mut(), &el)
        {
            let same_gap = match (*g0, *gap) {
                (Some(a), Some(b)) => (a - b).abs() < 1.0,
                (None, None) => true,
                // An arc in a pair run keeps the run's gap.
                (Some(_), None) => tracks.is_empty(),
                (None, Some(_)) => false,
            };
            if *d0 == d && *n0 == *net && *l0 == *layer && (*w0 - *width).abs() < 0.05 && same_gap {
                // Length-weighted: keep the longer piece's gap and sides.
                if *len > *len0 {
                    *s0 = *sides;
                    if gap.is_some() {
                        *g0 = *gap;
                    }
                }
                *len0 += len;
                t0.extend(tracks.iter().copied());
                continue;
            }
        }
        merged.push((el, d));
    }
    // Typical gaps over each merged run.
    for (el, _) in merged.iter_mut() {
        if let El::Line {
            tracks,
            gap,
            sides,
            net,
            layer,
            ..
        } = el
        {
            if tracks.len() > 1 {
                if gap.is_some() {
                    if let Some(p) = partner.get(net) {
                        *gap = median(
                            tracks
                                .iter()
                                .filter_map(|&t| board.pair_gap(&g.tracks[t], *p))
                                .collect(),
                        )
                        .or(*gap);
                    }
                }
                if let Some(gg) = geo.get(*layer as usize) {
                    let all: Vec<[Option<f64>; 2]> = tracks
                        .iter()
                        .map(|&t| side.sides(&g.tracks[t], side_reach(gg)))
                        .collect();
                    for s in 0..2 {
                        let have: Vec<f64> = all.iter().filter_map(|x| x[s]).collect();
                        sides[s] = if have.len() * 2 >= all.len() {
                            median(have)
                        } else {
                            None
                        };
                    }
                }
            }
        }
    }
    // Frequencies.
    let n = opt.points.max(2);
    let f: Vec<f64> = (1..=n).map(|k| opt.ghz * k as f64 / n as f64).collect();
    let via_kinds = merged
        .iter()
        .filter(|x| matches!(x.0, El::Via { .. }))
        .count();
    let mut ctx = Ctx {
        board,
        stackup,
        geo,
        opt,
        f: f.clone(),
        partner: partner.clone(),
        ground: ground.clone(),
        vias: HashMap::new(),
        notes: Vec::new(),
        progress,
        via_kinds,
        via_done: 0,
    };
    let mut built: Vec<Built> = Vec::new();
    for (i, (el, d)) in merged.iter().enumerate() {
        let next = merged.get(i + 1).map_or(*d, |x| x.1);
        let b = match el {
            El::Line { .. } => ctx.line(el, *d),
            El::Via { .. } => ctx.via(el, *d, false),
            El::Part { .. } => ctx.part(el, *d, next),
            El::Shunt { .. } => ctx.shunt(el, *d),
        };
        if let Some(mut b) = b {
            if b.mode_after.is_none() && next != *d {
                b.mode_after = Some(next);
            }
            built.push(b);
        }
    }
    rep.notes.append(&mut ctx.notes);
    for (el, d) in &merged {
        if let El::Line { net, .. } = el {
            let v = (*net, if *d { partner.get(net).copied() } else { None });
            if !rep.victims.contains(&v) {
                rep.victims.push(v);
            }
        }
    }
    let start_diff = merged.first().is_some_and(|x| x.1);
    rep.f = f;
    rep.fmin = opt.fmin;
    Ok(Prepared {
        rep,
        built,
        start_diff,
        load: opt.load.clone(),
    })
}

/// A path found and modelled, before the cascade.
struct Prepared {
    rep: Report,
    built: Vec<Built>,
    start_diff: bool,
    load: Option<Network>,
}

/// Cascades the sections into the report.
fn finish(p: &Prepared, built: &[Built]) -> Report {
    let mut rep = p.rep.clone();
    let f = rep.f.clone();
    let z1 = Ctx::z_ref(p.start_diff);
    let mut total: Vec<Abcd> = vec![IDENT; f.len()];
    let mut mode = p.start_diff;
    let top = f.len() - 1;
    let mut sections = Vec::new();
    for b in built {
        for (k, t) in total.iter_mut().enumerate() {
            *t = mul(t, &b.m[k]);
        }
        if let Some(m) = b.mode_after {
            mode = m;
        }
        let mut sec = b.sec.clone();
        sec.total_db = -s_from_abcd(&total[top], z1, Ctx::z_ref(mode))[2].db();
        sections.push(sec);
    }
    let z2 = Ctx::z_ref(mode);
    rep.z_ref = [z1, z2];
    rep.s = total.iter().map(|m| s_from_abcd(m, z1, z2)).collect();
    if let Some(load) = &p.load {
        let sedz = z2 > 75.0 && load.z_ref < 75.0;
        if sedz {
            rep.notes.push(
                "the load is a single-ended model at the pair's end: taken as its single-ended equivalent (SEDZ, Zdiff = 2 × Z)"
                    .into(),
            );
        }
        rep.load_z = f.iter().map(|&ff| load_z(load, ff, sedz)).collect();
        rep.s11_loaded = rep
            .load_z
            .iter()
            .zip(&total)
            .map(|(&zl, m)| crate::touchstone::z_to_gamma(z_in(m, zl), z1))
            .collect();
        let band = valid_band(load);
        if let Some((lo, hi)) = band {
            if lo > f[0] + 1e-9 || hi < f[top] - 1e-9 {
                rep.notes.push(format!(
                    "the load model is valid from {} to {} GHz; outside that it is held at its nearest point, and the worst return loss with it is taken inside",
                    trim_num(lo),
                    trim_num(hi)
                ));
            }
        }
        rep.load_band = band;
    }
    rep.length = built
        .iter()
        .filter(|b| b.sec.kind == "trace")
        .map(|b| b.sec.length)
        .sum();
    rep.sections = sections;
    rep
}

/// The load's impedance at `f` GHz (differential when `sedz`).
fn load_z(load: &Network, f: f64, sedz: bool) -> Cx {
    let g = at_clamped(load, f)
        .and_then(|v| v.first().copied())
        .unwrap_or(c(0.0));
    // A single-ended model (50 Ω) at a pair's end is the single-ended
    // equivalent (SEDZ): Γ is the same with the differential impedance
    // twice as high.
    let zr = if sedz { 2.0 * load.z_ref } else { load.z_ref };
    if sub(c(1.0), g).abs() < 1e-9 {
        c(OPEN_Z)
    } else {
        crate::touchstone::gamma_to_z(g, zr)
    }
}

// ---------------------------------------------------------------------
// Matching: values for the empty part sites.

/// What the tuner aims at.
#[derive(Clone, Debug)]
pub struct TuneGoal {
    /// Band, GHz (0 to 0: from `fmin` to the top of the sweep).
    pub band: (f64, f64),
    /// Weight of the mean insertion loss in the band against the worst
    /// return loss (both dB).
    pub il_weight: f64,
    /// Smallest and largest capacitor (F) and inductor (H) to try.
    pub c_range: (f64, f64),
    pub l_range: (f64, f64),
}

impl Default for TuneGoal {
    fn default() -> Self {
        TuneGoal {
            band: (0.0, 0.0),
            il_weight: 1.0,
            c_range: (0.1e-12, 100e-12),
            l_range: (0.3e-9, 47e-9),
        }
    }
}

/// The part chosen for a site.
#[derive(Clone, Debug)]
pub struct Choice {
    /// The site's designators (alternates on the same pads with /).
    pub site: String,
    pub series: bool,
    pub across: bool,
    /// "1.2 pF", "3.3 nH", "0 Ω link" or "open".
    pub part: String,
    pub at: [f64; 2],
}

/// The tuner's result.
#[derive(Clone, Debug)]
pub struct Tuned {
    pub before: Report,
    pub after: Report,
    pub choices: Vec<Choice>,
    /// Worst return loss in the band, dB, before and after (with the
    /// load when one is given).
    pub worst: (f64, f64),
    /// Mean insertion loss in the band, dB, before and after.
    pub il: (f64, f64),
    /// The band used, GHz.
    pub band: (f64, f64),
    pub evaluations: usize,
}

/// E12 values from `lo` to `hi`.
fn e12(lo: f64, hi: f64) -> Vec<f64> {
    const E: [f64; 12] = [1.0, 1.2, 1.5, 1.8, 2.2, 2.7, 3.3, 3.9, 4.7, 5.6, 6.8, 8.2];
    let mut out = Vec::new();
    let mut dec = 10f64.powf(lo.log10().floor());
    while dec <= hi * 1.0001 {
        for e in E {
            let v = e * dec;
            if v >= lo * 0.9999 && v <= hi * 1.0001 {
                out.push(v);
            }
        }
        dec *= 10.0;
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Cand {
    /// Nothing fitted: a 0 Ω link in series, open in shunt.
    Empty,
    Cap(f64),
    Ind(f64),
}

impl Cand {
    fn label(self, series: bool) -> String {
        match self {
            Cand::Empty if series => "0 Ω link".into(),
            Cand::Empty => "open".into(),
            Cand::Cap(c) => format!("{} pF", sig(c * 1e12)),
            Cand::Ind(l) => format!("{} nH", sig(l * 1e9)),
        }
    }

    /// The part's model in a case.
    fn model(self, pack: Package) -> Option<PartModel> {
        match self {
            Cand::Empty => None,
            Cand::Cap(farads) => {
                let m = pdn::CapModel::mlcc(farads, pack);
                Some(PartModel::Rlc {
                    r: if farads < 1e-9 { 0.15 } else { m.esr },
                    l: m.esl,
                    c: Some(farads),
                    cp: 0.0,
                })
            }
            // A chip inductor: Q about 30 at 2 GHz, 0.1 pF across.
            Cand::Ind(h) => Some(PartModel::Rlc {
                r: TAU * 2e9 * h / 30.0,
                l: h,
                c: None,
                cp: 0.1e-12,
            }),
        }
    }
}

impl Site {
    /// This site's ABCD at `f` GHz (frequency index `k`) with `cand` in it.
    fn abcd(&self, cand: Cand, f: f64, k: usize) -> Abcd {
        if self.series {
            let z = match cand.model(self.pack) {
                Some(m) => m.z(f).unwrap_or(c(0.0)),
                // A 0 Ω link: the case's own inductance.
                None => Cx::new(0.01, TAU * f * 1e9 * self.pack.esl() * 0.8),
            };
            let m = series(z);
            if self.diff {
                to_diff(&m)
            } else {
                m
            }
        } else {
            let zl = cand
                .model(self.pack)
                .and_then(|m| m.z(f))
                .unwrap_or(c(OPEN_Z));
            let zl = if self.diff && !self.across {
                zl.scale(2.0)
            } else {
                zl
            };
            let chain = self.stub.get(k).copied().unwrap_or(IDENT);
            shunt(c(1.0).div(z_in(&chain, zl)))
        }
    }
}

/// Chooses parts for the empty sites (TBD, DNI, or `opt.free`) on the
/// path for the best match over the band: the worst return loss (into
/// the load, when one is given) plus the mean insertion loss times
/// `il_weight`, each extra part costing a little so that empty sites
/// stay empty unless a part helps.
pub fn tune(
    board: &Board,
    stackup: &Stackup,
    opt: &Options,
    goal: &TuneGoal,
    progress: &dyn Fn(f64, &str),
) -> Result<Tuned, String> {
    let p = prepare(board, stackup, opt, &|x, w| progress(0.6 * x, w))?;
    let before = finish(&p, &p.built);
    let sites: Vec<usize> = (0..p.built.len())
        .filter(|&i| p.built[i].site.is_some())
        .collect();
    if sites.is_empty() {
        return Err(
            "no empty part sites on the path (TBD, DNI); name fitted parts to choose again (free)"
                .into(),
        );
    }
    let f = &p.rep.f;
    let top = f.last().copied().unwrap_or(0.0);
    let (mut lo, mut hi) = if goal.band.1 > goal.band.0 {
        goal.band
    } else {
        (opt.fmin, top)
    };
    lo = lo.max(opt.fmin);
    if let Some((a, b)) = before.load_band {
        lo = lo.max(a);
        hi = hi.min(b);
    }
    let ks: Vec<usize> = (0..f.len())
        .filter(|&k| f[k] >= lo - 1e-9 && f[k] <= hi + 1e-9)
        .collect();
    if ks.is_empty() {
        return Err(format!("no frequencies between {lo} and {hi} GHz"));
    }
    // Candidates per site.
    let caps = e12(goal.c_range.0, goal.c_range.1);
    let inds = e12(goal.l_range.0, goal.l_range.1);
    let cands: Vec<Vec<Cand>> = sites
        .iter()
        .map(|&i| {
            let s = p.built[i].site.as_ref().expect("site");
            let mut v = vec![Cand::Empty];
            if s.caps {
                v.extend(caps.iter().map(|&x| Cand::Cap(x)));
            }
            if s.inductors {
                v.extend(inds.iter().map(|&x| Cand::Ind(x)));
            }
            v
        })
        .collect();
    // Each site's ABCD for each candidate at the band's frequencies.
    let table: Vec<Vec<Vec<Abcd>>> = sites
        .iter()
        .zip(&cands)
        .map(|(&i, cs)| {
            let s = p.built[i].site.as_ref().expect("site");
            cs.iter()
                .map(|&cd| ks.iter().map(|&k| s.abcd(cd, f[k], k)).collect())
                .collect()
        })
        .collect();
    // The fixed sections between sites, multiplied out.
    let mut segs: Vec<Vec<Abcd>> = vec![vec![IDENT; ks.len()]; sites.len() + 1];
    let mut seg = 0;
    for (i, b) in p.built.iter().enumerate() {
        if sites.get(seg) == Some(&i) {
            seg += 1;
            continue;
        }
        for (j, &k) in ks.iter().enumerate() {
            segs[seg][j] = mul(&segs[seg][j], &b.m[k]);
        }
    }
    let z1 = Ctx::z_ref(p.start_diff);
    let z2 = before.z_ref[1];
    let sedz = p.load.as_ref().is_some_and(|l| z2 > 75.0 && l.z_ref < 75.0);
    let loads: Option<Vec<Cx>> = p
        .load
        .as_ref()
        .map(|l| ks.iter().map(|&k| load_z(l, f[k], sedz)).collect());
    let evals = std::cell::Cell::new(0usize);
    let score = |pick: &[usize]| -> (f64, f64, f64) {
        evals.set(evals.get() + 1);
        let mut worst = f64::MIN;
        let mut il = 0.0;
        for j in 0..ks.len() {
            let mut t = segs[0][j];
            for (n, &ci) in pick.iter().enumerate() {
                t = mul(&t, &table[n][ci][j]);
                t = mul(&t, &segs[n + 1][j]);
            }
            let s = s_from_abcd(&t, z1, z2);
            let rl = match &loads {
                Some(zl) => crate::touchstone::z_to_gamma(z_in(&t, zl[j]), z1).db(),
                None => s[0].db(),
            };
            worst = worst.max(rl);
            il += -s[2].db();
        }
        let il = il / ks.len() as f64;
        let parts = pick.iter().filter(|&&c| c != 0).count() as f64;
        (worst + goal.il_weight * il + 0.02 * parts, worst, il)
    };
    // Coordinate descent from several starts: all empty, then seeded
    // starts.
    let n = sites.len();
    let mut best: Vec<usize> = vec![0; n];
    let mut best_s = score(&best).0;
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let starts = 8;
    for start in 0..starts {
        progress(0.6 + 0.38 * start as f64 / starts as f64, "choosing parts");
        crate::cancel::check()?;
        let mut cur: Vec<usize> = if start == 0 {
            vec![0; n]
        } else {
            cands
                .iter()
                .map(|cs| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    // Half the sites empty, the rest at random.
                    if seed.is_multiple_of(2) {
                        0
                    } else {
                        (seed >> 8) as usize % cs.len()
                    }
                })
                .collect()
        };
        let mut cur_s = score(&cur).0;
        for _sweep in 0..10 {
            let mut moved = false;
            for si in 0..n {
                for ci in 0..cands[si].len() {
                    if ci == cur[si] {
                        continue;
                    }
                    let old = cur[si];
                    cur[si] = ci;
                    let sc = score(&cur).0;
                    if sc < cur_s - 1e-6 {
                        cur_s = sc;
                        moved = true;
                    } else {
                        cur[si] = old;
                    }
                }
            }
            if !moved {
                break;
            }
        }
        if cur_s < best_s {
            best_s = cur_s;
            best = cur;
        }
    }
    let (_, w0, il0) = score(&vec![0; n]);
    let (_, w1, il1) = score(&best);
    // The tuned path at every frequency.
    let mut built = p.built.clone();
    let mut choices = Vec::new();
    for (n_, &i) in sites.iter().enumerate() {
        let s = built[i].site.clone().expect("site");
        let cd = cands[n_][best[n_]];
        built[i].m = (0..f.len()).map(|k| s.abcd(cd, f[k], k)).collect();
        let label = cd.label(s.series);
        let name = s.names.join("/");
        built[i].sec.what = format!(
            "{name}: {label} (tuned){}",
            if s.series {
                ""
            } else if s.across {
                " across the pair"
            } else {
                " to ground"
            }
        );
        built[i].sec.model = cd
            .model(s.pack)
            .map_or("nothing fitted".to_string(), |m| m.describe());
        choices.push(Choice {
            site: name,
            series: s.series,
            across: s.across,
            part: label,
            at: built[i].sec.at,
        });
    }
    let mut before = before;
    before.fmin = lo;
    let mut after = finish(&p, &built);
    after.fmin = lo;
    after.notes.push(format!(
        "tuned {} site(s) for {}-{} GHz: worst return loss {:.1} -> {:.1} dB, mean insertion loss {:.3} -> {:.3} dB",
        n,
        trim_num(lo),
        trim_num(hi),
        w0,
        w1,
        il0,
        il1
    ));
    progress(1.0, "done");
    Ok(Tuned {
        before,
        after,
        choices,
        worst: (w0, w1),
        il: (il0, il1),
        band: (lo, hi),
        evaluations: evals.get(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Cx, b: Cx) -> bool {
        sub(a, b).abs() < 1e-9
    }

    #[test]
    fn s_and_abcd_round_trip_with_unequal_references() {
        let s = [
            Cx::new(0.1, 0.2),
            Cx::new(0.7, -0.3),
            Cx::new(0.6, -0.35),
            Cx::new(-0.2, 0.05),
        ];
        let m = abcd_from_s(&s, 50.0, 100.0);
        let back = s_from_abcd(&m, 50.0, 100.0);
        for k in 0..4 {
            assert!(close(s[k], back[k]), "{k}: {:?} {:?}", s[k], back[k]);
        }
    }

    #[test]
    fn a_matched_line_loses_only_its_attenuation() {
        // 1 inch of 50 Ω line at 0.5 dB/in.
        let alpha = 0.5 / 8.686 / 0.0254;
        let m = line(50.0, 4.0, 1000.0, alpha, 5.0);
        let s = s_from_abcd(&m, 50.0, 50.0);
        assert!((s[2].db() + 0.5).abs() < 1e-3);
        assert!(s[0].abs() < 1e-9);
        // The same as touchstone's line.
        let t = crate::touchstone::line(50.0, 4.0, 1000.0, 0.5, 5.0, 50.0);
        assert!(close(s[2], t[1]));
    }

    #[test]
    fn ideal_balun_matches_50_to_100() {
        let n = (1.0f64 / 2.0).sqrt();
        let m = [c(n), c(0.0), c(0.0), c(1.0 / n)];
        let s = s_from_abcd(&m, 50.0, 100.0);
        assert!(s[0].abs() < 1e-12 && (s[2].abs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_dc_block_passes_rf_and_stops_dc() {
        let cap = PartModel::Rlc {
            r: 0.15,
            l: 0.35e-9,
            c: Some(100e-12),
            cp: 0.0,
        };
        let at = |f: f64| -s_from_abcd(&series(cap.z(f).unwrap()), 50.0, 50.0)[2].db();
        assert!(at(0.001) > 10.0, "{}", at(0.001));
        assert!(at(1.0) < 0.1, "{}", at(1.0));
    }

    #[test]
    fn reads_henries_and_balun_ratios() {
        assert_eq!(parse_henries("10nH"), Some(10e-9));
        assert!((parse_henries("4n7").unwrap() - 4.7e-9).abs() < 1e-15);
        assert!((parse_henries("2.2 uH").unwrap() - 2.2e-6).abs() < 1e-15);
        assert_eq!(parse_henries("100R"), None);
        assert_eq!(balun_ratio("BAL 50:100 2.4GHz"), 2.0);
        assert_eq!(balun_ratio("1:1 balun"), 1.0);
        assert_eq!(balun_ratio("XFMR"), 2.0);
    }

    /// A 50 Ω microstrip from J1 through C1 to U1, with a via on the way.
    fn board() -> Board {
        use crate::{Component, Length, Net, Pad, PadShape, Point, Track};
        let mut b = Board {
            stackup: crate::stackfile::parse(
            "TOP, copper, 1.4mil\nPP1, dielectric, 4mil, 3.6, 0.004\nL2, copper, 1.4mil\nCore, dielectric, 40mil, 4.2, 0.02\nL3, copper, 1.4mil\nPP2, dielectric, 4mil, 3.6, 0.004\nBOTTOM, copper, 1.4mil\n",
        )
        .expect("stack-up"),
            ..Default::default()
        };
        for name in ["RF", "RF2", "GND"] {
            b.nets.push(Net {
                name: name.into(),
                routed_length: Length(0),
                via_length: Length(0),
                vias: 0,
                pads: 0,
                pieces: 1,
                has_pour: false,
            });
        }
        let comp = |d: &str, v: &str| {
            let mut c = Component {
                designator: d.into(),
                footprint: String::new(),
                library_ref: None,
                x: Length(0),
                y: Length(0),
                rotation_deg: 0.0,
                side: crate::Side::Top,
                sheet: None,
                params: Default::default(),
            };
            c.params.insert("Value".into(), v.into());
            c
        };
        b.components.push(comp("J1", "SMA"));
        b.components.push(comp("C1", "100pF"));
        b.components.push(comp("U1", "LNA"));
        let pad = |c: u32, name: &str, x: f64, net: u32, layer: u8| Pad {
            layer: LayerRef::Copper(layer),
            net: Some(net),
            component: Some(c),
            name: name.into(),
            at: Point::mil(x, 0.0),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        let g = &mut b.geometry;
        g.pads.push(pad(0, "1", 0.0, 0, 0));
        g.pads.push(pad(0, "2", 0.0, 2, 3)); // ground on the bottom
        g.pads.push(pad(1, "1", 1000.0, 0, 0));
        g.pads.push(pad(1, "2", 1040.0, 1, 0));
        for i in 0..4 {
            g.pads.push(pad(
                2,
                &format!("{}", i + 1),
                2040.0 + 40.0 * i as f64,
                if i == 0 { 1 } else { 2 },
                0,
            ));
        }
        let tr = |x0: f64, x1: f64, net: u32, layer: u8| Track {
            layer: LayerRef::Copper(layer),
            net: Some(net),
            start: Point::mil(x0, 0.0),
            end: Point::mil(x1, 0.0),
            width: Length::from_mil(8.0),
            component: None,
            text: false,
        };
        g.tracks.push(tr(0.0, 500.0, 0, 0));
        g.tracks.push(tr(500.0, 1000.0, 0, 0));
        g.tracks.push(tr(1040.0, 1500.0, 1, 0));
        g.tracks.push(tr(1500.0, 2040.0, 1, 0));
        b
    }

    #[test]
    fn follows_the_route_through_a_series_cap() {
        let b = board();
        let opt = Options {
            from: "J1".into(),
            ghz: 6.0,
            points: 30,
            vias_3d: false,
            ..Default::default()
        };
        let r = run(&b, &b.stackup, &opt, &|_, _| {}).expect("path");
        assert_eq!(r.start, "J1.1");
        assert_eq!(r.end, "U1.1");
        let kinds: Vec<&str> = r.sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec!["trace", "part", "trace"], "{:?}", r.sections);
        assert!((r.length - 2000.0).abs() < 1.0, "{}", r.length);
        let il = r.il_db();
        assert!(il > 0.05 && il < 3.0, "{il}");
        // A 100 pF block stops the lowest frequencies a little more.
        assert!(r.s[0][2].abs() <= 1.0 + 1e-9);
    }

    #[test]
    fn leaves_out_tbd_parts_and_keeps_the_fitted_one_beside_them() {
        use crate::{Length, Pad, PadShape, Point};
        let base = board();
        let opt = Options {
            from: "J1".into(),
            ghz: 6.0,
            points: 30,
            vias_3d: false,
            ..Default::default()
        };
        let want = run(&base, &base.stackup, &opt, &|_, _| {}).expect("path");
        let mut b = base.clone();
        let mut add = |d: &str, v: &str, pads: [(f64, f64, u32); 2]| {
            let mut c = b.components[1].clone();
            c.designator = d.into();
            c.params.insert("Value".into(), v.into());
            b.components.push(c);
            let ci = (b.components.len() - 1) as u32;
            for (k, (x, y, net)) in pads.into_iter().enumerate() {
                b.geometry.pads.push(Pad {
                    layer: LayerRef::Copper(0),
                    net: Some(net),
                    component: Some(ci),
                    name: format!("{}", k + 1),
                    at: Point::mil(x, y),
                    size_x: Length::from_mil(12.0),
                    size_y: Length::from_mil(12.0),
                    hole: Length(0),
                    shape: PadShape::Rect,
                    rotation_deg: 0.0,
                    outline: Vec::new(),
                    stack: Vec::new(),
                    fab: Default::default(),
                });
            }
        };
        // An alternate inductor footprint beside C1, not chosen.
        add("L1", "TBD0201", [(1000.0, 60.0, 0), (1040.0, 60.0, 1)]);
        // A shunt site on the line, not fitted.
        add("C2", "DNI", [(300.0, 4.0, 0), (300.0, 40.0, 2)]);
        // A 0 Ω footprint stacked on C1's pads: an alternative, not both.
        add("R1", "0", [(1000.0, 0.0, 0), (1040.0, 0.0, 1)]);
        let r = run(&b, &b.stackup, &opt, &|_, _| {}).expect("path");
        assert!(
            (r.il_db() - want.il_db()).abs() < 1e-6,
            "{} vs {}",
            r.il_db(),
            want.il_db()
        );
        let notes = r.notes.join(" | ");
        assert!(
            notes.contains("L1") && notes.contains("C2") && notes.contains("took C1"),
            "{notes}"
        );
        // --fit takes the other.
        let fit = Options {
            fit: vec!["R1".into()],
            ..opt.clone()
        };
        let rf = run(&b, &b.stackup, &fit, &|_, _| {}).expect("path");
        assert!(rf.notes.join(" ").contains("took R1"));
        // A 1 pF shunt there is seen.
        let mut b2 = b.clone();
        let c2 = b2
            .components
            .iter()
            .position(|c| c.designator == "C2")
            .unwrap();
        b2.components[c2]
            .params
            .insert("Value".into(), "1pF".into());
        let r2 = run(&b2, &b2.stackup, &opt, &|_, _| {}).expect("path");
        assert!(r2.sections.iter().any(|s| s.kind == "shunt"));
        assert!(r2.il_db() > want.il_db() + 0.05, "{}", r2.il_db());
        // An ideal open as its vendor model is no load at all.
        let open = crate::touchstone::parse("# GHz S MA R 50\n0 1 0\n10 1 0\n", 1).unwrap();
        let opt3 = Options {
            models: vec![("C2".into(), open)],
            ..opt.clone()
        };
        let r3 = run(&b2, &b2.stackup, &opt3, &|_, _| {}).expect("path");
        assert!(
            (r3.il_db() - want.il_db()).abs() < 1e-3,
            "{} vs {}",
            r3.il_db(),
            want.il_db()
        );
    }

    #[test]
    fn reads_a_models_valid_range() {
        let n = crate::touchstone::parse(
            "! Valid Frequency Range: 1 GHz to 6 GHz\n# hz S ma R 50\n5e8 0.3 10\n6e9 0.4 20\n",
            1,
        )
        .unwrap();
        assert_eq!(valid_band(&n), Some((1.0, 6.0)));
        let m = crate::touchstone::parse("# GHz S MA R 50\n0.5 0.3 10\n6 0.4 20\n", 1).unwrap();
        assert_eq!(valid_band(&m), Some((0.5, 6.0)));
    }

    #[test]
    fn tuner_matches_a_mismatched_load() {
        use crate::{Length, Pad, PadShape, Point};
        let mut b = board();
        // Two empty sites near the chip: a series one is C1's own place
        // (made TBD), a shunt one by U1.
        b.components[1].params.insert("Value".into(), "TBD".into());
        let mut c = b.components[1].clone();
        c.designator = "C9".into();
        b.components.push(c);
        let ci = (b.components.len() - 1) as u32;
        for (k, (x, y, net)) in [(1900.0, 4.0, 1u32), (1900.0, 40.0, 2u32)]
            .into_iter()
            .enumerate()
        {
            b.geometry.pads.push(Pad {
                layer: LayerRef::Copper(0),
                net: Some(net),
                component: Some(ci),
                name: format!("{}", k + 1),
                at: Point::mil(x, y),
                size_x: Length::from_mil(12.0),
                size_y: Length::from_mil(12.0),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 0.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            });
        }
        // A chip input of 20 - j30 Ω: badly matched.
        let g = crate::touchstone::z_to_gamma(Cx::new(20.0, -30.0), 50.0);
        let load = crate::touchstone::parse(
            &format!(
                "# GHz S RI R 50\n0 {} {}\n10 {} {}\n",
                g.re, g.im, g.re, g.im
            ),
            1,
        )
        .unwrap();
        let opt = Options {
            from: "J1".into(),
            ghz: 3.0,
            points: 60,
            vias_3d: false,
            load: Some(load),
            any_part: true,
            ..Default::default()
        };
        let goal = TuneGoal {
            band: (2.3, 2.5),
            ..Default::default()
        };
        let t = tune(&b, &b.stackup, &opt, &goal, &|_, _| {}).expect("tune");
        assert_eq!(t.choices.len(), 2, "{:?}", t.choices);
        assert!(t.worst.0 > -6.0, "before {}", t.worst.0);
        assert!(t.worst.1 < -12.0, "after {} {:?}", t.worst.1, t.choices);
    }

    #[test]
    fn placeholders() {
        for v in ["TBD", "TBD0201", "DNI", "dnp", "", "NC", "N/A"] {
            assert!(placeholder(v), "{v}");
        }
        assert!(!placeholder("18pF"));
        assert!(!placeholder("0R"));
        assert!(!placeholder("NCP18XH103"));
        assert!(!placeholder("NC7WV125K8X"));
        assert!(placeholder("NC-0402"));
    }
}
