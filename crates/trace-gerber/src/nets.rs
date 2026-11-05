//! Finds nets in bare copper: objects that touch on a layer are joined, and
//! plated holes join the layers they pass through.

use crate::geom::{self, Pt};
use std::collections::HashMap;
use trace_core::{Board, LayerRef, Net, PadShape};

/// Copper closer than this is treated as touching (mm).
const EPS: f64 = 0.001;

enum Geo {
    Cap { a: Pt, b: Pt, r: f64 },
    Poly(PolyIx),
}

struct Feature {
    item: usize,
    geo: Geo,
    bbox: [f64; 4],
}

/// A polygon with holes, indexed for fast edge and inside tests.
struct PolyIx {
    edges: Vec<(Pt, Pt)>,
    first: Pt,
    origin: Pt,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
    bands: Vec<Vec<u32>>,
}

impl PolyIx {
    fn new(rings: &[Vec<Pt>]) -> Self {
        let mut edges = Vec::new();
        for r in rings {
            for i in 0..r.len() {
                edges.push((r[i], r[(i + 1) % r.len()]));
            }
        }
        let all: Vec<Pt> = rings.iter().flatten().copied().collect();
        let bb = geom::bbox(&all);
        let (w, h) = ((bb[2] - bb[0]).max(1e-6), (bb[3] - bb[1]).max(1e-6));
        let n = ((edges.len() as f64).sqrt() / 2.0).clamp(1.0, 256.0);
        let cell = (w.max(h) / n).max(0.05);
        let nx = (w / cell).floor() as usize + 1;
        let ny = (h / cell).floor() as usize + 1;
        let mut cells = vec![Vec::new(); nx * ny];
        let mut bands = vec![Vec::new(); ny];
        for (k, (a, b)) in edges.iter().enumerate() {
            let x0 = (((a[0].min(b[0]) - bb[0]) / cell).floor() as usize).min(nx - 1);
            let x1 = (((a[0].max(b[0]) - bb[0]) / cell).floor() as usize).min(nx - 1);
            let y0 = (((a[1].min(b[1]) - bb[1]) / cell).floor() as usize).min(ny - 1);
            let y1 = (((a[1].max(b[1]) - bb[1]) / cell).floor() as usize).min(ny - 1);
            for y in y0..=y1 {
                bands[y].push(k as u32);
                for x in x0..=x1 {
                    cells[y * nx + x].push(k as u32);
                }
            }
        }
        PolyIx {
            edges,
            first: all.first().copied().unwrap_or([0.0, 0.0]),
            origin: [bb[0], bb[1]],
            cell,
            nx,
            ny,
            cells,
            bands,
        }
    }

