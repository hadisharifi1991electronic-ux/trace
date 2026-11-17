//! Bus simulation: a parallel bus (DDR data lanes, an address bus, any
//! source-synchronous group) simulated bit by bit, with timing against
//! its strobe or clock.
//!
//! Each bit's routed path is cascaded from the board (as
//! [`crate::pathloss`] does: every trace, via and part), and its eye is
//! built with the nets beside it switching as aggressors (the other bits
//! of the bus among them; [`crate::eye`]). The strobe gets the same
//! treatment, sending a clock. From each eye comes the window in which
//! the bit is valid at the receiver, measured from the moment the bit
//! leaves the driver: it closes with the loss, the reflections and the
//! crosstalk, and moves with the flight time. The strobe's edge lands at
//! its own flight time plus its launch offset (half a bit for a strobe
//! centred in the data, as DDR writes send it and DDR reads delay it),
//! with its own jitter. Setup and hold margins follow per bit, and the
//! receiver mask (DDR4 and later) is checked at the strobe.
//!
//! Three margins are given for the whole bus: as routed (the strobe
//! where its launch offset puts it), with the strobe centred on the
//! whole lane (read/write training per byte), and with every bit
//! deskewed on its own (per-bit training, where the controller has it).

use crate::eye::{self, EyeResult, EyeSetup};
use crate::pathloss::{self, Options, Report};
use crate::Board;
use std::fmt::Write;

/// What to simulate.
#[derive(Clone, Debug)]
pub struct BusSetup {
    /// Data nets: names or patterns (* and ?), several allowed.
    pub nets: Vec<String>,
    /// The strobe or clock: a net, or a pair by its name or either line.
    pub strobe: Option<String>,
    /// The driving part (the controller for writes, the memory for
    /// reads), by designator.
    pub from: String,
    /// The receiving part; None takes the farthest pin of each net.
    pub to: Option<String>,
    /// Transfers per second, millions (DDR4-3200: 3200).
    pub mts: f64,
    /// Swing the driver puts into a matched load, volts peak to peak.
    pub swing: f64,
    /// Edge, 20 to 80 %, ps (0: 30 % of a bit).
    pub rise_ps: f64,
    pub prbs: u32,
    /// Receiver setup and hold, ps (taken from the strobe's edge).
    pub setup_ps: f64,
    pub hold_ps: f64,
    /// Receiver mask: height (volts) and width (share of a bit), as
    /// DDR4's VdIVW and TdIVW.
    pub mask: Option<(f64, f64)>,
    /// When the strobe leaves the driver after the data bit starts, in
    /// bits: 0.5 for a strobe centred in the data, 0 edge aligned.
    pub strobe_offset_ui: f64,
    /// Nets beside each bit switching as aggressors.
    pub xtalk: bool,
    /// Solve each kind of via in 3D (slower).
    pub vias_3d: bool,
}

impl Default for BusSetup {
    fn default() -> Self {
        BusSetup {
            nets: Vec::new(),
            strobe: None,
            from: String::new(),
            to: None,
            mts: 1600.0,
            swing: 0.8,
            rise_ps: 0.0,
            prbs: 7,
            setup_ps: 0.0,
            hold_ps: 0.0,
            mask: None,
            strobe_offset_ui: 0.5,
            xtalk: true,
            vias_3d: false,
        }
    }
}

/// One bit of the bus.
#[derive(Clone, Debug, Default)]
pub struct Bit {
    pub net: String,
    pub from: String,
    pub to: String,
    /// Trace length, mil.
    pub length: f64,
    /// Insertion loss at half the transfer rate, dB.
    pub il_db: f64,
    /// Flight time to the middle of the eye less half a bit, ps.
    pub flight_ps: f64,
    /// Eye at its best sampling point.
    pub height: f64,
    pub width_ps: f64,
    pub jitter_ps: f64,
    /// Worst case height with every aggressor at its worst, volts.
    pub worst: f64,
    /// Summed crosstalk closure, volts.
    pub xtalk: f64,
    /// The worst aggressor.
    pub worst_aggressor: String,
    /// The valid window at the receiver, ps after the bit leaves the
    /// driver: [earliest end of the previous bit's last transition,
    /// earliest start of the next bit's].
    pub window: [f64; 2],
    /// Margins against the strobe, ps (None without a strobe).
    pub setup: Option<f64>,
    pub hold: Option<f64>,
    /// The eye's height over the mask's width at the strobe, volts.
    pub mask_height: Option<f64>,
    pub eye: EyeResult,
}

