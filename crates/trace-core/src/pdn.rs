//! Power delivery network (PDN) impedance: how the impedance a supply's
//! loads see changes with frequency, from the plane pair, the decoupling
//! capacitors and the regulator, as Sigrity OptimizePI and Altium's PDN
//! Analyzer show it.
//!
//! The supply's plane and the nearest ground plane under it form a cavity,
//! laid on a square grid: each cell has the pair's capacitance (and its
//! dielectric loss) to ground, neighbouring cells are joined by the pair's
//! inductance and both planes' sheet resistance (with skin effect). At
//! each frequency the grid is solved once (a banded LDLᵀ factorisation)
//! for the impedance between every port: the loads (the parts drawing
//! from the supply), the regulator and each capacitor, each port being the
//! cells under its pads on the supply. Capacitors (C, ESR, ESL and the
//! inductance of their mounting vias down to the cavity) and the regulator
//! (R + L) then end their ports, so trying another capacitor value, or
//! leaving one out, is a small matrix update: the optimiser tries many.

#![allow(clippy::needless_range_loop)]

use crate::fdtd::Cx;
use crate::{Board, LayerKind, LayerRef, Side};
use std::collections::HashMap;

const EPS0: f64 = 8.854_187_812_8e-12;
const MU0: f64 = 1.256_637_062_12e-6;
const MIL: f64 = 25.4e-6;
const RHO: f64 = crate::pi::RHO;
/// A capacitor left out: an open port.
const OPEN: f64 = 1e9;

/// Case size of a chip capacitor, from the distance between its pads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Package {
    P0201,
    P0402,
    P0603,
    P0805,
    P1206,
    Large,
}

impl Package {
    /// From the distance between the two pads' centres, mil.
    pub fn from_pitch(mil: f64) -> Package {
        match mil {
            m if m < 28.0 => Package::P0201,
            m if m < 50.0 => Package::P0402,
            m if m < 72.0 => Package::P0603,
            m if m < 100.0 => Package::P0805,
            m if m < 150.0 => Package::P1206,
            _ => Package::Large,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Package::P0201 => "0201",
            Package::P0402 => "0402",
            Package::P0603 => "0603",
            Package::P0805 => "0805",
            Package::P1206 => "1206",
            Package::Large => "large",
        }
    }

    /// The body's own inductance, H (typical MLCC).
    pub fn esl(self) -> f64 {
        match self {
            Package::P0201 => 0.25e-9,
            Package::P0402 => 0.35e-9,
            Package::P0603 => 0.5e-9,
            Package::P0805 => 0.65e-9,
            Package::P1206 => 0.9e-9,
            Package::Large => 1.5e-9,
        }
    }

    /// A usual decoupling value in this case, F, for capacitors whose
    /// value the board does not give.
    pub fn typical_farads(self) -> f64 {
        match self {
            Package::P0201 => 100e-9,
            Package::P0402 => 1e-6,
            Package::P0603 => 4.7e-6,
            Package::P0805 => 10e-6,
            Package::P1206 => 22e-6,
            Package::Large => 47e-6,
        }
    }

    /// The largest ceramic value commonly made in this case, F.
    pub fn max_farads(self) -> f64 {
        match self {
            Package::P0201 => 1e-6,
            Package::P0402 => 10e-6,
            Package::P0603 => 22e-6,
            Package::P0805 => 47e-6,
            Package::P1206 => 100e-6,
            Package::Large => 1e-3,
        }
    }
}

/// Values the optimiser may choose from, F.
pub const VALUES: [f64; 10] = [
    10e-9, 100e-9, 220e-9, 1e-6, 2.2e-6, 4.7e-6, 10e-6, 22e-6, 47e-6, 100e-6,
];

/// A capacitor's own model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapModel {
    pub farads: f64,
    /// Ω.
    pub esr: f64,
    /// H.
    pub esl: f64,
}

impl CapModel {
    /// A ceramic capacitor of this value and case: ESR falling with the
    /// value (about 40 mΩ at 100 nF, 10 mΩ at 10 µF), ESL from the case.
    pub fn mlcc(farads: f64, package: Package) -> CapModel {
        CapModel {
            farads,
            esr: (0.02 * (farads / 1e-6).powf(-0.3)).clamp(0.002, 0.3),
            esl: package.esl(),
        }
    }

    /// Its impedance at ω with `mount` more inductance in series.
    fn z(&self, w: f64, mount: f64) -> Cx {
        Cx::new(self.esr, w * (self.esl + mount) - 1.0 / (w * self.farads))
    }
}

/// A value such as 100nF, 0.1uF, 4u7, 1000pF or 10µF/16V, in farads.
pub fn parse_farads(s: &str) -> Option<f64> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let starts = c[i].is_ascii_digit() && (i == 0 || !c[i - 1].is_alphanumeric());
        if !starts {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut num = String::new();
        while j < c.len() && (c[j].is_ascii_digit() || c[j] == '.') {
            num.push(c[j]);
            j += 1;
        }
        while j < c.len() && c[j] == ' ' {
            j += 1;
        }
        let prefix = match c.get(j) {
            Some('p') | Some('P') => Some(1e-12),
            Some('n') | Some('N') => Some(1e-9),
            Some('u') | Some('U') | Some('µ') | Some('μ') => Some(1e-6),
            Some('m') => Some(1e-3),
            _ => None,
        };
        let mut k = j + usize::from(prefix.is_some());
        // 4u7: the prefix stands for the decimal point.
        let mut frac = String::new();
        if prefix.is_some() {
            while k < c.len() && c[k].is_ascii_digit() {
                frac.push(c[k]);
                k += 1;
            }
        }
        let farad = matches!(c.get(k), Some('F') | Some('f'));
        if prefix.is_some() || farad {
            let text = if frac.is_empty() {
                num.clone()
            } else {
                format!("{num}.{frac}")
            };
            if let Ok(v) = text.parse::<f64>() {
                let v = v * prefix.unwrap_or(1.0);
                if v > 0.0 && v < 1.0 {
                    return Some(v);
                }
            }
        }
        i = j.max(i + 1);
    }
    None
}

/// A supply voltage named in a net's name: 1V8, 3P3V, VDD_0V85, +5V.
pub fn volts_in_name(name: &str) -> Option<f64> {
    let u = name.to_ascii_uppercase();
    let c: Vec<char> = u.chars().collect();
    for i in 0..c.len() {
        if !c[i].is_ascii_digit() || (i > 0 && c[i - 1].is_ascii_digit()) {
            continue;
        }
        let mut j = i;
        while j < c.len() && c[j].is_ascii_digit() {
            j += 1;
        }
        if j < c.len() && (c[j] == 'V' || c[j] == 'P') {
            let mut k = j + 1;
            while k < c.len() && c[k].is_ascii_digit() {
                k += 1;
            }
            if c[j] == 'P' && (k == j + 1) {
                continue;
            }
            let whole: String = c[i..j].iter().collect();
            let frac: String = c[j + 1..k].iter().collect();
            let v: f64 = format!("{whole}.{}", if frac.is_empty() { "0" } else { &frac })
                .parse()
                .ok()?;
            if v > 0.0 && v <= 60.0 {
                return Some(v);
            }
        }
    }
    None
}

/// What to analyse.
#[derive(Clone, Debug)]
pub struct Setup {
    pub net: u32,
    /// The return net (None: the net most of the supply's capacitors
    /// return to, else one named like GND).
    pub ground: Option<u32>,
    /// Sweep, Hz, and points (log spaced).
    pub fmin: f64,
    pub fmax: f64,
    pub points: usize,
    /// Target impedance, Ω; None works it out from volts × ripple ÷ amps.
    pub target: Option<f64>,
    pub volts: Option<f64>,
    /// Allowed ripple, % of the supply.
    pub ripple_pct: f64,
    /// Highest frequency the target holds to, Hz.
    pub target_fmax: f64,
    /// Transient current, A.
    pub amps: f64,
    /// The regulator (None: guessed), its output resistance and inductance.
    pub vrm: Option<usize>,
    pub vrm_r: f64,
    pub vrm_l: f64,
    /// Loads (None: every active part on the net).
    pub loads: Vec<usize>,
    /// Grid cell, mil (0: chosen from the plane's size).
    pub cell_mil: f64,
    /// Drill of the capacitors' vias, mil, for their mounting inductance.
    pub via_drill_mil: f64,
    /// Capacitors without a value get a typical one for their case
    /// (else they are left out).
    pub assume: bool,
}