    fn inside(&self, p: Pt) -> bool {
        let y = ((p[1] - self.origin[1]) / self.cell).floor();
        if y < 0.0 || y as usize >= self.ny {
            return false;
        }
        let mut odd = false;
        for &k in &self.bands[y as usize] {
            let (a, b) = self.edges[k as usize];
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
                odd = !odd;
            }
        }
        odd
    }

    /// True when an edge comes within `r` of the segment `a`–`b`.
    fn near(&self, a: Pt, b: Pt, r: f64) -> bool {
        let cx = |v: f64| ((v - self.origin[0]) / self.cell).floor().clamp(0.0, (self.nx - 1) as f64) as usize;
        let cy = |v: f64| ((v - self.origin[1]) / self.cell).floor().clamp(0.0, (self.ny - 1) as f64) as usize;
        let (x0, x1) = (cx(a[0].min(b[0]) - r), cx(a[0].max(b[0]) + r));
        let (y0, y1) = (cy(a[1].min(b[1]) - r), cy(a[1].max(b[1]) + r));
        for y in y0..=y1 {
            for x in x0..=x1 {
                for &k in &self.cells[y * self.nx + x] {
                    let (e0, e1) = self.edges[k as usize];
                    if geom::seg_seg(a, b, e0, e1) <= r {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn touches(f: &Feature, g: &Feature) -> bool {
    match (&f.geo, &g.geo) {
        (Geo::Cap { a, b, r }, Geo::Cap { a: c, b: d, r: s }) => geom::seg_seg(*a, *b, *c, *d) <= r + s + EPS,
        (Geo::Cap { a, b, r }, Geo::Poly(p)) | (Geo::Poly(p), Geo::Cap { a, b, r }) => p.near(*a, *b, r + EPS) || p.inside(*a),
        (Geo::Poly(p), Geo::Poly(q)) => {
            let (small, big) = if p.edges.len() <= q.edges.len() { (p, q) } else { (q, p) };
            small.edges.iter().any(|(a, b)| big.near(*a, *b, EPS)) || big.inside(small.first) || small.inside(big.first)
        }
    }
}

struct Uf(Vec<usize>);

impl Uf {
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
            self.0[a.max(b)] = a.min(b);
        }
    }
}

fn mm(p: &trace_core::Point) -> Pt {
    [p.x.mm(), p.y.mm()]
}

fn cap_bbox(a: Pt, b: Pt, r: f64) -> [f64; 4] {
    [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[0].max(b[0]) + r, a[1].max(b[1]) + r]
}

fn pad_geo(p: &trace_core::Pad) -> Geo {
    let (x, y, sx, sy) = (p.at.x.mm(), p.at.y.mm(), p.size_x.mm(), p.size_y.mm());
    match p.shape {
        PadShape::Rect => {
            let ring: Vec<Pt> = geom::rect([0.0, 0.0], sx, sy)
                .into_iter()
                .map(|q| geom::add(geom::rotate(q, p.rotation_deg), [x, y]))
                .collect();
            Geo::Poly(PolyIx::new(&[ring]))
        }
        _ => {
            let (long, short) = (sx.max(sy), sx.min(sy));
            let ang = (p.rotation_deg + if sx >= sy { 0.0 } else { 90.0 }).to_radians();
            let half = (long - short) / 2.0;
            Geo::Cap {
                a: [x - half * ang.cos(), y - half * ang.sin()],
                b: [x + half * ang.cos(), y + half * ang.sin()],
                r: short / 2.0,
            }
        }
    }
}

fn geo_bbox(g: &Geo, fallback: [f64; 4]) -> [f64; 4] {
    match g {
        Geo::Cap { a, b, r } => cap_bbox(*a, *b, *r),
        Geo::Poly(_) => fallback,
    }
}

fn report_short(a: Option<String>, b: Option<String>, f: &Feature, g: &Feature) {
    if let (Some(a), Some(b)) = (a, b) {
        if a != b {
            let c = |x: &Feature| [(x.bbox[0] + x.bbox[2]) / 2.0, (x.bbox[1] + x.bbox[3]) / 2.0];
            let kind = |x: &Feature| match x.geo {
                Geo::Cap { .. } => "line/pad",
                Geo::Poly(_) => "polygon",
            };
            eprintln!("short: {a} ({} at {:.3?}) touches {b} ({} at {:.3?})", kind(f), c(f), kind(g), c(g));
        }
    }
}

/// Extra shape facts the board model does not hold.
#[derive(Debug, Default, Clone)]
pub struct Extra {
    /// Per via: pad radius on each layer it passes through.
    pub via_rings: Vec<Vec<(u8, f64)>>,
    /// Per through-hole pad: ring radius on each layer.
    pub pad_rings: HashMap<usize, Vec<(u8, f64)>>,
    /// Exact copper of irregular pads.
    pub exact: HashMap<usize, Vec<Vec<Pt>>>,
    /// The land of a through-hole pad on each layer that had one.
    pub layer_pads: HashMap<(usize, u8), (trace_core::Pad, Option<Vec<Vec<Pt>>>)>,
}

/// Gives every connected group of copper (two or more objects) a net.
pub fn trace(board: &mut Board, extra: &Extra) {
    let (via_rings, pad_rings) = (&extra.via_rings, &extra.pad_rings);
    let copper = board.stackup.copper_layers().count() as u8;
    let g = &board.geometry;
    let (nt, na, np, npo) = (g.tracks.len(), g.arcs.len(), g.pads.len(), g.polygons.len());
    let total = nt + na + np + npo + g.vias.len();
    let mut layers: Vec<Vec<Feature>> = (0..copper).map(|_| Vec::new()).collect();
    let push = |layer: u8, item: usize, geo: Geo, bbox: [f64; 4], layers: &mut Vec<Vec<Feature>>| {
        if let Some(l) = layers.get_mut(layer as usize) {
            l.push(Feature { item, geo, bbox });
        }
    };
    let copper_of = |l: &LayerRef| match l {
        LayerRef::Copper(i) => Some(*i),
        _ => None,
    };
    for (i, t) in g.tracks.iter().enumerate() {
        if let Some(l) = copper_of(&t.layer) {
            let (a, b, r) = (mm(&t.start), mm(&t.end), t.width.mm() / 2.0);
            push(l, i, Geo::Cap { a, b, r }, cap_bbox(a, b, r), &mut layers);
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        let Some(l) = copper_of(&a.layer) else { continue };
        let (c, rad, w) = (mm(&a.center), a.radius.mm(), a.width.mm() / 2.0);
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let n = (sweep / 10.0).ceil().max(1.0) as usize;
        let p = |k: usize| {
            let t = (a.start_deg + sweep * k as f64 / n as f64).to_radians();
            [c[0] + rad * t.cos(), c[1] + rad * t.sin()]
        };
        for k in 0..n {
            let (s, e) = (p(k), p(k + 1));
            push(l, nt + i, Geo::Cap { a: s, b: e, r: w }, cap_bbox(s, e, w), &mut layers);
        }
    }
    for (i, p) in g.pads.iter().enumerate() {
        let item = nt + na + i;
        match &p.layer {
            LayerRef::Copper(l) => {
                if let Some(rings) = extra.exact.get(&i) {
                    let all: Vec<Pt> = rings.iter().flatten().copied().collect();
                    let bb = geom::bbox(&all);
                    push(*l, item, Geo::Poly(PolyIx::new(rings)), bb, &mut layers);
                    continue;
                }
                let geo = pad_geo(p);
                let r = p.size_x.mm().max(p.size_y.mm());
                let bb = geo_bbox(&geo, cap_bbox(mm(&p.at), mm(&p.at), r));
                push(*l, item, geo, bb, &mut layers);
            }
            LayerRef::AllCopper if p.name != "NPTH" => {
                let rings = pad_rings.get(&i);
                let (lo, hi) = match rings {
                    Some(r) if !r.is_empty() => (r.iter().map(|x| x.0).min().unwrap(), r.iter().map(|x| x.0).max().unwrap()),
                    _ => (0, copper.saturating_sub(1)),
                };
                for l in lo..=hi {
                    let ring_r = rings.and_then(|r| r.iter().find(|x| x.0 == l)).map(|x| x.1);
                    if let Some((land, exact)) = extra.layer_pads.get(&(i, l)) {
                        if let Some(rs) = exact {
                            let all: Vec<Pt> = rs.iter().flatten().copied().collect();
                            push(l, item, Geo::Poly(PolyIx::new(rs)), geom::bbox(&all), &mut layers);
                        } else {
                            let geo = pad_geo(land);
                            let r = land.size_x.mm().max(land.size_y.mm());
                            let bb = geo_bbox(&geo, cap_bbox(mm(&land.at), mm(&land.at), r));
                            push(l, item, geo, bb, &mut layers);
                        }
                    } else if rings.is_none() && (l == lo || l == hi) {
                        let geo = pad_geo(p);
                        let r = p.size_x.mm().max(p.size_y.mm());
                        let bb = geo_bbox(&geo, cap_bbox(mm(&p.at), mm(&p.at), r));
                        push(l, item, geo, bb, &mut layers);
                    } else {
                        let r = ring_r.unwrap_or(p.hole.mm() / 2.0);
                        let c = mm(&p.at);
                        push(l, item, Geo::Cap { a: c, b: c, r }, cap_bbox(c, c, r), &mut layers);
                    }
                }
            }
            _ => {}
        }
    }
    for (i, poly) in g.polygons.iter().enumerate() {
        let Some(l) = copper_of(&poly.layer) else { continue };
        let mut rings: Vec<Vec<Pt>> = vec![poly.outline.iter().map(mm).collect()];
        rings.extend(poly.holes.iter().map(|h| h.iter().map(mm).collect()));
        let bb = geom::bbox(&rings[0]);
        push(l, nt + na + np + i, Geo::Poly(PolyIx::new(&rings)), bb, &mut layers);
    }
    for (i, v) in g.vias.iter().enumerate() {
        let item = nt + na + np + npo + i;
        let c = mm(&v.at);
        let default: Vec<(u8, f64)> = (v.from_layer..=v.to_layer).map(|l| (l, v.diameter.mm() / 2.0)).collect();
        let rings = via_rings.get(i).filter(|r| !r.is_empty()).unwrap_or(&default);
        for &(l, r) in rings {
            push(l, item, Geo::Cap { a: c, b: c, r }, cap_bbox(c, c, r), &mut layers);
        }
    }

    let mut uf = Uf((0..total).collect());
    // With TRACE_NET_DEBUG set on a board that already has nets, report
    // every join between two different nets.
    let debug = std::env::var_os("TRACE_NET_DEBUG").is_some();
    let existing: Vec<Option<String>> = if debug {
        let name = |n: Option<u32>| n.map(|n| board.nets[n as usize].name.clone());
        let mut v: Vec<Option<String>> = Vec::with_capacity(total);
        v.extend(g.tracks.iter().map(|x| name(x.net)));
        v.extend(g.arcs.iter().map(|x| name(x.net)));
        v.extend(g.pads.iter().map(|x| name(x.net)));
        v.extend(g.polygons.iter().map(|x| name(x.net)));
        v.extend(g.vias.iter().map(|x| name(x.net)));
        v
    } else {
        Vec::new()
    };
    let board_nets = |i: usize| existing.get(i).cloned().flatten();
    const CELL: f64 = 1.0;
    for feats in &layers {
        let mut grid: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        let mut stamp = vec![usize::MAX; feats.len()];
        for (fi, f) in feats.iter().enumerate() {
            let b = f.bbox;
            let (x0, x1) = ((b[0] / CELL).floor() as i64, (b[2] / CELL).floor() as i64);
            let (y0, y1) = ((b[1] / CELL).floor() as i64, (b[3] / CELL).floor() as i64);
            for gx in x0..=x1 {
                for gy in y0..=y1 {
                    let cell = grid.entry((gx, gy)).or_default();
                    for &o in cell.iter() {
                        let o = o as usize;
                        if stamp[o] == fi {
                            continue;
                        }
                        stamp[o] = fi;
                        let other = &feats[o];
                        if other.item == f.item || !geom::bbox_overlap(other.bbox, f.bbox) {
                            continue;
                        }
                        if uf.find(other.item) == uf.find(f.item) {
                            continue;
                        }
                        if touches(f, other) {
                            if debug {
                                report_short(board_nets(f.item), board_nets(other.item), f, other);
                            }
                            uf.union(f.item, other.item);
                        }
                    }
                    cell.push(fi as u32);
                }
            }
        }
    }

    // Items present on copper, grouped by root.
    let mut on_copper = vec![false; total];
    for feats in &layers {
        for f in feats {
            on_copper[f.item] = true;
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..total {
        if on_copper[i] {
            let r = uf.find(i);
            groups.entry(r).or_default().push(i);
        }
    }
    let mut list: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() >= 2).collect();
    list.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let width = list.len().to_string().len();
    board.nets.clear();
    let geo = &mut board.geometry;
    for (n, items) in list.iter().enumerate() {
        board.nets.push(Net {
            name: format!("Net-{:0width$}", n + 1),
            ..Default::default()
        });
        let id = Some(n as u32);
        for &i in items {
            if i < nt {
                geo.tracks[i].net = id;
            } else if i < nt + na {
                geo.arcs[i - nt].net = id;
            } else if i < nt + na + np {
                geo.pads[i - nt - na].net = id;
            } else if i < nt + na + np + npo {
                geo.polygons[i - nt - na - np].net = id;
            } else {
                geo.vias[i - nt - na - np - npo].net = id;
            }
        }
    }
}
