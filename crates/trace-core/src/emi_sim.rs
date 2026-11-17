//! EMI simulation: what the board radiates at the test distance, from
//! every fast net, the cables on its connectors, its power planes and its
//! decoupling loops, against FCC or CISPR 32, with each source's share
//! and what each fix gains.
//!
//! **Sources.** Every signal net and pair gets a waveform: from an IBIS
//! model when one is given for its driver, else from the user's
//! `NET=...` line, else a default for its kind, found from its name and
//! whether it is a pair (and said to be assumed):
//! - single-ended clocks: a trapezoid at the frequency in the name
//!   ("CLK_125M", "REF_245M76") or 50 MHz, its harmonics as lines;
//! - SerDes pairs (JESD, GT, PCIe, SFP…): PRBS at 12 Gb/s, 0.8 V, 30 ps;
//! - LVDS clocks (CLK, SYSREF pairs) at the name's frequency or 250 MHz;
//!   other LVDS pairs: PRBS at 800 Mb/s, 0.35 V, 300 ps;
//! - RF and LO pairs and nets: a tone at the name's frequency or 3.5 GHz,
//!   0 dBm for an LO or a transmitter's output, −20 dBm for a receiver's
//!   input (RX, ORX);
//! - other single-ended nets (CMOS data and control): PRBS at 50 Mb/s,
//!   3.3 V, 2 ns.
//!
//! Data spectra are the PRBS power in the receiver's bandwidth (120 kHz
//! below 1 GHz, 1 MHz above); clocks are lines. Levels are rms.
//!
//! **Mechanisms.**
//! 1. *Traces* (differential mode): each outer-layer segment and its
//!    return in the plane under it is a small loop, magnetic moment
//!    I·l·h, with the travelling wave's phase along the net. The loops'
//!    fields are added as vectors with their path phases toward 72
//!    directions over each side of the board, and the worst direction is
//!    kept (as a turntable and mast scan would find it), doubled for the
//!    test site's reflecting floor: E = η k² |r̂ × Σ m e^{jk r̂·p}| / (4π r)
//!    · 2, which for one loop is Ott's 2.63e-14 f² A I / r. A pair is its
//!    two lines with opposite currents, the negative one late by the
//!    pair's own length mismatch and 2 % weaker, so skew and imbalance
//!    show as common-mode radiation by themselves. Lines inside the
//!    board, between planes, do not radiate this way.
//! 2. *Cables* (common mode, current driven; Hockanson et al., IEEE
//!    Trans. EMC 1996): a net's return current crossing the plane drops
//!    V = jω L_ext I, L_ext ≈ μ0 Σ(l·h) / (π W) for a board W wide. That
//!    voltage drives each connector's cable against the board as a
//!    monopole of the cable's length L (Ott eq. 12-7, 150 Ω), fully when
//!    the net runs beside the connector and half from across the board
//!    (0.5 + 0.5 e^{−d / (diagonal/4)}); the cable radiating most for each
//!    net is kept. Cable lengths by kind: power 1.5 m, Ethernet 2 m, USB,
//!    video and RF test coax 1 m, U.FL/MMCX pigtails 0.1 m, a mated FMC
//!    board 0.15 m, headers 0.3 m, optical cages none (`--cable J1=2m`):
//!    E = 1.257e-6 f I_cm L / r, L no more than λ/π (resonance).
//! 3. *Plane cavity*: each power plane's impedance from the PDN model
//!    (its cavity, decoupling capacitors and regulator, so its
//!    resonances), times the share of the rail's switching current that
//!    gets past the die's own capacitance (20 nF per amp, at least 1 nF)
//!    and the package (0.3 nH) to the plane, is the voltage
//!    across the plane pair at its edge; the edge, a slot fed by that
//!    voltage, radiates E = k V L / (π r) over a quarter of the perimeter
//!    (at most λ/2).
//! 4. *Decoupling loops*: each capacitor's current (the plane voltage
//!    over its branch: ESR, ESL, mounting and C) in the loop of its pads
//!    and vias down to the plane (pitch × depth), as a small loop.
//!
//! Every contribution is in the limit's units (dBµV/m at its distance),
//! added as power across sources. Fixes are scored by re-running the
//! source with the change and reading the total at the worst frequency.

use super::{is_clock, is_ground, negatives, Limit};
use crate::drc::si::{plane_refs, Planes};
use crate::route::pt;
use crate::{Board, LayerRef};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::fmt::Write;

const C0: f64 = 2.998e8;
const MU0: f64 = 1.256_637_062_12e-6;
const ETA: f64 = 376.73;
const MIL: f64 = 25.4e-6;

/// What a net carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Clock,
    SerDes,
    LvdsClock,
    Lvds,
    Rf,
    Cmos,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Clock => "clock",
            Kind::SerDes => "SerDes",
            Kind::LvdsClock => "LVDS clock",
            Kind::Lvds => "LVDS data",
            Kind::Rf => "RF tone",
            Kind::Cmos => "CMOS data",
        }
    }
    fn is_data(self) -> bool {
        matches!(self, Kind::SerDes | Kind::Lvds | Kind::Cmos)
    }
}

/// A net's (or pair's) waveform.
#[derive(Clone, Debug)]
pub struct Signal {
    pub name: String,
    pub pos: u32,
    pub neg: Option<u32>,
    pub kind: Kind,
    /// Clock or tone frequency, or bit rate, Hz.
    pub freq: f64,
    /// Edge 10 to 90 %, s.
    pub rise: f64,
    /// Swing, V (differential peak to peak for a pair; for RF, dBm).
    pub swing: f64,
    /// Where the numbers came from: "IBIS …", "given", "default".
    pub origin: String,
    pub assumed: bool,
}

/// A user's waveform for nets: "NET=12Gbps@30ps,0.8V", "CLK*=125MHz",
/// "RX1_IN*=2.4GHz,0dBm".
#[derive(Clone, Debug)]
pub struct SignalSpec {
    pub pattern: String,
    pub freq: Option<f64>,
    pub rate: Option<f64>,
    pub rise: Option<f64>,
    pub volts: Option<f64>,
    pub dbm: Option<f64>,
}

impl SignalSpec {
    pub fn parse(s: &str) -> Result<SignalSpec, String> {
        let (pat, spec) = s
            .split_once('=')
            .ok_or_else(|| format!("{s}: NET=RATE|FREQ[@EDGE][,VOLTS|dBm]"))?;
        let mut out = SignalSpec {
            pattern: pat.trim().to_string(),
            freq: None,
            rate: None,
            rise: None,
            volts: None,
            dbm: None,
        };
        let (head, tail) = spec.split_once(',').unwrap_or((spec, ""));
        let (f, edge) = head.split_once('@').unwrap_or((head, ""));
        let fl = f.trim().to_ascii_lowercase();
        if let Some(r) = fl.strip_suffix("gbps").or_else(|| fl.strip_suffix("gb/s")) {
            out.rate = Some(
                r.trim()
                    .parse::<f64>()
                    .map_err(|_| format!("not a rate: {f}"))?
                    * 1e9,
            );
        } else if let Some(r) = fl.strip_suffix("mbps").or_else(|| fl.strip_suffix("mb/s")) {
            out.rate = Some(
                r.trim()
                    .parse::<f64>()
                    .map_err(|_| format!("not a rate: {f}"))?
                    * 1e6,
            );
        } else if !fl.is_empty() {
            out.freq = Some(super::freq_hz(&fl).ok_or_else(|| format!("not a frequency: {f}"))?);
        }
        if !edge.trim().is_empty() {
            out.rise =
                Some(super::time_s(edge.trim()).ok_or_else(|| format!("not an edge: {edge}"))?);
        }
        let t = tail.trim().to_ascii_lowercase();
        if let Some(d) = t.strip_suffix("dbm") {
            out.dbm = Some(
                d.trim()
                    .parse()
                    .map_err(|_| format!("not a power: {tail}"))?,
            );
        } else if let Some(v) = t.strip_suffix("mv") {
            out.volts = Some(
                v.trim()
                    .parse::<f64>()
                    .map_err(|_| format!("not a swing: {tail}"))?
                    / 1e3,
            );
        } else if let Some(v) = t.strip_suffix('v') {
            out.volts = Some(
                v.trim()
                    .parse()
                    .map_err(|_| format!("not a swing: {tail}"))?,
            );
        }
        Ok(out)
    }

    /// A whole kind of net: "class:rf", "class:serdes", "class:lvds",
    /// "class:clock", "class:cmos" (also "class:lo", the RF nets named LO).
    fn matches_kind(&self, kind: Kind, name: &str) -> bool {
        let Some(c) = self
            .pattern
            .to_ascii_lowercase()
            .strip_prefix("class:")
            .map(str::to_string)
        else {
            return false;
        };
        match c.as_str() {
            "rf" => kind == Kind::Rf,
            "lo" => kind == Kind::Rf && rf_role(name) == RfRole::Lo,
            "serdes" | "cml" => kind == Kind::SerDes,
            "lvds" => matches!(kind, Kind::Lvds | Kind::LvdsClock),
            "clock" | "clk" => matches!(kind, Kind::Clock | Kind::LvdsClock),
            "cmos" | "data" => kind == Kind::Cmos,
            _ => false,
        }
    }

    fn matches(&self, name: &str) -> bool {
        if self.pattern.to_ascii_lowercase().starts_with("class:") {
            return false;
        }
        if self.pattern.contains(['*', '?']) {
            crate::constraints::glob(&self.pattern, name)
        } else {
            name.eq_ignore_ascii_case(&self.pattern)
        }
    }
}

/// A rail's switching current: "VCCINT=2A@250MHz".
#[derive(Clone, Debug)]
pub struct RailSpec {
    pub pattern: String,
    pub amps: f64,
    pub freq: Option<f64>,
}

impl RailSpec {
    pub fn parse(s: &str) -> Result<RailSpec, String> {
        let (pat, spec) = s
            .split_once('=')
            .ok_or_else(|| format!("{s}: RAIL=AMPS[@FREQ]"))?;
        let (a, f) = spec.split_once('@').unwrap_or((spec, ""));
        let a = a.trim().trim_end_matches(['A', 'a']);
        Ok(RailSpec {
            pattern: pat.trim().to_string(),
            amps: a.parse().map_err(|_| format!("not a current: {a}"))?,
            freq: if f.trim().is_empty() {
                None
            } else {
                Some(super::freq_hz(f.trim()).ok_or_else(|| format!("not a frequency: {f}"))?)
            },
        })
    }
}