impl Setup {
    pub fn new(net: u32) -> Setup {
        Setup {
            net,
            ground: None,
            fmin: 100e3,
            fmax: 1e9,
            points: 60,
            target: None,
            volts: None,
            ripple_pct: 3.0,
            target_fmax: 100e6,
            amps: 1.0,
            vrm: None,
            vrm_r: 1e-3,
            vrm_l: 20e-9,
            loads: Vec::new(),
            cell_mil: 0.0,
            via_drill_mil: 10.0,
            assume: true,
        }
    }
}

/// A decoupling capacitor on the supply.
#[derive(Clone, Debug)]
pub struct Decap {
    pub part: usize,
    pub designator: String,
    /// Its value as the board gives it.
    pub value: String,
    pub model: CapModel,
    pub package: Package,
    /// Mounting inductance: its pads and vias down to the cavity, H.
    pub mount: f64,
    /// The board gives no value: a typical one for its case is taken.
    pub assumed: bool,
}

/// A two-pad part joining the supply to another (non-ground) net: a
/// ferrite bead, inductor or series resistor feeding a sub-rail, or the
/// one feeding this rail.
#[derive(Clone, Debug)]
pub struct Feed {
    pub part: usize,
    pub designator: String,
    /// The net on its other pad.
    pub net: u32,
    pub net_name: String,
    /// A ferrite bead or inductor (else a resistor or other part).
    pub bead: bool,
    /// Its value as the board gives it.
    pub value: String,
}

impl Feed {
    /// A sub-rail worth looking at on its own: through a bead, an
    /// inductor or a resistor of at most 1 Ω (a 0 Ω link).
    pub fn is_rail(&self) -> bool {
        self.bead || parse_ohms(&self.value).is_some_and(|r| r <= 1.0)
    }
}

/// Ohms from a value such as "0", "0R", "4R7", "10 ohm", "1k", "22Ω".
pub fn parse_ohms(s: &str) -> Option<f64> {
    let t = s
        .trim()
        .to_ascii_lowercase()
        .replace("ohms", "")
        .replace("ohm", "")
        .replace('Ω', "");
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    // "4R7", "0R", "1K5": the letter is the decimal point.
    for (ch, mul) in [('r', 1.0), ('k', 1e3), ('m', 1e6)] {
        if let Some(i) = t.find(ch) {
            let (a, b) = (&t[..i], &t[i + 1..]);
            let num = |x: &str| x.chars().all(|c| c.is_ascii_digit() || c == '.');
            if num(a) && num(b) && !(a.is_empty() && b.is_empty()) {
                let a: f64 = if a.is_empty() { 0.0 } else { a.parse().ok()? };
                let frac: f64 = if b.is_empty() {
                    0.0
                } else {
                    format!("0.{b}").parse().ok()?
                };
                return Some((a + frac) * mul);
            }
        }
    }
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let v: f64 = t[..end].parse().ok()?;
    let rest = t[end..].trim();
    let mul = match rest.chars().next() {
        None => 1.0,
        Some('k') => 1e3,
        Some('m') if rest.starts_with("meg") => 1e6,
        Some('m') => 1e-3,
        _ => return None,
    };
    Some(v * mul)
}

/// The two-pad parts joining `net` to another net that is not `ground`,
/// by designator.
pub fn feeds(board: &Board, net: u32, ground: u32) -> Vec<Feed> {
    let mut pads_of: std::collections::BTreeMap<usize, Vec<Option<u32>>> = Default::default();
    for p in &board.geometry.pads {
        if let Some(c) = p.component {
            pads_of.entry(c as usize).or_default().push(p.net);
        }
    }
    let mut out = Vec::new();
    for (part, nets) in pads_of {
        if nets.len() != 2 {
            continue;
        }
        let other = match (nets[0], nets[1]) {
            (Some(a), Some(b)) if a == net && b != net => b,
            (Some(a), Some(b)) if b == net && a != net => a,
            _ => continue,
        };
        if other == ground {
            continue;
        }
        let comp = &board.components[part];
        let des = comp.designator.to_ascii_uppercase();
        let prefix: String = des
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let value = comp
            .param(&["Value", "Comment", "Device Type"])
            .unwrap_or("")
            .to_string();
        let v = format!("{} {}", value, comp.footprint).to_ascii_uppercase();
        let net_name = board
            .nets
            .get(other as usize)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        // An inductor between two rails is a filter; one to a switch node
        // is a regulator's own output inductor (not a bead).
        let up = net_name.to_ascii_uppercase();
        let rail = volts_in_name(&net_name).is_some()
            || [
                "VDD", "VCC", "VIN", "VBAT", "VSYS", "PWR", "VREG", "AVDD", "DVDD",
            ]
            .iter()
            .any(|k| up.contains(k));
        let bead = matches!(prefix.as_str(), "FB" | "E" | "B" | "BD" | "FER")
            || matches!(prefix.as_str(), "L" | "FL") && rail
            || v.contains("BEAD")
            || v.contains("BLM")
            || v.contains("FERRITE");
        if prefix == "C" || parse_farads(&value).is_some() && !bead {
            continue;
        }
        out.push(Feed {
            part,
            designator: comp.designator.clone(),
            net: other,
            net_name,
            bead,
            value,
        });
    }
    out
}

/// A ferrite bead or filter inductor feeding a rail, seen from that rail:
/// its DC resistance and low-frequency inductance (above a few MHz a bead
/// is mostly loss, so the rail before it hardly shows).
pub const BEAD_R: f64 = 0.05;
pub const BEAD_L: f64 = 1e-6;

/// The plane pair analysed.
#[derive(Clone, Debug, Default)]
pub struct Cavity {
    pub power_layer: String,
    pub ground_layer: String,
    pub ground_net: String,
    pub d_mil: f64,
    pub er: f64,
    pub tan_d: f64,
    /// Area both planes share, square inches.
    pub area_in2: f64,
    /// The pair's own capacitance, F.
    pub farads: f64,
    pub cell_mil: f64,
    pub nodes: usize,
}

/// The cavity solved at every frequency, with its ports.
#[derive(Clone, Debug)]
pub struct Model {
    pub freqs: Vec<f64>,
    /// Load parts, in port order (ports 0..loads.len()).
    pub loads: Vec<usize>,
    pub vrm: Option<usize>,
    pub vrm_r: f64,
    pub vrm_l: f64,
    /// The regulator is a ferrite bead or inductor from another rail.
    pub vrm_bead: bool,
    /// Sub-rails this supply feeds through beads, inductors or 0 Ω links.
    pub sub_rails: Vec<Feed>,
    pub decaps: Vec<Decap>,
    pub cavity: Cavity,
    /// Target impedance, Ω, if known.
    pub target: Option<f64>,
    /// Highest frequency the target holds to, Hz (above it the package's
    /// and the die's own capacitance take over).
    pub band: f64,
    pub notes: Vec<String>,
    /// Port impedance matrix at each frequency: loads, then the regulator
    /// (when there is one), then the capacitors; row major.
    zp: Vec<Vec<Cx>>,
}

/// Impedance curves: |Z| at each load over the sweep, Ω.
pub type Curves = Vec<Vec<f64>>;

