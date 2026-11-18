//! Whole-board RF report: every RF path on the board found and run
//! through the path-loss engine ([`crate::pathloss`]) in one go.
//!
//! Each coaxial connector's signal pin (SMA, U.FL, MMCX and the like, or
//! any connector whose only signal pin sits among ground pins) starts a
//! path; it is followed through series parts and baluns to the farthest
//! chip pin. Paths are named and sorted by what their nets say they are
//! (RX, ORX, TX, LO, clock), get a chip port model as their load where
//! one is given for them (`--load PATTERN=file`), and are judged on the
//! worst return loss in the band and, when a budget is set, the
//! insertion loss. Each path lists where it reflects and loses most, so
//! the fix starts at the right part.
//!
//! The 3D via solves are kept between paths, so each kind of via is
//! solved once for the whole board.

use crate::fdtd::Cx;
use crate::pathloss::{self, Options, Report, ViaCache};
use crate::touchstone::Network;
use crate::{Board, Stackup};
use std::collections::HashSet;
use std::fmt::Write;

/// What the path carries, from its net names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Rx,
    Orx,
    Tx,
    Lo,
    Clock,
    Rf,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Rx => "RX",
            Class::Orx => "ORX",
            Class::Tx => "TX",
            Class::Lo => "LO",
            Class::Clock => "clock",
            Class::Rf => "RF",
        }
    }

    /// A colour for plots and the board (RGB).
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Class::Rx => [46, 120, 210],
            Class::Orx => [120, 80, 200],
            Class::Tx => [214, 90, 40],
            Class::Lo => [30, 160, 110],
            Class::Clock => [200, 150, 20],
            Class::Rf => [110, 110, 120],
        }
    }
}

/// How the path fared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    /// Return loss or insertion loss outside the goal.
    Check,
    /// No path could be followed.
    Failed,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Check => "CHECK",
            Status::Failed => "failed",
        }
    }
}

/// What to run.
#[derive(Clone, Debug)]
pub struct Setup {
    /// Band of interest, GHz: the sweep runs to the top, return loss is
    /// judged from the bottom.
    pub band: (f64, f64),
    pub points: usize,
    pub vias_3d: bool,
    /// Vendor part models, as for the path loss.
    pub models: Vec<(String, Network)>,
    /// Chip port models: a pattern (* and ?) matched against the end
    /// pin (U101.H17), its net, the path's name or its class, and the
    /// model, with a name for the report.
    pub loads: Vec<(String, String, Network)>,
    pub tan_d: f64,
    pub fit: Vec<String>,
    /// Return loss goal in the band, dB (positive: 10 means |S11| below
    /// -10 dB).
    pub rl_goal: f64,
    /// Insertion loss budget at the top of the band, dB, when set.
    pub il_budget: Option<f64>,
    /// Only these connectors (designator patterns); empty takes all.
    pub only: Vec<String>,
    /// More starts (pins, parts or nets) beside the connectors.
    pub extra: Vec<String>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            band: (0.5, 6.0),
            points: 200,
            vias_3d: true,
            models: Vec::new(),
            loads: Vec::new(),
            tan_d: 0.02,
            fit: Vec::new(),
            rl_goal: 10.0,
            il_budget: None,
            only: Vec::new(),
            extra: Vec::new(),
        }
    }
}

/// One RF path.
#[derive(Clone, Debug)]
pub struct Path {
    /// Its name, from its nets.
    pub name: String,
    pub class: Class,
    /// Where it starts (the connector pin, or the start given).
    pub from: String,
    /// The analysis, or why there is none.
    pub report: Result<Report, String>,
    /// The chip model it ends in, by name.
    pub load: Option<String>,
    pub status: Status,
    /// Why it needs a look, and where to start.
    pub findings: Vec<String>,
    /// The sections that reflect most: (what, |Γ|, where, mil).
    pub reflections: Vec<(String, f64, [f64; 2])>,
    /// The sections that lose most at the top: (what, dB, where).
    pub losses: Vec<(String, f64, [f64; 2])>,
}

impl Path {
    /// Insertion loss at `f` GHz, dB.
    pub fn il_at(&self, f: f64) -> Option<f64> {
        self.report.as_ref().ok().and_then(|r| r.il_at(f))
    }

