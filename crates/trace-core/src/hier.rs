//! Hierarchical schematics: which sheet a sheet symbol places, and how
//! the sheets' nets join into the project's nets.
//!
//! The ways of joining are Altium's net identifier scopes (Allegro's
//! OrCAD Capture joins the same way: ports to the hierarchical pins of
//! their block, global names everywhere):
//!
//! - Flat: ports join every port of the same name; net labels stay on
//!   their sheet.
//! - Hierarchical: a port joins the sheet entry of the same name on the
//!   sheet symbol that places its sheet; net labels stay on their sheet.
//! - Strict hierarchical: as hierarchical, and power ports too stay on
//!   their sheet.
//! - Global: net labels and ports join everywhere, and ports join their
//!   sheet entries.
//! - Automatic picks one from what the sheets hold: hierarchical when
//!   there are sheet entries, flat when there are ports, global otherwise.
//!
//! Power ports join everywhere except in strict hierarchical, and a part
//! drawn on two sheets (a multi-part component) joins the nets of its pins.

use crate::annotate::unannotated;
use crate::schematic::{Drawing, Pt, TagKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    #[default]
    Automatic,
    Flat,
    Hierarchical,
    Strict,
    Global,
}

impl Scope {
    pub const ALL: [Scope; 5] = [
        Scope::Automatic,
        Scope::Flat,
        Scope::Hierarchical,
        Scope::Strict,
        Scope::Global,
    ];

    /// Altium's `HierarchyMode` in a `.PrjPcb`. Strict hierarchical came
    /// later and was numbered after Global: LimeSDR-USB (mode 3, sheets
    /// joined by net labels alone) gives the board's 627 nets only when
    /// 3 is Global.
    pub fn from_altium(n: i64) -> Scope {
        match n {
            1 => Scope::Flat,
            2 => Scope::Hierarchical,
            3 => Scope::Global,
            4 => Scope::Strict,
            _ => Scope::Automatic,
        }
    }

    /// The `HierarchyMode` Altium writes for it.
    pub fn altium_number(self) -> i64 {
        match self {
            Scope::Automatic => 0,
            Scope::Flat => 1,
            Scope::Hierarchical => 2,
            Scope::Global => 3,
            Scope::Strict => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Automatic => "Automatic",
            Scope::Flat => "Flat (ports join everywhere)",
            Scope::Hierarchical => "Hierarchical (ports join sheet entries)",
            Scope::Strict => "Strict hierarchical (power ports local too)",
            Scope::Global => "Global (net labels and ports join everywhere)",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Scope::Automatic,
            "flat" => Scope::Flat,
            "hier" | "hierarchical" => Scope::Hierarchical,
            "strict" => Scope::Strict,
            "global" => Scope::Global,
            _ => return None,
        })
    }

    /// The scope Automatic stands for on these sheets.
    pub fn resolve(self, sheets: &[&Drawing]) -> Scope {
        if self != Scope::Automatic {
            return self;
        }
        let has = |k: TagKind| sheets.iter().any(|d| d.tags.iter().any(|t| t.kind == k));
        if has(TagKind::SheetEntry) {
            Scope::Hierarchical
        } else if has(TagKind::Port) {
            Scope::Flat
        } else {
            Scope::Global
        }
    }

    fn ports_to_entries(self) -> bool {
        matches!(self, Scope::Hierarchical | Scope::Strict | Scope::Global)
    }
}

const TOL: f64 = 1.0;

/// The file name a sheet symbol names, as the sheet's name: no folder, no
/// extension, upper case.
pub fn sheet_key(file: &str) -> String {
    let base = file.rsplit(['\\', '/']).next().unwrap_or(file).trim();
    let stem = match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    };
    stem.to_ascii_uppercase()
}