#[derive(Clone, Debug)]
pub struct SimSetup {
    pub limit: Limit,
    pub margin_db: f64,
    pub signals: Vec<SignalSpec>,
    /// IBIS models by component designator (every driving pin of the
    /// part takes its model's edge and swing).
    pub ibis: Vec<(String, crate::ibis::Ibis)>,
    /// Cable length on a connector, m (designator), over the defaults.
    pub cables: Vec<(String, f64)>,
    pub rails: Vec<RailSpec>,
    /// Single-ended data and control nets too (not only clocks, pairs
    /// and RF).
    pub cmos: bool,
    /// Solve the power planes (slower).
    pub planes: bool,
    /// Frequency points from 30 MHz to 6 GHz.
    pub points: usize,
    /// The near-field map's cell, mil (0: about 300 across the board).
    pub map_cell: f64,
}

impl Default for SimSetup {
    fn default() -> Self {
        SimSetup {
            limit: Limit::CisprB,
            margin_db: 6.0,
            signals: Vec::new(),
            ibis: Vec::new(),
            cables: Vec::new(),
            rails: Vec::new(),
            cmos: true,
            planes: true,
            points: 500,
            map_cell: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mechanism {
    Trace,
    Cable(String),
    Cavity,
    Decaps,
}

impl Mechanism {
    pub fn label(&self) -> String {
        match self {
            Mechanism::Trace => "traces".into(),
            Mechanism::Cable(j) => format!("cable on {j}"),
            Mechanism::Cavity => "plane cavity edge".into(),
            Mechanism::Decaps => "decoupling loops".into(),
        }
    }
}

/// One source's field through one mechanism.
#[derive(Clone, Debug)]
pub struct Contribution {
    pub source: String,
    pub mechanism: Mechanism,
    /// dBµV/m at the limit's distance per frequency point (−200: none).
    pub db: Vec<f64>,
}

impl Contribution {
    pub fn peak(&self, limit: &[f64]) -> (usize, f64) {
        let mut best = (0, f64::MIN);
        for (k, (&d, &l)) in self.db.iter().zip(limit).enumerate() {
            if d - l > best.1 {
                best = (k, d - l);
            }
        }
        best
    }
}

#[derive(Clone, Debug)]
pub struct Fix {
    pub source: String,
    pub text: String,
    /// dB the total drops at the worst frequency.
    pub gain_db: f64,
}

/// Near field over the board at one frequency.
#[derive(Clone, Debug)]
pub struct NearMap {
    pub f: f64,
    pub x0: f64,
    pub y0: f64,
    pub cell: f64,
    pub w: usize,
    pub h: usize,
    /// Name, unit and values (row-major, row 0 at y0).
    pub maps: Vec<(String, &'static str, Vec<f32>)>,
}

#[derive(Clone, Debug)]
pub struct SimResult {
    pub freqs: Vec<f64>,
    pub limit: Vec<f64>,
    pub limit_kind: Limit,
    pub margin_db: f64,
    pub total: Vec<f64>,
    pub contributions: Vec<Contribution>,
    pub signals: Vec<Signal>,
    pub connectors: Vec<(String, String, f64)>,
    pub fixes: Vec<Fix>,
    /// Worst frequency and margin (limit − total; negative fails).
    pub worst: (f64, f64),
    pub map: Option<NearMap>,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------
// Waveforms.

/// A frequency given in a net name: 125M, 245M76, 122P88M, 1G, 100MHZ.
pub fn freq_in_name(name: &str) -> Option<f64> {
    for t in name
        .to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
    {
        let t = t.trim_end_matches("HZ");
        for (unit, k) in [('M', 1e6), ('G', 1e9), ('K', 1e3)] {
            if let Some(i) = t.find(unit) {
                let (a, b) = (&t[..i], &t[i + 1..]);
                if a.is_empty() || !a.chars().all(|c| c.is_ascii_digit() || c == 'P') {
                    continue;
                }
                if !b.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let a = a.replace('P', ".");
                let v: f64 = if b.is_empty() {
                    a.parse().ok()?
                } else {
                    format!("{a}.{b}").parse().ok()?
                };
                let f = v * k;
                if (1e6..=20e9).contains(&f) {
                    return Some(f);
                }
            }
        }
    }
    None
}

fn kind_of(name: &str, pair: bool) -> Kind {
    let u = name.to_ascii_uppercase();
    let has = |ks: &[&str]| ks.iter().any(|k| u.contains(k));
    if pair {
        if has(&["DSI", "CSI", "CAM", "MIPI"]) && !has(&["SERD"]) {
            return Kind::Lvds;
        }
        if has(&[
            "SERD", "JESD", "GTX", "GTH", "GTY", "GTP", "MGT", "PCIE", "SATA", "SFP", "QSFP",
            "USB3", "SSTX", "SSRX", "LANE", "HDMI", "DP_", "_GT", "XAUI", "SGMII",
        ]) {
            return Kind::SerDes;
        }
        if is_clock(name) || has(&["SYSREF", "CLK", "REF"]) {
            return Kind::LvdsClock;
        }
        if has(&["RF", "LO", "ORX", "ANT", "_IN", "_OUT", "BALUN"]) {
            return Kind::Rf;
        }
        return Kind::Lvds;
    }
    if is_clock(name) {
        return Kind::Clock;
    }
    if has(&["RF_", "_RF", "LO_", "_LO", "ANT"]) {
        return Kind::Rf;
    }
    Kind::Cmos
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RfRole {
    /// A local oscillator: 0 dBm.
    Lo,
    /// A transmitter's output: 0 dBm.
    Out,
    /// A receiver's input (RX, ORX): −20 dBm.
    In,
}

fn rf_role(name: &str) -> RfRole {
    let u = name.to_ascii_uppercase();
    let toks: Vec<&str> = u.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if toks
        .iter()
        .any(|t| t.starts_with("LO") || t.ends_with("LO"))
    {
        RfRole::Lo
    } else if u.contains("RX") || u.contains("_IN") || u.contains("ANT") {
        RfRole::In
    } else {
        RfRole::Out
    }
}

fn defaults(kind: Kind, name: &str) -> (f64, f64, f64) {
    let f_name = freq_in_name(name);
    let u = name.to_ascii_uppercase();
    let has = |ks: &[&str]| ks.iter().any(|k| u.contains(k));
    match kind {
        Kind::Clock => (f_name.unwrap_or(50e6), 1e-9, 3.3),
        Kind::SerDes if has(&["HDMI"]) && has(&["CK", "CLK"]) => (340e6, 100e-12, 1.0),
        Kind::SerDes if has(&["HDMI", "DP_"]) => (3.4e9, 100e-12, 1.0),
        Kind::SerDes if has(&["USB3", "SSTX", "SSRX"]) => (5e9, 50e-12, 1.0),
        Kind::SerDes if has(&["PCIE"]) => (8e9, 40e-12, 1.0),
        Kind::SerDes if has(&["SATA"]) => (6e9, 50e-12, 0.6),
        Kind::SerDes if has(&["SFP", "XAUI"]) => (10.3125e9, 30e-12, 0.8),
        Kind::SerDes if has(&["SGMII"]) => (1.25e9, 100e-12, 0.8),
        Kind::SerDes => (12e9, 30e-12, 0.8),
        Kind::Lvds if has(&["USB"]) => (480e6, 500e-12, 0.8),
        Kind::Lvds if has(&["DSI", "CSI", "CAM", "MIPI"]) => (1e9, 150e-12, 0.4),
        Kind::LvdsClock => (f_name.unwrap_or(250e6), 300e-12, 0.7),
        Kind::Lvds => (800e6, 300e-12, 0.7),
        Kind::Rf => (f_name.unwrap_or(3.5e9), 50e-12, 0.0),
        Kind::Cmos => (50e6, 2e-9, 3.3),
    }
}

/// Spectral lines or densities of a signal's current (per line of a
/// pair): (Hz, A rms) at the frequencies it has, given the grid.
fn current_spectrum(s: &Signal, i_pp: f64, grid: &[f64]) -> Vec<(f64, f64)> {
    let sinc = |x: f64| if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
    let fmax = *grid.last().unwrap_or(&6e9);
    let fmin = *grid.first().unwrap_or(&30e6);
    match s.kind {
        Kind::Rf => {
            // P dBm into 50 Ω.
            let p = 1e-3 * 10f64.powf(s.swing / 10.0);
            vec![(s.freq, (p / 50.0).sqrt())]
        }
        Kind::Clock | Kind::LvdsClock => {
            let mut out = Vec::new();
            let mut n = 1usize;
            while n as f64 * s.freq <= fmax && n < 4000 {
                let f = n as f64 * s.freq;
                if f >= fmin {
                    let c = i_pp * sinc(n as f64 * PI / 2.0).abs() * sinc(PI * f * s.rise).abs();
                    if c > 0.0 {
                        out.push((f, c / 2f64.sqrt()));
                    }
                }
                n += 1;
            }
            out
        }
        _ => {
            // NRZ PRBS: one-sided PSD (A²/2)·Tb·sinc²(πfTb)·sinc²(πf tr),
            // in the receiver's bandwidth.
            let tb = 1.0 / s.freq;
            grid.iter()
                .map(|&f| {
                    let rbw = if f < 1e9 { 120e3 } else { 1e6 };
                    let psd = i_pp * i_pp / 2.0
                        * tb
                        * sinc(PI * f * tb).powi(2)
                        * sinc(PI * f * s.rise).powi(2);
                    (f, (psd * rbw).sqrt())
                })
                .collect()
        }
    }
}

// ---------------------------------------------------------------------
// Geometry of a net.

#[derive(Clone, Copy, Debug)]
struct Seg {
    /// Middle and unit direction, m; length and height over its
    /// reference plane, m.
    mid: [f64; 2],
    dir: [f64; 2],
    len: f64,
    h: f64,
    /// +1 for the top side (plane under it), −1 for the bottom side;
    /// 0 inside (between planes).
    side: i8,
    /// Path length from the driver, m (for the wave's phase).
    s: f64,
    layer: usize,
    width: f64,
    eeff: f64,
}

struct NetGeo {
    segs: Vec<Seg>,
    len: f64,
}

fn net_geo(
    board: &Board,
    planes: &Planes,
    geo: &[crate::impedance::LayerGeometry],
    ncu: usize,
    net: u32,
    driver: [f64; 2],
) -> NetGeo {
    let mut segs = Vec::new();
    let mut len = 0.0;
    for t in &board.geometry.tracks {
        let (LayerRef::Copper(l), Some(n)) = (&t.layer, t.net) else {
            continue;
        };
        if n != net || t.component.is_some() {
            continue;
        }
        let l = *l as usize;
        let (a, b) = (pt(t.start), pt(t.end));
        let d = [b[0] - a[0], b[1] - a[1]];
        let lm = d[0].hypot(d[1]);
        if lm < 0.01 {
            continue;
        }
        let refs = plane_refs(planes, geo, l);
        let lg = geo.get(l);
        let h_mil = refs
            .first()
            .map(|r| r.1)
            .or_else(|| lg.and_then(|x| x.below.or(x.above)))
            .unwrap_or(5.0)
            .max(0.5);
        let outer = l == 0 || l + 1 == ncu;
        let side = if !outer {
            0
        } else if l == 0 {
            1
        } else {
            -1
        };
        let er = lg.map_or(4.0, |x| x.er);
        let w = t.width.mil();
        let eeff = if outer {
            crate::impedance::microstrip_eeff(w.max(1.0), h_mil, er)
        } else {
            er
        };
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let s = 1.15 * (mid[0] - driver[0]).hypot(mid[1] - driver[1]) * MIL;
        segs.push(Seg {
            mid: [mid[0] * MIL, mid[1] * MIL],
            dir: [d[0] / lm, d[1] / lm],
            len: lm * MIL,
            h: h_mil * MIL,
            side,
            s,
            layer: l,
            width: w * MIL,
            eeff,
        });
        len += lm * MIL;
    }
    NetGeo { segs, len }
}

/// Directions over one side: (unit vector), θ from the board's normal.
fn directions(side: f64) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for &th in &[90.0f64, 60.0, 30.0] {
        let th = th.to_radians();
        for k in 0..24 {
            let ph = 2.0 * PI * k as f64 / 24.0;
            out.push([th.sin() * ph.cos(), th.sin() * ph.sin(), side * th.cos()]);
        }
    }
    out
}

/// |E| at 1 m (V/m per A rms of the net's drive) from a net's outer
/// segments at f: the worst direction over each side, site doubling in.
/// `lines`: per line of the net (one, or a pair's two), the segments
/// and the line's current phasor relative to the drive.
fn trace_e(lines: &[(&[Seg], (f64, f64))], f: f64, dirs: &[Vec<[f64; 3]>; 2]) -> f64 {
    let k = 2.0 * PI * f / C0;
    let mut worst: f64 = 0.0;
    for (si, side) in [1i8, -1].iter().enumerate() {
        let any = lines.iter().any(|(s, _)| s.iter().any(|g| g.side == *side));
        if !any {
            continue;
        }
        let ds = &dirs[si];
        for d in ds.iter() {
            // Σ m e^{j(k r̂·p − β s)}: m horizontal, ẑ × dir (times side).
            let (mut mx, mut my) = ((0.0, 0.0), (0.0, 0.0));
            for (segs, (ar, ai)) in lines {
                for g in segs.iter().filter(|g| g.side == *side) {
                    let beta = k * g.eeff.sqrt();
                    let ph = k * (d[0] * g.mid[0] + d[1] * g.mid[1]) - beta * g.s;
                    let (sn, cs) = ph.sin_cos();
                    // Current phasor (ar + j ai) e^{j ph}.
                    let re = ar * cs - ai * sn;
                    let im = ar * sn + ai * cs;
                    let a = g.len * g.h * *side as f64;
                    // ẑ × u = (−u_y, u_x).
                    mx.0 += -g.dir[1] * a * re;
                    mx.1 += -g.dir[1] * a * im;
                    my.0 += g.dir[0] * a * re;
                    my.1 += g.dir[0] * a * im;
                }
            }
            // |r̂ × M|² = |M|² − |r̂·M|² (M complex, horizontal).
            let m2 = mx.0 * mx.0 + mx.1 * mx.1 + my.0 * my.0 + my.1 * my.1;
            let dr = (d[0] * mx.0 + d[1] * my.0, d[0] * mx.1 + d[1] * my.1);
            let t2 = (m2 - dr.0 * dr.0 - dr.1 * dr.1).max(0.0);
            worst = worst.max(t2.sqrt());
        }
    }
    2.0 * ETA * k * k * worst / (4.0 * PI)
}

/// μ0 Σ l·h / (π W): the plane's share of a net's loop, H.
fn l_ext(segs: &[Seg], width_m: f64) -> f64 {
    MU0 * segs.iter().map(|g| g.len * g.h).sum::<f64>() / (PI * width_m.max(0.01))
}

/// Cable radiation at 1 m per volt of common-mode drive at f.
fn cable_e_per_v(f: f64, len_m: f64, z_extra: f64) -> f64 {
    let leff = len_m.min(C0 / (PI * f));
    let icm = 1.0 / (150.0 + z_extra);
    1.257e-6 * f * icm * leff
}

fn db_uv(e: f64) -> f64 {
    if e > 0.0 {
        20.0 * (e / 1e-6).log10()
    } else {
        -200.0
    }
}

fn add_db(a: f64, b: f64) -> f64 {
    if a <= -199.0 {
        return b;
    }
    if b <= -199.0 {
        return a;
    }
    10.0 * (10f64.powf(a / 10.0) + 10f64.powf(b / 10.0)).log10()
}

/// A connector and the cable it is taken to carry.
#[derive(Clone, Debug)]
struct Conn {
    des: String,
    what: String,
    /// Cable length, m.
    len: f64,
    /// Where it is (its pads' centre), mil.
    at: [f64; 2],
}

fn power_name(nm: &str) -> bool {
    let u = nm.to_ascii_uppercase();
    crate::pdn::volts_in_name(&u).is_some()
        || [
            "VIN", "VCC", "VDD", "PWR", "POWER", "VBUS", "VBAT", "+12", "+5", "12V", "5V", "24V",
        ]
        .iter()
        .any(|k| u.contains(k))
}

/// The connectors and the cable each is taken to carry. Optical cages
/// carry none; logos and holes (no pad on a signal net) are not
/// connectors.
fn connectors(board: &Board, given: &[(String, f64)]) -> Vec<Conn> {
    let mut out = Vec::new();
    // Each part's pads: how many, whether any is on a signal net,
    // whether any is on a supply, and their centre.
    let mut pads: HashMap<u32, (usize, bool, bool, [f64; 2])> = HashMap::new();
    for p in &board.geometry.pads {
        if let Some(ci) = p.component {
            let e = pads.entry(ci).or_default();
            e.0 += 1;
            let q = pt(p.at);
            e.3[0] += q[0];
            e.3[1] += q[1];
            if let Some(n) = p.net {
                let nm = board.nets.get(n as usize).map_or("", |x| x.name.as_str());
                if !is_ground(nm) {
                    e.1 = true;
                }
                if power_name(nm) {
                    e.2 = true;
                }
            }
        }
    }
    for (ci, c) in board.components.iter().enumerate() {
        // Logos, fiducials, mounting holes: no pads on a signal net.
        if !pads.get(&(ci as u32)).is_some_and(|p| p.0 >= 2 && p.1) {
            continue;
        }
        let d = c.designator.to_ascii_uppercase();
        let text = format!(
            "{} {} {}",
            c.footprint,
            c.library_ref.clone().unwrap_or_default(),
            c.param(&[
                "Comment",
                "Value",
                "Description",
                "Part Number",
                "PartNumber"
            ])
            .unwrap_or("")
        )
        .to_ascii_uppercase();
        // Whole words (or a word's start before a digit or dash), so
        // "SMALL" is not an SMA.
        let words: Vec<&str> = text
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '-'))
            .filter(|w| !w.is_empty())
            .collect();
        let has = |ks: &[&str]| {
            ks.iter().any(|k| {
                words.iter().any(|w| {
                    w.starts_with(k)
                        && w[k.len()..]
                            .chars()
                            .next()
                            .is_none_or(|ch| !ch.is_ascii_alphabetic() || k.ends_with('-'))
                })
            })
        };
        let is_conn = d.starts_with('J') && !d.starts_with("JP")
            || d.starts_with("CN")
            || d.starts_with('P') && d[1..].chars().all(|c| c.is_ascii_digit()) && d.len() > 1;
        if !is_conn && !has(&["SMA", "USB", "RJ45", "FMC", "HDMI", "CONN"]) {
            continue;
        }
        let info = pads[&(ci as u32)];
        let (what, len) = if has(&["SFP", "QSFP"]) {
            ("optical cage", 0.0)
        } else if has(&["U.FL", "UFL", "MMCX", "MHF", "IPEX", "W.FL"]) {
            // Inside the box, to an antenna or module: a short pigtail.
            ("coax pigtail", 0.1)
        } else if has(&["SMA", "SMP", "2.92", "K-", "BNC", "COAX", "N-"]) {
            ("RF coax (test cable)", 1.0)
        } else if has(&["JACK", "BARREL", "DCJ", "PJ-", "TERMINAL", "PHOENIX"])
            || info.2 && info.0 <= 6
        {
            ("power cable", 1.5)
        } else if has(&["USB"]) {
            ("USB cable", 1.0)
        } else if has(&["RJ45", "ETH", "MAGJACK"]) {
            ("Ethernet cable", 2.0)
        } else if has(&["HDMI", "DISPLAYPORT"]) {
            ("video cable", 1.0)
        } else if has(&["FMC", "ASP-", "SAMTEC", "SEAF", "SEAM", "MEZZ"]) {
            ("mated board", 0.15)
        } else if has(&["HDR", "HEADER", "PIN"]) {
            ("header wires", 0.3)
        } else {
            ("connector", 0.5)
        };
        let len = given
            .iter()
            .find(|(g, _)| g.eq_ignore_ascii_case(&c.designator))
            .map_or(len, |g| g.1);
        if len > 0.0 {
            let n = info.0.max(1) as f64;
            out.push(Conn {
                des: c.designator.clone(),
                what: what.to_string(),
                len,
                at: [info.3[0] / n, info.3[1] / n],
            });
        }
    }
    out
}

// ---------------------------------------------------------------------

/// Runs the simulation. Long: checks for a cancel.
pub fn simulate(
    board: &Board,
    setup: &SimSetup,
    progress: &dyn Fn(f64),
) -> Result<SimResult, String> {
    let mut notes = Vec::new();
    let g = &board.geometry;
    let planes = Planes::new(board);
    let geo = crate::impedance::layer_geometry(&board.stackup);
    let ncu = planes.layers.len().max(geo.len());
    // The grid and the limit.
    let n = setup.points.max(50);
    let (f0, f1) = (30e6f64, 6e9f64);
    let freqs: Vec<f64> = (0..n)
        .map(|k| f0 * (f1 / f0).powf(k as f64 / (n - 1) as f64))
        .collect();
    let lim: Vec<(f64, f64)> = freqs
        .iter()
        .map(|&f| setup.limit.at(f).unwrap_or((60.0, 3.0)))
        .collect();
    let limit: Vec<f64> = lim.iter().map(|l| l.0).collect();
    let bin = |f: f64| -> Option<usize> {
        if !(f0 * 0.995..=f1 * 1.005).contains(&f) {
            return None;
        }
        let x = (f / f0).ln() / (f1 / f0).ln() * (n - 1) as f64;
        Some((x.round() as usize).min(n - 1))
    };
    // Board size (for L_ext) from the outline, else the tracks.
    let pts: Vec<[f64; 2]> = if g.outline.len() >= 3 {
        g.outline.iter().map(|&p| pt(p)).collect()
    } else {
        g.tracks
            .iter()
            .flat_map(|t| [pt(t.start), pt(t.end)])
            .collect()
    };
    if pts.is_empty() {
        return Err("the board has no outline or tracks".into());
    }
    let bx = pts
        .iter()
        .fold([f64::MAX, f64::MIN, f64::MAX, f64::MIN], |b, p| {
            [
                b[0].min(p[0]),
                b[1].max(p[0]),
                b[2].min(p[1]),
                b[3].max(p[1]),
            ]
        });
    let (bw, bh) = ((bx[1] - bx[0]) * MIL, (bx[3] - bx[2]) * MIL);
    let width = bw.min(bh).max(0.01);
    // Nets that own plane copper, and the parts' pin counts (the driver
    // of a net is taken to be its pin on the part with the most pins).
    let mut plane_nets: HashSet<u32> = HashSet::new();
    for p in g.polygons.iter().filter(|p| !p.teardrop) {
        if let (LayerRef::Copper(l), Some(net)) = (&p.layer, p.net) {
            if planes.is_plane(*l as usize) {
                plane_nets.insert(net);
            }
        }
    }
    let mut pins_of: HashMap<u32, usize> = HashMap::new();
    for p in &g.pads {
        if let Some(c) = p.component {
            *pins_of.entry(c).or_default() += 1;
        }
    }
    let mut net_pads: HashMap<u32, Vec<&crate::Pad>> = HashMap::new();
    for p in &g.pads {
        if let Some(nn) = p.net {
            net_pads.entry(nn).or_default().push(p);
        }
    }
    let driver_of = |net: u32| -> ([f64; 2], Option<u32>, String) {
        let pads = net_pads.get(&net);
        let best = pads.and_then(|v| {
            v.iter().max_by_key(|p| {
                p.component
                    .map_or(0, |c| pins_of.get(&c).copied().unwrap_or(0))
            })
        });
        match best {
            Some(p) => (pt(p.at), p.component, p.name.clone()),
            None => ([0.0, 0.0], None, String::new()),
        }
    };
    let name = |n: u32| {
        board
            .nets
            .get(n as usize)
            .map_or(format!("net {n}"), |x| x.name.clone())
    };
    let is_power = |n: u32| {
        let nm = name(n);
        let u = nm.to_ascii_uppercase();
        plane_nets.contains(&n)
            || is_ground(&nm)
            || crate::pdn::volts_in_name(&nm).is_some()
            || ["VCC", "VDD", "VEE", "VBAT", "VIN", "VOUT", "PWR", "VREF"]
                .iter()
                .any(|k| u.contains(k))
    };
    // Pairs first.
    let mut pairs = board.diff_pairs.clone();
    pairs.extend(crate::zcheck::name_pairs(board));
    let id = |s: &str| {
        board
            .nets
            .iter()
            .position(|n| n.name == s)
            .map(|i| i as u32)
    };
    let mut taken: HashSet<u32> = HashSet::new();
    let mut signals: Vec<Signal> = Vec::new();
    let add = |pos: u32, neg: Option<u32>, nm: String, signals: &mut Vec<Signal>| {
        let kind = kind_of(&nm, neg.is_some());
        let (mut freq, mut rise, mut swing) = defaults(kind, &nm);
        let mut kind = kind;
        let mut origin = "default for its kind".to_string();
        let mut assumed = true;
        if kind == Kind::Rf {
            swing = match rf_role(&nm) {
                RfRole::Lo | RfRole::Out => 0.0,
                RfRole::In => -20.0,
            };
        }
        // IBIS for the driver's part.
        let (_, comp, pin) = driver_of(pos);
        if let Some(ci) = comp {
            let des = &board.components[ci as usize].designator;
            if let Some((_, ib)) = setup.ibis.iter().find(|(d, _)| d.eq_ignore_ascii_case(des)) {
                let model = ib
                    .components
                    .iter()
                    .find_map(|c| c.pin(&pin))
                    .and_then(|p| ib.model(&p.model))
                    .filter(|m| m.kind.drives());
                if let Some(m) = model {
                    if let Some(r) = m.ramp {
                        let dv = r.rise.0.at(crate::ibis::Corner::Typ);
                        let dt = r.rise.1.at(crate::ibis::Corner::Typ);
                        if dv > 0.0 && dt > 0.0 {
                            rise = dt / 0.6 * 0.8;
                            swing = dv / 0.6 * if neg.is_some() { 2.0 } else { 1.0 };
                            origin = format!("IBIS {des} pin {pin} model {}", m.name);
                            assumed = false;
                        }
                    }
                }
            }
        }
        // The user's line wins.
        if let Some(sp) = setup
            .signals
            .iter()
            .find(|s| s.matches(&nm) || s.matches(&name(pos)))
            .or_else(|| setup.signals.iter().find(|s| s.matches_kind(kind, &nm)))
        {
            if let Some(r) = sp.rate {
                freq = r;
                if !kind.is_data() {
                    kind = if neg.is_some() {
                        Kind::SerDes
                    } else {
                        Kind::Cmos
                    };
                }
            }
            if let Some(f) = sp.freq {
                freq = f;
                if kind.is_data() {
                    kind = if neg.is_some() {
                        Kind::LvdsClock
                    } else {
                        Kind::Clock
                    };
                }
            }
            if let Some(r) = sp.rise {
                rise = r;
            }
            if let Some(v) = sp.volts {
                swing = v;
            }
            if let Some(d) = sp.dbm {
                kind = Kind::Rf;
                swing = d;
            }
            origin = "given".into();
            assumed = false;
        }
        signals.push(Signal {
            name: nm,
            pos,
            neg,
            kind,
            freq,
            rise,
            swing,
            origin,
            assumed,
        });
    };
    for p in &pairs {
        if let (Some(a), Some(b)) = (id(&p.positive), id(&p.negative)) {
            if taken.contains(&a) || taken.contains(&b) || is_power(a) {
                continue;
            }
            taken.insert(a);
            taken.insert(b);
            add(a, Some(b), p.name.clone(), &mut signals);
        }
    }
    // Clock pairs by name.
    for (i, nn) in board.nets.iter().enumerate() {
        let i = i as u32;
        if taken.contains(&i) || !is_clock(&nn.name) {
            continue;
        }
        for neg in negatives(&nn.name) {
            if let Some(j) = id(&neg).filter(|j| !taken.contains(j)) {
                taken.insert(i);
                taken.insert(j);
                add(i, Some(j), nn.name.clone(), &mut signals);
                break;
            }
        }
    }
    // Pairs by other names: DP/DN, DP/DM (USB D+ D-), P/N endings.
    for (i, nn) in board.nets.iter().enumerate() {
        let i = i as u32;
        if taken.contains(&i) || is_power(i) {
            continue;
        }
        let u = nn.name.clone();
        let mut cands: Vec<String> = Vec::new();
        for (p, q) in [
            ("DP", "DN"),
            ("DP", "DM"),
            ("_P", "_N"),
            ("P", "N"),
            ("+", "-"),
        ] {
            if let Some(k) = u.rfind(p) {
                let tail = &u[k + p.len()..];
                if tail.chars().all(|c| c.is_ascii_digit()) {
                    cands.push(format!("{}{q}{tail}", &u[..k]));
                }
            }
        }
        if let Some(j) = cands
            .iter()
            .find_map(|c| id(c))
            .filter(|j| *j != i && !taken.contains(j))
        {
            taken.insert(i);
            taken.insert(j);
            add(i, Some(j), u, &mut signals);
        }
    }
    // Single-ended nets with tracks.
    let mut routed: HashMap<u32, f64> = HashMap::new();
    for t in &g.tracks {
        if let (LayerRef::Copper(_), Some(nn)) = (&t.layer, t.net) {
            if t.component.is_none() {
                *routed.entry(nn).or_default() +=
                    (pt(t.end)[0] - pt(t.start)[0]).hypot(pt(t.end)[1] - pt(t.start)[1]);
            }
        }
    }
    let mut se: Vec<u32> = routed.keys().copied().collect();
    se.sort();
    for nn in se {
        if taken.contains(&nn) || is_power(nn) || routed[&nn] < 200.0 {
            continue;
        }
        let nm = name(nn);
        let k = kind_of(&nm, false);
        let given = setup.signals.iter().any(|s| s.matches(&nm));
        if k == Kind::Cmos && !setup.cmos && !given {
            continue;
        }
        add(nn, None, nm, &mut signals);
    }
    if signals.is_empty() {
        return Err("no routed signal nets".into());
    }
    let conns = connectors(board, &setup.cables);
    let diag = (bx[1] - bx[0]).hypot(bx[3] - bx[2]).max(100.0);
    if conns.is_empty() {
        notes.push("no connectors found: no cable common mode counted".into());
    } else {
        notes.push(format!(
            "common mode: {} connectors with cables ({}); each net drives the one that radiates most for it, more strongly the nearer the net runs to it",
            conns.len(),
            conns
                .iter()
                .map(|c| format!("{} {} {:.2} m", c.des, c.what, c.len))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let dirs = [directions(1.0), directions(-1.0)];
    // Per signal: contributions (traces, cable). In parallel.
    let ns = signals.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let cancel = crate::cancel::current();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let chunk = ns.div_ceil(threads).max(1);
    type Per = (Contribution, Option<Contribution>, f64);
    let mut per: Vec<Option<Per>> = vec![None; ns];
    let eval = |s: &Signal, tweak: Tweak| -> Per {
        let (drv, _, _) = driver_of(s.pos);
        let gp = net_geo(board, &planes, &geo, ncu, s.pos, drv);
        let gn = s.neg.map(|nn| net_geo(board, &planes, &geo, ncu, nn, drv));
        // Line current, A peak to peak.
        let z0 = 50.0;
        let i_pp = match (s.kind, s.neg) {
            (Kind::Rf, _) => 0.0,
            (_, Some(_)) => s.swing / (2.0 * z0),
            (_, None) => s.swing / (2.0 * z0),
        };
        let mut sg = s.clone();
        if tweak.slow {
            sg.rise *= 2.0;
        }
        let spec = current_spectrum(&sg, i_pp, &freqs);
        let skew = match (&gn, tweak.matched) {
            (Some(gn), false) => {
                let e = gp.segs.first().map_or(3.5, |x| x.eeff);
                (gn.len - gp.len) * e.sqrt() / C0
            }
            _ => 0.0,
        };
        let mut tr = vec![-200.0; n];
        let mut cbs = vec![vec![-200.0; n]; conns.len()];
        let centre = {
            let all: Vec<&Seg> = gp
                .segs
                .iter()
                .chain(gn.iter().flat_map(|g| g.segs.iter()))
                .collect();
            let w: f64 = all.iter().map(|g| g.len).sum::<f64>().max(1e-12);
            [
                all.iter().map(|g| g.mid[0] * g.len).sum::<f64>() / w / MIL,
                all.iter().map(|g| g.mid[1] * g.len).sum::<f64>() / w / MIL,
            ]
        };
        let lext = l_ext(&gp.segs, width)
            + gn.as_ref().map_or(0.0, |g| l_ext(&g.segs, width))
                * if s.neg.is_some() { 0.0 } else { 1.0 };
        for &(f, i) in &spec {
            let Some(b) = bin(f) else { continue };
            let r = lim[b].1;
            let segs_out = if tweak.inner {
                Vec::new()
            } else {
                gp.segs.clone()
            };
            let e = match &gn {
                None => trace_e(&[(&segs_out, (1.0, 0.0))], f, &dirs) * i,
                Some(gn) => {
                    let ph = -2.0 * PI * f * skew;
                    let a = -(1.0 - 0.02);
                    let negs = if tweak.inner {
                        Vec::new()
                    } else {
                        gn.segs.clone()
                    };
                    trace_e(
                        &[
                            (&segs_out, (1.0, 0.0)),
                            (&negs, (a * ph.cos(), a * ph.sin())),
                        ],
                        f,
                        &dirs,
                    ) * i
                }
            } / r;
            tr[b] = add_db(tr[b], db_uv(e));
            if !conns.is_empty() {
                // Common-mode drive: a single net's return current; a
                // pair's is its imbalance and skew share of it.
                let icm = match &gn {
                    None => i,
                    Some(_) => {
                        let ph = 2.0 * PI * f * skew;
                        let a = 1.0 - 0.02;
                        let re = 1.0 - a * ph.cos();
                        let im = a * ph.sin();
                        i * (re * re + im * im).sqrt() / 2.0
                    }
                };
                let l = if s.neg.is_some() {
                    l_ext(&gp.segs, width)
                } else {
                    lext
                };
                let v = 2.0 * PI * f * l * icm;
                for (ci, c) in conns.iter().enumerate() {
                    // The ground noise reaches a cable more fully the
                    // nearer the net runs to it (half of it from across
                    // the board).
                    let d = (centre[0] - c.at[0]).hypot(centre[1] - c.at[1]);
                    let near = 0.5 + 0.5 * (-d / (0.25 * diag)).exp();
                    let z = if tweak.choke && Some(ci) == tweak.conn {
                        600.0
                    } else {
                        0.0
                    };
                    let ec = v * near * cable_e_per_v(f, c.len, z) / r;
                    cbs[ci][b] = add_db(cbs[ci][b], db_uv(ec));
                }
            }
        }
        // The cable that radiates most for this net.
        let best = (0..conns.len()).max_by(|&a, &b| {
            let pk = |k: usize| {
                cbs[k]
                    .iter()
                    .zip(&limit)
                    .map(|(d, l)| d - l)
                    .fold(f64::MIN, f64::max)
            };
            pk(a).total_cmp(&pk(b))
        });
        let cb = best
            .map(|k| cbs[k].clone())
            .unwrap_or_else(|| vec![-200.0; n]);
        let peak = tr
            .iter()
            .zip(&limit)
            .chain(cb.iter().zip(&limit))
            .map(|(d, l)| d - l)
            .fold(f64::MIN, f64::max);
        (
            Contribution {
                source: s.name.clone(),
                mechanism: Mechanism::Trace,
                db: tr,
            },
            best.map(|k| Contribution {
                source: s.name.clone(),
                mechanism: Mechanism::Cable(conns[k].des.clone()),
                db: cb,
            }),
            peak,
        )
    };
    std::thread::scope(|sc| {
        for (q, part) in per.chunks_mut(chunk).enumerate() {
            let signals = &signals;
            let eval = &eval;
            let done = &done;
            let cancel = cancel.clone();
            sc.spawn(move || {
                let _g = crate::cancel::adopt(&cancel);
                for (m, slot) in part.iter_mut().enumerate() {
                    if crate::cancel::cancelled() {
                        return;
                    }
                    *slot = Some(eval(&signals[q * chunk + m], Tweak::default()));
                    done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    crate::cancel::check()?;
    progress(0.6);
    let mut contributions: Vec<Contribution> = Vec::new();
    for p in per.into_iter().flatten() {
        contributions.push(p.0);
        if let Some(c) = p.1 {
            contributions.push(c);
        }
    }
    // Power planes: cavity edge and decoupling loops.
    let mut rail_models: Vec<(String, crate::pdn::Model, f64, f64)> = Vec::new();
    if setup.planes {
        let mut rails: Vec<u32> = plane_nets
            .iter()
            .copied()
            .filter(|&nn| !is_ground(&name(nn)))
            .filter(|&nn| {
                let nm = name(nn).to_ascii_uppercase();
                crate::pdn::volts_in_name(&nm).is_some()
                    || [
                        "VCC", "VDD", "VEE", "VBAT", "VIN", "PWR", "3V3", "1V8", "5V", "VIO",
                        "AVDD", "DVDD", "REG",
                    ]
                    .iter()
                    .any(|k| nm.contains(k))
            })
            .collect();
        rails.sort();
        let fastest_clock = signals
            .iter()
            .filter(|s| matches!(s.kind, Kind::Clock))
            .map(|s| s.freq)
            .fold(0.0, f64::max);
        for (ri, &rn) in rails.iter().take(10).enumerate() {
            crate::cancel::check()?;
            progress(0.6 + 0.3 * ri as f64 / rails.len().clamp(1, 10) as f64);
            let rname = name(rn);
            let mut ps = crate::pdn::Setup::new(rn);
            ps.fmin = 30e6;
            ps.fmax = 3e9;
            ps.points = 90;
            let model = match crate::pdn::model(board, &ps) {
                Ok(m) => m,
                Err(e) => {
                    notes.push(format!("rail {rname}: no plane model ({e})"));
                    continue;
                }
            };
            if model.decaps.iter().all(|d| d.spare) {
                notes.push(format!(
                    "plane net {rname}: no decoupling capacitors, not taken as a rail"
                ));
                continue;
            }
            let spec = setup.rails.iter().find(|r| {
                crate::constraints::glob(&r.pattern, &rname)
                    || r.pattern.eq_ignore_ascii_case(&rname)
            });
            let core = ["INT", "CORE", "VCCINT", "0V8", "0V85", "0V9", "1V0"]
                .iter()
                .any(|k| rname.to_ascii_uppercase().contains(k));
            let amps = spec.map_or(if core { 0.5 } else { 0.1 }, |r| r.amps);
            let fsw = spec.and_then(|r| r.freq).unwrap_or(if fastest_clock > 0.0 {
                fastest_clock
            } else {
                100e6
            });
            if spec.is_none() {
                notes.push(format!(
                    "rail {rname}: switching current assumed {amps:.2} A peak to peak at {:.0} MHz, 1 ns edges (give --rail {rname}=A@MHz)",
                    fsw / 1e6
                ));
            }
            rail_models.push((rname, model, amps, fsw));
        }
    }
    for (rname, model, amps, fsw) in &rail_models {
        let (c, d) = rail_fields(
            board,
            &geo,
            model,
            *amps,
            *fsw,
            1.0,
            &freqs,
            &bin,
            &lim,
            bw.max(bh),
            rname,
            false,
        );
        contributions.push(c);
        contributions.push(d);
    }
    progress(0.92);
    // Total, worst.
    let total_of = |cs: &[Contribution]| -> Vec<f64> {
        (0..n)
            .map(|k| cs.iter().fold(-200.0, |a, c| add_db(a, c.db[k])))
            .collect()
    };
    let total = total_of(&contributions);
    let (wk, wm) = (0..n)
        .map(|k| (k, limit[k] - total[k]))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.0));
    contributions.retain(|c| c.db.iter().any(|&d| d > -199.0));
    contributions.sort_by(|a, b| {
        b.db[wk]
            .total_cmp(&a.db[wk])
            .then(b.peak(&limit).1.total_cmp(&a.peak(&limit).1))
    });
    // Fixes for the top sources at the worst frequency.
    let mut fixes = Vec::new();
    let tot_w = total[wk];
    let without = |skip: &dyn Fn(&Contribution) -> bool, extra: &[Contribution]| -> f64 {
        contributions
            .iter()
            .filter(|c| !skip(c))
            .chain(extra.iter())
            .fold(-200.0, |a, c| add_db(a, c.db[wk]))
    };
    let mut seen: HashSet<String> = HashSet::new();
    for c in contributions.iter().take(8) {
        if !seen.insert(format!("{}{:?}", c.source, c.mechanism)) {
            continue;
        }
        if let Some(s) = signals.iter().find(|s| s.name == c.source) {
            let mut try_fix = |t: Tweak, text: String| {
                let (a, b, _) = eval(s, t);
                let extra: Vec<Contribution> = std::iter::once(a).chain(b).collect();
                let after = without(
                    &|x: &Contribution| {
                        x.source == s.name
                            && matches!(x.mechanism, Mechanism::Trace | Mechanism::Cable(_))
                    },
                    &extra,
                );
                let gain = tot_w - after;
                if gain > 0.05 {
                    fixes.push(Fix {
                        source: s.name.clone(),
                        text,
                        gain_db: gain,
                    });
                }
            };
            match &c.mechanism {
                Mechanism::Trace => {
                    try_fix(
                        Tweak {
                            inner: true,
                            ..Default::default()
                        },
                        format!("route {} on an inner layer, between planes", s.name),
                    );
                    if s.neg.is_some() {
                        try_fix(
                            Tweak {
                                matched: true,
                                ..Default::default()
                            },
                            format!("match {}'s two lengths (skew to 0)", s.name),
                        );
                    } else if s.kind != Kind::Rf {
                        try_fix(
                            Tweak {
                                slow: true,
                                ..Default::default()
                            },
                            format!(
                                "slow {}'s edges to {:.2} ns (series resistor or drive strength)",
                                s.name,
                                s.rise * 2e9
                            ),
                        );
                    }
                }
                Mechanism::Cable(j) => {
                    try_fix(
                        Tweak {
                            choke: true,
                            conn: conns.iter().position(|c| &c.des == j),
                            ..Default::default()
                        },
                        format!("common-mode choke or ferrite (600 Ω) on the cable at {j}"),
                    );
                    if s.neg.is_some() {
                        try_fix(
                            Tweak {
                                matched: true,
                                ..Default::default()
                            },
                            format!("match {}'s two lengths (less common mode)", s.name),
                        );
                    }
                }
                _ => {}
            }
        } else if let Some((rname, model, amps, fsw)) = rail_models.iter().find(|r| r.0 == c.source)
        {
            let (cc, dd) = rail_fields(
                board,
                &geo,
                model,
                *amps,
                *fsw,
                1.0,
                &freqs,
                &bin,
                &lim,
                bw.max(bh),
                rname,
                true,
            );
            let after = without(&|x: &Contribution| x.source == *rname, &[cc, dd]);
            let gain = tot_w - after;
            if gain > 0.05 {
                fixes.push(Fix {
                    source: rname.clone(),
                    text: format!("add two more capacitors like {}'s on {rname} near the load, vias in the pads", model.decaps.first().map_or("the first".to_string(), |d| d.designator.clone())),
                    gain_db: gain,
                });
            }
        }
    }
    fixes.sort_by(|a, b| b.gain_db.total_cmp(&a.gain_db));
    let mut seen_fix: HashSet<String> = HashSet::new();
    fixes.retain(|f| seen_fix.insert(f.text.clone()));
    let assumed = signals.iter().filter(|s| s.assumed).count();
    if assumed > 0 {
        notes.push(format!(
            "{assumed} of {} sources use defaults for their kind (give NET=RATE@EDGE,SWING lines or IBIS models for real drivers)",
            signals.len()
        ));
    }
    notes.push("cables are modelled as monopoles against the board, the board's own size as an antenna is not; inner traces radiate only through the planes and cables".into());
    // Near field at the worst frequency.
    let map = near_map(
        board,
        &planes,
        &geo,
        ncu,
        &signals,
        &freqs[wk],
        setup.map_cell,
        &driver_of,
        bx,
        &|s: &Signal, f: f64| {
            let i_pp = if s.kind == Kind::Rf {
                0.0
            } else {
                s.swing / 100.0
            };
            let sp = current_spectrum(s, i_pp, &[f * 0.999, f, f * 1.001]);
            // The line nearest f (a clock's harmonic within the bin) or the
            // density there.
            sp.iter()
                .filter(|(ff, _)| bin(*ff) == bin(f))
                .map(|x| x.1)
                .fold(0.0, f64::max)
        },
    );
    progress(1.0);
    Ok(SimResult {
        freqs: freqs.clone(),
        limit,
        limit_kind: setup.limit,
        margin_db: setup.margin_db,
        total,
        contributions,
        signals,
        connectors: conns
            .iter()
            .map(|c| (c.des.clone(), c.what.clone(), c.len))
            .collect(),
        fixes,
        worst: (freqs[wk], wm),
        map,
        notes,
    })
}

#[derive(Clone, Copy, Default)]
struct Tweak {
    inner: bool,
    matched: bool,
    slow: bool,
    choke: bool,
    /// The connector the choke goes on.
    conn: Option<usize>,
}

/// The cavity edge and decoupling loops of one rail.
#[allow(clippy::too_many_arguments)]
fn rail_fields(
    board: &Board,
    geo: &[crate::impedance::LayerGeometry],
    model: &crate::pdn::Model,
    amps: f64,
    fsw: f64,
    _k: f64,
    freqs: &[f64],
    bin: &dyn Fn(f64) -> Option<usize>,
    lim: &[(f64, f64)],
    span_m: f64,
    rname: &str,
    more_caps: bool,
) -> (Contribution, Contribution) {
    let n = freqs.len();
    let built = model.as_built();
    let mut caps = built.clone();
    if more_caps {
        if let Some(m) = built.iter().flatten().next() {
            caps.push(Some(*m));
            caps.push(Some(*m));
        }
    }
    // |Z| at the first load, log-interpolated over the model's sweep;
    // two more capacitors are taken as two more such branches beside it.
    let curves = model.evaluate(&built);
    let zc = curves.first().cloned().unwrap_or_default();
    let mf = &model.freqs;
    let z_at = |f: f64| -> f64 {
        if mf.is_empty() {
            return 0.0;
        }
        if f <= mf[0] {
            return zc[0];
        }
        let k = mf.partition_point(|&x| x < f);
        if k >= mf.len() {
            return *zc.last().unwrap_or(&0.0);
        }
        let t = (f / mf[k - 1]).ln() / (mf[k] / mf[k - 1]).ln();
        (zc[k - 1].ln() * (1.0 - t) + zc[k].ln() * t).exp()
    };
    let branch = |m: &crate::pdn::CapModel, mount: f64, f: f64| -> f64 {
        let w = 2.0 * PI * f;
        let x = w * (m.esl + mount) - 1.0 / (w * m.farads);
        m.esr.hypot(x)
    };
    let extra = |f: f64, z: f64| -> f64 {
        if !more_caps {
            return z;
        }
        let Some((d, m)) = model
            .decaps
            .iter()
            .zip(&built)
            .find_map(|(d, m)| m.map(|m| (d, m)))
        else {
            return z;
        };
        let zb = branch(&m, d.mount, f);
        1.0 / (1.0 / z + 2.0 / zb)
    };
    // The cavity's size: the plane's area as a square.
    let side =
        (model.cavity.farads / (8.854e-12 * model.cavity.er.max(1.0)) * model.cavity.d_mil * MIL)
            .max(0.0)
            .sqrt()
            .min(span_m.max(0.01));
    let perim = 4.0 * side;
    // Depth from the board's surface to the plane, for the caps' loops.
    let depth_of = |layer: &str, bottom: bool| -> f64 {
        let i = geo.iter().position(|g| g.name == layer).unwrap_or(1);
        let mut z = 0.0;
        if bottom {
            for g in geo.iter().skip(i + 1) {
                z += g.t + g.above.unwrap_or(0.0);
            }
            z += geo.get(i + 1).map_or(0.0, |_| 0.0);
        } else {
            for g in geo.iter().take(i) {
                z += g.t + g.below.unwrap_or(0.0);
            }
        }
        (z * MIL).max(0.1e-3)
    };
    let sinc = |x: f64| if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
    let mut cav = vec![-200.0; n];
    let mut dec = vec![-200.0; n];
    let mut h = 1usize;
    while h as f64 * fsw <= freqs[n - 1] && h < 4000 {
        let f = h as f64 * fsw;
        h += 1;
        let Some(b) = bin(f) else { continue };
        let r = lim[b].1;
        let i =
            amps * sinc((h - 1) as f64 * PI / 2.0).abs() * sinc(PI * f * 1e-9).abs() / 2f64.sqrt();
        if i <= 0.0 {
            continue;
        }
        let z = extra(f, z_at(f));
        // The die's own capacitance behind the package inductance takes
        // most of the fast current; the plane sees the rest.
        let w = 2.0 * PI * f;
        let c_die = (20e-9 * amps).max(1e-9);
        let zd = 5e-3f64.hypot(1.0 / (w * c_die));
        let zp = w * 0.3e-9;
        let x = zp - 1.0 / (w * c_die);
        let i = i * zd / (5e-3f64.hypot(x).hypot(z)).max(1e-9);
        let v = z * i;
        let k = 2.0 * PI * f / C0;
        let le = (perim / 4.0).min(C0 / f / 2.0);
        let e = k * v * le / (PI * r);
        cav[b] = add_db(cav[b], db_uv(e));
        // Each capacitor's current in its loop.
        let mut p2 = 0.0;
        for (d, m) in model.decaps.iter().zip(&caps) {
            let Some(m) = m else { continue };
            let ib = v / branch(m, d.mount, f);
            let bottom = board
                .components
                .get(d.part)
                .is_some_and(|c| c.side == crate::Side::Bottom);
            let pitch = match d.package {
                crate::pdn::Package::P0201 => 20.0,
                crate::pdn::Package::P0402 => 40.0,
                crate::pdn::Package::P0603 => 60.0,
                crate::pdn::Package::P0805 => 80.0,
                _ => 120.0,
            } * MIL;
            let area = pitch * depth_of(&model.cavity.power_layer, bottom);
            let e = 2.63e-14 * f * f * area * ib / r;
            p2 += e * e;
        }
        if more_caps {
            // The two added ones, like the first.
            if let Some((d, Some(m))) = model.decaps.iter().zip(&built).next() {
                let ib = v / branch(m, d.mount, f);
                let e =
                    2.63e-14 * f * f * 40.0 * MIL * depth_of(&model.cavity.power_layer, false) * ib
                        / r;
                p2 += 2.0 * e * e;
            }
        }
        dec[b] = add_db(dec[b], db_uv(p2.sqrt()));
    }
    (
        Contribution {
            source: rname.to_string(),
            mechanism: Mechanism::Cavity,
            db: cav,
        },
        Contribution {
            source: rname.to_string(),
            mechanism: Mechanism::Decaps,
            db: dec,
        },
    )
}

/// The near field at f: |H| 1 mm above the top and below the bottom
/// (each outer segment and its image in the plane, Biot-Savart, the
/// travelling wave's phase along each net, nets added as power), and the
/// surface current density on each copper layer (the track's current
/// over its width plus its return spread in the plane under it,
/// J = I / (π h (1 + (d/h)²))).
#[allow(clippy::too_many_arguments)]
fn near_map(
    board: &Board,
    planes: &Planes,
    geo: &[crate::impedance::LayerGeometry],
    ncu: usize,
    signals: &[Signal],
    f: &f64,
    cell: f64,
    driver_of: &dyn Fn(u32) -> ([f64; 2], Option<u32>, String),
    bx: [f64; 4],
    current_at: &dyn Fn(&Signal, f64) -> f64,
) -> Option<NearMap> {
    let f = *f;
    let (wm, hm) = (bx[1] - bx[0], bx[3] - bx[2]);
    let cell = if cell > 0.0 {
        cell
    } else {
        (wm.max(hm) / 300.0).max(5.0)
    };
    let (w, h) = (
        (wm / cell).ceil() as usize + 1,
        (hm / cell).ceil() as usize + 1,
    );
    if w * h > 4_000_000 || w < 2 || h < 2 {
        return None;
    }
    let k = 2.0 * PI * f / C0;
    let z_obs = 1e-3;
    let mut h_top = vec![0.0f64; w * h];
    let mut h_bot = vec![0.0f64; w * h];
    let mut js: Vec<Vec<f64>> = vec![vec![0.0; w * h]; ncu];
    let reach = 4.0 * z_obs / MIL; // mil, where a segment's H still counts
    for s in signals {
        let i0 = current_at(s, f);
        if i0 <= 0.0 {
            continue;
        }
        let (drv, _, _) = driver_of(s.pos);
        let lines: Vec<(NetGeo, f64)> =
            std::iter::once((net_geo(board, planes, geo, ncu, s.pos, drv), 1.0))
                .chain(
                    s.neg
                        .map(|nn| (net_geo(board, planes, geo, ncu, nn, drv), -1.0)),
                )
                .collect();
        // Per net: complex H (3 components) per cell, then power-added.
        let mut ht: HashMap<usize, [f64; 6]> = HashMap::new();
        let mut hb: HashMap<usize, [f64; 6]> = HashMap::new();
        for (ng, sign) in &lines {
            for g in &ng.segs {
                let beta = k * g.eeff.sqrt();
                let (sn, cs) = (beta * g.s).sin_cos();
                let (ire, iim) = (sign * i0 * cs, -sign * i0 * sn);
                // Current density on its layer and the plane it returns in.
                let refs = plane_refs(planes, geo, g.layer);
                let hm_ = g.h / MIL;
                let a = [
                    g.mid[0] / MIL - g.dir[0] * g.len / MIL / 2.0,
                    g.mid[1] / MIL - g.dir[1] * g.len / MIL / 2.0,
                ];
                let b = [
                    g.mid[0] / MIL + g.dir[0] * g.len / MIL / 2.0,
                    g.mid[1] / MIL + g.dir[1] * g.len / MIL / 2.0,
                ];
                let r_spread = (4.0 * hm_).max(cell);
                let (ia, ja) = (
                    (((a[0].min(b[0]) - r_spread - bx[0]) / cell)
                        .floor()
                        .max(0.0)) as usize,
                    (((a[1].min(b[1]) - r_spread - bx[2]) / cell)
                        .floor()
                        .max(0.0)) as usize,
                );
                let (ib, jb) = (
                    ((((a[0].max(b[0]) + r_spread - bx[0]) / cell).ceil()) as usize).min(w - 1),
                    ((((a[1].max(b[1]) + r_spread - bx[2]) / cell).ceil()) as usize).min(h - 1),
                );
                for j in ja..=jb {
                    for i in ia..=ib {
                        let p = [bx[0] + i as f64 * cell, bx[2] + j as f64 * cell];
                        let d = crate::route::point_seg(p, a, b);
                        let q = j * w + i;
                        if d <= g.width / MIL / 2.0 + cell / 2.0 {
                            js[g.layer][q] += (i0 / g.width.max(1e-5)).powi(2);
                        }
                        if let Some(&(pl, hh)) = refs.first() {
                            let jr = i0 / (PI * hh * MIL * (1.0 + (d / hh).powi(2)));
                            if pl < ncu {
                                js[pl][q] += jr * jr;
                            }
                        }
                    }
                }
                if g.side == 0 {
                    continue;
                }
                // H 1 mm off the surface: the segment and its image.
                let reach_c = (reach / cell).ceil() as isize;
                let (ci, cj) = (
                    ((g.mid[0] / MIL - bx[0]) / cell) as isize,
                    ((g.mid[1] / MIL - bx[2]) / cell) as isize,
                );
                let half = (g.len / MIL / 2.0 / cell).ceil() as isize + reach_c;
                let target = if g.side > 0 { &mut ht } else { &mut hb };
                for j in (cj - half).max(0)..=(cj + half).min(h as isize - 1) {
                    for i in (ci - half).max(0)..=(ci + half).min(w as isize - 1) {
                        let p = [
                            (bx[0] + i as f64 * cell) * MIL,
                            (bx[2] + j as f64 * cell) * MIL,
                        ];
                        let mut hv = [0.0f64; 3];
                        for (zsrc, sgn) in [(0.0, 1.0), (-2.0 * g.h, -1.0)] {
                            let hh = biot_savart(g, p, z_obs - zsrc);
                            for c in 0..3 {
                                hv[c] += sgn * hh[c];
                            }
                        }
                        let e = target
                            .entry(j as usize * w + i as usize)
                            .or_insert([0.0; 6]);
                        for c in 0..3 {
                            e[2 * c] += hv[c] * ire;
                            e[2 * c + 1] += hv[c] * iim;
                        }
                    }
                }
            }
        }
        for (src, dst) in [(&ht, &mut h_top), (&hb, &mut h_bot)] {
            for (&q, v) in src.iter() {
                dst[q] += v.iter().map(|x| x * x).sum::<f64>();
            }
        }
    }
    let mut maps = vec![
        (
            "|H| 1 mm above the top".to_string(),
            "A/m",
            h_top.iter().map(|v| v.sqrt() as f32).collect(),
        ),
        (
            "|H| 1 mm below the bottom".to_string(),
            "A/m",
            h_bot.iter().map(|v| v.sqrt() as f32).collect(),
        ),
    ];
    for (l, j) in js.iter().enumerate() {
        if j.iter().any(|&v| v > 0.0) {
            let nm = super::si::layer_name(board, l);
            maps.push((
                format!("current density on {nm}"),
                "A/m",
                j.iter().map(|v| v.sqrt() as f32).collect(),
            ));
        }
    }
    Some(NearMap {
        f,
        x0: bx[0],
        y0: bx[2],
        cell,
        w,
        h,
        maps,
    })
}

/// H (A/m per A) at p (m, on the plane z above the segment) from a
/// straight current element, Biot-Savart for a finite segment.
fn biot_savart(g: &Seg, p: [f64; 2], z: f64) -> [f64; 3] {
    let a = [
        g.mid[0] - g.dir[0] * g.len / 2.0,
        g.mid[1] - g.dir[1] * g.len / 2.0,
        0.0,
    ];
    let r = [p[0] - a[0], p[1] - a[1], z];
    let u = [g.dir[0], g.dir[1], 0.0];
    let along = r[0] * u[0] + r[1] * u[1];
    let perp = [r[0] - along * u[0], r[1] - along * u[1], r[2]];
    let d = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2])
        .sqrt()
        .max(1e-5);
    let c1 = along / (along * along + d * d).sqrt();
    let rem = along - g.len;
    let c2 = rem / (rem * rem + d * d).sqrt();
    let mag = (c1 - c2) / (4.0 * PI * d);
    // Direction u × perp̂.
    let dirv = [
        u[1] * perp[2] - u[2] * perp[1],
        u[2] * perp[0] - u[0] * perp[2],
        u[0] * perp[1] - u[1] * perp[0],
    ];
    [dirv[0] / d * mag, dirv[1] / d * mag, dirv[2] / d * mag]
}

// ---------------------------------------------------------------------
// Reports.

impl SimResult {
    pub fn margin_at_worst(&self) -> f64 {
        self.worst.1
    }

    /// Each source's worst margin, by source and mechanism, worst first.
    pub fn ranked(&self) -> Vec<(&Contribution, f64, f64)> {
        let mut v: Vec<(&Contribution, f64, f64)> = self
            .contributions
            .iter()
            .map(|c| {
                let (k, m) = c.peak(&self.limit);
                (c, self.freqs[k], -m)
            })
            .collect();
        v.sort_by(|a, b| a.2.total_cmp(&b.2));
        v
    }

    pub fn report(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "EMI simulation against {}: worst margin {:+.1} dB at {:.1} MHz ({})",
            self.limit_kind.name(),
            self.worst.1,
            self.worst.0 / 1e6,
            if self.worst.1 < 0.0 {
                "FAILS"
            } else if self.worst.1 < self.margin_db {
                "passes, inside the margin wanted"
            } else {
                "passes"
            }
        );
        let _ = writeln!(
            s,
            "{} sources ({} from IBIS or given)",
            self.signals.len(),
            self.signals.iter().filter(|x| !x.assumed).count()
        );
        let _ = writeln!(
            s,
            "\nTop contributions (margin to the limit at each one's worst frequency):"
        );
        for (c, f, m) in self.ranked().iter().take(25) {
            let _ = writeln!(
                s,
                "  {:>+7.1} dB  {:>8.1} MHz  {:<20} {}",
                m,
                f / 1e6,
                c.mechanism.label(),
                c.source
            );
        }
        if !self.fixes.is_empty() {
            let _ = writeln!(
                s,
                "\nFixes, by what they gain at {:.1} MHz:",
                self.worst.0 / 1e6
            );
            for x in &self.fixes {
                let _ = writeln!(s, "  {:>5.1} dB  {}", x.gain_db, x.text);
            }
        }
        let _ = writeln!(s, "\nSources:");
        for x in &self.signals {
            let _ = writeln!(
                s,
                "  {:<28} {:<11} {}  {}",
                x.name,
                x.kind.label(),
                match x.kind {
                    Kind::Rf => format!("{:.3} GHz, {:.1} dBm", x.freq / 1e9, x.swing),
                    k if k.is_data() => format!(
                        "{:.3} Gb/s, {:.0} ps, {:.2} V",
                        x.freq / 1e9,
                        x.rise * 1e12,
                        x.swing
                    ),
                    _ => format!(
                        "{:.3} MHz, {:.2} ns, {:.2} V",
                        x.freq / 1e6,
                        x.rise * 1e9,
                        x.swing
                    ),
                },
                x.origin
            );
        }
        for c in &self.connectors {
            let _ = writeln!(s, "Connector {}: {} {:.2} m", c.0, c.1, c.2);
        }
        for n in &self.notes {
            let _ = writeln!(s, "Note: {n}");
        }
        s
    }

    /// The spectrum: the limit, the total and the top sources stacked
    /// (each drawn as the running power sum, largest last).
    pub fn svg(&self, title: &str) -> String {
        let (w, h) = (1100.0, 560.0);
        let (l, r, t, b) = (70.0, 300.0, 50.0, 60.0);
        let pw = w - l - r;
        let ph = h - t - b;
        let fx = |f: f64| l + pw * (f / 30e6).ln() / (6e9f64 / 30e6).ln();
        let lo = self.limit.iter().cloned().fold(f64::MAX, f64::min) - 60.0;
        let hi = self
            .limit
            .iter()
            .chain(self.total.iter())
            .cloned()
            .fold(f64::MIN, f64::max)
            + 10.0;
        let lo = (lo / 10.0).floor() * 10.0;
        let hi = (hi / 10.0).ceil() * 10.0;
        let fy = |d: f64| t + ph * (1.0 - (d.max(lo) - lo) / (hi - lo));
        let mut s = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" font-family="sans-serif" font-size="11">
<rect width="100%" height="100%" fill="white"/>
<text x="{l}" y="24" font-size="15" font-weight="bold">{} — {} (worst {:+.1} dB at {:.0} MHz)</text>
"#,
            esc(title),
            self.limit_kind.name(),
            self.worst.1,
            self.worst.0 / 1e6
        );
        let mut d = lo;
        while d <= hi + 1e-9 {
            let y = fy(d);
            let _ = writeln!(
                s,
                r##"<line x1="{l}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="#e4e4e4"/><text x="{:.1}" y="{:.1}" text-anchor="end">{d:.0}</text>"##,
                l + pw,
                l - 6.0,
                y + 4.0
            );
            d += 10.0;
        }
        for f in [30e6, 50e6, 100e6, 200e6, 500e6, 1e9, 2e9, 3e9, 6e9] {
            let x = fx(f);
            let _ = writeln!(
                s,
                r##"<line x1="{x:.1}" y1="{t}" x2="{x:.1}" y2="{:.1}" stroke="#e4e4e4"/><text x="{x:.1}" y="{:.1}" text-anchor="middle">{}</text>"##,
                t + ph,
                t + ph + 16.0,
                if f >= 1e9 {
                    format!("{} GHz", f / 1e9)
                } else {
                    format!("{} MHz", f / 1e6)
                }
            );
        }
        let _ = writeln!(
            s,
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle">frequency</text><text x="16" y="{:.1}" transform="rotate(-90 16 {:.1})" text-anchor="middle">dBµV/m at the limit's distance</text>"#,
            l + pw / 2.0,
            h - 14.0,
            t + ph / 2.0,
            t + ph / 2.0
        );
        // Stacked: the top sources' running sums, largest first so it
        // is drawn at the bottom of the stack... draw each source alone.
        let palette = [
            "#2a6fdb", "#e0663d", "#3aa45b", "#9b59b6", "#d4a017", "#17a2b8", "#c0392b", "#7f8c8d",
        ];
        let top: Vec<&Contribution> = self.ranked().iter().take(8).map(|x| x.0).collect();
        for (i, c) in top.iter().enumerate() {
            let mut pth = String::new();
            for (k, &v) in c.db.iter().enumerate() {
                if v <= -199.0 {
                    continue;
                }
                let x = fx(self.freqs[k]);
                let _ = write!(pth, "M{x:.1},{:.1}L{x:.1},{:.1}", fy(lo), fy(v));
            }
            let col = palette[i % palette.len()];
            let _ = writeln!(
                s,
                r#"<path d="{pth}" stroke="{col}" stroke-width="1.2" opacity="0.75"/>"#
            );
            let y = t + 14.0 + i as f64 * 18.0;
            let _ = writeln!(
                s,
                r#"<rect x="{:.1}" y="{:.1}" width="12" height="10" fill="{col}"/><text x="{:.1}" y="{:.1}">{} ({})</text>"#,
                l + pw + 14.0,
                y - 9.0,
                l + pw + 30.0,
                y,
                esc(&c.source),
                esc(&c.mechanism.label())
            );
        }
        // Total envelope and the limit.
        let line = |v: &[f64]| -> String {
            let mut p = String::new();
            let mut pen = false;
            for (k, &d) in v.iter().enumerate() {
                if d <= -199.0 {
                    pen = false;
                    continue;
                }
                let _ = write!(
                    p,
                    "{}{:.1},{:.1}",
                    if pen { "L" } else { "M" },
                    fx(self.freqs[k]),
                    fy(d)
                );
                pen = true;
            }
            p
        };
        let _ = writeln!(
            s,
            r#"<path d="{}" fill="none" stroke="black" stroke-width="1"/>"#,
            line(&self.total)
        );
        let _ = writeln!(
            s,
            r##"<path d="{}" fill="none" stroke="#d62728" stroke-width="2.2"/>"##,
            line(&self.limit)
        );
        let mlim: Vec<f64> = self.limit.iter().map(|x| x - self.margin_db).collect();
        let _ = writeln!(
            s,
            r##"<path d="{}" fill="none" stroke="#d62728" stroke-width="1" stroke-dasharray="5 4"/>"##,
            line(&mlim)
        );
        let y = t + 14.0 + top.len() as f64 * 18.0 + 10.0;
        let _ = writeln!(
            s,
            r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#d62728" stroke-width="2.2"/><text x="{:.1}" y="{:.1}">limit (dashed: less the margin)</text>"##,
            l + pw + 14.0,
            y - 4.0,
            l + pw + 26.0,
            y - 4.0,
            l + pw + 30.0,
            y
        );
        let _ = writeln!(
            s,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="black"/><text x="{:.1}" y="{:.1}">total of all sources</text>"#,
            l + pw + 14.0,
            y + 14.0,
            l + pw + 26.0,
            y + 14.0,
            l + pw + 30.0,
            y + 18.0
        );
        s.push_str("</svg>\n");
        s
    }

    /// The near-field maps as a page: each map on the board outline, its
    /// own dB scale 40 dB deep.
    pub fn map_svg(&self, board: &Board, title: &str) -> Option<String> {
        let m = self.map.as_ref()?;
        let mut body = String::new();
        let mut y = 60.0;
        let width = 640usize;
        let outline: Vec<[f64; 2]> = board.geometry.outline.iter().map(|&p| pt(p)).collect();
        for (name, unit, vals) in &m.maps {
            let top = vals.iter().cloned().fold(0.0f32, f32::max) as f64;
            if top.is_nan() || top <= 0.0 {
                continue;
            }
            let img = m.raster(vals, top, 40.0, width, &outline);
            let _ = writeln!(
                body,
                r#"<text x="20" y="{:.0}" font-size="13">{} (peak {} {unit}; 0 to −40 dB)</text><image x="20" y="{:.0}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                y,
                esc(name),
                crate::emfield::si(top),
                y + 6.0,
                img.w,
                img.h,
                crate::emfield::base64(&crate::emfield::png(&img))
            );
            y += img.h as f64 + 30.0;
        }
        Some(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{y:.0}" font-family="sans-serif"><rect width="100%" height="100%" fill="white"/><text x="20" y="28" font-size="16" font-weight="bold">{} — near field at {:.1} MHz (the worst frequency)</text><text x="20" y="46" font-size="11" fill="gray">rms, from the sources' currents at that frequency; nets added as power; bright is strong.</text>
{body}</svg>
"#,
            width + 40,
            esc(title),
            m.f / 1e6
        ))
    }
}

impl NearMap {
    /// A map as a picture `width` px across (top row at the board's top),
    /// the outline in white.
    pub fn raster(
        &self,
        vals: &[f32],
        top: f64,
        range: f64,
        width: usize,
        outline: &[[f64; 2]],
    ) -> crate::emfield::Raster {
        let sc = width as f64 / self.w as f64;
        let hgt = ((self.h as f64 * sc).round() as usize).max(4);
        let mut r = crate::emfield::Raster::new(width, hgt, [0, 0, 4]);
        for py in 0..hgt {
            let j = self.h - 1 - ((py as f64 / sc) as usize).min(self.h - 1);
            for px in 0..width {
                let i = ((px as f64 / sc) as usize).min(self.w - 1);
                let v = vals[j * self.w + i] as f64;
                let c = crate::emfield::colour(crate::emfield::scale_t(v, top, range, 20.0));
                r.put(px as i64, py as i64, c);
            }
        }
        let to = |p: [f64; 2]| {
            [
                (p[0] - self.x0) / self.cell * sc,
                hgt as f64 - (p[1] - self.y0) / self.cell * sc,
            ]
        };
        for k in 0..outline.len() {
            let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
            r.line(to(a), to(b), [230, 230, 240]);
        }
        r
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(x: f64, len: f64, h: f64) -> Seg {
        Seg {
            mid: [x, 0.0],
            dir: [1.0, 0.0],
            len,
            h,
            side: 1,
            s: 0.0,
            layer: 0,
            width: 1e-4,
            eeff: 1.0,
        }
    }

    #[test]
    fn one_small_loop_is_otts_formula() {
        // 1 cm of trace 1 mm over the plane, 1 A at 100 MHz, 1 m away.
        let d = [directions(1.0), directions(-1.0)];
        let g = [seg(0.0, 0.01, 0.001)];
        let e = trace_e(&[(&g, (1.0, 0.0))], 100e6, &d);
        let ott = 2.63e-14 * 1e16 * 1e-5 * 1.0;
        assert!((e / ott - 1.0).abs() < 0.01, "{e} vs {ott}");
    }

    #[test]
    fn a_balanced_pair_radiates_far_less_than_one_line() {
        let d = [directions(1.0), directions(-1.0)];
        let p: Vec<Seg> = (0..20)
            .map(|k| seg(k as f64 * 0.005, 0.005, 1e-4))
            .collect();
        let nn: Vec<Seg> = p
            .iter()
            .map(|g| Seg {
                mid: [g.mid[0], 2e-4],
                ..*g
            })
            .collect();
        let one = trace_e(&[(&p, (1.0, 0.0))], 1e9, &d);
        let pair = trace_e(&[(&p, (1.0, 0.0)), (&nn, (-1.0, 0.0))], 1e9, &d);
        let skewed = trace_e(
            &[
                (&p, (1.0, 0.0)),
                (&nn, (-0.98 * (0.3f64).cos(), -0.98 * (0.3f64).sin())),
            ],
            1e9,
            &d,
        );
        assert!(pair < one / 100.0, "pair {pair} one {one}");
        assert!(skewed > pair * 10.0 && skewed < one, "skewed {skewed}");
    }

    #[test]
    fn names_give_frequencies() {
        assert_eq!(freq_in_name("REF_CLK_245M76"), Some(245.76e6));
        assert_eq!(freq_in_name("CLK125M"), None);
        assert_eq!(freq_in_name("CLK_125M"), Some(125e6));
        assert_eq!(freq_in_name("OSC_100MHZ"), Some(100e6));
        assert_eq!(freq_in_name("FPGA_CLK"), None);
    }

    #[test]
    fn biot_savart_long_line() {
        // Long line: H = I / (2π d).
        let g = Seg {
            mid: [0.0, 0.0],
            len: 100.0,
            ..seg(0.0, 100.0, 1e-3)
        };
        let h = biot_savart(&g, [0.0, 0.0], 0.01);
        let m = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
        assert!((m * 2.0 * PI * 0.01 - 1.0).abs() < 0.01, "{m}");
    }
}
