//! 3D clearance: part bodies that run into each other or come too close,
//! parts taller (or lower) than the board's Height rules or a limit, and
//! bodies hanging over the board edge. Bodies come from the parts' 3D
//! models where the caller has them meshed, otherwise from boxes estimated
//! from the pads (`body`).
//!
//! Two bodies are first compared by their outlines seen from above (convex
//! hulls) and height ranges. Where those meet, the models' height maps
//! decide, so a part under a shield can's lid or under a connector's
//! overhang is not taken for a collision.

use crate::body::part_bodies;
use crate::drc::height_rules;
use crate::geometry::Body3d;
use crate::{variant, Board};
use std::collections::HashMap;

/// Height map cell (mm).
const CELL: f64 = 0.1;

/// A part model's triangles, in the model's own frame (mm).
#[derive(Clone, Copy, Debug)]
pub struct ModelMesh<'a> {
    pub positions: &'a [[f32; 3]],
    pub indices: &'a [u32],
}

/// What to check.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// Least room between two bodies on one side (mm); 0 checks only that
    /// they do not touch.
    pub clearance: f64,
    /// Tallest part allowed on each side (mm), on top of Height rules.
    pub max_top: Option<f64>,
    pub max_bottom: Option<f64>,
    /// Report bodies over the board edge.
    pub edge: bool,
    /// Give parts without a model an estimated box.
    pub estimate: bool,
}

impl Default for Goal {
    fn default() -> Self {
        Goal {
            clearance: 0.0,
            max_top: None,
            max_bottom: None,
            edge: true,
            estimate: true,
        }
    }
}

/// One body on the board.
#[derive(Clone, Debug)]
pub struct Solid {
    pub component: usize,
    pub bottom: bool,
    /// Outline seen from above (mm, convex, counter-clockwise).
    pub hull: Vec<[f64; 2]>,
    /// Height range off the surface it stands on (mm).
    pub z: (f64, f64),
    /// The body this came from, or `None` for an estimated box.
    pub body: Option<usize>,
}

/// A problem found.
#[derive(Clone, Debug, PartialEq)]
pub enum Issue {
    /// Two bodies overlap.
    Collision { a: usize, b: usize, at: [f64; 2] },
    /// Two bodies closer than the clearance.
    TooClose {
        a: usize,
        b: usize,
        gap: f64,
        need: f64,
        at: [f64; 2],
    },
    /// A part taller than its limit.
    TooTall {
        part: usize,
        height: f64,
        limit: f64,
        rule: Option<String>,
        estimated: bool,
    },
    /// A part lower than its Height rule's least height.
    TooLow {
        part: usize,
        height: f64,
        limit: f64,
        rule: String,
        estimated: bool,
    },
    /// A body over the board edge.
    OverEdge { part: usize, by: f64, at: [f64; 2] },
}

