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

/// How a route bends between fixed points, cycled with Shift+Space as in
/// Altium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    Line45,
    Arc45,
    Line90,
    Arc90,
    Any,
}

impl Corner {
    pub fn next(self) -> Self {
        match self {
            Corner::Line45 => Corner::Arc45,
            Corner::Arc45 => Corner::Line90,
            Corner::Line90 => Corner::Arc90,
            Corner::Arc90 => Corner::Any,
            Corner::Any => Corner::Line45,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Corner::Line45 => "45°",
            Corner::Arc45 => "45° arc",
            Corner::Line90 => "90°",
            Corner::Arc90 => "90° arc",
            Corner::Any => "any angle",
        }
    }
}

/// A straight run or a round corner of a route.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line(P, P),
    /// Stored counter-clockwise from `start_deg` to `end_deg`; `from` and
    /// `to` are the ends in the order the route runs through them.
    Arc {
        c: P,
        r: f64,
        start_deg: f64,
        end_deg: f64,
        from: P,
        to: P,
    },
}

impl Piece {
    pub fn start(&self) -> P {
        match *self {
            Piece::Line(a, _) => a,
            Piece::Arc { from, .. } => from,
        }
    }
    pub fn end(&self) -> P {
        match *self {
            Piece::Line(_, b) => b,
            Piece::Arc { to, .. } => to,
        }
    }
    pub fn length(&self) -> f64 {
        match *self {
            Piece::Line(a, b) => (b[0] - a[0]).hypot(b[1] - a[1]),
            Piece::Arc {
                r,
                start_deg,
                end_deg,
                ..
            } => r * (end_deg - start_deg).rem_euclid(360.0).to_radians(),
        }
    }
    /// Straight chords that follow the piece in route order, for drawing
    /// and clearance checks.
    pub fn chords(&self) -> Vec<(P, P)> {
        match *self {
            Piece::Line(a, b) => vec![(a, b)],
            Piece::Arc {
                c,
                r,
                start_deg,
                end_deg,
                from,
                to,
            } => {
                let sweep = (end_deg - start_deg).rem_euclid(360.0);
                let n = (sweep / 6.0).ceil().max(1.0) as usize;
                let mut pts: Vec<P> = (0..=n)
                    .map(|i| {
                        let t = (start_deg + sweep * i as f64 / n as f64).to_radians();
                        [c[0] + r * t.cos(), c[1] + r * t.sin()]
                    })
                    .collect();
                let d = |p: P, q: P| (p[0] - q[0]).hypot(p[1] - q[1]);
                if d(pts[0], from) > d(pts[n], from) {
                    pts.reverse();
                }
                pts[0] = from;
                pts[n] = to;
                pts.windows(2).map(|w| (w[0], w[1])).collect()
            }
        }
    }
}

/// The pieces of a route from `a` to `b` in the given corner mode. `flip`
/// swaps which run comes first (Space in Altium); `arc` is the size of a
/// round corner as a fraction of the largest that fits.
pub fn corner_path(a: P, b: P, mode: Corner, flip: bool, arc: f64) -> Vec<Piece> {
    let line = |p: P, q: P| -> Vec<Piece> {
        if (q[0] - p[0]).hypot(q[1] - p[1]) < 1e-6 {
            vec![]
        } else {
            vec![Piece::Line(p, q)]
        }
    };
    let mid = match mode {
        Corner::Any => return line(a, b),
        Corner::Line45 | Corner::Arc45 => {
            let p = path45(a, b, flip);
            if p.len() < 2 {
                return line(a, b);
            }
            p[0]
        }
        Corner::Line90 | Corner::Arc90 => {
            if (b[0] - a[0]).abs() < 1e-6 || (b[1] - a[1]).abs() < 1e-6 {
                return line(a, b);
            }
            if flip {
                [a[0], b[1]]
            } else {
                [b[0], a[1]]
            }
        }
    };
    if matches!(mode, Corner::Arc45 | Corner::Arc90) {
        if let Some(f) = fillet(a, mid, b, arc) {
            return f;
        }
    }
    let mut v = line(a, mid);
    v.extend(line(mid, b));
    v
}