/// How the sheets of a project nest.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// For each sheet, the sheet and sheet symbol (`Drawing::links`) that
    /// place it first, if any does.
    pub parent: Vec<Option<(usize, usize)>>,
    /// For each sheet, the sheet each of its sheet symbols places, when
    /// the project has it.
    pub child: Vec<Vec<Option<usize>>>,
    /// For each sheet, the sheet symbol each tag is a sheet entry of.
    pub entry_of: Vec<Vec<Option<usize>>>,
    /// Sheets no sheet symbol places, in project order.
    pub roots: Vec<usize>,
    /// How many sheets up the top is (0 for a top sheet).
    pub depth: Vec<usize>,
}

impl Tree {
    /// Sheets in the order a tree list shows them, with their depth.
    pub fn order(&self) -> Vec<(usize, usize)> {
        let n = self.parent.len();
        let mut seen = vec![false; n];
        let mut out = Vec::new();
        fn walk(t: &Tree, s: usize, d: usize, seen: &mut [bool], out: &mut Vec<(usize, usize)>) {
            if seen[s] {
                return;
            }
            seen[s] = true;
            out.push((s, d));
            for c in t.child[s].iter().flatten() {
                if t.parent[*c].map(|p| p.0) == Some(s) {
                    walk(t, *c, d + 1, seen, out);
                }
            }
        }
        for &r in &self.roots {
            walk(self, r, 0, &mut seen, &mut out);
        }
        // Sheets only reachable through a loop of sheet symbols.
        for s in 0..n {
            walk(self, s, 0, &mut seen, &mut out);
        }
        out
    }
}

fn on_box(p: Pt, min: Pt, max: Pt) -> bool {
    p[0] >= min[0] - TOL && p[0] <= max[0] + TOL && p[1] >= min[1] - TOL && p[1] <= max[1] + TOL
}

/// Builds the sheet tree; `names` are the sheets' file names (or stems).
pub fn tree(names: &[&str], sheets: &[&Drawing]) -> Tree {
    let n = sheets.len();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (i, name) in names.iter().enumerate().take(n) {
        by_key.entry(sheet_key(name)).or_insert(i);
    }
    let mut t = Tree {
        parent: vec![None; n],
        child: Vec::with_capacity(n),
        entry_of: Vec::with_capacity(n),
        roots: Vec::new(),
        depth: vec![0; n],
    };
    for (s, d) in sheets.iter().enumerate() {
        let kids: Vec<Option<usize>> = d
            .links
            .iter()
            .map(|l| by_key.get(&sheet_key(&l.file)).copied().filter(|&c| c != s))
            .collect();
        for (li, c) in kids.iter().enumerate() {
            if let Some(c) = *c {
                if t.parent[c].is_none() {
                    t.parent[c] = Some((s, li));
                }
            }
        }
        t.child.push(kids);
        // An entry sits on the edge of its sheet symbol; the smallest box
        // that holds it wins where boxes touch.
        t.entry_of.push(
            d.tags
                .iter()
                .map(|tag| {
                    if tag.kind != TagKind::SheetEntry {
                        return None;
                    }
                    let p = tag.at.first().copied()?;
                    d.links
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| on_box(p, l.min, l.max))
                        .min_by(|a, b| {
                            let area = |l: &crate::schematic::SheetLink| {
                                (l.max[0] - l.min[0]) * (l.max[1] - l.min[1])
                            };
                            area(a.1).total_cmp(&area(b.1))
                        })
                        .map(|(i, _)| i)
                })
                .collect(),
        );
    }
    t.roots = (0..n).filter(|&s| t.parent[s].is_none()).collect();
    for (s, d) in t.order() {
        t.depth[s] = d;
    }
    t
}

/// One placement of a sheet: the top sheet, or a sheet symbol's channel.
#[derive(Clone, Debug, Default)]
pub struct Instance {
    pub sheet: usize,
    /// The instance and sheet symbol that place it.
    pub parent: Option<(usize, usize)>,
    /// Added to its parts' designators (R1 becomes R1_CH2) when its sheet
    /// is placed more than once: the channel names from the top down.
    pub suffix: String,
}

