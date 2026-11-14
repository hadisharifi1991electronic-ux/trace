//! Annotation: designators for parts that have none yet (R?, U?A), or new
//! ones for every part, numbered in the order they sit on the sheets, as
//! Altium's and Allegro's annotate commands do. Gates of one component
//! (U1A, U1B) share a number; new gates fill a component of the same kind
//! on the same sheet that lacks them before a new one is started.

use crate::schematic::{Pt, Sheet};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The order parts are numbered in on each sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Row by row from the top, left to right in each row.
    AcrossThenDown,
    /// Column by column from the left, top to bottom in each.
    DownThenAcross,
}

#[derive(Clone, Debug)]
pub struct AnnotateGoal {
    pub order: Order,
    /// Number every part again, not only those without a number.
    pub all: bool,
    /// The first number of each prefix.
    pub start: u32,
    /// Number each sheet from `start + sheet index × step` (100: the
    /// second sheet's parts are R101…).
    pub sheet_step: Option<u32>,
}

impl Default for AnnotateGoal {
    fn default() -> Self {
        AnnotateGoal {
            order: Order::AcrossThenDown,
            all: false,
            start: 1,
            sheet_step: None,
        }
    }
}

/// One part given a new designator.
#[derive(Clone, Debug, PartialEq)]
pub struct Renamed {
    pub sheet: usize,
    pub part: usize,
    pub from: String,
    pub to: String,
}

/// A designator's letters and number: R12 is ("R", 12); R? and U are
/// ("R", None) and ("U", None).
pub fn split(designator: &str) -> (String, Option<u32>) {
    let d = designator.trim();
    let digits = d.len() - d.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (head, num) = d.split_at(d.len() - digits);
    let head = head.trim_end_matches('?');
    if head.contains('?') {
        return (head.replace('?', ""), None);
    }
    (head.to_string(), num.parse().ok())
}

/// Whether a designator still needs a number.
pub fn unannotated(designator: &str) -> bool {
    designator.contains('?') || split(designator).1.is_none()
}

/// Components started in a run: designator and the gates taken.
type Packages = Vec<(String, BTreeSet<u32>)>;

struct Inst {
    sheet: usize,
    part: usize,
    designator: String,
    gate: u32,
    lib: String,
    at: Pt,
}

/// Rows (or columns) are this tall: parts whose centres are closer than
/// this across the order count as one row.
const BAND: f64 = 400.0;

/// The new designators, in the order given.
pub fn annotate(sheets: &[&Sheet], goal: &AnnotateGoal) -> Vec<Renamed> {
    let mut all: Vec<Inst> = Vec::new();
    for (si, s) in sheets.iter().enumerate() {
        for (pi, p) in s.drawing.parts.iter().enumerate() {
            if p.designator.trim().is_empty() {
                continue;
            }
            all.push(Inst {
                sheet: si,
                part: pi,
                designator: p.designator.clone(),
                gate: p.gate,
                lib: s
                    .symbols
                    .get(p.symbol)
                    .map(|y| y.library_ref.clone())
                    .unwrap_or_default(),
                at: [(p.min[0] + p.max[0]) / 2.0, (p.min[1] + p.max[1]) / 2.0],
            });
        }
    }
    let renumber = |i: &Inst| goal.all || unannotated(&i.designator);
    // Numbers kept, by prefix.
    let mut used: HashMap<String, BTreeSet<u32>> = HashMap::new();
    for i in all.iter().filter(|i| !renumber(i)) {
        let (p, n) = split(&i.designator);
        used.entry(p.to_ascii_uppercase()).or_default().extend(n);
    }
    let key = |i: &Inst| -> (usize, i64, i64) {
        let (a, b) = match goal.order {
            Order::AcrossThenDown => ((-i.at[1] / BAND).floor(), i.at[0]),
            Order::DownThenAcross => ((i.at[0] / BAND).floor(), -i.at[1]),
        };
        (i.sheet, a as i64, b.round() as i64)
    };
    let mut todo: Vec<&Inst> = all.iter().filter(|i| renumber(i)).collect();
    todo.sort_by_key(|i| key(i));
    // Gates that already shared a designator keep sharing one (when all
    // are numbered again): old designator -> new.
    let mut regrouped: HashMap<(usize, String), String> = HashMap::new();
    // Packages started in this run: (sheet, prefix, lib) -> (designator,
    // gates taken), in order.
    let mut open: BTreeMap<(usize, String, String), Packages> = BTreeMap::new();
    let mut out = Vec::new();
    for i in todo {
        let (prefix, _) = split(&i.designator);
        let upper = prefix.to_ascii_uppercase();
        let shared = goal.all && !unannotated(&i.designator) && i.gate > 0;
        // Gates of a numbered component stay together, even across
        // sheets (an FPGA's banks), numbered from the first one's sheet.
        let group_key = (0usize, i.designator.to_ascii_uppercase());
        let to = if shared && regrouped.contains_key(&group_key) {
            regrouped[&group_key].clone()
        } else {
            let pk = (i.sheet, upper.clone(), i.lib.clone());
            let fill = (i.gate > 0 && !shared)
                .then(|| {
                    open.get_mut(&pk).and_then(|v| {
                        v.iter_mut()
                            .find(|(_, g)| !g.contains(&i.gate))
                            .map(|(d, g)| {
                                g.insert(i.gate);
                                d.clone()
                            })
                    })
                })
                .flatten();
            match fill {
                Some(d) => d,
                None => {
                    let base = goal.start
                        + goal
                            .sheet_step
                            .map_or(0, |s| s.saturating_mul(i.sheet as u32));
                    let set = used.entry(upper.clone()).or_default();
                    let mut n = base;
                    while set.contains(&n) {
                        n += 1;
                    }
                    set.insert(n);
                    let d = format!("{prefix}{n}");
                    if i.gate > 0 {
                        open.entry(pk)
                            .or_default()
                            .push((d.clone(), [i.gate].into()));
                    }
                    if shared {
                        regrouped.insert(group_key, d.clone());
                    }
                    d
                }
            }
        };
        if to != i.designator {
            out.push(Renamed {
                sheet: i.sheet,
                part: i.part,
                from: i.designator.clone(),
                to,
            });
        }
    }
    out
}

