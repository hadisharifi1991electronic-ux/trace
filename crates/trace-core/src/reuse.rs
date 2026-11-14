//! Design reuse: a block laid out once (a channel, a supply) copied onto
//! its other instances, as Altium's room copy and Allegro's place
//! replicate do. The copy's parts are matched to the block's through
//! their nets, starting from one pair of anchor parts, then placed the
//! same way about their anchor, and the block's tracks, arcs and vias are
//! copied over with their nets mapped.

use crate::edit::Edit;
use crate::place::Placement;
use crate::{Board, LayerRef, Length, Point};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// The block and its copy, part by part.
#[derive(Clone, Debug, Default)]
pub struct BlockMatch {
    /// (block part, copy part), the anchors first.
    pub pairs: Vec<(usize, usize)>,
    /// Block parts with no counterpart found.
    pub unmatched: Vec<usize>,
    /// Block net -> copy net.
    pub nets: BTreeMap<u32, u32>,
}

/// What a copy did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReuseReport {
    pub parts: usize,
    pub tracks: usize,
    pub arcs: usize,
    pub vias: usize,
    /// The copy's own routing taken up first: tracks, arcs and vias.
    pub removed: usize,
    pub unmatched: Vec<String>,
}

impl ReuseReport {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} parts placed; {} tracks, {} arcs and {} vias copied",
            self.parts, self.tracks, self.arcs, self.vias
        );
        if self.removed > 0 {
            s.push_str(&format!(
                " (the copy's own {} pieces of routing taken up first)",
                self.removed
            ));
        }
        if !self.unmatched.is_empty() {
            s.push_str(&format!(
                "; no counterpart for {}",
                self.unmatched.join(" ")
            ));
        }
        s
    }
}