impl Bit {
    pub fn passes(&self, s: &BusSetup) -> bool {
        self.height > 0.0
            && self.setup.is_none_or(|v| v >= 0.0)
            && self.hold.is_none_or(|v| v >= 0.0)
            && match (self.mask_height, s.mask) {
                (Some(h), Some((v, _))) => h >= v,
                _ => true,
            }
    }
}

/// The strobe or clock.
#[derive(Clone, Debug, Default)]
pub struct Strobe {
    pub net: String,
    pub from: String,
    pub to: String,
    pub length: f64,
    pub flight_ps: f64,
    /// Its edge at the receiver, ps after the data bit leaves the
    /// driver (flight time plus launch offset).
    pub edge_ps: f64,
    /// Spread of its edges (crosstalk and ringing), ps peak to peak.
    pub jitter_ps: f64,
    pub eye: EyeResult,
}

/// Margin of the whole bus: setup and hold, ps, and the bit that sets
/// each.
#[derive(Clone, Debug, Default)]
pub struct Margin {
    pub setup: f64,
    pub hold: f64,
    pub setup_bit: String,
    pub hold_bit: String,
}

impl Margin {
    pub fn total(&self) -> f64 {
        self.setup + self.hold
    }
}

#[derive(Clone, Debug, Default)]
pub struct BusResult {
    pub mts: f64,
    pub ui_ps: f64,
    pub bits: Vec<Bit>,
    pub strobe: Option<Strobe>,
    /// Nets left out, with why.
    pub skipped: Vec<(String, String)>,
    /// Where every bit is valid, ps after launch (start, end).
    pub common: [f64; 2],
    /// As routed (needs the strobe), with the strobe centred on the
    /// lane, and with each bit deskewed.
    pub routed: Option<Margin>,
    pub trained: Margin,
    pub deskewed: Margin,
    pub setup_ps: f64,
    pub hold_ps: f64,
    pub mask: Option<(f64, f64)>,
    pub swing: f64,
    pub notes: Vec<String>,
}

/// Net order by name with numbers compared as numbers (DQ2 before DQ10).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let mut out: Vec<(String, u64)> = Vec::new();
        let mut text = String::new();
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                if !num.is_empty() {
                    out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                    num.clear();
                }
                text.push(c.to_ascii_uppercase());
            }
        }
        out.push((text, num.parse().unwrap_or(0)));
        out
    };
    key(a).cmp(&key(b))
}

/// The nets that the patterns name, in natural order.
pub fn expand(board: &Board, patterns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in patterns.iter().flat_map(|p| p.split(',')) {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        for n in &board.nets {
            let hit = if p.contains('*') || p.contains('?') {
                crate::constraints::glob(p, &n.name)
            } else {
                n.name.eq_ignore_ascii_case(p)
            };
            if hit && !out.contains(&n.name) {
                out.push(n.name.clone());
            }
        }
    }
    out.sort_by(|a, b| natural(a, b));
    out
}

/// The positive line of a pair named by its pair name or either line,
/// with its negative; a plain net stays as it is.
fn pair_of(board: &Board, name: &str) -> (String, Option<String>) {
    let mut pairs = board.diff_pairs.clone();
    pairs.extend(crate::zcheck::name_pairs(board));
    for p in &pairs {
        if p.name.eq_ignore_ascii_case(name)
            || p.positive.eq_ignore_ascii_case(name)
            || p.negative.eq_ignore_ascii_case(name)
        {
            return (p.positive.clone(), Some(p.negative.clone()));
        }
    }
    let find = |x: &str| {
        board
            .nets
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(x))
            .map(|n| n.name.clone())
    };
    let exact = find(name).unwrap_or(name.to_string());
    // A negative line by the usual names: DQS0 / DQSN0, DQS0_N, DQS0#,
    // CK_P / CK_N.
    let digits = exact.len() - exact.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, num) = exact.split_at(exact.len() - digits);
    let mut tries = vec![
        format!("{stem}N{num}"),
        format!("{stem}_N{num}"),
        format!("{exact}N"),
        format!("{exact}_N"),
        format!("{exact}#"),
    ];
    for (p, n) in [("_P", "_N"), ("P", "N"), ("+", "-")] {
        if let Some(base) = exact.strip_suffix(p) {
            tries.insert(0, format!("{base}{n}"));
        }
    }
    let neg = tries.iter().find_map(|t| find(t));
    (exact, neg)
}

