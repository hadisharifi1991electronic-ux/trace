//! Pin and gate swapping, as Altium's and Allegro's swap commands do it:
//! two pins (or every pin of two gates) trade nets on the board, the short
//! routing that leaves each pad (up to where it meets anything else of its
//! net) goes with it, and the schematic is changed to match by trading the
//! pins' numbers (and, for gates, their part letters).

use crate::edit::Edit;
use crate::route::{pt, P};
use crate::schematic::{Drawing, Pt, Sheet};
use crate::{Board, LayerRef};
use std::collections::{BTreeSet, HashMap};

/// A pad of a part by name: (part index, pad name).
pub type PadRef = (usize, String);

#[derive(Clone, Debug, Default)]
pub struct Swapped {
    pub edits: Vec<Edit>,
    /// Pads whose nets changed.
    pub pads: usize,
    /// Tracks, arcs and vias that went with them.
    pub copper: usize,
    /// Pins whose routing reaches other pads, so it stayed where it was
    /// (and now needs routing again).
    pub left: Vec<String>,
}

impl Swapped {
    pub fn summary(&self, board: &Board) -> String {
        let _ = board;
        let mut s = format!(
            "{} pads swapped nets, with {} tracks, arcs and vias",
            self.pads, self.copper
        );
        if !self.left.is_empty() {
            s.push_str(&format!(
                "; routing to other pads left in place on {} (route again)",
                self.left.join(", ")
            ));
        }
        s
    }
}

fn pads_named(board: &Board, part: usize, name: &str) -> Vec<usize> {
    board
        .geometry
        .pads
        .iter()
        .enumerate()
        .filter(|(_, p)| p.component == Some(part as u32) && p.name.eq_ignore_ascii_case(name))
        .map(|(i, _)| i)
        .collect()
}

/// Copper of one net as pieces with their touch points.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum Piece {
    Track(usize),
    Arc(usize),
    Via(usize),
}

