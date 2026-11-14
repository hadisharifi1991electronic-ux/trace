//! Electrical rule check of the schematic sheets, with the checks Altium's
//! and Allegro's (OrCAD's) ERC share: parts without a designator or with
//! one used twice, pins left unconnected (an error for inputs), nets with
//! only one pin or none, outputs driving each other or a supply, inputs
//! with nothing driving them, nets with two names, and wire ends left
//! hanging. A No-ERC marker on a pin or wire end leaves it out.

use crate::annotate::unannotated;
use crate::schematic::{Drawing, PinKind, Pt, Sheet, TagKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    Unannotated,
    DuplicateDesignator,
    FloatingInput,
    UnconnectedPin,
    SinglePinNet,
    NoPins,
    OutputConflict,
    NoDriver,
    TwoNames,
    FloatingLabel,
    WireEnd,
}

impl Rule {
    pub const ALL: [Rule; 11] = [
        Rule::Unannotated,
        Rule::DuplicateDesignator,
        Rule::FloatingInput,
        Rule::UnconnectedPin,
        Rule::SinglePinNet,
        Rule::NoPins,
        Rule::OutputConflict,
        Rule::NoDriver,
        Rule::TwoNames,
        Rule::FloatingLabel,
        Rule::WireEnd,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rule::Unannotated => "Part without a designator number",
            Rule::DuplicateDesignator => "Designator used twice",
            Rule::FloatingInput => "Input pin not connected",
            Rule::UnconnectedPin => "Pin not connected",
            Rule::SinglePinNet => "Net with only one pin",
            Rule::NoPins => "Net with no pins",
            Rule::OutputConflict => "Outputs drive each other",
            Rule::NoDriver => "Inputs with nothing driving them",
            Rule::TwoNames => "Net with two names",
            Rule::FloatingLabel => "Net label not on a wire",
            Rule::WireEnd => "Wire end not connected",
        }
    }

    /// How serious it is unless the person says otherwise.
    pub fn level(self) -> Level {
        match self {
            Rule::Unannotated
            | Rule::DuplicateDesignator
            | Rule::FloatingInput
            | Rule::OutputConflict => Level::Error,
            _ => Level::Warning,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub rule: Rule,
    pub level: Level,
    pub sheet: usize,
    pub at: Pt,
    pub text: String,
}

const TOL: f64 = 1.0;

fn same(a: Pt, b: Pt) -> bool {
    (a[0] - b[0]).abs() <= TOL && (a[1] - b[1]).abs() <= TOL
}

fn on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy <= TOL * TOL
}

/// Pins meant to be left open, by name.
fn no_connect_name(name: &str) -> bool {
    let n = name
        .trim()
        .to_ascii_uppercase()
        .replace(['.', '_', '/', ' '], "");
    n == "NC"
        || n == "DNC"
        || n == "NIC"
        || n == "RSVD"
        || n == "RESERVED"
        || (n.starts_with("NC") && n[2..].chars().all(|c| c.is_ascii_digit()))
}

fn marked(d: &Drawing, p: Pt) -> bool {
    d.no_erc.iter().any(|&m| same(m, p))
}

/// A pin of the project: (sheet, pin index).
type PinRef = (usize, usize);

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