fn shoelace(r: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

/// Copper area of `net` on each copper layer, square mil.
fn layer_areas(board: &Board, net: u32, layers: usize) -> Vec<f64> {
    let mut a = vec![0.0; layers];
    for p in board
        .geometry
        .polygons
        .iter()
        .filter(|p| p.net == Some(net))
    {
        let LayerRef::Copper(l) = p.layer else {
            continue;
        };
        let ring = |r: &[crate::Point]| {
            shoelace(&r.iter().map(|q| [q.x.mil(), q.y.mil()]).collect::<Vec<_>>())
        };
        if let Some(x) = a.get_mut(l as usize) {
            *x += ring(&p.outline) - p.holes.iter().map(|h| ring(h)).sum::<f64>();
        }
    }
    a
}

/// Copper depths, layer names and dielectrics (thickness m, εr, tanδ).
type Stack = (Vec<f64>, Vec<String>, Vec<(f64, f64, f64)>);

/// Each copper layer's depth (its middle) from the top, m, and the
/// dielectrics between copper layers: (thickness m, εr, tanδ).
fn stack(board: &Board) -> Stack {
    let mut z = Vec::new();
    let mut names = Vec::new();
    let mut gaps = Vec::new();
    let mut depth = 0.0;
    let (mut t, mut t_over_er, mut loss) = (0.0, 0.0, 0.0);
    for l in &board.stackup.layers {
        let th = (l.thickness.mil() * MIL).max(0.0);
        match l.kind {
            LayerKind::Copper => {
                if !z.is_empty() {
                    let er = if t_over_er > 0.0 { t / t_over_er } else { 4.2 };
                    let td = if t > 0.0 { loss / t } else { 0.02 };
                    gaps.push((t, er, td));
                }
                (t, t_over_er, loss) = (0.0, 0.0, 0.0);
                z.push(depth + th / 2.0);
                names.push(l.name.clone());
            }
            LayerKind::Dielectric if !z.is_empty() => {
                let er = l.dielectric_constant.filter(|e| *e >= 1.0).unwrap_or(4.2);
                t += th;
                t_over_er += th / er;
                loss += th * l.loss_tangent.unwrap_or(0.02);
            }
            _ => {}
        }
        depth += th;
    }
    if z.len() < 2 {
        z = vec![0.0, 62.0 * MIL];
        names = vec!["Top".into(), "Bottom".into()];
        gaps = vec![(62.0 * MIL, 4.2, 0.02)];
    }
    (z, names, gaps)
}

/// Fills the grid cells whose centres lie in the net's copper on `layer`.
fn raster(
    board: &Board,
    net: u32,
    layer: u8,
    origin: [f64; 2],
    cell: f64,
    cols: usize,
    rows: usize,
) -> Vec<bool> {
    let mut fill = vec![false; cols * rows];
    for p in board
        .geometry
        .polygons
        .iter()
        .filter(|p| p.net == Some(net) && p.layer == LayerRef::Copper(layer))
    {
        // Clearance holes smaller than a few cells (a BGA's antipads) are
        // left in: the copper between them still carries the plane, and a
        // grid that coarse would otherwise break it into specks.
        let small = |r: &Vec<crate::Point>| {
            let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
            for q in r {
                lo = [lo[0].min(q.x.mil()), lo[1].min(q.y.mil())];
                hi = [hi[0].max(q.x.mil()), hi[1].max(q.y.mil())];
            }
            (hi[0] - lo[0]).max(hi[1] - lo[1]) < 4.0 * cell
        };
        let rings: Vec<Vec<[f64; 2]>> = std::iter::once(&p.outline)
            .chain(p.holes.iter().filter(|h| !small(h)))
            .map(|r| r.iter().map(|q| [q.x.mil(), q.y.mil()]).collect())
            .collect();
        for j in 0..rows {
            let y = origin[1] + (j as f64 + 0.5) * cell;
            let mut xs: Vec<f64> = Vec::new();
            for r in &rings {
                for k in 0..r.len() {
                    let (a, b) = (r[k], r[(k + 1) % r.len()]);
                    if (a[1] > y) != (b[1] > y) {
                        xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for span in xs.chunks(2) {
                if let [x0, x1] = *span {
                    let i0 = ((x0 - origin[0]) / cell - 0.5).ceil().max(0.0) as usize;
                    let i1 = ((x1 - origin[0]) / cell - 0.5).floor();
                    if i1 < 0.0 {
                        continue;
                    }
                    for i in i0..=(i1 as usize).min(cols - 1) {
                        fill[j * cols + i] = true;
                    }
                }
            }
        }
    }
    fill
}

/// The return net: where most of the supply's two-pad capacitors go,
/// else the net named most like ground with the most copper.
pub fn guess_ground(board: &Board, net: u32) -> Option<u32> {
    let g = &board.geometry;
    let mut by_part: HashMap<u32, Vec<Option<u32>>> = HashMap::new();
    for p in &g.pads {
        if let Some(c) = p.component {
            by_part.entry(c).or_default().push(p.net);
        }
    }
    let mut votes: HashMap<u32, usize> = HashMap::new();
    for (&part, nets) in &by_part {
        // Only capacitors vote: beads and resistors go to other rails.
        let is_cap = board.components.get(part as usize).is_some_and(|c| {
            let d = c.designator.to_ascii_uppercase();
            d.starts_with('C') && !d.starts_with("CON")
                || c.param(&["Value", "Comment", "Capacitance"])
                    .and_then(parse_farads)
                    .is_some()
        });
        if is_cap && nets.len() == 2 && nets.contains(&Some(net)) {
            if let Some(o) = nets.iter().flatten().find(|&&n| n != net) {
                let name = board
                    .nets
                    .get(*o as usize)
                    .map(|n| n.name.to_ascii_uppercase())
                    .unwrap_or_default();
                let named = name.contains("GND") || name.contains("VSS");
                *votes.entry(*o).or_default() += if named { 3 } else { 1 };
            }
        }
    }
    if let Some((&n, _)) = votes
        .iter()
        .max_by_key(|(n, v)| (**v, std::cmp::Reverse(**n)))
    {
        return Some(n);
    }
    let layers = stack(board).0.len();
    board
        .nets
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            let u = n.name.to_ascii_uppercase();
            u.contains("GND") || u.contains("VSS") || u == "0V"
        })
        .map(|(i, _)| {
            (
                i as u32,
                layer_areas(board, i as u32, layers).iter().sum::<f64>(),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A complex symmetric band matrix (lower band kept), factored as LDLᵀ.
struct Band {
    n: usize,
    b: usize,
    a: Vec<Cx>,
}

impl Band {
    fn new(n: usize, b: usize) -> Band {
        Band {
            n,
            b,
            a: vec![Cx::default(); n * (b + 1)],
        }
    }
    fn at(&mut self, i: usize, j: usize) -> &mut Cx {
        let (i, j) = if i >= j { (i, j) } else { (j, i) };
        &mut self.a[i * (self.b + 1) + (i - j)]
    }
    fn get(&self, i: usize, j: usize) -> Cx {
        self.a[i * (self.b + 1) + (i - j)]
    }
    /// LDLᵀ in place: L below the diagonal, D on it.
    fn factor(&mut self) {
        let (n, b, w) = (self.n, self.b, self.b + 1);
        let mut v = vec![Cx::default(); b + 1];
        for j in 0..n {
            let lo = j.saturating_sub(b);
            let mut d = self.get(j, j);
            for k in lo..j {
                let l = self.get(j, k);
                let dk = self.get(k, k);
                v[k - lo] = l.mul(dk);
                d = d.add(l.mul(v[k - lo]).scale(-1.0));
            }
            if d.abs() < 1e-300 {
                d = Cx::new(1e-300, 0.0);
            }
            self.a[j * w] = d;
            for i in j + 1..(j + b + 1).min(n) {
                let lo_i = i.saturating_sub(b);
                let mut s = self.get(i, j);
                for k in lo.max(lo_i)..j {
                    s = s.add(self.get(i, k).mul(v[k - lo]).scale(-1.0));
                }
                self.a[i * w + (i - j)] = s.div(d);
            }
        }
    }
    /// Solves in place after `factor`.
    fn solve(&self, x: &mut [Cx]) {
        let (n, b) = (self.n, self.b);
        for i in 0..n {
            let lo = i.saturating_sub(b);
            let mut s = x[i];
            for k in lo..i {
                s = s.add(self.get(i, k).mul(x[k]).scale(-1.0));
            }
            x[i] = s;
        }
        for i in 0..n {
            x[i] = x[i].div(self.get(i, i));
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for k in i + 1..(i + b + 1).min(n) {
                s = s.add(self.get(k, i).mul(x[k]).scale(-1.0));
            }
            x[i] = s;
        }
    }
}

/// Inverts a dense complex matrix (k × k, row major), partial pivoting.
fn invert(m: &[Cx], k: usize) -> Vec<Cx> {
    let mut a = m.to_vec();
    let mut inv = vec![Cx::default(); k * k];
    for i in 0..k {
        inv[i * k + i] = Cx::new(1.0, 0.0);
    }
    for c in 0..k {
        let p = (c..k)
            .max_by(|&x, &y| a[x * k + c].abs().total_cmp(&a[y * k + c].abs()))
            .unwrap_or(c);
        if p != c {
            for j in 0..k {
                a.swap(c * k + j, p * k + j);
                inv.swap(c * k + j, p * k + j);
            }
        }
        let d = a[c * k + c];
        let d = if d.abs() < 1e-300 {
            Cx::new(1e-300, 0.0)
        } else {
            d
        };
        let r = Cx::new(1.0, 0.0).div(d);
        for j in 0..k {
            a[c * k + j] = a[c * k + j].mul(r);
            inv[c * k + j] = inv[c * k + j].mul(r);
        }
        for i in 0..k {
            if i == c {
                continue;
            }
            let f = a[i * k + c];
            if f.abs() == 0.0 {
                continue;
            }
            for j in 0..k {
                a[i * k + j] = a[i * k + j].add(f.mul(a[c * k + j]).scale(-1.0));
                inv[i * k + j] = inv[i * k + j].add(f.mul(inv[c * k + j]).scale(-1.0));
            }
        }
    }
    inv
}

/// Builds the cavity and its port matrix over the sweep.
pub fn model(board: &Board, setup: &Setup) -> Result<Model, String> {
    let g = &board.geometry;
    let net = setup.net;
    let net_name = board
        .nets
        .get(net as usize)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let mut notes = Vec::new();
    let (zs, names, gaps) = stack(board);
    let nl = zs.len();
    let ground = setup
        .ground
        .or_else(|| guess_ground(board, net))
        .ok_or("no ground net found; name one")?;
    if ground == net {
        return Err("the supply and ground nets are the same".into());
    }
    let pa = layer_areas(board, net, nl);
    let ga = layer_areas(board, ground, nl);
    let (pl, parea) = pa
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, a)| (i, *a))
        .unwrap_or((0, 0.0));
    if parea <= 0.0 {
        return Err(format!(
            "{net_name} has no plane or pour: the PDN analysis needs the supply on a plane over a ground plane"
        ));
    }
    let gl = (0..nl)
        .filter(|&l| l != pl && ga[l] > 0.0)
        .min_by(|&a, &b| {
            (zs[a] - zs[pl])
                .abs()
                .total_cmp(&(zs[b] - zs[pl]).abs())
                .then(ga[b].total_cmp(&ga[a]))
        })
        .ok_or("the ground net has no plane or pour on another layer")?;
    let (l0, l1) = (pl.min(gl), pl.max(gl));
    let (mut d, mut t_over_er, mut loss) = (0.0, 0.0, 0.0);
    for gap in &gaps[l0..l1] {
        d += gap.0;
        t_over_er += gap.0 / gap.1;
        loss += gap.0 * gap.2;
    }
    let (er, tan_d) = if d > 0.0 && t_over_er > 0.0 {
        (d / t_over_er, loss / d)
    } else {
        d = 4.0 * MIL * (l1 - l0) as f64;
        notes.push(format!(
            "the stack-up gives no dielectric thickness between {} and {}: taken as {:.0} mil of εr 4.2 (give a stack-up file with --stackup)",
            names[l0],
            names[l1],
            d / MIL
        ));
        (4.2, 0.02)
    };
    if l1 - l0 > 1 {
        notes.push(format!(
            "{} and {} are {} layers apart; the copper between is not part of the cavity",
            names[pl],
            names[gl],
            l1 - l0
        ));
    }
    // Grid over the supply plane.
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in g
        .polygons
        .iter()
        .filter(|p| p.net == Some(net) && p.layer == LayerRef::Copper(pl as u8))
    {
        for q in &p.outline {
            lo = [lo[0].min(q.x.mil()), lo[1].min(q.y.mil())];
            hi = [hi[0].max(q.x.mil()), hi[1].max(q.y.mil())];
        }
    }
    let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
    let cell = if setup.cell_mil > 0.0 {
        setup.cell_mil
    } else {
        (area / 5000.0).sqrt().clamp(10.0, 250.0)
    };
    let cols = ((hi[0] - lo[0]) / cell).ceil().max(1.0) as usize;
    let rows = ((hi[1] - lo[1]) / cell).ceil().max(1.0) as usize;
    let origin = lo;
    let pw = raster(board, net, pl as u8, origin, cell, cols, rows);
    let gw = raster(board, ground, gl as u8, origin, cell, cols, rows);
    // Nodes numbered along the shorter side, for a narrow band.
    let by_cols = cols > rows;
    let mut id = vec![usize::MAX; cols * rows];
    let mut n = 0;
    let (outer, inner) = if by_cols { (cols, rows) } else { (rows, cols) };
    for o in 0..outer {
        for k in 0..inner {
            let (i, j) = if by_cols { (o, k) } else { (k, o) };
            let c = j * cols + i;
            if pw[c] && gw[c] {
                id[c] = n;
                n += 1;
            }
        }
    }
    if n == 0 {
        return Err(format!(
            "{} and {} do not overlap: no plane pair",
            names[pl], names[gl]
        ));
    }
    let mut links: Vec<(usize, usize)> = Vec::new();
    let mut band = 0;
    for j in 0..rows {
        for i in 0..cols {
            let a = id[j * cols + i];
            if a == usize::MAX {
                continue;
            }
            for (di, dj) in [(1, 0), (0, 1)] {
                if i + di < cols && j + dj < rows {
                    let b = id[(j + dj) * cols + i + di];
                    if b != usize::MAX {
                        links.push((a, b));
                        band = band.max(a.abs_diff(b));
                    }
                }
            }
        }
    }
    let wavelength_mil = 3e8 / (setup.fmax * er.sqrt()) / MIL;
    if cell > wavelength_mil / 15.0 {
        notes.push(format!(
            "cells of {cell:.0} mil are coarse for {:.0} MHz; set a smaller cell for accuracy up there",
            setup.fmax / 1e6
        ));
    }
    let cell_m = cell * MIL;
    let c_cell = EPS0 * er * cell_m * cell_m / d;
    let t_p = board
        .stackup
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Copper)
        .map(|l| l.thickness.mil() * MIL)
        .map(|t| if t > 0.0 { t } else { 1.4 * MIL })
        .collect::<Vec<_>>();
    let t_of = |l: usize| t_p.get(l).copied().unwrap_or(1.4 * MIL);
    // The node under a point, or the nearest within a few cells.
    let node_at = |x: f64, y: f64| -> Option<usize> {
        let ci = ((x - origin[0]) / cell).floor() as i64;
        let cj = ((y - origin[1]) / cell).floor() as i64;
        for r in 0..=4i64 {
            let mut best: Option<(f64, usize)> = None;
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs().max(dj.abs()) != r {
                        continue;
                    }
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i as usize >= cols || j as usize >= rows {
                        continue;
                    }
                    let k = id[j as usize * cols + i as usize];
                    if k != usize::MAX {
                        let d2 = (di * di + dj * dj) as f64;
                        if best.is_none_or(|b| d2 < b.0) {
                            best = Some((d2, k));
                        }
                    }
                }
            }
            if let Some((_, k)) = best {
                return Some(k);
            }
        }
        None
    };
    // Ports: the nodes under each part's pads on the supply.
    let port_of = |part: usize| -> Vec<usize> {
        let mut v: Vec<usize> = g
            .pads
            .iter()
            .filter(|p| p.net == Some(net) && p.component == Some(part as u32))
            .filter_map(|p| node_at(p.at.x.mil(), p.at.y.mil()))
            .collect();
        v.sort_unstable();
        v
    };
    let designator = |i: usize| {
        board
            .components
            .get(i)
            .map(|c| c.designator.clone())
            .unwrap_or_default()
    };
    // Capacitors: two pads, one on the supply, one on ground.
    let mut pads_of: HashMap<usize, Vec<&crate::Pad>> = HashMap::new();
    for p in &g.pads {
        if let Some(c) = p.component {
            pads_of.entry(c as usize).or_default().push(p);
        }
    }
    let mut decaps = Vec::new();
    let mut decap_ports = Vec::new();
    let mut parts: Vec<usize> = pads_of.keys().copied().collect();
    parts.sort_unstable();
    let mut no_value = Vec::new();
    for &part in &parts {
        let pads = &pads_of[&part];
        if pads.len() != 2 {
            continue;
        }
        let (Some(a), Some(b)) = (
            pads.iter().find(|p| p.net == Some(net)),
            pads.iter().find(|p| p.net == Some(ground)),
        ) else {
            continue;
        };
        let comp = &board.components[part];
        let value = comp
            .param(&["Value", "Comment", "Capacitance", "Device Type"])
            .unwrap_or("")
            .to_string();
        let farads = parse_farads(&value).or_else(|| parse_farads(&comp.footprint));
        let des = comp.designator.to_ascii_uppercase();
        let is_cap = des.starts_with('C') && !des.starts_with("CON") || farads.is_some();
        if !is_cap {
            continue;
        }
        let (ax, ay, bx, by) = (a.at.x.mil(), a.at.y.mil(), b.at.x.mil(), b.at.y.mil());
        let pitch = (ax - bx).hypot(ay - by).max(10.0);
        let package = Package::from_pitch(pitch);
        let assumed = farads.is_none();
        let farads = match farads {
            Some(f) => f,
            None if setup.assume => package.typical_farads(),
            None => {
                no_value.push(comp.designator.clone());
                continue;
            }
        };
        if assumed {
            no_value.push(comp.designator.clone());
        }
        // Loop from the surface down to the middle of the cavity.
        let surface = if comp.side == Side::Bottom { nl - 1 } else { 0 };
        let mid = (zs[pl] + zs[gl]) / 2.0;
        let h = (zs[surface] - mid).abs().max(d / 2.0);
        let r = setup.via_drill_mil.max(4.0) / 2.0;
        let mount = MU0 * h / std::f64::consts::PI * (pitch / r).ln().max(0.5) + 0.1e-9;
        let port = port_of(part);
        if port.is_empty() {
            notes.push(format!(
                "{} is not over the plane pair; left out",
                comp.designator
            ));
            continue;
        }
        decap_ports.push(port);
        decaps.push(Decap {
            part,
            designator: comp.designator.clone(),
            value,
            model: CapModel::mlcc(farads, package),
            package,
            mount,
            assumed,
        });
    }
    if !no_value.is_empty() {
        notes.push(format!(
            "{} capacitors have no value to read, {}: {}",
            no_value.len(),
            if setup.assume {
                "taken as a typical value for their case (0201 100 nF, 0402 1 µF, 0603 4.7 µF, 0805 10 µF, 1206 22 µF)"
            } else {
                "left out"
            },
            no_value.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    let all_feeds = feeds(board, net, ground);
    let on_plane = |i: usize| !port_of(i).is_empty();
    let guessed = crate::pi::guess_source(board, net)
        .filter(|v| !setup.loads.contains(v))
        .filter(|&v| {
            // A connector off the planes says little; a bead may say more.
            let des = board.components[v].designator.to_ascii_uppercase();
            on_plane(v) || !matches!(des.chars().next(), Some('J' | 'P' | 'X'))
        });
    // A rail with chips of its own and a bead to another rail is a
    // sub-rail fed by it (the one from the rail with the most parts); a
    // rail with only beads to others is a distribution rail.
    let has_chips = crate::pi::default_loads(board, net, usize::MAX)
        .iter()
        .any(|&(p, _)| {
            on_plane(p)
                && !decaps.iter().any(|d| d.part == p)
                && !all_feeds.iter().any(|f| f.part == p)
                && !setup.loads.is_empty() == setup.loads.contains(&p)
        });
    let bead_in = || {
        if !has_chips {
            return None;
        }
        all_feeds
            .iter()
            .filter(|f| f.bead && on_plane(f.part) && !setup.loads.contains(&f.part))
            .max_by_key(|f| crate::pi::parts_on(board, f.net).len())
            .map(|f| f.part)
    };
    let vrm = setup.vrm.or(guessed).or_else(bead_in);
    let vrm_bead =
        setup.vrm.is_none() && vrm.is_some_and(|v| all_feeds.iter().any(|f| f.part == v && f.bead));
    let (vrm_r, vrm_l) = if vrm_bead {
        (BEAD_R, BEAD_L)
    } else {
        (setup.vrm_r, setup.vrm_l)
    };
    if let Some(v) = vrm.filter(|_| vrm_bead) {
        notes.push(format!(
            "fed through {} (a bead or filter inductor) from {}: taken as {} + {:.1} µH toward that rail",
            designator(v),
            all_feeds
                .iter()
                .find(|f| f.part == v)
                .map(|f| f.net_name.clone())
                .unwrap_or_default(),
            ohm_text(BEAD_R),
            BEAD_L * 1e6,
        ));
    }
    let sub_rails: Vec<Feed> = all_feeds
        .iter()
        .filter(|f| Some(f.part) != vrm && f.is_rail())
        .cloned()
        .collect();
    let loads: Vec<usize> = if setup.loads.is_empty() {
        let skip: Vec<usize> = decaps
            .iter()
            .map(|d| d.part)
            .chain(all_feeds.iter().map(|f| f.part))
            .collect();
        let chips: Vec<usize> = crate::pi::default_loads(board, net, vrm.unwrap_or(usize::MAX))
            .into_iter()
            .map(|p| p.0)
            .filter(|p| !skip.contains(p))
            .collect();
        if chips.iter().any(|&c| on_plane(c)) {
            chips
        } else {
            // Nothing draws from the rail itself: look from what it feeds.
            let fed: Vec<usize> = sub_rails
                .iter()
                .map(|f| f.part)
                .filter(|&p| on_plane(p))
                .collect();
            if !fed.is_empty() {
                notes.push(format!(
                    "no chip draws from {net_name} directly; looking from the parts feeding its sub-rails ({}); analyse a sub-rail for a chip's own decoupling",
                    fed.iter().take(8).map(|&p| designator(p)).collect::<Vec<_>>().join(", ")
                ));
                fed
            } else {
                chips
            }
        }
    } else {
        setup.loads.clone()
    };
    if !sub_rails.is_empty() {
        notes.push(format!(
            "feeds {} sub-rail{}: {}",
            sub_rails.len(),
            if sub_rails.len() == 1 { "" } else { "s" },
            sub_rails
                .iter()
                .take(10)
                .map(|f| format!("{} ({})", f.net_name, f.designator))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut ports: Vec<Vec<usize>> = Vec::new();
    let mut kept_loads = Vec::new();
    for &l in &loads {
        let p = port_of(l);
        if p.is_empty() {
            notes.push(format!(
                "{} is not over the plane pair; left out",
                designator(l)
            ));
        } else {
            ports.push(p);
            kept_loads.push(l);
        }
    }
    if kept_loads.is_empty() {
        return Err("no load over the plane pair to look from".into());
    }
    let vrm = vrm.and_then(|v| {
        let p = port_of(v);
        if p.is_empty() {
            notes.push(format!(
                "the regulator {} is not over the plane pair",
                designator(v)
            ));
            None
        } else {
            ports.push(p);
            Some(v)
        }
    });
    if vrm.is_none() {
        notes.push("no regulator: only the planes and capacitors are counted".into());
    }
    ports.extend(decap_ports);
    let np = ports.len();
    // The sweep.
    let points = setup.points.max(2);
    let freqs: Vec<f64> = (0..points)
        .map(|i| setup.fmin * (setup.fmax / setup.fmin).powf(i as f64 / (points - 1) as f64))
        .collect();
    let (tp, tg) = (t_of(pl), t_of(gl));
    let solve_at = |f: f64| -> Vec<Cx> {
        let w = std::f64::consts::TAU * f;
        let skin = |t: f64| ((RHO / t).powi(2) + std::f64::consts::PI * f * MU0 * RHO).sqrt();
        let zb = Cx::new(skin(tp) + skin(tg), w * MU0 * d);
        let yb = Cx::new(1.0, 0.0).div(zb);
        let ycell = Cx::new(w * c_cell * tan_d, w * c_cell);
        let mut m = Band::new(n, band);
        for i in 0..n {
            *m.at(i, i) = ycell;
        }
        for &(a, b) in &links {
            *m.at(a, a) = m.get(a, a).add(yb);
            *m.at(b, b) = m.get(b, b).add(yb);
            *m.at(a, b) = m.get(a.max(b), a.min(b)).add(yb.scale(-1.0));
        }
        m.factor();
        let mut z = vec![Cx::default(); np * np];
        let mut x = vec![Cx::default(); n];
        for j in 0..np {
            x.iter_mut().for_each(|v| *v = Cx::default());
            let wj = 1.0 / ports[j].len() as f64;
            for &c in &ports[j] {
                x[c] = x[c].add(Cx::new(wj, 0.0));
            }
            m.solve(&mut x);
            for i in 0..np {
                let wi = 1.0 / ports[i].len() as f64;
                let mut s = Cx::default();
                for &c in &ports[i] {
                    s = s.add(x[c]);
                }
                z[i * np + j] = s.scale(wi);
            }
        }
        z
    };
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let mut zp: Vec<Vec<Cx>> = vec![Vec::new(); points];
    std::thread::scope(|s| {
        let chunks: Vec<_> = (0..threads)
            .map(|t| {
                let freqs = &freqs;
                let solve_at = &solve_at;
                s.spawn(move || {
                    (t..freqs.len())
                        .step_by(threads)
                        .map(|k| (k, solve_at(freqs[k])))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in chunks {
            for (k, z) in h.join().unwrap_or_default() {
                zp[k] = z;
            }
        }
    });
    let target = setup.target.or_else(|| {
        let v = setup.volts.or_else(|| volts_in_name(&net_name))?;
        (setup.amps > 0.0).then(|| v * setup.ripple_pct / 100.0 / setup.amps)
    });
    if setup.target.is_none() && setup.volts.is_none() {
        match volts_in_name(&net_name) {
            Some(v) => notes.push(format!(
                "target from {v} V (read from the net's name), {} % ripple, {} A step",
                setup.ripple_pct, setup.amps
            )),
            None => notes.push("no target: give the supply's volts and current step".into()),
        }
    }
    let shared = (0..n).count() as f64 * cell * cell / 1e6;
    Ok(Model {
        freqs,
        loads: kept_loads,
        vrm,
        vrm_r,
        vrm_l,
        vrm_bead,
        sub_rails,
        decaps,
        cavity: Cavity {
            power_layer: names[pl].clone(),
            ground_layer: names[gl].clone(),
            ground_net: board
                .nets
                .get(ground as usize)
                .map(|n| n.name.clone())
                .unwrap_or_default(),
            d_mil: d / MIL,
            er,
            tan_d,
            area_in2: shared,
            farads: c_cell * n as f64,
            cell_mil: cell,
            nodes: n,
        },
        target,
        band: setup.target_fmax.min(setup.fmax),
        notes,
        zp,
    })
}

/// Working state for trying capacitor changes: at each frequency, the
/// inverse of the ended ports' matrix and what each load sees through it.
struct Ended {
    /// Indices of ended ports (the regulator, then every capacitor).
    t: Vec<usize>,
    minv: Vec<Vec<Cx>>,
    /// c[f][load][k] = (M⁻¹ b_load)[k].
    c: Vec<Vec<Vec<Cx>>>,
    /// Impedance at each load: z[f][load].
    z: Vec<Vec<Cx>>,
    /// Termination now on each ended port, per frequency.
    term: Vec<Vec<Cx>>,
}

impl Model {
    fn np(&self) -> usize {
        self.loads.len() + usize::from(self.vrm.is_some()) + self.decaps.len()
    }

    fn first_cap(&self) -> usize {
        self.loads.len() + usize::from(self.vrm.is_some())
    }

    fn cap_z(&self, k: usize, cap: Option<&CapModel>, w: f64) -> Cx {
        match cap {
            Some(m) => m.z(w, self.decaps[k].mount),
            None => Cx::new(OPEN, 0.0),
        }
    }

    fn ended(&self, caps: &[Option<CapModel>]) -> Ended {
        let (np, nl, fc) = (self.np(), self.loads.len(), self.first_cap());
        let t: Vec<usize> = (nl..np).collect();
        let k = t.len();
        let mut e = Ended {
            t: t.clone(),
            minv: Vec::new(),
            c: Vec::new(),
            z: Vec::new(),
            term: Vec::new(),
        };
        for (fi, &f) in self.freqs.iter().enumerate() {
            let w = std::f64::consts::TAU * f;
            let zp = &self.zp[fi];
            let term: Vec<Cx> = t
                .iter()
                .map(|&p| {
                    if p < fc {
                        Cx::new(self.vrm_r, w * self.vrm_l)
                    } else {
                        self.cap_z(p - fc, caps[p - fc].as_ref(), w)
                    }
                })
                .collect();
            let mut m = vec![Cx::default(); k * k];
            for a in 0..k {
                for b in 0..k {
                    m[a * k + b] = zp[t[a] * np + t[b]];
                }
                m[a * k + a] = m[a * k + a].add(term[a]);
            }
            let minv = if k > 0 { invert(&m, k) } else { Vec::new() };
            let mut cs = Vec::with_capacity(nl);
            let mut zs = Vec::with_capacity(nl);
            for l in 0..nl {
                let bl: Vec<Cx> = t.iter().map(|&p| zp[p * np + l]).collect();
                let c: Vec<Cx> = (0..k)
                    .map(|a| (0..k).fold(Cx::default(), |s, b| s.add(minv[a * k + b].mul(bl[b]))))
                    .collect();
                let q = (0..k).fold(Cx::default(), |s, a| s.add(bl[a].mul(c[a])));
                zs.push(zp[l * np + l].add(q.scale(-1.0)));
                cs.push(c);
            }
            e.minv.push(minv);
            e.c.push(cs);
            e.z.push(zs);
            e.term.push(term);
        }
        e
    }

    /// |Z| at each load over the sweep, with these capacitors (one entry
    /// per `decaps`; None leaves it off).
    pub fn evaluate(&self, caps: &[Option<CapModel>]) -> Curves {
        let e = self.ended(caps);
        (0..self.loads.len())
            .map(|l| e.z.iter().map(|z| z[l].abs()).collect())
            .collect()
    }

    /// The capacitors as on the board.
    pub fn as_built(&self) -> Vec<Option<CapModel>> {
        self.decaps.iter().map(|d| Some(d.model)).collect()
    }

    /// How many sweep points lie in the target's band.
    pub fn in_band(&self) -> usize {
        self.freqs
            .iter()
            .take_while(|&&f| f <= self.band * 1.0001)
            .count()
            .max(1)
    }

    /// Worst |Z| ÷ target over the curves within the target's band:
    /// (ratio, frequency, load index).
    pub fn worst(&self, curves: &Curves) -> Option<(f64, f64, usize)> {
        let t = self.target?;
        let nb = self.in_band();
        let mut best: Option<(f64, f64, usize)> = None;
        for (l, c) in curves.iter().enumerate() {
            for (fi, &z) in c.iter().enumerate().take(nb) {
                let r = z / t;
                if best.is_none_or(|b| r > b.0) {
                    best = Some((r, self.freqs[fi], l));
                }
            }
        }
        best
    }

    /// Frequency bands where any load is over the target, Hz.
    pub fn over(&self, curves: &Curves) -> Vec<(f64, f64)> {
        let Some(t) = self.target else {
            return Vec::new();
        };
        let mut out: Vec<(f64, f64)> = Vec::new();
        let mut start: Option<f64> = None;
        let nb = self.in_band();
        for (fi, &f) in self.freqs.iter().enumerate().take(nb) {
            let bad = curves.iter().any(|c| c[fi] > t);
            match (bad, start) {
                (true, None) => start = Some(f),
                (false, Some(s)) => {
                    out.push((s, self.freqs[fi - 1]));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push((s, self.freqs[nb - 1]));
        }
        out
    }
}

/// One change the optimiser suggests.
#[derive(Clone, Debug)]
pub struct Change {
    pub decap: usize,
    pub from: Option<CapModel>,
    /// None takes the capacitor off.
    pub to: Option<CapModel>,
}

/// The optimiser's result.
#[derive(Clone, Debug)]
pub struct Plan {
    pub caps: Vec<Option<CapModel>>,
    pub changes: Vec<Change>,
    /// Worst |Z| ÷ target before and after.
    pub before: f64,
    pub after: f64,
    pub count_before: usize,
    pub count_after: usize,
}

impl Plan {
    pub fn summary(&self) -> String {
        format!(
            "worst {:.2}× target -> {:.2}×; {} capacitors -> {}, {} changed",
            self.before,
            self.after,
            self.count_before,
            self.count_after,
            self.changes.len()
        )
    }
}

/// Pick capacitor values (from [`VALUES`], within each case's limit) for
/// the existing sites so every load stays under the target, then take off
/// the capacitors that are not needed. Without a target it only lowers
/// the worst impedance.
pub fn optimise(model: &Model, start: &[Option<CapModel>]) -> Plan {
    let target = model.target.unwrap_or(1.0);
    let nf = model.freqs.len();
    let nl = model.loads.len();
    let fc = model.first_cap() - model.loads.len();
    let mut e = model.ended(start);
    let mut caps = start.to_vec();
    let nb = model.in_band();
    let has_target = model.target.is_some();
    // How far over the target the band is: its worst point plus its mean,
    // on a log scale, nothing for points under the target. Without a
    // target, the same on |Z| itself.
    let fold = |worst_at: &mut dyn FnMut(usize) -> f64| -> f64 {
        let (mut top, mut sum) = (f64::MIN, 0.0);
        for fi in 0..nb {
            let g = worst_at(fi).max(1e-12).ln();
            let g = if has_target { g.max(0.0) } else { g };
            top = top.max(g);
            sum += g;
        }
        top + sum / nb as f64
    };
    let score_of = |z: &Vec<Vec<Cx>>| -> f64 {
        fold(&mut |fi| z[fi].iter().map(|v| v.abs() / target).fold(0.0, f64::max))
    };
    let ratio_of = |z: &Vec<Vec<Cx>>| -> f64 {
        z.iter()
            .take(nb)
            .flat_map(|zf| zf.iter().map(|v| v.abs() / target))
            .fold(0.0, f64::max)
    };
    // The score if port `a` (of the ended ones) took termination `new`.
    let trial = |e: &Ended, a: usize, new: &[Cx]| -> f64 {
        let k = e.t.len();
        fold(&mut |fi| {
            let m = e.minv[fi][a * k + a];
            let delta = new[fi].add(e.term[fi][a].scale(-1.0));
            let den = Cx::new(1.0, 0.0).add(delta.mul(m));
            let g = delta.div(den);
            (0..nl).fold(0.0, |w: f64, l| {
                let c = e.c[fi][l][a];
                w.max(e.z[fi][l].add(c.mul(c).mul(g)).abs() / target)
            })
        })
    };
    let accept = |e: &mut Ended, a: usize, new: &[Cx]| {
        let k = e.t.len();
        for fi in 0..nf {
            let m = e.minv[fi][a * k + a];
            let delta = new[fi].add(e.term[fi][a].scale(-1.0));
            let g = delta.div(Cx::new(1.0, 0.0).add(delta.mul(m)));
            let col: Vec<Cx> = (0..k).map(|i| e.minv[fi][i * k + a]).collect();
            for i in 0..k {
                for j in 0..k {
                    let v = e.minv[fi][i * k + j].add(col[i].mul(col[j]).mul(g).scale(-1.0));
                    e.minv[fi][i * k + j] = v;
                }
            }
            for l in 0..nl {
                let ca = e.c[fi][l][a];
                for i in 0..k {
                    e.c[fi][l][i] = e.c[fi][l][i].add(col[i].mul(ca).mul(g).scale(-1.0));
                }
                e.z[fi][l] = e.z[fi][l].add(ca.mul(ca).mul(g));
            }
            e.term[fi][a] = new[fi];
        }
    };
    let terms = |k: usize, cap: Option<&CapModel>| -> Vec<Cx> {
        model
            .freqs
            .iter()
            .map(|&f| model.cap_z(k, cap, std::f64::consts::TAU * f))
            .collect()
    };
    let before = ratio_of(&e.z);
    let mut score = score_of(&e.z);
    // 1. Values, site by site, while that lowers the worst.
    for _ in 0..4 {
        let mut better = false;
        for k in 0..model.decaps.len() {
            let a = fc + k;
            let pkg = model.decaps[k].package;
            let mut best: Option<(f64, CapModel)> = None;
            for &v in VALUES.iter().filter(|&&v| v <= pkg.max_farads()) {
                let cand = CapModel::mlcc(v, pkg);
                if caps[k].map(|c| c.farads) == Some(v) {
                    continue;
                }
                let s = trial(&e, a, &terms(k, Some(&cand)));
                if s < score - 1e-4 && best.is_none_or(|b| s < b.0) {
                    best = Some((s, cand));
                }
            }
            if let Some((s, cand)) = best {
                accept(&mut e, a, &terms(k, Some(&cand)));
                caps[k] = Some(cand);
                score = s;
                better = true;
            }
        }
        if !better {
            break;
        }
    }
    // 2. Off with what is not needed, while under the target.
    if has_target && score <= 1e-9 {
        loop {
            let mut best: Option<(f64, usize)> = None;
            for k in 0..model.decaps.len() {
                if caps[k].is_none() {
                    continue;
                }
                let s = trial(&e, fc + k, &terms(k, None));
                if s <= 1e-9 && best.is_none_or(|b| s < b.0) {
                    best = Some((s, k));
                }
            }
            let Some((s, k)) = best else { break };
            accept(&mut e, fc + k, &terms(k, None));
            caps[k] = None;
            score = s;
        }
    }
    // The working state drifts a little through the updates: measure the
    // result afresh.
    let after = ratio_of(&model.ended(&caps).z);
    let changes = (0..caps.len())
        .filter(|&k| caps[k].map(|c| c.farads) != start[k].map(|c| c.farads))
        .map(|k| Change {
            decap: k,
            from: start[k],
            to: caps[k],
        })
        .collect();
    let _ = score;
    Plan {
        count_before: start.iter().flatten().count(),
        count_after: caps.iter().flatten().count(),
        caps,
        changes,
        before,
        after,
    }
}

/// A capacitance for people: 100 nF, 4.7 µF.
pub fn farads_text(f: f64) -> String {
    let (v, u) = if f >= 1e-6 {
        (f / 1e-6, "µF")
    } else if f >= 1e-9 {
        (f / 1e-9, "nF")
    } else {
        (f / 1e-12, "pF")
    };
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s} {u}")
}

/// A frequency for people: 120 kHz, 35.5 MHz.
/// A frequency band as text; a band of one point reads as that point.
pub fn band_text(a: f64, b: f64) -> String {
    if hz_text(a) == hz_text(b) {
        hz_text(a)
    } else {
        format!("{}–{}", hz_text(a), hz_text(b))
    }
}

pub fn hz_text(f: f64) -> String {
    if f >= 1e9 {
        format!("{:.3} GHz", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1} MHz", f / 1e6)
    } else {
        format!("{:.0} kHz", f / 1e3)
    }
}

/// An impedance for people: 12.3 mΩ.
pub fn ohm_text(z: f64) -> String {
    if z >= 1.0 {
        format!("{z:.2} Ω")
    } else {
        format!("{:.1} mΩ", z * 1e3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn board(size: f64, d_mil: f64) -> Board {
        let mut b = Board::default();
        for (i, name) in ["Top", "VDD", "GND", "Bottom"].iter().enumerate() {
            if i > 0 {
                b.stackup.layers.push(StackLayer {
                    name: format!("D{i}"),
                    altium_id: None,
                    kind: LayerKind::Dielectric,
                    thickness: Length::from_mil(if i == 2 { d_mil } else { 5.0 }),
                    dielectric_constant: Some(4.0),
                    loss_tangent: Some(0.0),
                    roughness_um: None,
                    material: None,
                });
            }
            b.stackup.layers.push(StackLayer {
                name: name.to_string(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for n in ["VDD_1V0", "GND"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        for (l, net) in [(1u8, 0u32), (2, 1)] {
            b.geometry.polygons.push(Polygon {
                teardrop: false,
                layer: LayerRef::Copper(l),
                net: Some(net),
                outline: vec![
                    Point::mil(0.0, 0.0),
                    Point::mil(size, 0.0),
                    Point::mil(size, size),
                    Point::mil(0.0, size),
                ],
                holes: Vec::new(),
                pour: None,
                component: None,
            });
        }
        b
    }

    fn part(b: &mut Board, des: &str, value: &str, pads: &[(f64, f64, u32)]) -> usize {
        let i = b.components.len();
        let mut params = std::collections::BTreeMap::new();
        if !value.is_empty() {
            params.insert("Value".to_string(), value.to_string());
        }
        b.components.push(Component {
            designator: des.into(),
            footprint: String::new(),
            library_ref: None,
            x: Length::from_mil(pads[0].0),
            y: Length::from_mil(pads[0].1),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params,
        });
        for (k, &(x, y, net)) in pads.iter().enumerate() {
            b.geometry.pads.push(Pad {
                layer: LayerRef::Copper(0),
                net: Some(net),
                component: Some(i as u32),
                name: (k + 1).to_string(),
                at: Point::mil(x, y),
                size_x: Length::from_mil(20.0),
                size_y: Length::from_mil(20.0),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 0.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            });
        }
        i
    }

    #[test]
    fn follows_beads_to_sub_rails() {
        assert_eq!(parse_ohms("0R"), Some(0.0));
        assert_eq!(parse_ohms("4R7"), Some(4.7));
        assert_eq!(parse_ohms("10 ohm"), Some(10.0));
        assert_eq!(parse_ohms("1k5"), Some(1500.0));
        assert_eq!(parse_ohms("22Ω"), Some(22.0));
        assert_eq!(parse_ohms("BLM18"), None);
        let mut b = board(1000.0, 4.0);
        // The rail on the left half; a sub-rail on the right, through FB1.
        b.geometry.polygons[0].outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(500.0, 0.0),
            Point::mil(500.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        b.nets.push(Net {
            name: "VSUB_1V0".into(),
            ..Default::default()
        });
        let mut sub = b.geometry.polygons[0].clone();
        sub.net = Some(2);
        sub.outline = vec![
            Point::mil(520.0, 0.0),
            Point::mil(1000.0, 0.0),
            Point::mil(1000.0, 1000.0),
            Point::mil(520.0, 1000.0),
        ];
        b.geometry.polygons.push(sub);
        let fb = part(
            &mut b,
            "FB1",
            "BLM18PG221",
            &[(480.0, 500.0, 0), (540.0, 500.0, 2)],
        );
        part(
            &mut b,
            "C2",
            "10uF",
            &[(200.0, 400.0, 0), (200.0, 440.0, 1)],
        );
        let u1 = part(&mut b, "U1", "", &[(800.0, 500.0, 2), (820.0, 500.0, 1)]);
        part(&mut b, "C1", "1uF", &[(700.0, 400.0, 2), (700.0, 440.0, 1)]);
        part(&mut b, "R1", "10k", &[(300.0, 700.0, 0), (340.0, 700.0, 2)]);
        let f = feeds(&b, 0, 1);
        assert_eq!(f.len(), 2);
        assert!(f.iter().any(|f| f.part == fb && f.bead && f.is_rail()));
        assert!(f.iter().any(|f| f.designator == "R1" && !f.is_rail()));
        // The rail feeds only the sub-rail: seen from the bead.
        let mut s = Setup::new(0);
        s.points = 8;
        let m = model(&b, &s).unwrap();
        assert_eq!(m.loads, vec![fb]);
        assert_eq!(m.sub_rails.len(), 1);
        assert_eq!(m.sub_rails[0].net, 2);
        assert!(m.notes.iter().any(|n| n.contains("no chip draws")));
        // The sub-rail: fed by the bead, seen from the chip.
        let mut s = Setup::new(2);
        s.points = 8;
        let m = model(&b, &s).unwrap();
        assert_eq!(m.vrm, Some(fb));
        assert!(m.vrm_bead);
        assert_eq!(m.loads, vec![u1]);
        assert_eq!(m.decaps.len(), 1);
        assert!((m.vrm_l - BEAD_L).abs() < 1e-12);
    }

    #[test]
    fn reads_values_and_names() {
        assert!((parse_farads("100nF").unwrap() - 100e-9).abs() < 1e-15);
        assert!((parse_farads("0.1uF 16V X7R").unwrap() - 1e-7).abs() < 1e-15);
        assert!((parse_farads("4u7").unwrap() - 4.7e-6).abs() < 1e-15);
        assert_eq!(parse_farads("1000pF"), Some(1e-9));
        assert!((parse_farads("10µF/6.3V").unwrap() - 10e-6).abs() < 1e-15);
        assert_eq!(parse_farads("CAP_0402"), None);
        assert_eq!(parse_farads("10k"), None);
        assert_eq!(volts_in_name("VDD_1V8"), Some(1.8));
        assert_eq!(volts_in_name("+3P3V"), Some(3.3));
        assert_eq!(volts_in_name("VCC5V"), Some(5.0));
        assert_eq!(volts_in_name("GND"), None);
        assert_eq!(Package::from_pitch(40.0), Package::P0402);
    }

    #[test]
    fn band_ldlt_matches_a_dense_solve() {
        // A small complex symmetric system.
        let n = 6;
        let mut m = Band::new(n, 2);
        let mut dense = vec![Cx::default(); n * n];
        for i in 0..n {
            for j in i.saturating_sub(2)..=i {
                let v = if i == j {
                    Cx::new(4.0 + i as f64, 1.0)
                } else {
                    Cx::new(-1.0, 0.3 * (i + j) as f64)
                };
                *m.at(i, j) = v;
                dense[i * n + j] = v;
                dense[j * n + i] = v;
            }
        }
        let inv = invert(&dense, n);
        m.factor();
        let mut x: Vec<Cx> = (0..n).map(|i| Cx::new(i as f64, 1.0)).collect();
        let rhs = x.clone();
        m.solve(&mut x);
        for i in 0..n {
            let want = (0..n).fold(Cx::default(), |s, j| s.add(inv[i * n + j].mul(rhs[j])));
            assert!((x[i].re - want.re).abs() < 1e-9 && (x[i].im - want.im).abs() < 1e-9);
        }
    }

    #[test]
    fn plane_pair_is_its_capacitance_then_resonates() {
        // 2 × 2 inch planes 4 mil apart, εr 4: about 4.6 nF; the first
        // cavity resonance (seen from a corner) at c / (2 a √εr) ≈ 1.48 GHz.
        let mut b = board(2000.0, 4.0);
        part(
            &mut b,
            "U1",
            "",
            &[(30.0, 30.0, 0), (60.0, 30.0, 1), (30.0, 60.0, 0)],
        );
        let mut s = Setup::new(0);
        s.fmin = 1e6;
        s.fmax = 2e9;
        s.points = 120;
        s.cell_mil = 50.0;
        s.loads = vec![0];
        let m = model(&b, &s).unwrap();
        let c = 8.854e-12 * 4.0 * (2000.0 * MIL).powi(2) / (4.0 * MIL);
        assert!((m.cavity.farads - c).abs() / c < 0.02, "{:?}", m.cavity);
        let z = m.evaluate(&m.as_built());
        let z1 = 1.0 / (std::f64::consts::TAU * 1e6 * c);
        assert!((z[0][0] - z1).abs() / z1 < 0.05, "{} vs {z1}", z[0][0]);
        // First peak above 500 MHz.
        let (fi, _) = z[0]
            .iter()
            .enumerate()
            .filter(|(i, _)| m.freqs[*i] > 5e8)
            .fold((0, 0.0), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
        let f = m.freqs[fi];
        assert!(f > 1.3e9 && f < 1.65e9, "peak at {f}");
    }

    #[test]
    fn a_decap_sets_a_dip_and_the_optimiser_meets_the_target() {
        let mut b = board(2000.0, 4.0);
        part(
            &mut b,
            "U1",
            "",
            &[(1000.0, 1000.0, 0), (1040.0, 1000.0, 1)],
        );
        for k in 0..6 {
            let x = 900.0 + 40.0 * k as f64;
            part(
                &mut b,
                &format!("C{k}"),
                "100nF",
                &[(x, 1100.0, 0), (x, 1140.0, 1)],
            );
        }
        let mut s = Setup::new(0);
        s.fmin = 100e3;
        s.fmax = 1e9;
        s.points = 50;
        s.loads = vec![0];
        s.target = Some(0.02);
        let m = model(&b, &s).unwrap();
        assert_eq!(m.decaps.len(), 6);
        assert_eq!(m.cavity.ground_net, "GND");
        let built = m.evaluate(&m.as_built());
        let bare = m.evaluate(&[None; 6]);
        // Six 100 nF in parallel: far below the bare planes at 10 MHz.
        let at = |c: &Curves, f: f64| {
            let i = m.freqs.iter().position(|&x| x >= f).unwrap();
            c[0][i]
        };
        assert!(at(&built, 10e6) < at(&bare, 10e6) / 5.0);
        let plan = optimise(&m, &m.as_built());
        assert!(plan.after <= plan.before + 1e-9, "{}", plan.summary());
        let again = m.worst(&m.evaluate(&plan.caps)).unwrap().0;
        assert!((again - plan.after).abs() < 1e-6);
    }
}
