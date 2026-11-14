//! Power integrity: DC voltage drop (IR drop) on a supply net.
//!
//! The net's copper on each layer (pours, pads) is laid on a square grid,
//! each cell a node joined to its neighbours by the copper's sheet
//! conductance; tracks join the cells along them with the conductance of
//! their own width, so tracks narrower than a cell keep their resistance;
//! vias join the layers they reach through their plated barrels. The
//! source part's pads are held at the supply voltage, each load draws its
//! current evenly from its pads, and the network is solved by conjugate
//! gradients. The return path (ground) is taken as ideal.

#![allow(clippy::needless_range_loop)]

use crate::{Board, LayerKind, LayerRef, Pad, PadShape};
use std::collections::HashMap;

/// Copper resistivity at 20 °C, Ω·m.
pub const RHO: f64 = 1.72e-8;
const MIL: f64 = 25.4e-6;

/// What to solve.
#[derive(Clone, Debug)]
pub struct Setup {
    pub net: u32,
    /// The part feeding the net (regulator or its output inductor).
    pub source: usize,
    pub volts: f64,
    /// Loads: part index and current drawn, A.
    pub loads: Vec<(usize, f64)>,
    /// Plating thickness of via barrels, mil.
    pub plating_mil: f64,
    /// Largest grid size, mil; finer where the copper is small.
    pub max_cell_mil: f64,
}

impl Setup {
    pub fn new(net: u32, source: usize, volts: f64) -> Setup {
        Setup {
            net,
            source,
            volts,
            loads: Vec::new(),
            plating_mil: 0.8,
            max_cell_mil: 10.0,
        }
    }
}

/// One load's result.
#[derive(Clone, Debug)]
pub struct LoadResult {
    pub part: usize,
    pub amps: f64,
    /// Lowest and mean voltage at its pads, V.
    pub v_min: f64,
    pub v_mean: f64,
    /// False when none of its pads joins the source through the net.
    pub connected: bool,
}

/// The voltage over one layer's copper.
#[derive(Clone, Debug)]
pub struct LayerMap {
    pub layer: u8,
    pub name: String,
    /// Voltage per cell, row by row from the bottom left; NaN where the
    /// layer has none of the net's copper.
    pub v: Vec<f32>,
}

/// The solved net.
#[derive(Clone, Debug)]
pub struct Result {
    pub loads: Vec<LoadResult>,
    pub maps: Vec<LayerMap>,
    /// Grid: lower left corner (mil), cell size (mil), columns, rows.
    pub origin: [f64; 2],
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub volts: f64,
    /// Largest drop anywhere, V.
    pub max_drop: f64,
    /// Power lost in the copper, W.
    pub loss_w: f64,
    /// The via carrying most current: where (mil), amps.
    pub worst_via: Option<([f64; 2], f64)>,
    /// The track carrying the densest current: where (mil), amps, A/mm².
    pub worst_track: Option<([f64; 2], f64, f64)>,
    /// Number of vias of the net that carry current between layers.
    pub vias: usize,
    pub notes: Vec<String>,
}

/// Copper thickness of each copper layer, m (1 oz where unknown).
fn thicknesses(board: &Board) -> (Vec<f64>, Vec<f64>, Vec<String>) {
    let mut t = Vec::new();
    let mut z = Vec::new();
    let mut names = Vec::new();
    let mut depth = 0.0;
    for l in &board.stackup.layers {
        let th = l.thickness.mil() * MIL;
        if l.kind == LayerKind::Copper {
            t.push(if th > 0.0 { th } else { 1.4 * MIL });
            z.push(depth + th / 2.0);
            names.push(l.name.clone());
        }
        depth += th.max(0.0);
    }
    if t.is_empty() {
        t = vec![1.4 * MIL, 1.4 * MIL];
        z = vec![0.0, 62.0 * MIL];
        names = vec!["Top".into(), "Bottom".into()];
    }
    (t, z, names)
}

/// Whether point `p` lies in pad `pad` (its rotated shape), mil.
fn in_pad(pad: &Pad, p: [f64; 2]) -> bool {
    if pad.outline.len() >= 3 {
        let v: Vec<[f64; 2]> = pad.outline.iter().map(|q| [q.x.mil(), q.y.mil()]).collect();
        return in_ring(&v, p);
    }
    let (s, c) = (-pad.rotation_deg).to_radians().sin_cos();
    let (dx, dy) = (p[0] - pad.at.x.mil(), p[1] - pad.at.y.mil());
    let (x, y) = (dx * c - dy * s, dx * s + dy * c);
    let (w, h) = (pad.size_x.mil() / 2.0, pad.size_y.mil() / 2.0);
    match pad.shape {
        PadShape::Rect | PadShape::Octagon => x.abs() <= w && y.abs() <= h,
        PadShape::Round => {
            let r = w.min(h);
            let (ex, ey) = ((x.abs() - (w - r)).max(0.0), (y.abs() - (h - r)).max(0.0));
            ex * ex + ey * ey <= r * r
        }
    }
}

