//! Design compare: what changed between two versions of a board.
//!
//! Parts are matched by designator, nets by name (a net whose pins are
//! all the same under a new name counts as renamed), rules by kind and
//! name, stack-up layers by position. Routing is compared net by net:
//! length on each layer and the vias.

use crate::{Board, LayerRef};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

/// What to compare and how closely.
#[derive(Clone, Debug)]
pub struct Options {
    /// A part counts as moved past this, mil.
    pub move_tol: f64,
    /// A net's routing counts as changed past this length, mil, and this
    /// share of its length.
    pub length_tol: f64,
    pub length_share: f64,
    /// Compare routing (tracks, arcs and vias).
    pub routing: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            move_tol: 1.0,
            length_tol: 10.0,
            length_share: 0.02,
            routing: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Added,
    Removed,
    Changed,
    Renamed,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Removed => "removed",
            Kind::Changed => "changed",
            Kind::Renamed => "renamed",
        }
    }
}

/// One difference.
#[derive(Clone, Debug)]
pub struct Change {
    /// "part", "net", "routing", "rule", "class", "pair", "stack-up" or
    /// "board".
    pub area: &'static str,
    pub kind: Kind,
    /// Designator, net, rule or layer name.
    pub what: String,
    pub detail: String,
    /// Where to look, mil (on the second board, or the first when it is
    /// gone there).
    pub at: Option<[f64; 2]>,
    /// Where it was on the first board, when that differs.
    pub was: Option<[f64; 2]>,
}

/// Everything that differs, in order: board, stack-up, parts, nets,
/// routing, classes and pairs, rules.
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub changes: Vec<Change>,
    /// Totals of each board: parts, nets, tracks, vias.
    pub totals: [[usize; 4]; 2],
}

impl Comparison {
    /// How many changes in an area.
    pub fn count(&self, area: &str) -> usize {
        self.changes.iter().filter(|c| c.area == area).count()
    }

    /// A text report.
    pub fn report(&self, a: &str, b: &str) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "Compare {a} (first) with {b} (second)");
        for (k, name) in [(0, a), (1, b)] {
            let t = self.totals[k];
            let _ = writeln!(
                o,
                "  {name}: {} parts, {} nets, {} tracks, {} vias",
                t[0], t[1], t[2], t[3]
            );
        }
        if self.changes.is_empty() {
            let _ = writeln!(o, "No differences.");
            return o;
        }
        let mut areas: Vec<&str> = Vec::new();
        for c in &self.changes {
            if !areas.contains(&c.area) {
                areas.push(c.area);
            }
        }
        for area in areas {
            let _ = writeln!(o, "\n{} ({}):", title(area), self.count(area));
            for c in self.changes.iter().filter(|c| c.area == area) {
                let _ = writeln!(o, "  {:<8} {:<24} {}", c.kind.word(), c.what, c.detail);
            }
        }
        o
    }

    /// One row per change.
    pub fn csv(&self) -> String {
        let mut o = String::from("area,change,what,detail,x_mil,y_mil\n");
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        for c in &self.changes {
            let (x, y) = c.at.map_or((String::new(), String::new()), |p| {
                (format!("{:.1}", p[0]), format!("{:.1}", p[1]))
            });
            let _ = writeln!(
                o,
                "{},{},{},{},{x},{y}",
                c.area,
                c.kind.word(),
                q(&c.what),
                q(&c.detail)
            );
        }
        o
    }
}

/// The heading for an area.
pub fn title(area: &str) -> &'static str {
    match area {
        "board" => "Board",
        "stack-up" => "Stack-up",
        "part" => "Parts",
        "net" => "Nets",
        "routing" => "Routing",
        "class" => "Net classes",
        "pair" => "Differential pairs",
        "rule" => "Rules",
        _ => "Other",
    }
}

fn mil(l: crate::Length) -> f64 {
    l.mil()
}

fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// Value and part number of a part, as one line.
fn part_value(c: &crate::Component) -> (Option<String>, Option<String>) {
    (
        c.param(&["Value", "Comment", "Val"]).map(str::to_string),
        c.param(&[
            "Part Number",
            "PartNumber",
            "MPN",
            "Manufacturer Part Number",
            "Manufacturer Part Number 1",
        ])
        .map(str::to_string),
    )
}

