//! Board-level EMI check: the layout faults that make a board radiate,
//! and an estimate of what its fast nets radiate against the FCC and
//! CISPR limits.
//!
//! **Faults** (for the fast nets: clocks and the nets given, and every
//! differential pair):
//! - a track over a gap in its reference plane, or across a split
//!   between two plane nets (the return current detours round it, and
//!   the loop it opens is a slot antenna);
//! - a via that takes the net to a layer with another reference plane
//!   with no return via (same plane net) or decoupling capacitor
//!   (different plane nets) near it;
//! - a fast track near the board's edge (its field fringes past the
//!   plane's edge; kept at least five times its height over the plane);
//! - ground stitching along the board's edge farther apart than a
//!   twentieth of a wavelength at the highest frequency of interest.
//!
//! **Radiated emission** (differential mode, Ott, *Electromagnetic
//! Compatibility Engineering*, eq. 12-3): a net on an outer layer and
//! its return in the plane under it form a small loop of area A
//! (length times height). With harmonic current I at frequency f, the
//! field at distance r, over a reflecting ground as the test site has,
//! is E = 2.63e-14 f² A I / r (V/m, SI units). The net's current is a
//! trapezoid of the swing over twice its impedance (a series terminated
//! line while the edge travels): harmonic n of a 50 % duty clock has
//! I·|sinc(nπ/2)|·|sinc(nπ f tr)|. Nets inside the board, between two
//! planes, are taken not to radiate this way. Cables, the usual largest
//! source (common mode), are not modelled: the margin here is the
//! board's own.

use crate::drc::si::{self, Planes};
use crate::route::{point_seg, pt, P};
use crate::{Board, LayerRef};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::fmt::Write;

const C0: f64 = 2.998e8;

/// A fast net: its fundamental, edge and swing.
#[derive(Clone, Debug)]
pub struct FastNet {
    /// Net name or pattern (* and ?).
    pub pattern: String,
    /// Clock or toggle frequency, Hz (a data net: half its bit rate).
    pub freq: f64,
    /// Edge, 10 to 90 %, seconds.
    pub rise: f64,
    /// Swing, volts.
    pub volts: f64,
}

impl FastNet {
    /// "CLK*=25MHz@2ns,3.3V" (edge and swing optional).
    pub fn parse(s: &str) -> Result<FastNet, String> {
        let (pat, spec) = s
            .split_once('=')
            .ok_or_else(|| format!("{s}: NET=FREQ[@RISE][,VOLTS]"))?;
        let mut parts = spec.split(',');
        let fr = parts.next().unwrap_or("");
        let (f, r) = fr.split_once('@').unwrap_or((fr, ""));
        let freq = freq_hz(f).ok_or_else(|| format!("{s}: frequency {f}"))?;
        let rise = if r.is_empty() {
            default_rise(freq)
        } else {
            time_s(r).ok_or_else(|| format!("{s}: edge {r}"))?
        };
        let volts = match parts.next() {
            Some(v) => v
                .trim()
                .trim_end_matches(['V', 'v'])
                .parse::<f64>()
                .map_err(|_| format!("{s}: volts {v}"))?,
            None => 3.3,
        };
        Ok(FastNet {
            pattern: pat.trim().to_string(),
            freq,
            rise,
            volts,
        })
    }
}

/// An edge for a clock when none is given: a tenth of its period, at
/// most 2 ns and at least 100 ps.
fn default_rise(f: f64) -> f64 {
    (0.1 / f).clamp(100e-12, 2e-9)
}

fn freq_hz(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase();
    let (num, k) = if let Some(v) = t.strip_suffix("ghz") {
        (v, 1e9)
    } else if let Some(v) = t.strip_suffix("mhz") {
        (v, 1e6)
    } else if let Some(v) = t.strip_suffix("khz") {
        (v, 1e3)
    } else if let Some(v) = t.strip_suffix("hz") {
        (v, 1.0)
    } else {
        (t.as_str(), 1e6)
    };
    num.trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * k)
        .filter(|v| *v > 0.0)
}

fn time_s(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase();
    let (num, k) = if let Some(v) = t.strip_suffix("ps") {
        (v, 1e-12)
    } else if let Some(v) = t.strip_suffix("ns") {
        (v, 1e-9)
    } else {
        (t.as_str(), 1e-9)
    };
    num.trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * k)
        .filter(|v| *v > 0.0)
}

/// The emission limit to compare against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// FCC Part 15 class B, 3 m.
    FccB,
    /// FCC Part 15 class A, 10 m.
    FccA,
    /// CISPR 32 (EN 55032) class B: 10 m below 1 GHz, 3 m above.
    CisprB,
    /// CISPR 32 class A.
    CisprA,
}