fn in_ring(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let (a, b) = (v[i], v[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Copper layers a pad has copper on.
fn pad_layers(p: &Pad, layers: usize) -> Vec<u8> {
    match p.layer {
        LayerRef::Copper(l) => vec![l],
        LayerRef::AllCopper => (0..layers as u8).collect(),
        _ => Vec::new(),
    }
}

/// Solves the IR drop of `setup.net`.
pub fn solve(board: &Board, setup: &Setup) -> std::result::Result<Result, String> {
    let net = setup.net;
    let g = &board.geometry;
    let (thick, zpos, names) = thicknesses(board);
    let nl = thick.len();
    let mut notes = Vec::new();
    // Extent of the net's copper.
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    let mut grow = |x: f64, y: f64| {
        lo[0] = lo[0].min(x);
        lo[1] = lo[1].min(y);
        hi[0] = hi[0].max(x);
        hi[1] = hi[1].max(y);
    };
    let polys: Vec<_> = g
        .polygons
        .iter()
        .filter(|p| p.net == Some(net) && matches!(p.layer, LayerRef::Copper(_)))
        .collect();
    for p in &polys {
        for q in &p.outline {
            grow(q.x.mil(), q.y.mil());
        }
    }
    for t in g.tracks.iter().filter(|t| t.net == Some(net)) {
        grow(t.start.x.mil(), t.start.y.mil());
        grow(t.end.x.mil(), t.end.y.mil());
    }
    for p in g.pads.iter().filter(|p| p.net == Some(net)) {
        let r = p.size_x.mil().max(p.size_y.mil());
        grow(p.at.x.mil() - r, p.at.y.mil() - r);
        grow(p.at.x.mil() + r, p.at.y.mil() + r);
    }
    if lo[0] > hi[0] {
        return Err("the net has no copper".into());
    }
    // Cell size: about 250 000 cells over the extent, no coarser than
    // `max_cell_mil`, no finer than 1 mil.
    let area = (hi[0] - lo[0] + 1.0) * (hi[1] - lo[1] + 1.0);
    let cell = (area / 250_000.0)
        .sqrt()
        .clamp(1.0, setup.max_cell_mil.max(1.0));
    let origin = [lo[0] - cell, lo[1] - cell];
    let cols = ((hi[0] - origin[0]) / cell).ceil() as usize + 2;
    let rows = ((hi[1] - origin[1]) / cell).ceil() as usize + 2;
    let per = cols * rows;
    let center = |i: usize, j: usize| {
        [
            origin[0] + (i as f64 + 0.5) * cell,
            origin[1] + (j as f64 + 0.5) * cell,
        ]
    };
    let cell_of = |p: [f64; 2]| -> Option<usize> {
        let i = ((p[0] - origin[0]) / cell).floor();
        let j = ((p[1] - origin[1]) / cell).floor();
        (i >= 0.0 && j >= 0.0 && (i as usize) < cols && (j as usize) < rows)
            .then(|| j as usize * cols + i as usize)
    };
    // Area copper per layer (pours and pads).
    let mut area_cu = vec![vec![false; per]; nl];
    for p in &polys {
        let LayerRef::Copper(l) = p.layer else {
            continue;
        };
        let Some(fill) = area_cu.get_mut(l as usize) else {
            continue;
        };
        let rings: Vec<Vec<[f64; 2]>> = std::iter::once(&p.outline)
            .chain(&p.holes)
            .map(|r| r.iter().map(|q| [q.x.mil(), q.y.mil()]).collect())
            .collect();
        // Scanlines through the cell centres, even-odd over all rings.
        for j in 0..rows {
            let y = center(0, j)[1];
            let mut xs: Vec<f64> = Vec::new();
            for r in &rings {
                for k in 0..r.len() {
                    let (a, b) = (r[k], r[(k + 1) % r.len()]);
                    if (a[1] > y) != (b[1] > y) {
                        xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for span in xs.chunks(2) {
                if let [x0, x1] = *span {
                    let i0 = ((x0 - origin[0]) / cell - 0.5).ceil().max(0.0) as usize;
                    let i1 = ((x1 - origin[0]) / cell - 0.5).floor();
                    if i1 < 0.0 {
                        continue;
                    }
                    for i in i0..=(i1 as usize).min(cols - 1) {
                        fill[j * cols + i] = true;
                    }
                }
            }
        }
    }
    let net_pads: Vec<&Pad> = g.pads.iter().filter(|p| p.net == Some(net)).collect();
    // Cells each pad covers, per layer (at least the cell at its centre).
    let mut pad_cells: Vec<Vec<(u8, usize)>> = Vec::with_capacity(net_pads.len());
    for p in &net_pads {
        let mut cells = Vec::new();
        let r = p.size_x.mil().max(p.size_y.mil()) / 2.0 + cell;
        let (px, py) = (p.at.x.mil(), p.at.y.mil());
        for l in pad_layers(p, nl) {
            let mut any = false;
            let i0 = ((px - r - origin[0]) / cell).floor().max(0.0) as usize;
            let j0 = ((py - r - origin[1]) / cell).floor().max(0.0) as usize;
            let i1 = (((px + r - origin[0]) / cell).ceil() as usize).min(cols - 1);
            let j1 = (((py + r - origin[1]) / cell).ceil() as usize).min(rows - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if in_pad(p, center(i, j)) {
                        cells.push((l, j * cols + i));
                        any = true;
                    }
                }
            }
            if !any {
                if let Some(c) = cell_of([px, py]) {
                    cells.push((l, c));
                }
            }
        }
        for &(l, c) in &cells {
            if let Some(a) = area_cu.get_mut(l as usize) {
                a[c] = true;
            }
        }
        pad_cells.push(cells);
    }
    // Node numbering: area cells, then cells that only tracks touch.
    let mut id: Vec<Vec<u32>> = vec![vec![u32::MAX; per]; nl];
    let mut nodes: Vec<(u8, usize)> = Vec::new();
    let node = |l: u8, c: usize, id: &mut Vec<Vec<u32>>, nodes: &mut Vec<(u8, usize)>| {
        let slot = &mut id[l as usize][c];
        if *slot == u32::MAX {
            *slot = nodes.len() as u32;
            nodes.push((l, c));
        }
        *slot as usize
    };
    for l in 0..nl {
        for c in 0..per {
            if area_cu[l][c] {
                node(l as u8, c, &mut id, &mut nodes);
            }
        }
    }
    // Conductances: (a, b, G, kind) where kind 0 area, 1 track (with the
    // track's width), 2 via (its index).
    let mut links: Vec<(usize, usize, f64)> = Vec::new();
    let mut track_link: Vec<(usize, f64, [f64; 2])> = Vec::new(); // link, width m, where
    let mut via_link: Vec<(usize, usize)> = Vec::new(); // link, via
    for l in 0..nl {
        let gs = thick[l] / RHO;
        for j in 0..rows {
            for i in 0..cols {
                let c = j * cols + i;
                if !area_cu[l][c] {
                    continue;
                }
                if i + 1 < cols && area_cu[l][c + 1] {
                    links.push((id[l][c] as usize, id[l][c + 1] as usize, gs));
                }
                if j + 1 < rows && area_cu[l][c + cols] {
                    links.push((id[l][c] as usize, id[l][c + cols] as usize, gs));
                }
            }
        }
    }
    // Tracks and arcs: chains through the cells along them.
    let mut chain =
        |l: u8, pts: &[[f64; 2]], w: f64, id: &mut Vec<Vec<u32>>, nodes: &mut Vec<(u8, usize)>| {
            if (l as usize) >= nl || w <= 0.0 {
                return;
            }
            let mut prev: Option<(usize, [f64; 2])> = None;
            for seg in pts.windows(2) {
                let (a, b) = (seg[0], seg[1]);
                let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                let steps = ((len / (cell * 0.5)).ceil() as usize).max(1);
                for k in 0..=steps {
                    let f = k as f64 / steps as f64;
                    let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                    let Some(c) = cell_of(p) else {
                        continue;
                    };
                    let n = node(l, c, id, nodes);
                    match prev {
                        Some((pn, pp)) if pn != n => {
                            let d = (p[0] - pp[0]).hypot(p[1] - pp[1]).max(cell * 0.25) * MIL;
                            let gt = w * MIL * thick[l as usize] / (RHO * d);
                            track_link.push((links.len(), w * MIL, p));
                            links.push((pn, n, gt));
                            prev = Some((n, p));
                        }
                        Some(_) => {}
                        None => prev = Some((n, p)),
                    }
                }
            }
        };
    for t in g.tracks.iter().filter(|t| t.net == Some(net) && !t.text) {
        if let LayerRef::Copper(l) = t.layer {
            chain(
                l,
                &[
                    [t.start.x.mil(), t.start.y.mil()],
                    [t.end.x.mil(), t.end.y.mil()],
                ],
                t.width.mil(),
                &mut id,
                &mut nodes,
            );
        }
    }
    for a in g.arcs.iter().filter(|a| a.net == Some(net)) {
        if let LayerRef::Copper(l) = a.layer {
            let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
            if sweep == 0.0 {
                sweep = 360.0;
            }
            let n = ((sweep / 10.0).ceil() as usize).max(2);
            let (cx, cy, r) = (a.center.x.mil(), a.center.y.mil(), a.radius.mil());
            let pts: Vec<[f64; 2]> = (0..=n)
                .map(|k| {
                    let t = (a.start_deg + sweep * k as f64 / n as f64).to_radians();
                    [cx + r * t.cos(), cy + r * t.sin()]
                })
                .collect();
            chain(l, &pts, a.width.mil(), &mut id, &mut nodes);
        }
    }
    // Vias: barrel segments between the layers they reach that have the
    // net's copper there.
    let mut net_vias = 0;
    for (vi, v) in g
        .vias
        .iter()
        .enumerate()
        .filter(|(_, v)| v.net == Some(net))
    {
        let Some(c) = cell_of([v.at.x.mil(), v.at.y.mil()]) else {
            continue;
        };
        let (a, b) = (
            v.from_layer.min(v.to_layer) as usize,
            (v.from_layer.max(v.to_layer) as usize).min(nl - 1),
        );
        let reached: Vec<usize> = (a..=b)
            .filter(|&l| id[l][c] != u32::MAX && v.drilled(l as u8).is_none())
            .collect();
        if reached.len() < 2 {
            continue;
        }
        net_vias += 1;
        let r_in = v.hole.mil() / 2.0 * MIL;
        let ring = std::f64::consts::PI
            * ((r_in + setup.plating_mil * MIL).powi(2) - r_in.powi(2)).max(1e-14);
        for w in reached.windows(2) {
            let len = (zpos[w[1]] - zpos[w[0]]).abs().max(1e-6);
            via_link.push((links.len(), vi));
            links.push((
                id[w[0]][c] as usize,
                id[w[1]][c] as usize,
                ring / (RHO * len),
            ));
        }
    }
    let n = nodes.len();
    // Pads of the source and loads, by part.
    let part_cells = |part: usize, id: &Vec<Vec<u32>>| -> Vec<Vec<usize>> {
        net_pads
            .iter()
            .zip(&pad_cells)
            .filter(|(p, _)| p.component == Some(part as u32))
            .map(|(_, cells)| {
                cells
                    .iter()
                    .filter_map(|&(l, c)| {
                        let k = id[l as usize][c];
                        (k != u32::MAX).then_some(k as usize)
                    })
                    .collect()
            })
            .collect()
    };
    let src: Vec<usize> = part_cells(setup.source, &id)
        .into_iter()
        .flatten()
        .collect();
    if src.is_empty() {
        return Err(format!(
            "{} has no pad on the net",
            board
                .components
                .get(setup.source)
                .map_or("the source", |c| c.designator.as_str())
        ));
    }
    let mut fixed = vec![false; n];
    for &s in &src {
        fixed[s] = true;
    }
    // Which nodes the source reaches.
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(a, b, gab) in &links {
        adj[a].push((b, gab));
        adj[b].push((a, gab));
    }
    let mut reach = vec![false; n];
    let mut stack = src.clone();
    for &s in &src {
        reach[s] = true;
    }
    while let Some(a) = stack.pop() {
        for &(b, _) in &adj[a] {
            if !reach[b] {
                reach[b] = true;
                stack.push(b);
            }
        }
    }
    // Load currents into the nodes.
    let mut inject = vec![0.0; n];
    let mut load_nodes: Vec<Vec<usize>> = Vec::new();
    for &(part, amps) in &setup.loads {
        let pads = part_cells(part, &id);
        let live: Vec<&Vec<usize>> = pads
            .iter()
            .filter(|cells| cells.iter().any(|&k| reach[k]))
            .collect();
        let all: Vec<usize> = pads.iter().flatten().copied().collect();
        if !live.is_empty() {
            let per_pad = amps / live.len() as f64;
            for cells in &live {
                let cells: Vec<usize> = cells.iter().copied().filter(|&k| reach[k]).collect();
                for &k in &cells {
                    inject[k] -= per_pad / cells.len() as f64;
                }
            }
        }
        load_nodes.push(all);
    }
    // Unknowns: reached, not fixed.
    let mut uid = vec![u32::MAX; n];
    let mut free = Vec::new();
    for k in 0..n {
        if reach[k] && !fixed[k] {
            uid[k] = free.len() as u32;
            free.push(k);
        }
    }
    let u = free.len();
    let mut diag = vec![0.0; u];
    let mut rhs = vec![0.0; u];
    let mut nb: Vec<Vec<(u32, f64)>> = vec![Vec::new(); u];
    for &(a, b, gab) in &links {
        if !reach[a] || !reach[b] {
            continue;
        }
        match (uid[a], uid[b]) {
            (ua, ub) if ua != u32::MAX && ub != u32::MAX => {
                diag[ua as usize] += gab;
                diag[ub as usize] += gab;
                nb[ua as usize].push((ub, gab));
                nb[ub as usize].push((ua, gab));
            }
            (ua, _) if ua != u32::MAX => {
                diag[ua as usize] += gab;
                rhs[ua as usize] += gab * setup.volts;
            }
            (_, ub) if ub != u32::MAX => {
                diag[ub as usize] += gab;
                rhs[ub as usize] += gab * setup.volts;
            }
            _ => {}
        }
    }
    for (k, &node) in free.iter().enumerate() {
        rhs[k] += inject[node];
    }
    let x = pcg(&diag, &nb, &rhs, vec![setup.volts; u]);
    let mut v = vec![f64::NAN; n];
    for k in 0..n {
        if fixed[k] {
            v[k] = setup.volts;
        } else if uid[k] != u32::MAX {
            v[k] = x[uid[k] as usize];
        }
    }
    // Results.
    let loads = setup
        .loads
        .iter()
        .zip(&load_nodes)
        .map(|(&(part, amps), cells)| {
            let vs: Vec<f64> = cells
                .iter()
                .map(|&k| v[k])
                .filter(|x| x.is_finite())
                .collect();
            LoadResult {
                part,
                amps,
                v_min: vs.iter().copied().fold(f64::NAN, f64::min),
                v_mean: if vs.is_empty() {
                    f64::NAN
                } else {
                    vs.iter().sum::<f64>() / vs.len() as f64
                },
                connected: !vs.is_empty(),
            }
        })
        .collect::<Vec<_>>();
    for l in &loads {
        if !l.connected {
            notes.push(format!(
                "{} does not join {} through the net's copper",
                board.components[l.part].designator, board.components[setup.source].designator
            ));
        }
    }
    let mut loss_w = 0.0;
    let mut worst_via: Option<([f64; 2], f64)> = None;
    let mut worst_track: Option<([f64; 2], f64, f64)> = None;
    let mut is_via = HashMap::new();
    for &(li, vi) in &via_link {
        is_via.insert(li, vi);
    }
    let mut is_track = HashMap::new();
    for &(li, w, p) in &track_link {
        is_track.insert(li, (w, p));
    }
    for (li, &(a, b, gab)) in links.iter().enumerate() {
        if !(v[a].is_finite() && v[b].is_finite()) {
            continue;
        }
        let i = (v[a] - v[b]) * gab;
        loss_w += i * (v[a] - v[b]);
        if let Some(&vi) = is_via.get(&li) {
            let at = g.vias[vi].at;
            if worst_via.is_none_or(|w| i.abs() > w.1) {
                worst_via = Some(([at.x.mil(), at.y.mil()], i.abs()));
            }
        }
        if let Some(&(w, p)) = is_track.get(&li) {
            let (l, _) = nodes[a];
            let density = i.abs() / (w * thick[l as usize]) / 1e6;
            if worst_track.is_none_or(|t| density > t.2) {
                worst_track = Some((p, i.abs(), density));
            }
        }
    }
    let max_drop = v
        .iter()
        .filter(|x| x.is_finite())
        .map(|x| setup.volts - x)
        .fold(0.0, f64::max);
    let mut maps: Vec<LayerMap> = Vec::new();
    for l in 0..nl {
        let mut vals = vec![f32::NAN; per];
        let mut any = false;
        for c in 0..per {
            let k = id[l][c];
            if k != u32::MAX && v[k as usize].is_finite() {
                vals[c] = v[k as usize] as f32;
                any = true;
            }
        }
        if any {
            maps.push(LayerMap {
                layer: l as u8,
                name: names.get(l).cloned().unwrap_or_default(),
                v: vals,
            });
        }
    }
    let unreached = (0..n).filter(|&k| !reach[k]).count();
    if unreached * 200 >= n {
        notes.push(format!(
            "{:.0} % of the net's copper does not join the source and was left out",
            100.0 * unreached as f64 / n as f64
        ));
    }
    Ok(Result {
        loads,
        maps,
        origin,
        cell,
        cols,
        rows,
        volts: setup.volts,
        max_drop,
        loss_w,
        worst_via,
        worst_track,
        vias: net_vias,
        notes,
    })
}

/// Conjugate gradients with a symmetric Gauss-Seidel preconditioner on
/// the matrix of `diag` and off-diagonal −`nb`.
fn pcg(diag: &[f64], nb: &[Vec<(u32, f64)>], rhs: &[f64], mut x: Vec<f64>) -> Vec<f64> {
    let u = diag.len();
    if u == 0 {
        return x;
    }
    let apply = |x: &[f64], y: &mut [f64]| {
        for i in 0..u {
            let mut s = diag[i] * x[i];
            for &(j, k) in &nb[i] {
                s -= k * x[j as usize];
            }
            y[i] = s;
        }
    };
    let pre = |r: &[f64], z: &mut [f64]| {
        for i in 0..u {
            let mut s = r[i];
            for &(j, k) in &nb[i] {
                if (j as usize) < i {
                    s += k * z[j as usize];
                }
            }
            z[i] = s / diag[i];
        }
        for i in (0..u).rev() {
            let mut s = 0.0;
            for &(j, k) in &nb[i] {
                if (j as usize) > i {
                    s += k * z[j as usize];
                }
            }
            z[i] += s / diag[i];
        }
    };
    let mut ax = vec![0.0; u];
    apply(&x, &mut ax);
    let mut r: Vec<f64> = rhs.iter().zip(&ax).map(|(b, a)| b - a).collect();
    let norm0 = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut z = vec![0.0; u];
    pre(&r, &mut z);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let mut q = vec![0.0; u];
    for _ in 0..50_000 {
        apply(&p, &mut q);
        let pq: f64 = p.iter().zip(&q).map(|(a, b)| a * b).sum();
        if pq <= 0.0 {
            break;
        }
        let alpha = rz / pq;
        let mut rn = 0.0;
        for i in 0..u {
            x[i] += alpha * p[i];
            r[i] -= alpha * q[i];
            rn += r[i] * r[i];
        }
        if rn.sqrt() < 1e-12 * norm0 {
            break;
        }
        pre(&r, &mut z);
        let rz2: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz2 / rz;
        rz = rz2;
        for i in 0..u {
            p[i] = z[i] + beta * p[i];
        }
    }
    x
}

impl Result {
    /// The drop map of one layer as SVG, `px` wide: copper coloured from
    /// no drop (dark blue) to the largest drop (yellow), the source's and
    /// loads' places marked by the caller's `marks` (mil, label).
    pub fn svg(
        &self,
        map: &LayerMap,
        px: f64,
        title: &str,
        marks: &[([f64; 2], String)],
    ) -> String {
        use std::fmt::Write;
        let k = px / (self.cols as f64);
        let h = self.rows as f64 * k;
        let mut o = String::new();
        let _ = writeln!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {px:.0} {:.0}" font-family="Helvetica, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#0f1218"/><g shape-rendering="crispEdges">"##,
            h + 40.0
        );
        let drop = self.max_drop.max(1e-12);
        for j in 0..self.rows {
            let y = h - (j as f64 + 1.0) * k;
            let mut i = 0;
            while i < self.cols {
                let v = map.v[j * self.cols + i];
                if v.is_nan() {
                    i += 1;
                    continue;
                }
                let q = |v: f32| {
                    ((self.volts - v as f64) / drop * 31.0)
                        .round()
                        .clamp(0.0, 31.0) as u8
                };
                let level = q(v);
                let mut e = i + 1;
                while e < self.cols {
                    let w = map.v[j * self.cols + e];
                    if w.is_nan() || q(w) != level {
                        break;
                    }
                    e += 1;
                }
                let [r, gg, b] = drop_color(level as f64 / 31.0);
                let _ = write!(
                    o,
                    r#"<rect x="{:.2}" y="{y:.2}" width="{:.2}" height="{:.2}" fill="rgb({r},{gg},{b})"/>"#,
                    i as f64 * k,
                    (e - i) as f64 * k + 0.2,
                    k + 0.2
                );
                i = e;
            }
        }
        o.push_str("</g>\n");
        for (p, label) in marks {
            let x = (p[0] - self.origin[0]) / self.cell * k;
            let y = h - (p[1] - self.origin[1]) / self.cell * k;
            let _ = writeln!(
                o,
                r##"<circle cx="{x:.1}" cy="{y:.1}" r="4" fill="none" stroke="#ffffff"/><text x="{:.1}" y="{:.1}" fill="#ffffff">{label}</text>"##,
                x + 6.0,
                y - 6.0
            );
        }
        let _ = writeln!(
            o,
            r##"<text x="6" y="{:.1}" fill="#e8ecf4">{} · drop 0 to {:.1} mV</text></svg>"##,
            h + 24.0,
            title.replace('&', "&amp;").replace('<', "&lt;"),
            self.max_drop * 1000.0
        );
        o
    }
}

/// Colour for a drop from 0 (none) to 1 (the largest).
pub fn drop_color(f: f64) -> [u8; 3] {
    let stops = [
        [30.0, 50.0, 110.0],
        [30.0, 140.0, 160.0],
        [120.0, 200.0, 90.0],
        [250.0, 210.0, 60.0],
        [240.0, 90.0, 50.0],
    ];
    let f = f.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (f.floor() as usize).min(stops.len() - 2);
    let t = f - i as f64;
    [0, 1, 2].map(|k| (stops[i][k] + (stops[i + 1][k] - stops[i][k]) * t).round() as u8)
}

/// Parts on `net` other than `source`, with their pad count there.
pub fn parts_on(board: &Board, net: u32) -> Vec<(usize, usize)> {
    let mut m: HashMap<usize, usize> = HashMap::new();
    for p in board.geometry.pads.iter().filter(|p| p.net == Some(net)) {
        if let Some(c) = p.component {
            *m.entry(c as usize).or_default() += 1;
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// The parts likely to draw current from `net`, with their pad count
/// there: all but the source, capacitors, test points and mounting holes
/// (or all but the source when nothing else is left).
pub fn default_loads(board: &Board, net: u32, source: usize) -> Vec<(usize, usize)> {
    let parts: Vec<_> = parts_on(board, net)
        .into_iter()
        .filter(|p| p.0 != source)
        .collect();
    let passive = |i: usize| {
        let d = board.components[i].designator.to_ascii_uppercase();
        let pre: String = d.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        matches!(pre.as_str(), "C" | "TP" | "PP" | "MH" | "H" | "FID")
    };
    let active: Vec<_> = parts.iter().copied().filter(|p| !passive(p.0)).collect();
    if active.is_empty() {
        parts
    } else {
        active
    }
}

/// A likely source for a supply net: the part with a pad named like an
/// output ("OUT", "VOUT"), else an inductor (a regulator's output), else a
/// small IC with two or more pads on the net (a regulator's output pins),
/// else a connector bringing the supply in.
pub fn guess_source(board: &Board, net: u32) -> Option<usize> {
    let parts = parts_on(board, net);
    let des = |i: usize| board.components[i].designator.to_ascii_uppercase();
    let prefix = |i: usize| {
        des(i)
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
    };
    let total = |i: usize| {
        board
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(i as u32))
            .count()
    };
    board
        .geometry
        .pads
        .iter()
        .filter(|p| p.net == Some(net))
        .find(|p| p.name.to_ascii_uppercase().contains("OUT"))
        .and_then(|p| p.component.map(|c| c as usize))
        .or_else(|| parts.iter().map(|p| p.0).find(|&i| prefix(i) == "L"))
        .or_else(|| {
            parts
                .iter()
                .filter(|p| {
                    p.1 >= 2
                        && matches!(prefix(p.0).as_str(), "U" | "IC" | "VR")
                        && total(p.0) <= 24
                })
                .min_by_key(|p| (std::cmp::Reverse(p.1), total(p.0)))
                .map(|p| p.0)
        })
        .or_else(|| {
            parts
                .iter()
                .map(|p| p.0)
                .find(|&i| matches!(prefix(i).as_str(), "J" | "P" | "CN" | "X"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn board_with(layers: usize) -> Board {
        let mut b = Board::default();
        for i in 0..layers {
            if i > 0 {
                b.stackup.layers.push(StackLayer {
                    name: format!("D{i}"),
                    altium_id: None,
                    kind: LayerKind::Dielectric,
                    thickness: Length::from_mil(10.0),
                    dielectric_constant: Some(4.2),
                    loss_tangent: None,
                    roughness_um: None,
                    material: None,
                });
            }
            b.stackup.layers.push(StackLayer {
                name: format!("L{i}"),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.nets.push(Net {
            name: "VDD".into(),
            ..Default::default()
        });
        for d in ["U1", "U2"] {
            b.components.push(Component {
                designator: d.into(),
                footprint: String::new(),
                library_ref: None,
                x: Length::from_mil(0.0),
                y: Length::from_mil(0.0),
                rotation_deg: 0.0,
                side: Side::Top,
                sheet: None,
                params: Default::default(),
            });
        }
        b
    }

    fn pad(part: u32, x: f64, y: f64, w: f64, h: f64, layer: LayerRef) -> Pad {
        Pad {
            layer,
            net: Some(0),
            component: Some(part),
            name: "1".into(),
            at: Point::mil(x, y),
            size_x: Length::from_mil(w),
            size_y: Length::from_mil(h),
            hole: Length::from_mil(0.0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    fn rect(l: u8, x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            teardrop: false,
            layer: LayerRef::Copper(l),
            net: Some(0),
            outline: vec![
                Point::mil(x0, y0),
                Point::mil(x1, y0),
                Point::mil(x1, y1),
                Point::mil(x0, y1),
            ],
            holes: Vec::new(),
            pour: None,
            component: None,
        }
    }

    #[test]
    fn a_strip_of_pour_drops_rho_l_over_wt() {
        let mut b = board_with(2);
        b.geometry.polygons.push(rect(0, 0.0, 0.0, 2000.0, 200.0));
        b.geometry
            .pads
            .push(pad(0, 10.0, 100.0, 20.0, 200.0, LayerRef::Copper(0)));
        b.geometry
            .pads
            .push(pad(1, 1990.0, 100.0, 20.0, 200.0, LayerRef::Copper(0)));
        let mut s = Setup::new(0, 0, 1.0);
        s.loads.push((1, 2.0));
        let r = solve(&b, &s).unwrap();
        // Between the pads' centres: 1980 mil of 200 mil wide 1.4 mil copper.
        let want = 2.0 * RHO * 1980.0 / (200.0 * 1.4 * MIL);
        let drop = 1.0 - r.loads[0].v_mean;
        assert!((drop - want).abs() < 0.03 * want, "{drop} vs {want}");
        assert!(
            (r.loss_w - 2.0 * drop).abs() < 0.03 * r.loss_w,
            "{} {}",
            r.loss_w,
            drop
        );
    }

    #[test]
    fn a_thin_track_keeps_its_own_resistance() {
        let mut b = board_with(2);
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: Point::mil(0.0, 0.0),
            end: Point::mil(3000.0, 0.0),
            width: Length::from_mil(4.0),
            component: None,
            text: false,
        });
        b.geometry
            .pads
            .push(pad(0, 0.0, 0.0, 4.0, 4.0, LayerRef::Copper(0)));
        b.geometry
            .pads
            .push(pad(1, 3000.0, 0.0, 4.0, 4.0, LayerRef::Copper(0)));
        let mut s = Setup::new(0, 0, 1.0);
        s.loads.push((1, 0.5));
        let r = solve(&b, &s).unwrap();
        let want = 0.5 * RHO * 3000.0 / (4.0 * 1.4 * MIL);
        let drop = 1.0 - r.loads[0].v_min;
        assert!((drop - want).abs() < 0.02 * want, "{drop} vs {want}");
        let (_, amps, density) = r.worst_track.unwrap();
        assert!((amps - 0.5).abs() < 1e-6);
        assert!((density - 0.5 / (4.0 * 1.4 * MIL * MIL * 1e6)).abs() < 0.01 * density);
    }

    #[test]
    fn two_layers_joined_by_vias_halve_the_drop() {
        let mut one = board_with(2);
        one.geometry.polygons.push(rect(0, 0.0, 0.0, 2000.0, 200.0));
        let mut two = one.clone();
        two.geometry.polygons.push(rect(1, 0.0, 0.0, 2000.0, 200.0));
        for x in (0..=2000).step_by(100) {
            for y in [40.0, 100.0, 160.0] {
                two.geometry.vias.push(Via {
                    net: Some(0),
                    from_layer: 0,
                    to_layer: 1,
                    at: Point::mil(x as f64, y),
                    diameter: Length::from_mil(20.0),
                    hole: Length::from_mil(10.0),
                    rings: Vec::new(),
                    fab: Default::default(),
                });
            }
        }
        let drop = |b: &mut Board| {
            b.geometry
                .pads
                .push(pad(0, 10.0, 100.0, 20.0, 200.0, LayerRef::Copper(0)));
            b.geometry
                .pads
                .push(pad(1, 1990.0, 100.0, 20.0, 200.0, LayerRef::Copper(0)));
            let mut s = Setup::new(0, 0, 1.0);
            s.loads.push((1, 2.0));
            let r = solve(b, &s).unwrap();
            1.0 - r.loads[0].v_mean
        };
        let (d1, d2) = (drop(&mut one), drop(&mut two));
        // Nearly half: the current crosses to the second layer through
        // vias near the pads.
        assert!(d2 < 0.6 * d1 && d2 > 0.45 * d1, "{d1} {d2}");
    }

    #[test]
    fn a_part_off_the_copper_is_reported() {
        let mut b = board_with(2);
        b.geometry.polygons.push(rect(0, 0.0, 0.0, 500.0, 200.0));
        b.geometry
            .pads
            .push(pad(0, 10.0, 100.0, 20.0, 200.0, LayerRef::Copper(0)));
        b.geometry
            .pads
            .push(pad(1, 1500.0, 100.0, 20.0, 20.0, LayerRef::Copper(0)));
        let mut s = Setup::new(0, 0, 1.0);
        s.loads.push((1, 1.0));
        let r = solve(&b, &s).unwrap();
        assert!(!r.loads[0].connected);
        assert!(r.notes.iter().any(|n| n.contains("U2")));
    }
}