impl Instance {
    /// A part's designator in this instance, as on the board.
    pub fn designator(&self, d: &str) -> String {
        if self.suffix.is_empty() {
            d.to_string()
        } else {
            format!("{d}_{}", self.suffix)
        }
    }
}

/// The sheets' nets joined into the project's.
#[derive(Clone, Debug, Default)]
pub struct Joined {
    /// Every placement of every sheet; a sheet placed once has one.
    pub instances: Vec<Instance>,
    /// The project net of each instance's nets: `of[instance][net]`.
    pub of: Vec<Vec<usize>>,
    /// How many project nets there are.
    pub count: usize,
    /// The scope used (never Automatic once the sheets are known).
    pub scope: Scope,
    pub tree: Tree,
    /// Sheet entries with no port of their name on the sheet they place:
    /// (sheet, tag).
    pub lone_entries: Vec<(usize, usize)>,
    /// Ports with no sheet entry of their name on the sheet symbol that
    /// places their sheet, on sheets that some sheet symbol places.
    pub lone_ports: Vec<(usize, usize)>,
    /// Sheet symbols naming a sheet the project does not have: (sheet, link).
    pub missing: Vec<(usize, usize)>,
}

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

/// Sheet nesting deeper than this is taken for a loop.
const MAX_DEPTH: usize = 32;

/// Every placement of the sheets, from the top sheets down.
fn instances(sheets: &[&Drawing], tree: &Tree) -> Vec<Instance> {
    // How many channels place each sheet, over every sheet symbol.
    let mut placed = vec![0usize; sheets.len()];
    for (s, d) in sheets.iter().enumerate() {
        for (li, c) in tree.child[s].iter().enumerate() {
            if let Some(c) = *c {
                placed[c] += d.links[li].channels().len();
            }
        }
    }
    let mut out: Vec<Instance> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut seen_top = vec![false; sheets.len()];
    // Top sheets, then any sheet only reachable through a loop.
    let tops: Vec<usize> = tree
        .roots
        .iter()
        .copied()
        .chain(tree.order().into_iter().map(|(s, _)| s))
        .collect();
    for top in tops {
        if seen_top[top] || out.iter().any(|i| i.sheet == top) {
            continue;
        }
        seen_top[top] = true;
        out.push(Instance {
            sheet: top,
            parent: None,
            suffix: String::new(),
        });
        stack.push((out.len() - 1, 0));
        while let Some((i, depth)) = stack.pop() {
            if depth >= MAX_DEPTH {
                continue;
            }
            let s = out[i].sheet;
            for (li, c) in tree.child[s].iter().enumerate() {
                let Some(c) = *c else { continue };
                for ch in sheets[s].links[li].channels() {
                    let mut suffix = out[i].suffix.clone();
                    if placed[c] > 1 && !ch.is_empty() {
                        if !suffix.is_empty() {
                            suffix.push('_');
                        }
                        suffix.push_str(&ch);
                    }
                    out.push(Instance {
                        sheet: c,
                        parent: Some((i, li)),
                        suffix,
                    });
                    stack.push((out.len() - 1, depth + 1));
                }
            }
        }
    }
    out
}

