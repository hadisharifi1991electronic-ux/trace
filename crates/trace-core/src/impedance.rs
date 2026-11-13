//! Characteristic impedance of tracks from the board stack-up.
//!
//! Closed-form models, good to a few percent against field solvers for
//! usual PCB geometry:
//! - microstrip: Hammerstad and Jensen, with Wheeler's thickness correction;
//! - stripline: Wheeler's formula for a centred line, and the parallel-plate
//!   combination for an off-centre (dual) stripline;
//! - edge-coupled pairs: the IPC-2141 coupling factors.
//!
//! Lengths are in any one unit (mil is used here); only ratios matter.

use crate::{Board, LayerKind, LayerRef, Length};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Single-ended impedance of a surface microstrip.
/// `w` width, `t` copper thickness, `h` dielectric height to the plane.
pub fn microstrip(w: f64, t: f64, h: f64, er: f64) -> f64 {
    if w <= 0.0 || h <= 0.0 {
        return f64::NAN;
    }
    // Hammerstad and Jensen: a thick strip acts as a wider thin one.
    let u0 = w / h;
    let du = if t > 0.0 {
        let tn = t / h;
        let coth = 1.0 / (6.517 * u0).sqrt().tanh();
        let du1 = tn / PI * (1.0 + 4.0 * std::f64::consts::E / (tn * coth * coth)).ln();
        0.5 * (1.0 + 1.0 / (er - 1.0).max(0.0).sqrt().cosh()) * du1
    } else {
        0.0
    };
    let w = w + du * h;
    let u = w / h;
    let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
    let z_air = 60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln();
    let a = 1.0
        + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0
        + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
    let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
    let e_eff = (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b);
    z_air / e_eff.sqrt()
}

/// Effective permittivity of a microstrip (for propagation delay).
pub fn microstrip_eeff(w: f64, h: f64, er: f64) -> f64 {
    let u = w / h;
    let a = 1.0
        + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0
        + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
    let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
    (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b)
}

/// Centred stripline: `b` is the plane-to-plane spacing (including the
/// strip), `t` the strip thickness.
pub fn stripline(w: f64, t: f64, b: f64, er: f64) -> f64 {
    if w <= 0.0 || b <= t {
        return f64::NAN;
    }
    let bt = b - t;
    let x = t / b;
    let m = 6.0 / (3.0 + 2.0 * x / (1.0 - x));
    let dw = if t > 0.0 {
        bt * x / (PI * (1.0 - x))
            * (1.0
                - 0.5 * ((x / (2.0 - x)).powi(2) + (0.0796 * x / (w / b + 1.1 * x)).powf(m)).ln())
    } else {
        0.0
    };
    let wp = (w + dw) / bt;
    let a = 4.0 / (PI * wp);
    30.0 / er.sqrt() * (1.0 + a * (2.0 * a + (4.0 * a * a + 6.27).sqrt())).ln()
}

/// Off-centre stripline: `h1` and `h2` are the dielectric heights from the
/// strip's faces to the plane above and below.
pub fn dual_stripline(w: f64, t: f64, h1: f64, h2: f64, er: f64) -> f64 {
    let z1 = stripline(w, t, 2.0 * h1 + t, er);
    let z2 = stripline(w, t, 2.0 * h2 + t, er);
    2.0 * z1 * z2 / (z1 + z2)
}

/// Differential impedance of an edge-coupled microstrip pair with gap `s`.
pub fn microstrip_diff(z0: f64, s: f64, h: f64) -> f64 {
    2.0 * z0 * (1.0 - 0.48 * (-0.96 * s / h).exp())
}

/// Differential impedance of an edge-coupled stripline pair with gap `s`
/// between planes `b` apart.
pub fn stripline_diff(z0: f64, s: f64, b: f64) -> f64 {
    2.0 * z0 * (1.0 - 0.347 * (-2.9 * s / b).exp())
}

/// How a copper layer sits in the stack-up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerGeometry {
    /// Copper index from the top (0 = top).
    pub copper: u8,
    pub name: String,
    /// Copper thickness, mil.
    pub t: f64,
    /// Dielectric to the next copper above and below, mil (None at the
    /// board's surface).
    pub above: Option<f64>,
    pub below: Option<f64>,
    /// Permittivity of the dielectric next to the layer.
    pub er: f64,
}

impl LayerGeometry {
    pub fn is_outer(&self) -> bool {
        self.above.is_none() || self.below.is_none()
    }

    /// Single-ended impedance of a track of width `w` mil on this layer.
    pub fn z0(&self, w: f64) -> f64 {
        match (self.above, self.below) {
            (None, Some(h)) | (Some(h), None) => microstrip(w, self.t, h, self.er),
            (Some(h1), Some(h2)) => dual_stripline(w, self.t, h1, h2, self.er),
            (None, None) => f64::NAN,
        }
    }

