//! Interactive routing support: 45° paths, and the copper of other nets a
//! new track or via must keep its clearance from. Units are mil.

use crate::{Board, LayerRef, Length, PadShape};
use std::collections::HashMap;

pub type P = [f64; 2];

/// Clearance used when the board carries no rule.
pub const DEFAULT_CLEARANCE: f64 = 6.0;
/// Track width used when the board carries no rule.
pub const DEFAULT_WIDTH: f64 = 8.0;

/// The corners of a route from `a` to `b` made of one straight and one 45°
/// run. `diagonal_first` picks which comes first. Returns the points after
/// `a`, ending with `b`.
pub fn path45(a: P, b: P, diagonal_first: bool) -> Vec<P> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax < 1e-6 || ay < 1e-6 || (ax - ay).abs() < 1e-6 {
        return vec![b];
    }
    let (sx, sy) = (dx.signum(), dy.signum());
    let d = ax.min(ay);
    let mid = if diagonal_first {
        [a[0] + sx * d, a[1] + sy * d]
    } else if ax > ay {
        [a[0] + sx * (ax - ay), a[1]]
    } else {
        [a[0], a[1] + sy * (ay - ax)]
    };
    vec![mid, b]
}

/// Board-wide clearance, from an enabled Altium-style `Clearance` rule
/// that applies to everything, else [`DEFAULT_CLEARANCE`].
pub fn board_clearance(board: &Board) -> f64 {
    rule_param(board, "Clearance", &["GAP"]).unwrap_or(DEFAULT_CLEARANCE)
}

/// Preferred track width from a `Width` rule that applies to everything.
pub fn board_width(board: &Board) -> f64 {
    rule_param(
        board,
        "Width",
        &["PREFEREDWIDTH", "PREFERREDWIDTH", "MINLIMIT"],
    )
    .unwrap_or(DEFAULT_WIDTH)
}

fn rule_param(board: &Board, kind: &str, keys: &[&str]) -> Option<f64> {
    let mut rules: Vec<_> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case(kind))
        .collect();
    // Rules for everything are the lowest priority (highest number) in Altium.
    rules.sort_by_key(|r| {
        let all = r.scope.iter().all(|s| s.trim().eq_ignore_ascii_case("All"));
        (!all, -r.priority)
    });
    rules.iter().find_map(|r| {
        keys.iter().find_map(|k| {
            r.params
                .iter()
                .find(|(pk, _)| pk.eq_ignore_ascii_case(k))
                .and_then(|(_, v)| Length::parse(v))
                .map(|l| l.mil())
                .filter(|&v| v > 0.0)
        })
    })
}

/// The via size used most on the board (diameter, hole), or 20/10 mil.
pub fn common_via(board: &Board) -> (f64, f64) {
    let mut count: HashMap<(i64, i64), usize> = HashMap::new();
    for v in &board.geometry.vias {
        *count.entry((v.diameter.0, v.hole.0)).or_default() += 1;
    }
    count
        .into_iter()
        .max_by_key(|&(k, n)| (n, k))
        .map(|((d, h), _)| (Length(d).mil(), Length(h).mil()))
        .unwrap_or((20.0, 10.0))
}

#[derive(Clone, Debug)]
pub enum Shape {
    /// A segment swept by a radius; a point when both ends are the same.
    Capsule { a: P, b: P, r: f64 },
    /// A filled convex or concave outline.
    Poly(Vec<P>),
}

impl Shape {
    fn bbox(&self) -> [f64; 4] {
        match self {
            Shape::Capsule { a, b, r } => [
                a[0].min(b[0]) - r,
                a[1].min(b[1]) - r,
                a[0].max(b[0]) + r,
                a[1].max(b[1]) + r,
            ],
            Shape::Poly(v) => v
                .iter()
                .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                }),
        }
    }

    /// Gap between this shape and a capsule (negative when they overlap).
    fn gap(&self, a: P, b: P, r: f64) -> f64 {
        match self {
            Shape::Capsule { a: c, b: d, r: s } => seg_seg(a, b, *c, *d) - r - s,
            Shape::Poly(v) => {
                if v.len() < 3 {
                    return f64::MAX;
                }
                if inside(a, v) || inside(b, v) {
                    return -r;
                }
                let mut best = f64::MAX;
                for i in 0..v.len() {
                    let (p, q) = (v[i], v[(i + 1) % v.len()]);
                    best = best.min(seg_seg(a, b, p, q));
                }
                best - r
            }
        }
    }
}

/// Where a new piece of copper breaks clearance.
#[derive(Clone, Debug)]
pub struct Hit {
    /// The gap found, which is less than the clearance (negative = overlap).
    pub gap: f64,
    pub net: Option<u32>,
    /// A point on the obstacle, to mark it.
    pub near: P,
}

