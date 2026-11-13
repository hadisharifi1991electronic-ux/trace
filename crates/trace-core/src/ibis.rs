//! IBIS (I/O Buffer Information Specification) files: the components, their
//! pins and package parasitics, and the buffer models with their V-I tables,
//! ramps and switching waveforms. Units are SI (volts, amps, seconds, ohms,
//! farads, henries).
//!
//! Only what a behavioural buffer simulation needs is kept. Submodels,
//! package models, external (SPICE or AMI) models and series switches are
//! skipped.

use std::collections::HashMap;

/// Process corner: the typ, min (slow, weak) or max (fast, strong) column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Corner {
    #[default]
    Typ,
    Min,
    Max,
}

impl Corner {
    pub fn label(self) -> &'static str {
        match self {
            Corner::Typ => "Typical",
            Corner::Min => "Slow (min)",
            Corner::Max => "Fast (max)",
        }
    }
}

/// A typ/min/max value; a missing min or max takes the typical one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tmm {
    pub typ: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Tmm {
    pub fn at(&self, c: Corner) -> f64 {
        match c {
            Corner::Typ => self.typ,
            Corner::Min => self.min.unwrap_or(self.typ),
            Corner::Max => self.max.unwrap_or(self.typ),
        }
    }
    fn parse(words: &[&str]) -> Option<Tmm> {
        let typ = num(words.first()?)?;
        Some(Tmm {
            typ,
            min: words.get(1).and_then(|w| num(w)),
            max: words.get(2).and_then(|w| num(w)),
        })
    }
}

/// A V-I table: voltage and the current at each corner. For the pullup and
/// power clamp, the voltage is the supply minus the pin voltage, as in the
/// file. Current is positive into the pin.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub rows: Vec<(f64, Tmm)>,
}

impl Table {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// The current at `v`, interpolated, and held at the ends.
    pub fn at(&self, v: f64, c: Corner) -> f64 {
        interp(&self.rows, v, c)
    }
}

fn interp(rows: &[(f64, Tmm)], x: f64, c: Corner) -> f64 {
    match rows.len() {
        0 => 0.0,
        1 => rows[0].1.at(c),
        _ => {
            if x <= rows[0].0 {
                return rows[0].1.at(c);
            }
            let last = rows.len() - 1;
            if x >= rows[last].0 {
                return rows[last].1.at(c);
            }
            let i = rows.partition_point(|r| r.0 <= x).clamp(1, last);
            let (x0, y0) = (rows[i - 1].0, rows[i - 1].1.at(c));
            let (x1, y1) = (rows[i].0, rows[i].1.at(c));
            if (x1 - x0).abs() < 1e-30 {
                y1
            } else {
                y0 + (y1 - y0) * (x - x0) / (x1 - x0)
            }
        }
    }
}

/// A switching waveform into a fixture.
#[derive(Clone, Debug, Default)]
pub struct Waveform {
    pub r_fixture: f64,
    pub v_fixture: Tmm,
    pub c_fixture: f64,
    pub l_fixture: f64,
    /// Time and pin voltage.
    pub rows: Vec<(f64, Tmm)>,
}

impl Waveform {
    pub fn at(&self, t: f64, c: Corner) -> f64 {
        interp(&self.rows, t, c)
    }
    pub fn duration(&self) -> f64 {
        self.rows.last().map_or(0.0, |r| r.0) - self.rows.first().map_or(0.0, |r| r.0)
    }
}

/// [Ramp]: the 20 to 80 % swing and the time it takes, rising and falling,
/// into `r_load`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ramp {
    pub rise: (Tmm, Tmm),
    pub fall: (Tmm, Tmm),
    pub r_load: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Input,
    #[default]
    Output,
    Io,
    ThreeState,
    OpenDrain,
    OpenSink,
    OpenSource,
    Terminator,
    /// Series, ECL and other kinds not modelled.
    Other,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s.to_ascii_lowercase().replace('_', "/").as_str() {
            "input" | "input/ecl" | "input/diff" => Kind::Input,
            "output" | "output/ecl" => Kind::Output,
            "i/o" | "i/o/ecl" | "i/o/open/drain" | "i/o/open/sink" | "i/o/open/source" => Kind::Io,
            "3-state" | "3-state/ecl" => Kind::ThreeState,
            "open/drain" => Kind::OpenDrain,
            "open/sink" => Kind::OpenSink,
            "open/source" => Kind::OpenSource,
            "terminator" => Kind::Terminator,
            _ => Kind::Other,
        }
    }
    /// Whether the model can drive a line.
    pub fn drives(self) -> bool {
        !matches!(self, Kind::Input | Kind::Terminator | Kind::Other)
    }
}