/// A route shifted sideways by `d` (positive = to the left of the way it
/// runs), as for one track of a differential pair along a centre line.
/// Sharp corners between straight runs meet at their mitre; other breaks
/// are joined with a short straight piece.
pub fn offset(pieces: &[Piece], d: f64) -> Vec<Piece> {
    let shifted: Vec<Piece> = pieces
        .iter()
        .filter(|p| p.length() > 1e-9)
        .map(|p| match *p {
            Piece::Line(a, b) => {
                let l = (b[0] - a[0]).hypot(b[1] - a[1]);
                let n = [-(b[1] - a[1]) / l * d, (b[0] - a[0]) / l * d];
                Piece::Line([a[0] + n[0], a[1] + n[1]], [b[0] + n[0], b[1] + n[1]])
            }
            Piece::Arc {
                c,
                r,
                start_deg,
                end_deg,
                from,
                to,
            } => {
                let at_start = {
                    let t = start_deg.to_radians();
                    let q = [c[0] + r * t.cos(), c[1] + r * t.sin()];
                    (q[0] - from[0]).hypot(q[1] - from[1]) < 1e-6 * r.max(1.0)
                };
                // Counter-clockwise travel has the centre on its left.
                let r2 = if at_start { r - d } else { r + d }.max(1e-6);
                let k = r2 / r;
                let sc = |p: P| [c[0] + (p[0] - c[0]) * k, c[1] + (p[1] - c[1]) * k];
                Piece::Arc {
                    c,
                    r: r2,
                    start_deg,
                    end_deg,
                    from: sc(from),
                    to: sc(to),
                }
            }
        })
        .collect();
    let mut out: Vec<Piece> = Vec::with_capacity(shifted.len());
    for p in shifted {
        if let Some(last) = out.last_mut() {
            let (e, s) = (last.end(), p.start());
            if (e[0] - s[0]).hypot(e[1] - s[1]) > 1e-6 {
                if let (Piece::Line(a, b), Piece::Line(c, dd)) = (*last, p) {
                    if let Some(x) = line_cross(a, b, c, dd) {
                        *last = Piece::Line(a, x);
                        out.push(Piece::Line(x, dd));
                        continue;
                    }
                }
                out.push(Piece::Line(e, s));
            }
        }
        out.push(p);
    }
    out
}

/// Where the infinite lines through a-b and c-d meet, unless parallel.
fn line_cross(a: P, b: P, c: P, d: P) -> Option<P> {
    let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-9 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    Some([a[0] + t * r[0], a[1] + t * r[1]])
}