/// Pins of each net by name: "R1.2".
fn net_pins(b: &Board) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in &b.geometry.pads {
        let (Some(n), Some(c)) = (p.net, p.component) else {
            continue;
        };
        let (Some(net), Some(comp)) = (b.nets.get(n as usize), b.components.get(c as usize)) else {
            continue;
        };
        if p.name.is_empty() {
            continue;
        }
        out.entry(net.name.clone())
            .or_default()
            .insert(format!("{}.{}", comp.designator, p.name));
    }
    for n in &b.nets {
        out.entry(n.name.clone()).or_default();
    }
    out
}

/// Routing of each net: length per copper layer (mil), vias, and a spot.
#[derive(Default)]
struct Routed {
    layers: BTreeMap<u8, f64>,
    vias: usize,
    sum: [f64; 2],
    n: usize,
}

impl Routed {
    fn length(&self) -> f64 {
        self.layers.values().sum()
    }
    fn at(&self) -> Option<[f64; 2]> {
        (self.n > 0).then(|| [self.sum[0] / self.n as f64, self.sum[1] / self.n as f64])
    }
}

fn routing(b: &Board) -> HashMap<String, Routed> {
    let mut out: HashMap<String, Routed> = HashMap::new();
    let name = |n: u32| b.nets.get(n as usize).map(|x| x.name.clone());
    let g = &b.geometry;
    for t in &g.tracks {
        let (Some(n), LayerRef::Copper(l)) = (t.net, &t.layer) else {
            continue;
        };
        if t.text || t.component.is_some() {
            continue;
        }
        let Some(nm) = name(n) else { continue };
        let r = out.entry(nm).or_default();
        let len = (t.end.x.mil() - t.start.x.mil()).hypot(t.end.y.mil() - t.start.y.mil());
        *r.layers.entry(*l).or_insert(0.0) += len;
        r.sum[0] += (t.start.x.mil() + t.end.x.mil()) / 2.0;
        r.sum[1] += (t.start.y.mil() + t.end.y.mil()) / 2.0;
        r.n += 1;
    }
    for a in &g.arcs {
        let (Some(n), LayerRef::Copper(l)) = (a.net, &a.layer) else {
            continue;
        };
        if a.component.is_some() {
            continue;
        }
        let Some(nm) = name(n) else { continue };
        let r = out.entry(nm).or_default();
        let sweep = (a.end_deg - a.start_deg).abs().to_radians();
        *r.layers.entry(*l).or_insert(0.0) += a.radius.mil() * sweep;
        r.sum[0] += a.center.x.mil();
        r.sum[1] += a.center.y.mil();
        r.n += 1;
    }
    for v in &g.vias {
        let Some(nm) = v.net.and_then(name) else {
            continue;
        };
        let r = out.entry(nm).or_default();
        r.vias += 1;
        r.sum[0] += v.at.x.mil();
        r.sum[1] += v.at.y.mil();
        r.n += 1;
    }
    out
}

fn layer_names(b: &Board) -> Vec<String> {
    b.stackup.copper_layers().map(|l| l.name.clone()).collect()
}

fn bounds(b: &Board) -> Option<[f64; 4]> {
    let o = &b.geometry.outline;
    if o.is_empty() {
        return None;
    }
    let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in o {
        r[0] = r[0].min(p.x.mil());
        r[1] = r[1].min(p.y.mil());
        r[2] = r[2].max(p.x.mil());
        r[3] = r[3].max(p.y.mil());
    }
    Some(r)
}