/// One buffer model.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub name: String,
    pub kind: Kind,
    pub c_comp: Tmm,
    pub voltage: Option<Tmm>,
    pub pullup_ref: Option<Tmm>,
    pub pulldown_ref: Option<Tmm>,
    pub power_clamp_ref: Option<Tmm>,
    pub gnd_clamp_ref: Option<Tmm>,
    pub vinl: Option<f64>,
    pub vinh: Option<f64>,
    pub pulldown: Table,
    pub pullup: Table,
    pub gnd_clamp: Table,
    pub power_clamp: Table,
    pub ramp: Option<Ramp>,
    pub rising: Vec<Waveform>,
    pub falling: Vec<Waveform>,
    /// Terminator resistors to ground and to the supply.
    pub rgnd: Option<Tmm>,
    pub rpower: Option<Tmm>,
}

impl Model {
    /// The supply the pullup and power clamp are referenced to.
    pub fn vcc(&self, c: Corner) -> f64 {
        self.pullup_ref
            .or(self.voltage)
            .or(self.power_clamp_ref)
            .map_or(0.0, |v| v.at(c))
    }
    /// The ground the pulldown is referenced to.
    pub fn vss(&self, c: Corner) -> f64 {
        self.pulldown_ref.map_or(0.0, |v| v.at(c))
    }
    pub fn power_clamp_vcc(&self, c: Corner) -> f64 {
        self.power_clamp_ref
            .or(self.voltage)
            .or(self.pullup_ref)
            .map_or(0.0, |v| v.at(c))
    }
    pub fn gnd_clamp_vss(&self, c: Corner) -> f64 {
        self.gnd_clamp_ref.map_or(0.0, |v| v.at(c))
    }

    /// Current into the pin at pin voltage `v` with the pullup on by `ku`
    /// and the pulldown by `kd` (0 to 1), clamps and terminator resistors
    /// included.
    pub fn current(&self, v: f64, ku: f64, kd: f64, c: Corner) -> f64 {
        let vcc = self.vcc(c);
        let vss = self.vss(c);
        let mut i = 0.0;
        if ku > 0.0 {
            i += ku * self.pullup.at(vcc - v, c);
        }
        if kd > 0.0 {
            i += kd * self.pulldown.at(v - vss, c);
        }
        i += self.power_clamp.at(self.power_clamp_vcc(c) - v, c);
        i += self.gnd_clamp.at(v - self.gnd_clamp_vss(c), c);
        if let Some(r) = self.rgnd.map(|r| r.at(c)).filter(|r| *r > 0.0) {
            i += (v - self.gnd_clamp_vss(c)) / r;
        }
        if let Some(r) = self.rpower.map(|r| r.at(c)).filter(|r| *r > 0.0) {
            i += (v - self.power_clamp_vcc(c)) / r;
        }
        i
    }

    /// The slope of [`Model::current`] in `v`, numerically.
    pub fn conductance(&self, v: f64, ku: f64, kd: f64, c: Corner) -> f64 {
        let h = 1e-4;
        (self.current(v + h, ku, kd, c) - self.current(v - h, ku, kd, c)) / (2.0 * h)
    }

    /// The input thresholds: Vinl and Vinh, or a third and two thirds of
    /// the supply.
    pub fn thresholds(&self, c: Corner) -> (f64, f64) {
        let vcc = self.vcc(c).max(self.power_clamp_vcc(c));
        (
            self.vinl.unwrap_or(vcc / 3.0),
            self.vinh.unwrap_or(2.0 * vcc / 3.0),
        )
    }
}

/// Package or pin parasitics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rlc {
    pub r: f64,
    pub l: f64,
    pub c: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Pin {
    pub name: String,
    pub signal: String,
    pub model: String,
    /// The pin's own parasitics, when the [Pin] table gives them.
    pub rlc: Option<Rlc>,
}

#[derive(Clone, Debug, Default)]
pub struct Component {
    pub name: String,
    pub package: Rlc,
    pub pins: Vec<Pin>,
    /// Differential pins: pin and its inverting pin.
    pub diff_pins: Vec<(String, String)>,
}