/// Rounds the corner at `m` between legs to `a` and `b`.
fn fillet(a: P, m: P, b: P, frac: f64) -> Option<Vec<Piece>> {
    let (la, lb) = (
        (a[0] - m[0]).hypot(a[1] - m[1]),
        (b[0] - m[0]).hypot(b[1] - m[1]),
    );
    if la < 1e-6 || lb < 1e-6 {
        return None;
    }
    let u1 = [(a[0] - m[0]) / la, (a[1] - m[1]) / la];
    let u2 = [(b[0] - m[0]) / lb, (b[1] - m[1]) / lb];
    let theta = (u1[0] * u2[0] + u1[1] * u2[1]).clamp(-1.0, 1.0).acos();
    if theta < 1e-3 || (std::f64::consts::PI - theta) < 1e-3 {
        return None;
    }
    let half = theta / 2.0;
    let t = la.min(lb) * frac.clamp(0.05, 1.0);
    let r = t * half.tan();
    let p1 = [m[0] + u1[0] * t, m[1] + u1[1] * t];
    let p2 = [m[0] + u2[0] * t, m[1] + u2[1] * t];
    let (wx, wy) = (u1[0] + u2[0], u1[1] + u2[1]);
    let wl = wx.hypot(wy);
    let k = r / half.sin() / wl;
    let c = [m[0] + wx * k, m[1] + wy * k];
    let ang = |p: P| (p[1] - c[1]).atan2(p[0] - c[0]).to_degrees();
    let (s1, s2) = (ang(p1), ang(p2));
    let (start_deg, end_deg) = if (s2 - s1).rem_euclid(360.0) <= 180.0 {
        (s1, s2)
    } else {
        (s2, s1)
    };
    let mut v = Vec::new();
    if la - t > 1e-6 {
        v.push(Piece::Line(a, p1));
    }
    v.push(Piece::Arc {
        c,
        r,
        start_deg,
        end_deg,
        from: p1,
        to: p2,
    });
    if lb - t > 1e-6 {
        v.push(Piece::Line(p2, b));
    }
    Some(v)
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
        self.check(Some(layer), a, b, w / 2.0, [net, None], clearance)
    }

    /// As [`Obstacles::check_track`] for one track of a pair: copper of
    /// either net of the pair is not an obstacle.
    pub fn check_pair_track(
        &self,
        layer: u8,
        a: P,
        b: P,
        w: f64,
        nets: [Option<u32>; 2],
        clearance: f64,
    ) -> Option<Hit> {
        self.check(Some(layer), a, b, w / 2.0, nets, clearance)
    }

    /// A through via of a pair.
    pub fn check_pair_via(
        &self,
        c: P,
        d: f64,
        nets: [Option<u32>; 2],
        clearance: f64,
    ) -> Option<Hit> {
        self.check(None, c, c, d / 2.0, nets, clearance)
    }

    /// The same for a through via of diameter `d` at `c`.
    pub fn check_via(&self, c: P, d: f64, net: Option<u32>, clearance: f64) -> Option<Hit> {
        self.check(None, c, c, d / 2.0, [net, None], clearance)
    }

    fn check(
        &self,
        layer: Option<u8>,
        a: P,
        b: P,
        r: f64,
        nets: [Option<u32>; 2],
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
                    if o.net.is_some() && nets.contains(&o.net) {
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
    fn round_corners() {
        // 90° arc: quarter circle of radius 40 tangent to both legs.
        let v = corner_path([0.0, 0.0], [100.0, 40.0], Corner::Arc90, false, 1.0);
        assert_eq!(v.len(), 2);
        let Piece::Arc { c, r, from, to, .. } = v[1] else {
            panic!("{v:?}")
        };
        assert!(
            (r - 40.0).abs() < 1e-9 && (c[0] - 60.0).abs() < 1e-9 && (c[1] - 40.0).abs() < 1e-9
        );
        assert!((from[0] - 60.0).abs() < 1e-9 && (to[1] - 40.0).abs() < 1e-9);
        let ch = v[1].chords();
        assert_eq!(ch.first().unwrap().0, from);
        assert_eq!(ch.last().unwrap().1, to);
        assert!((v[1].length() - 40.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // 45° arc ends where the route ends.
        let v = corner_path([0.0, 0.0], [100.0, 40.0], Corner::Arc45, false, 1.0);
        assert_eq!(v.last().unwrap().end(), [100.0, 40.0]);
        assert_eq!(
            corner_path([0.0, 0.0], [10.0, 30.0], Corner::Any, false, 1.0).len(),
            1
        );
        assert_eq!(
            corner_path([0.0, 0.0], [10.0, 30.0], Corner::Line90, true, 1.0)[0],
            Piece::Line([0.0, 0.0], [0.0, 30.0])
        );
    }

    #[test]
    fn pair_offsets() {
        // An L: right then up. The left track (+d) runs inside the corner.
        let c = vec![
            Piece::Line([0.0, 0.0], [100.0, 0.0]),
            Piece::Line([100.0, 0.0], [100.0, 100.0]),
        ];
        let l = offset(&c, 5.0);
        assert_eq!(
            l,
            vec![
                Piece::Line([0.0, 5.0], [95.0, 5.0]),
                Piece::Line([95.0, 5.0], [95.0, 100.0]),
            ]
        );
        let r = offset(&c, -5.0);
        assert_eq!(r[0].end(), [105.0, -5.0]);
        // Round corners keep their centre: the inner track has the smaller radius.
        let arc = corner_path([0.0, 0.0], [100.0, 40.0], Corner::Arc90, false, 1.0);
        let inner = offset(&arc, 5.0);
        let Piece::Arc { r, .. } = inner[1] else {
            panic!()
        };
        assert!((r - 35.0).abs() < 1e-9);
        assert!((inner[0].end()[0] - inner[1].start()[0]).abs() < 1e-9);
        let outer = offset(&arc, -5.0);
        let Piece::Arc { r, .. } = outer[1] else {
            panic!()
        };
        assert!((r - 45.0).abs() < 1e-9);
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