fn number(d: &str) -> Option<i64> {
    let digits: String = d
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

fn prefix(d: &str) -> String {
    d.chars().take_while(|c| !c.is_ascii_digit()).collect()
}

/// Pads of each part: pin name -> net.
fn pins(board: &Board) -> Vec<BTreeMap<String, Option<u32>>> {
    let mut v = vec![BTreeMap::new(); board.components.len()];
    for p in &board.geometry.pads {
        if let Some(m) = p.component.and_then(|c| v.get_mut(c as usize)) {
            m.entry(p.name.clone()).or_insert(p.net);
        }
    }
    v
}

/// Matches the block's parts (`block`, anchor first) with those of its
/// copy, starting from `anchor`, the copy's counterpart of the block's
/// anchor.
pub fn match_block(board: &Board, block: &[usize], anchor: usize) -> Result<BlockMatch, String> {
    let n = board.components.len();
    let Some(&a0) = block.first() else {
        return Err("no parts in the block".into());
    };
    if a0 >= n || anchor >= n {
        return Err("no such part".into());
    }
    if block.contains(&anchor) {
        return Err(format!(
            "{} is in the block itself: pick a part of the copy",
            board.components[anchor].designator
        ));
    }
    let (ca, cb) = (&board.components[a0], &board.components[anchor]);
    if ca.footprint != cb.footprint {
        return Err(format!(
            "{} ({}) and {} ({}) have different footprints",
            ca.designator, ca.footprint, cb.designator, cb.footprint
        ));
    }
    let pin = pins(board);
    let in_block: HashSet<usize> = block.iter().copied().collect();
    // Parts on each net.
    let mut on_net: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, m) in pin.iter().enumerate() {
        for net in m.values().flatten() {
            let e = on_net.entry(*net).or_default();
            if e.last() != Some(&i) {
                e.push(i);
            }
        }
    }
    let offset = number(&cb.designator)
        .zip(number(&ca.designator))
        .map(|(b, a)| b - a);
    let mut m = BlockMatch::default();
    let mut used: HashSet<usize> = HashSet::new();
    let mut matched: HashMap<usize, usize> = HashMap::new();
    let mut queue = VecDeque::new();
    let add = |s: usize,
               t: usize,
               m: &mut BlockMatch,
               used: &mut HashSet<usize>,
               matched: &mut HashMap<usize, usize>,
               queue: &mut VecDeque<(usize, usize)>| {
        m.pairs.push((s, t));
        used.insert(t);
        matched.insert(s, t);
        queue.push_back((s, t));
    };
    add(a0, anchor, &mut m, &mut used, &mut matched, &mut queue);
    // Which copy part fits a block part best: same footprint and prefix,
    // the anchors' designator offset, the same pins on the mapped nets.
    let score = |s: usize, t: usize, nets: &BTreeMap<u32, u32>| -> Option<i64> {
        let (a, b) = (&board.components[s], &board.components[t]);
        if a.footprint != b.footprint {
            return None;
        }
        let mut agree = 0i64;
        for (name, net) in &pin[s] {
            if let (Some(x), Some(Some(y))) = (net, pin[t].get(name)) {
                match nets.get(x) {
                    Some(z) if z == y => agree += 10,
                    Some(_) => return None,
                    None => {}
                }
            }
        }
        if prefix(&a.designator) == prefix(&b.designator) {
            agree += 1;
        }
        if let (Some(o), Some(x), Some(y)) = (offset, number(&a.designator), number(&b.designator))
        {
            if y - x == o {
                agree += 5;
            }
        }
        Some(agree)
    };
    while let Some((s, t)) = queue.pop_front() {
        // Nets of the pair map.
        for (name, net) in &pin[s] {
            if let (Some(x), Some(Some(y))) = (net, pin[t].get(name)) {
                m.nets.entry(*x).or_insert(*y);
            }
        }
        // Block parts on the nets just mapped, not yet matched.
        let mut next: Vec<usize> = Vec::new();
        for net in pin[s].values().flatten() {
            let Some(&tn) = m.nets.get(net) else { continue };
            // A net shared by block and copy (ground, a supply) says
            // nothing about which part is which.
            if tn == *net {
                continue;
            }
            for &p in on_net.get(net).into_iter().flatten() {
                if in_block.contains(&p) && !matched.contains_key(&p) && !next.contains(&p) {
                    next.push(p);
                }
            }
        }
        for p in next {
            if matched.contains_key(&p) {
                continue;
            }
            // Candidates: parts outside the block on the copy's nets.
            let mut best: Option<(i64, usize)> = None;
            for net in pin[p].values().flatten() {
                let Some(&tn) = m.nets.get(net) else { continue };
                if tn == *net {
                    continue;
                }
                for &q in on_net.get(&tn).into_iter().flatten() {
                    if in_block.contains(&q) || used.contains(&q) {
                        continue;
                    }
                    if let Some(sc) = score(p, q, &m.nets) {
                        if best.is_none_or(|(b, _)| sc > b) {
                            best = Some((sc, q));
                        }
                    }
                }
            }
            if let Some((_, q)) = best {
                add(p, q, &mut m, &mut used, &mut matched, &mut queue);
            }
        }
    }
    // Parts joined only by shared nets (decoupling capacitors on a
    // supply): by designator offset.
    for &p in block {
        if matched.contains_key(&p) {
            continue;
        }
        let want = offset
            .zip(number(&board.components[p].designator))
            .map(|(o, x)| x + o);
        let pre = prefix(&board.components[p].designator);
        let q = (0..n).find(|&q| {
            !in_block.contains(&q)
                && !used.contains(&q)
                && board.components[q].footprint == board.components[p].footprint
                && prefix(&board.components[q].designator) == pre
                && want.is_some()
                && number(&board.components[q].designator) == want
        });
        match q {
            Some(q) => {
                m.pairs.push((p, q));
                used.insert(q);
                matched.insert(p, q);
                for (name, net) in &pin[p] {
                    if let (Some(x), Some(Some(y))) = (net, pin[q].get(name)) {
                        m.nets.entry(*x).or_insert(*y);
                    }
                }
            }
            None => m.unmatched.push(p),
        }
    }
    Ok(m)
}

