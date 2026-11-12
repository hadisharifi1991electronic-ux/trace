//! Walkaround and push-and-shove for the interactive router.
//!
//! Walkaround finds a path of 0°, 45° and 90° segments around copper of
//! other nets: an A* search on a fine grid, then the corners pulled tight.
//! Shoving moves the tracks a new route runs into: each one is routed again
//! between its own ends, around the new copper.

use crate::edit::Edit;
use crate::route::{Obstacles, P};
use crate::{Board, LayerRef, Length, Point, Track};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// A capsule the search must also keep clear of: from, to, radius.
pub type Capsule = (P, P, f64);

/// Everything a search needs to know about what is in the way.
pub struct Ask<'a> {
    pub obstacles: &'a Obstacles,
    pub layer: u8,
    /// Half the width of what is routed (for a pair, of both tracks and
    /// the gap between them).
    pub r: f64,
    pub nets: [Option<u32>; 2],
    pub clearance: f64,
    /// More copper to keep clear of: the new route, when shoving.
    pub extra: &'a [Capsule],
    /// Board tracks not to count as obstacles (the ones being shoved).
    pub skip: &'a [usize],
}

impl Ask<'_> {
    fn clear(&self, a: P, b: P) -> bool {
        self.clear_r(a, b, self.r)
    }

    /// As `clear`, for something `r` wide instead.
    fn clear_r(&self, a: P, b: P, r: f64) -> bool {
        let skip = |t: usize| self.skip.contains(&t);
        if !self
            .obstacles
            .clear(self.layer, a, b, r, self.nets, self.clearance, &skip)
        {
            return false;
        }
        self.extra
            .iter()
            .all(|&(c, d, w)| crate::route::seg_seg(a, b, c, d) - w - r >= self.clearance - 1e-3)
    }
}

#[derive(PartialEq)]
struct Node {
    f: f64,
    g: f64,
    at: (i32, i32),
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.g.partial_cmp(&o.g).unwrap_or(Ordering::Equal))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// A path from `from` to `to` that keeps clearance, as corner points
/// (first `from`, last `to`), or `None` when there is none nearby.
pub fn walkaround(ask: &Ask, from: P, to: P) -> Option<Vec<P>> {
    search(ask, from, to, false).map(|(p, _)| p)
}

/// As [`walkaround`], but when `to` cannot be reached, the path as far
/// towards it as it goes, stopping short of what is in the way. The flag
/// says whether it reached `to`.
pub fn walk_toward(ask: &Ask, from: P, to: P) -> Option<(Vec<P>, bool)> {
    search(ask, from, to, true)
}

