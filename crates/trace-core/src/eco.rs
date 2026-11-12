//! Engineering change orders: carrying schematic edits over to the board.
//!
//! The sheets as they were read and as they are now are each turned into a
//! project net list. Only what the edits changed is carried over, so the
//! way sheets join (flat, by net name) need not match the source tool's
//! exactly: a net the edits left alone is never touched on the board.

use crate::edit::Edit;
use crate::schematic::Drawing;
use crate::{Board, Net};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A pin as (designator, pin number), upper case.
pub type PinKey = (String, String);

fn key(d: &str, p: &str) -> PinKey {
    (d.trim().to_ascii_uppercase(), p.trim().to_ascii_uppercase())
}

/// A net of the whole project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectNet {
    pub name: Option<String>,
    pub pins: BTreeSet<PinKey>,
}

/// The project's nets: each sheet's nets, joined across sheets by name
/// (net labels, power ports, ports and sheet entries alike).
pub fn netlist(sheets: &[&Drawing]) -> Vec<ProjectNet> {
    let mut out: Vec<ProjectNet> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut by_pin: HashMap<PinKey, usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for d in sheets {
        for n in &d.nets {
            let pins: Vec<PinKey> = n
                .pins
                .iter()
                .filter_map(|&p| {
                    let pin = d.pins.get(p as usize)?;
                    let part = d.parts.get(pin.part as usize)?;
                    Some(key(&part.designator, &pin.number))
                })
                .collect();
            if pins.is_empty() && n.name.is_none() {
                continue;
            }
            let i = out.len();
            out.push(ProjectNet {
                name: n.name.clone(),
                pins: pins.iter().cloned().collect(),
            });
            parent.push(i);
            if let Some(name) = &n.name {
                match by_name.get(&name.to_ascii_uppercase()) {
                    Some(&o) => {
                        let (a, b) = (find(&mut parent, i), find(&mut parent, o));
                        parent[a] = b;
                    }
                    None => {
                        by_name.insert(name.to_ascii_uppercase(), i);
                    }
                }
            }
            // A pin drawn twice (a part on two sheets) joins its nets.
            for k in pins {
                match by_pin.get(&k) {
                    Some(&o) => {
                        let (a, b) = (find(&mut parent, i), find(&mut parent, o));
                        parent[a] = b;
                    }
                    None => {
                        by_pin.insert(k, i);
                    }
                }
            }
        }
    }
    let mut merged: HashMap<usize, ProjectNet> = HashMap::new();
    let mut order = Vec::new();
    for (i, n) in out.into_iter().enumerate() {
        let r = find(&mut parent, i);
        let m = merged.entry(r).or_insert_with(|| {
            order.push(r);
            ProjectNet::default()
        });
        if m.name.is_none() {
            m.name = n.name;
        }
        m.pins.extend(n.pins);
    }
    order
        .into_iter()
        .filter_map(|r| merged.remove(&r))
        .collect()
}

/// One change the plan makes, for the person to read before applying.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A pin moves to another net (or to none, or to a new one).
    Pin {
        designator: String,
        pin: String,
        from: Option<String>,
        to: Option<String>,
        new_net: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    /// A net takes the name its label now gives it.
    RenameNet {
        from: String,
        to: String,
    },
    /// A part on the sheets that the board does not have.
    NotOnBoard {
        designator: String,
    },
    /// A part taken off the sheets; its footprint stays on the board.
    Removed {
        designator: String,
    },
}

impl Change {
    pub fn describe(&self) -> String {
        let net = |n: &Option<String>| n.clone().unwrap_or_else(|| "no net".into());
        match self {
            Change::Pin {
                designator,
                pin,
                from,
                to,
                new_net,
            } => format!(
                "{designator}-{pin}: moves from {} to {}{}",
                net(from),
                net(to),
                if *new_net { " (new net)" } else { "" }
            ),
            Change::Rename { from, to } => format!("Rename {from} to {to}"),
            Change::RenameNet { from, to } => format!("Rename net {from} to {to}"),
            Change::NotOnBoard { designator } => {
                format!("{designator} is not on the board: place its footprint by hand")
            }
            Change::Removed { designator } => {
                format!("{designator} was removed from the sheets; its footprint stays")
            }
        }
    }
}