/// Turns and moves points of the block onto the copy.
struct Xf {
    from: [f64; 2],
    to: [f64; 2],
    deg: f64,
    sin: f64,
    cos: f64,
}

impl Xf {
    fn new(a: Placement, b: Placement) -> Xf {
        let deg = b.rotation_deg - a.rotation_deg;
        let (sin, cos) = deg.to_radians().sin_cos();
        Xf {
            from: [a.x.0 as f64, a.y.0 as f64],
            to: [b.x.0 as f64, b.y.0 as f64],
            deg,
            sin,
            cos,
        }
    }
    fn pt(&self, p: Point) -> Point {
        let (dx, dy) = (p.x.0 as f64 - self.from[0], p.y.0 as f64 - self.from[1]);
        Point {
            x: Length((self.to[0] + dx * self.cos - dy * self.sin).round() as i64),
            y: Length((self.to[1] + dx * self.sin + dy * self.cos).round() as i64),
        }
    }
}

fn copper(l: &LayerRef) -> bool {
    matches!(l, LayerRef::Copper(_))
}

/// Places the copy's parts as the block's and copies the block's routing:
/// tracks, arcs and vias of the mapped nets lying within the block's
/// area (its pads' extent and `margin` around).
/// With `clear`, the copy's own routing in its area (tracks, arcs and
/// vias of the nets the block's map to, other than shared ones) is taken
/// up first.
pub fn replicate(
    board: &Board,
    m: &BlockMatch,
    margin: Length,
    clear: bool,
) -> (Vec<Edit>, ReuseReport) {
    let mut rep = ReuseReport {
        unmatched: m
            .unmatched
            .iter()
            .map(|&i| board.components[i].designator.clone())
            .collect(),
        ..Default::default()
    };
    let Some(&(a, b)) = m.pairs.first() else {
        return (Vec::new(), rep);
    };
    let (Some(pa), Some(pb)) = (Placement::of(board, a), Placement::of(board, b)) else {
        return (Vec::new(), rep);
    };
    if pa.side != pb.side {
        return (Vec::new(), rep);
    }
    let xf = Xf::new(pa, pb);
    let mut edits = Vec::new();
    for &(s, t) in &m.pairs {
        let (Some(ps), Some(from)) = (Placement::of(board, s), Placement::of(board, t)) else {
            continue;
        };
        let at = xf.pt(Point { x: ps.x, y: ps.y });
        let to = Placement {
            x: at.x,
            y: at.y,
            rotation_deg: (ps.rotation_deg + xf.deg).rem_euclid(360.0),
            side: ps.side,
        };
        if (to.x.0 - from.x.0).abs() > 0
            || (to.y.0 - from.y.0).abs() > 0
            || (to.rotation_deg - from.rotation_deg).abs() > 1e-6
            || to.side != from.side
        {
            edits.push(Edit::PlaceComponent { index: t, from, to });
            rep.parts += 1;
        }
    }
    // The block's area.
    let block: HashSet<u32> = m.pairs.iter().map(|&(s, _)| s as u32).collect();
    let (mut lo, mut hi) = ([i64::MAX; 2], [i64::MIN; 2]);
    for p in board
        .geometry
        .pads
        .iter()
        .filter(|p| p.component.is_some_and(|c| block.contains(&c)))
    {
        let r = p.size_x.0.max(p.size_y.0) / 2;
        lo = [lo[0].min(p.at.x.0 - r), lo[1].min(p.at.y.0 - r)];
        hi = [hi[0].max(p.at.x.0 + r), hi[1].max(p.at.y.0 + r)];
    }
    if lo[0] > hi[0] {
        return (edits, rep);
    }
    let (lo, hi) = (
        [lo[0] - margin.0, lo[1] - margin.0],
        [hi[0] + margin.0, hi[1] + margin.0],
    );
    let inside = |p: Point| p.x.0 >= lo[0] && p.x.0 <= hi[0] && p.y.0 >= lo[1] && p.y.0 <= hi[1];
    let net = |n: Option<u32>| n.and_then(|x| m.nets.get(&x).copied());
    let g = &board.geometry;
    // Nets the block shares with the rest of the board (ground, supplies)
    // map to themselves; of those only the stubs from the block's pads
    // and the vias they reach are copied, not whatever crosses the area.
    let shared = |n: Option<u32>| n.is_some_and(|x| m.nets.get(&x) == Some(&x));
    let block_pads: Vec<&crate::Pad> = g
        .pads
        .iter()
        .filter(|p| p.component.is_some_and(|c| block.contains(&c)))
        .collect();
    let on_pad = |q: Point, n: Option<u32>| {
        block_pads.iter().any(|p| {
            p.net == n
                && (q.x.0 - p.at.x.0).abs() * 2 <= p.size_x.0.max(p.size_y.0)
                && (q.y.0 - p.at.y.0).abs() * 2 <= p.size_x.0.max(p.size_y.0)
        })
    };
    let stub = |t: &crate::Track| !shared(t.net) || on_pad(t.start, t.net) || on_pad(t.end, t.net);
    // Ends of the stubs copied, for their vias.
    let ends: HashSet<(i64, i64, Option<u32>)> = g
        .tracks
        .iter()
        .filter(|t| t.component.is_none() && shared(t.net) && stub(t))
        .flat_map(|t| {
            [
                (t.start.x.0, t.start.y.0, t.net),
                (t.end.x.0, t.end.y.0, t.net),
            ]
        })
        .collect();
    // What is there already, so copying twice adds nothing.
    let key = |l: &LayerRef, a: Point, b: Point| {
        let (a, b) = if (a.x.0, a.y.0) <= (b.x.0, b.y.0) {
            (a, b)
        } else {
            (b, a)
        };
        (
            format!("{l:?}"),
            a.x.0 / 1000,
            a.y.0 / 1000,
            b.x.0 / 1000,
            b.y.0 / 1000,
        )
    };
    // The copy's area, and its own routing there.
    let corners = [
        xf.pt(Point {
            x: Length(lo[0]),
            y: Length(lo[1]),
        }),
        xf.pt(Point {
            x: Length(hi[0]),
            y: Length(lo[1]),
        }),
        xf.pt(Point {
            x: Length(hi[0]),
            y: Length(hi[1]),
        }),
        xf.pt(Point {
            x: Length(lo[0]),
            y: Length(hi[1]),
        }),
    ];
    let (tlo, thi) = (
        [
            corners.iter().map(|c| c.x.0).min().unwrap_or(0),
            corners.iter().map(|c| c.y.0).min().unwrap_or(0),
        ],
        [
            corners.iter().map(|c| c.x.0).max().unwrap_or(0),
            corners.iter().map(|c| c.y.0).max().unwrap_or(0),
        ],
    );
    let in_copy =
        |p: Point| p.x.0 >= tlo[0] && p.x.0 <= thi[0] && p.y.0 >= tlo[1] && p.y.0 <= thi[1];
    let own: HashSet<u32> = m
        .nets
        .iter()
        .filter(|(a, b)| a != b)
        .map(|(_, b)| *b)
        .collect();
    let mine = |n: Option<u32>| clear && n.is_some_and(|x| own.contains(&x));
    let gone_t: Vec<usize> = (0..g.tracks.len())
        .filter(|&i| {
            let t = &g.tracks[i];
            t.component.is_none()
                && copper(&t.layer)
                && mine(t.net)
                && in_copy(t.start)
                && in_copy(t.end)
        })
        .collect();
    let gone_a: Vec<usize> = (0..g.arcs.len())
        .filter(|&i| {
            let a = &g.arcs[i];
            a.component.is_none() && copper(&a.layer) && mine(a.net) && in_copy(a.center)
        })
        .collect();
    let gone_v: Vec<usize> = (0..g.vias.len())
        .filter(|&i| mine(g.vias[i].net) && in_copy(g.vias[i].at))
        .collect();
    for &i in gone_t.iter().rev() {
        edits.push(Edit::RemoveTrack {
            index: i,
            track: g.tracks[i].clone(),
        });
    }
    for &i in gone_a.iter().rev() {
        edits.push(Edit::RemoveArc {
            index: i,
            arc: g.arcs[i].clone(),
        });
    }
    for &i in gone_v.iter().rev() {
        edits.push(Edit::RemoveVia {
            index: i,
            via: g.vias[i].clone(),
        });
    }
    rep.removed = gone_t.len() + gone_a.len() + gone_v.len();
    let gone_tracks: HashSet<usize> = gone_t.iter().copied().collect();
    let gone_vias: HashSet<usize> = gone_v.iter().copied().collect();
    let have: HashSet<_> = g
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| !gone_tracks.contains(i))
        .map(|(_, t)| key(&t.layer, t.start, t.end))
        .collect();
    let have_vias: HashSet<_> = g
        .vias
        .iter()
        .enumerate()
        .filter(|(i, _)| !gone_vias.contains(i))
        .map(|(_, v)| (v.at.x.0 / 1000, v.at.y.0 / 1000))
        .collect();
    let mut k = g.tracks.len() - gone_t.len();
    for t in &g.tracks {
        if t.component.is_some()
            || !copper(&t.layer)
            || !inside(t.start)
            || !inside(t.end)
            || !stub(t)
        {
            continue;
        }
        let Some(n) = net(t.net) else { continue };
        let (s, e) = (xf.pt(t.start), xf.pt(t.end));
        if have.contains(&key(&t.layer, s, e)) {
            continue;
        }
        let mut c = t.clone();
        c.net = Some(n);
        c.start = s;
        c.end = e;
        edits.push(Edit::InsertTrack { index: k, track: c });
        k += 1;
        rep.tracks += 1;
    }
    let mut k = g.arcs.len() - gone_a.len();
    for a in &g.arcs {
        if a.component.is_some() || !copper(&a.layer) || !inside(a.center) || shared(a.net) {
            continue;
        }
        let Some(n) = net(a.net) else { continue };
        let mut c = a.clone();
        c.net = Some(n);
        c.center = xf.pt(a.center);
        c.start_deg = (a.start_deg + xf.deg).rem_euclid(360.0);
        c.end_deg = (a.end_deg + xf.deg).rem_euclid(360.0);
        edits.push(Edit::InsertArc { index: k, arc: c });
        k += 1;
        rep.arcs += 1;
    }
    let mut k = g.vias.len() - gone_v.len();
    for v in &g.vias {
        if !inside(v.at) || (shared(v.net) && !ends.contains(&(v.at.x.0, v.at.y.0, v.net))) {
            continue;
        }
        let Some(n) = net(v.net) else { continue };
        let at = xf.pt(v.at);
        if have_vias.contains(&(at.x.0 / 1000, at.y.0 / 1000)) {
            continue;
        }
        let mut c = v.clone();
        c.net = Some(n);
        c.at = at;
        edits.push(Edit::InsertVia { index: k, via: c });
        k += 1;
        rep.vias += 1;
    }
    (edits, rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn comp(d: &str, x: f64, rot: f64) -> Component {
        Component {
            designator: d.into(),
            footprint: if d.starts_with('U') { "SOT23" } else { "0402" }.into(),
            library_ref: None,
            x: Length::from_mil(x),
            y: Length::from_mil(0.0),
            rotation_deg: rot,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        }
    }

    fn pad(c: u32, name: &str, x: f64, y: f64, net: u32) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(c),
            name: name.into(),
            at: Point::mil(x, y),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    #[test]
    fn copies_a_channel_onto_its_twin() {
        let mut b = Board::default();
        // Nets: 0 GND, 1 IN1, 2 OUT1, 3 IN2, 4 OUT2.
        for n in ["GND", "IN1", "OUT1", "IN2", "OUT2"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        // Channel 1 laid out: U1 at 0, R1 to its right joined by OUT1.
        b.components.push(comp("U1", 0.0, 0.0));
        b.components.push(comp("R1", 200.0, 90.0));
        // Channel 2 piled up anywhere: U2 at 1000 (anchor), R2 at 5000.
        b.components.push(comp("U2", 1000.0, 0.0));
        b.components.push(comp("R2", 5000.0, 0.0));
        // C1/C2 on GND only: matched by designator offset.
        b.components.push(comp("C1", 0.0, 0.0));
        b.components.push(comp("C2", 7000.0, 0.0));
        for (c, x0, inn, out) in [(0u32, 0.0, 1u32, 2u32), (2, 1000.0, 3, 4)] {
            b.geometry.pads.push(pad(c, "1", x0 - 30.0, 0.0, inn));
            b.geometry.pads.push(pad(c, "2", x0 + 30.0, 0.0, out));
            b.geometry.pads.push(pad(c, "3", x0, -40.0, 0));
        }
        b.geometry.pads.push(pad(1, "1", 200.0, 30.0, 2));
        b.geometry.pads.push(pad(1, "2", 200.0, -30.0, 0));
        b.geometry.pads.push(pad(3, "1", 5000.0, 30.0, 4));
        b.geometry.pads.push(pad(3, "2", 5000.0, -30.0, 0));
        b.geometry.pads.push(pad(4, "1", 0.0, 100.0, 0));
        b.geometry.pads.push(pad(4, "2", 30.0, 100.0, 0));
        b.geometry.pads.push(pad(5, "1", 7000.0, 100.0, 0));
        b.geometry.pads.push(pad(5, "2", 7030.0, 100.0, 0));
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(2),
            start: Point::mil(30.0, 0.0),
            end: Point::mil(200.0, 30.0),
            width: Length::from_mil(8.0),
            component: None,
            text: false,
        });
        let m = match_block(&b, &[0, 1, 4], 2).unwrap();
        assert_eq!(m.pairs, vec![(0, 2), (1, 3), (4, 5)]);
        assert!(m.unmatched.is_empty());
        assert_eq!(m.nets.get(&2), Some(&4));
        assert_eq!(m.nets.get(&0), Some(&0));
        let (edits, rep) = replicate(&b, &m, Length::from_mil(50.0), true);
        assert_eq!((rep.parts, rep.tracks), (2, 1));
        for e in &edits {
            e.apply(&mut b);
        }
        assert_eq!(b.components[3].x, Length::from_mil(1200.0));
        assert_eq!(b.components[3].rotation_deg, 90.0);
        let t = b.geometry.tracks.last().unwrap();
        assert_eq!(t.net, Some(4));
        assert_eq!(t.start, Point::mil(1030.0, 0.0));
        // Copying again adds nothing.
        let (_, again) = replicate(
            &b,
            &match_block(&b, &[0, 1, 4], 2).unwrap(),
            Length::from_mil(50.0),
            false,
        );
        assert_eq!((again.parts, again.tracks), (0, 0));
        // Taking up the copy's routing first replaces it.
        let (_, redo) = replicate(
            &b,
            &match_block(&b, &[0, 1, 4], 2).unwrap(),
            Length::from_mil(50.0),
            true,
        );
        assert_eq!((redo.removed, redo.tracks), (1, 1));
        assert!(match_block(&b, &[0, 1], 1).is_err());
    }
}