/// "U1.A5": the part's pin on the net.
fn pin_on(board: &Board, part: &str, net: &str) -> Option<String> {
    let id = board.nets.iter().position(|n| n.name == net)? as u32;
    let comp = board
        .components
        .iter()
        .position(|c| c.designator.eq_ignore_ascii_case(part))? as u32;
    let des = &board.components[comp as usize].designator;
    board
        .geometry
        .pads
        .iter()
        .filter(|p| p.component == Some(comp) && p.net == Some(id))
        .map(|p| format!("{des}.{}", p.name))
        .min_by(|a, b| natural(a, b))
}

/// Opening of the eye (symmetric about the threshold) over `width_ps`
/// centred `at_ps` from its sampling point, volts; 0 when closed or
/// outside the eye's two bits.
pub fn mask_height(e: &EyeResult, at_ps: f64, width_ps: f64) -> f64 {
    let n = e.inner.len();
    if n < 3 || e.ui_ps <= 0.0 {
        return 0.0;
    }
    let per = (n - 1) / 2;
    let dt = e.ui_ps / per as f64;
    let (a, b) = (at_ps - width_ps / 2.0, at_ps + width_ps / 2.0);
    if a < -e.ui_ps || b > e.ui_ps {
        return 0.0;
    }
    // Sample index for a time, with linear interpolation.
    let value = |t: f64| {
        let x = t / dt + per as f64;
        let i = (x.floor() as usize).min(n - 2);
        let f = x - i as f64;
        let g = |k: usize| {
            let (one, zero) = e.inner[k];
            (one as f64).min(-(zero as f64))
        };
        g(i) * (1.0 - f) + g(i + 1) * f
    };
    let steps = ((b - a) / dt).ceil().max(1.0) as usize * 2;
    let mut low = f64::MAX;
    for k in 0..=steps {
        low = low.min(value(a + (b - a) * k as f64 / steps as f64));
    }
    (2.0 * low).max(0.0)
}

struct Solved {
    net: String,
    from: String,
    to: String,
    rep: Report,
    eye: EyeResult,
}

fn solve_one(
    board: &Board,
    setup: &BusSetup,
    net: &str,
    clock: bool,
) -> std::result::Result<Solved, String> {
    let from =
        pin_on(board, &setup.from, net).ok_or_else(|| format!("no pin of {} on it", setup.from))?;
    let to = match &setup.to {
        Some(p) => Some(pin_on(board, p, net).ok_or_else(|| format!("no pin of {p} on it"))?),
        None => None,
    };
    let gbps = setup.mts / 1000.0;
    // To where the edge's spectrum is down to 1 %.
    let rise = if setup.rise_ps > 0.0 {
        setup.rise_ps * 1e-12
    } else {
        0.3e-9 / gbps
    };
    let sigma = rise / 1.683;
    let need = (100f64.ln() / (2.0 * std::f64::consts::PI.powi(2) * sigma * sigma)).sqrt() * 1e-9;
    let ghz = (2.5 * gbps).max(1.1 * need).ceil().max(4.0);
    let opt = Options {
        from: from.clone(),
        to: to.clone(),
        ghz,
        points: (ghz * 40.0).ceil() as usize,
        vias_3d: setup.vias_3d,
        ..Default::default()
    };
    let rep = pathloss::run(board, &board.stackup, &opt, &|_, _| {})?;
    let mut es = EyeSetup {
        gbps,
        amplitude: setup.swing,
        rise_ps: setup.rise_ps,
        prbs: setup.prbs,
        clock,
        ..Default::default()
    };
    if setup.xtalk {
        es.aggressors = eye::aggressors(board, &rep, setup.swing);
    }
    let e = eye::eye(&rep, &es)?;
    Ok(Solved {
        net: net.to_string(),
        to: if rep.end.is_empty() {
            to.unwrap_or_default()
        } else {
            rep.end.clone()
        },
        from,
        rep,
        eye: e,
    })
}