    /// Differential impedance of a pair, width `w` and gap `s` mil.
    pub fn zdiff(&self, w: f64, s: f64) -> f64 {
        let z0 = self.z0(w);
        match (self.above, self.below) {
            (None, Some(h)) | (Some(h), None) => microstrip_diff(z0, s, h),
            (Some(h1), Some(h2)) => stripline_diff(z0, s, h1 + h2 + self.t),
            (None, None) => f64::NAN,
        }
    }

    /// The width that gives `target` ohms single-ended (bisection), mil.
    pub fn width_for(&self, target: f64) -> Option<f64> {
        solve(|w| self.z0(w), target, 0.5, 100.0)
    }

    /// The gap that gives `target` ohms differential at width `w`, mil.
    pub fn gap_for(&self, w: f64, target: f64) -> Option<f64> {
        solve(|s| self.zdiff(w, s), target, 0.5, 200.0)
    }

    /// Propagation delay, ps per inch.
    pub fn delay_ps_per_in(&self, w: f64) -> f64 {
        let e = match (self.above, self.below) {
            (None, Some(h)) | (Some(h), None) => microstrip_eeff(w, h, self.er),
            _ => self.er,
        };
        // c = 11.803 in/ns.
        1000.0 * e.sqrt() / 11.803
    }
}

