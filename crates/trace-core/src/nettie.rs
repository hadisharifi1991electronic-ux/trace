//! Net ties: parts that join two nets on purpose (a star point between
//! analog and digital ground, a current-sense Kelvin joint), as Altium's
//! net tie parts and Allegro's net short property allow. The rule check
//! lets a net tie's own copper (its pads and drawn copper) join its nets.

use crate::Board;
use std::collections::HashSet;

/// Names that mark a footprint or part as a net tie.
pub fn tie_name(designator: &str, footprint: &str) -> bool {
    let d = designator.trim().to_ascii_uppercase();
    let prefix: String = d.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let f: String = footprint
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    prefix == "NT" || f.contains("NETTIE") || f.contains("NETSHORT")
}

/// The parts that are net ties: marked by hand, named as one, or with
/// pads of different nets laid one on another (as Allegro boards draw
/// them).
pub fn ties(board: &Board) -> HashSet<usize> {
    let mut out: HashSet<usize> = board
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            board.net_ties.iter().any(|d| d == &c.designator)
                || tie_name(&c.designator, &c.footprint)
        })
        .map(|(i, _)| i)
        .collect();
    let mut pads: std::collections::HashMap<u32, Vec<&crate::Pad>> = Default::default();
    for p in &board.geometry.pads {
        if let Some(c) = p.component {
            pads.entry(c).or_default().push(p);
        }
    }
    for (c, v) in pads {
        if v.len() > 8 || out.contains(&(c as usize)) {
            continue;
        }
        let stacked = v.iter().enumerate().any(|(i, a)| {
            v[i + 1..].iter().any(|b| {
                a.net.is_some()
                    && b.net.is_some()
                    && a.net != b.net
                    && (a.layer == b.layer
                        || matches!(a.layer, crate::LayerRef::AllCopper)
                        || matches!(b.layer, crate::LayerRef::AllCopper))
                    && (a.at.x.0 - b.at.x.0).abs() * 2
                        < a.size_x.0.max(a.size_y.0) + b.size_x.0.max(b.size_y.0)
                    && (a.at.y.0 - b.at.y.0).abs() * 2
                        < a.size_x.0.max(a.size_y.0) + b.size_x.0.max(b.size_y.0)
            })
        });
        if stacked {
            out.insert(c as usize);
        }
    }
    out
}

/// Marks or unmarks parts (by designator) as net ties.
pub fn set(board: &mut Board, designators: &[String], tie: bool) {
    for d in designators {
        let has = board.net_ties.iter().position(|x| x == d);
        match (tie, has) {
            (true, None) => board.net_ties.push(d.clone()),
            (false, Some(i)) => {
                board.net_ties.remove(i);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_net_ties_by_name() {
        assert!(tie_name("NT1", "R0402"));
        assert!(tie_name("X1", "NetTie-2_SMD"));
        assert!(tie_name("J9", "NET_TIE_0603"));
        assert!(!tie_name("R1", "R0402"));
        // A thermistor is not a net tie.
        assert!(!tie_name("NTC1", "R0402"));
    }
}