/// Simulates the bus.
pub fn run(
    board: &Board,
    setup: &BusSetup,
    progress: &(dyn Fn(f64, &str) + Sync),
) -> std::result::Result<BusResult, String> {
    if setup.mts <= 0.0 {
        return Err("give the transfer rate (MT/s)".into());
    }
    if setup.from.trim().is_empty() {
        return Err("give the driving part (its designator)".into());
    }
    let ui = 1e6 / setup.mts;
    let strobe = setup.strobe.as_ref().map(|s| pair_of(board, s));
    let mut nets = expand(board, &setup.nets);
    if let Some((p, n)) = &strobe {
        nets.retain(|x| x != p && Some(x) != n.as_ref());
    }
    if nets.is_empty() {
        return Err("no net matches the bus's names".into());
    }
    const MOST: usize = 72;
    let mut notes = Vec::new();
    if nets.len() > MOST {
        notes.push(format!(
            "{} nets match; the first {MOST} are simulated",
            nets.len()
        ));
        nets.truncate(MOST);
    }
    // Jobs: the strobe first (a clock), then the bits.
    let mut jobs: Vec<(String, bool)> = Vec::new();
    if let Some((p, _)) = &strobe {
        jobs.push((p.clone(), true));
    }
    jobs.extend(nets.iter().map(|n| (n.clone(), false)));
    let total = jobs.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<std::result::Result<Solved, String>>>> =
        (0..total).map(|_| std::sync::Mutex::new(None)).collect();
    progress(0.0, "finding and cascading each bit's path");
    let token = crate::cancel::current();
    std::thread::scope(|sc| {
        for _ in 0..threads.min(total) {
            sc.spawn(|| loop {
                let _g = crate::cancel::adopt(&token);
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= total || crate::cancel::cancelled() {
                    break;
                }
                let (net, clock) = &jobs[i];
                let r = solve_one(board, setup, net, *clock);
                if let Ok(mut slot) = results[i].lock() {
                    *slot = Some(r);
                }
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                progress(d as f64 / total as f64, net);
            });
        }
    });
    crate::cancel::check()?;
    let mut solved: Vec<(String, std::result::Result<Solved, String>)> = jobs
        .iter()
        .zip(results)
        .map(|((n, _), m)| {
            (
                n.clone(),
                m.into_inner()
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| Err("not solved".into())),
            )
        })
        .collect();
    let mut skipped = Vec::new();
    let strobe_res = if strobe.is_some() {
        let (name, r) = solved.remove(0);
        match r {
            Ok(s) => Some(s),
            Err(e) => return Err(format!("strobe {name}: {e}")),
        }
    } else {
        None
    };
    let mut bits: Vec<Bit> = Vec::new();
    for (name, r) in solved {
        match r {
            Ok(s) => {
                let e = &s.eye;
                let sample = e.sample_ns * 1e3;
                let worst_aggressor = e
                    .xtalk
                    .first()
                    .map(|x| format!("{} ({:.0} mV)", x.name, x.closure * 1e3))
                    .unwrap_or_default();
                bits.push(Bit {
                    net: s.net,
                    from: s.from,
                    to: s.to,
                    length: s.rep.length,
                    il_db: s.rep.il_at(setup.mts / 2000.0).unwrap_or(0.0),
                    // The mean threshold crossing less the bit it ends.
                    flight_ps: sample + e.cross_ps[1] - ui,
                    height: e.height,
                    width_ps: e.width_ps,
                    jitter_ps: e.jitter_ps,
                    worst: e.worst_with_xtalk,
                    xtalk: e.xtalk.iter().map(|x| x.closure).sum::<f64>().max(0.0),
                    worst_aggressor,
                    window: [sample + e.cross_ps[2] - ui, sample + e.cross_ps[0]],
                    setup: None,
                    hold: None,
                    mask_height: None,
                    eye: s.eye,
                });
            }
            Err(e) => skipped.push((name, e)),
        }
    }
    if bits.is_empty() {
        let why = skipped
            .iter()
            .take(3)
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("no bit of the bus could be simulated ({why})"));
    }
    let strobe = strobe_res.map(|s| {
        let e = &s.eye;
        let sample = e.sample_ns * 1e3;
        Strobe {
            net: match &strobe {
                Some((p, Some(n))) => format!("{p} / {n}"),
                _ => s.net.clone(),
            },
            from: s.from,
            to: s.to,
            length: s.rep.length,
            flight_ps: sample + e.cross_ps[1] - ui,
            edge_ps: setup.strobe_offset_ui * ui + sample + e.cross_ps[1] - ui,
            jitter_ps: (e.cross_ps[2] - e.cross_ps[0]).max(0.0),
            eye: s.eye,
        }
    });
    let js = strobe.as_ref().map_or(0.0, |s| s.jitter_ps);
    let (tsu, th) = (setup.setup_ps, setup.hold_ps);
    let mask_w = setup.mask.map(|(_, w)| w * ui);
    // As routed.
    let mut routed = None;
    if let Some(s) = &strobe {
        let mut m = Margin {
            setup: f64::MAX,
            hold: f64::MAX,
            ..Default::default()
        };
        for b in bits.iter_mut() {
            let su = s.edge_ps - js / 2.0 - b.window[0] - tsu;
            let ho = b.window[1] - (s.edge_ps + js / 2.0) - th;
            b.setup = Some(su);
            b.hold = Some(ho);
            if su < m.setup {
                m.setup = su;
                m.setup_bit = b.net.clone();
            }
            if ho < m.hold {
                m.hold = ho;
                m.hold_bit = b.net.clone();
            }
            if let Some(w) = mask_w {
                let at = s.edge_ps - b.eye.sample_ns * 1e3;
                b.mask_height = Some(mask_height(&b.eye, at, w + js));
            }
        }
        routed = Some(m);
    }
    // Trained: the strobe centred on the lane's common window.
    let common = [
        bits.iter().map(|b| b.window[0]).fold(f64::MIN, f64::max),
        bits.iter().map(|b| b.window[1]).fold(f64::MAX, f64::min),
    ];
    let spare = common[1] - common[0] - js - tsu - th;
    let first = |key: &dyn Fn(&Bit) -> f64, low: bool| {
        bits.iter()
            .min_by(|a, b| {
                let (x, y) = (key(a), key(b));
                if low {
                    x.total_cmp(&y)
                } else {
                    y.total_cmp(&x)
                }
            })
            .map_or(String::new(), |b| b.net.clone())
    };
    let trained = Margin {
        setup: spare / 2.0,
        hold: spare / 2.0,
        setup_bit: first(&|b| b.window[0], false),
        hold_bit: first(&|b| b.window[1], true),
    };
    // Deskewed: each bit centred on its own.
    let own = |b: &Bit| (b.window[1] - b.window[0] - js - tsu - th) / 2.0;
    let worst = bits
        .iter()
        .min_by(|a, b| own(a).total_cmp(&own(b)))
        .map_or(String::new(), |b| b.net.clone());
    let d = bits.iter().map(own).fold(f64::MAX, f64::min);
    let deskewed = Margin {
        setup: d,
        hold: d,
        setup_bit: worst.clone(),
        hold_bit: worst,
    };
    // Without a strobe, the mask is checked at the trained centre.
    if strobe.is_none() {
        if let Some(w) = mask_w {
            let centre = (common[0] + common[1]) / 2.0;
            for b in bits.iter_mut() {
                b.mask_height = Some(mask_height(&b.eye, centre - b.eye.sample_ns * 1e3, w));
            }
        }
    }
    if !setup.xtalk {
        notes.push("crosstalk left out".into());
    }
    if !setup.vias_3d {
        notes.push(
            "vias from a coaxial estimate (drill, antipad and barrel); --vias-3d solves each kind in 3D"
                .into(),
        );
    }
    notes.push(
        "the driver is a matched source and each receiver its port's impedance; give IBIS models with `trace sim` for a nonlinear driver, ODT and power noise (SSN is not included here)"
            .into(),
    );
    let mut all_notes: Vec<String> = Vec::new();
    for b in &bits {
        for n in &b.eye.notes {
            let line = format!("{}: {n}", b.net);
            if !all_notes.iter().any(|x| x.ends_with(n.as_str())) {
                all_notes.push(line);
            }
        }
    }
    notes.extend(all_notes.into_iter().take(6));
    Ok(BusResult {
        mts: setup.mts,
        ui_ps: ui,
        bits,
        strobe,
        skipped,
        common,
        routed,
        trained,
        deskewed,
        setup_ps: tsu,
        hold_ps: th,
        mask: setup.mask,
        swing: setup.swing,
        notes,
    })
}