/// Joins the sheets' nets (`Drawing::connect` must have run). Without
/// `names`, sheet symbols cannot be followed, and every name joins every
/// other of the same text, whatever names it.
pub fn join(sheets: &[&Drawing], names: Option<&[&str]>, scope: Scope) -> Joined {
    let n = sheets.len();
    let mut out = Joined::default();
    let tree = match names {
        Some(names) => tree(names, sheets),
        None => Tree {
            parent: vec![None; n],
            child: sheets.iter().map(|d| vec![None; d.links.len()]).collect(),
            entry_of: sheets.iter().map(|d| vec![None; d.tags.len()]).collect(),
            roots: (0..n).collect(),
            depth: vec![0; n],
        },
    };
    out.instances = instances(sheets, &tree);
    let mut first = Vec::with_capacity(out.instances.len());
    let mut total = 0;
    for inst in &out.instances {
        first.push(total);
        total += sheets[inst.sheet].nets.len();
    }
    // The net each tag is on.
    let net_of_tag: Vec<Vec<usize>> = sheets
        .iter()
        .map(|d| {
            let mut v = vec![usize::MAX; d.tags.len()];
            for (ni, net) in d.nets.iter().enumerate() {
                for &t in &net.tags {
                    if let Some(x) = v.get_mut(t as usize) {
                        *x = ni;
                    }
                }
            }
            v
        })
        .collect();
    let mut dsu = Dsu((0..total).collect());
    let mut global: HashMap<String, usize> = HashMap::new();
    let mut by_global = |dsu: &mut Dsu, text: &str, g: usize| {
        let k = text.trim().to_ascii_uppercase();
        if k.is_empty() {
            return;
        }
        match global.get(&k) {
            Some(&o) => dsu.join(g, o),
            None => {
                global.insert(k, g);
            }
        }
    };
    let scope = if names.is_some() {
        scope.resolve(sheets)
    } else {
        Scope::Global
    };
    out.scope = scope;
    for (ii, inst) in out.instances.iter().enumerate() {
        let s = inst.sheet;
        for (t, tag) in sheets[s].tags.iter().enumerate() {
            let ni = net_of_tag[s][t];
            if ni == usize::MAX {
                continue;
            }
            let everywhere = names.is_none()
                || match tag.kind {
                    TagKind::Power => scope != Scope::Strict,
                    TagKind::Label => scope == Scope::Global,
                    TagKind::Port => matches!(scope, Scope::Flat | Scope::Global),
                    TagKind::SheetEntry => false,
                };
            if everywhere {
                // Labels, ports and power ports share names, as Altium
                // joins a label to the power net it names.
                by_global(&mut dsu, &tag.text, first[ii] + ni);
            }
        }
    }
    if names.is_some() {
        // Ports of each sheet, by name.
        let ports: Vec<HashMap<String, Vec<usize>>> = sheets
            .iter()
            .map(|d| {
                let mut m: HashMap<String, Vec<usize>> = HashMap::new();
                for (t, tag) in d.tags.iter().enumerate() {
                    if tag.kind == TagKind::Port && !tag.text.trim().is_empty() {
                        m.entry(tag.text.trim().to_ascii_uppercase())
                            .or_default()
                            .push(t);
                    }
                }
                m
            })
            .collect();
        let mut port_used: Vec<Vec<bool>> =
            sheets.iter().map(|d| vec![false; d.tags.len()]).collect();
        for (s, d) in sheets.iter().enumerate() {
            for (li, c) in tree.child[s].iter().enumerate() {
                if c.is_none() {
                    out.missing.push((s, li));
                }
            }
            for (t, tag) in d.tags.iter().enumerate() {
                let Some(li) = tree.entry_of[s][t] else {
                    continue;
                };
                let Some(c) = tree.child[s][li] else {
                    continue;
                };
                match ports[c].get(&tag.text.trim().to_ascii_uppercase()) {
                    Some(ps) => ps.iter().for_each(|&p| port_used[c][p] = true),
                    None => out.lone_entries.push((s, t)),
                }
            }
        }
        if scope.ports_to_entries() {
            for (ci, child) in out.instances.iter().enumerate() {
                let Some((pi, li)) = child.parent else {
                    continue;
                };
                let (s, c) = (out.instances[pi].sheet, child.sheet);
                for (t, tag) in sheets[s].tags.iter().enumerate() {
                    if tree.entry_of[s][t] != Some(li) {
                        continue;
                    }
                    let a = net_of_tag[s][t];
                    let Some(ps) = ports[c].get(&tag.text.trim().to_ascii_uppercase()) else {
                        continue;
                    };
                    for &p in ps {
                        let b = net_of_tag[c][p];
                        if a != usize::MAX && b != usize::MAX {
                            dsu.join(first[pi] + a, first[ci] + b);
                        }
                    }
                }
            }
            for (c, d) in sheets.iter().enumerate() {
                if tree.parent[c].is_none() {
                    continue;
                }
                for (t, tag) in d.tags.iter().enumerate() {
                    if tag.kind == TagKind::Port && !port_used[c][t] {
                        out.lone_ports.push((c, t));
                    }
                }
            }
        }
    }
    // A part drawn on two sheets joins its pins' nets.
    let mut by_pin: HashMap<(String, String), usize> = HashMap::new();
    for (ii, inst) in out.instances.iter().enumerate() {
        let d = sheets[inst.sheet];
        for (ni, net) in d.nets.iter().enumerate() {
            for &p in &net.pins {
                let Some(pin) = d.pins.get(p as usize) else {
                    continue;
                };
                let Some(part) = d.parts.get(pin.part as usize) else {
                    continue;
                };
                if unannotated(&part.designator) {
                    continue;
                }
                let k = (
                    inst.designator(part.designator.trim()).to_ascii_uppercase(),
                    pin.number.trim().to_ascii_uppercase(),
                );
                match by_pin.get(&k) {
                    Some(&o) => dsu.join(first[ii] + ni, o),
                    None => {
                        by_pin.insert(k, first[ii] + ni);
                    }
                }
            }
        }
    }
    let mut index: HashMap<usize, usize> = HashMap::new();
    out.of = out
        .instances
        .iter()
        .enumerate()
        .map(|(ii, inst)| {
            (0..sheets[inst.sheet].nets.len())
                .map(|ni| {
                    let r = dsu.find(first[ii] + ni);
                    let next = index.len();
                    *index.entry(r).or_insert(next)
                })
                .collect()
        })
        .collect();
    out.count = index.len();
    out.tree = tree;
    out
}

