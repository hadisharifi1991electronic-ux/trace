//! Via stitching and via fences: ground vias placed for you.
//!
//! Stitching fills a net's copper areas with a grid of vias, joining its
//! pours and planes layer to layer. A fence lines chosen tracks (an RF
//! line, a clock) with a row of vias on each side, closer than a twentieth
//! of a wavelength, so the line's field stays between them.
//!
//! Every via is checked before it is kept: inside the board and clear of
//! its edge and slots, clear of other copper by the clearance on every
//! layer it passes (tracks, pads, holes, vias, and other nets' copper
//! areas), out of keepouts and pour cutouts, and landing fully on the
//! net's own copper on at least two layers, so it joins something.

use crate::route::{point_seg, pt, Obstacles, P};
use crate::{Board, Fab, LayerRef, Length, Point, Via};
use std::collections::HashMap;

/// The vias to place, mil and copper layers (0 = top).
#[derive(Clone, Debug)]
pub struct ViaSpec {
    pub drill: f64,
    pub pad: f64,
    pub from: u8,
    pub to: u8,
}

impl ViaSpec {
    /// The board's most used via, top to bottom.
    pub fn of(board: &Board) -> ViaSpec {
        let (pad, drill) = crate::route::common_via(board);
        let n = board.stackup.copper_layers().count().max(1) as u8;
        ViaSpec {
            drill,
            pad,
            from: 0,
            to: n - 1,
        }
    }
}

/// How to place the vias, mil.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Copper to copper clearance.
    pub clearance: f64,
    /// From a via's pad to the board edge and slots.
    pub edge: f64,
    /// Copper layers of its own net a via must land on fully.
    pub min_layers: usize,
}

impl Rules {
    pub fn of(board: &Board) -> Rules {
        Rules {
            clearance: crate::route::board_clearance(board),
            edge: 20.0,
            min_layers: 2,
        }
    }
}

/// Stitching a net's copper.
#[derive(Clone, Debug)]
pub struct StitchGoal {
    pub net: u32,
    pub via: ViaSpec,
    pub rules: Rules,
    /// Grid pitch, mil.
    pub grid: f64,
    /// Every other row shifted by half the pitch.
    pub stagger: bool,
    /// Only in this box (x0, y0, x1, y1 mil), if given.
    pub area: Option<[f64; 4]>,
}

/// Fencing tracks.
#[derive(Clone, Debug)]
pub struct FenceGoal {
    /// The nets whose tracks are fenced.
    pub nets: Vec<u32>,
    /// The net the fence vias join (ground).
    pub ground: u32,
    pub via: ViaSpec,
    pub rules: Rules,
    /// Along the track, centre to centre, mil.
    pub pitch: f64,
    /// From the track's edge to the fence vias' pads, mil.
    pub gap: f64,
    /// Rows on each side.
    pub rows: usize,
    /// Only tracks on these copper layers; empty for all.
    pub layers: Vec<u8>,
}

/// A fence pitch of a twentieth of a wavelength at `f_ghz` in a
/// dielectric of `er`, mil.
pub fn fence_pitch(f_ghz: f64, er: f64) -> f64 {
    let lambda = 11_802.85 / (f_ghz.max(0.01) * er.max(1.0).sqrt());
    lambda / 20.0
}

/// Why a spot was left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Off the board, or too near its edge or a slot.
    pub edge: usize,
    /// Too near other copper (or a keepout or cutout).
    pub clearance: usize,
    /// Not fully on the net's copper on enough layers.
    pub no_copper: usize,
    /// Too near a via of the net already there or just placed.
    pub spacing: usize,
}

impl Skipped {
    pub fn total(&self) -> usize {
        self.edge + self.clearance + self.no_copper + self.spacing
    }
}

#[derive(Clone, Debug)]
pub struct Placed {
    pub vias: Vec<Via>,
    /// Clearance holes to cut in other nets' planes: (polygon, ring).
    pub holes: Vec<(usize, Vec<Point>)>,
    pub tried: usize,
    pub skipped: Skipped,
}

