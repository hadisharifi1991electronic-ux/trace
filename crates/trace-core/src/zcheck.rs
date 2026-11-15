//! Board-wide impedance check: every controlled trace and via against its
//! target impedance.
//!
//! Targets come from rules (a net or pair name pattern and ohms), and by
//! default every differential pair (100 Ω, 90 Ω for USB, 85 Ω for PCIe
//! and SATA) and every net on the signal pin of a coaxial connector
//! (50 Ω). Traces are solved per net, layer and width with the 2D field
//! solver (coplanar pour beside them counted); vias are grouped by their
//! build (layers, drill, pad, antipad, pitch, return vias) and each kind
//! solved once in 3D, which gives its impedance as a TDR would show it.

use crate::fdtd::{self, ViaSetup};
use crate::impedance::LayerGeometry;
use crate::{Board, LayerRef, Stackup};
use std::collections::{BTreeMap, HashMap};

/// A target: nets (or pairs, by the pair's name or either net's name)
/// matching `pattern` (with * and ?) should be `ohms`.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub pattern: String,
    /// Differential (a pair's Zdiff) or single-ended.
    pub diff: bool,
    pub ohms: f64,
}

#[derive(Clone, Debug)]
pub struct Goal {
    /// Checked first, in order; the first match wins.
    pub rules: Vec<Rule>,
    /// Every differential pair not matched by a rule, at its usual
    /// impedance by name.
    pub pairs: bool,
    /// Nets on a coaxial connector's signal pin, 50 Ω.
    pub rf: bool,
    /// Allowed difference, a fraction of the target.
    pub tolerance: f64,
    /// Trace runs shorter than this on one layer and width are taken as
    /// neck-downs at pads and left out, mil.
    pub min_length: f64,
    /// Solve the vias in 3D.
    pub vias: bool,
    /// The most kinds of via to solve (the most common first).
    pub via_kinds: usize,
    /// Band of the via solves, GHz (their impedance is read from the low
    /// part of it).
    pub via_fmax: f64,
    /// Highest frequency of interest, GHz: runs shorter than a twentieth
    /// of a wavelength there are electrically short (noted, not failed).
    pub ghz: f64,
    /// Nets named like a pair (X_P/X_N, X+/X-, XP/XN) that run side by
    /// side count as pairs where the board defines none for them.
    pub pairs_by_name: bool,
}

impl Default for Goal {
    fn default() -> Goal {
        Goal {
            rules: Vec::new(),
            pairs: true,
            rf: true,
            tolerance: 0.10,
            min_length: 30.0,
            vias: true,
            via_kinds: 6,
            via_fmax: 20.0,
            ghz: 6.0,
            pairs_by_name: true,
        }
    }
}

/// The usual differential impedance of a pair, from its name.
pub fn pair_target(name: &str) -> f64 {
    let n = name.to_ascii_uppercase();
    if n.contains("USB") {
        90.0
    } else if n.contains("PCIE") || n.contains("PCI_E") || n.contains("SATA") {
        85.0
    } else {
        100.0
    }
}

/// A controlled net: its target and, for a pair, the partner.
#[derive(Clone, Debug)]
pub struct Controlled {
    pub net: u32,
    pub partner: Option<u32>,
    /// The pair's name or the net's.
    pub name: String,
    pub diff: bool,
    pub ohms: f64,
    /// Where the target came from.
    pub why: String,
}

fn is_ground(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.contains("GND") || n.contains("VSS") || n == "0V"
}