/// Kind of name, sheet depth and order seen: the lowest names the net.
type NameRank = (u8, usize, usize);

/// The name of each project net: a power port's first, then a net
/// label's, a port's and a sheet entry's, the one nearest the top sheet
/// winning among the same kind. A name local to a channel's sheet gets
/// the channel's suffix (EN_CH2).
pub fn net_names(sheets: &[&Drawing], j: &Joined) -> Vec<Option<String>> {
    let mut best: Vec<Option<(NameRank, String)>> = vec![None; j.count];
    let mut order = 0;
    let mut depth = vec![0usize; j.instances.len()];
    for (ii, inst) in j.instances.iter().enumerate() {
        if let Some((p, _)) = inst.parent {
            depth[ii] = depth[p] + 1;
        }
    }
    for (ii, inst) in j.instances.iter().enumerate() {
        let d = sheets[inst.sheet];
        for (ni, net) in d.nets.iter().enumerate() {
            let pn = j.of[ii][ni];
            for &t in &net.tags {
                let tag = &d.tags[t as usize];
                if tag.text.trim().is_empty() {
                    continue;
                }
                let rank = match tag.kind {
                    TagKind::Power => 0,
                    TagKind::Label => 1,
                    TagKind::Port => 2,
                    TagKind::SheetEntry => 3,
                };
                order += 1;
                let k = (rank, depth[ii], order);
                if best[pn].as_ref().is_none_or(|b| k < b.0) {
                    let global = match tag.kind {
                        TagKind::Power => j.scope != Scope::Strict,
                        TagKind::Label => j.scope == Scope::Global,
                        TagKind::Port => matches!(j.scope, Scope::Flat | Scope::Global),
                        TagKind::SheetEntry => false,
                    };
                    let name = if global || inst.suffix.is_empty() {
                        tag.text.clone()
                    } else {
                        format!("{}_{}", tag.text, inst.suffix)
                    };
                    best[pn] = Some((k, name));
                }
            }
        }
    }
    best.into_iter().map(|b| b.map(|b| b.1)).collect()
}