/// What carrying the edits over does, and the board edit that does it.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub edit: Option<Edit>,
}

/// Works out the board changes for sheets edited from `before` to
/// `after`. `renames` maps designators as now on the sheets to what they
/// were (and still are on the board).
pub fn plan(
    before: &[&Drawing],
    after: &[&Drawing],
    renames: &HashMap<String, String>,
    board: &Board,
) -> Plan {
    let old = netlist(before);
    let new = netlist(after);
    // A designator as on the board.
    let on_board = |k: &PinKey| -> PinKey {
        match renames.get(&k.0) {
            Some(o) => (o.to_ascii_uppercase(), k.1.clone()),
            None => k.clone(),
        }
    };
    // Board pads by pin, and each pin's board net.
    let mut pads: HashMap<PinKey, Vec<usize>> = HashMap::new();
    for (i, p) in board.geometry.pads.iter().enumerate() {
        if let Some(c) = p.component.and_then(|c| board.components.get(c as usize)) {
            pads.entry(key(&c.designator, &p.name)).or_default().push(i);
        }
    }
    let pad_net = |k: &PinKey| -> Option<u32> {
        pads.get(k)
            .and_then(|v| v.iter().find_map(|&i| board.geometry.pads[i].net))
    };
    let old_of: HashMap<&PinKey, usize> = old
        .iter()
        .enumerate()
        .flat_map(|(i, n)| n.pins.iter().map(move |k| (k, i)))
        .collect();
    // Each old net's board net: the one most of its pins are on.
    let board_of_old: Vec<Option<u32>> = old
        .iter()
        .map(|n| {
            let mut count: HashMap<u32, usize> = HashMap::new();
            for k in &n.pins {
                if let Some(b) = pad_net(&on_board(k)) {
                    *count.entry(b).or_default() += 1;
                }
            }
            count
                .into_iter()
                .max_by_key(|&(b, c)| (c, u32::MAX - b))
                .map(|(b, _)| b)
        })
        .collect();
    let by_name: HashMap<String, u32> = board
        .nets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.to_ascii_uppercase(), i as u32))
        .collect();

    let mut nets = board.nets.clone();
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut targets: Vec<Option<Option<u32>>> = vec![None; new.len()];
    // Nets the edits left as they were keep their board net.
    for (i, n) in new.iter().enumerate() {
        let olds: HashSet<usize> = n
            .pins
            .iter()
            .filter_map(|k| old_of.get(k).copied())
            .collect();
        if olds.len() == 1 {
            let o = *olds.iter().next().unwrap();
            if old[o].pins == n.pins && old[o].name == n.name {
                if let Some(b) = board_of_old[o] {
                    claimed.insert(b);
                }
                targets[i] = Some(None); // untouched
            }
        }
    }
    // The rest, biggest first, each takes a board net.
    let mut rest: Vec<usize> = (0..new.len()).filter(|&i| targets[i].is_none()).collect();
    rest.sort_by_key(|&i| std::cmp::Reverse(new[i].pins.len()));
    let mut changes = Vec::new();
    let mut moves: Vec<(usize, Option<u32>, Option<u32>)> = Vec::new();
    let name_of =
        |nets: &[Net], n: Option<u32>| n.and_then(|n| nets.get(n as usize)).map(|n| n.name.clone());
    for i in rest {
        let n = &new[i];
        let mut new_net = false;
        let target: Option<u32> = if n.pins.len() < 2 && n.name.is_none() {
            // A pin left on its own is on no net.
            None
        } else if let Some(b) = n
            .name
            .as_ref()
            .and_then(|nm| by_name.get(&nm.to_ascii_uppercase()))
        {
            Some(*b)
        } else {
            // The board net of the old net giving the most pins.
            let mut count: HashMap<usize, usize> = HashMap::new();
            for k in &n.pins {
                if let Some(&o) = old_of.get(k) {
                    *count.entry(o).or_default() += 1;
                }
            }
            let mut by_size: Vec<(usize, usize)> = count.into_iter().collect();
            by_size.sort_by_key(|&(o, c)| (std::cmp::Reverse(c), o));
            let inherited = by_size
                .iter()
                .filter_map(|&(o, _)| board_of_old[o])
                .find(|b| !claimed.contains(b));
            match inherited {
                Some(b) => {
                    // A label that renames the net renames it on the board.
                    if let Some(name) = &n.name {
                        let old = &nets[b as usize].name;
                        if !old.eq_ignore_ascii_case(name) {
                            changes.push(Change::RenameNet {
                                from: old.clone(),
                                to: name.clone(),
                            });
                            nets[b as usize].name = name.clone();
                        }
                    }
                    Some(b)
                }
                None if n.pins.len() >= 2 || n.name.is_some() => {
                    let name = n.name.clone().unwrap_or_else(|| {
                        let (d, p) = n.pins.iter().next().cloned().unwrap_or_default();
                        format!("Net{d}_{p}")
                    });
                    nets.push(Net {
                        name,
                        ..Default::default()
                    });
                    new_net = true;
                    Some(nets.len() as u32 - 1)
                }
                None => None,
            }
        };
        if let Some(t) = target {
            claimed.insert(t);
        }
        for k in &n.pins {
            let bk = on_board(k);
            let Some(list) = pads.get(&bk) else { continue };
            let from = pad_net(&bk);
            if from == target {
                continue;
            }
            for &p in list {
                moves.push((p, board.geometry.pads[p].net, target));
            }
            changes.push(Change::Pin {
                designator: k.0.clone(),
                pin: k.1.clone(),
                from: name_of(&board.nets, from),
                to: name_of(&nets, target),
                new_net,
            });
        }
    }

    // Renames, and parts only on one side.
    let mut names = Vec::new();
    for (now, was) in renames {
        if now == was {
            continue;
        }
        if let Some(c) = board
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(was))
        {
            names.push((c, board.components[c].designator.clone(), now.clone()));
            changes.push(Change::Rename {
                from: was.clone(),
                to: now.clone(),
            });
        }
    }
    let parts = |sheets: &[&Drawing]| -> BTreeSet<String> {
        sheets
            .iter()
            .flat_map(|d| d.parts.iter().map(|p| p.designator.to_ascii_uppercase()))
            .collect()
    };
    let (was, now) = (parts(before), parts(after));
    let board_parts: HashSet<String> = board
        .components
        .iter()
        .map(|c| c.designator.to_ascii_uppercase())
        .collect();
    for d in now.difference(&was) {
        let as_board = renames
            .get(d)
            .map(|o| o.to_ascii_uppercase())
            .unwrap_or(d.clone());
        if !board_parts.contains(&as_board) {
            changes.push(Change::NotOnBoard {
                designator: d.clone(),
            });
        }
    }
    let renamed_from: HashSet<String> = renames.values().map(|v| v.to_ascii_uppercase()).collect();
    for d in was.difference(&now) {
        if !renamed_from.contains(d) && board_parts.contains(d) {
            changes.push(Change::Removed {
                designator: d.clone(),
            });
        }
    }
    changes.sort_by_key(|c| match c {
        Change::Rename { .. } => 0,
        Change::RenameNet { .. } => 1,
        Change::Pin { .. } => 2,
        _ => 2,
    });
    names.sort();
    let renamed_net = nets.iter().zip(&board.nets).any(|(a, b)| a.name != b.name);
    let edit =
        (!moves.is_empty() || !names.is_empty() || renamed_net || nets.len() != board.nets.len())
            .then(|| Edit::Netlist {
                nets_before: board.nets.clone(),
                nets_after: nets,
                pads: moves,
                names,
            });
    Plan { changes, edit }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::*;
    use crate::*;

    /// R1 and R2 side by side, each pin 1 wired to a GND label, pins 2 to
    /// their own nets.
    fn drawing() -> Drawing {
        let mut d = Drawing::default();
        for (k, x) in [0.0, 1000.0].into_iter().enumerate() {
            d.parts.push(SchPart {
                designator: format!("R{}", k + 1),
                symbol: 0,
                min: [x - 100.0, -50.0],
                max: [x + 100.0, 50.0],
            });
            for (n, dx) in [(1, -200.0), (2, 200.0)] {
                d.pins.push(SchPin {
                    at: [x + dx, 0.0],
                    base: [x + dx / 2.0, 0.0],
                    part: k as u32,
                    name: String::new(),
                    number: n.to_string(),
                });
            }
        }
        d.add_label([-200.0, 0.0], "GND");
        d.add_label([800.0, 0.0], "GND");
        d.add_label([200.0, 0.0], "A");
        d.add_label([1200.0, 0.0], "B");
        d
    }

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["GND", "A", "B"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        for (c, nets) in [("R1", [0, 1]), ("R2", [0, 2])] {
            b.components.push(Component {
                designator: c.into(),
                footprint: "0402".into(),
                library_ref: None,
                x: Length(0),
                y: Length(0),
                rotation_deg: 0.0,
                side: Side::Top,
                sheet: None,
                params: Default::default(),
            });
            for (i, n) in nets.into_iter().enumerate() {
                b.geometry.pads.push(Pad {
                    layer: LayerRef::Copper(0),
                    net: Some(n),
                    component: Some(b.components.len() as u32 - 1),
                    name: (i + 1).to_string(),
                    at: Point::default(),
                    size_x: Length(1),
                    size_y: Length(1),
                    hole: Length(0),
                    shape: PadShape::Rect,
                    rotation_deg: 0.0,
                    outline: Vec::new(),
                    stack: Vec::new(),
                    fab: Default::default(),
                });
            }
        }
        b
    }

    #[test]
    fn untouched_sheets_change_nothing() {
        let d = drawing();
        let p = plan(&[&d], &[&d], &HashMap::new(), &board());
        assert!(p.changes.is_empty() && p.edit.is_none(), "{:?}", p.changes);
    }

    #[test]
    fn relabelled_and_joined_pins_move() {
        let d = drawing();
        let mut e = d.clone();
        // R2-2 joins net A by its label.
        e.rename_tag(3, "A");
        let mut b = board();
        let p = plan(&[&d], &[&e], &HashMap::new(), &b);
        assert_eq!(
            p.changes,
            vec![Change::Pin {
                designator: "R2".into(),
                pin: "2".into(),
                from: Some("B".into()),
                to: Some("A".into()),
                new_net: false,
            }]
        );
        p.edit.clone().unwrap().apply(&mut b);
        assert_eq!(b.geometry.pads[3].net, Some(1));
        // Undo puts it back.
        p.edit.unwrap().inverse().apply(&mut b);
        assert_eq!(b.geometry.pads[3].net, Some(2));

        // A new label name makes a new net; a deleted label leaves R1-2
        // alone on no net.
        let mut e = d.clone();
        e.rename_tag(3, "CLK");
        e.delete_tag(2);
        let p = plan(&[&d], &[&e], &HashMap::new(), &board());
        let text: Vec<String> = p.changes.iter().map(Change::describe).collect();
        assert!(
            text.contains(&"Rename net B to CLK".to_string()),
            "{text:?}"
        );
        assert!(
            text.contains(&"R1-2: moves from A to no net".to_string()),
            "{text:?}"
        );
    }

    #[test]
    fn renames_reach_the_board() {
        let d = drawing();
        let mut e = d.clone();
        e.rename_part(1, "R9");
        let renames: HashMap<String, String> = [("R9".to_string(), "R2".to_string())].into();
        let mut b = board();
        let p = plan(&[&d], &[&e], &renames, &b);
        assert_eq!(p.changes.len(), 1, "{:?}", p.changes);
        p.edit.unwrap().apply(&mut b);
        assert_eq!(b.components[1].designator, "R9");
    }
}