struct Obstacle {
    /// Copper layer, or `None` for every layer.
    layer: Option<u8>,
    net: Option<u32>,
    shape: Shape,
}

const CELL: f64 = 100.0;

/// Other copper a route must keep clear of, indexed for quick lookups.
pub struct Obstacles {
    items: Vec<Obstacle>,
    grid: HashMap<(i32, i32), Vec<usize>>,
}

fn pt(p: crate::Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn rect(c: P, sx: f64, sy: f64, rot: f64) -> Vec<P> {
    let (s, co) = rot.to_radians().sin_cos();
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|&(u, v)| {
            let (u, v) = (u * sx / 2.0, v * sy / 2.0);
            [c[0] + u * co - v * s, c[1] + u * s + v * co]
        })
        .collect()
}

fn land(c: P, shape: PadShape, sx: f64, sy: f64, rot: f64, outline: &[crate::Point]) -> Shape {
    if outline.len() >= 3 {
        return Shape::Poly(outline.iter().map(|&p| pt(p)).collect());
    }
    match shape {
        PadShape::Round => {
            // Round pads with unequal sides are ovals: a capsule.
            let (long, short) = (sx.max(sy), sx.min(sy));
            let half = (long - short) / 2.0;
            let ang = if sx >= sy { rot } else { rot + 90.0 }.to_radians();
            let (dx, dy) = (half * ang.cos(), half * ang.sin());
            Shape::Capsule {
                a: [c[0] - dx, c[1] - dy],
                b: [c[0] + dx, c[1] + dy],
                r: short / 2.0,
            }
        }
        PadShape::Rect | PadShape::Octagon => Shape::Poly(rect(c, sx, sy, rot)),
    }
}

impl Obstacles {
    pub fn new(board: &Board) -> Self {
        let copper = board.stackup.copper_layers().count().max(1) as u8;
        let g = &board.geometry;
        let mut items = Vec::new();
        let layer_of = |l: &LayerRef| match l {
            LayerRef::Copper(i) => Some(Some(*i)),
            LayerRef::AllCopper => Some(None),
            _ => None,
        };
        for t in &g.tracks {
            if let Some(layer) = layer_of(&t.layer) {
                items.push(Obstacle {
                    layer,
                    net: t.net,
                    shape: Shape::Capsule {
                        a: pt(t.start),
                        b: pt(t.end),
                        r: t.width.mil() / 2.0,
                    },
                });
            }
        }
        for a in &g.arcs {
            let Some(layer) = layer_of(&a.layer) else {
                continue;
            };
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
                items.push(Obstacle {
                    layer,
                    net: a.net,
                    shape: Shape::Capsule {
                        a: at(i),
                        b: at(i + 1),
                        r: a.width.mil() / 2.0,
                    },
                });
            }
        }
        for v in &g.vias {
            let c = pt(v.at);
            let (lo, hi) = (
                v.from_layer.min(v.to_layer),
                v.from_layer.max(v.to_layer).min(copper - 1),
            );
            for l in lo..=hi {
                let d = v
                    .rings
                    .iter()
                    .find(|(rl, _)| *rl == l)
                    .map(|(_, d)| d.mil())
                    .unwrap_or(v.diameter.mil());
                items.push(Obstacle {
                    layer: Some(l),
                    net: v.net,
                    shape: Shape::Capsule {
                        a: c,
                        b: c,
                        r: d.max(v.hole.mil()) / 2.0,
                    },
                });
            }
        }
        for p in &g.pads {
            let c = pt(p.at);
            let Some(layer) = layer_of(&p.layer) else {
                continue;
            };
            if p.stack.is_empty() {
                items.push(Obstacle {
                    layer,
                    net: p.net,
                    shape: land(
                        c,
                        p.shape,
                        p.size_x.mil(),
                        p.size_y.mil(),
                        p.rotation_deg,
                        &p.outline,
                    ),
                });
            } else {
                for l in &p.stack {
                    items.push(Obstacle {
                        layer: Some(l.layer),
                        net: p.net,
                        shape: land(
                            c,
                            l.shape,
                            l.size_x.mil(),
                            l.size_y.mil(),
                            l.rotation_deg,
                            &l.outline,
                        ),
                    });
                }
            }
            // The drilled hole goes through every layer.
            if p.hole.0 > 0 {
                items.push(Obstacle {
                    layer: None,
                    net: p.net,
                    shape: Shape::Capsule {
                        a: c,
                        b: c,
                        r: p.hole.mil() / 2.0,
                    },
                });
            }
        }
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, o) in items.iter().enumerate() {
            let b = o.shape.bbox();
            for gx in (b[0] / CELL).floor() as i32..=(b[2] / CELL).floor() as i32 {
                for gy in (b[1] / CELL).floor() as i32..=(b[3] / CELL).floor() as i32 {
                    grid.entry((gx, gy)).or_default().push(i);
                }
            }
        }
        Obstacles { items, grid }
    }

    /// The worst clearance break for a track from `a` to `b` of width `w`
    /// on `layer` carrying `net`, if any copper of another net is closer
    /// than `clearance`.
    pub fn check_track(
        &self,
        layer: u8,
        a: P,
        b: P,
        w: f64,
        net: Option<u32>,
        clearance: f64,
    ) -> Option<Hit> {
        self.check(Some(layer), a, b, w / 2.0, net, clearance)
    }

    /// The same for a through via of diameter `d` at `c`.
    pub fn check_via(&self, c: P, d: f64, net: Option<u32>, clearance: f64) -> Option<Hit> {
        self.check(None, c, c, d / 2.0, net, clearance)
    }

    fn check(
        &self,
        layer: Option<u8>,
        a: P,
        b: P,
        r: f64,
        net: Option<u32>,
        clearance: f64,
    ) -> Option<Hit> {
        let reach = r + clearance;
        let (x0, y0) = (a[0].min(b[0]) - reach, a[1].min(b[1]) - reach);
        let (x1, y1) = (a[0].max(b[0]) + reach, a[1].max(b[1]) + reach);
        let mut seen = std::collections::HashSet::new();
        let mut worst: Option<Hit> = None;
        for gx in (x0 / CELL).floor() as i32..=(x1 / CELL).floor() as i32 {
            for gy in (y0 / CELL).floor() as i32..=(y1 / CELL).floor() as i32 {
                let Some(list) = self.grid.get(&(gx, gy)) else {
                    continue;
                };
                for &i in list {
                    if !seen.insert(i) {
                        continue;
                    }
                    let o = &self.items[i];
                    if net.is_some() && o.net == net {
                        continue;
                    }
                    if let (Some(l), Some(ol)) = (layer, o.layer) {
                        if l != ol {
                            continue;
                        }
                    }
                    let gap = o.shape.gap(a, b, r);
                    if gap < clearance - 1e-3 && worst.as_ref().is_none_or(|w| gap < w.gap) {
                        let near = match &o.shape {
                            Shape::Capsule { a, b, .. } => {
                                [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
                            }
                            Shape::Poly(v) => {
                                let n = v.len().max(1) as f64;
                                v.iter()
                                    .fold([0.0, 0.0], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n])
                            }
                        };
                        worst = Some(Hit {
                            gap,
                            net: o.net,
                            near,
                        });
                    }
                }
            }
        }
        worst
    }
}