impl Limit {
    pub fn parse(s: &str) -> Option<Limit> {
        match s.to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "fccb" | "b" | "fcc" => Some(Limit::FccB),
            "fcca" | "a" => Some(Limit::FccA),
            "cisprb" | "cispr" | "en55032b" | "cispr32b" | "en55032" => Some(Limit::CisprB),
            "cispra" | "en55032a" | "cispr32a" => Some(Limit::CisprA),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Limit::FccB => "FCC Part 15 class B",
            Limit::FccA => "FCC Part 15 class A",
            Limit::CisprB => "CISPR 32 class B",
            Limit::CisprA => "CISPR 32 class A",
        }
    }

    /// The limit at `f` Hz: dBµV/m and the distance it is measured at,
    /// m. None outside 30 MHz to 6 GHz.
    pub fn at(self, f: f64) -> Option<(f64, f64)> {
        let m = f / 1e6;
        if !(30.0..=6000.0).contains(&m) {
            return None;
        }
        Some(match self {
            Limit::FccB => (
                if m < 88.0 {
                    40.0
                } else if m < 216.0 {
                    43.5
                } else if m < 960.0 {
                    46.0
                } else {
                    54.0
                },
                3.0,
            ),
            Limit::FccA => (
                if m < 88.0 {
                    39.1
                } else if m < 216.0 {
                    43.5
                } else if m < 960.0 {
                    46.4
                } else {
                    49.5
                },
                10.0,
            ),
            Limit::CisprB => {
                if m < 230.0 {
                    (30.0, 10.0)
                } else if m <= 1000.0 {
                    (37.0, 10.0)
                } else if m < 3000.0 {
                    (50.0, 3.0)
                } else {
                    (54.0, 3.0)
                }
            }
            Limit::CisprA => {
                if m < 230.0 {
                    (40.0, 10.0)
                } else if m <= 1000.0 {
                    (47.0, 10.0)
                } else if m < 3000.0 {
                    (56.0, 3.0)
                } else {
                    (60.0, 3.0)
                }
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct EmiSetup {
    /// Nets given with their frequency.
    pub nets: Vec<FastNet>,
    /// Also take nets named as clocks (CLK, SCK, XTAL, OSC…) and every
    /// differential pair as fast.
    pub auto: bool,
    /// For auto-found clocks: frequency (Hz), edge (s) and swing (V).
    pub auto_freq: f64,
    pub auto_rise: f64,
    pub auto_volts: f64,
    pub limit: Limit,
    /// Margin wanted below the limit, dB.
    pub margin_db: f64,
}

impl Default for EmiSetup {
    fn default() -> Self {
        EmiSetup {
            nets: Vec::new(),
            auto: true,
            auto_freq: 50e6,
            auto_rise: 1e-9,
            auto_volts: 3.3,
            limit: Limit::FccB,
            margin_db: 6.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

/// A layout fault.
#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    /// "plane gap", "split", "layer change", "board edge", "edge stitching".
    pub kind: &'static str,
    pub net: String,
    pub layer: Option<String>,
    /// Where, mil.
    pub at: [f64; 2],
    /// What was found and what is wanted, mil.
    pub found: f64,
    pub need: f64,
    pub text: String,
    /// What to do about it.
    pub fix: String,
}

/// A net's estimated emission.
#[derive(Clone, Debug, Default)]
pub struct Emission {
    pub net: String,
    pub freq: f64,
    pub rise: f64,
    pub volts: f64,
    /// Assumed rather than given.
    pub assumed: bool,
    /// Impedance of its outer-layer track, Ω.
    pub z0: f64,
    /// Length on outer and inner layers, mil.
    pub outer: f64,
    pub inner: f64,
    /// Loop area (outer-layer length times height), mm².
    pub area_mm2: f64,
    /// Per harmonic: Hz, dBµV/m at the limit's distance, limit dBµV/m.
    pub spectrum: Vec<(f64, f64, f64)>,
    /// The harmonic closest to (or over) its limit.
    pub worst: Option<(f64, f64, f64)>,
}

impl Emission {
    pub fn margin(&self) -> Option<f64> {
        self.worst.map(|(_, e, l)| l - e)
    }
}

#[derive(Clone, Debug, Default)]
pub struct EmiResult {
    pub findings: Vec<Finding>,
    pub emissions: Vec<Emission>,
    pub limit: Option<Limit>,
    pub margin_db: f64,
    /// Highest frequency the stitching is held to, Hz.
    pub fmax: f64,
    pub fast_nets: usize,
    pub notes: Vec<String>,
}

fn is_clock(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    let tokens: Vec<&str> = u
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let starts = [
        "CLK", "SCK", "SCLK", "MCLK", "BCLK", "LRCLK", "REFCLK", "XTAL", "OSC", "XO", "CK", "TCK",
        "PCLK", "HCLK", "SYSCLK",
    ];
    tokens.iter().any(|t| {
        starts.iter().any(|s| {
            t == s || (t.starts_with(s) && t[s.len()..].chars().all(|c| c.is_ascii_digit()))
        }) || t.ends_with("CLK")
            || t.contains("CLK") && t.len() <= 10
    })
}

/// Names a pair's negative line might have.
fn negatives(name: &str) -> Vec<String> {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, num) = name.split_at(name.len() - digits);
    let mut out = vec![
        format!("{stem}N{num}"),
        format!("{stem}_N{num}"),
        format!("{name}N"),
        format!("{name}_N"),
        format!("{name}#"),
    ];
    for (p, n) in [("_P", "_N"), ("P", "N"), ("+", "-")] {
        if let Some(base) = name.strip_suffix(p) {
            out.push(format!("{base}{n}"));
        }
    }
    out
}

fn is_ground(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    u.contains("GND") || u.contains("VSS") || u == "0V" || u.contains("GROUND")
}

/// Runs the check.
pub fn check(board: &Board, setup: &EmiSetup) -> EmiResult {
    let mut notes = Vec::new();
    // The fast nets: given ones first.
    let mut fast: HashMap<u32, (FastNet, bool)> = HashMap::new();
    for f in &setup.nets {
        let mut hit = false;
        for (i, n) in board.nets.iter().enumerate() {
            let m = if f.pattern.contains(['*', '?']) {
                crate::constraints::glob(&f.pattern, &n.name)
            } else {
                n.name.eq_ignore_ascii_case(&f.pattern)
            };
            if m {
                fast.insert(i as u32, (f.clone(), false));
                hit = true;
            }
        }
        if !hit {
            notes.push(format!("no net matches {}", f.pattern));
        }
    }
    let mut pair_nets: HashSet<u32> = HashSet::new();
    if setup.auto {
        let mut pairs = board.diff_pairs.clone();
        pairs.extend(crate::zcheck::name_pairs(board));
        let id = |s: &str| {
            board
                .nets
                .iter()
                .position(|n| n.name == s)
                .map(|i| i as u32)
        };
        for p in &pairs {
            for n in [id(&p.positive), id(&p.negative)].into_iter().flatten() {
                pair_nets.insert(n);
            }
        }
        // Clock pairs by name: CLK / CLKN, CLK_P / CLK_N.
        for (i, n) in board.nets.iter().enumerate() {
            if !is_clock(&n.name) {
                continue;
            }
            for neg in negatives(&n.name) {
                if let Some(j) = board
                    .nets
                    .iter()
                    .position(|m| m.name.eq_ignore_ascii_case(&neg))
                {
                    pair_nets.insert(i as u32);
                    pair_nets.insert(j as u32);
                }
            }
        }
        for (i, n) in board.nets.iter().enumerate() {
            let i = i as u32;
            if fast.contains_key(&i)
                || pair_nets.contains(&i)
                || !is_clock(&n.name)
                || is_ground(&n.name)
            {
                continue;
            }
            fast.insert(
                i,
                (
                    FastNet {
                        pattern: n.name.clone(),
                        freq: setup.auto_freq,
                        rise: setup.auto_rise,
                        volts: setup.auto_volts,
                    },
                    true,
                ),
            );
        }
    }
    let planes = Planes::new(board);
    let hs = si::heights(board);
    let geo = crate::impedance::layer_geometry(&board.stackup);
    let ncu = planes.layers.len().max(geo.len());
    let net_name = |n: u32| {
        board
            .nets
            .get(n as usize)
            .map_or(format!("net {n}"), |x| x.name.clone())
    };
    let layer_name = |l: usize| si::layer_name(board, l);
    // Nets that own plane copper.
    let mut plane_nets: HashSet<u32> = HashSet::new();
    for p in board.geometry.polygons.iter().filter(|p| !p.teardrop) {
        if let (LayerRef::Copper(l), Some(net)) = (&p.layer, p.net) {
            if planes.is_plane(*l as usize) {
                plane_nets.insert(net);
            }
        }
    }
    let checked: HashSet<u32> = fast
        .keys()
        .copied()
        .chain(pair_nets.iter().copied())
        .filter(|n| !plane_nets.contains(n))
        .collect();
    // Highest frequency of interest: the fastest edge's knee (0.35 / tr),
    // at least 1 GHz.
    let fmax = fast
        .values()
        .map(|(f, _)| 0.35 / f.rise)
        .fold(1e9, f64::max)
        .min(6e9);
    let mut findings: Vec<Finding> = Vec::new();
    // 1. Reference plane gaps and splits.
    for v in si::return_path_hits(board, &planes) {
        let Some(net) = v.nets[0] else { continue };
        if !checked.contains(&net) {
            continue;
        }
        let split = v.rule.starts_with("crosses a split");
        findings.push(Finding {
            severity: if split || v.found > 4.0 * v.need {
                Severity::High
            } else {
                Severity::Medium
            },
            kind: if split { "split" } else { "plane gap" },
            net: net_name(net),
            layer: v.layer.map(|l| layer_name(l as usize)),
            at: v.at,
            found: v.found,
            need: v.need,
            text: if split {
                format!("track {}", v.rule)
            } else {
                format!("{:.0} mil of track {}", v.found, v.rule)
            },
            fix: if split {
                "route round the split, or bridge it with a stitching capacitor between the two plane nets right beside the crossing".into()
            } else {
                "keep the plane solid under the track (move the void or the track), or move the track to a layer over solid plane".into()
            },
        });
    }
    // Per copper layer, the plane layers next to it, nearest first.
    let refs_of = |l: usize| -> Vec<usize> {
        let (up, down) = hs.get(l).copied().unwrap_or((None, None));
        let mut r: Vec<(usize, f64)> = Vec::new();
        if l > 0 && planes.is_plane(l - 1) {
            r.push((l - 1, up.unwrap_or(f64::MAX)));
        }
        if l + 1 < ncu && planes.is_plane(l + 1) {
            r.push((l + 1, down.unwrap_or(f64::MAX)));
        }
        r.sort_by(|a, b| a.1.total_cmp(&b.1));
        r.into_iter().map(|x| x.0).collect()
    };
    // 2. Layer changes: the layers each fast net's tracks use at a via.
    let g = &board.geometry;
    let mut track_layers_at: HashMap<(u32, i64, i64), HashSet<usize>> = HashMap::new();
    let key = |p: P| ((p[0] * 2.0).round() as i64, (p[1] * 2.0).round() as i64);
    for t in &g.tracks {
        let (LayerRef::Copper(l), Some(net)) = (&t.layer, t.net) else {
            continue;
        };
        if !checked.contains(&net) {
            continue;
        }
        for p in [pt(t.start), pt(t.end)] {
            let k = key(p);
            track_layers_at
                .entry((net, k.0, k.1))
                .or_default()
                .insert(*l as usize);
        }
    }
    // Ground vias, and capacitors between plane nets, for the return.
    let ground_vias: Vec<(P, Option<u32>)> = g
        .vias
        .iter()
        .filter(|v| v.net.is_some_and(|n| plane_nets.contains(&n)))
        .map(|v| (pt(v.at), v.net))
        .collect();
    let caps: Vec<(P, HashSet<u32>)> = board
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.designator.to_ascii_uppercase().starts_with('C'))
        .map(|(ci, _)| {
            let pads: Vec<&crate::Pad> = g
                .pads
                .iter()
                .filter(|p| p.component == Some(ci as u32))
                .collect();
            let n = pads.len().max(1) as f64;
            let c = pads.iter().fold([0.0, 0.0], |s, p| {
                let q = pt(p.at);
                [s[0] + q[0] / n, s[1] + q[1] / n]
            });
            (c, pads.iter().filter_map(|p| p.net).collect())
        })
        .filter(|(_, nets): &(P, HashSet<u32>)| nets.len() == 2)
        .collect();
    let er_avg = geo.iter().map(|x| x.er).sum::<f64>() / geo.len().max(1) as f64;
    let er_avg = if er_avg > 1.0 { er_avg } else { 4.2 };
    let lambda_mil = |f: f64| C0 / (f * er_avg.sqrt()) / 25.4e-6;
    let via_need = (lambda_mil(fmax) / 40.0).clamp(30.0, 150.0);
    let mut seen_change: HashSet<(u32, usize, usize)> = HashSet::new();
    for v in &g.vias {
        let Some(net) = v.net else { continue };
        if !checked.contains(&net) {
            continue;
        }
        let at = pt(v.at);
        let k = key(at);
        let Some(layers) = track_layers_at.get(&(net, k.0, k.1)) else {
            continue;
        };
        let mut ls: Vec<usize> = layers.iter().copied().collect();
        ls.sort();
        if ls.len() < 2 {
            continue;
        }
        let (la, lb) = (ls[0], *ls.last().unwrap_or(&ls[0]));
        let (ra, rb) = (refs_of(la), refs_of(lb));
        let (Some(&pa), Some(&pb)) = (ra.first(), rb.first()) else {
            continue;
        };
        if pa == pb || ra.contains(&pb) || rb.contains(&pa) {
            continue;
        }
        let na = planes.at(pa, at).flatten();
        let nb = planes.at(pb, at).flatten();
        let (Some(na), Some(nb)) = (na, nb) else {
            continue;
        };
        if !seen_change.insert((net, pa.min(pb), pa.max(pb))) {
            // One finding per net and pair of planes: the worst via is
            // kept below.
        }
        if na == nb {
            let d = ground_vias
                .iter()
                .filter(|(_, n)| *n == Some(na))
                .map(|(p, _)| (p[0] - at[0]).hypot(p[1] - at[1]))
                .fold(f64::MAX, f64::min);
            if d > via_need {
                findings.push(Finding {
                    severity: if d > 3.0 * via_need {
                        Severity::High
                    } else {
                        Severity::Medium
                    },
                    kind: "layer change",
                    net: net_name(net),
                    layer: Some(format!("{} to {}", layer_name(la), layer_name(lb))),
                    at,
                    found: d,
                    need: via_need,
                    text: format!(
                        "via changes the reference from {} to {} ({}); the nearest {} via is {}",
                        layer_name(pa),
                        layer_name(pb),
                        net_name(na),
                        net_name(na),
                        if d == f64::MAX {
                            "nowhere".to_string()
                        } else {
                            format!("{d:.0} mil away")
                        }
                    ),
                    fix: format!(
                        "add a {} via within {:.0} mil of it",
                        net_name(na),
                        via_need
                    ),
                });
            }
        } else {
            let d = caps
                .iter()
                .filter(|(_, ns)| ns.contains(&na) && ns.contains(&nb))
                .map(|(p, _)| (p[0] - at[0]).hypot(p[1] - at[1]))
                .fold(f64::MAX, f64::min);
            let need = 4.0 * via_need;
            if d > need {
                findings.push(Finding {
                    severity: Severity::Medium,
                    kind: "layer change",
                    net: net_name(net),
                    layer: Some(format!("{} to {}", layer_name(la), layer_name(lb))),
                    at,
                    found: d,
                    need,
                    text: format!(
                        "via changes the reference from {} ({}) to {} ({}); the nearest capacitor between them is {}",
                        layer_name(pa),
                        net_name(na),
                        layer_name(pb),
                        net_name(nb),
                        if d == f64::MAX {
                            "nowhere".to_string()
                        } else {
                            format!("{d:.0} mil away")
                        }
                    ),
                    fix: format!(
                        "keep the net on layers that share a reference, or put a capacitor between {} and {} within {:.0} mil",
                        net_name(na),
                        net_name(nb),
                        need
                    ),
                });
            }
        }
    }
    // 3. Fast tracks near the board's edge.
    let outline: Vec<P> = g.outline.iter().map(|&p| pt(p)).collect();
    let edge_dist = |p: P| -> f64 {
        if outline.len() < 2 {
            return f64::MAX;
        }
        (0..outline.len())
            .map(|k| point_seg(p, outline[k], outline[(k + 1) % outline.len()]))
            .fold(f64::MAX, f64::min)
    };
    let mut near_edge: HashMap<u32, (f64, f64, P, usize)> = HashMap::new();
    for t in &g.tracks {
        let (LayerRef::Copper(l), Some(net)) = (&t.layer, t.net) else {
            continue;
        };
        if !checked.contains(&net) || t.component.is_some() {
            continue;
        }
        let l = *l as usize;
        let h = refs_of(l)
            .first()
            .map(|&r| {
                let (up, down) = hs.get(l).copied().unwrap_or((None, None));
                if r < l { up } else { down }.unwrap_or(5.0)
            })
            .unwrap_or(5.0);
        let need = (5.0 * h).max(25.0);
        let (a, b) = (pt(t.start), pt(t.end));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let steps = (len / 10.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let f = s as f64 / steps as f64;
            let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            let d = edge_dist(p) - t.width.mil() / 2.0;
            if d < need {
                let e = near_edge.entry(net).or_insert((f64::MAX, need, p, l));
                if d < e.0 {
                    *e = (d, need, p, l);
                }
            }
        }
    }
    for (net, (d, need, p, l)) in near_edge {
        findings.push(Finding {
            severity: if d < need / 2.0 {
                Severity::Medium
            } else {
                Severity::Low
            },
            kind: "board edge",
            net: net_name(net),
            layer: Some(layer_name(l)),
            at: p,
            found: d.max(0.0),
            need,
            text: format!(
                "track {:.0} mil from the board's edge (keep five times its height over the plane)",
                d.max(0.0)
            ),
            fix: format!("move it at least {need:.0} mil in from the edge"),
        });
    }
    // 4. Ground stitching along the edge.
    let mut ground: Vec<u32> = plane_nets
        .iter()
        .copied()
        .filter(|n| is_ground(&net_name(*n)))
        .collect();
    // The main ground (most vias) first.
    let vias_of = |n: u32| g.vias.iter().filter(|v| v.net == Some(n)).count();
    ground.sort_by_key(|&n| std::cmp::Reverse(vias_of(n)));
    let plane_layers = (0..ncu).filter(|&l| planes.is_plane(l)).count();
    if outline.len() >= 3 && !ground.is_empty() && plane_layers >= 2 {
        let band = 150.0;
        // Position along the outline of each ground via near the edge.
        let mut cum = vec![0.0];
        for k in 0..outline.len() {
            let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
            cum.push(cum[k] + (b[0] - a[0]).hypot(b[1] - a[1]));
        }
        let perim = *cum.last().unwrap_or(&0.0);
        let mut pos: Vec<f64> = Vec::new();
        for v in &g.vias {
            if !v.net.is_some_and(|n| ground.contains(&n)) {
                continue;
            }
            let p = pt(v.at);
            let mut best = (f64::MAX, 0.0);
            for k in 0..outline.len() {
                let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
                let d = point_seg(p, a, b);
                if d < best.0 {
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let l2 = (dx * dx + dy * dy).max(1e-9);
                    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                    best = (d, cum[k] + t * l2.sqrt());
                }
            }
            if best.0 <= band {
                pos.push(best.1);
            }
        }
        // Ground pads near the edge count too (mounting holes, connector
        // shells).
        for p in &g.pads {
            if p.net.is_some_and(|n| ground.contains(&n)) && edge_dist(pt(p.at)) <= band {
                let q = pt(p.at);
                let mut best = (f64::MAX, 0.0);
                for k in 0..outline.len() {
                    let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
                    let d = point_seg(q, a, b);
                    if d < best.0 {
                        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                        let l2 = (dx * dx + dy * dy).max(1e-9);
                        let t = (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                        best = (d, cum[k] + t * l2.sqrt());
                    }
                }
                pos.push(best.1);
            }
        }
        pos.sort_by(|a, b| a.total_cmp(b));
        let need = lambda_mil(fmax) / 20.0;
        let at_pos = |s: f64| -> P {
            let s = s.rem_euclid(perim.max(1e-9));
            let k = cum
                .partition_point(|&c| c <= s)
                .saturating_sub(1)
                .min(outline.len() - 1);
            let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
            let seg = (cum[k + 1] - cum[k]).max(1e-9);
            let t = (s - cum[k]) / seg;
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        if pos.is_empty() {
            findings.push(Finding {
                severity: Severity::Medium,
                kind: "edge stitching",
                net: net_name(ground[0]),
                layer: None,
                at: outline[0],
                found: perim,
                need,
                text: "no ground vias along the board's edge".into(),
                fix: format!(
                    "stitch the ground planes together along the edge, every {need:.0} mil or closer"
                ),
            });
        } else {
            let mut gaps: Vec<(f64, f64)> = pos
                .windows(2)
                .map(|w| (w[1] - w[0], (w[0] + w[1]) / 2.0))
                .collect();
            let (first, last) = (pos[0], *pos.last().unwrap_or(&pos[0]));
            gaps.push((perim - last + first, last + (perim - last + first) / 2.0));
            gaps.sort_by(|a, b| b.0.total_cmp(&a.0));
            let over: Vec<&(f64, f64)> = gaps.iter().filter(|g| g.0 > need).collect();
            for (gap, mid) in over.iter().take(5) {
                findings.push(Finding {
                    severity: if *gap > 3.0 * need {
                        Severity::Medium
                    } else {
                        Severity::Low
                    },
                    kind: "edge stitching",
                    net: net_name(ground[0]),
                    layer: None,
                    at: at_pos(*mid),
                    found: *gap,
                    need,
                    text: format!(
                        "{gap:.0} mil of board edge with no ground via (a twentieth of a wavelength at {:.2} GHz is {need:.0} mil)",
                        fmax / 1e9
                    ),
                    fix: format!("add ground vias along this edge every {need:.0} mil"),
                });
            }
            if over.len() > 5 {
                notes.push(format!(
                    "{} more stretches of edge without stitching",
                    over.len() - 5
                ));
            }
        }
    }
    // 5. Radiated emission of each fast net (not the pairs: their fields
    // cancel to first order).
    let mut emissions = Vec::new();
    let outer = |l: usize| l == 0 || l + 1 == ncu;
    for (&net, (f, assumed)) in &fast {
        if plane_nets.contains(&net) {
            continue;
        }
        let (mut out_len, mut in_len, mut area, mut zsum) = (0.0, 0.0, 0.0, 0.0);
        for t in &g.tracks {
            let (LayerRef::Copper(l), Some(n)) = (&t.layer, t.net) else {
                continue;
            };
            if n != net || t.component.is_some() {
                continue;
            }
            let l = *l as usize;
            let len = (pt(t.end)[0] - pt(t.start)[0]).hypot(pt(t.end)[1] - pt(t.start)[1]);
            let lg = geo.get(l);
            if outer(l) {
                let h = lg.and_then(|x| x.below.or(x.above)).unwrap_or(5.0).max(0.5);
                let er = lg.map_or(4.2, |x| x.er);
                let tk = lg.map_or(1.4, |x| x.t);
                out_len += len;
                area += len * h;
                zsum += len * crate::impedance::microstrip(t.width.mil(), tk, h, er);
            } else {
                in_len += len;
            }
        }
        if out_len + in_len <= 0.0 {
            continue;
        }
        let z0 = if out_len > 0.0 { zsum / out_len } else { 50.0 };
        let z0 = if z0.is_finite() && z0 > 5.0 { z0 } else { 50.0 };
        let area_m2 = area * (25.4e-6f64).powi(2);
        let amp = f.volts / (2.0 * z0);
        let mut spectrum = Vec::new();
        let mut worst: Option<(f64, f64, f64)> = None;
        let sinc = |x: f64| if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
        let mut n = 1usize;
        loop {
            let fh = n as f64 * f.freq;
            if fh > 6e9 || n > 2001 {
                break;
            }
            if let Some((lim, r)) = setup.limit.at(fh) {
                let i_n = amp
                    * sinc(n as f64 * PI / 2.0).abs()
                    * sinc(n as f64 * PI * f.freq * f.rise).abs();
                let e = 2.63e-14 * fh * fh * area_m2 * i_n / r;
                if e > 0.0 {
                    let db = 20.0 * (e / 1e-6).log10();
                    spectrum.push((fh, db, lim));
                    if worst.is_none_or(|w| lim - db < w.2 - w.1) {
                        worst = Some((fh, db, lim));
                    }
                }
            }
            n += 2;
        }
        emissions.push(Emission {
            net: net_name(net),
            freq: f.freq,
            rise: f.rise,
            volts: f.volts,
            assumed: *assumed,
            z0,
            outer: out_len,
            inner: in_len,
            area_mm2: area * 0.0254 * 0.0254,
            spectrum,
            worst,
        });
    }
    emissions.sort_by(|a, b| {
        a.margin()
            .unwrap_or(f64::MAX)
            .total_cmp(&b.margin().unwrap_or(f64::MAX))
    });
    for e in &emissions {
        if let (Some(m), Some((fh, db, lim))) = (e.margin(), e.worst) {
            if m < setup.margin_db {
                findings.push(Finding {
                    severity: if m < 0.0 { Severity::High } else { Severity::Medium },
                    kind: "emission",
                    net: e.net.clone(),
                    layer: None,
                    at: [0.0, 0.0],
                    found: db,
                    need: lim - setup.margin_db,
                    text: format!(
                        "radiates an estimated {db:.1} dBµV/m at {:.1} MHz against a limit of {lim:.1} ({:+.1} dB){}",
                        fh / 1e6,
                        m,
                        if e.assumed { "; its frequency was assumed" } else { "" }
                    ),
                    fix: format!(
                        "route its {:.0} mil on the outer layers inside, between planes, or slow its edge (now {:.2} ns)",
                        e.outer,
                        e.rise * 1e9
                    ),
                });
            }
        }
    }
    // Put the location of an emission finding on the net's first track.
    for f in findings.iter_mut().filter(|f| f.kind == "emission") {
        if let Some(t) = g.tracks.iter().find(|t| {
            t.net
                .and_then(|n| board.nets.get(n as usize))
                .is_some_and(|n| n.name == f.net)
        }) {
            f.at = pt(t.start);
        }
    }
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(b.found.total_cmp(&a.found))
    });
    if plane_layers == 0 {
        notes.push(
            "no copper layer is mostly plane: the return path and stitching checks need planes"
                .into(),
        );
    }
    if fast.values().any(|(_, a)| *a) {
        notes.push(format!(
            "clock nets found by name were taken at {:.0} MHz, {:.1} ns edges and {:.1} V; give --net NAME=FREQ@EDGE,VOLTS for real numbers",
            setup.auto_freq / 1e6,
            setup.auto_rise * 1e9,
            setup.auto_volts
        ));
    }
    notes.push(
        "the emission is the board's own (differential mode); cables on its connectors usually radiate more (common mode) and are not modelled"
            .into(),
    );
    EmiResult {
        findings,
        emissions,
        limit: Some(setup.limit),
        margin_db: setup.margin_db,
        fmax,
        fast_nets: checked.len(),
        notes,
    }
}

impl EmiResult {
    pub fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }

    pub fn report(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "EMI check: {} fast net(s) and pairs; {} high, {} medium, {} low; stitching held to {:.2} GHz",
            self.fast_nets,
            self.count(Severity::High),
            self.count(Severity::Medium),
            self.count(Severity::Low),
            self.fmax / 1e9
        );
        if !self.emissions.is_empty() {
            let _ = writeln!(
                o,
                "\nEstimated radiation against {} (wanting {:.0} dB of margin):",
                self.limit.map_or("", |l| l.name()),
                self.margin_db
            );
            let _ = writeln!(
                o,
                "  {:<22} {:>9} {:>7} {:>5} {:>7} {:>7} {:>8} {:>10} {:>8} {:>8}",
                "Net",
                "f MHz",
                "edge ns",
                "V",
                "outer",
                "inner",
                "loop mm²",
                "worst MHz",
                "dBµV/m",
                "margin"
            );
            for e in &self.emissions {
                let (wf, wd, m) = match e.worst {
                    Some((f, d, l)) => (
                        format!("{:.1}", f / 1e6),
                        format!("{d:.1}"),
                        format!("{:+.1}", l - d),
                    ),
                    None => ("-".into(), "-".into(), "-".into()),
                };
                let _ = writeln!(
                    o,
                    "  {:<22} {:>9.2} {:>7.2} {:>5.1} {:>7.0} {:>7.0} {:>8.1} {:>10} {:>8} {:>8}{}",
                    e.net,
                    e.freq / 1e6,
                    e.rise * 1e9,
                    e.volts,
                    e.outer,
                    e.inner,
                    e.area_mm2,
                    wf,
                    wd,
                    m,
                    if e.assumed { "  (assumed)" } else { "" }
                );
            }
        }
        if !self.findings.is_empty() {
            let _ = writeln!(o, "\nFindings, worst first:");
        }
        for f in &self.findings {
            let _ = writeln!(
                o,
                "  [{}] {} {}{}: {} at ({:.0}, {:.0}) mil\n      fix: {}",
                f.severity.label(),
                f.kind,
                f.net,
                f.layer
                    .as_ref()
                    .map_or(String::new(), |l| format!(" on {l}")),
                f.text,
                f.at[0],
                f.at[1],
                f.fix
            );
        }
        if self.findings.is_empty() {
            let _ = writeln!(o, "\nNo findings.");
        }
        for n in &self.notes {
            let _ = writeln!(o, "note: {n}");
        }
        o
    }

    pub fn csv(&self) -> String {
        let mut o = String::from("severity,kind,net,layer,x_mil,y_mil,found,need,what,fix\n");
        let q = |s: &str| format!("\"{}\"", s.replace('"', "'"));
        for f in &self.findings {
            let _ = writeln!(
                o,
                "{},{},{},{},{:.1},{:.1},{:.2},{:.2},{},{}",
                f.severity.label(),
                f.kind,
                q(&f.net),
                q(f.layer.as_deref().unwrap_or("")),
                f.at[0],
                f.at[1],
                f.found,
                f.need,
                q(&f.text),
                q(&f.fix)
            );
        }
        o
    }

    /// The fast nets' estimated spectra against the limit line.
    pub fn svg(&self, title: &str) -> String {
        let (w, h) = (920.0, 520.0);
        let (l, r, t, b) = (70.0, 180.0, 60.0, 460.0);
        let (f0, f1) = (30e6f64.log10(), 6e9f64.log10());
        let x = |f: f64| l + (w - l - r) * (f.log10() - f0) / (f1 - f0);
        let (d0, d1) = (-20.0, 80.0);
        let y = |d: f64| b - (b - t) * ((d - d0) / (d1 - d0)).clamp(0.0, 1.0);
        let mut o = String::new();
        let _ = writeln!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="Segoe UI, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#fff"/>"##
        );
        let _ = writeln!(
            o,
            r##"<text x="{l}" y="24" font-size="14" fill="#111">{}</text><text x="{l}" y="42" fill="#444">Estimated radiation (differential mode) against {}</text>"##,
            esc(title),
            self.limit.map_or("", |x| x.name())
        );
        for d in (-20..=80).step_by(10) {
            let yy = y(d as f64);
            let _ = writeln!(
                o,
                r##"<line x1="{l}" x2="{}" y1="{yy:.1}" y2="{yy:.1}" stroke="#eceff4"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{d}</text>"##,
                w - r,
                l - 6.0,
                yy + 4.0
            );
        }
        for f in [30e6, 100e6, 300e6, 1e9, 3e9, 6e9] {
            let xx = x(f);
            let _ = writeln!(
                o,
                r##"<line x1="{xx:.1}" x2="{xx:.1}" y1="{t}" y2="{b}" stroke="#eceff4"/><text x="{xx:.1}" y="{}" text-anchor="middle" fill="#555">{}</text>"##,
                b + 16.0,
                if f >= 1e9 {
                    format!("{} GHz", f / 1e9)
                } else {
                    format!("{} MHz", f / 1e6)
                }
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="18" y="{}" transform="rotate(-90 18 {})" text-anchor="middle" fill="#555">dBµV/m at the limit's distance</text>"##,
            (t + b) / 2.0,
            (t + b) / 2.0
        );
        // The limit line.
        if let Some(lim) = self.limit {
            let mut d = String::new();
            let mut f = 30e6;
            let mut first = true;
            while f <= 6e9 {
                if let Some((v, _)) = lim.at(f) {
                    let _ = write!(
                        d,
                        "{}{:.1},{:.1}",
                        if first { "M" } else { "L" },
                        x(f),
                        y(v)
                    );
                    first = false;
                }
                f *= 1.01;
            }
            let _ = writeln!(
                o,
                r##"<path d="{d}" fill="none" stroke="#d4380d" stroke-width="2"/>"##
            );
        }
        let colours = [
            "#1f5fbf", "#2f9e6e", "#8e44ad", "#c27c0e", "#16a2b8", "#5b6577",
        ];
        for (k, e) in self.emissions.iter().take(6).enumerate() {
            let c = colours[k % colours.len()];
            for (f, d, _) in &e.spectrum {
                let _ = writeln!(
                    o,
                    r##"<line x1="{:.1}" x2="{:.1}" y1="{b}" y2="{:.1}" stroke="{c}" stroke-opacity="0.75" stroke-width="1.2"/>"##,
                    x(*f),
                    x(*f),
                    y(*d)
                );
            }
            let _ = writeln!(
                o,
                r##"<rect x="{}" y="{}" width="10" height="10" fill="{c}"/><text x="{}" y="{}" fill="#222">{}</text>"##,
                w - r + 14.0,
                t + 18.0 * k as f64,
                w - r + 30.0,
                t + 18.0 * k as f64 + 9.0,
                esc(&e.net)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fast_nets() {
        let f = FastNet::parse("CLK*=25MHz@2ns,1.8V").unwrap();
        assert_eq!(f.pattern, "CLK*");
        assert!((f.freq - 25e6).abs() < 1.0);
        assert!((f.rise - 2e-9).abs() < 1e-15);
        assert!((f.volts - 1.8).abs() < 1e-9);
        let g = FastNet::parse("MCLK=12.288").unwrap();
        assert!((g.freq - 12.288e6).abs() < 1.0);
        assert!((g.volts - 3.3).abs() < 1e-9);
    }

    #[test]
    fn clock_names() {
        for n in [
            "CLK", "SYS_CLK", "SPI_SCK", "XTAL_IN", "DDR_CK", "REFCLK0", "I2S_BCLK",
        ] {
            assert!(is_clock(n), "{n}");
        }
        for n in ["DDR_D0", "USB_DP", "GND", "BLOCK_EN", "CKE"] {
            assert!(!is_clock(n), "{n}");
        }
    }

    #[test]
    fn ott_loop_example() {
        // Ott's worked example: a 10 cm² loop carrying 20 mA at 50 MHz
        // gives 219 µV/m (46.8 dBµV/m) at 3 m in free space (1.316e-14).
        let e = 1.316e-14 * 50e6f64.powi(2) * 10e-4 * 0.02 / 3.0;
        let db = 20.0 * (e / 1e-6).log10();
        assert!((db - 46.8).abs() < 0.5, "{db}");
        // Class B at 3 m, 100 MHz: 43.5 dBµV/m.
        assert_eq!(Limit::FccB.at(100e6), Some((43.5, 3.0)));
        assert_eq!(Limit::CisprB.at(2e9), Some((50.0, 3.0)));
        assert_eq!(Limit::FccB.at(10e6), None);
    }
}