impl Issue {
    /// The parts it is about.
    pub fn parts(&self) -> Vec<usize> {
        match *self {
            Issue::Collision { a, b, .. } | Issue::TooClose { a, b, .. } => vec![a, b],
            Issue::TooTall { part, .. }
            | Issue::TooLow { part, .. }
            | Issue::OverEdge { part, .. } => {
                vec![part]
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Issue::Collision { .. } => "Collision",
            Issue::TooClose { .. } => "TooClose",
            Issue::TooTall { .. } => "TooTall",
            Issue::TooLow { .. } => "TooLow",
            Issue::OverEdge { .. } => "OverEdge",
        }
    }

    pub fn describe(&self, board: &Board) -> String {
        let d = |i: usize| {
            board
                .components
                .get(i)
                .map_or("?", |c| c.designator.as_str())
                .to_string()
        };
        let est = |e: bool| if e { " (estimated)" } else { "" };
        match self {
            Issue::Collision { a, b, .. } => {
                format!("{} and {} run into each other", d(*a), d(*b))
            }
            Issue::TooClose {
                a, b, gap, need, ..
            } => format!(
                "{} and {} are {gap:.2} mm apart, need {need:.2}",
                d(*a),
                d(*b)
            ),
            Issue::TooTall {
                part,
                height,
                limit,
                rule,
                estimated,
            } => format!(
                "{} is {height:.2} mm tall{}, limit {limit:.2}{}",
                d(*part),
                est(*estimated),
                rule.as_ref().map_or(String::new(), |r| format!(" ({r})"))
            ),
            Issue::TooLow {
                part,
                height,
                limit,
                rule,
                estimated,
            } => format!(
                "{} is {height:.2} mm tall{}, at least {limit:.2} ({rule})",
                d(*part),
                est(*estimated)
            ),
            Issue::OverEdge { part, by, .. } => {
                format!("{} hangs {by:.2} mm over the board edge", d(*part))
            }
        }
    }

    /// Where to look (mm).
    pub fn at(&self, board: &Board) -> [f64; 2] {
        match self {
            Issue::Collision { at, .. }
            | Issue::TooClose { at, .. }
            | Issue::OverEdge { at, .. } => *at,
            Issue::TooTall { part, .. } | Issue::TooLow { part, .. } => board
                .components
                .get(*part)
                .map_or([0.0; 2], |c| [c.x.mm(), c.y.mm()]),
        }
    }
}

/// What the check found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    /// Bodies from models, and estimated boxes.
    pub models: usize,
    pub estimated: usize,
}

impl Report {
    pub fn count(&self, kind: &str) -> usize {
        self.issues.iter().filter(|i| i.kind() == kind).count()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} bodies from models, {} estimated: {} collisions, {} too close, {} too tall, {} too low, {} over the edge",
            self.models,
            self.estimated,
            self.count("Collision"),
            self.count("TooClose"),
            self.count("TooTall"),
            self.count("TooLow"),
            self.count("OverEdge")
        )
    }
}

type Mat = [[f64; 3]; 3];