/// Compares board `a` (the first, older) with `b` (the second, newer).
pub fn compare(a: &Board, b: &Board, opt: &Options) -> Comparison {
    let mut ch: Vec<Change> = Vec::new();
    let totals = |x: &Board| {
        [
            x.components.len(),
            x.nets.len(),
            x.geometry
                .tracks
                .iter()
                .filter(|t| !t.text && t.component.is_none() && t.net.is_some())
                .count(),
            x.geometry.vias.len(),
        ]
    };
    // Board outline.
    match (bounds(a), bounds(b)) {
        (Some(p), Some(q)) => {
            let (wa, ha, wb, hb) = (p[2] - p[0], p[3] - p[1], q[2] - q[0], q[3] - q[1]);
            if (wa - wb).abs() > opt.move_tol || (ha - hb).abs() > opt.move_tol {
                ch.push(Change {
                    area: "board",
                    kind: Kind::Changed,
                    what: "outline".into(),
                    detail: format!(
                        "{} x {} mil -> {} x {} mil",
                        num(wa),
                        num(ha),
                        num(wb),
                        num(hb)
                    ),
                    at: Some([(q[0] + q[2]) / 2.0, (q[1] + q[3]) / 2.0]),
                    was: None,
                });
            } else if a.geometry.outline.len() != b.geometry.outline.len() {
                ch.push(Change {
                    area: "board",
                    kind: Kind::Changed,
                    what: "outline".into(),
                    detail: format!(
                        "same size, {} -> {} corners",
                        a.geometry.outline.len(),
                        b.geometry.outline.len()
                    ),
                    at: Some([(q[0] + q[2]) / 2.0, (q[1] + q[3]) / 2.0]),
                    was: None,
                });
            }
        }
        (None, Some(_)) => ch.push(Change {
            area: "board",
            kind: Kind::Added,
            what: "outline".into(),
            detail: String::new(),
            at: None,
            was: None,
        }),
        (Some(_), None) => ch.push(Change {
            area: "board",
            kind: Kind::Removed,
            what: "outline".into(),
            detail: String::new(),
            at: None,
            was: None,
        }),
        (None, None) => {}
    }
    // Stack-up.
    let (la, lb) = (&a.stackup.layers, &b.stackup.layers);
    if layer_names(a).len() != layer_names(b).len() {
        ch.push(Change {
            area: "stack-up",
            kind: Kind::Changed,
            what: "copper layers".into(),
            detail: format!("{} -> {}", layer_names(a).len(), layer_names(b).len()),
            at: None,
            was: None,
        });
    }
    for i in 0..la.len().max(lb.len()) {
        match (la.get(i), lb.get(i)) {
            (Some(x), Some(y)) => {
                let mut d = Vec::new();
                if x.name != y.name {
                    d.push(format!("name {} -> {}", x.name, y.name));
                }
                if x.kind != y.kind {
                    d.push(format!("{:?} -> {:?}", x.kind, y.kind));
                }
                if (mil(x.thickness) - mil(y.thickness)).abs() > 0.01 {
                    d.push(format!(
                        "thickness {} -> {} mil",
                        num(mil(x.thickness)),
                        num(mil(y.thickness))
                    ));
                }
                let opt_num = |v: Option<f64>| v.map_or("none".to_string(), num);
                if x.dielectric_constant.map(|v| (v * 100.0).round())
                    != y.dielectric_constant.map(|v| (v * 100.0).round())
                {
                    d.push(format!(
                        "Er {} -> {}",
                        opt_num(x.dielectric_constant),
                        opt_num(y.dielectric_constant)
                    ));
                }
                if x.loss_tangent.map(|v| (v * 1e4).round())
                    != y.loss_tangent.map(|v| (v * 1e4).round())
                {
                    d.push(format!(
                        "Df {} -> {}",
                        x.loss_tangent.map_or("none".into(), |v| format!("{v:.4}")),
                        y.loss_tangent.map_or("none".into(), |v| format!("{v:.4}"))
                    ));
                }
                if x.material != y.material {
                    d.push(format!(
                        "material {} -> {}",
                        x.material.as_deref().unwrap_or("none"),
                        y.material.as_deref().unwrap_or("none")
                    ));
                }
                if !d.is_empty() {
                    ch.push(Change {
                        area: "stack-up",
                        kind: Kind::Changed,
                        what: format!("{} ({})", y.name, i + 1),
                        detail: d.join("; "),
                        at: None,
                        was: None,
                    });
                }
            }
            (None, Some(y)) => ch.push(Change {
                area: "stack-up",
                kind: Kind::Added,
                what: format!("{} ({})", y.name, i + 1),
                detail: format!("{:?}, {} mil", y.kind, num(mil(y.thickness))),
                at: None,
                was: None,
            }),
            (Some(x), None) => ch.push(Change {
                area: "stack-up",
                kind: Kind::Removed,
                what: format!("{} ({})", x.name, i + 1),
                detail: format!("{:?}, {} mil", x.kind, num(mil(x.thickness))),
                at: None,
                was: None,
            }),
            (None, None) => {}
        }
    }
    // Parts.
    let pa: BTreeMap<&str, &crate::Component> = a
        .components
        .iter()
        .map(|c| (c.designator.as_str(), c))
        .collect();
    let pb: BTreeMap<&str, &crate::Component> = b
        .components
        .iter()
        .map(|c| (c.designator.as_str(), c))
        .collect();
    let spot = |c: &crate::Component| Some([c.x.mil(), c.y.mil()]);
    for (d, x) in &pa {
        let Some(y) = pb.get(d) else {
            let (v, _) = part_value(x);
            ch.push(Change {
                area: "part",
                kind: Kind::Removed,
                what: d.to_string(),
                detail: format!(
                    "{}{}",
                    x.footprint,
                    v.map_or(String::new(), |v| format!(", {v}"))
                ),
                at: spot(x),
                was: None,
            });
            continue;
        };
        let mut det = Vec::new();
        if x.footprint != y.footprint {
            det.push(format!("footprint {} -> {}", x.footprint, y.footprint));
        }
        let ((va, na), (vb, nb)) = (part_value(x), part_value(y));
        if va != vb {
            det.push(format!(
                "value {} -> {}",
                va.as_deref().unwrap_or("none"),
                vb.as_deref().unwrap_or("none")
            ));
        }
        if na != nb {
            det.push(format!(
                "part number {} -> {}",
                na.as_deref().unwrap_or("none"),
                nb.as_deref().unwrap_or("none")
            ));
        }
        if x.side != y.side {
            det.push(format!("side {:?} -> {:?}", x.side, y.side));
        }
        let dx = y.x.mil() - x.x.mil();
        let dy = y.y.mil() - x.y.mil();
        if dx.hypot(dy) > opt.move_tol {
            det.push(format!(
                "moved {} mil (x {:+.1}, y {:+.1})",
                num(dx.hypot(dy)),
                dx,
                dy
            ));
        }
        let rot = (y.rotation_deg - x.rotation_deg).rem_euclid(360.0);
        if rot > 0.05 && rot < 359.95 {
            det.push(format!(
                "rotated {} -> {} deg",
                num(x.rotation_deg),
                num(y.rotation_deg)
            ));
        }
        if !det.is_empty() {
            ch.push(Change {
                area: "part",
                kind: Kind::Changed,
                what: d.to_string(),
                detail: det.join("; "),
                at: spot(y),
                was: spot(x),
            });
        }
    }
    for (d, y) in &pb {
        if !pa.contains_key(d) {
            let (v, _) = part_value(y);
            ch.push(Change {
                area: "part",
                kind: Kind::Added,
                what: d.to_string(),
                detail: format!(
                    "{}{}",
                    y.footprint,
                    v.map_or(String::new(), |v| format!(", {v}"))
                ),
                at: spot(y),
                was: None,
            });
        }
    }
    // Nets: renamed (the same pins under a new name), added, removed, and
    // pins joined or left.
    let (na, nb) = (net_pins(a), net_pins(b));
    let pin_at = |bd: &Board, pin: &str| -> Option<[f64; 2]> {
        let (d, p) = pin.rsplit_once('.')?;
        let ci = bd.components.iter().position(|c| c.designator == d)? as u32;
        bd.geometry
            .pads
            .iter()
            .find(|x| x.component == Some(ci) && x.name == p)
            .map(|x| [x.at.x.mil(), x.at.y.mil()])
    };
    let net_at = |bd: &Board, pins: &BTreeSet<String>| pins.iter().find_map(|p| pin_at(bd, p));
    let gone: Vec<&String> = na.keys().filter(|k| !nb.contains_key(*k)).collect();
    let new: Vec<&String> = nb.keys().filter(|k| !na.contains_key(*k)).collect();
    let mut renamed: HashMap<String, String> = HashMap::new();
    for g in &gone {
        let pins = &na[*g];
        if pins.is_empty() {
            continue;
        }
        if let Some(n) = new
            .iter()
            .find(|n| nb[**n] == *pins && !renamed.values().any(|v| v == **n))
        {
            renamed.insert((*g).clone(), (*n).clone());
        }
    }
    let list = |s: &BTreeSet<String>| -> String {
        let v: Vec<&String> = s.iter().collect();
        if v.len() > 8 {
            format!(
                "{} and {} more",
                v[..8]
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                v.len() - 8
            )
        } else {
            v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        }
    };
    for g in &gone {
        if let Some(n) = renamed.get(*g) {
            ch.push(Change {
                area: "net",
                kind: Kind::Renamed,
                what: (*g).clone(),
                detail: format!("-> {n} (same {} pins)", na[*g].len()),
                at: net_at(b, &nb[n]),
                was: None,
            });
        } else {
            ch.push(Change {
                area: "net",
                kind: Kind::Removed,
                what: (*g).clone(),
                detail: if na[*g].is_empty() {
                    "no pins".into()
                } else {
                    format!("pins {}", list(&na[*g]))
                },
                at: net_at(a, &na[*g]),
                was: None,
            });
        }
    }
    for n in &new {
        if renamed.values().any(|v| v == *n) {
            continue;
        }
        ch.push(Change {
            area: "net",
            kind: Kind::Added,
            what: (*n).clone(),
            detail: if nb[*n].is_empty() {
                "no pins".into()
            } else {
                format!("pins {}", list(&nb[*n]))
            },
            at: net_at(b, &nb[*n]),
            was: None,
        });
    }
    for (name, pins) in &na {
        let Some(other) = nb.get(name) else { continue };
        if pins == other {
            continue;
        }
        let joined: BTreeSet<String> = other.difference(pins).cloned().collect();
        let left: BTreeSet<String> = pins.difference(other).cloned().collect();
        let mut d = Vec::new();
        if !joined.is_empty() {
            d.push(format!("now also {}", list(&joined)));
        }
        if !left.is_empty() {
            d.push(format!("no longer {}", list(&left)));
        }
        ch.push(Change {
            area: "net",
            kind: Kind::Changed,
            what: name.clone(),
            detail: d.join("; "),
            at: net_at(b, &joined).or_else(|| net_at(b, other)),
            was: None,
        });
    }
    // Routing, net by net (renamed nets compare under their new name).
    if opt.routing {
        let (ra, rb) = (routing(a), routing(b));
        let (lna, lnb) = (layer_names(a), layer_names(b));
        let lname = |names: &[String], l: u8| {
            names
                .get(l as usize)
                .cloned()
                .unwrap_or_else(|| format!("L{}", l + 1))
        };
        let mut names: BTreeSet<String> = ra.keys().cloned().collect();
        names.extend(rb.keys().map(|k| {
            renamed
                .iter()
                .find(|(_, v)| *v == k)
                .map_or(k.clone(), |(old, _)| old.clone())
        }));
        for name in names {
            let new_name = renamed.get(&name).cloned().unwrap_or_else(|| name.clone());
            let empty = Routed::default();
            let x = ra.get(&name).unwrap_or(&empty);
            let y = rb.get(&new_name).unwrap_or(&empty);
            let (lx, ly) = (x.length(), y.length());
            let mut d = Vec::new();
            let tol = opt.length_tol.max(opt.length_share * lx.max(ly));
            if (lx - ly).abs() > tol {
                d.push(format!(
                    "length {:.0} -> {:.0} mil ({:+.0})",
                    lx,
                    ly,
                    ly - lx
                ));
            }
            if x.vias != y.vias {
                d.push(format!("vias {} -> {}", x.vias, y.vias));
            }
            let sa: Vec<String> = x.layers.keys().map(|l| lname(&lna, *l)).collect();
            let sb: Vec<String> = y.layers.keys().map(|l| lname(&lnb, *l)).collect();
            if sa != sb {
                d.push(format!(
                    "layers {} -> {}",
                    if sa.is_empty() {
                        "none".into()
                    } else {
                        sa.join(",")
                    },
                    if sb.is_empty() {
                        "none".into()
                    } else {
                        sb.join(",")
                    }
                ));
            }
            if d.is_empty() {
                continue;
            }
            let kind = if lx == 0.0 && x.vias == 0 {
                Kind::Added
            } else if ly == 0.0 && y.vias == 0 {
                Kind::Removed
            } else {
                Kind::Changed
            };
            ch.push(Change {
                area: "routing",
                kind,
                what: new_name,
                detail: d.join("; "),
                at: y.at().or_else(|| x.at()),
                was: x.at(),
            });
        }
    }
    // Net classes and pairs.
    let classes = |x: &Board| -> BTreeMap<String, BTreeSet<String>> {
        x.net_classes
            .iter()
            .map(|c| (c.name.clone(), c.members.iter().cloned().collect()))
            .collect()
    };
    let (ca, cb) = (classes(a), classes(b));
    for (n, m) in &ca {
        match cb.get(n) {
            None => ch.push(Change {
                area: "class",
                kind: Kind::Removed,
                what: n.clone(),
                detail: format!("{} nets", m.len()),
                at: None,
                was: None,
            }),
            Some(o) if o != m => {
                let add: BTreeSet<String> = o.difference(m).cloned().collect();
                let rem: BTreeSet<String> = m.difference(o).cloned().collect();
                let mut d = Vec::new();
                if !add.is_empty() {
                    d.push(format!("now also {}", list(&add)));
                }
                if !rem.is_empty() {
                    d.push(format!("no longer {}", list(&rem)));
                }
                ch.push(Change {
                    area: "class",
                    kind: Kind::Changed,
                    what: n.clone(),
                    detail: d.join("; "),
                    at: None,
                    was: None,
                });
            }
            _ => {}
        }
    }
    for (n, m) in &cb {
        if !ca.contains_key(n) {
            ch.push(Change {
                area: "class",
                kind: Kind::Added,
                what: n.clone(),
                detail: format!("{} nets", m.len()),
                at: None,
                was: None,
            });
        }
    }
    let pairs = |x: &Board| -> BTreeMap<String, (String, String)> {
        x.diff_pairs
            .iter()
            .map(|p| (p.name.clone(), (p.positive.clone(), p.negative.clone())))
            .collect()
    };
    let (pa2, pb2) = (pairs(a), pairs(b));
    for (n, v) in &pa2 {
        match pb2.get(n) {
            None => ch.push(Change {
                area: "pair",
                kind: Kind::Removed,
                what: n.clone(),
                detail: format!("{} / {}", v.0, v.1),
                at: None,
                was: None,
            }),
            Some(o) if o != v => ch.push(Change {
                area: "pair",
                kind: Kind::Changed,
                what: n.clone(),
                detail: format!("{} / {} -> {} / {}", v.0, v.1, o.0, o.1),
                at: None,
                was: None,
            }),
            _ => {}
        }
    }
    for (n, v) in &pb2 {
        if !pa2.contains_key(n) {
            ch.push(Change {
                area: "pair",
                kind: Kind::Added,
                what: n.clone(),
                detail: format!("{} / {}", v.0, v.1),
                at: None,
                was: None,
            });
        }
    }
    // Rules, by kind and name.
    fn rules(x: &Board) -> BTreeMap<(String, String), &crate::Rule> {
        x.rules
            .iter()
            .map(|r| ((r.kind.clone(), r.name.clone()), r))
            .collect()
    }
    let (ra, rb) = (rules(a), rules(b));
    for (k, x) in &ra {
        let what = format!("{} ({})", k.1, k.0);
        let Some(y) = rb.get(k) else {
            ch.push(Change {
                area: "rule",
                kind: Kind::Removed,
                what,
                detail: String::new(),
                at: None,
                was: None,
            });
            continue;
        };
        let mut d = Vec::new();
        if x.enabled != y.enabled {
            d.push(if y.enabled {
                "turned on".into()
            } else {
                "turned off".to_string()
            });
        }
        if x.priority != y.priority {
            d.push(format!("priority {} -> {}", x.priority, y.priority));
        }
        if x.scope != y.scope {
            d.push(format!(
                "scope {} -> {}",
                x.scope.join(" "),
                y.scope.join(" ")
            ));
        }
        let keys: BTreeSet<&String> = x.params.keys().chain(y.params.keys()).collect();
        for key in keys {
            let (u, v) = (x.params.get(key), y.params.get(key));
            if u != v {
                d.push(format!(
                    "{key} {} -> {}",
                    u.map_or("none", |s| s.as_str()),
                    v.map_or("none", |s| s.as_str())
                ));
            }
        }
        if !d.is_empty() {
            ch.push(Change {
                area: "rule",
                kind: Kind::Changed,
                what,
                detail: d.join("; "),
                at: None,
                was: None,
            });
        }
    }
    for k in rb.keys() {
        if !ra.contains_key(k) {
            ch.push(Change {
                area: "rule",
                kind: Kind::Added,
                what: format!("{} ({})", k.1, k.0),
                detail: String::new(),
                at: None,
                was: None,
            });
        }
    }
    Comparison {
        changes: ch,
        totals: [totals(a), totals(b)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, Geometry, Length, Net, Pad, PadShape, Point, Side, Track};

    fn part(d: &str, x: f64, value: &str) -> Component {
        Component {
            designator: d.into(),
            footprint: "R0402".into(),
            library_ref: None,
            x: Length::from_mil(x),
            y: Length::from_mil(0.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: [("Value".to_string(), value.to_string())]
                .into_iter()
                .collect(),
        }
    }

    fn pad(c: u32, name: &str, net: u32, x: f64) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(c),
            name: name.into(),
            at: Point::mil(x, 0.0),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length::from_mil(0.0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    fn board(nets: &[&str], parts: Vec<Component>, pads: Vec<Pad>, track_len: f64) -> Board {
        Board {
            nets: nets
                .iter()
                .map(|n| Net {
                    name: n.to_string(),
                    ..Default::default()
                })
                .collect(),
            components: parts,
            geometry: Geometry {
                pads,
                tracks: vec![Track {
                    layer: LayerRef::Copper(0),
                    net: Some(0),
                    start: Point::mil(0.0, 0.0),
                    end: Point::mil(track_len, 0.0),
                    width: Length::from_mil(5.0),
                    component: None,
                    text: false,
                }],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn finds_moved_changed_added_renamed_and_rerouted() {
        let a = board(
            &["SIG", "OLD"],
            vec![part("R1", 0.0, "10k"), part("R2", 100.0, "1k")],
            vec![
                pad(0, "1", 0, 0.0),
                pad(1, "1", 0, 100.0),
                pad(1, "2", 1, 120.0),
            ],
            100.0,
        );
        let b = board(
            &["SIG", "NEW"],
            vec![
                part("R1", 50.0, "4k7"),
                part("R2", 100.0, "1k"),
                part("C1", 200.0, "100n"),
            ],
            vec![
                pad(0, "1", 0, 50.0),
                pad(1, "1", 0, 100.0),
                pad(1, "2", 1, 120.0),
                pad(2, "1", 0, 200.0),
            ],
            300.0,
        );
        let c = compare(&a, &b, &Options::default());
        let find = |area: &str, what: &str| {
            c.changes
                .iter()
                .find(|x| x.area == area && x.what == what)
                .unwrap_or_else(|| panic!("{area} {what}: {}", c.report("a", "b")))
        };
        let r1 = find("part", "R1");
        assert!(
            r1.detail.contains("value 10k -> 4k7") && r1.detail.contains("moved 50"),
            "{}",
            r1.detail
        );
        assert_eq!(find("part", "C1").kind, Kind::Added);
        assert_eq!(find("net", "OLD").kind, Kind::Renamed);
        assert!(find("net", "SIG").detail.contains("now also C1.1"));
        assert!(find("routing", "SIG").detail.contains("length 100 -> 300"));
        assert!(
            c.changes.iter().all(|x| x.what != "R2"),
            "{}",
            c.report("a", "b")
        );
        // The same board twice: nothing.
        assert!(compare(&a, &a, &Options::default()).changes.is_empty());
    }
}