/// Nets on the signal pin of coaxial connectors.
pub fn rf_nets(board: &Board) -> Vec<(u32, String)> {
    const KEYS: [&str; 9] = [
        "SMA", "U.FL", "UFL", "MMCX", "MCX", "SMP", "BNC", "COAX", "IPEX",
    ];
    let mut out = Vec::new();
    for (ci, c) in board.components.iter().enumerate() {
        let d = c.designator.to_ascii_uppercase();
        // Connectors only: the SMA and SMB diode packages share the names.
        if !(d.starts_with('J') || d.starts_with('P') || d.starts_with('X') || d.starts_with("CN"))
        {
            continue;
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
        let text = text.to_ascii_uppercase();
        if !KEYS.iter().any(|k| text.contains(k)) {
            continue;
        }
        for p in &board.geometry.pads {
            if p.component != Some(ci as u32) {
                continue;
            }
            let Some(n) = p.net else { continue };
            let name = &board.nets[n as usize].name;
            if !is_ground(name) && !out.iter().any(|(m, _)| *m == n) {
                out.push((n, c.designator.clone()));
            }
        }
    }
    out
}

/// Pairs by name: nets X_P/X_N, X+/X-, XP/XN (also _DP/_DN, _T/_C) not in
/// a defined pair, that run side by side somewhere.
pub fn name_pairs(board: &Board) -> Vec<crate::DiffPair> {
    const ENDS: [(&str, &str); 6] = [
        ("_P", "_N"),
        ("+", "-"),
        ("_DP", "_DN"),
        ("_T", "_C"),
        ("P", "N"),
        ("_POS", "_NEG"),
    ];
    let paired: std::collections::HashSet<&str> = board
        .diff_pairs
        .iter()
        .flat_map(|p| [p.positive.as_str(), p.negative.as_str()])
        .collect();
    let idx: HashMap<String, u32> = board
        .nets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.to_ascii_uppercase(), i as u32))
        .collect();
    let mut out = Vec::new();
    let mut taken: std::collections::HashSet<u32> = Default::default();
    for (i, n) in board.nets.iter().enumerate() {
        let i = i as u32;
        if paired.contains(n.name.as_str()) || taken.contains(&i) {
            continue;
        }
        let up = n.name.to_ascii_uppercase();
        for (pe, ne) in ENDS {
            let Some(stem) = up.strip_suffix(pe) else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let Some(&j) = idx.get(&format!("{stem}{ne}")) else {
                continue;
            };
            if taken.contains(&j) || paired.contains(board.nets[j as usize].name.as_str()) {
                continue;
            }
            // Side by side on at least a fifth of the positive's tracks.
            let tracks: Vec<&crate::Track> = board
                .geometry
                .tracks
                .iter()
                .filter(|t| t.net == Some(i) && !t.text)
                .collect();
            if tracks.is_empty() {
                continue;
            }
            let coupled = tracks
                .iter()
                .filter(|t| board.pair_gap(t, j).is_some())
                .count();
            if coupled * 5 < tracks.len() {
                continue;
            }
            let name = stem.trim_end_matches('_').to_string();
            out.push(crate::DiffPair {
                name: board.nets[i as usize]
                    .name
                    .get(..name.len())
                    .unwrap_or(&name)
                    .to_string(),
                positive: n.name.clone(),
                negative: board.nets[j as usize].name.clone(),
            });
            taken.insert(i);
            taken.insert(j);
            break;
        }
    }
    out
}

/// The board with the pairs found by name added (when the goal asks for
/// them), and those pairs.
pub fn with_name_pairs(board: &Board, goal: &Goal) -> (Option<Board>, Vec<crate::DiffPair>) {
    if !(goal.pairs && goal.pairs_by_name) {
        return (None, Vec::new());
    }
    let extra = name_pairs(board);
    if extra.is_empty() {
        return (None, extra);
    }
    let mut b = board.clone();
    b.diff_pairs.extend(extra.iter().cloned());
    (Some(b), extra)
}

