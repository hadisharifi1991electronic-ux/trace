//! Small 2-D helpers: arcs, outlines of apertures and strokes, point tests
//! and polygon booleans. Units are millimetres.

use crate::gerber::{Aperture, PrimKind, Shape, Xform};
use std::f64::consts::{PI, TAU};

pub type Pt = [f64; 2];

/// Largest gap between an arc and its chords, in mm.
const CHORD_ERROR: f64 = 0.002;

pub fn rotate(p: Pt, deg: f64) -> Pt {
    if deg == 0.0 {
        return p;
    }
    let (s, c) = deg.to_radians().sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

pub fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}

/// Sweep angle in radians (0, 2π] going from `a` to `b` around `c`.
pub fn sweep(c: Pt, a: Pt, b: Pt, cw: bool) -> f64 {
    let ta = (a[1] - c[1]).atan2(a[0] - c[0]);
    let tb = (b[1] - c[1]).atan2(b[0] - c[0]);
    let mut s = if cw { ta - tb } else { tb - ta };
    s = s.rem_euclid(TAU);
    if s < 1e-9 {
        s = TAU;
    }
    s
}

fn segments_for(r: f64, sweep: f64) -> usize {
    if r <= CHORD_ERROR {
        return 4.max((sweep / (PI / 4.0)).ceil() as usize);
    }
    let step = 2.0 * (1.0 - CHORD_ERROR / r).clamp(-1.0, 1.0).acos();
    let step = step.clamp(PI / 180.0, PI / 8.0);
    ((sweep / step).ceil() as usize).clamp(2, 720)
}