/// Applies the new designators to the sheets: the parts, their drawn
/// designators and their symbol entries.
pub fn apply(sheets: &mut [&mut Sheet], renamed: &[Renamed]) {
    for r in renamed {
        let Some(s) = sheets.get_mut(r.sheet) else {
            continue;
        };
        let Some(sym) = s.drawing.parts.get(r.part).map(|p| p.symbol) else {
            continue;
        };
        s.drawing.rename_part(r.part, &r.to);
        if let Some(y) = s.symbols.get_mut(sym) {
            y.designator = r.to.clone();
        }
    }
}

/// The renames of numbered components, old -> new, for carrying over to
/// the board (parts without a number are not on it yet).
pub fn component_renames(renamed: &[Renamed]) -> BTreeMap<String, String> {
    renamed
        .iter()
        .filter(|r| !unannotated(&r.from))
        .map(|r| (r.from.clone(), r.to.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::*;

    fn part(s: &mut Sheet, d: &str, gate: u32, lib: &str, x: f64, y: f64) {
        s.symbols.push(Symbol {
            designator: d.into(),
            library_ref: lib.into(),
            ..Default::default()
        });
        s.drawing.parts.push(SchPart {
            designator: d.into(),
            symbol: s.symbols.len() - 1,
            min: [x - 50.0, y - 50.0],
            max: [x + 50.0, y + 50.0],
            gate,
        });
    }

    #[test]
    fn splits_designators() {
        assert_eq!(split("R12"), ("R".into(), Some(12)));
        assert_eq!(split("R?"), ("R".into(), None));
        assert_eq!(split("U?"), ("U".into(), None));
        assert_eq!(split("TP"), ("TP".into(), None));
        assert!(unannotated("C?"));
        assert!(!unannotated("C3"));
    }

    #[test]
    fn numbers_new_parts_in_order_and_fills_gates() {
        let mut s = Sheet::default();
        part(&mut s, "R?", 0, "RES", 2000.0, 1000.0);
        part(&mut s, "R?", 0, "RES", 1000.0, 1000.0);
        part(&mut s, "R2", 0, "RES", 500.0, 3000.0);
        part(&mut s, "R?", 0, "RES", 500.0, 2000.0);
        part(&mut s, "U?", 1, "LM358", 3000.0, 3000.0);
        part(&mut s, "U?", 2, "LM358", 3500.0, 3000.0);
        part(&mut s, "U?", 1, "LM358", 4000.0, 3000.0);
        let r = annotate(&[&s], &AnnotateGoal::default());
        let to = |i: usize| r.iter().find(|x| x.part == i).map(|x| x.to.clone());
        // Top row first (y up): R? at y=2000 gets R1, then the row at 1000
        // left to right: R3, R4 (R2 is taken).
        assert_eq!(to(3).as_deref(), Some("R1"));
        assert_eq!(to(1).as_deref(), Some("R3"));
        assert_eq!(to(0).as_deref(), Some("R4"));
        assert_eq!(to(2), None);
        // U?A and U?B share U1; the second A starts U2.
        assert_eq!(to(4).as_deref(), Some("U1"));
        assert_eq!(to(5).as_deref(), Some("U1"));
        assert_eq!(to(6).as_deref(), Some("U2"));
        let mut s2 = s.clone();
        apply(&mut [&mut s2], &r);
        assert_eq!(s2.drawing.parts[0].designator, "R4");
        assert_eq!(s2.symbols[0].designator, "R4");
        assert!(annotate(&[&s2], &AnnotateGoal::default()).is_empty());
    }

    #[test]
    fn numbers_everything_again_by_sheet() {
        let mut a = Sheet::default();
        part(&mut a, "R7", 0, "RES", 1000.0, 1000.0);
        part(&mut a, "U3", 1, "LM358", 2000.0, 1000.0);
        part(&mut a, "U3", 2, "LM358", 3000.0, 1000.0);
        let mut b = Sheet::default();
        part(&mut b, "R1", 0, "RES", 1000.0, 1000.0);
        let goal = AnnotateGoal {
            all: true,
            sheet_step: Some(100),
            ..Default::default()
        };
        let r = annotate(&[&a, &b], &goal);
        let to = |s: usize, i: usize| {
            r.iter()
                .find(|x| x.sheet == s && x.part == i)
                .map(|x| x.to.clone())
        };
        assert_eq!(to(0, 0).as_deref(), Some("R1"));
        assert_eq!(to(0, 1).as_deref(), Some("U1"));
        assert_eq!(to(0, 2).as_deref(), Some("U1"));
        assert_eq!(to(1, 0).as_deref(), Some("R101"));
        let m = component_renames(&r);
        assert_eq!(m.get("R7").map(String::as_str), Some("R1"));
        assert_eq!(m.get("R1").map(String::as_str), Some("R101"));
    }
}