impl BusResult {
    fn setup_like(&self) -> BusSetup {
        BusSetup {
            mask: self.mask,
            ..Default::default()
        }
    }

    pub fn failing(&self) -> usize {
        let s = self.setup_like();
        self.bits.iter().filter(|b| !b.passes(&s)).count()
    }

    /// The verdict: the margin that applies (as routed when there is a
    /// strobe) and whether every bit passes.
    pub fn passes(&self) -> bool {
        self.failing() == 0
            && self.routed.as_ref().map_or(self.trained.setup >= 0.0, |m| {
                m.setup >= 0.0 && m.hold >= 0.0
            })
    }

    pub fn report(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "Bus at {} MT/s (bit {:.1} ps), {} bit(s), swing {:.3} V{}",
            trim(self.mts),
            self.ui_ps,
            self.bits.len(),
            self.swing,
            if self.setup_ps > 0.0 || self.hold_ps > 0.0 {
                format!(
                    ", receiver setup {} ps / hold {} ps",
                    trim(self.setup_ps),
                    trim(self.hold_ps)
                )
            } else {
                String::new()
            }
        );
        if let Some((v, w)) = self.mask {
            let _ = writeln!(
                o,
                "Receiver mask: {:.0} mV over {:.2} UI ({:.1} ps)",
                v * 1e3,
                w,
                w * self.ui_ps
            );
        }
        if let Some(s) = &self.strobe {
            let _ = writeln!(
                o,
                "Strobe {} ({} to {}): {:.0} mil, flight {:.1} ps, edge at {:.1} ps after the data launch, jitter {:.1} ps p-p",
                s.net, s.from, s.to, s.length, s.flight_ps, s.edge_ps, s.jitter_ps
            );
        }
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "{:<18} {:>7} {:>6} {:>7} {:>6} {:>7} {:>6} {:>6} {:>6} {:>7} {:>7} {:>6}  status",
            "Net",
            "mil",
            "IL dB",
            "flight",
            "skew",
            "height",
            "width",
            "jitter",
            "xtalk",
            "setup",
            "hold",
            "mask"
        );
        let refd = self
            .strobe
            .as_ref()
            .map(|s| s.flight_ps)
            .unwrap_or_else(|| {
                self.bits.iter().map(|b| b.flight_ps).sum::<f64>() / self.bits.len().max(1) as f64
            });
        let s = self.setup_like();
        for b in &self.bits {
            let opt =
                |v: Option<f64>, k: f64| v.map_or("-".to_string(), |x| format!("{:.1}", x * k));
            let _ = writeln!(
                o,
                "{:<18} {:>7.0} {:>6.2} {:>7.1} {:>+6.1} {:>7.1} {:>6.1} {:>6.1} {:>6.1} {:>7} {:>7} {:>6}  {}",
                b.net,
                b.length,
                b.il_db,
                b.flight_ps,
                b.flight_ps - refd,
                b.height * 1e3,
                b.width_ps,
                b.jitter_ps,
                b.xtalk * 1e3,
                opt(b.setup, 1.0),
                opt(b.hold, 1.0),
                opt(b.mask_height, 1e3),
                if b.passes(&s) { "ok" } else { "FAIL" }
            );
        }
        let _ = writeln!(
            o,
            "\n(ps; height, xtalk and mask in mV; skew against the {})",
            if self.strobe.is_some() {
                "strobe's flight time"
            } else {
                "bus's mean"
            }
        );
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "Every bit valid from {:.1} to {:.1} ps after launch ({:.1} ps, {:.0} % of a bit)",
            self.common[0],
            self.common[1],
            self.common[1] - self.common[0],
            100.0 * (self.common[1] - self.common[0]) / self.ui_ps
        );
        let line = |o: &mut String, what: &str, m: &Margin| {
            let _ = writeln!(
                o,
                "{what}: setup {:+.1} ps ({}), hold {:+.1} ps ({}), total {:+.1} ps",
                m.setup,
                m.setup_bit,
                m.hold,
                m.hold_bit,
                m.total()
            );
        };
        if let Some(m) = &self.routed {
            line(&mut o, "As routed", m);
        }
        line(
            &mut o,
            "Strobe centred on the lane (training)",
            &self.trained,
        );
        line(
            &mut o,
            "Each bit deskewed (per-bit training)",
            &self.deskewed,
        );
        if let Some(b) = self
            .bits
            .iter()
            .max_by(|a, b| a.xtalk.total_cmp(&b.xtalk))
            .filter(|b| b.xtalk > 0.0)
        {
            let _ = writeln!(
                o,
                "Most crosstalk: {} closed by {:.1} mV, worst from {}",
                b.net,
                b.xtalk * 1e3,
                b.worst_aggressor
            );
        }
        let fails = self.failing();
        let _ = writeln!(
            o,
            "{}",
            if self.passes() {
                "PASS: every bit meets timing".to_string()
            } else if fails > 0 {
                format!("FAIL: {fails} bit(s) miss timing or the mask")
            } else {
                "FAIL: the bus has no timing margin".to_string()
            }
        );
        for (n, e) in &self.skipped {
            let _ = writeln!(o, "skipped {n}: {e}");
        }
        for n in &self.notes {
            let _ = writeln!(o, "note: {n}");
        }
        o
    }

    pub fn csv(&self) -> String {
        let mut o = String::from(
            "net,from,to,length_mil,il_db,flight_ps,valid_from_ps,valid_to_ps,height_mv,width_ps,jitter_ps,worst_mv,xtalk_mv,setup_ps,hold_ps,mask_mv,status\n",
        );
        let s = self.setup_like();
        let opt = |v: Option<f64>, k: f64| v.map_or(String::new(), |x| format!("{:.2}", x * k));
        for b in &self.bits {
            let _ = writeln!(
                o,
                "{},{},{},{:.1},{:.3},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{},{},{}",
                b.net,
                b.from,
                b.to,
                b.length,
                b.il_db,
                b.flight_ps,
                b.window[0],
                b.window[1],
                b.height * 1e3,
                b.width_ps,
                b.jitter_ps,
                b.worst * 1e3,
                b.xtalk * 1e3,
                opt(b.setup, 1.0),
                opt(b.hold, 1.0),
                opt(b.mask_height, 1e3),
                if b.passes(&s) { "ok" } else { "fail" }
            );
        }
        o
    }

    /// Timing chart: each bit's valid window against the strobe's edge,
    /// with the setup and hold the receiver needs.
    pub fn svg(&self, title: &str) -> String {
        let n = self.bits.len().max(1);
        let row = 22.0;
        let (l, r, t) = (170.0, 120.0, 70.0);
        let w = 980.0;
        let h = t + row * n as f64 + 76.0;
        // Time axis: a bit and a half either side of the reference.
        let centre = self
            .strobe
            .as_ref()
            .map_or((self.common[0] + self.common[1]) / 2.0, |s| s.edge_ps);
        let span = 1.25 * self.ui_ps;
        let x = |ps: f64| l + (w - l - r) * ((ps - centre) / (2.0 * span) + 0.5);
        let mut o = String::new();
        let _ = writeln!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="Segoe UI, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#fff"/>"##
        );
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="24" font-size="14" fill="#111">{}</text>"##,
            esc(title)
        );
        let headline = match &self.routed {
            Some(m) => format!(
                "{} MT/s · as routed: setup {:+.1} ps, hold {:+.1} ps · trained {:+.1} / {:+.1} ps",
                trim(self.mts),
                m.setup,
                m.hold,
                self.trained.setup,
                self.trained.hold
            ),
            None => format!(
                "{} MT/s · every bit valid for {:.1} ps of {:.1} · trained margin {:+.1} ps each side",
                trim(self.mts),
                self.common[1] - self.common[0],
                self.ui_ps,
                self.trained.setup
            ),
        };
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="42" fill="#444">{}</text>"##,
            esc(&headline)
        );
        let bottom = t + row * n as f64;
        // Grid every quarter bit.
        for k in -5..=5 {
            let ps = centre + k as f64 * self.ui_ps / 4.0;
            let xi = x(ps);
            let _ = writeln!(
                o,
                r##"<line x1="{xi:.1}" x2="{xi:.1}" y1="{}" y2="{bottom}" stroke="{}"/>"##,
                t - 6.0,
                if k == 0 { "#9aa3b2" } else { "#eceff4" }
            );
            if k % 2 == 0 {
                let _ = writeln!(
                    o,
                    r##"<text x="{xi:.1}" y="{}" text-anchor="middle" fill="#555">{}</text>"##,
                    bottom + 16.0,
                    trim(((ps - centre) * 10.0).round() / 10.0)
                );
            }
        }
        let _ = writeln!(
            o,
            r##"<text x="{}" y="{}" text-anchor="middle" fill="#555">ps from the {}</text>"##,
            (l + w - r) / 2.0,
            bottom + 34.0,
            if self.strobe.is_some() {
                "strobe's edge at the receiver"
            } else {
                "centre of the common window"
            }
        );
        // The receiver's need around the strobe.
        let js = self.strobe.as_ref().map_or(0.0, |s| s.jitter_ps);
        let need = [
            centre - js / 2.0 - self.setup_ps,
            centre + js / 2.0 + self.hold_ps,
        ];
        if need[1] - need[0] > 0.1 {
            let _ = writeln!(
                o,
                r##"<rect x="{:.1}" y="{}" width="{:.1}" height="{}" fill="#d4380d" fill-opacity="0.12"/>"##,
                x(need[0]),
                t - 6.0,
                x(need[1]) - x(need[0]),
                bottom - t + 6.0
            );
        }
        // The receiver mask's width at the strobe.
        if let Some((_, mw)) = self.mask {
            let half = mw * self.ui_ps / 2.0 + js / 2.0;
            let _ = writeln!(
                o,
                r##"<rect x="{:.1}" y="{}" width="{:.1}" height="{}" fill="none" stroke="#d4380d" stroke-dasharray="4 3"/>"##,
                x(centre - half),
                t - 6.0,
                x(centre + half) - x(centre - half),
                bottom - t + 6.0
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="{}" fill="#555"><tspan fill="#2f9e6e">■</tspan> where the bit is valid at the receiver · <tspan fill="#d4380d">■</tspan> setup + hold + strobe jitter{} · right: setup / hold margin</text>"##,
            bottom + 56.0,
            if self.mask.is_some() {
                ", dashed: the receiver mask"
            } else {
                ""
            }
        );
        let s = self.setup_like();
        for (i, b) in self.bits.iter().enumerate() {
            let y = t + row * i as f64;
            let ok = b.passes(&s);
            let _ = writeln!(
                o,
                r##"<text x="{}" y="{:.1}" text-anchor="end" fill="#222">{}</text>"##,
                l - 8.0,
                y + 15.0,
                esc(&b.net)
            );
            let _ = writeln!(
                o,
                r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#d5dae3"/>"##,
                w - r,
                y + row / 2.0,
                y + row / 2.0
            );
            let a = x(b.window[0]);
            let z = x(b.window[1]);
            if z > a {
                let _ = writeln!(
                    o,
                    r##"<rect x="{a:.1}" y="{:.1}" width="{:.1}" height="{}" rx="3" fill="{}"/>"##,
                    y + 4.0,
                    z - a,
                    row - 8.0,
                    if ok { "#2f9e6e" } else { "#d4380d" }
                );
            }
            let text = match (b.setup, b.hold) {
                (Some(su), Some(ho)) => format!("{su:+.0} / {ho:+.0} ps"),
                _ => format!("{:.0} ps", b.window[1] - b.window[0]),
            };
            let _ = writeln!(
                o,
                r##"<text x="{}" y="{:.1}" fill="{}">{}</text>"##,
                w - r + 8.0,
                y + 15.0,
                if ok { "#2f6e4e" } else { "#b02a0a" },
                text
            );
        }
        let _ = writeln!(o, "</svg>");
        o
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fdtd::Cx;
    use std::f64::consts::PI;

    fn line(len_ns: f64, loss: f64) -> Report {
        let f: Vec<f64> = (1..=400).map(|k| 20.0 * k as f64 / 400.0).collect();
        let s = f
            .iter()
            .map(|&g| {
                let s21 = Cx::expj(-2.0 * PI * g * len_ns).scale((-loss * g / 8.686).exp());
                [Cx::default(), s21, s21, Cx::default()]
            })
            .collect();
        Report {
            f,
            s,
            z_ref: [50.0, 50.0],
            ..Default::default()
        }
    }

    #[test]
    fn natural_order_puts_numbers_in_order() {
        let mut v = vec!["DQ10", "DQ2", "DQ1", "DM0"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["DM0", "DQ1", "DQ2", "DQ10"]);
    }

    #[test]
    fn window_and_strobe_edge_follow_the_flight_time() {
        // A clean 1 ns line at 1 Gbit/s: the bit is valid from 1 ns
        // (plus the edge) to 2 ns after launch; a clock's edges land at
        // 1 ns past each launch.
        let rep = line(1.0, 0.0);
        let e = eye::eye(
            &rep,
            &EyeSetup {
                gbps: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        let s = e.sample_ns * 1e3;
        let w = [s + e.cross_ps[2] - 1000.0, s + e.cross_ps[0]];
        assert!((w[0] - 1000.0).abs() < 25.0, "{w:?} {}", e.summary());
        assert!((w[1] - 2000.0).abs() < 25.0, "{w:?}");
        let c = eye::eye(
            &rep,
            &EyeSetup {
                gbps: 1.0,
                clock: true,
                ..Default::default()
            },
        )
        .unwrap();
        let edge = c.sample_ns * 1e3 + c.cross_ps[1] - 1000.0;
        assert!((edge - 1000.0).abs() < 25.0, "{edge}");
        assert!(c.cross_ps[2] - c.cross_ps[0] < 2.0);
        // The mask fits at the centre and not across the edge.
        let mid = mask_height(&e, 0.0, 200.0);
        assert!(mid > 0.7, "{mid}");
        assert!(mask_height(&e, 500.0, 200.0) < 0.1);
    }

    #[test]
    fn loss_narrows_the_window() {
        let clean = eye::eye(
            &line(1.0, 0.0),
            &EyeSetup {
                gbps: 4.0,
                ..Default::default()
            },
        )
        .unwrap();
        let lossy = eye::eye(
            &line(1.0, 2.5),
            &EyeSetup {
                gbps: 4.0,
                ..Default::default()
            },
        )
        .unwrap();
        let width = |e: &EyeResult| e.cross_ps[0] - (e.cross_ps[2] - e.ui_ps);
        assert!(width(&lossy) < width(&clean) - 5.0);
        assert!((width(&clean) - clean.width_ps).abs() < 1.0);
    }
}