/// Checks the sheets of one project.
pub fn check(sheets: &[&Sheet]) -> Vec<Issue> {
    let mut out = Vec::new();
    let pin_name = |s: usize, p: usize| -> String {
        let d = &sheets[s].drawing;
        let pin = &d.pins[p];
        let part = d
            .parts
            .get(pin.part as usize)
            .map_or("?".into(), |q| q.full_designator());
        if pin.name.is_empty() || pin.name == pin.number {
            format!("{part}.{}", pin.number)
        } else {
            format!("{part}.{} ({})", pin.number, pin.name)
        }
    };

    // Designators.
    let mut by_des: BTreeMap<(String, u32), Vec<(usize, usize)>> = BTreeMap::new();
    for (si, s) in sheets.iter().enumerate() {
        for (pi, p) in s.drawing.parts.iter().enumerate() {
            let at = [(p.min[0] + p.max[0]) / 2.0, (p.min[1] + p.max[1]) / 2.0];
            if p.designator.trim().is_empty() {
                continue;
            }
            if unannotated(&p.designator) {
                out.push(Issue {
                    rule: Rule::Unannotated,
                    level: Level::Error,
                    sheet: si,
                    at,
                    text: format!("{} has no number", p.full_designator()),
                });
                continue;
            }
            by_des
                .entry((p.designator.to_ascii_uppercase(), p.gate))
                .or_default()
                .push((si, pi));
        }
    }
    for ((_, _), v) in by_des.iter().filter(|(_, v)| v.len() > 1) {
        for &(si, pi) in v {
            let p = &sheets[si].drawing.parts[pi];
            out.push(Issue {
                rule: Rule::DuplicateDesignator,
                level: Level::Error,
                sheet: si,
                at: [(p.min[0] + p.max[0]) / 2.0, (p.min[1] + p.max[1]) / 2.0],
                text: format!("{} is used {} times", p.full_designator(), v.len()),
            });
        }
    }

    // Project nets: each sheet's nets, joined by name and by pins drawn on
    // two sheets.
    let mut first = Vec::new();
    let mut n = 0;
    for s in sheets {
        first.push(n);
        n += s.drawing.nets.len();
    }
    let mut dsu = Dsu((0..n).collect());
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut by_pin: HashMap<(String, String), usize> = HashMap::new();
    for (si, s) in sheets.iter().enumerate() {
        let d = &s.drawing;
        for (ni, net) in d.nets.iter().enumerate() {
            let g = first[si] + ni;
            for &t in &net.tags {
                let text = d.tags[t as usize].text.trim().to_ascii_uppercase();
                if text.is_empty() {
                    continue;
                }
                match by_name.get(&text) {
                    Some(&o) => dsu.join(g, o),
                    None => {
                        by_name.insert(text, g);
                    }
                }
            }
            for &p in &net.pins {
                let pin = &d.pins[p as usize];
                let Some(part) = d.parts.get(pin.part as usize) else {
                    continue;
                };
                if unannotated(&part.designator) {
                    continue;
                }
                let k = (
                    part.designator.to_ascii_uppercase(),
                    pin.number.to_ascii_uppercase(),
                );
                match by_pin.get(&k) {
                    Some(&o) => dsu.join(g, o),
                    None => {
                        by_pin.insert(k, g);
                    }
                }
            }
        }
    }
    #[derive(Default)]
    struct PNet {
        pins: Vec<PinRef>,
        names: BTreeSet<String>,
        /// Shown names (labels and power ports), for the two-names check.
        labels: BTreeMap<String, (usize, Pt)>,
        power_port: bool,
        wired: bool,
        tags: Vec<(usize, Pt)>,
    }
    let mut nets: BTreeMap<usize, PNet> = BTreeMap::new();
    for (si, s) in sheets.iter().enumerate() {
        let d = &s.drawing;
        for (ni, net) in d.nets.iter().enumerate() {
            let r = dsu.find(first[si] + ni);
            let e = nets.entry(r).or_default();
            e.pins.extend(net.pins.iter().map(|&p| (si, p as usize)));
            e.wired |= !net.wires.is_empty();
            for &t in &net.tags {
                let tag = &d.tags[t as usize];
                let at = tag.at.first().copied().unwrap_or_default();
                e.tags.push((si, at));
                if tag.text.trim().is_empty() {
                    continue;
                }
                e.names.insert(tag.text.trim().to_string());
                if matches!(tag.kind, TagKind::Label | TagKind::Power) {
                    e.labels
                        .entry(tag.text.trim().to_ascii_uppercase())
                        .or_insert((si, at));
                }
                e.power_port |= tag.kind == TagKind::Power;
            }
        }
    }
    for net in nets.values() {
        let name = net
            .names
            .iter()
            .next()
            .cloned()
            .unwrap_or_else(|| "(no name)".into());
        let alone = net.pins.len() == 1 && !net.wired && net.tags.is_empty();
        if alone {
            let (si, p) = net.pins[0];
            let d = &sheets[si].drawing;
            let pin = &d.pins[p];
            if marked(d, pin.at) || no_connect_name(&pin.name) {
                continue;
            }
            let (rule, what) = match pin.kind {
                PinKind::Input => (Rule::FloatingInput, "input"),
                PinKind::Power => (Rule::UnconnectedPin, "power pin"),
                _ => (Rule::UnconnectedPin, "pin"),
            };
            out.push(Issue {
                rule,
                level: rule.level(),
                sheet: si,
                at: pin.at,
                text: format!("{what} {} is not connected", pin_name(si, p)),
            });
            continue;
        }
        if net.pins.is_empty() {
            // Bus names (D[0..7]) stand for many nets.
            if net.names.iter().any(|n| n.contains("..")) {
                continue;
            }
            if let Some(&(si, at)) = net.tags.first() {
                if !marked(&sheets[si].drawing, at) {
                    out.push(Issue {
                        rule: Rule::NoPins,
                        level: Level::Warning,
                        sheet: si,
                        at,
                        text: format!("{name} joins no pins"),
                    });
                }
            }
            continue;
        }
        if net.pins.len() == 1 && !net.power_port {
            let (si, p) = net.pins[0];
            let d = &sheets[si].drawing;
            if !marked(d, d.pins[p].at) && !no_connect_name(&d.pins[p].name) {
                out.push(Issue {
                    rule: Rule::SinglePinNet,
                    level: Level::Warning,
                    sheet: si,
                    at: d.pins[p].at,
                    text: format!("{name} has only one pin, {}", pin_name(si, p)),
                });
            }
        }
        // Drivers.
        let kind = |&(si, p): &PinRef| sheets[si].drawing.pins[p].kind;
        let count = |k: PinKind| net.pins.iter().filter(|r| kind(r) == k).count();
        let outputs = count(PinKind::Output);
        let supplies = count(PinKind::Power);
        let open = count(PinKind::OpenCollector) + count(PinKind::OpenEmitter);
        let at_first = |k: PinKind| {
            net.pins
                .iter()
                .find(|r| kind(r) == k)
                .map(|&(si, p)| (si, sheets[si].drawing.pins[p].at))
        };
        let list = |k: &[PinKind]| {
            let v: Vec<String> = net
                .pins
                .iter()
                .filter(|r| k.contains(&kind(r)))
                .take(4)
                .map(|&(si, p)| pin_name(si, p))
                .collect();
            v.join(", ")
        };
        if outputs >= 2 || (outputs >= 1 && (supplies + open) >= 1) {
            if let Some((si, at)) = at_first(PinKind::Output) {
                let what = if outputs >= 2 {
                    "outputs"
                } else if supplies > 0 {
                    "an output and a supply pin"
                } else {
                    "an output and an open-collector pin"
                };
                out.push(Issue {
                    rule: Rule::OutputConflict,
                    level: Level::Error,
                    sheet: si,
                    at,
                    text: format!(
                        "{name}: {what} drive each other ({})",
                        list(&[
                            PinKind::Output,
                            PinKind::Power,
                            PinKind::OpenCollector,
                            PinKind::OpenEmitter
                        ])
                    ),
                });
            }
        }
        let inputs = count(PinKind::Input);
        if inputs > 0 && inputs == net.pins.len() && !net.power_port {
            if let Some((si, at)) = at_first(PinKind::Input) {
                out.push(Issue {
                    rule: Rule::NoDriver,
                    level: Level::Warning,
                    sheet: si,
                    at,
                    text: format!(
                        "{name}: nothing drives {} ({})",
                        if inputs == 1 {
                            "the input"
                        } else {
                            "the inputs"
                        },
                        list(&[PinKind::Input])
                    ),
                });
            }
        }
        if net.labels.len() > 1 {
            let (&(si, at), names): (&(usize, Pt), Vec<&str>) = (
                net.labels.values().next().unwrap(),
                net.labels.keys().map(String::as_str).collect(),
            );
            let shown: Vec<&str> = net
                .names
                .iter()
                .filter(|n| names.contains(&n.to_ascii_uppercase().as_str()))
                .map(String::as_str)
                .collect();
            out.push(Issue {
                rule: Rule::TwoNames,
                level: Level::Warning,
                sheet: si,
                at,
                text: format!("one net is named {}", shown.join(" and ")),
            });
        }
    }

    // Wire ends touching nothing, on wires no label names (a stub carrying
    // a label is a common way to draw a connection), and labels touching
    // no wire or pin.
    for (si, s) in sheets.iter().enumerate() {
        let d = &s.drawing;
        let mut labelled = vec![false; d.wires.len()];
        for n in d.nets.iter().filter(|n| !n.tags.is_empty()) {
            for &w in &n.wires {
                if let Some(x) = labelled.get_mut(w as usize) {
                    *x = true;
                }
            }
        }
        for n in &d.nets {
            if !n.wires.is_empty() || !n.pins.is_empty() {
                continue;
            }
            for &t in &n.tags {
                let tag = &d.tags[t as usize];
                let at = tag.at.first().copied().unwrap_or_default();
                if tag.kind == TagKind::Label && !marked(d, at) && !tag.text.contains("..") {
                    out.push(Issue {
                        rule: Rule::FloatingLabel,
                        level: Level::Warning,
                        sheet: si,
                        at,
                        text: format!("net label {} touches no wire or pin", tag.text),
                    });
                }
            }
        }
        for (wi, w) in d.wires.iter().enumerate() {
            if labelled[wi] {
                continue;
            }
            for &end in [w.first(), w.last()].into_iter().flatten() {
                let touched = d.pins.iter().any(|p| same(p.at, end))
                    || d.tags.iter().any(|t| t.at.iter().any(|&a| same(a, end)))
                    || d.junctions.iter().any(|&j| same(j, end))
                    || d.wires.iter().enumerate().any(|(oi, o)| {
                        o.windows(2).any(|seg| {
                            on_segment(end, seg[0], seg[1])
                                && (oi != wi || !(same(seg[0], end) || same(seg[1], end)))
                        })
                    })
                    || d.buses
                        .iter()
                        .any(|bus| bus.windows(2).any(|seg| on_segment(end, seg[0], seg[1])))
                    || marked(d, end)
                    || d.links.iter().any(|l| {
                        end[0] >= l.min[0] - TOL
                            && end[0] <= l.max[0] + TOL
                            && end[1] >= l.min[1] - TOL
                            && end[1] <= l.max[1] + TOL
                    });
                if !touched {
                    out.push(Issue {
                        rule: Rule::WireEnd,
                        level: Level::Warning,
                        sheet: si,
                        at: end,
                        text: "a wire ends without touching a pin, wire or label".into(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        (a.level, a.rule, a.sheet)
            .cmp(&(b.level, b.rule, b.sheet))
            .then(a.text.cmp(&b.text))
    });
    out
}

/// A line per rule: how many of each.
pub fn summary(issues: &[Issue]) -> String {
    let e = issues.iter().filter(|i| i.level == Level::Error).count();
    let w = issues.len() - e;
    let mut by: BTreeMap<Rule, usize> = BTreeMap::new();
    for i in issues {
        *by.entry(i.rule).or_default() += 1;
    }
    let parts: Vec<String> = by
        .iter()
        .map(|(r, n)| format!("{n} {}", r.label().to_lowercase()))
        .collect();
    if issues.is_empty() {
        "no errors or warnings".into()
    } else {
        format!("{e} errors, {w} warnings: {}", parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::*;

    fn part(s: &mut Sheet, d: &str, x: f64, pins: &[(&str, &str, PinKind)]) {
        s.symbols.push(Symbol {
            designator: d.into(),
            ..Default::default()
        });
        let i = s.drawing.parts.len() as u32;
        s.drawing.parts.push(SchPart {
            designator: d.into(),
            symbol: s.symbols.len() - 1,
            min: [x - 100.0, -100.0],
            max: [x + 100.0, 100.0],
            gate: 0,
        });
        for (k, (num, name, kind)) in pins.iter().enumerate() {
            let y = 100.0 * k as f64;
            s.drawing.pins.push(SchPin {
                at: [x + 200.0, y],
                base: [x + 100.0, y],
                part: i,
                name: name.to_string(),
                number: num.to_string(),
                kind: *kind,
            });
        }
    }

    fn rules(v: &[Issue]) -> Vec<Rule> {
        v.iter().map(|i| i.rule).collect()
    }

    #[test]
    fn finds_the_usual_mistakes() {
        use PinKind::*;
        let mut s = Sheet::default();
        // U1: an output on net A, an input left open, an NC pin.
        part(
            &mut s,
            "U1",
            0.0,
            &[
                ("1", "OUT", Output),
                ("2", "IN", Input),
                ("3", "NC", Passive),
            ],
        );
        // U2: an output also on A, and an input on B with nothing else.
        part(
            &mut s,
            "U2",
            1000.0,
            &[("1", "Y", Output), ("2", "A", Input)],
        );
        part(&mut s, "R?", 2000.0, &[("1", "", Passive)]);
        part(&mut s, "R1", 3000.0, &[("1", "", Passive)]);
        part(&mut s, "R1", 4000.0, &[("1", "", Passive)]);
        let d = &mut s.drawing;
        d.add_label([200.0, 0.0], "A");
        d.add_label([1200.0, 0.0], "A");
        d.add_label([1200.0, 100.0], "B");
        d.add_wire(vec![[1200.0, 100.0], [1300.0, 100.0], [1300.0, 500.0]]);
        // A loose wire no label names.
        d.add_wire(vec![[5000.0, 500.0], [5100.0, 500.0]]);
        d.add_label([3200.0, 0.0], "C");
        d.add_label([3200.0, 0.0], "D");
        d.no_erc.push([4200.0, 0.0]);
        d.connect();
        let v = check(&[&s]);
        let r = rules(&v);
        assert!(r.contains(&Rule::Unannotated), "{v:#?}");
        assert_eq!(
            r.iter()
                .filter(|&&x| x == Rule::DuplicateDesignator)
                .count(),
            2
        );
        assert!(r.contains(&Rule::FloatingInput));
        assert!(r.contains(&Rule::OutputConflict));
        assert!(r.contains(&Rule::NoDriver));
        assert!(r.contains(&Rule::TwoNames));
        assert!(r.contains(&Rule::WireEnd));
        // The NC pin and the pin under a No-ERC marker are left alone.
        assert!(!v.iter().any(|i| i.text.contains("U1.3")));
        assert!(!v.iter().any(|i| i.at == [4200.0, 0.0]));
        assert!(summary(&v).contains("errors"));
    }

    #[test]
    fn a_clean_sheet_passes() {
        use PinKind::*;
        let mut s = Sheet::default();
        part(&mut s, "U1", 0.0, &[("1", "OUT", Output)]);
        part(&mut s, "U2", 1000.0, &[("1", "IN", Input)]);
        s.drawing.add_wire(vec![
            [200.0, 0.0],
            [600.0, 0.0],
            [600.0, 300.0],
            [1200.0, 300.0],
        ]);
        s.drawing.add_wire(vec![[1200.0, 300.0], [1200.0, 0.0]]);
        s.drawing.connect();
        let v = check(&[&s]);
        assert!(v.is_empty(), "{v:#?}");
    }
}