fn point_seg(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn seg_seg(a: P, b: P, c: P, d: P) -> f64 {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    point_seg(a, c, d)
        .min(point_seg(b, c, d))
        .min(point_seg(c, a, b))
        .min(point_seg(d, a, b))
}

fn inside(p: P, v: &[P]) -> bool {
    let mut odd = false;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let (a, b) = (v[i], v[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            odd = !odd;
        }
        j = i;
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_45() {
        let p = path45([0.0, 0.0], [100.0, 40.0], false);
        assert_eq!(p, vec![[60.0, 0.0], [100.0, 40.0]]);
        let p = path45([0.0, 0.0], [100.0, 40.0], true);
        assert_eq!(p, vec![[40.0, 40.0], [100.0, 40.0]]);
        let p = path45([0.0, 0.0], [-30.0, -90.0], false);
        assert_eq!(p, vec![[0.0, -60.0], [-30.0, -90.0]]);
        assert_eq!(path45([0.0, 0.0], [50.0, 0.0], false).len(), 1);
    }

    #[test]
    fn gaps() {
        let s = Shape::Capsule {
            a: [0.0, 0.0],
            b: [100.0, 0.0],
            r: 5.0,
        };
        // Parallel track 20 mil away centre to centre, both 10 mil wide.
        assert!((s.gap([0.0, 20.0], [100.0, 20.0], 5.0) - 10.0).abs() < 1e-9);
        // Crossing.
        assert!(s.gap([50.0, -50.0], [50.0, 50.0], 5.0) < 0.0);
        let r = Shape::Poly(rect([0.0, 0.0], 20.0, 10.0, 0.0));
        assert!((r.gap([0.0, 15.0], [0.0, 15.0], 2.0) - 8.0).abs() < 1e-9);
        assert!(r.gap([0.0, 0.0], [1.0, 0.0], 2.0) < 0.0);
    }
}