    /// The return loss judged: into the load when there is one, else
    /// the path's own (50 or 100 Ω at the far end). (dB, GHz).
    pub fn worst_rl(&self) -> Option<(f64, f64)> {
        let r = self.report.as_ref().ok()?;
        if !r.s11_loaded.is_empty() {
            r.worst_loaded()
        } else {
            r.worst_rl()
        }
    }

    /// Number of vias and parts on it.
    pub fn counts(&self) -> (usize, usize) {
        self.report.as_ref().map_or((0, 0), |r| {
            (
                r.sections.iter().filter(|s| s.kind == "via").count(),
                r.sections
                    .iter()
                    .filter(|s| matches!(s.kind, "part" | "balun" | "shunt"))
                    .count(),
            )
        })
    }

    /// A file-friendly name.
    pub fn file_stem(&self) -> String {
        let s: String = self
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{}_{}", self.class.name(), s)
    }
}

/// The whole report.
#[derive(Clone, Debug, Default)]
pub struct RfReport {
    pub paths: Vec<Path>,
    pub band: (f64, f64),
    pub rl_goal: f64,
    pub il_budget: Option<f64>,
    /// Kinds of via solved in 3D for the whole board.
    pub via_kinds: usize,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------
// Finding the starts.

fn is_ground(name: &str) -> bool {
    crate::zcheck::is_ground(name)
}

fn is_power(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    crate::pdn::volts_in_name(&u).is_some()
        || ["VCC", "VDD", "VIN", "VBUS", "VBAT", "PWR"]
            .iter()
            .any(|k| u.contains(k))
}

fn connector_designator(d: &str) -> bool {
    let d = d.to_ascii_uppercase();
    (d.starts_with('J') && !d.starts_with("JP"))
        || d.starts_with('X')
        || d.starts_with("CN")
        || d.starts_with("SMA")
        || (d.starts_with('P') && d.len() > 1 && d[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Coaxial words, as whole words (or a word's start before a digit or a
/// dash), so "SMALL" is not an SMA.
fn coax_words(text: &str) -> bool {
    const KEYS: [&str; 13] = [
        "SMA", "U.FL", "UFL", "MMCX", "MCX", "SMP", "SMB", "BNC", "COAX", "IPEX", "W.FL", "MHF",
        "2.92",
    ];
    let up = text.to_ascii_uppercase();
    up.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|w| !w.is_empty())
        .any(|w| {
            KEYS.iter().any(|k| {
                w.starts_with(k)
                    && w[k.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_ascii_alphabetic())
            })
        })
}

/// The RF starts: each coaxial connector's signal pin (designator.pin)
/// with the connector's designator.
pub fn starts(board: &Board, only: &[String]) -> Vec<(String, String)> {
    let g = &board.geometry;
    let mut out = Vec::new();
    for (ci, c) in board.components.iter().enumerate() {
        if !connector_designator(&c.designator) {
            continue;
        }
        if !only.is_empty()
            && !only
                .iter()
                .any(|p| crate::constraints::glob(p, &c.designator))
        {
            continue;
        }
        let pads: Vec<&crate::Pad> = g
            .pads
            .iter()
            .filter(|p| p.component == Some(ci as u32))
            .collect();
        if pads.len() < 2 || pads.len() > 12 {
            continue;
        }
        let mut signal: Vec<(&crate::Pad, u32)> = Vec::new();
        let mut grounds = 0;
        for p in &pads {
            if let Some(n) = p.net {
                let nm = board.nets.get(n as usize).map_or("", |x| x.name.as_str());
                if is_ground(nm) {
                    grounds += 1;
                } else if !signal.iter().any(|s| s.1 == n) {
                    signal.push((p, n));
                }
            }
        }
        let mut text = format!(
            "{} {}",
            c.footprint,
            c.library_ref.clone().unwrap_or_default()
        );
        for v in c.params.values() {
            text.push(' ');
            text.push_str(v);
        }
        // A coaxial part by name, or one signal pin among ground pins
        // (an SMA footprint named only by its maker's number).
        let coax = coax_words(&text) || (signal.len() == 1 && grounds >= 2);
        if !coax || signal.len() != 1 {
            continue;
        }
        let (pad, n) = signal[0];
        let nm = board.nets.get(n as usize).map_or("", |x| x.name.as_str());
        if is_power(nm) {
            continue;
        }
        out.push((
            format!("{}.{}", c.designator, pad.name),
            c.designator.clone(),
        ));
    }
    out
}

// ---------------------------------------------------------------------
// Naming.

fn tokens(name: &str) -> Vec<String> {
    name.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// The class a net name says, if any.
fn class_of(name: &str) -> Option<Class> {
    let ts = tokens(name);
    let starts = |t: &str, k: &str| {
        t.strip_prefix(k).is_some_and(|r| {
            r.chars()
                .all(|c| c.is_ascii_digit() || c == 'P' || c == 'N')
        })
    };
    let any = |f: &dyn Fn(&str) -> bool| ts.iter().any(|t| f(t));
    if any(&|t| starts(t, "ORX") || t.starts_with("ORX")) {
        Some(Class::Orx)
    } else if any(&|t| t.starts_with("RX") || t.ends_with("RX") || t.contains("RXIN")) {
        Some(Class::Rx)
    } else if any(&|t| t.starts_with("TX") || t.ends_with("TX") || t.contains("TXOUT")) {
        Some(Class::Tx)
    } else if any(&|t| starts(t, "LO") || t.ends_with("LO") && t.len() <= 6) {
        Some(Class::Lo)
    } else if any(&|t| {
        t.contains("CLK")
            || t.contains("SYSREF")
            || t.starts_with("REF")
            || t.contains("OSC")
            || t.starts_with("XO")
    }) {
        Some(Class::Clock)
    } else {
        None
    }
}

/// A name nobody gave (Net_123, N12345, NetC3_1).
fn generic(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    let rest = u
        .strip_prefix("NET_")
        .or_else(|| u.strip_prefix("NET"))
        .or_else(|| u.strip_prefix('N'))
        .unwrap_or(&u);
    rest.chars()
        .all(|c| c.is_ascii_digit() || c == '_' || c == '-')
        || u.starts_with("NET") && u.contains('_') && u.len() < 14
        || u.starts_with("UNNAMED")
}

/// Strips a pair's polarity from a net name.
fn stem(name: &str) -> String {
    for e in ["_P", "_N", "+", "-", "_POS", "_NEG"] {
        if let Some(s) = name.strip_suffix(e) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    name.to_string()
}

/// The path's name and class from its nets: the chip end's named net
/// first, then the others in order from the chip.
fn name_path(rep: &Report, from_des: &str) -> (String, Class) {
    let mut nets: Vec<&str> = Vec::new();
    for s in rep.sections.iter().rev() {
        if !nets.contains(&s.net.as_str()) {
            nets.push(&s.net);
        }
    }
    let class = nets.iter().find_map(|n| class_of(n)).unwrap_or(Class::Rf);
    let named = nets
        .iter()
        .find(|n| !generic(n) && class_of(n) == Some(class))
        .or_else(|| nets.iter().find(|n| !generic(n)));
    let name = match named {
        Some(n) => stem(n),
        None => format!("{from_des} to {}", rep.end),
    };
    (name, class)
}

// ---------------------------------------------------------------------
// Judging.

fn reflection(z: f64, zr: f64) -> f64 {
    ((z - zr) / (z + zr)).abs()
}

fn judge(path: &mut Path, setup: &Setup) {
    let Ok(rep) = &path.report else {
        path.status = Status::Failed;
        return;
    };
    let top = rep.f.last().copied().unwrap_or(setup.band.1);
    // Sections that reflect: an impedance off the path's reference, on a
    // run long enough to matter (a twentieth of a wavelength at the top,
    // in the board) or any via or part.
    let lambda20 = 299_792_458.0 / (top * 1e9) / 25.4e-6 / 20.0 / 2.0;
    let mut refl: Vec<(String, f64, [f64; 2])> = rep
        .sections
        .iter()
        .filter_map(|s| {
            let z = s.z?;
            let zr = if s.diff { 100.0 } else { 50.0 };
            if s.kind == "trace" && s.length < lambda20 {
                return None;
            }
            let g = reflection(z, zr);
            (g > 0.1).then(|| {
                (
                    format!(
                        "{} [{}] {:.0} Ω{} against {:.0}",
                        s.what,
                        s.net,
                        z,
                        if s.diff { " diff" } else { "" },
                        zr
                    ),
                    g,
                    s.at,
                )
            })
        })
        .collect();
    refl.sort_by(|a, b| b.1.total_cmp(&a.1));
    refl.truncate(4);
    let mut loss: Vec<(String, f64, [f64; 2])> = rep
        .sections
        .iter()
        .filter(|s| s.loss_db > 0.02)
        .map(|s| (format!("{} [{}]", s.what, s.net), s.loss_db, s.at))
        .collect();
    loss.sort_by(|a, b| b.1.total_cmp(&a.1));
    loss.truncate(3);
    let mut findings = Vec::new();
    if let Some((rl, f)) = path.worst_rl() {
        if rl > -setup.rl_goal {
            findings.push(format!(
                "return loss {:.1} dB at {:.2} GHz{} is worse than the {:.0} dB goal",
                rl,
                f,
                if path.load.is_some() {
                    " (into the chip)"
                } else {
                    ""
                },
                setup.rl_goal
            ));
        }
    }
    if let Some(b) = setup.il_budget {
        let il = rep.il_db();
        if il > b {
            findings.push(format!(
                "insertion loss {il:.2} dB at {top} GHz is over the {b:.2} dB budget"
            ));
        }
    }
    if !findings.is_empty() {
        if let Some(r) = refl.first() {
            findings.push(format!("start with {} (|Γ| {:.2})", r.0, r.1));
        }
    }
    path.status = if findings.is_empty() {
        Status::Pass
    } else {
        Status::Check
    };
    path.findings = findings;
    path.reflections = refl;
    path.losses = loss;
}

fn load_for<'a>(
    setup: &'a Setup,
    rep: &Report,
    name: &str,
    class: Class,
) -> Option<&'a (String, String, Network)> {
    let end_net = rep.sections.last().map(|s| s.net.as_str()).unwrap_or("");
    setup.loads.iter().find(|(pat, _, _)| {
        [rep.end.as_str(), end_net, name, class.name()]
            .iter()
            .any(|t| crate::constraints::glob(pat, t))
    })
}

// ---------------------------------------------------------------------

/// Finds and runs every RF path. Long when vias are solved in 3D (each
/// kind once); checks for a cancel between paths.
pub fn run(
    board: &Board,
    stackup: &Stackup,
    setup: &Setup,
    progress: &dyn Fn(f64, &str),
) -> Result<RfReport, String> {
    let mut list: Vec<(String, String)> = starts(board, &setup.only);
    for e in &setup.extra {
        if !list.iter().any(|(p, _)| p.eq_ignore_ascii_case(e)) {
            let des = e.split(['.', ':']).next().unwrap_or(e).to_string();
            list.push((e.clone(), des));
        }
    }
    let mut out = RfReport {
        band: setup.band,
        rl_goal: setup.rl_goal,
        il_budget: setup.il_budget,
        ..Default::default()
    };
    if list.is_empty() {
        return Err(
            "no RF connectors found (coaxial parts, or connectors with one signal pin among ground pins); give starts with --from"
                .into(),
        );
    }
    let mut vias = ViaCache::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let n = list.len() as f64;
    for (k, (from, des)) in list.iter().enumerate() {
        crate::cancel::check()?;
        let base = k as f64 / n;
        progress(base, &format!("path {} of {}: {from}", k + 1, list.len()));
        let opt = Options {
            from: from.clone(),
            to: None,
            ghz: setup.band.1,
            points: setup.points,
            vias_3d: setup.vias_3d,
            models: setup.models.clone(),
            load: None,
            tan_d: setup.tan_d,
            fmin: setup.band.0,
            fit: setup.fit.clone(),
            ..Default::default()
        };
        let sub = |p: f64, what: &str| progress(base + p / n, &format!("{from}: {what}"));
        let mut r = pathloss::run_shared(board, stackup, &opt, &mut vias, &sub);
        if let Err(e) = &r {
            if crate::cancel::is_cancel(e) {
                return Err(e.clone());
            }
        }
        // A path between two connectors is found from both ends; keep it
        // once.
        if let Ok(rep) = &r {
            let a = rep.start.split('.').next().unwrap_or("").to_string();
            let b = rep.end.split('.').next().unwrap_or("").to_string();
            let key = if a <= b { (a, b) } else { (b, a) };
            if !seen.insert(key) {
                continue;
            }
        }
        let (name, class) = match &r {
            Ok(rep) => name_path(rep, des),
            Err(_) => (des.clone(), Class::Rf),
        };
        let mut load = None;
        if let Ok(rep) = &r {
            if let Some((_, lname, net)) = load_for(setup, rep, &name, class) {
                // Run again with the load: the cascade is cheap now that
                // the vias are solved.
                let opt = Options {
                    load: Some(net.clone()),
                    ..opt.clone()
                };
                let again = pathloss::run_shared(board, stackup, &opt, &mut vias, &sub);
                if let Err(e) = &again {
                    if crate::cancel::is_cancel(e) {
                        return Err(e.clone());
                    }
                }
                if again.is_ok() {
                    r = again;
                    load = Some(lname.clone());
                }
            }
        }
        let mut path = Path {
            name,
            class,
            from: from.clone(),
            report: r,
            load,
            status: Status::Pass,
            findings: Vec::new(),
            reflections: Vec::new(),
            losses: Vec::new(),
        };
        judge(&mut path, setup);
        out.paths.push(path);
    }
    out.paths.sort_by_key(|p| (p.class, natural(&p.name)));
    out.via_kinds = vias.values().filter(|v| v.is_ok()).count();
    if !setup.vias_3d {
        out.notes.push(
            "vias are coaxial estimates (quick); run without --quick-vias to solve them in 3D"
                .into(),
        );
    }
    if setup.loads.is_empty() {
        out.notes.push(
            "no chip port models given: return loss is into 50 Ω (100 Ω for a pair) at the chip pin; give --load PATTERN=chip.s1p for the real one"
                .into(),
        );
    }
    progress(1.0, "done");
    Ok(out)
}

/// Sort key that puts RX2 before RX10.
fn natural(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
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
}

// ---------------------------------------------------------------------
// Output.

/// Standard marks inside the band, and its top.
pub fn marks(band: (f64, f64)) -> Vec<f64> {
    let mut m: Vec<f64> = [
        0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 30.0, 40.0,
    ]
    .into_iter()
    .filter(|f| *f >= band.0 - 1e-9 && *f < band.1 - 1e-9)
    .collect();
    if m.len() > 4 {
        // Keep it readable: the bottom, two between, the top.
        let k = m.len();
        m = vec![m[0], m[k / 3], m[2 * k / 3]];
    }
    m.push(band.1);
    m
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl RfReport {
    pub fn failing(&self) -> usize {
        self.paths
            .iter()
            .filter(|p| p.status != Status::Pass)
            .count()
    }

    pub fn summary(&self) -> String {
        let ok = self
            .paths
            .iter()
            .filter(|p| p.status == Status::Pass)
            .count();
        let check = self
            .paths
            .iter()
            .filter(|p| p.status == Status::Check)
            .count();
        let failed = self.paths.len() - ok - check;
        format!(
            "{} RF path(s), {}-{} GHz: {ok} pass, {check} to check{}; return loss goal {:.0} dB{}; {} kind(s) of via solved in 3D",
            self.paths.len(),
            self.band.0,
            self.band.1,
            if failed > 0 {
                format!(", {failed} not followed")
            } else {
                String::new()
            },
            self.rl_goal,
            self.il_budget
                .map_or(String::new(), |b| format!(", insertion loss budget {b:.2} dB")),
            self.via_kinds
        )
    }

    pub fn text(&self) -> String {
        let m = marks(self.band);
        let mut o = String::new();
        let _ = writeln!(o, "Whole-board RF report");
        let _ = writeln!(o, "{}", self.summary());
        let _ = writeln!(o);
        let mut head = format!("{:<6} {:<22} {:<12} {:<12}", "class", "path", "from", "to");
        for f in &m {
            let _ = write!(head, " {:>8}", format!("IL@{f}"));
        }
        let _ = write!(
            head,
            " {:>14} {:>7} {:>5} {:>5}  status",
            "worst RL", "length", "vias", "parts"
        );
        let _ = writeln!(o, "{head}");
        for p in &self.paths {
            let (v, parts) = p.counts();
            let mut line = format!(
                "{:<6} {:<22} {:<12} {:<12}",
                p.class.name(),
                p.name,
                p.report
                    .as_ref()
                    .map_or(p.from.as_str(), |r| r.start.as_str()),
                p.report.as_ref().map_or("-", |r| r.end.as_str())
            );
            for f in &m {
                let _ = write!(
                    line,
                    " {:>8}",
                    p.il_at(*f).map_or("-".into(), |d| format!("{d:.2}"))
                );
            }
            let rl = p.worst_rl().map_or("-".to_string(), |(d, f)| {
                format!("{d:.1}@{f:.2}{}", if p.load.is_some() { "*" } else { "" })
            });
            let len = p
                .report
                .as_ref()
                .map_or("-".to_string(), |r| format!("{:.2} in", r.length / 1000.0));
            let _ = write!(
                line,
                " {rl:>14} {len:>7} {v:>5} {parts:>5}  {}",
                p.status.name()
            );
            let _ = writeln!(o, "{line}");
        }
        let _ = writeln!(
            o,
            "\nIL in dB at GHz; worst RL in dB@GHz from {} GHz (* = into the chip's port model).",
            self.band.0
        );
        for p in &self.paths {
            let _ = writeln!(o, "\n{} {} ({}):", p.class.name(), p.name, p.status.name());
            match &p.report {
                Err(e) => {
                    let _ = writeln!(o, "  not followed from {}: {e}", p.from);
                }
                Ok(r) => {
                    let _ = writeln!(o, "  {}", r.summary());
                    if let Some(l) = &p.load {
                        let _ = writeln!(o, "  load: {l}");
                    }
                    for f in &p.findings {
                        let _ = writeln!(o, "  ! {f}");
                    }
                    if !p.reflections.is_empty() {
                        let _ = writeln!(o, "  reflects most:");
                        for (w, g, at) in &p.reflections {
                            let _ = writeln!(
                                o,
                                "    |Γ| {g:.2}  {w}  at ({:.0}, {:.0}) mil",
                                at[0], at[1]
                            );
                        }
                    }
                    if !p.losses.is_empty() {
                        let _ = writeln!(o, "  loses most at {} GHz:", self.band.1);
                        for (w, d, _) in &p.losses {
                            let _ = writeln!(o, "    {d:.3} dB  {w}");
                        }
                    }
                    for n in &r.notes {
                        let _ = writeln!(o, "  note: {n}");
                    }
                }
            }
        }
        for n in &self.notes {
            let _ = writeln!(o, "\nNote: {n}");
        }
        o
    }

    pub fn csv(&self) -> String {
        let m = marks(self.band);
        let mut o = String::from("class,path,from,to,load");
        for f in &m {
            let _ = write!(o, ",il_db_{f}ghz");
        }
        o.push_str(",worst_rl_db,worst_rl_ghz,length_in,vias,parts,status,finding\n");
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        for p in &self.paths {
            let _ = write!(
                o,
                "{},{},{},{},{}",
                p.class.name(),
                q(&p.name),
                q(p.report
                    .as_ref()
                    .map_or(p.from.as_str(), |r| r.start.as_str())),
                q(p.report.as_ref().map_or("", |r| r.end.as_str())),
                q(p.load.as_deref().unwrap_or(""))
            );
            for f in &m {
                let _ = write!(
                    o,
                    ",{}",
                    p.il_at(*f).map_or(String::new(), |d| format!("{d:.4}"))
                );
            }
            let (v, parts) = p.counts();
            let (rl, rf) = p
                .worst_rl()
                .map_or((String::new(), String::new()), |(d, f)| {
                    (format!("{d:.2}"), format!("{f:.3}"))
                });
            let len = p
                .report
                .as_ref()
                .map_or(String::new(), |r| format!("{:.3}", r.length / 1000.0));
            let _ = writeln!(
                o,
                ",{rl},{rf},{len},{v},{parts},{},{}",
                p.status.name(),
                q(p.findings
                    .first()
                    .map_or(p.report.as_ref().err().map_or("", |e| e.as_str()), |s| s
                        .as_str()))
            );
        }
        o
    }

    /// Every path's insertion and return loss on one chart, coloured by
    /// class.
    pub fn svg(&self, title: &str) -> String {
        let ok: Vec<&Path> = self.paths.iter().filter(|p| p.report.is_ok()).collect();
        let rows = ok.len().max(1);
        let legend_h = 18.0 * rows as f64 + 20.0;
        let (w, l, r) = (980.0, 64.0, 300.0);
        let h = 640.0f64.max(80.0 + legend_h);
        let pw = w - l - r;
        let fmax = self.band.1.max(1e-9);
        let x = |f: f64| l + pw * f / fmax;
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
        let il_lo = ok
            .iter()
            .filter_map(|p| p.report.as_ref().ok())
            .flat_map(|r| r.s.iter().map(|s| s[2].db()))
            .fold(0.0f64, f64::min)
            .max(-40.0);
        let nice = |v: f64| {
            let steps = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0];
            let s = steps.into_iter().find(|s| v / s <= 8.0).unwrap_or(20.0);
            (((v / s).ceil()).max(1.0) * s, s)
        };
        let (il_span, il_step) = nice(-il_lo);
        let (rl_span, rl_step) = (40.0, 5.0);
        let rl_of = |p: &Path| -> Vec<Cx> {
            let r = p.report.as_ref().unwrap();
            if r.s11_loaded.is_empty() {
                r.s.iter().map(|s| s[0]).collect()
            } else {
                r.s11_loaded.clone()
            }
        };
        let panels = [
            (44.0, 310.0, il_span, il_step, "insertion |S21|, dB"),
            (
                350.0,
                h - 50.0,
                rl_span,
                rl_step,
                "return |S11|, dB (into the chip where a model is given)",
            ),
        ];
        for (k, (t, b, span, step, name)) in panels.iter().enumerate() {
            let y = |d: f64| t + (b - t) * (-d).clamp(0.0, *span) / span;
            let mut d = 0.0;
            while d <= span + 1e-9 {
                let _ = writeln!(
                    o,
                    r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#e3e6ec"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{}</text>"##,
                    l + pw,
                    y(-d),
                    y(-d),
                    l - 6.0,
                    y(-d) + 4.0,
                    if d == 0.0 {
                        "0".to_string()
                    } else {
                        format!("-{}", trim(d))
                    }
                );
                d += step;
            }
            let fstep = if fmax > 12.0 {
                2.0
            } else if fmax > 3.0 {
                1.0
            } else {
                0.5
            };
            let mut f = 0.0;
            while f <= fmax + 1e-9 {
                let _ = writeln!(
                    o,
                    r##"<line x1="{:.1}" x2="{:.1}" y1="{t}" y2="{b}" stroke="#eef0f4"/><text x="{:.1}" y="{}" text-anchor="middle" fill="#555">{}</text>"##,
                    x(f),
                    x(f),
                    x(f),
                    b + 16.0,
                    trim(f)
                );
                f += fstep;
            }
            let _ = writeln!(
                o,
                r##"<text x="{l}" y="{}" fill="#222">{}</text>"##,
                t - 6.0,
                esc(name)
            );
            if k == 1 {
                // The goal and the band.
                let _ = writeln!(
                    o,
                    r##"<line x1="{:.1}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#c0392b" stroke-dasharray="6 4"/><rect x="{l}" y="{t}" width="{:.1}" height="{:.1}" fill="#000" fill-opacity="0.04"/>"##,
                    x(self.band.0),
                    l + pw,
                    y(-self.rl_goal),
                    y(-self.rl_goal),
                    (x(self.band.0) - l).max(0.0),
                    b - t
                );
            }
            for (i, p) in ok.iter().enumerate() {
                let rep = p.report.as_ref().unwrap();
                let vals: Vec<f64> = if k == 0 {
                    rep.s.iter().map(|s| s[2].db()).collect()
                } else {
                    rl_of(p).iter().map(|s| s.db()).collect()
                };
                let c = p.class.rgb();
                let dash = ["", " stroke-dasharray=\"6 3\"", " stroke-dasharray=\"2 3\""][i % 3];
                let mut d = String::new();
                for (j, (f, v)) in rep.f.iter().zip(&vals).enumerate() {
                    let _ = write!(
                        d,
                        "{}{:.1},{:.1}",
                        if j == 0 { "M" } else { " L" },
                        x(*f),
                        y(*v)
                    );
                }
                let _ = writeln!(
                    o,
                    r##"<path d="{d}" fill="none" stroke="rgb({},{},{})" stroke-width="1.6"{dash}/>"##,
                    c[0], c[1], c[2]
                );
            }
        }
        // Legend.
        let lx = l + pw + 20.0;
        for (i, p) in ok.iter().enumerate() {
            let yy = 56.0 + 18.0 * i as f64;
            let c = p.class.rgb();
            let dash = ["", " stroke-dasharray=\"6 3\"", " stroke-dasharray=\"2 3\""][i % 3];
            let _ = writeln!(
                o,
                r##"<line x1="{lx}" x2="{}" y1="{yy}" y2="{yy}" stroke="rgb({},{},{})" stroke-width="2"{dash}/><text x="{}" y="{}" fill="{}">{} {}{}</text>"##,
                lx + 22.0,
                c[0],
                c[1],
                c[2],
                lx + 28.0,
                yy + 4.0,
                if p.status == Status::Pass {
                    "#222"
                } else {
                    "#c0392b"
                },
                p.class.name(),
                esc(&p.name),
                if p.status == Status::Pass {
                    ""
                } else {
                    " (check)"
                }
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="{}" y="{}" text-anchor="middle" fill="#555">GHz</text></svg>"##,
            l + pw / 2.0,
            h - 12.0
        );
        o
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_from_names() {
        assert_eq!(class_of("RX1_IN_P"), Some(Class::Rx));
        assert_eq!(class_of("ORX2_P"), Some(Class::Orx));
        assert_eq!(class_of("TX3_OUT_N"), Some(Class::Tx));
        assert_eq!(class_of("EXT_LO1_P"), Some(Class::Lo));
        assert_eq!(class_of("DEV_CLK_IN"), Some(Class::Clock));
        assert_eq!(class_of("LOGO2"), None);
        assert_eq!(class_of("Net_517"), None);
        assert!(generic("Net_517"));
        assert!(generic("N12345"));
        assert!(!generic("RX1_IN_P"));
        assert_eq!(stem("RX1_IN_P"), "RX1_IN");
    }

    #[test]
    fn coax_by_word_not_by_part_of_one() {
        assert!(coax_words("SMA_EDGE_142-0701"));
        assert!(coax_words("U.FL-R-SMT"));
        assert!(!coax_words("SMALL_OUTLINE"));
    }

    #[test]
    fn finds_the_sma_and_runs_its_path_to_the_chip() {
        let mut b = crate::pathloss::tests::board();
        b.nets[1].name = "RX1_IN".into();
        // The SMA's footprint names it; a second ground pad.
        b.components[0].footprint = "SMA_EDGE".into();
        let mut gp = b.geometry.pads[1].clone();
        gp.name = "3".into();
        b.geometry.pads.push(gp);
        assert_eq!(
            starts(&b, &[]),
            vec![("J1.1".to_string(), "J1".to_string())]
        );
        // Named only by its maker's number: one signal pin among grounds.
        b.components[0].footprint = "142-0701-801".into();
        b.components[0].params.clear();
        assert_eq!(starts(&b, &[]).len(), 1);
        let setup = Setup {
            band: (0.5, 6.0),
            points: 30,
            vias_3d: false,
            ..Default::default()
        };
        let r = run(&b, &b.stackup, &setup, &|_, _| {}).expect("report");
        assert_eq!(r.paths.len(), 1);
        let p = &r.paths[0];
        assert_eq!(p.class, Class::Rx);
        assert_eq!(p.name, "RX1_IN");
        let rep = p.report.as_ref().expect("path");
        assert_eq!((rep.start.as_str(), rep.end.as_str()), ("J1.1", "U1.1"));
        assert!(p.il_at(6.0).is_some_and(|d| d > 0.0 && d < 3.0));
        assert!(r.text().contains("RX1_IN"));
        assert!(r.csv().lines().count() == 2);
        assert!(r.svg("t").starts_with("<svg"));
        // A tight goal makes it a path to check.
        let strict = Setup {
            rl_goal: 60.0,
            ..setup
        };
        let r = run(&b, &b.stackup, &strict, &|_, _| {}).expect("report");
        assert_eq!(r.paths[0].status, Status::Check);
        assert_eq!(r.failing(), 1);
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["RX10", "RX2", "RX1"];
        v.sort_by_key(|s| natural(s));
        assert_eq!(v, vec!["RX1", "RX2", "RX10"]);
    }

    #[test]
    fn marks_stay_inside_the_band() {
        assert_eq!(marks((0.5, 6.0)), vec![0.5, 2.0, 4.0, 6.0]);
        assert_eq!(marks((1.0, 3.0)), vec![1.0, 2.0, 3.0]);
    }
}