fn search(ask: &Ask, from: P, to: P, partial: bool) -> Option<(Vec<P>, bool)> {
    if ask.clear(from, to) {
        return Some((vec![from, to], true));
    }
    // A cursor inside other copper cannot be reached: search less.
    let cap = if ask.clear(to, to) { 40_000 } else { 6_000 };
    let dist = (to[0] - from[0]).hypot(to[1] - from[1]);
    // Grid fine enough to pass between pads, coarse enough to be quick.
    let step = ((ask.r + ask.clearance) / 2.0)
        .clamp(1.5, 25.0)
        .max(dist / 400.0);
    let margin = (dist * 0.6).max(30.0 * step);
    let lo = [from[0].min(to[0]) - margin, from[1].min(to[1]) - margin];
    let hi = [from[0].max(to[0]) + margin, from[1].max(to[1]) + margin];
    let at = |c: (i32, i32)| [from[0] + c.0 as f64 * step, from[1] + c.1 as f64 * step];
    let inside = |p: P| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
    let goal = (
        ((to[0] - from[0]) / step).round() as i32,
        ((to[1] - from[1]) / step).round() as i32,
    );
    let h = |c: (i32, i32)| {
        let (dx, dy) = ((c.0 - goal.0).abs() as f64, (c.1 - goal.1).abs() as f64);
        (dx.max(dy) + (std::f64::consts::SQRT_2 - 1.0) * dx.min(dy)) * step
    };
    let mut ok: HashMap<(i32, i32), bool> = HashMap::new();
    let mut free = |c: (i32, i32)| -> bool {
        *ok.entry(c).or_insert_with(|| {
            let p = at(c);
            inside(p) && ask.clear(p, p)
        })
    };
    // Cells with room to spare: every step to or from one is clear
    // without checking it, which saves most checks in open board.
    let mut room: HashMap<(i32, i32), bool> = HashMap::new();
    let mut roomy = |c: (i32, i32)| -> bool {
        *room.entry(c).or_insert_with(|| {
            let p = at(c);
            ask.clear_r(p, p, ask.r + 1.5 * step)
        })
    };
    let mut best_g: HashMap<(i32, i32), f64> = HashMap::new();
    let mut came: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut open = BinaryHeap::new();
    let start = (0, 0);
    best_g.insert(start, 0.0);
    open.push(Node {
        f: h(start),
        g: 0.0,
        at: start,
    });
    let mut expanded = 0;
    let mut end = None;
    // The cell nearest `to` reached so far, for a partial path.
    let mut nearest = (h(start), start);
    while let Some(Node { g, at: c, .. }) = open.pop() {
        if g > best_g.get(&c).copied().unwrap_or(f64::MAX) + 1e-9 {
            continue;
        }
        // Done when the last stretch to `to` itself is clear.
        if (c.0 - goal.0).abs() <= 2 && (c.1 - goal.1).abs() <= 2 && ask.clear(at(c), to) {
            end = Some(c);
            break;
        }
        if h(c) < nearest.0 - 1e-9 {
            nearest = (h(c), c);
        }
        expanded += 1;
        if expanded > cap {
            break;
        }
        for (dx, dy) in DIRS {
            let n = (c.0 + dx, c.1 + dy);
            if !free(n) {
                continue;
            }
            let cost = if dx != 0 && dy != 0 {
                std::f64::consts::SQRT_2 * step
            } else {
                step
            };
            let ng = g + cost;
            if ng + 1e-9 >= best_g.get(&n).copied().unwrap_or(f64::MAX) {
                continue;
            }
            // The step itself: copper can come closer between two free
            // points than at either.
            if !roomy(c) && !roomy(n) && !ask.clear(at(c), at(n)) {
                continue;
            }
            best_g.insert(n, ng);
            came.insert(n, c);
            open.push(Node {
                f: ng + h(n),
                g: ng,
                at: n,
            });
        }
    }
    let reached = end.is_some();
    let end = match end {
        Some(e) => e,
        // Only worth showing when it gets somewhere.
        None if partial && nearest.1 != start && nearest.0 < h(start) - step => nearest.1,
        None => return None,
    };
    let mut cells = vec![end];
    while let Some(&p) = came.get(cells.last().unwrap()) {
        cells.push(p);
    }
    cells.reverse();
    let mut pts: Vec<P> = cells.iter().map(|&c| at(c)).collect();
    pts[0] = from;
    if reached
        && pts
            .last()
            .is_some_and(|p| (p[0] - to[0]).hypot(p[1] - to[1]) > 1e-6)
    {
        pts.push(to);
    }
    Some((tighten(ask, pts), reached))
}

/// Keeps only the corners of a grid path, then pulls it tight: from each
/// corner, the furthest later one that a clear one- or two-segment 45°
/// path reaches.
fn tighten(ask: &Ask, pts: Vec<P>) -> Vec<P> {
    let mut corners: Vec<P> = Vec::with_capacity(pts.len());
    for p in pts {
        if corners.len() >= 2 {
            let (a, b) = (corners[corners.len() - 2], corners[corners.len() - 1]);
            let cross = (b[0] - a[0]) * (p[1] - b[1]) - (b[1] - a[1]) * (p[0] - b[0]);
            if cross.abs() < 1e-6 {
                corners.pop();
            }
        }
        corners.push(p);
    }
    let mut out = vec![corners[0]];
    let mut i = 0;
    while i + 1 < corners.len() {
        let mut next = i + 1;
        let mut via = vec![corners[i + 1]];
        for j in (i + 2..corners.len()).rev() {
            if let Some(p) = join45(ask, corners[i], corners[j]) {
                next = j;
                via = p;
                break;
            }
        }
        out.extend(via);
        i = next;
    }
    out
}