/// Where to look for the other end of a port or sheet entry: for a sheet
/// entry, the sheet it leads into and the port's tag there; for a port,
/// the parent sheet and the sheet entry's tag.
pub fn other_end(
    sheets: &[&Drawing],
    tree: &Tree,
    sheet: usize,
    tag: usize,
) -> Option<(usize, usize)> {
    let d = sheets.get(sheet)?;
    let t = d.tags.get(tag)?;
    let key = t.text.trim().to_ascii_uppercase();
    match t.kind {
        TagKind::SheetEntry => {
            let li = tree.entry_of.get(sheet)?.get(tag).copied().flatten()?;
            let c = tree.child[sheet][li]?;
            let p = sheets[c].tags.iter().position(|x| {
                x.kind == TagKind::Port && x.text.trim().eq_ignore_ascii_case(&key)
            })?;
            Some((c, p))
        }
        TagKind::Port => {
            let (p, li) = tree.parent.get(sheet).copied().flatten()?;
            let e = sheets[p].tags.iter().enumerate().position(|(i, x)| {
                x.kind == TagKind::SheetEntry
                    && tree.entry_of[p][i] == Some(li)
                    && x.text.trim().eq_ignore_ascii_case(&key)
            })?;
            Some((p, e))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::{NetTag, SchPart, SchPin, SheetLink};

    fn tag(at: Pt, text: &str, kind: TagKind) -> NetTag {
        NetTag {
            at: vec![at],
            text: text.into(),
            kind,
            graphic: None,
        }
    }

    fn pin(at: Pt, part: u32) -> SchPin {
        SchPin {
            at,
            base: at,
            part,
            number: "1".into(),
            ..Default::default()
        }
    }

    /// A top sheet placing one child twice-named block: entry EN wired to
    /// R1, and a local label X on both sheets.
    fn project() -> Vec<Drawing> {
        let mut top = Drawing {
            wires: vec![
                vec![[0.0, 0.0], [100.0, 0.0]],
                vec![[0.0, 500.0], [50.0, 500.0]],
            ],
            pins: vec![pin([0.0, 0.0], 0), pin([0.0, 500.0], 1)],
            parts: vec![
                SchPart {
                    designator: "R1".into(),
                    ..Default::default()
                },
                SchPart {
                    designator: "R2".into(),
                    ..Default::default()
                },
            ],
            tags: vec![
                tag([100.0, 0.0], "EN", TagKind::SheetEntry),
                tag([50.0, 500.0], "X", TagKind::Label),
                tag([100.0, 200.0], "SPARE", TagKind::SheetEntry),
            ],
            links: vec![SheetLink {
                min: [100.0, -100.0],
                max: [400.0, 300.0],
                file: "Sub\\Child.SchDoc".into(),
                name: "CH1".into(),
            }],
            ..Default::default()
        };
        let mut child = Drawing {
            wires: vec![
                vec![[0.0, 0.0], [100.0, 0.0]],
                vec![[0.0, 500.0], [50.0, 500.0]],
            ],
            pins: vec![pin([100.0, 0.0], 0), pin([0.0, 500.0], 1)],
            parts: vec![
                SchPart {
                    designator: "R3".into(),
                    ..Default::default()
                },
                SchPart {
                    designator: "R4".into(),
                    ..Default::default()
                },
            ],
            tags: vec![
                tag([0.0, 0.0], "EN", TagKind::Port),
                tag([50.0, 500.0], "X", TagKind::Label),
                tag([900.0, 900.0], "LOOSE", TagKind::Port),
            ],
            ..Default::default()
        };
        top.connect();
        child.connect();
        vec![top, child]
    }

    fn net_of_pin(d: &Drawing, p: u32) -> usize {
        d.nets.iter().position(|n| n.pins.contains(&p)).unwrap()
    }

    #[test]
    fn ports_join_their_sheet_entries() {
        let p = project();
        let refs: Vec<&Drawing> = p.iter().collect();
        let names = ["Top.SchDoc", "child.schdoc"];
        let t = tree(&names, &refs);
        assert_eq!(t.parent, vec![None, Some((0, 0))]);
        assert_eq!(t.roots, vec![0]);
        assert_eq!(t.depth, vec![0, 1]);
        assert_eq!(other_end(&refs, &t, 0, 0), Some((1, 0)));
        assert_eq!(other_end(&refs, &t, 1, 0), Some((0, 0)));

        let j = join(&refs, Some(&names), Scope::Automatic);
        assert_eq!(j.scope, Scope::Hierarchical);
        let r1 = j.of[0][net_of_pin(&p[0], 0)];
        let r3 = j.of[1][net_of_pin(&p[1], 0)];
        assert_eq!(r1, r3, "EN joins through the sheet entry");
        let x_top = j.of[0][net_of_pin(&p[0], 1)];
        let x_child = j.of[1][net_of_pin(&p[1], 1)];
        assert_ne!(x_top, x_child, "net labels stay on their sheet");
        assert_eq!(j.lone_entries, vec![(0, 2)]);
        assert_eq!(j.lone_ports, vec![(1, 2)]);
        assert!(j.missing.is_empty());
        let names_of = net_names(&refs, &j);
        assert_eq!(names_of[r1].as_deref(), Some("EN"));

        let g = join(&refs, Some(&names), Scope::Global);
        assert_eq!(g.of[0][net_of_pin(&p[0], 1)], g.of[1][net_of_pin(&p[1], 1)]);
        let f = join(&refs, None, Scope::Automatic);
        assert_eq!(f.of[0][net_of_pin(&p[0], 1)], f.of[1][net_of_pin(&p[1], 1)]);
    }

    #[test]
    fn channels_get_their_own_nets_and_designators() {
        let mut p = project();
        // A second sheet symbol placing the same sheet, its EN wired to R2.
        let top = &mut p[0];
        top.links[0].name = "CH1".into();
        top.links.push(SheetLink {
            min: [100.0, 400.0],
            max: [400.0, 800.0],
            file: "Child.SchDoc".into(),
            name: "CH2".into(),
        });
        top.tags[1] = tag([100.0, 500.0], "EN", TagKind::SheetEntry);
        top.wires[1] = vec![[0.0, 500.0], [100.0, 500.0]];
        top.connect();
        let refs: Vec<&Drawing> = p.iter().collect();
        let names = ["Top", "Child"];
        let j = join(&refs, Some(&names), Scope::Hierarchical);
        let sfx: Vec<&str> = j.instances.iter().map(|i| i.suffix.as_str()).collect();
        assert_eq!(sfx, ["", "CH1", "CH2"]);
        let nets = crate::eco::netlist_in(&refs, Some(&names), Scope::Hierarchical);
        let net_with = |d: &str| {
            nets.iter()
                .find(|n| n.pins.contains(&(d.to_string(), "1".to_string())))
                .unwrap()
        };
        assert!(net_with("R1").pins.contains(&("R3_CH1".into(), "1".into())));
        assert!(net_with("R2").pins.contains(&("R3_CH2".into(), "1".into())));
        assert_ne!(net_with("R1"), net_with("R2"));
        assert_eq!(net_with("R4_CH2").name.as_deref(), Some("X_CH2"));

        let rep = SheetLink {
            min: [0.0; 2],
            max: [0.0; 2],
            file: String::new(),
            name: "Repeat(CH,1,3)".into(),
        };
        assert_eq!(rep.channels(), ["CH1", "CH2", "CH3"]);
    }

    #[test]
    fn reads_altium_scope_names() {
        assert_eq!(Scope::from_altium(3), Scope::Global);
        for s in Scope::ALL {
            assert_eq!(Scope::from_altium(s.altium_number()), s);
        }
        assert_eq!(Scope::from_altium(0), Scope::Automatic);
        assert_eq!(sheet_key("A\\B/Power.SchDoc"), "POWER");
        assert_eq!(Scope::parse("hier"), Some(Scope::Hierarchical));
    }
}