/// The nets the check covers, with their targets.
pub fn controlled(board: &Board, goal: &Goal) -> Vec<Controlled> {
    let idx: HashMap<&str, u32> = board
        .nets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.as_str(), i as u32))
        .collect();
    let mut out: Vec<Controlled> = Vec::new();
    let mut taken: std::collections::HashSet<u32> = Default::default();
    // Pairs first (a rule may name the pair or either net).
    for p in &board.diff_pairs {
        let (Some(&a), Some(&b)) = (idx.get(p.positive.as_str()), idx.get(p.negative.as_str()))
        else {
            continue;
        };
        let rule = goal.rules.iter().find(|r| {
            [&p.name, &p.positive, &p.negative]
                .iter()
                .any(|n| crate::constraints::glob(&r.pattern, n))
        });
        // A single-ended rule on a pair's net: each line at that, the pair
        // at twice it (uncoupled).
        let (ohms, why) = match rule {
            Some(r) => (
                if r.diff { r.ohms } else { r.ohms * 2.0 },
                format!("rule {}", r.pattern),
            ),
            None if goal.pairs => (pair_target(&p.name), "pair".to_string()),
            None => continue,
        };
        out.push(Controlled {
            net: a,
            partner: Some(b),
            name: p.name.clone(),
            diff: true,
            ohms,
            why,
        });
        taken.insert(a);
        taken.insert(b);
    }
    for (i, n) in board.nets.iter().enumerate() {
        let i = i as u32;
        if taken.contains(&i) {
            continue;
        }
        if let Some(r) = goal
            .rules
            .iter()
            .find(|r| !r.diff && crate::constraints::glob(&r.pattern, &n.name))
        {
            out.push(Controlled {
                net: i,
                partner: None,
                name: n.name.clone(),
                diff: false,
                ohms: r.ohms,
                why: format!("rule {}", r.pattern),
            });
            taken.insert(i);
        }
    }
    if goal.rf {
        for (n, part) in rf_nets(board) {
            if taken.insert(n) {
                out.push(Controlled {
                    net: n,
                    partner: None,
                    name: board.nets[n as usize].name.clone(),
                    diff: false,
                    ohms: 50.0,
                    why: format!("coax connector {part}"),
                });
            }
        }
    }
    out
}

/// What was found off (or on) target.
#[derive(Clone, Debug)]
pub enum Kind {
    Trace {
        layer: u8,
        width: f64,
        /// Edge gap to the partner, for a coupled pair run.
        gap: Option<f64>,
        length: f64,
    },
    Via {
        setup: ViaSetup,
        /// How many vias are built this way.
        count: usize,
    },
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub net: u32,
    pub name: String,
    pub kind: Kind,
    pub z: f64,
    pub target: f64,
    /// Differential (a pair's Zdiff) or single-ended.
    pub diff: bool,
    /// Where to look, mil.
    pub at: [f64; 2],
    /// What would bring it to the target.
    pub hint: Option<String>,
    /// Shorter than a twentieth of a wavelength at the frequency of
    /// interest: noted, not failed.
    pub short: bool,
}

impl Finding {
    /// Off the target, as a fraction of it (signed).
    pub fn off(&self) -> f64 {
        (self.z - self.target) / self.target
    }