/// The routing that leaves pad `start` and reaches no other pad of its
/// net: Ok(pieces), or Err if it does reach one.
fn stub(board: &Board, start: usize) -> Result<BTreeSet<Piece>, ()> {
    let g = &board.geometry;
    let pad = &g.pads[start];
    let Some(net) = pad.net else {
        return Ok(BTreeSet::new());
    };
    const TOL: f64 = 0.5;
    let layer_of = |l: &LayerRef| match l {
        LayerRef::Copper(i) => Some(*i),
        _ => None,
    };
    // Every piece: its layers (None = all) and points it joins at, with a
    // reach (a via's radius).
    let mut items: Vec<(Piece, Option<u8>, Vec<P>, f64)> = Vec::new();
    for (i, t) in g.tracks.iter().enumerate() {
        if t.net == Some(net) && t.component.is_none() {
            if let Some(l) = layer_of(&t.layer) {
                items.push((Piece::Track(i), Some(l), vec![pt(t.start), pt(t.end)], 0.0));
            }
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        if a.net == Some(net) && a.component.is_none() {
            if let Some(l) = layer_of(&a.layer) {
                let c = pt(a.center);
                let r = a.radius.mil();
                let e = |d: f64| {
                    [
                        c[0] + r * d.to_radians().cos(),
                        c[1] + r * d.to_radians().sin(),
                    ]
                };
                items.push((
                    Piece::Arc(i),
                    Some(l),
                    vec![e(a.start_deg), e(a.end_deg)],
                    0.0,
                ));
            }
        }
    }
    for (i, v) in g.vias.iter().enumerate() {
        if v.net == Some(net) {
            items.push((Piece::Via(i), None, vec![pt(v.at)], v.diameter.mil() / 2.0));
        }
    }
    // Pads of the net: (index, layer, centre, reach).
    let pads: Vec<(usize, Option<u8>, P, f64)> = g
        .pads
        .iter()
        .enumerate()
        .filter(|(_, p)| p.net == Some(net))
        .map(|(i, p)| {
            let l = if p.hole.0 > 0 {
                None
            } else {
                layer_of(&p.layer)
            };
            (i, l, pt(p.at), p.size_x.mil().max(p.size_y.mil()) / 2.0)
        })
        .collect();
    let meet = |la: Option<u8>, lb: Option<u8>| la.is_none() || lb.is_none() || la == lb;
    let touches = |a: &(Piece, Option<u8>, Vec<P>, f64), b: &(Piece, Option<u8>, Vec<P>, f64)| {
        meet(a.1, b.1)
            && a.2.iter().any(|p| {
                b.2.iter()
                    .any(|q| (p[0] - q[0]).hypot(p[1] - q[1]) <= a.3.max(b.3) + TOL)
            })
    };
    let on_pad = |it: &(Piece, Option<u8>, Vec<P>, f64), pd: &(usize, Option<u8>, P, f64)| {
        meet(it.1, pd.1)
            && it
                .2
                .iter()
                .any(|p| (p[0] - pd.2[0]).hypot(p[1] - pd.2[1]) <= pd.3 + it.3 + TOL)
    };
    let me = pads.iter().find(|p| p.0 == start).copied().unwrap_or((
        start,
        layer_of(&pad.layer),
        pt(pad.at),
        0.0,
    ));
    let mut seen = vec![false; items.len()];
    let mut stack: Vec<usize> = (0..items.len())
        .filter(|&i| on_pad(&items[i], &me))
        .collect();
    for &i in &stack {
        seen[i] = true;
    }
    let mut out = BTreeSet::new();
    while let Some(i) = stack.pop() {
        if pads.iter().any(|p| p.0 != start && on_pad(&items[i], p)) {
            return Err(());
        }
        out.insert(items[i].0);
        for j in 0..items.len() {
            if !seen[j] && touches(&items[i], &items[j]) {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    Ok(out)
}

/// Swaps the nets of pad pairs (each pair on one part or two), taking each
/// pad's own routing along.
pub fn swap_pads(board: &Board, pairs: &[(PadRef, PadRef)]) -> Result<Swapped, String> {
    let g = &board.geometry;
    let des = |p: usize| {
        board
            .components
            .get(p)
            .map_or("?".to_string(), |c| c.designator.clone())
    };
    let mut pad_moves: Vec<(usize, Option<u32>, Option<u32>)> = Vec::new();
    let mut copper: HashMap<Piece, Option<u32>> = HashMap::new();
    let mut out = Swapped::default();
    for ((pa, na), (pb, nb)) in pairs {
        let a = pads_named(board, *pa, na);
        let b = pads_named(board, *pb, nb);
        if a.is_empty() {
            return Err(format!("{} has no pad {na}", des(*pa)));
        }
        if b.is_empty() {
            return Err(format!("{} has no pad {nb}", des(*pb)));
        }
        let (neta, netb) = (g.pads[a[0]].net, g.pads[b[0]].net);
        if neta == netb {
            continue;
        }
        for (pads, to, name) in [
            (&a, netb, format!("{}.{na}", des(*pa))),
            (&b, neta, format!("{}.{nb}", des(*pb))),
        ] {
            let mut kept = false;
            for &i in pads.iter() {
                pad_moves.push((i, g.pads[i].net, to));
                match stub(board, i) {
                    Ok(set) => {
                        for piece in set {
                            copper.insert(piece, to);
                        }
                    }
                    Err(()) => kept = true,
                }
            }
            if kept {
                out.left.push(name);
            }
        }
    }
    if pad_moves.is_empty() {
        return Err("those pins are already on the same net".into());
    }
    out.pads = pad_moves.len();
    out.copper = copper.len();
    out.edits.push(Edit::Netlist {
        nets_before: board.nets.clone(),
        nets_after: board.nets.clone(),
        pads: pad_moves,
        names: Vec::new(),
    });
    let mut pieces: Vec<(Piece, Option<u32>)> = copper.into_iter().collect();
    pieces.sort_by_key(|p| p.0);
    for (piece, net) in pieces {
        match piece {
            Piece::Track(i) => {
                let mut t = g.tracks[i].clone();
                out.edits.push(Edit::RemoveTrack {
                    index: i,
                    track: t.clone(),
                });
                t.net = net;
                out.edits.push(Edit::InsertTrack { index: i, track: t });
            }
            Piece::Arc(i) => {
                let mut a = g.arcs[i].clone();
                out.edits.push(Edit::RemoveArc {
                    index: i,
                    arc: a.clone(),
                });
                a.net = net;
                out.edits.push(Edit::InsertArc { index: i, arc: a });
            }
            Piece::Via(i) => {
                let mut v = g.vias[i].clone();
                out.edits.push(Edit::RemoveVia {
                    index: i,
                    via: v.clone(),
                });
                v.net = net;
                out.edits.push(Edit::InsertVia { index: i, via: v });
            }
        }
    }
    Ok(out)
}

/// One drawn gate of a multi-part component: (sheet, part index).
pub type GateRef = (usize, usize);

/// The gates drawn for a designator: (sheet, part, gate letter).
pub fn gates_of(sheets: &[&Sheet], designator: &str) -> Vec<(usize, usize, u32)> {
    let mut v = Vec::new();
    for (si, s) in sheets.iter().enumerate() {
        for (pi, p) in s.drawing.parts.iter().enumerate() {
            if p.gate > 0 && p.designator.eq_ignore_ascii_case(designator) {
                v.push((si, pi, p.gate));
            }
        }
    }
    v.sort_by_key(|g| g.2);
    v
}

fn pins_of(s: &Sheet, part: usize) -> Vec<(usize, Pt, String, String)> {
    let p = &s.drawing.parts[part];
    s.drawing
        .pins
        .iter()
        .enumerate()
        .filter(|(_, q)| q.part as usize == part)
        .map(|(i, q)| {
            (
                i,
                [q.at[0] - p.min[0], q.at[1] - p.min[1]],
                q.name.clone(),
                q.number.clone(),
            )
        })
        .collect()
}

/// Matches the pins of two drawn gates of the same symbol: by where they
/// sit on the symbol, or else by name. Returns (pin of a, pin of b) as
/// pin indices on their sheets.
pub fn match_gates(
    sheets: &[&Sheet],
    a: GateRef,
    b: GateRef,
) -> Result<Vec<(usize, usize)>, String> {
    let (sa, sb) = (sheets[a.0], sheets[b.0]);
    let lib = |s: &Sheet, i: usize| {
        s.symbols
            .get(s.drawing.parts[i].symbol)
            .map(|y| y.library_ref.clone())
            .unwrap_or_default()
    };
    if !lib(sa, a.1).eq_ignore_ascii_case(&lib(sb, b.1)) {
        return Err("the gates are of different parts".into());
    }
    let pa = pins_of(sa, a.1);
    let pb = pins_of(sb, b.1);
    if pa.len() != pb.len() || pa.is_empty() {
        return Err("the gates have different pins".into());
    }
    let by_place: Option<Vec<(usize, usize)>> = pa
        .iter()
        .map(|x| {
            pb.iter()
                .find(|y| (x.1[0] - y.1[0]).abs() < 1.0 && (x.1[1] - y.1[1]).abs() < 1.0)
                .map(|y| (x.0, y.0))
        })
        .collect();
    if let Some(v) = by_place {
        return Ok(v);
    }
    pa.iter()
        .map(|x| {
            pb.iter()
                .find(|y| !x.2.is_empty() && y.2.eq_ignore_ascii_case(&x.2))
                .map(|y| (x.0, y.0))
                .ok_or_else(|| format!("no pin {} on the other gate", x.2))
        })
        .collect()
}

/// The pad pairs a gate swap trades on the board.
pub fn gate_pads(
    board: &Board,
    sheets: &[&Sheet],
    a: GateRef,
    b: GateRef,
) -> Result<Vec<(PadRef, PadRef)>, String> {
    let pairs = match_gates(sheets, a, b)?;
    let part = |d: &str| {
        board
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or_else(|| format!("{d} is not on the board"))
    };
    let da = &sheets[a.0].drawing.parts[a.1].designator;
    let db = &sheets[b.0].drawing.parts[b.1].designator;
    let (ca, cb) = (part(da)?, part(db)?);
    Ok(pairs
        .into_iter()
        .map(|(x, y)| {
            (
                (ca, sheets[a.0].drawing.pins[x].number.clone()),
                (cb, sheets[b.0].drawing.pins[y].number.clone()),
            )
        })
        .collect())
}

/// Trades the numbers of each pair of pins of part `designator` on one
/// drawing, all at once. Returns how many pins changed.
pub fn swap_numbers(d: &mut Drawing, designator: &str, pairs: &[(String, String)]) -> usize {
    let mut map: HashMap<String, String> = HashMap::new();
    for (a, b) in pairs {
        map.insert(a.to_ascii_uppercase(), b.clone());
        map.insert(b.to_ascii_uppercase(), a.clone());
    }
    let parts: Vec<bool> = d
        .parts
        .iter()
        .map(|p| p.designator.eq_ignore_ascii_case(designator))
        .collect();
    let mut n = 0;
    for pin in d.pins.iter_mut() {
        if !parts.get(pin.part as usize).copied().unwrap_or(false) {
            continue;
        }
        if let Some(to) = map.get(&pin.number.to_ascii_uppercase()) {
            pin.number = to.clone();
            n += 1;
        }
    }
    if n > 0 {
        d.connect();
    }
    n
}

/// Trades pin numbers `a` and `b` of part `designator` on the sheets.
/// Returns how many pins changed.
pub fn sheet_swap_pins(sheets: &mut [&mut Sheet], designator: &str, a: &str, b: &str) -> usize {
    let pairs = [(a.to_string(), b.to_string())];
    sheets
        .iter_mut()
        .map(|s| swap_numbers(&mut s.drawing, designator, &pairs))
        .sum()
}

/// Trades two drawn gates: their pins' numbers, and their designators
/// and part letters, so each drawing now stands for the other gate.
pub fn sheet_swap_gates(sheets: &mut [&mut Sheet], a: GateRef, b: GateRef) -> Result<(), String> {
    let pairs = {
        let refs: Vec<&Sheet> = sheets.iter().map(|s| &**s).collect();
        match_gates(&refs, a, b)?
    };
    let num = |s: &Sheet, i: usize| s.drawing.pins[i].number.clone();
    let nums: Vec<(String, String)> = pairs
        .iter()
        .map(|&(x, y)| (num(sheets[a.0], x), num(sheets[b.0], y)))
        .collect();
    for (&(x, y), (na, nb)) in pairs.iter().zip(&nums) {
        sheets[a.0].drawing.pins[x].number = nb.clone();
        sheets[b.0].drawing.pins[y].number = na.clone();
    }
    let qa = sheets[a.0].drawing.parts[a.1].clone();
    let qb = sheets[b.0].drawing.parts[b.1].clone();
    for (r, from, to) in [(a, &qa, &qb), (b, &qb, &qa)] {
        let d = &mut sheets[r.0].drawing;
        let (old, new) = (from.full_designator(), to.full_designator());
        for t in d.texts.iter_mut().filter(|t| t.part == Some(r.1 as u32)) {
            if t.text == old {
                t.text = new.clone();
            }
        }
        d.parts[r.1].designator = to.designator.clone();
        d.parts[r.1].gate = to.gate;
    }
    for s in sheets.iter_mut() {
        s.drawing.connect();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::*;
    use crate::*;

    fn board() -> Board {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "Top".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
        for n in ["A", "B", "C"] {
            b.nets.push(Net {
                name: n.into(),
                pieces: 1,
                ..Default::default()
            });
        }
        for d in ["U1", "R1"] {
            b.components.push(Component {
                designator: d.into(),
                footprint: "X".into(),
                library_ref: None,
                x: Length(0),
                y: Length(0),
                rotation_deg: 0.0,
                side: Side::Top,
                sheet: None,
                params: Default::default(),
            });
        }
        let pad = |c: u32, name: &str, x: f64, net: u32| Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(c),
            name: name.into(),
            at: Point::mil(x, 0.0),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        // U1.1 on A with a stub track, U1.2 on B routed to R1.1.
        b.geometry.pads.push(pad(0, "1", 0.0, 0));
        b.geometry.pads.push(pad(0, "2", 100.0, 1));
        b.geometry.pads.push(pad(1, "1", 1000.0, 1));
        let track = |x0: f64, x1: f64, y: f64, net: u32| Track {
            layer: LayerRef::Copper(0),
            net: Some(net),
            start: Point::mil(x0, y),
            end: Point::mil(x1, y),
            width: Length::from_mil(5.0),
            component: None,
            text: false,
        };
        b.geometry.tracks.push(Track {
            end: Point::mil(0.0, -200.0),
            ..track(0.0, 0.0, 0.0, 0)
        });
        b.geometry.tracks.push(track(100.0, 1000.0, 0.0, 1));
        b
    }

    #[test]
    fn pins_trade_nets_and_stubs() {
        let b = board();
        let s = swap_pads(&b, &[((0, "1".into()), (0, "2".into()))]).unwrap();
        let mut after = b.clone();
        for e in &s.edits {
            e.apply(&mut after);
        }
        let g = &after.geometry;
        assert_eq!(g.pads[0].net, Some(1));
        assert_eq!(g.pads[1].net, Some(0));
        // The stub went with pad 1; the routed track to R1 stayed.
        assert_eq!(g.tracks[0].net, Some(1));
        assert_eq!(g.tracks[1].net, Some(1));
        assert_eq!(s.left, vec!["U1.2".to_string()]);
        assert!(swap_pads(&b, &[((0, "1".into()), (0, "9".into()))]).is_err());
    }

    fn gate(s: &mut Sheet, gate: u32, x: f64, nums: [&str; 3]) {
        let i = s.drawing.parts.len();
        s.drawing.parts.push(SchPart {
            designator: "U1".into(),
            symbol: 0,
            min: [x, 0.0],
            max: [x + 200.0, 200.0],
            gate,
        });
        s.drawing.texts.push(SchText {
            at: [x, 250.0],
            text: format!("U1{}", (b'A' + gate as u8 - 1) as char),
            size: 100.0,
            color: [0, 0, 0],
            rot: 0,
            just: 0,
            bold: false,
            italic: false,
            wrap: None,
            part: Some(i as u32),
        });
        for (k, n) in nums.iter().enumerate() {
            s.drawing.pins.push(SchPin {
                at: [x - 100.0, 50.0 * k as f64],
                base: [x, 50.0 * k as f64],
                part: i as u32,
                name: ["IN+", "IN-", "OUT"][k].into(),
                number: n.to_string(),
                kind: PinKind::Passive,
            });
        }
    }

    #[test]
    fn gates_trade_pins_and_letters() {
        let mut s = Sheet::default();
        s.symbols.push(Symbol {
            designator: "U1".into(),
            library_ref: "LM358".into(),
            ..Default::default()
        });
        gate(&mut s, 1, 0.0, ["3", "2", "1"]);
        gate(&mut s, 2, 1000.0, ["5", "6", "7"]);
        let g = gates_of(&[&s], "U1");
        assert_eq!(g.len(), 2);
        let pairs = match_gates(&[&s], (0, 0), (0, 1)).unwrap();
        assert_eq!(pairs, vec![(0, 3), (1, 4), (2, 5)]);
        sheet_swap_gates(&mut [&mut s], (0, 0), (0, 1)).unwrap();
        let nums: Vec<&str> = s.drawing.pins.iter().map(|p| p.number.as_str()).collect();
        assert_eq!(nums, ["5", "6", "7", "3", "2", "1"]);
        assert_eq!(s.drawing.parts[0].gate, 2);
        assert_eq!(s.drawing.texts[0].text, "U1B");
        assert_eq!(s.drawing.texts[1].text, "U1A");
        assert_eq!(sheet_swap_pins(&mut [&mut s], "U1", "5", "6"), 2);
        assert_eq!(s.drawing.pins[0].number, "6");
    }
}