fn mul(a: Mat, b: Mat) -> Mat {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn rot(a: f64, axis: usize) -> Mat {
    let (s, c) = a.sin_cos();
    match axis {
        0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
        1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
        _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// Model frame to board: x, y on the board (mm) and height off the
/// surface the part stands on. The same placement the 3D view draws.
fn placement(body: &Body3d) -> impl Fn([f32; 3]) -> [f64; 3] {
    let [rx, ry, rz] = body.model_rotation_deg;
    // Altium's z rotation already includes the part's own rotation. On the
    // bottom it turns the same way before the model is turned over.
    let rz = (rz - body.rotation_deg).to_radians();
    let m = mul(
        rot(rz, 2),
        mul(rot(ry.to_radians(), 1), rot(rx.to_radians(), 0)),
    );
    let (x0, y0, dz) = (body.at.x.mm(), body.at.y.mm(), body.standoff.mm());
    let flip = if body.bottom { -1.0 } else { 1.0 };
    move |p: [f32; 3]| {
        let p = p.map(f64::from);
        let q = [0, 1, 2].map(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2]);
        [x0 + q[0], y0 + flip * q[1], q[2] + dz]
    }
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Convex hull, counter-clockwise.
fn hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
    if pts.len() < 3 {
        return pts;
    }
    let mut h: Vec<[f64; 2]> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = h.len();
        let iter: Box<dyn Iterator<Item = &[f64; 2]>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

fn in_convex(p: [f64; 2], h: &[[f64; 2]]) -> bool {
    let n = h.len();
    n >= 3 && (0..n).all(|i| cross(h[i], h[(i + 1) % n], p) >= -1e-9)
}

/// Whether two convex outlines overlap (separating axis test).
fn overlap(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    for (p, q) in [(a, b), (b, a)] {
        let n = p.len();
        for i in 0..n {
            let (u, v) = (p[i], p[(i + 1) % n]);
            // q wholly on the outer side of this edge.
            if q.iter().all(|&w| cross(u, v, w) <= 1e-9) {
                return false;
            }
        }
    }
    true
}

/// Gap between two convex outlines (0 when they overlap).
fn gap(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    if overlap(a, b) {
        return 0.0;
    }
    let mut d = f64::MAX;
    for (p, q) in [(a, b), (b, a)] {
        let n = q.len();
        for &w in p {
            for i in 0..n {
                d = d.min(seg_dist(w, q[i], q[(i + 1) % n]));
            }
        }
    }
    d
}

fn bbox(h: &[[f64; 2]]) -> [f64; 4] {
    h.iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        })
}

type HeightMap = HashMap<(i32, i32), (f32, f32)>;

fn cell(x: f64) -> i32 {
    (x / CELL).floor() as i32
}

/// Each cell the model covers and the height range of its surfaces there.
fn height_map(body: &Body3d, mesh: ModelMesh) -> HeightMap {
    let place = placement(body);
    let pts: Vec<[f64; 3]> = mesh.positions.iter().map(|&p| place(p)).collect();
    let mut map = HeightMap::new();
    let mut stack: Vec<[[f64; 3]; 3]> = Vec::new();
    for t in mesh.indices.chunks_exact(3) {
        let Some(tri) = (0..3)
            .map(|k| pts.get(t[k] as usize).copied())
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if tri.iter().all(|p| p[2] <= 0.01) {
            continue;
        }
        stack.push([tri[0], tri[1], tri[2]]);
        while let Some(t) = stack.pop() {
            // Split along the longest edge until it fits a cell.
            let len = |i: usize| {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                (a[0] - b[0]).hypot(a[1] - b[1])
            };
            let (k, l) = (0..3)
                .map(|i| (i, len(i)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or((0, 0.0));
            if l > CELL && stack.len() < 1_000_000 {
                let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let m = [0, 1, 2].map(|i| (a[i] + b[i]) / 2.0);
                stack.push([a, m, c]);
                stack.push([m, b, c]);
                continue;
            }
            let lo = t.iter().map(|p| p[2]).fold(f64::MAX, f64::min).max(0.0) as f32;
            let hi = t.iter().map(|p| p[2]).fold(f64::MIN, f64::max) as f32;
            if hi <= 0.01 {
                continue;
            }
            let (x0, x1) = (
                cell(t.iter().map(|p| p[0]).fold(f64::MAX, f64::min)),
                cell(t.iter().map(|p| p[0]).fold(f64::MIN, f64::max)),
            );
            let (y0, y1) = (
                cell(t.iter().map(|p| p[1]).fold(f64::MAX, f64::min)),
                cell(t.iter().map(|p| p[1]).fold(f64::MIN, f64::max)),
            );
            for x in x0..=x1 {
                for y in y0..=y1 {
                    let e = map.entry((x, y)).or_insert((lo, hi));
                    e.0 = e.0.min(lo);
                    e.1 = e.1.max(hi);
                }
            }
        }
    }
    map
}

/// The bodies of the fitted parts: each model the caller meshed, and an
/// estimated box for a part with none (when `estimate`).
pub fn solids(board: &Board, meshes: &[Option<ModelMesh>], estimate: bool) -> Vec<Solid> {
    let mut out = Vec::new();
    let mut has_model = vec![false; board.components.len()];
    for (bi, body) in board.geometry.bodies.iter().enumerate() {
        let Some(c) = body.component.map(|c| c as usize) else {
            continue;
        };
        if c >= board.components.len() || !variant::fitted(board, c) || body.opacity <= 0.05 {
            continue;
        }
        let Some(Some(mesh)) = meshes.get(body.model as usize) else {
            continue;
        };
        let place = placement(body);
        let mut z = (f64::MAX, f64::MIN);
        let mut flat = Vec::new();
        for &p in mesh.positions {
            let q = place(p);
            // Leads through the board do not count.
            if q[2] < -0.05 {
                continue;
            }
            z = (z.0.min(q[2].max(0.0)), z.1.max(q[2]));
            flat.push([q[0], q[1]]);
        }
        let h = hull(flat);
        if h.len() < 3 || z.1 <= 0.01 {
            continue;
        }
        has_model[c] = true;
        out.push(Solid {
            component: c,
            bottom: body.bottom,
            hull: h,
            z,
            body: Some(bi),
        });
    }
    if estimate {
        for b in part_bodies(board) {
            if has_model[b.component] || !variant::fitted(board, b.component) {
                continue;
            }
            out.push(Solid {
                component: b.component,
                bottom: b.bottom,
                hull: hull(b.corners.to_vec()),
                z: (0.0, b.height),
                body: None,
            });
        }
    }
    out
}

/// The height range of a body at a cell, if it covers it.
fn at_cell(s: &Solid, map: Option<&HeightMap>, c: (i32, i32)) -> Option<(f32, f32)> {
    match map {
        Some(m) => m.get(&c).copied(),
        None => {
            let p = [(c.0 as f64 + 0.5) * CELL, (c.1 as f64 + 0.5) * CELL];
            in_convex(p, &s.hull).then_some((s.z.0 as f32, s.z.1 as f32))
        }
    }
}

/// The closest two bodies come where their height ranges meet (mm, on
/// the cell grid; 0 when they share a cell), within `reach`, and where.
fn closest(
    a: &Solid,
    ma: Option<&HeightMap>,
    b: &Solid,
    mb: Option<&HeightMap>,
    reach: f64,
) -> Option<(f64, [f64; 2])> {
    // Walk the cells of the smaller one.
    let cells = |s: &Solid, m: Option<&HeightMap>| -> Vec<((i32, i32), (f32, f32))> {
        match m {
            Some(m) => m.iter().map(|(&k, &v)| (k, v)).collect(),
            None => {
                let bb = bbox(&s.hull);
                let mut v = Vec::new();
                for x in cell(bb[0])..=cell(bb[2]) {
                    for y in cell(bb[1])..=cell(bb[3]) {
                        if let Some(z) = at_cell(s, None, (x, y)) {
                            v.push(((x, y), z));
                        }
                    }
                }
                v
            }
        }
    };
    let (sa, sb) = (cells(a, ma), cells(b, mb));
    let (walk, other, om) = if sa.len() <= sb.len() {
        (sa, b, mb)
    } else {
        (sb, a, ma)
    };
    let ob = bbox(&other.hull);
    let r = (reach / CELL).ceil() as i32;
    let mut best: Option<(f64, [f64; 2])> = None;
    for ((x, y), z) in walk {
        let p = [(x as f64 + 0.5) * CELL, (y as f64 + 0.5) * CELL];
        if p[0] < ob[0] - reach - CELL
            || p[0] > ob[2] + reach + CELL
            || p[1] < ob[1] - reach - CELL
            || p[1] > ob[3] + reach + CELL
        {
            continue;
        }
        for dx in -r..=r {
            for dy in -r..=r {
                let Some(w) = at_cell(other, om, (x + dx, y + dy)) else {
                    continue;
                };
                if !(z.0 < w.1 - 0.01 && w.0 < z.1 - 0.01) {
                    continue;
                }
                let d = (((dx.abs() - 1).max(0) as f64).hypot((dy.abs() - 1).max(0) as f64)) * CELL;
                if d <= reach && best.is_none_or(|b| d < b.0) {
                    best = Some((d, p));
                }
            }
        }
    }
    best
}

/// Checks the board's part bodies. `meshes` holds the meshed model of each
/// of `board.geometry.models`, where there is one.
pub fn check(board: &Board, meshes: &[Option<ModelMesh>], goal: &Goal) -> Report {
    let solids = solids(board, meshes, goal.estimate);
    let mut rep = Report {
        models: solids.iter().filter(|s| s.body.is_some()).count(),
        estimated: solids.iter().filter(|s| s.body.is_none()).count(),
        ..Default::default()
    };
    let reach = goal.clearance.max(0.0);
    let boxes: Vec<[f64; 4]> = solids.iter().map(|s| bbox(&s.hull)).collect();
    // Candidate pairs: same side, outlines within reach, heights overlap.
    let size = 5.0;
    let mut grid: HashMap<(bool, i32, i32), Vec<usize>> = HashMap::new();
    for (i, b) in boxes.iter().enumerate() {
        for x in (b[0] / size).floor() as i32..=(b[2] / size).floor() as i32 {
            for y in (b[1] / size).floor() as i32..=(b[3] / size).floor() as i32 {
                grid.entry((solids[i].bottom, x, y)).or_default().push(i);
            }
        }
    }
    let origin = |c: usize| {
        let p = &board.components[c];
        [p.x.mm(), p.y.mm()]
    };
    let mut pairs = std::collections::BTreeSet::new();
    for list in grid.values() {
        for (n, &i) in list.iter().enumerate() {
            for &j in &list[n + 1..] {
                let (a, b) = (&solids[i], &solids[j]);
                if a.component == b.component {
                    continue;
                }
                let (ba, bb) = (boxes[i], boxes[j]);
                if ba[0] > bb[2] + reach
                    || bb[0] > ba[2] + reach
                    || ba[1] > bb[3] + reach
                    || bb[1] > ba[3] + reach
                {
                    continue;
                }
                if !(a.z.0 < b.z.1 - 0.01 && b.z.0 < a.z.1 - 0.01) {
                    continue;
                }
                // Alternate footprints placed on one spot.
                let (oa, ob) = (origin(a.component), origin(b.component));
                if (oa[0] - ob[0]).hypot(oa[1] - ob[1]) < 0.05 {
                    continue;
                }
                if gap(&a.hull, &b.hull) <= reach {
                    pairs.insert((i.min(j), i.max(j)));
                }
            }
        }
    }
    let mut maps: HashMap<usize, HeightMap> = HashMap::new();
    let mut map_of = |i: usize| -> Option<HeightMap> {
        let bi = solids[i].body?;
        if let Some(m) = maps.get(&i) {
            return Some(m.clone());
        }
        let body = &board.geometry.bodies[bi];
        let mesh = meshes.get(body.model as usize).copied().flatten()?;
        let m = height_map(body, mesh);
        maps.insert(i, m.clone());
        Some(m)
    };
    // One issue per pair of parts.
    let mut seen = std::collections::HashSet::new();
    for (i, j) in pairs {
        let (a, b) = (&solids[i], &solids[j]);
        let key = (a.component.min(b.component), a.component.max(b.component));
        if seen.contains(&key) {
            continue;
        }
        let (ma, mb) = (map_of(i), map_of(j));
        let Some((d, at)) = closest(a, ma.as_ref(), b, mb.as_ref(), reach) else {
            continue;
        };
        seen.insert(key);
        // A hull gap is exact where the bodies are apart.
        let hull_gap = gap(&a.hull, &b.hull);
        if d <= 0.0 && hull_gap <= 0.0 {
            rep.issues.push(Issue::Collision {
                a: key.0,
                b: key.1,
                at,
            });
        } else if reach > 0.0 {
            rep.issues.push(Issue::TooClose {
                a: key.0,
                b: key.1,
                gap: hull_gap.max(d),
                need: reach,
                at,
            });
        }
    }
    // Heights, part by part.
    let rules = height_rules(board);
    let mut tall: HashMap<usize, (f64, bool, bool)> = HashMap::new();
    for s in &solids {
        let e = tall.entry(s.component).or_insert((0.0, s.bottom, true));
        e.0 = e.0.max(s.z.1);
        e.2 &= s.body.is_none();
    }
    let mut parts: Vec<_> = tall.into_iter().collect();
    parts.sort_by_key(|p| p.0);
    for (c, (h, bottom, estimated)) in parts {
        let rule = rules.get(c).cloned().flatten();
        let side_max = if bottom {
            goal.max_bottom
        } else {
            goal.max_top
        };
        let mut limit: Option<(f64, Option<String>)> = rule
            .as_ref()
            .and_then(|r| r.max.map(|m| (m, Some(r.name.clone()))));
        if let Some(m) = side_max {
            if limit.as_ref().is_none_or(|l| m < l.0) {
                limit = Some((m, None));
            }
        }
        if let Some((limit, rule)) = limit {
            if h > limit + 1e-3 {
                rep.issues.push(Issue::TooTall {
                    part: c,
                    height: h,
                    limit,
                    rule,
                    estimated,
                });
            }
        }
        if let Some((min, name)) =
            rule.and_then(|r| r.min.filter(|&m| m > 0.0).map(|m| (m, r.name)))
        {
            if h < min - 1e-3 {
                rep.issues.push(Issue::TooLow {
                    part: c,
                    height: h,
                    limit: min,
                    rule: name,
                    estimated,
                });
            }
        }
    }
    // Over the edge.
    let outline: Vec<[f64; 2]> = board
        .geometry
        .outline
        .iter()
        .map(|p| [p.x.mm(), p.y.mm()])
        .collect();
    if goal.edge && outline.len() >= 3 {
        let mut worst: HashMap<usize, (f64, [f64; 2])> = HashMap::new();
        for s in &solids {
            for &p in &s.hull {
                if inside_ring(&outline, p) {
                    continue;
                }
                let d = (0..outline.len())
                    .map(|i| seg_dist(p, outline[i], outline[(i + 1) % outline.len()]))
                    .fold(f64::MAX, f64::min);
                let e = worst.entry(s.component).or_insert((0.0, p));
                if d > e.0 {
                    *e = (d, p);
                }
            }
        }
        let mut list: Vec<_> = worst.into_iter().filter(|w| w.1 .0 > 0.05).collect();
        list.sort_by_key(|w| w.0);
        for (part, (by, at)) in list {
            rep.issues.push(Issue::OverEdge { part, by, at });
        }
    }
    rep
}

fn inside_ring(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = v.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (v[i], v[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Board, Component, Length, Point, Rule, Side};

    /// A box model `w` x `d` x `h` mm standing on its origin, or a lid
    /// `t` thick at height `h` when `lid`.
    fn cube(w: f32, d: f32, z0: f32, z1: f32) -> (Vec<[f32; 3]>, Vec<u32>) {
        let (x, y) = (w / 2.0, d / 2.0);
        let mut p = Vec::new();
        for &z in &[z0, z1] {
            p.extend([[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]]);
        }
        let f: [[u32; 4]; 6] = [
            [0, 1, 2, 3],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut i = Vec::new();
        for q in f {
            i.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        (p, i)
    }

    fn part(b: &mut Board, d: &str, x: f64, y: f64, model: u32) -> usize {
        b.components.push(Component {
            designator: d.into(),
            footprint: "BOX".into(),
            library_ref: None,
            x: Length::from_mm(x),
            y: Length::from_mm(y),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        let c = b.components.len() - 1;
        b.geometry.bodies.push(Body3d {
            component: Some(c as u32),
            model,
            at: Point::mil(x / 0.0254, y / 0.0254),
            rotation_deg: 0.0,
            model_rotation_deg: [0.0; 3],
            standoff: Length::from_mm(0.0),
            bottom: false,
            opacity: 1.0,
        });
        c
    }

    #[test]
    fn bodies_heights_and_edges() {
        let mut b = Board::default();
        let mm = |x: f64, y: f64| Point::mil(x / 0.0254, y / 0.0254);
        b.geometry.outline = vec![mm(0.0, 0.0), mm(50.0, 0.0), mm(50.0, 30.0), mm(0.0, 30.0)];
        let chip = cube(2.0, 2.0, 0.0, 1.0);
        let tall = cube(4.0, 4.0, 0.0, 12.0);
        // A can: a lid 0.2 mm thick 3 mm up, on four thin walls.
        let mut can = cube(10.0, 10.0, 2.8, 3.0);
        for (dx, dy, w, d) in [
            (-4.9, 0.0, 0.2, 10.0),
            (4.9, 0.0, 0.2, 10.0),
            (0.0, -4.9, 10.0, 0.2),
            (0.0, 4.9, 10.0, 0.2),
        ] {
            let (p, i) = cube(w, d, 0.0, 3.0);
            let base = can.0.len() as u32;
            can.0.extend(p.iter().map(|q| [q[0] + dx, q[1] + dy, q[2]]));
            can.1.extend(i.iter().map(|k| k + base));
        }
        let meshes = [
            Some(ModelMesh {
                positions: &chip.0,
                indices: &chip.1,
            }),
            Some(ModelMesh {
                positions: &tall.0,
                indices: &tall.1,
            }),
            Some(ModelMesh {
                positions: &can.0,
                indices: &can.1,
            }),
        ];
        // U1 and U2 overlap by 0.5 mm; U3 is 0.3 mm from U2.
        part(&mut b, "U1", 5.0, 5.0, 0);
        part(&mut b, "U2", 6.5, 5.0, 0);
        part(&mut b, "U3", 8.8, 5.0, 0);
        // U4 sits under the can SH1, clear of its walls.
        part(&mut b, "SH1", 30.0, 15.0, 2);
        part(&mut b, "U4", 30.0, 15.0 + 1.0, 0);
        // J1 is 12 mm tall and hangs 1 mm over the right edge.
        part(&mut b, "J1", 49.0, 15.0, 1);
        let goal = Goal::default();
        let r = check(&b, &meshes, &goal);
        assert_eq!(r.models, 6);
        let kinds: Vec<String> = r.issues.iter().map(|i| i.describe(&b)).collect();
        assert_eq!(
            kinds,
            [
                "U1 and U2 run into each other",
                "J1 hangs 1.00 mm over the board edge"
            ],
            "{r:?}"
        );
        // With 0.5 mm clearance U2 and U3 are too close.
        let r = check(
            &b,
            &meshes,
            &Goal {
                clearance: 0.5,
                ..goal.clone()
            },
        );
        let close: Vec<&Issue> = r.issues.iter().filter(|i| i.kind() == "TooClose").collect();
        assert_eq!(close.len(), 1, "{r:?}");
        let Issue::TooClose { a, b: o, gap, .. } = close[0] else {
            unreachable!()
        };
        assert_eq!((*a, *o), (1, 2));
        assert!((gap - 0.3).abs() < 0.11, "{gap}");
        // A Height rule for the top side, and a lower limit for one part.
        b.rules.push(Rule {
            kind: "Height".into(),
            name: "Height".into(),
            enabled: true,
            priority: 2,
            scope: vec!["OnTopLayer".into()],
            params: [
                ("MAXHEIGHT".to_string(), "10mm".to_string()),
                ("MINHEIGHT".to_string(), "0mm".to_string()),
            ]
            .into(),
        });
        b.rules.push(Rule {
            kind: "Height".into(),
            name: "Tall chips".into(),
            enabled: true,
            priority: 1,
            scope: vec!["InComponent('U3')".into()],
            params: [
                ("MAXHEIGHT".to_string(), "20mm".to_string()),
                ("MINHEIGHT".to_string(), "2mm".to_string()),
            ]
            .into(),
        });
        let r = check(
            &b,
            &meshes,
            &Goal {
                edge: false,
                ..goal.clone()
            },
        );
        let kinds: Vec<String> = r.issues.iter().map(|i| i.describe(&b)).collect();
        assert_eq!(
            kinds,
            [
                "U1 and U2 run into each other",
                "U3 is 1.00 mm tall, at least 2.00 (Tall chips)",
                "J1 is 12.00 mm tall, limit 10.00 (Height)",
            ],
            "{r:?}"
        );
        // A side limit tighter than the rule wins.
        let r = check(
            &b,
            &meshes,
            &Goal {
                edge: false,
                max_top: Some(2.5),
                ..goal
            },
        );
        assert_eq!(r.count("TooTall"), 2, "{r:?}");
    }
}