    pub fn describe(&self, geo: &[LayerGeometry]) -> String {
        let lname = |l: u8| {
            geo.get(l as usize)
                .map_or(format!("L{}", l + 1), |g| g.name.clone())
        };
        let what = match &self.kind {
            Kind::Trace {
                layer,
                width,
                gap,
                length,
            } => {
                let g = gap.map_or(String::new(), |g| format!(" gap {g:.2}"));
                format!(
                    "trace on {} width {width:.2}{g} mil, {length:.0} mil long",
                    lname(*layer)
                )
            }
            Kind::Via { setup, count } => format!(
                "via {} to {}, drill {:.1} pad {:.1} antipad {:.1} mil{} ({} like it)",
                lname(setup.from),
                lname(setup.to),
                setup.drill,
                setup.pad,
                setup.antipad,
                setup
                    .pair
                    .map_or(String::new(), |p| format!(", pitch {p:.1}")),
                count
            ),
        };
        format!(
            "{}: {what}: {:.1} Ω against {:.0} Ω ({:+.0} %){}{}",
            self.name,
            self.z,
            self.target,
            self.off() * 100.0,
            if self.short {
                " (electrically short)"
            } else {
                ""
            },
            self.hint
                .as_ref()
                .map_or(String::new(), |h| format!("; {h}"))
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Every checked item, the worst first.
    pub findings: Vec<Finding>,
    pub tolerance: f64,
    pub nets: usize,
    /// Via kinds not solved (over `via_kinds`), and vias they hold.
    pub vias_skipped: (usize, usize),
    pub notes: Vec<String>,
}

impl Report {
    pub fn failing(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| !f.short && f.off().abs() > self.tolerance)
    }

    pub fn summary(&self) -> String {
        let n = self.failing().count();
        let traces = self
            .findings
            .iter()
            .filter(|f| matches!(f.kind, Kind::Trace { .. }))
            .count();
        let vias = self.findings.len() - traces;
        let short = self
            .findings
            .iter()
            .filter(|f| f.short && f.off().abs() > self.tolerance)
            .count();
        format!(
            "{} controlled net(s): {n} of {} checks off target by more than {:.0} % ({traces} trace runs, {vias} via kinds){}",
            self.nets,
            self.findings.len(),
            self.tolerance * 100.0,
            if short > 0 {
                format!("; {short} more off but electrically short")
            } else {
                String::new()
            }
        )
    }

    /// One line per check, as CSV.
    pub fn csv(&self, geo: &[LayerGeometry]) -> String {
        let mut s = String::from("net,kind,layer,width_mil,gap_mil,length_mil,count,z_ohm,target_ohm,off_pct,x_mil,y_mil,hint\n");
        for f in &self.findings {
            let lname = |l: u8| {
                geo.get(l as usize)
                    .map_or(String::new(), |g| g.name.clone())
            };
            let (kind, layer, w, gap, len, count) = match &f.kind {
                Kind::Trace {
                    layer,
                    width,
                    gap,
                    length,
                } => (
                    "trace",
                    lname(*layer),
                    format!("{width:.2}"),
                    gap.map_or(String::new(), |g| format!("{g:.2}")),
                    format!("{length:.0}"),
                    String::new(),
                ),
                Kind::Via { setup, count } => (
                    "via",
                    format!("{}-{}", lname(setup.from), lname(setup.to)),
                    format!("{:.1}", setup.drill),
                    setup.pair.map_or(String::new(), |p| format!("{p:.1}")),
                    String::new(),
                    count.to_string(),
                ),
            };
            s.push_str(&format!(
                "\"{}\",{kind},{layer},{w},{gap},{len},{count},{:.1},{:.0},{:.1},{:.0},{:.0},\"{}\"\n",
                f.name,
                f.z,
                f.target,
                f.off() * 100.0,
                f.at[0],
                f.at[1],
                f.hint.clone().unwrap_or_default()
            ));
        }
        s
    }
}

fn mil(p: &crate::Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

/// Runs the check. `progress` gets the fraction done and what is solving.
pub fn check(
    board: &Board,
    stackup: &Stackup,
    goal: &Goal,
    progress: &dyn Fn(f64, &str),
) -> Report {
    let geo = crate::impedance::layer_geometry(stackup);
    let (own, by_name) = with_name_pairs(board, goal);
    let board = own.as_ref().unwrap_or(board);
    let nets = controlled(board, goal);
    let mut rep = Report {
        tolerance: goal.tolerance,
        nets: nets.len(),
        ..Default::default()
    };
    if !by_name.is_empty() {
        rep.notes.push(format!(
            "{} pair(s) found by name (the board defines none for them): {}",
            by_name.len(),
            by_name
                .iter()
                .take(8)
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if nets.is_empty() {
        rep.notes.push(
            "no controlled nets: the board has no differential pairs or coax connectors; add a rule"
                .into(),
        );
        return rep;
    }
    if geo.iter().all(|g| g.above.is_none() && g.below.is_none()) {
        rep.notes.push(
            "the stack-up has no dielectric thicknesses, so no trace or via can be solved; give one (Impedance window, or --stackup file)"
                .into(),
        );
        return rep;
    }
    progress(0.0, "traces");
    let by_net: HashMap<u32, &Controlled> = nets.iter().map(|c| (c.net, c)).collect();
    let partner_of: HashMap<u32, &Controlled> = nets
        .iter()
        .filter_map(|c| c.partner.map(|p| (p, c)))
        .collect();
    // Trace runs: the field solver per net, layer and width.
    let imp = board.net_impedance_model(&geo, true);
    // The longest segment of each run, to point at.
    let mut longest: HashMap<(u32, u8, i64), ([f64; 2], f64)> = HashMap::new();
    for t in &board.geometry.tracks {
        let (Some(n), LayerRef::Copper(c)) = (t.net, &t.layer) else {
            continue;
        };
        let (a, b) = (mil(&t.start), mil(&t.end));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let e = longest
            .entry((n, *c, t.width.0))
            .or_insert(([0.0; 2], -1.0));
        if len > e.1 {
            *e = ([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], len);
        }
    }
    for r in &imp {
        if r.length < goal.min_length {
            continue;
        }
        let (c, partner_side) = match (by_net.get(&r.net), partner_of.get(&r.net)) {
            (Some(c), _) => (*c, false),
            (None, Some(c)) => (*c, true),
            _ => continue,
        };
        let Some(g) = geo.get(r.layer as usize) else {
            continue;
        };
        let at = longest
            .get(&(r.net, r.layer, crate::Length::from_mil(r.width).0))
            .map_or([0.0; 2], |l| l.0);
        let (z, target, diff, hint) = match (c.partner.is_some(), r.zdiff) {
            // A coupled run of a pair.
            (true, Some(zd)) if zd.is_finite() => {
                if partner_side {
                    continue; // the positive net's run speaks for both
                }
                let hint = g
                    .gap_for_field(r.width, c.ohms)
                    .map(|s| format!("gap {s:.2} mil at this width gives {:.0} Ω", c.ohms));
                (zd, c.ohms, true, hint)
            }
            // An uncoupled stretch of a pair: each line should be half.
            // Short ones are break-outs at the pins.
            (true, _) if r.length < goal.min_length.max(100.0) => continue,
            (true, _) => {
                let hint = g
                    .width_for_field(c.ohms / 2.0)
                    .map(|w| format!("width {w:.2} mil gives {:.0} Ω", c.ohms / 2.0));
                (r.z0, c.ohms / 2.0, false, hint)
            }
            (false, _) => {
                let hint = g.width_for_field(c.ohms).map(|w| {
                    format!(
                        "width {w:.2} mil gives {:.0} Ω (without side copper)",
                        c.ohms
                    )
                });
                (r.z0, c.ohms, false, hint)
            }
        };
        if !z.is_finite() {
            continue;
        }
        // A twentieth of a wavelength at the frequency of interest, mil.
        let eeff = crate::field::single(g, r.width).map_or(g.er, |l| l.eeff);
        let short = goal.ghz > 0.0
            && r.length < 299_792_458.0 / (goal.ghz * 1e9 * eeff.max(1.0).sqrt()) / 20.0 / 25.4e-6;
        rep.findings.push(Finding {
            short,
            net: r.net,
            name: if diff {
                c.name.clone()
            } else {
                board.nets[r.net as usize].name.clone()
            },
            kind: Kind::Trace {
                layer: r.layer,
                width: r.width,
                gap: if diff { r.gap } else { None },
                length: r.length,
            },
            z,
            target,
            diff,
            at,
            hint,
        });
    }
    if goal.vias {
        vias(board, stackup, &geo, &nets, goal, &mut rep, progress);
    }
    rep.findings
        .sort_by(|a, b| b.off().abs().total_cmp(&a.off().abs()));
    progress(1.0, "done");
    rep
}

/// A via as built: its setup and where it is.
struct Built {
    setup: ViaSetup,
    net: u32,
    at: [f64; 2],
}

/// Signal layers a via joins: copper of its own net (tracks, arcs, pads)
/// touching it.
fn joined_layers(board: &Board, v: &crate::Via) -> Vec<(u8, Option<f64>)> {
    let c = mil(&v.at);
    let r = v.diameter.mil() / 2.0 + 0.5;
    let mut out: BTreeMap<u8, Option<f64>> = BTreeMap::new();
    let g = &board.geometry;
    for t in &g.tracks {
        let (Some(n), LayerRef::Copper(l)) = (t.net, &t.layer) else {
            continue;
        };
        if Some(n) != v.net || t.text {
            continue;
        }
        let near = [mil(&t.start), mil(&t.end)]
            .iter()
            .any(|p| (p[0] - c[0]).hypot(p[1] - c[1]) <= r);
        if near {
            let w = t.width.mil();
            let e = out.entry(*l).or_insert(Some(w));
            *e = Some(e.map_or(w, |x| x.max(w)));
        }
    }
    for a in &g.arcs {
        let (Some(n), LayerRef::Copper(l)) = (a.net, &a.layer) else {
            continue;
        };
        if Some(n) != v.net {
            continue;
        }
        let cc = mil(&a.center);
        let rad = a.radius.mil();
        let near = [a.start_deg, a.end_deg].iter().any(|d| {
            let (s, co) = d.to_radians().sin_cos();
            (cc[0] + rad * co - c[0]).hypot(cc[1] + rad * s - c[1]) <= r
        });
        if near {
            out.entry(*l).or_insert(Some(a.width.mil()));
        }
    }
    for p in &g.pads {
        if p.net != v.net || p.hole.0 > 0 {
            continue;
        }
        let LayerRef::Copper(l) = p.layer else {
            continue;
        };
        let pc = mil(&p.at);
        let half = p.size_x.mil().max(p.size_y.mil()) / 2.0;
        if (pc[0] - c[0]).hypot(pc[1] - c[1]) <= half {
            out.entry(l).or_insert(None);
        }
    }
    out.into_iter().collect()
}

/// The clearance hole around a via in other nets' copper: its radius,
/// from the nearest edge of the hole that holds the via's centre, and
/// whether that hole also holds `other`.
fn antipad(board: &Board, v: &crate::Via, other: Option<[f64; 2]>) -> Option<(f64, bool)> {
    let c = mil(&v.at);
    let pad = v.diameter.mil();
    let mut radii: Vec<(f64, bool)> = Vec::new();
    for p in board
        .geometry
        .polygons
        .iter()
        .filter(|p| p.net.is_some() && p.net != v.net)
    {
        for hole in &p.holes {
            let pts: Vec<[f64; 2]> = hole.iter().map(mil).collect();
            if pts.len() < 3 || !fdtd::point_in(&pts, c) {
                continue;
            }
            let r = pts
                .iter()
                .zip(pts.iter().cycle().skip(1))
                .map(|(a, b)| fdtd::seg_dist(c, *a, *b))
                .fold(f64::MAX, f64::min);
            if r > pad / 2.0 && r < pad * 4.0 {
                let both = other.is_some_and(|o| fdtd::point_in(&pts, o));
                radii.push((r, both));
            }
        }
    }
    radii.sort_by(|a, b| a.0.total_cmp(&b.0));
    radii.get(radii.len() / 2).map(|&(r, b)| (2.0 * r, b))
}

fn vias(
    board: &Board,
    stackup: &Stackup,
    geo: &[LayerGeometry],
    nets: &[Controlled],
    goal: &Goal,
    rep: &mut Report,
    progress: &dyn Fn(f64, &str),
) {
    let g = &board.geometry;
    let ground: Vec<bool> = board.nets.iter().map(|n| is_ground(&n.name)).collect();
    let mut built: Vec<(usize, Built)> = Vec::new(); // (index into nets)
    for (ci, c) in nets.iter().enumerate() {
        for v in g.vias.iter().filter(|v| v.net == Some(c.net)) {
            let layers = joined_layers(board, v);
            if layers.len() < 2 {
                continue;
            }
            let (from, wf) = layers[0];
            let (to, wt) = layers[layers.len() - 1];
            let at = mil(&v.at);
            // The partner's via beside it, for a pair.
            let mate = c.partner.and_then(|p| {
                g.vias
                    .iter()
                    .filter(|o| o.net == Some(p))
                    .map(|o| {
                        let q = mil(&o.at);
                        ((q[0] - at[0]).hypot(q[1] - at[1]), q)
                    })
                    .filter(|(d, _)| *d < 6.0 * v.diameter.mil().max(10.0))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            });
            if c.partner.is_some() && mate.is_none() {
                continue; // a pair's net leaving on its own (rare)
            }
            let mut s = ViaSetup::new(from, to);
            s.drill = v.hole.mil();
            s.pad = v.diameter.mil();
            let (ap, oval) = antipad(board, v, mate.map(|m| m.1)).unwrap_or((s.pad + 12.0, false));
            // For an oval round both, the nearest edge gives its width.
            s.antipad = ap;
            s.antipad_oval = oval;
            s.pair = mate.map(|m| m.0);
            s.w_from = wf;
            s.w_to = wt;
            // Return vias: ground vias near the signal via.
            let reach = 4.0 * s.antipad.max(s.pad);
            let mut d: Vec<f64> = g
                .vias
                .iter()
                .filter(|o| o.net.is_some_and(|n| ground.get(n as usize) == Some(&true)))
                .map(|o| {
                    let q = mil(&o.at);
                    (q[0] - at[0]).hypot(q[1] - at[1])
                })
                .filter(|&x| x < reach)
                .collect();
            d.sort_by(f64::total_cmp);
            d.truncate(if s.pair.is_some() { 4 } else { 6 });
            s.returns = d.len();
            s.return_dist = if d.is_empty() {
                s.antipad / 2.0 + 10.0
            } else {
                (d.iter().sum::<f64>() / d.len() as f64).max(s.antipad / 2.0 + s.pad / 2.0)
            };
            s.fmax_ghz = goal.via_fmax;
            // The impedance comes from the low band, so few points; the
            // grid stays the solver's own (a coarser one moved a Clocker
            // pair via from 78 to 89 Ω).
            s.points = 60;
            built.push((
                ci,
                Built {
                    setup: s,
                    net: c.net,
                    at,
                },
            ));
        }
    }
    // Kinds: the same build (rounded) and the same target.
    let key = |ci: usize, s: &ViaSetup| {
        let r = |x: f64| (x * 2.0).round() as i64;
        format!(
            "{} {} {} {} {} {} {} {:?} {} {}",
            nets[ci].ohms as i64,
            s.from,
            s.to,
            r(s.drill),
            r(s.pad),
            r(s.antipad),
            s.antipad_oval,
            s.pair.map(r),
            s.returns,
            (s.return_dist / 5.0).round() as i64,
        )
    };
    let mut kinds: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, (ci, b)) in built.iter().enumerate() {
        kinds.entry(key(*ci, &b.setup)).or_default().push(i);
    }
    let mut kinds: Vec<Vec<usize>> = kinds.into_values().collect();
    kinds.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let solve: Vec<Vec<usize>> = kinds.iter().take(goal.via_kinds).cloned().collect();
    let skipped = &kinds[solve.len()..];
    rep.vias_skipped = (skipped.len(), skipped.iter().map(|k| k.len()).sum());
    let n = solve.len().max(1) as f64;
    for (k, members) in solve.iter().enumerate() {
        let (ci, b) = &built[members[0]];
        let c = &nets[*ci];
        let what = format!("via kind {} of {}", k + 1, solve.len());
        let r = fdtd::solve_with(stackup, &b.setup, &|p| progress((k as f64 + p) / n, &what));
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                rep.notes.push(format!("{}: via not solved: {e}", c.name));
                continue;
            }
        };
        let Some(z) = r.z_via else {
            rep.notes
                .push(format!("{}: via too short to read its impedance", c.name));
            continue;
        };
        let target = c.ohms;
        // Coaxial estimate of the antipad for the target.
        let er = geo
            .get(b.setup.from as usize)
            .map_or(4.0, |g| g.er)
            .max(1.0);
        let slope = 60.0 / er.sqrt() * if b.setup.pair.is_some() { 2.0 } else { 1.0 };
        let want = b.setup.antipad * ((target - z) / slope).exp();
        let hint = Some(format!(
            "antipad about {want:.0} mil (Via window: Solve sizes it exactly)"
        ));
        // Only the positive net's vias are listed, so a pair counts once.
        let count = members.len();
        rep.findings.push(Finding {
            short: false,
            net: b.net,
            name: c.name.clone(),
            kind: Kind::Via {
                setup: b.setup.clone(),
                count,
            },
            z,
            target,
            diff: b.setup.pair.is_some(),
            at: b.at,
            hint,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_targets_by_name() {
        assert_eq!(pair_target("USB_DP"), 90.0);
        assert_eq!(pair_target("PCIE_TX0"), 85.0);
        assert_eq!(pair_target("SYSREF"), 100.0);
    }
}