/// Points along an arc, from `a` to `b` inclusive.
pub fn arc_points(c: Pt, a: Pt, b: Pt, cw: bool, full: bool) -> Vec<Pt> {
    let r = dist(c, a);
    let sw = if full { TAU } else { sweep(c, a, b, cw) };
    let n = segments_for(r, sw);
    let t0 = (a[1] - c[1]).atan2(a[0] - c[0]);
    let dir = if cw { -1.0 } else { 1.0 };
    let mut pts: Vec<Pt> = (0..=n)
        .map(|i| {
            let t = t0 + dir * sw * i as f64 / n as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect();
    if !full {
        *pts.last_mut().unwrap() = b;
    }
    pts
}

pub fn circle(c: Pt, d: f64) -> Vec<Pt> {
    let r = d / 2.0;
    let n = segments_for(r, TAU).max(8);
    (0..n)
        .map(|i| {
            let t = TAU * i as f64 / n as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

pub fn rect(c: Pt, w: f64, h: f64) -> Vec<Pt> {
    let (x, y) = (w / 2.0, h / 2.0);
    vec![
        [c[0] - x, c[1] - y],
        [c[0] + x, c[1] - y],
        [c[0] + x, c[1] + y],
        [c[0] - x, c[1] + y],
    ]
}

pub fn regular_polygon(c: Pt, d: f64, n: u32, rot_deg: f64) -> Vec<Pt> {
    let r = d / 2.0;
    (0..n)
        .map(|i| {
            let t = rot_deg.to_radians() + TAU * i as f64 / n as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

/// The four copper quarters of a thermal relief ring.
pub fn thermal_quarters(c: Pt, outer: f64, inner: f64, gap: f64) -> Vec<Vec<Pt>> {
    let (ro, ri) = (outer / 2.0, inner / 2.0);
    if ro <= 0.0 || ri >= ro {
        return Vec::new();
    }
    let half = |r: f64| ((gap / 2.0) / r).clamp(-1.0, 1.0).asin();
    let (ho, hi) = (half(ro), if ri > gap / 2.0 { half(ri) } else { PI / 4.0 });
    let mut out = Vec::new();
    for k in 0..4 {
        let base = k as f64 * PI / 2.0;
        let (a0, a1) = (base + ho, base + PI / 2.0 - ho);
        if a1 <= a0 {
            continue;
        }
        let n = segments_for(ro, a1 - a0).max(2);
        let mut pts: Vec<Pt> = (0..=n)
            .map(|i| {
                let t = a0 + (a1 - a0) * i as f64 / n as f64;
                [c[0] + ro * t.cos(), c[1] + ro * t.sin()]
            })
            .collect();
        let (b0, b1) = (base + hi, base + PI / 2.0 - hi);
        if ri > gap / 2.0 && b1 > b0 {
            let m = segments_for(ri, b1 - b0).max(2);
            pts.extend((0..=m).rev().map(|i| {
                let t = b0 + (b1 - b0) * i as f64 / m as f64;
                [c[0] + ri * t.cos(), c[1] + ri * t.sin()]
            }));
        } else {
            pts.push([
                c[0] + gap / 2.0 * (base + PI / 4.0).cos() * std::f64::consts::SQRT_2,
                c[1] + gap / 2.0 * (base + PI / 4.0).sin() * std::f64::consts::SQRT_2,
            ]);
        }
        out.push(pts);
    }
    out
}

/// A line of width `w` with square (butt) ends.
pub fn thick_line(a: Pt, b: Pt, w: f64) -> Vec<Pt> {
    let len = dist(a, b);
    if len < 1e-12 {
        return rect(a, w, w);
    }
    let (nx, ny) = (
        -(b[1] - a[1]) / len * w / 2.0,
        (b[0] - a[0]) / len * w / 2.0,
    );
    vec![
        [a[0] + nx, a[1] + ny],
        [a[0] - nx, a[1] - ny],
        [b[0] - nx, b[1] - ny],
        [b[0] + nx, b[1] + ny],
    ]
}

/// Obround (stadium) of overall size `w` by `h`.
pub fn obround(c: Pt, w: f64, h: f64) -> Vec<Pt> {
    if (w - h).abs() < 1e-9 {
        return circle(c, w);
    }
    let r = w.min(h) / 2.0;
    let half = (w.max(h) - w.min(h)) / 2.0;
    let (a, b) = if w > h {
        ([c[0] - half, c[1]], [c[0] + half, c[1]])
    } else {
        ([c[0], c[1] - half], [c[0], c[1] + half])
    };
    stadium(a, b, r)
}

/// Outline of a line from `a` to `b` with round ends of radius `r`.
pub fn stadium(a: Pt, b: Pt, r: f64) -> Vec<Pt> {
    let len = dist(a, b);
    if len < 1e-9 {
        return circle(a, 2.0 * r);
    }
    let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
    let n = segments_for(r, PI).max(4);
    let mut pts = Vec::with_capacity(2 * n + 2);
    for i in 0..=n {
        let t = ang - PI / 2.0 + PI * i as f64 / n as f64;
        pts.push([b[0] + r * t.cos(), b[1] + r * t.sin()]);
    }
    for i in 0..=n {
        let t = ang + PI / 2.0 + PI * i as f64 / n as f64;
        pts.push([a[0] + r * t.cos(), a[1] + r * t.sin()]);
    }
    pts
}

/// Outline of a polyline stroked with a round pen.
pub fn stroke_path(pts: &[Pt], w: f64) -> Vec<Vec<Pt>> {
    pts.windows(2)
        .map(|s| stadium(s[0], s[1], w / 2.0))
        .collect()
}

/// Convex hull (monotone chain), counter-clockwise.
pub fn hull(mut pts: Vec<Pt>) -> Vec<Pt> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| dist(*a, *b) < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Pt, a: Pt, b: Pt| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<Pt> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Pt> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Smallest rectangle around a point set: (centre, width, height, degrees).
pub fn min_rect(pts: &[Pt]) -> (Pt, f64, f64, f64) {
    let h = hull(pts.to_vec());
    if h.is_empty() {
        return ([0.0, 0.0], 0.0, 0.0, 0.0);
    }
    let mut best = (f64::MAX, [0.0, 0.0], 0.0, 0.0, 0.0);
    // Axis-aligned first, so square-ish shapes keep a zero rotation.
    let mut angles = vec![0.0];
    for i in 0..h.len() {
        let (a, b) = (h[i], h[(i + 1) % h.len()]);
        angles.push((b[1] - a[1]).atan2(b[0] - a[0]));
    }
    for t in angles {
        let (s, c) = t.sin_cos();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &h {
            let x = p[0] * c + p[1] * s;
            let y = -p[0] * s + p[1] * c;
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        let area = (x1 - x0) * (y1 - y0);
        if area < best.0 - 1e-9 {
            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            best = (
                area,
                [cx * c - cy * s, cx * s + cy * c],
                x1 - x0,
                y1 - y0,
                t.to_degrees(),
            );
        }
    }
    let (_, c, w, hgt, mut deg) = best;
    let (mut w, mut hgt) = (w, hgt);
    deg = deg.rem_euclid(180.0);
    if deg >= 90.0 - 1e-6 {
        deg -= 90.0;
        std::mem::swap(&mut w, &mut hgt);
    }
    if deg.abs() < 1e-6 {
        deg = 0.0;
    }
    (c, w, hgt, deg)
}

pub fn area(poly: &[Pt]) -> f64 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

pub fn bbox(pts: &[Pt]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

pub fn bbox_union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

pub fn bbox_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Even-odd point test over several rings.
pub fn inside(rings: &[Vec<Pt>], p: Pt) -> bool {
    let mut odd = false;
    for ring in rings {
        let n = ring.len();
        let mut j = n.wrapping_sub(1);
        for i in 0..n {
            let (a, b) = (ring[i], ring[j]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                odd = !odd;
            }
            j = i;
        }
    }
    odd
}

/// Distance from `p` to the segment `a`–`b`.
pub fn point_seg(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

pub fn seg_seg(a: Pt, b: Pt, c: Pt, d: Pt) -> f64 {
    let cross = |o: Pt, p: Pt, q: Pt| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    let (d1, d2, d3, d4) = (
        cross(c, d, a),
        cross(c, d, b),
        cross(a, b, c),
        cross(a, b, d),
    );
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return 0.0;
    }
    point_seg(a, c, d)
        .min(point_seg(b, c, d))
        .min(point_seg(c, a, b))
        .min(point_seg(d, a, b))
}

/// Filled outlines of a flashed aperture, as (dark, outline) pairs in order.
pub fn flash_shapes(ap: &Aperture, at: Pt, xf: &Xform) -> Vec<(bool, Vec<Pt>)> {
    let place =
        |pts: Vec<Pt>| -> Vec<Pt> { pts.into_iter().map(|p| add(xf.apply(p), at)).collect() };
    let o = [0.0, 0.0];
    let mut out = match &ap.shape {
        Shape::Circle(d) => vec![(true, place(circle(o, *d)))],
        Shape::Rect(w, h) => vec![(true, place(rect(o, *w, *h)))],
        Shape::Obround(w, h) => vec![(true, place(obround(o, *w, *h)))],
        Shape::Polygon { d, n, rot } => vec![(true, place(regular_polygon(o, *d, *n, *rot)))],
        Shape::Macro(prims) => prims
            .iter()
            .map(|p| match &p.kind {
                PrimKind::Circle { c, d } => (p.on, place(circle(*c, *d))),
                PrimKind::Outline(pts) => (p.on, place(pts.clone())),
            })
            .collect(),
        Shape::Block(_) => Vec::new(),
    };
    if ap.hole > 0.0 {
        out.push((false, place(circle(o, ap.hole))));
    }
    out
}

/// Outline of a straight draw with the given aperture.
pub fn line_shape(ap: &Aperture, a: Pt, b: Pt) -> Vec<Pt> {
    match &ap.shape {
        Shape::Rect(w, h) => {
            let mut pts = rect(a, *w, *h);
            pts.extend(rect(b, *w, *h));
            hull(pts)
        }
        _ => stadium(a, b, aperture_width(ap) / 2.0),
    }
}

/// Width of the pen when an aperture is used for drawing.
pub fn aperture_width(ap: &Aperture) -> f64 {
    match &ap.shape {
        Shape::Circle(d) => *d,
        Shape::Rect(w, h) | Shape::Obround(w, h) => w.min(*h),
        Shape::Polygon { d, .. } => *d,
        Shape::Macro(_) | Shape::Block(_) => {
            let pts: Vec<Pt> = flash_shapes(ap, [0.0, 0.0], &Xform::default())
                .into_iter()
                .filter(|s| s.0)
                .flat_map(|s| s.1)
                .collect();
            let b = bbox(&pts);
            if pts.is_empty() {
                0.0
            } else {
                (b[2] - b[0]).min(b[3] - b[1])
            }
        }
    }
}

/// `subject - clip` with polygon booleans; returns outer rings with holes.
pub fn difference(subject: &[Vec<Pt>], clip: &[Vec<Pt>]) -> Vec<Vec<Vec<Pt>>> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::single::SingleFloatOverlay;
    let subj: Vec<Vec<Pt>> = subject.to_vec();
    // Every cut removes copper, whichever way it was drawn: with mixed
    // directions two overlapping cuts would cancel under the non-zero rule.
    let clip: Vec<Vec<Pt>> = clip
        .iter()
        .map(|r| {
            let mut r = r.clone();
            if area(&r) < 0.0 {
                r.reverse();
            }
            r
        })
        .collect();
    subj.overlay(&clip, OverlayRule::Difference, FillRule::NonZero)
}

/// Union of rings, as outer rings with holes.
pub fn union(rings: &[Vec<Pt>]) -> Vec<Vec<Vec<Pt>>> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::float::simplify::SimplifyShape;
    let r: Vec<Vec<Pt>> = rings.to_vec();
    r.simplify_shape(FillRule::NonZero)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_rect_is_found() {
        let pts: Vec<Pt> = rect([1.0, 2.0], 2.0, 1.0)
            .into_iter()
            .map(|p| rotate(p, 30.0))
            .collect();
        let (_, w, h, deg) = min_rect(&pts);
        assert!((w - 2.0).abs() < 1e-6 && (h - 1.0).abs() < 1e-6, "{w} {h}");
        assert!((deg - 30.0).abs() < 1e-6, "{deg}");
    }

    #[test]
    fn overlapping_cuts_both_cut() {
        let mut cw = circle([0.0, 0.0], 2.0);
        cw.reverse();
        let out = difference(
            &[rect([0.0, 0.0], 10.0, 10.0)],
            &[rect([0.0, 0.0], 1.5, 1.5), cw],
        );
        let total: f64 = out
            .iter()
            .map(|s| s.iter().map(|r| area(r)).sum::<f64>())
            .sum();
        assert!(total.abs() < 100.0 - 3.0, "{total}");
    }

    #[test]
    fn hole_is_cut() {
        let out = difference(
            &[rect([0.0, 0.0], 10.0, 10.0)],
            &[rect([0.0, 0.0], 2.0, 2.0)],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 2);
    }
}
