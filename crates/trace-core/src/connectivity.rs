//! Works out which copper pieces of each net touch, so we can tell whether a
//! net is fully routed and how much via barrel its signal really travels.

use crate::{Board, LayerRef, Length, PadShape};
use std::collections::HashMap;

/// A copper shape reduced to a capsule: a segment swept by a radius, in mil.
#[derive(Clone, Copy)]
struct Capsule {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    r: f64,
}

impl Capsule {
    fn point(x: f64, y: f64, r: f64) -> Self {
        Capsule {
            x1: x,
            y1: y,
            x2: x,
            y2: y,
            r,
        }
    }
    fn bbox(&self) -> (f64, f64, f64, f64) {
        (
            self.x1.min(self.x2) - self.r,
            self.y1.min(self.y2) - self.r,
            self.x1.max(self.x2) + self.r,
            self.y1.max(self.y2) + self.r,
        )
    }
    fn touches(&self, o: &Capsule) -> bool {
        seg_seg_dist(self, o) <= self.r + o.r + 0.05
    }
}

fn point_seg(px: f64, py: f64, s: &Capsule) -> f64 {
    let (dx, dy) = (s.x2 - s.x1, s.y2 - s.y1);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 {
        (((px - s.x1) * dx + (py - s.y1) * dy) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - (s.x1 + t * dx)).hypot(py - (s.y1 + t * dy))
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

fn seg_seg_dist(a: &Capsule, b: &Capsule) -> f64 {
    // Proper crossing means distance zero.
    let d1 = cross(b.x2 - b.x1, b.y2 - b.y1, a.x1 - b.x1, a.y1 - b.y1);
    let d2 = cross(b.x2 - b.x1, b.y2 - b.y1, a.x2 - b.x1, a.y2 - b.y1);
    let d3 = cross(a.x2 - a.x1, a.y2 - a.y1, b.x1 - a.x1, b.y1 - a.y1);
    let d4 = cross(a.x2 - a.x1, a.y2 - a.y1, b.x2 - a.x1, b.y2 - a.y1);
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    point_seg(a.x1, a.y1, b)
        .min(point_seg(a.x2, a.y2, b))
        .min(point_seg(b.x1, b.y1, a))
        .min(point_seg(b.x2, b.y2, a))
}

struct UnionFind(Vec<usize>);
impl UnionFind {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Wire,
    Via(usize),
    Pad,
    Area,
}

struct Feature {
    item: usize,
    layer: u8,
    shape: Capsule,
    kind: Kind,
}

const CELL: f64 = 100.0;

/// Routed length and via count per net, from copper tracks and arcs.
fn lengths(board: &mut Board) {
    let count = board.nets.len();
    let mut length_mil = vec![0.0f64; count];
    let mut vias = vec![0usize; count];
    let slot = |n: Option<u32>| n.map(|n| n as usize).filter(|&n| n < count);
    let g = &board.geometry;
    for t in g
        .tracks
        .iter()
        .filter(|t| matches!(t.layer, LayerRef::Copper(_)))
    {
        if let Some(n) = slot(t.net) {
            length_mil[n] +=
                (t.end.x.mil() - t.start.x.mil()).hypot(t.end.y.mil() - t.start.y.mil());
        }
    }
    for a in g
        .arcs
        .iter()
        .filter(|a| matches!(a.layer, LayerRef::Copper(_)))
    {
        if let Some(n) = slot(a.net) {
            let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
            if sweep == 0.0 {
                sweep = 360.0;
            }
            length_mil[n] += a.radius.mil() * sweep.to_radians();
        }
    }
    for v in &g.vias {
        if let Some(n) = slot(v.net) {
            vias[n] += 1;
        }
    }
    for ((net, mil), v) in board.nets.iter_mut().zip(length_mil).zip(vias) {
        net.routed_length = Length::from_mil(mil);
        net.vias = v;
    }
}

/// Recomputes every net's routed length, vias, `pieces`, `pads` and
/// `via_length`. Call after any change to the geometry.
pub fn analyse(board: &mut Board) {
    lengths(board);
    let copper = board.stackup.copper_layers().count() as u8;
    if copper == 0 {
        return;
    }
    let depth = board.stackup.copper_depths();
    let g = &board.geometry;
    let layers_of = |l: &LayerRef| -> Vec<u8> {
        match l {
            LayerRef::Copper(i) => vec![*i],
            LayerRef::AllCopper => (0..copper).collect(),
            _ => vec![],
        }
    };

    // Features grouped by net; each source item gets one union-find slot.
    let mut by_net: HashMap<u32, (Vec<Feature>, usize)> = HashMap::new();
    let mut add = |net: Option<u32>, layers: Vec<u8>, shapes: Vec<Capsule>, kind: Kind| -> usize {
        let Some(net) = net else { return 0 };
        if layers.is_empty() {
            return 0;
        }
        let entry = by_net.entry(net).or_insert_with(|| (Vec::new(), 0));
        let item = entry.1;
        entry.1 += 1;
        for &layer in &layers {
            for &shape in &shapes {
                entry.0.push(Feature {
                    item,
                    layer,
                    shape,
                    kind,
                });
            }
        }
        item
    };

    for t in &g.tracks {
        let c = Capsule {
            x1: t.start.x.mil(),
            y1: t.start.y.mil(),
            x2: t.end.x.mil(),
            y2: t.end.y.mil(),
            r: t.width.mil() / 2.0,
        };
        add(t.net, layers_of(&t.layer), vec![c], Kind::Wire);
    }
    for a in &g.arcs {
        // Flatten to short chords.
        let (cx, cy, r) = (a.center.x.mil(), a.center.y.mil(), a.radius.mil());
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let n = (sweep / 15.0).ceil().max(1.0) as usize;
        let pt = |i: usize| {
            let t = (a.start_deg + sweep * i as f64 / n as f64).to_radians();
            (cx + r * t.cos(), cy + r * t.sin())
        };
        let shapes = (0..n)
            .map(|i| {
                let ((x1, y1), (x2, y2)) = (pt(i), pt(i + 1));
                Capsule {
                    x1,
                    y1,
                    x2,
                    y2,
                    r: a.width.mil() / 2.0,
                }
            })
            .collect();
        add(a.net, layers_of(&a.layer), shapes, Kind::Wire);
    }
    for (vi, v) in g.vias.iter().enumerate() {
        let (lo, hi) = (
            v.from_layer.min(v.to_layer),
            v.from_layer.max(v.to_layer).min(copper - 1),
        );
        add(
            v.net,
            (lo..=hi).collect(),
            vec![Capsule::point(
                v.at.x.mil(),
                v.at.y.mil(),
                v.diameter.mil() / 2.0,
            )],
            Kind::Via(vi),
        );
    }
    for p in &g.pads {
        let (x, y, sx, sy) = (p.at.x.mil(), p.at.y.mil(), p.size_x.mil(), p.size_y.mil());
        let shape = if (sx - sy).abs() < 1e-6 {
            Capsule::point(x, y, sx.max(sy) / 2.0)
        } else {
            // Long axis as the spine; rectangles keep their corners covered.
            let (long, short) = (sx.max(sy), sx.min(sy));
            let ang = (p.rotation_deg + if sx >= sy { 0.0 } else { 90.0 }).to_radians();
            let half = (long - short) / 2.0;
            let r = if p.shape == PadShape::Round {
                short / 2.0
            } else {
                short / 2.0 * std::f64::consts::SQRT_2
            };
            Capsule {
                x1: x - half * ang.cos(),
                y1: y - half * ang.sin(),
                x2: x + half * ang.cos(),
                y2: y + half * ang.sin(),
                r,
            }
        };
        add(p.net, layers_of(&p.layer), vec![shape], Kind::Pad);
    }

    // Copper areas (regions and poured fills): their edges join what they
    // cross; what sits inside them is joined in a second pass below.
    let mut areas: HashMap<u32, Vec<(usize, u8, usize)>> = HashMap::new();
    for (pi, p) in g.polygons.iter().enumerate() {
        let (Some(net), &LayerRef::Copper(layer)) = (p.net, &p.layer) else {
            continue;
        };
        if p.outline.len() < 3 {
            continue;
        }
        let ring = |r: &[crate::Point]| -> Vec<Capsule> {
            (0..r.len())
                .map(|i| {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    Capsule {
                        x1: a.x.mil(),
                        y1: a.y.mil(),
                        x2: b.x.mil(),
                        y2: b.y.mil(),
                        r: 0.0,
                    }
                })
                .collect()
        };
        let mut shapes = ring(&p.outline);
        for h in &p.holes {
            shapes.extend(ring(h));
        }
        let item = add(Some(net), vec![layer], shapes, Kind::Area);
        areas.entry(net).or_default().push((item, layer, pi));
    }
    let inside = |x: f64, y: f64, ring: &[crate::Point]| {
        let mut c = false;
        let mut j = ring.len() - 1;
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[j]);
            let (ax, ay, bx, by) = (a.x.mil(), a.y.mil(), b.x.mil(), b.y.mil());
            if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
                c = !c;
            }
            j = i;
        }
        c
    };

    let mut via_layers: HashMap<usize, (u8, u8)> = HashMap::new();
    let mut results: HashMap<u32, (usize, usize)> = HashMap::new(); // net -> (pieces, pads)
    for (net, (features, items)) in &by_net {
        let mut uf = UnionFind((0..*items).collect());
        let mut grid: HashMap<(u8, i64, i64), Vec<usize>> = HashMap::new();
        for (fi, f) in features.iter().enumerate() {
            let (x0, y0, x1, y1) = f.shape.bbox();
            for gx in (x0 / CELL).floor() as i64..=(x1 / CELL).floor() as i64 {
                for gy in (y0 / CELL).floor() as i64..=(y1 / CELL).floor() as i64 {
                    let cell = grid.entry((f.layer, gx, gy)).or_default();
                    for &other in cell.iter() {
                        let o = &features[other];
                        if o.item != f.item && f.shape.touches(&o.shape) {
                            uf.union(f.item, o.item);
                            for (via, layer) in [(f.kind, o.layer), (o.kind, f.layer)] {
                                if let Kind::Via(vi) = via {
                                    let e = via_layers.entry(vi).or_insert((layer, layer));
                                    *e = (e.0.min(layer), e.1.max(layer));
                                }
                            }
                        }
                    }
                    cell.push(fi);
                }
            }
        }
        for &(area, layer, pi) in areas.get(net).map(Vec::as_slice).unwrap_or(&[]) {
            let p = &g.polygons[pi];
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for q in &p.outline {
                x0 = x0.min(q.x.mil());
                y0 = y0.min(q.y.mil());
                x1 = x1.max(q.x.mil());
                y1 = y1.max(q.y.mil());
            }
            for f in features
                .iter()
                .filter(|f| f.layer == layer && f.kind != Kind::Area)
            {
                let (x, y) = (f.shape.x1, f.shape.y1);
                if x < x0 || x > x1 || y < y0 || y > y1 || uf.find(f.item) == uf.find(area) {
                    continue;
                }
                if inside(x, y, &p.outline) && !p.holes.iter().any(|h| inside(x, y, h)) {
                    uf.union(f.item, area);
                }
            }
        }
        let mut pad_roots: Vec<usize> = features
            .iter()
            .filter(|f| f.kind == Kind::Pad)
            .map(|f| f.item)
            .collect();
        pad_roots.sort_unstable();
        pad_roots.dedup();
        let pads = pad_roots.len();
        let mut roots: Vec<usize> = pad_roots.into_iter().map(|i| uf.find(i)).collect();
        roots.sort_unstable();
        roots.dedup();
        results.insert(*net, (roots.len(), pads));
    }

    let mut barrel = vec![0i64; board.nets.len()];
    for (vi, (lo, hi)) in via_layers {
        let v = &board.geometry.vias[vi];
        if let (Some(n), Some(a), Some(b)) = (v.net, depth.get(lo as usize), depth.get(hi as usize))
        {
            if let Some(slot) = barrel.get_mut(n as usize) {
                *slot += (b.0 - a.0).abs();
            }
        }
    }
    for (i, net) in board.nets.iter_mut().enumerate() {
        let (pieces, pads) = results.get(&(i as u32)).copied().unwrap_or((0, 0));
        net.pieces = pieces;
        net.pads = pads;
        net.via_length = Length(barrel[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn pt(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        }
    }
    fn pad(x: f64) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: None,
            name: "1".into(),
            at: pt(x, 0.0),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(20.0),
            hole: Length(0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
        }
    }
    fn track(layer: u8, x1: f64, x2: f64) -> Track {
        Track {
            component: None,
            layer: LayerRef::Copper(layer),
            net: Some(0),
            start: pt(x1, 0.0),
            end: pt(x2, 0.0),
            width: Length::from_mil(5.0),
        }
    }
    fn board(tracks: Vec<Track>, vias: Vec<Via>) -> Board {
        let layer = |name: &str, kind, mil| StackLayer {
            name: name.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(mil),
            dielectric_constant: None,
            material: None,
        };
        let mut b = Board::default();
        b.stackup.layers = vec![
            layer("Top", LayerKind::Copper, 2.0),
            layer("Core", LayerKind::Dielectric, 10.0),
            layer("Bot", LayerKind::Copper, 2.0),
        ];
        b.nets = vec![Net {
            name: "N".into(),
            ..Default::default()
        }];
        b.geometry.pads = vec![pad(0.0), pad(100.0)];
        b.geometry.tracks = tracks;
        b.geometry.vias = vias;
        analyse(&mut b);
        b
    }

    #[test]
    fn joined_net_is_one_piece() {
        let b = board(vec![track(0, 0.0, 60.0), track(0, 60.0, 100.0)], vec![]);
        assert_eq!((b.nets[0].pads, b.nets[0].pieces), (2, 1));
        assert!(b.nets[0].is_complete());
    }

    #[test]
    fn gap_leaves_net_open() {
        let b = board(vec![track(0, 0.0, 40.0), track(0, 60.0, 100.0)], vec![]);
        assert_eq!(b.nets[0].pieces, 2);
        assert!(!b.nets[0].is_complete());
    }

    #[test]
    fn via_joins_layers_and_counts_barrel() {
        let via = Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: pt(50.0, 0.0),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: Vec::new(),
        };
        // Top 0..50, via down, bottom 50..100. The far pad is on top only.
        let b = board(
            vec![
                track(0, 0.0, 50.0),
                track(1, 50.0, 100.0),
                track(0, 100.0, 100.0),
            ],
            vec![via],
        );
        // The bottom track ends under the far pad but on another layer: still open...
        assert_eq!(b.nets[0].pieces, 2);
        // ...but the via is used on both layers: barrel = centre to centre = 1 + 10 + 1 mil.
        assert!((b.nets[0].via_length.mil() - 12.0).abs() < 1e-6);
    }
}