impl Component {
    /// The package parasitics of a pin: its own, or the component's.
    pub fn rlc(&self, pin: &Pin) -> Rlc {
        pin.rlc.unwrap_or(self.package)
    }
    pub fn pin(&self, name: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
    /// The inverting pin of a differential pin, either way round.
    pub fn partner(&self, name: &str) -> Option<&str> {
        self.diff_pins.iter().find_map(|(a, b)| {
            if a.eq_ignore_ascii_case(name) {
                Some(b.as_str())
            } else if b.eq_ignore_ascii_case(name) {
                Some(a.as_str())
            } else {
                None
            }
        })
    }
}

/// A parsed IBIS file.
#[derive(Clone, Debug, Default)]
pub struct Ibis {
    pub version: String,
    pub components: Vec<Component>,
    pub models: Vec<Model>,
    /// [Model Selector]: name and the models it offers, first the default.
    pub selectors: HashMap<String, Vec<String>>,
}

impl Ibis {
    /// A model by name, or the first model of a selector by that name.
    pub fn model(&self, name: &str) -> Option<&Model> {
        let find = |n: &str| self.models.iter().find(|m| m.name.eq_ignore_ascii_case(n));
        find(name).or_else(|| {
            self.selectors
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.iter().find_map(|n| find(n)))
        })
    }
}

/// A number as IBIS writes it: an optional scale letter (T G M k m u n p f)
/// and unit after the digits; `NA` is none.
pub fn num(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches('=');
    if s.eq_ignore_ascii_case("na") || s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s0 = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s0
    };
    let mut any = digits(&mut i);
    if i < b.len() && b[i] == b'.' {
        i += 1;
        any |= digits(&mut i);
    }
    if !any {
        return None;
    }
    // An exponent only when digits follow.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            digits(&mut i);
        }
    }
    let v: f64 = s[..i].parse().ok()?;
    let scale = match b.get(i) {
        Some(b'T') => 1e12,
        Some(b'G') => 1e9,
        Some(b'M') => 1e6,
        Some(b'k') | Some(b'K') => 1e3,
        Some(b'm') => 1e-3,
        Some(b'u') => 1e-6,
        Some(b'n') => 1e-9,
        Some(b'p') => 1e-12,
        Some(b'f') => 1e-15,
        _ => 1.0,
    };
    Some(v * scale)
}

/// What the data lines under the last keyword are.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    None,
    Package,
    Pin,
    DiffPin,
    ModelParams,
    Pulldown,
    Pullup,
    GndClamp,
    PowerClamp,
    Ramp,
    Rising,
    Falling,
    Selector,
}