impl Placed {
    /// The vias as one list of edits, after the board's own.
    pub fn edits(&self, board: &Board) -> Vec<crate::edit::Edit> {
        let mut out = Vec::new();
        if !self.holes.is_empty() {
            let before = board.geometry.polygons.clone();
            let mut after = before.clone();
            for (i, ring) in &self.holes {
                if let Some(p) = after.get_mut(*i) {
                    p.holes.push(ring.clone());
                }
            }
            out.push(crate::edit::Edit::SetPolygons { before, after });
        }
        let n = board.geometry.vias.len();
        out.extend(
            self.vias
                .iter()
                .enumerate()
                .map(|(i, v)| crate::edit::Edit::InsertVia {
                    index: n + i,
                    via: v.clone(),
                }),
        );
        out
    }

    /// A line on what was done.
    pub fn summary(&self) -> String {
        let s = &self.skipped;
        format!(
            "{} vias placed of {} spots tried; left out: {} clearance, {} not on the net's copper, {} spacing, {} board edge",
            self.vias.len(),
            self.tried,
            s.clearance,
            s.no_copper,
            s.spacing,
            s.edge
        )
    }
}

const CELL: f64 = 100.0;

/// A copper area's edges, indexed.
struct Area {
    layer: Option<u8>,
    net: Option<u32>,
    /// The board polygon it is, and whether a pour makes it (poured again
    /// after the vias go in, so it clears them itself).
    poly: Option<usize>,
    poured: bool,
    bbox: [f64; 4],
    edges: Vec<(P, P)>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn cell(v: f64) -> i32 {
    (v / CELL).floor() as i32
}

impl Area {
    fn new(layer: Option<u8>, net: Option<u32>, rings: &[&[Point]]) -> Option<Area> {
        let mut edges = Vec::new();
        for r in rings {
            if r.len() < 3 {
                continue;
            }
            let v: Vec<P> = r.iter().map(|&p| pt(p)).collect();
            for i in 0..v.len() {
                let (a, b) = (v[i], v[(i + 1) % v.len()]);
                if a != b {
                    edges.push((a, b));
                }
            }
        }
        if edges.is_empty() {
            return None;
        }
        let mut bbox = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, (a, b)) in edges.iter().enumerate() {
            let e = [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ];
            bbox = [
                bbox[0].min(e[0]),
                bbox[1].min(e[1]),
                bbox[2].max(e[2]),
                bbox[3].max(e[3]),
            ];
            for gx in cell(e[0])..=cell(e[2]) {
                for gy in cell(e[1])..=cell(e[3]) {
                    grid.entry((gx, gy)).or_default().push(i as u32);
                }
            }
        }
        Some(Area {
            layer,
            net,
            poly: None,
            poured: false,
            bbox,
            edges,
            grid,
        })
    }

    fn near_box(&self, p: P, r: f64) -> bool {
        p[0] >= self.bbox[0] - r
            && p[0] <= self.bbox[2] + r
            && p[1] >= self.bbox[1] - r
            && p[1] <= self.bbox[3] + r
    }