/// A clear 45° path from `a` to `b` (one straight or diagonal segment, or
/// two), without `a`.
fn join45(ask: &Ask, a: P, b: P) -> Option<Vec<P>> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let straight = dx.abs() < 1e-6 || dy.abs() < 1e-6 || (dx.abs() - dy.abs()).abs() < 1e-6;
    if straight {
        return ask.clear(a, b).then(|| vec![b]);
    }
    for diag_first in [true, false] {
        let rest = crate::route::path45(a, b, diag_first);
        let mut path = vec![a];
        path.extend(rest.iter().copied());
        if path.windows(2).all(|w| ask.clear(w[0], w[1])) {
            return Some(rest);
        }
    }
    None
}

/// Board tracks moved out of the way of a new route, as edits to apply
/// after the route's own: removals, then the shoved copies.
pub struct Shove {
    pub remove: Vec<usize>,
    pub add: Vec<Track>,
}

impl Shove {
    /// The edits, for a board that will have `tracks` tracks when they
    /// start (after the route's own are in).
    pub fn edits(&self, board: &Board, tracks: usize) -> Vec<Edit> {
        let mut out = Vec::new();
        let mut remove = self.remove.clone();
        remove.sort_unstable_by(|a, b| b.cmp(a));
        for &i in &remove {
            out.push(Edit::RemoveTrack {
                index: i,
                track: board.geometry.tracks[i].clone(),
            });
        }
        for (at, t) in (tracks - remove.len()..).zip(&self.add) {
            out.push(Edit::InsertTrack {
                index: at,
                track: t.clone(),
            });
        }
        out
    }
}