/// Finds x in [lo, hi] with f(x) = target, for f falling as x grows.
fn solve(f: impl Fn(f64) -> f64, target: f64, lo: f64, hi: f64) -> Option<f64> {
    let (mut lo, mut hi) = (lo, hi);
    let (flo, fhi) = (f(lo), f(hi));
    if !(flo.is_finite() && fhi.is_finite()) {
        return None;
    }
    let falling = flo > fhi;
    if (target - flo) * (target - fhi) > 0.0 {
        return None;
    }
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let fm = f(mid);
        if (fm > target) == falling {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// Default permittivity when the stack-up has none (FR-4).
pub const DEFAULT_ER: f64 = 4.2;

impl Board {
    /// The geometry of each copper layer of the board's own stack-up.
    pub fn layer_geometry(&self) -> Vec<LayerGeometry> {
        layer_geometry(&self.stackup)
    }

    /// Impedance of every routed net, per layer and width it uses.
    pub fn net_impedance(&self) -> Vec<NetImpedance> {
        self.net_impedance_with(&self.layer_geometry())
    }
}

/// The geometry of each copper layer, top to bottom. Every copper layer is
/// taken as a possible reference plane.
pub fn layer_geometry(stackup: &crate::Stackup) -> Vec<LayerGeometry> {
    {
        let layers = &stackup.layers;
        let coppers: Vec<usize> = layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind == LayerKind::Copper)
            .map(|(i, _)| i)
            .collect();
        let gap = |a: usize, b: usize| -> (f64, f64) {
            // Total dielectric between two stack positions, and its mean Er
            // weighted by thickness.
            let (mut h, mut eh) = (0.0, 0.0);
            for l in &layers[a + 1..b] {
                if l.kind == LayerKind::Dielectric {
                    let t = l.thickness.mil();
                    h += t;
                    eh += t * l
                        .dielectric_constant
                        .filter(|e| *e > 1.0)
                        .unwrap_or(DEFAULT_ER);
                }
            }
            (h, if h > 0.0 { eh / h } else { DEFAULT_ER })
        };
        coppers
            .iter()
            .enumerate()
            .map(|(ci, &i)| {
                let above = (ci > 0).then(|| gap(coppers[ci - 1], i));
                let below = (ci + 1 < coppers.len()).then(|| gap(i, coppers[ci + 1]));
                let er = match (above, below) {
                    (Some(a), Some(b)) if a.0 + b.0 > 0.0 => (a.0 * a.1 + b.0 * b.1) / (a.0 + b.0),
                    (Some(a), _) => a.1,
                    (_, Some(b)) => b.1,
                    _ => DEFAULT_ER,
                };
                LayerGeometry {
                    copper: ci as u8,
                    name: layers[i].name.clone(),
                    t: layers[i].thickness.mil().max(0.0),
                    above: above.map(|a| a.0).filter(|h| *h > 0.0),
                    below: below.map(|b| b.0).filter(|h| *h > 0.0),
                    er,
                }
            })
            .collect()
    }
}

impl Board {
    /// Impedance of every routed net for a given layer geometry (an edited
    /// stack-up, say).
    pub fn net_impedance_with(&self, geo: &[LayerGeometry]) -> Vec<NetImpedance> {
        let pair_of = self.pair_partners();
        let mut acc: std::collections::HashMap<(u32, u8, i64), (f64, Vec<f64>)> =
            Default::default();
        for t in &self.geometry.tracks {
            let (Some(n), LayerRef::Copper(c)) = (t.net, &t.layer) else {
                continue;
            };
            let len = (t.end.x.mil() - t.start.x.mil()).hypot(t.end.y.mil() - t.start.y.mil());
            let e = acc.entry((n, *c, t.width.0)).or_default();
            e.0 += len;
            if let Some(&other) = pair_of.get(&n) {
                if let Some(g) = self.pair_gap(t, other) {
                    e.1.push(g);
                }
            }
        }
        let mut out: Vec<NetImpedance> = acc
            .into_iter()
            .filter_map(|((net, c, w), (length, mut gaps))| {
                let g = geo.get(c as usize)?;
                let width = Length(w).mil();
                gaps.sort_by(f64::total_cmp);
                let gap = (!gaps.is_empty()).then(|| gaps[gaps.len() / 2]);
                Some(NetImpedance {
                    net,
                    layer: c,
                    width,
                    length,
                    z0: g.z0(width),
                    gap,
                    zdiff: gap.map(|s| g.zdiff(width, s)),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            (a.net, a.layer)
                .cmp(&(b.net, b.layer))
                .then(b.length.total_cmp(&a.length))
        });
        out
    }

    /// Net index -> its differential partner.
    pub(crate) fn pair_partners(&self) -> std::collections::HashMap<u32, u32> {
        let idx = |name: &str| {
            self.nets
                .iter()
                .position(|n| n.name == name)
                .map(|i| i as u32)
        };
        let mut m = std::collections::HashMap::new();
        for p in &self.diff_pairs {
            if let (Some(a), Some(b)) = (idx(&p.positive), idx(&p.negative)) {
                m.insert(a, b);
                m.insert(b, a);
            }
        }
        m
    }

    /// Edge gap from a track to the nearest parallel track of `other` on the
    /// same layer, when they run side by side.
    pub(crate) fn pair_gap(&self, t: &crate::Track, other: u32) -> Option<f64> {
        let (ax, ay) = (t.start.x.mil(), t.start.y.mil());
        let (bx, by) = (t.end.x.mil(), t.end.y.mil());
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 5.0 {
            return None;
        }
        let (ux, uy) = (dx / len, dy / len);
        let mid = ((ax + bx) / 2.0, (ay + by) / 2.0);
        let mut best: Option<f64> = None;
        for o in &self.geometry.tracks {
            if o.net != Some(other) || o.layer != t.layer {
                continue;
            }
            let (cx, cy) = (o.start.x.mil(), o.start.y.mil());
            let (ex, ey) = (o.end.x.mil(), o.end.y.mil());
            let (odx, ody) = (ex - cx, ey - cy);
            let ol = (odx * odx + ody * ody).sqrt();
            if ol < 5.0 || (odx * uy - ody * ux).abs() / ol > 0.05 {
                continue; // not parallel
            }
            // Perpendicular distance from our midpoint to their line, and
            // whether our midpoint falls alongside their segment.
            let along = ((mid.0 - cx) * odx + (mid.1 - cy) * ody) / ol;
            if along < 0.0 || along > ol {
                continue;
            }
            let d = ((mid.0 - cx) * ody - (mid.1 - cy) * odx).abs() / ol;
            let gap = d - (t.width.mil() + o.width.mil()) / 2.0;
            if gap > 0.0 && gap < 50.0 && best.is_none_or(|b| gap < b) {
                best = Some(gap);
            }
        }
        best
    }
}

/// One net's impedance on one layer at one width.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetImpedance {
    pub net: u32,
    /// Copper index from the top.
    pub layer: u8,
    /// Track width, mil.
    pub width: f64,
    /// Routed length at this width on this layer, mil.
    pub length: f64,
    pub z0: f64,
    /// Typical edge gap to the pair partner, mil.
    pub gap: Option<f64>,
    pub zdiff: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn microstrip_matches_references() {
        // 10 mil wide, 1.4 mil copper, 5.5 mil FR-4 (Er 4.2): about 51 ohm.
        let z = microstrip(10.0, 1.4, 5.5, 4.2);
        assert!(near(z, 50.0, 3.5), "{z}");
        // Wider is lower.
        assert!(microstrip(20.0, 1.4, 5.5, 4.2) < z);
    }

    #[test]
    fn stripline_matches_references() {
        // 5 mil wide, 0.6 mil copper, 5 mil above and below, Er 4.2: the
        // IPC-2141 estimate is 43 ohm, field solvers about 45.
        let z = dual_stripline(5.0, 0.6, 5.0, 5.0, 4.2);
        assert!(near(z, 44.5, 2.0), "{z}");
        // Centred: the dual model reduces to the plain one.
        let s = stripline(5.0, 0.6, 10.6, 4.2);
        assert!(near(z, s, 0.01));
    }

    #[test]
    fn solves_for_width() {
        let g = LayerGeometry {
            copper: 0,
            name: "Top".into(),
            t: 1.4,
            above: None,
            below: Some(5.5),
            er: 4.2,
        };
        let w = g.width_for(50.0).unwrap();
        assert!(near(g.z0(w), 50.0, 0.01));
        let s = g.gap_for(w * 0.6, 100.0);
        if let Some(s) = s {
            assert!(near(g.zdiff(w * 0.6, s), 100.0, 0.05));
        }
    }
}