    /// Inside (even-odd, so holes count out).
    fn contains(&self, p: P) -> bool {
        if !self.near_box(p, 0.0) {
            return false;
        }
        let gy = cell(p[1]);
        let mut seen: Vec<u32> = Vec::new();
        for gx in cell(p[0])..=cell(self.bbox[2]) {
            if let Some(list) = self.grid.get(&(gx, gy)) {
                seen.extend(list);
            }
        }
        seen.sort_unstable();
        seen.dedup();
        let mut odd = false;
        for i in seen {
            let (a, b) = self.edges[i as usize];
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                odd = !odd;
            }
        }
        odd
    }

    /// Whether an edge is closer than `r`.
    fn edge_within(&self, p: P, r: f64) -> bool {
        if !self.near_box(p, r) {
            return false;
        }
        for gx in cell(p[0] - r)..=cell(p[0] + r) {
            for gy in cell(p[1] - r)..=cell(p[1] + r) {
                if let Some(list) = self.grid.get(&(gx, gy)) {
                    for &i in list {
                        let (a, b) = self.edges[i as usize];
                        if point_seg(p, a, b) < r {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn on(&self, l: u8) -> bool {
        self.layer.is_none_or(|x| x == l)
    }
}

/// Everything a new via is checked against.
struct Site {
    obstacles: Obstacles,
    /// Copper areas, with the board's keepouts and cutouts (net `None`,
    /// always in the way).
    copper: Vec<Area>,
    blocked: Vec<Area>,
    outline: Option<Area>,
    slots: Vec<Area>,
    keep_tracks: Vec<(P, P, f64)>,
    /// Vias of the net, there and placed, by cell.
    vias: HashMap<(i32, i32), Vec<P>>,
}

#[derive(PartialEq)]
enum Why {
    Edge,
    Clearance,
    NoCopper,
    Spacing,
}

impl Site {
    fn new(board: &Board, net: u32) -> Site {
        let g = &board.geometry;
        let layer = |l: &LayerRef| match l {
            LayerRef::Copper(i) => Some(Some(*i)),
            LayerRef::AllCopper => Some(None),
            _ => None,
        };
        let mut copper = Vec::new();
        for (i, p) in g.polygons.iter().enumerate() {
            if let Some(l) = layer(&p.layer) {
                let mut rings: Vec<&[Point]> = vec![&p.outline];
                rings.extend(p.holes.iter().map(|h| h.as_slice()));
                if let Some(mut a) = Area::new(l, p.net, &rings) {
                    a.poly = Some(i);
                    a.poured = p.pour.is_some();
                    copper.push(a);
                }
            }
        }
        let mut blocked = Vec::new();
        for p in &g.cutouts {
            if let Some(l) = layer(&p.layer) {
                blocked.extend(Area::new(l, None, &[&p.outline]));
            }
        }
        for p in g.polygons.iter().chain(&g.areas) {
            if p.layer == LayerRef::Keepout {
                blocked.extend(Area::new(None, None, &[&p.outline]));
            }
        }
        let keep_tracks = g
            .tracks
            .iter()
            .filter(|t| t.layer == LayerRef::Keepout)
            .map(|t| (pt(t.start), pt(t.end), t.width.mil() / 2.0))
            .collect();
        let mut vias: HashMap<(i32, i32), Vec<P>> = HashMap::new();
        for v in g.vias.iter().filter(|v| v.net == Some(net)) {
            let c = pt(v.at);
            vias.entry((cell(c[0]), cell(c[1]))).or_default().push(c);
        }
        Site {
            obstacles: Obstacles::new(board),
            copper,
            blocked,
            outline: Area::new(None, None, &[&g.outline]),
            slots: g
                .slots
                .iter()
                .filter_map(|s| Area::new(None, None, &[s]))
                .collect(),
            keep_tracks,
            vias,
        }
    }

    /// Whether a via fits at `c`; if so, the planes of other nets it
    /// passes through, which need a clearance hole cut.
    fn check(
        &self,
        c: P,
        net: u32,
        v: &ViaSpec,
        r: &Rules,
        spacing: f64,
    ) -> Result<Vec<usize>, Why> {
        let rp = v.pad.max(v.drill) / 2.0;
        // The board edge and slots.
        if let Some(o) = &self.outline {
            if !o.contains(c) || o.edge_within(c, rp + r.edge) {
                return Err(Why::Edge);
            }
        }
        if self
            .slots
            .iter()
            .any(|s| s.contains(c) || s.edge_within(c, rp + r.edge))
        {
            return Err(Why::Edge);
        }
        // Spacing from the net's vias.
        let reach = (spacing / CELL).ceil() as i32;
        let (gx, gy) = (cell(c[0]), cell(c[1]));
        for x in gx - reach..=gx + reach {
            for y in gy - reach..=gy + reach {
                if let Some(list) = self.vias.get(&(x, y)) {
                    if list
                        .iter()
                        .any(|q| (q[0] - c[0]).hypot(q[1] - c[1]) < spacing)
                    {
                        return Err(Why::Spacing);
                    }
                }
            }
        }
        // Tracks, pads, holes and vias, on every layer it passes.
        if self
            .obstacles
            .check_span_via(c, 2.0 * rp, &[], r.clearance, v.from, v.to)
            .is_some()
        {
            return Err(Why::Clearance);
        }
        if self
            .keep_tracks
            .iter()
            .any(|&(a, b, w)| point_seg(c, a, b) < w + rp)
        {
            return Err(Why::Clearance);
        }
        let span = |a: &Area| (v.from..=v.to).any(|l| a.on(l));
        if self
            .blocked
            .iter()
            .filter(|a| span(a))
            .any(|a| a.contains(c) || a.edge_within(c, rp))
        {
            return Err(Why::Clearance);
        }
        // Copper areas: its own landed on; others' kept clear of on the
        // via's end layers, where it has pads, and passed through on the
        // layers between with a clearance hole (a pour clears it when
        // poured again; a plane gets the hole cut).
        let mut landed = vec![false; v.to as usize + 1];
        let mut cuts = Vec::new();
        let hole = (rp + r.clearance) * 1.03;
        for a in self.copper.iter().filter(|a| span(a)) {
            if a.net == Some(net) {
                if a.contains(c) && !a.edge_within(c, rp) {
                    for l in v.from..=v.to {
                        if a.on(l) {
                            landed[l as usize] = true;
                        }
                    }
                }
            } else if a.contains(c) || a.edge_within(c, rp + r.clearance) {
                let between = a.layer.is_some_and(|l| l > v.from && l < v.to);
                if !between {
                    return Err(Why::Clearance);
                }
                if a.poured {
                    continue;
                }
                // A plane: the hole must sit wholly in its copper.
                if !a.contains(c) || a.edge_within(c, hole) {
                    return Err(Why::Clearance);
                }
                cuts.extend(a.poly);
            }
        }
        let need = r.min_layers.min((v.to - v.from) as usize + 1);
        if landed.iter().filter(|&&x| x).count() < need {
            return Err(Why::NoCopper);
        }
        Ok(cuts)
    }

    fn keep(&mut self, c: P) {
        self.vias
            .entry((cell(c[0]), cell(c[1])))
            .or_default()
            .push(c);
    }
}

fn via_at(c: P, net: u32, v: &ViaSpec) -> Via {
    Via {
        net: Some(net),
        from_layer: v.from,
        to_layer: v.to,
        at: Point::mil(c[0], c[1]),
        diameter: Length::from_mil(v.pad.max(v.drill)),
        hole: Length::from_mil(v.drill),
        rings: Vec::new(),
        fab: Fab::default(),
    }
}

fn place(
    board: &Board,
    net: u32,
    v: &ViaSpec,
    r: &Rules,
    spacing: f64,
    spots: impl Iterator<Item = P>,
) -> Placed {
    let mut site = Site::new(board, net);
    let mut out = Placed {
        vias: Vec::new(),
        holes: Vec::new(),
        tried: 0,
        skipped: Skipped::default(),
    };
    let mut cut_at: Vec<P> = Vec::new();
    for c in spots {
        out.tried += 1;
        match site.check(c, net, v, r, spacing) {
            Ok(cuts) => {
                // Holes cut in one plane must not run into each other.
                let rad =
                    (v.pad.max(v.drill) / 2.0 + r.clearance) / (std::f64::consts::PI / 16.0).cos();
                if !cuts.is_empty()
                    && cut_at
                        .iter()
                        .any(|q: &P| (q[0] - c[0]).hypot(q[1] - c[1]) < 2.0 * rad + 1.0)
                {
                    out.skipped.spacing += 1;
                    continue;
                }
                if !cuts.is_empty() {
                    cut_at.push(c);
                }
                site.keep(c);
                out.vias.push(via_at(c, net, v));
                let ring: Vec<Point> = (0..16)
                    .map(|k| {
                        let t = std::f64::consts::PI * (2 * k + 1) as f64 / 16.0;
                        Point::mil(c[0] + rad * t.cos(), c[1] + rad * t.sin())
                    })
                    .collect();
                out.holes
                    .extend(cuts.into_iter().map(|i| (i, ring.clone())));
            }
            Err(Why::Edge) => out.skipped.edge += 1,
            Err(Why::Clearance) => out.skipped.clearance += 1,
            Err(Why::NoCopper) => out.skipped.no_copper += 1,
            Err(Why::Spacing) => out.skipped.spacing += 1,
        }
    }
    out
}

fn check_spec(board: &Board, v: &ViaSpec) -> Result<(), String> {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    if v.from >= v.to || v.to >= n.max(2) {
        return Err(format!(
            "the vias must join two different copper layers of the {n}"
        ));
    }
    if v.drill <= 0.0 || v.pad < v.drill {
        return Err("the via pad must be at least the drill".into());
    }
    Ok(())
}

/// Fills `goal.net`'s copper areas with vias on a grid.
pub fn stitch(board: &Board, goal: &StitchGoal) -> Result<Placed, String> {
    check_spec(board, &goal.via)?;
    if (goal.net as usize) >= board.nets.len() {
        return Err("no such net".into());
    }
    let grid = goal.grid.max(goal.via.pad + goal.rules.clearance);
    // The net's copper areas on the via's layers bound the grid.
    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in &board.geometry.polygons {
        let on = match p.layer {
            LayerRef::Copper(l) => (goal.via.from..=goal.via.to).contains(&l),
            LayerRef::AllCopper => true,
            _ => false,
        };
        if !on || p.net != Some(goal.net) {
            continue;
        }
        for q in &p.outline {
            let q = pt(*q);
            bb = [
                bb[0].min(q[0]),
                bb[1].min(q[1]),
                bb[2].max(q[0]),
                bb[3].max(q[1]),
            ];
        }
    }
    if let Some(a) = goal.area {
        bb = [
            bb[0].max(a[0].min(a[2])),
            bb[1].max(a[1].min(a[3])),
            bb[2].min(a[0].max(a[2])),
            bb[3].min(a[1].max(a[3])),
        ];
    }
    if bb[0] > bb[2] || bb[1] > bb[3] {
        return Err(format!(
            "{} has no copper areas on those layers{}",
            board.nets[goal.net as usize].name,
            if goal.area.is_some() {
                " in that area"
            } else {
                ""
            }
        ));
    }
    let rows = ((bb[3] - bb[1]) / grid).ceil() as i64 + 1;
    let cols = ((bb[2] - bb[0]) / grid).ceil() as i64 + 1;
    if rows * cols > 4_000_000 {
        return Err("the grid is too fine for the area".into());
    }
    // A grid fixed to the origin, so stitching again lines up.
    let (y0, x0) = ((bb[1] / grid).floor() as i64, (bb[0] / grid).floor() as i64);
    let stagger = goal.stagger;
    let spots = (y0..=y0 + rows).flat_map(move |j| {
        let shift = if stagger && j.rem_euclid(2) == 1 {
            grid / 2.0
        } else {
            0.0
        };
        (x0..=x0 + cols).map(move |i| [i as f64 * grid + shift, j as f64 * grid])
    });
    let spots: Vec<P> = spots
        .filter(|c| c[0] >= bb[0] && c[0] <= bb[2] && c[1] >= bb[1] && c[1] <= bb[3])
        .collect();
    Ok(place(
        board,
        goal.net,
        &goal.via,
        &goal.rules,
        grid * 0.7,
        spots.into_iter(),
    ))
}

/// Lines `goal.nets`' tracks with ground vias on both sides.
pub fn fence(board: &Board, goal: &FenceGoal) -> Result<Placed, String> {
    check_spec(board, &goal.via)?;
    if goal.nets.is_empty() {
        return Err("no nets to fence".into());
    }
    if goal.nets.contains(&goal.ground) {
        return Err("the fence's own net cannot be fenced".into());
    }
    let pitch = goal.pitch.max(goal.via.pad + goal.rules.clearance);
    let rp = goal.via.pad.max(goal.via.drill) / 2.0;
    let gap = goal.gap.max(goal.rules.clearance);
    let row_pitch = 2.0 * rp + goal.rules.clearance.max(pitch / 2.0);
    let g = &board.geometry;
    // Every stretch of fenced copper as a chord, with its half width.
    let mut chords: Vec<(P, P, f64)> = Vec::new();
    let on = |l: &LayerRef| match l {
        LayerRef::Copper(i) => goal.layers.is_empty() || goal.layers.contains(i),
        _ => false,
    };
    for t in &g.tracks {
        if t.text || !on(&t.layer) || !t.net.is_some_and(|n| goal.nets.contains(&n)) {
            continue;
        }
        chords.push((pt(t.start), pt(t.end), t.width.mil() / 2.0));
    }
    for a in &g.arcs {
        if !on(&a.layer) || !a.net.is_some_and(|n| goal.nets.contains(&n)) {
            continue;
        }
        let (c, r) = (pt(a.center), a.radius.mil());
        let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        if sweep == 0.0 {
            sweep = 360.0;
        }
        let n = (sweep / 10.0).ceil().max(1.0) as usize;
        let at = |i: usize| {
            let t = (a.start_deg + sweep * i as f64 / n as f64).to_radians();
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        };
        for i in 0..n {
            chords.push((at(i), at(i + 1), a.width.mil() / 2.0));
        }
    }
    if chords.is_empty() {
        return Err("those nets have no tracks to fence".into());
    }
    let mut spots = Vec::new();
    for &(a, b, hw) in &chords {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if len < 1e-6 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let n = (len / pitch).ceil().max(1.0) as usize;
        for row in 0..goal.rows.max(1) {
            let d = hw + gap + rp + row as f64 * row_pitch;
            for side in [1.0, -1.0] {
                let (ox, oy) = (-uy * d * side, ux * d * side);
                for k in 0..=n {
                    let s = len * k as f64 / n as f64;
                    spots.push([a[0] + ux * s + ox, a[1] + uy * s + oy]);
                }
            }
        }
    }
    Ok(place(
        board,
        goal.ground,
        &goal.via,
        &goal.rules,
        pitch * 0.6,
        spots.into_iter(),
    ))
}

/// The net named `name` (any case).
pub fn net_named(board: &Board, name: &str) -> Option<u32> {
    board
        .nets
        .iter()
        .position(|n| n.name == name)
        .or_else(|| {
            board
                .nets
                .iter()
                .position(|n| n.name.eq_ignore_ascii_case(name))
        })
        .map(|i| i as u32)
}

/// The likeliest ground net: the one with the most copper area.
pub fn ground_net(board: &Board) -> Option<u32> {
    let mut by: HashMap<u32, f64> = HashMap::new();
    for p in &board.geometry.polygons {
        if let (Some(n), true) = (p.net, matches!(p.layer, LayerRef::Copper(_))) {
            let v: Vec<P> = p.outline.iter().map(|&q| pt(q)).collect();
            let mut a = 0.0;
            for i in 0..v.len() {
                let (p0, p1) = (v[i], v[(i + 1) % v.len()]);
                a += p0[0] * p1[1] - p1[0] * p0[1];
            }
            *by.entry(n).or_default() += a.abs() / 2.0;
        }
    }
    let named = board.nets.iter().position(|n| {
        let u = n.name.to_ascii_uppercase();
        u == "GND" || u == "DGND" || u == "AGND" || u == "GROUND" || u == "VSS"
    });
    by.into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, _)| n)
        .or(named.map(|i| i as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerKind, Net, Polygon, StackLayer, Track};

    fn board() -> Board {
        let mut b = Board::default();
        for (i, k) in ["TOP", "GND", "BOT"].iter().enumerate() {
            if i > 0 {
                b.stackup.layers.push(StackLayer {
                    name: format!("D{i}"),
                    altium_id: None,
                    kind: LayerKind::Dielectric,
                    thickness: Length::from_mil(5.0),
                    dielectric_constant: Some(3.6),
                    loss_tangent: None,
                    roughness_um: None,
                    material: None,
                });
            }
            b.stackup.layers.push(StackLayer {
                name: k.to_string(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for n in ["GND", "RF"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![
                Point::mil(x0, y0),
                Point::mil(x1, y0),
                Point::mil(x1, y1),
                Point::mil(x0, y1),
            ]
        };
        b.geometry.outline = sq(0.0, 0.0, 2000.0, 1000.0);
        // GND on all three layers, with a hole around the RF line on top.
        for l in 0..3u8 {
            b.geometry.polygons.push(Polygon {
                layer: LayerRef::Copper(l),
                net: Some(0),
                outline: sq(50.0, 50.0, 1950.0, 950.0),
                holes: if l == 0 {
                    vec![sq(200.0, 480.0, 1800.0, 520.0)]
                } else {
                    vec![]
                },
                pour: None,
                component: None,
            });
        }
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: Point::mil(250.0, 500.0),
            end: Point::mil(1750.0, 500.0),
            width: Length::from_mil(12.0),
            component: None,
            text: false,
        });
        b
    }

    fn spec() -> ViaSpec {
        ViaSpec {
            drill: 8.0,
            pad: 16.0,
            from: 0,
            to: 2,
        }
    }

    fn rules() -> Rules {
        Rules {
            clearance: 6.0,
            edge: 20.0,
            min_layers: 2,
        }
    }

    #[test]
    fn stitching_fills_the_plane_and_keeps_clear() {
        let b = board();
        let goal = StitchGoal {
            net: 0,
            via: spec(),
            rules: rules(),
            grid: 100.0,
            stagger: false,
            area: None,
        };
        let p = stitch(&b, &goal).unwrap();
        assert!(p.vias.len() > 100, "{}", p.summary());
        for v in &p.vias {
            let c = pt(v.at);
            // Clear of the RF track (6 mil half width, 8 pad radius, 6 gap).
            assert!(point_seg(c, [250.0, 500.0], [1750.0, 500.0]) >= 20.0 - 1e-9);
            // Inside the plane and away from the board edge.
            assert!(c[0] >= 50.0 + 8.0 && c[0] <= 1950.0 - 8.0);
        }
        // Stitching again adds nothing.
        let mut b2 = b.clone();
        for e in p.edits(&b) {
            e.apply(&mut b2);
        }
        let again = stitch(&b2, &goal).unwrap();
        assert!(again.vias.is_empty(), "{}", again.summary());
    }

    #[test]
    fn vias_pass_another_nets_inner_plane_through_a_hole() {
        let mut b = board();
        b.nets.push(Net {
            name: "VCC".into(),
            ..Default::default()
        });
        // The middle layer is VCC's plane, not a pour.
        b.geometry.polygons[1].net = Some(2);
        let goal = StitchGoal {
            net: 0,
            via: spec(),
            rules: rules(),
            grid: 100.0,
            stagger: false,
            area: None,
        };
        let p = stitch(&b, &goal).unwrap();
        assert!(p.vias.len() > 100, "{}", p.summary());
        assert_eq!(p.holes.len(), p.vias.len());
        assert!(p.holes.iter().all(|h| h.0 == 1 && h.1.len() == 16));
        let mut b2 = b.clone();
        for e in p.edits(&b) {
            e.apply(&mut b2);
        }
        assert_eq!(b2.geometry.polygons[1].holes.len(), p.vias.len());
        // A via whose end layer is the VCC plane is refused there.
        let mut short = goal.clone();
        short.via.to = 1;
        short.rules.min_layers = 1;
        assert!(stitch(&b, &short).unwrap().vias.is_empty());
    }

    #[test]
    fn fence_lines_the_track_on_both_sides() {
        let mut b = board();
        // No hole in the top copper: the fence vias land on it.
        b.geometry.polygons[0].holes.clear();
        b.geometry.polygons[0].holes = vec![vec![
            Point::mil(200.0, 470.0),
            Point::mil(1800.0, 470.0),
            Point::mil(1800.0, 530.0),
            Point::mil(200.0, 530.0),
        ]];
        let goal = FenceGoal {
            nets: vec![1],
            ground: 0,
            via: spec(),
            rules: rules(),
            pitch: fence_pitch(6.0, 3.6),
            gap: 10.0,
            rows: 1,
            layers: vec![],
        };
        assert!((goal.pitch - 51.8).abs() < 0.5, "{}", goal.pitch);
        let p = fence(&b, &goal).unwrap();
        let (mut up, mut down) = (0, 0);
        for v in &p.vias {
            let c = pt(v.at);
            let d = c[1] - 500.0;
            assert!((d.abs() - 24.0).abs() < 1e-6, "{c:?}");
            if d > 0.0 {
                up += 1
            } else {
                down += 1
            }
        }
        // 1500 mil of track at about 52 mil: about 30 a side.
        assert!(up >= 28 && down >= 28, "{up} {down}: {}", p.summary());
        assert_eq!(up, down);
        assert!(fence(
            &b,
            &FenceGoal {
                ground: 1,
                ..goal.clone()
            }
        )
        .is_err());
    }
}