/// Reads an IBIS file's text.
pub fn parse(text: &str) -> Result<Ibis, String> {
    let mut ibis = Ibis::default();
    let mut comment = '|';
    let mut section = Section::None;
    let mut selector: Option<String> = None;
    // Inside a section to skip whole (package or external models,
    // submodels) until its end keyword.
    let mut skip_until: Option<&'static str> = None;
    for raw in text.lines() {
        let line = match raw.find(comment) {
            Some(i) => &raw[..i],
            None => raw,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else {
                continue;
            };
            let key = rest[..end]
                .to_ascii_lowercase()
                .replace('_', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let args = rest[end + 1..].trim();
            if let Some(stop) = skip_until {
                if key == stop {
                    skip_until = None;
                }
                continue;
            }
            let words: Vec<&str> = args.split_whitespace().collect();
            section = Section::None;
            match key.as_str() {
                "ibis ver" => ibis.version = args.to_string(),
                "comment char" => {
                    if let Some(c) = args.chars().next() {
                        comment = c;
                    }
                }
                "component" => ibis.components.push(Component {
                    name: args.to_string(),
                    ..Default::default()
                }),
                "package" => section = Section::Package,
                "pin" => section = Section::Pin,
                "diff pin" => section = Section::DiffPin,
                "model selector" => {
                    selector = Some(args.to_string());
                    ibis.selectors.insert(args.to_string(), Vec::new());
                    section = Section::Selector;
                }
                "model" => {
                    ibis.models.push(Model {
                        name: args.to_string(),
                        ..Default::default()
                    });
                    section = Section::ModelParams;
                }
                "submodel" => skip_until = Some("end submodel"),
                "define package model" => skip_until = Some("end package model"),
                "external model" => skip_until = Some("end external model"),
                "external circuit" => skip_until = Some("end external circuit"),
                "algorithmic model" => skip_until = Some("end algorithmic model"),
                "begin board description" => skip_until = Some("end board description"),
                "end" => break,
                _ => {
                    let Some(m) = ibis.models.last_mut() else {
                        continue;
                    };
                    match key.as_str() {
                        "voltage range" => m.voltage = Tmm::parse(&words),
                        "pullup reference" => m.pullup_ref = Tmm::parse(&words),
                        "pulldown reference" => m.pulldown_ref = Tmm::parse(&words),
                        "power clamp reference" => m.power_clamp_ref = Tmm::parse(&words),
                        "gnd clamp reference" => m.gnd_clamp_ref = Tmm::parse(&words),
                        "rgnd" => m.rgnd = Tmm::parse(&words),
                        "rpower" => m.rpower = Tmm::parse(&words),
                        "pulldown" => section = Section::Pulldown,
                        "pullup" => section = Section::Pullup,
                        "gnd clamp" => section = Section::GndClamp,
                        "power clamp" => section = Section::PowerClamp,
                        "ramp" => section = Section::Ramp,
                        "rising waveform" => {
                            m.rising.push(Waveform::default());
                            section = Section::Rising;
                        }
                        "falling waveform" => {
                            m.falling.push(Waveform::default());
                            section = Section::Falling;
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        if skip_until.is_some() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        // "Name = value" and "Name value" alike.
        let kv = || -> Option<(String, Vec<&str>)> {
            let (k, rest) = match line.split_once('=') {
                Some((k, v)) => (k.trim(), v.split_whitespace().collect()),
                None => (words[0], words[1..].to_vec()),
            };
            Some((k.to_ascii_lowercase(), rest))
        };
        match section {
            Section::None => {}
            Section::Package => {
                let Some(c) = ibis.components.last_mut() else {
                    continue;
                };
                if let Some((k, v)) = kv() {
                    let x = Tmm::parse(&v).map_or(0.0, |t| t.typ);
                    match k.as_str() {
                        "r_pkg" => c.package.r = x,
                        "l_pkg" => c.package.l = x,
                        "c_pkg" => c.package.c = x,
                        _ => {}
                    }
                }
            }
            Section::Pin => {
                let Some(c) = ibis.components.last_mut() else {
                    continue;
                };
                if words.len() < 3 {
                    continue;
                }
                let rlc = (words.len() >= 6).then(|| Rlc {
                    r: num(words[3]).unwrap_or(c.package.r),
                    l: num(words[4]).unwrap_or(c.package.l),
                    c: num(words[5]).unwrap_or(c.package.c),
                });
                c.pins.push(Pin {
                    name: words[0].to_string(),
                    signal: words[1].to_string(),
                    model: words[2].to_string(),
                    rlc,
                });
            }
            Section::DiffPin => {
                if let (Some(c), true) = (ibis.components.last_mut(), words.len() >= 2) {
                    c.diff_pins
                        .push((words[0].to_string(), words[1].to_string()));
                }
            }
            Section::Selector => {
                if let (Some(s), Some(w)) = (&selector, words.first()) {
                    if let Some(list) = ibis.selectors.get_mut(s) {
                        list.push(w.to_string());
                    }
                }
            }
            Section::ModelParams => {
                let Some(m) = ibis.models.last_mut() else {
                    continue;
                };
                let Some((k, v)) = kv() else { continue };
                match k.as_str() {
                    "model_type" => m.kind = v.first().map_or(Kind::Other, |s| Kind::parse(s)),
                    "c_comp" => m.c_comp = Tmm::parse(&v).unwrap_or_default(),
                    // IBIS 4: C_comp split by where it sits; their sum is
                    // the die capacitance.
                    "c_comp_pullup" | "c_comp_pulldown" | "c_comp_power_clamp"
                    | "c_comp_gnd_clamp" => {
                        if let Some(t) = Tmm::parse(&v) {
                            let add = |a: Option<f64>, b: Option<f64>, base: f64, bt: f64| {
                                Some(a.unwrap_or(base) + b.unwrap_or(bt))
                            };
                            let c = m.c_comp;
                            m.c_comp = Tmm {
                                typ: c.typ + t.typ,
                                min: add(c.min, t.min, c.typ, t.typ),
                                max: add(c.max, t.max, c.typ, t.typ),
                            };
                        }
                    }
                    "vinl" => m.vinl = v.first().and_then(|s| num(s)),
                    "vinh" => m.vinh = v.first().and_then(|s| num(s)),
                    _ => {}
                }
            }
            Section::Pulldown | Section::Pullup | Section::GndClamp | Section::PowerClamp => {
                let Some(m) = ibis.models.last_mut() else {
                    continue;
                };
                let (Some(v), Some(i)) = (num(words[0]), Tmm::parse(&words[1..])) else {
                    continue;
                };
                let t = match section {
                    Section::Pulldown => &mut m.pulldown,
                    Section::Pullup => &mut m.pullup,
                    Section::GndClamp => &mut m.gnd_clamp,
                    _ => &mut m.power_clamp,
                };
                t.rows.push((v, i));
            }
            Section::Ramp => {
                let Some(m) = ibis.models.last_mut() else {
                    continue;
                };
                let Some((k, v)) = kv() else { continue };
                let ramp = m.ramp.get_or_insert(Ramp {
                    r_load: 50.0,
                    ..Default::default()
                });
                let pair = |v: &[&str]| -> Option<(Tmm, Tmm)> {
                    let split: Vec<(Option<f64>, Option<f64>)> = v
                        .iter()
                        .take(3)
                        .map(|w| match w.split_once('/') {
                            Some((a, b)) => (num(a), num(b)),
                            None => (None, None),
                        })
                        .collect();
                    let (dv, dt) = *split.first()?;
                    type Pick = fn(&(Option<f64>, Option<f64>)) -> Option<f64>;
                    let get = |i: usize, f: Pick| split.get(i).and_then(f);
                    Some((
                        Tmm {
                            typ: dv?,
                            min: get(1, |p| p.0),
                            max: get(2, |p| p.0),
                        },
                        Tmm {
                            typ: dt?,
                            min: get(1, |p| p.1),
                            max: get(2, |p| p.1),
                        },
                    ))
                };
                match k.as_str() {
                    "dv/dt_r" => {
                        if let Some(p) = pair(&v) {
                            ramp.rise = p;
                        }
                    }
                    "dv/dt_f" => {
                        if let Some(p) = pair(&v) {
                            ramp.fall = p;
                        }
                    }
                    "r_load" => ramp.r_load = v.first().and_then(|s| num(s)).unwrap_or(50.0),
                    _ => {}
                }
            }
            Section::Rising | Section::Falling => {
                let Some(m) = ibis.models.last_mut() else {
                    continue;
                };
                let w = match section {
                    Section::Rising => m.rising.last_mut(),
                    _ => m.falling.last_mut(),
                };
                let Some(w) = w else { continue };
                if let Some(t) = num(words[0]).filter(|_| !line.contains('=')) {
                    if let Some(v) = Tmm::parse(&words[1..]) {
                        w.rows.push((t, v));
                    }
                    continue;
                }
                let Some((k, v)) = kv() else { continue };
                let x = v.first().and_then(|s| num(s));
                match k.as_str() {
                    "r_fixture" => w.r_fixture = x.unwrap_or(50.0),
                    "v_fixture" => w.v_fixture.typ = x.unwrap_or(0.0),
                    "v_fixture_min" => w.v_fixture.min = x,
                    "v_fixture_max" => w.v_fixture.max = x,
                    "c_fixture" => w.c_fixture = x.unwrap_or(0.0),
                    "l_fixture" => w.l_fixture = x.unwrap_or(0.0),
                    _ => {}
                }
            }
        }
    }
    if ibis.models.is_empty() {
        return Err("no [Model] in the file".into());
    }
    for m in &mut ibis.models {
        for t in [
            &mut m.pulldown,
            &mut m.pullup,
            &mut m.gnd_clamp,
            &mut m.power_clamp,
        ] {
            t.rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    Ok(ibis)
}

/// Reads an IBIS file from disk.
pub fn load(path: &std::path::Path) -> Result<Ibis, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&String::from_utf8_lossy(&bytes))
}

#[cfg(test)]
pub(crate) const SAMPLE: &str = r#"
[IBIS Ver]   4.2
[File Name]  sample.ibs
[Component]  DEMO
[Manufacturer] Trace
[Package]
|        typ    min    max
R_pkg    0.2    0.1    0.3
L_pkg    2nH    1nH    3nH
C_pkg    0.5pF  0.4pF  0.6pF
[Pin]  signal_name  model_name
1      OUT          drv33
2      IN           rcv33
3      OUT_P        drv33
4      OUT_N        drv33
[Diff Pin] inv_pin vdiff tdelay_typ tdelay_min tdelay_max
3          4       0.1   0          NA         NA
[Model Selector] drv
drv33   3.3 V driver
[Model]  drv33
Model_type   Output
C_comp       2pF   1.5pF  2.5pF
Vinl = 0.8
Vinh = 2.0
[Voltage Range]  3.3V  3.0V  3.6V
[Pulldown]
| V      I(typ)    I(min)    I(max)
-3.3     -0.132    -0.110    -0.150
0.0      0         0         0
3.3      0.132     0.110     0.150
6.6      0.132     0.110     0.150
[Pullup]
-3.3     0.132     0.110     0.150
0.0      0         0         0
3.3      -0.132    -0.110    -0.150
6.6      -0.132    -0.110    -0.150
[Ramp]
dV/dt_r   1.98/0.25n   1.80/0.35n   2.16/0.18n
dV/dt_f   1.98/0.25n   1.80/0.35n   2.16/0.18n
R_load = 50
[Rising Waveform]
R_fixture = 50
V_fixture = 0
0.0n     0.0    0.0   0.0
0.5n     1.6    1.5   1.7
1.0n     1.65   1.55  1.75
[Model]  rcv33
Model_type   Input
C_comp       3pF   NA     NA
[Voltage Range]  3.3V  3.0V  3.6V
[GND Clamp]
-3.3     -1.0      NA        NA
-0.7     -0.01     NA        NA
0.0      0         NA        NA
3.3      0         NA        NA
[POWER Clamp]
-3.3     1.0       NA        NA
-0.7     0.01      NA        NA
0.0      0         NA        NA
3.3      0         NA        NA
[Submodel] hidden
[Pulldown]
0  0  0  0
[End Submodel]
[End]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_scale_and_units() {
        assert_eq!(num("2pF"), Some(2e-12));
        assert_eq!(num("3.3V"), Some(3.3));
        assert_eq!(num("1.5e-9"), Some(1.5e-9));
        assert_eq!(num("4.7k"), Some(4700.0));
        assert_eq!(num("-0.5m"), Some(-0.5e-3));
        assert!((num("2M").unwrap() - 2e6).abs() < 1e-6);
        assert_eq!(num("NA"), None);
        assert_eq!(num("=50"), Some(50.0));
        assert_eq!(num("1500hm"), Some(1500.0));
    }

    #[test]
    fn reads_the_sample() {
        let f = parse(SAMPLE).unwrap();
        assert_eq!(f.components.len(), 1);
        let c = &f.components[0];
        assert_eq!(c.pins.len(), 4);
        assert!((c.package.l - 2e-9).abs() < 1e-15);
        assert_eq!(c.partner("4"), Some("3"));
        assert_eq!(f.models.len(), 2);
        let d = f.model("drv33").unwrap();
        assert_eq!(d.kind, Kind::Output);
        assert!((d.c_comp.typ - 2e-12).abs() < 1e-18);
        assert!((d.vcc(Corner::Typ) - 3.3).abs() < 1e-9);
        assert!((d.vcc(Corner::Max) - 3.6).abs() < 1e-9);
        let r = d.ramp.unwrap();
        assert!((r.rise.1.typ - 0.25e-9).abs() < 1e-18);
        assert!((r.rise.1.max.unwrap() - 0.18e-9).abs() < 1e-18);
        assert_eq!(d.rising.len(), 1);
        assert_eq!(d.rising[0].rows.len(), 3);
        // The pulldown sinks 66 mA at 1.65 V; the pullup sources it.
        assert!((d.current(1.65, 0.0, 1.0, Corner::Typ) - 0.066).abs() < 1e-9);
        assert!((d.current(1.65, 1.0, 0.0, Corner::Typ) + 0.066).abs() < 1e-9);
        // The submodel did not overwrite the receiver.
        let rx = f.model("rcv33").unwrap();
        assert!(rx.pulldown.is_empty());
        assert_eq!(rx.kind, Kind::Input);
        // Clamps conduct below ground.
        assert!(rx.current(-1.0, 0.0, 0.0, Corner::Typ) < -0.01);
        // A selector names its default model.
        assert_eq!(f.model("drv").unwrap().name, "drv33");
    }
}