/// Moves the tracks of other nets that `route` (capsules on `layer`)
/// runs into: each is routed again between its own ends around the new
/// copper and what is already there. `None` when something other than a
/// track is in the way, or a track cannot be moved.
pub fn shove(
    board: &Board,
    obstacles: &Obstacles,
    layer: u8,
    route: &[Capsule],
    nets: [Option<u32>; 2],
    clearance: f64,
) -> Option<Shove> {
    let mut hit = Vec::new();
    for &(a, b, r) in route {
        if !obstacles.tracks_hit(layer, a, b, r, nets, clearance, &mut hit) {
            return None;
        }
    }
    if hit.is_empty() {
        return None;
    }
    if hit.len() > 24 {
        return None;
    }
    let mut extra: Vec<Capsule> = route.to_vec();
    let mut add = Vec::new();
    // Nearest first, so the tracks next to the route move before the ones
    // behind them.
    let g = &board.geometry;
    let p = |q: Point| [q.x.mil(), q.y.mil()];
    for &ti in &hit {
        let t = &g.tracks[ti];
        let (a, b, r) = (p(t.start), p(t.end), t.width.mil() / 2.0);
        let ask = Ask {
            obstacles,
            layer,
            r,
            nets: [t.net, None],
            clearance,
            extra: &extra,
            skip: &hit,
        };
        // Its ends stay where they are, so they must be clear already.
        if !ask.clear(a, a) || !ask.clear(b, b) {
            return None;
        }
        let path = walkaround(&ask, a, b)?;
        for w in path.windows(2) {
            extra.push((w[0], w[1], r));
            add.push(Track {
                component: None,
                layer: LayerRef::Copper(layer),
                net: t.net,
                start: Point {
                    x: Length::from_mil(w[0][0]),
                    y: Length::from_mil(w[0][1]),
                },
                end: Point {
                    x: Length::from_mil(w[1][0]),
                    y: Length::from_mil(w[1][1]),
                },
                width: t.width,
                text: false,
            });
        }
    }
    Some(Shove { remove: hit, add })
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

    fn board(tracks: Vec<Track>, pads: Vec<Pad>) -> Board {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "Top".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            material: None,
        });
        b.geometry.tracks = tracks;
        b.geometry.pads = pads;
        b
    }

    fn pad(x: f64, y: f64, net: u32) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: None,
            name: String::new(),
            at: pt(x, y),
            size_x: Length::from_mil(40.0),
            size_y: Length::from_mil(40.0),
            hole: Length(0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    #[test]
    fn goes_round_a_pad_at_45_degrees() {
        let b = board(Vec::new(), vec![pad(100.0, 0.0, 1)]);
        let obs = Obstacles::new(&b);
        let ask = Ask {
            obstacles: &obs,
            layer: 0,
            r: 3.0,
            nets: [Some(0), None],
            clearance: 5.0,
            extra: &[],
            skip: &[],
        };
        let path = walkaround(&ask, [0.0, 0.0], [200.0, 0.0]).expect("a way round");
        assert!(path.len() > 2);
        for w in path.windows(2) {
            assert!(ask.clear(w[0], w[1]), "{path:?} {w:?}");
            let (dx, dy) = ((w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs());
            assert!(dx < 1e-6 || dy < 1e-6 || (dx - dy).abs() < 1e-6, "{w:?}");
        }
        // Not far off the straight line: around the pad, not the board.
        let len: f64 = path
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum();
        assert!(len < 260.0, "{len}");
    }

    #[test]
    fn stops_short_of_a_pad_it_cannot_get_round() {
        // The cursor is on a pad of another net: the route goes up to it.
        let b = board(Vec::new(), vec![pad(200.0, 0.0, 1)]);
        let obs = Obstacles::new(&b);
        let ask = Ask {
            obstacles: &obs,
            layer: 0,
            r: 3.0,
            nets: [Some(0), None],
            clearance: 5.0,
            extra: &[],
            skip: &[],
        };
        assert!(walkaround(&ask, [0.0, 0.0], [200.0, 0.0]).is_none());
        let (path, reached) = walk_toward(&ask, [0.0, 0.0], [200.0, 0.0]).expect("part way");
        assert!(!reached);
        let end = *path.last().unwrap();
        // Pad edge at 180, so the track's centre stops by 172.
        assert!(end[0] > 160.0 && end[0] <= 172.0 + 1e-6, "{path:?}");
        assert!(path.windows(2).all(|w| ask.clear(w[0], w[1])));
    }

    #[test]
    fn a_track_in_the_way_is_pushed_aside() {
        let t = Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: pt(0.0, 10.0),
            end: pt(300.0, 10.0),
            width: Length::from_mil(6.0),
            text: false,
        };
        let b = board(vec![t], Vec::new());
        let obs = Obstacles::new(&b);
        // A new track along y = 0 from x 100 to 200: 10 mil centre to
        // centre, too close for 5 mil clearance.
        let route = [([100.0, 0.0], [200.0, 0.0], 3.0)];
        let s = shove(&b, &obs, 0, &route, [Some(0), None], 5.0).expect("shoved");
        assert_eq!(s.remove, vec![0]);
        for t in &s.add {
            let (a, c) = (
                [t.start.x.mil(), t.start.y.mil()],
                [t.end.x.mil(), t.end.y.mil()],
            );
            let gap = crate::route::seg_seg(a, c, [100.0, 0.0], [200.0, 0.0]) - 6.0;
            assert!(gap >= 5.0 - 1e-3, "{gap}");
        }
        let ends = (s.add.first().unwrap().start, s.add.last().unwrap().end);
        assert_eq!(ends, (pt(0.0, 10.0), pt(300.0, 10.0)));
    }
}
