//! Crosstalk between parallel tracks, for the simulator.
//!
//! Two tracks of different nets running side by side on one layer form a
//! coupled pair of lines. Their even and odd modes (Garg and Bahl's
//! capacitances for microstrip, Cohn's formulas for stripline) give the
//! mutual inductance and capacitance ratios, and from those the backward
//! (near-end) coupling Kb and forward (far-end) coupling Kf. The
//! aggressor's waves are taken as not disturbed by the victim (weak
//! coupling), and its noise enters the victim's lines as extra incoming
//! waves at their ends.

use crate::impedance::{microstrip, microstrip_eeff, LayerGeometry};
use crate::{Board, LayerRef};
use std::f64::consts::PI;

const E0: f64 = 8.854e-12;
const C0: f64 = 2.998e8;

/// K(k') / K(k), Hilberg's approximation.
fn kk(k: f64) -> f64 {
    let k = k.clamp(1e-12, 1.0 - 1e-12);
    let kp = (1.0 - k * k).sqrt();
    if k <= std::f64::consts::FRAC_1_SQRT_2 {
        (2.0 * (1.0 + kp.sqrt()) / (1.0 - kp.sqrt())).ln() / PI
    } else {
        PI / (2.0 * (1.0 + k.sqrt()) / (1.0 - k.sqrt())).ln()
    }
}

/// Even and odd mode impedances and effective permittivities of two
/// tracks `w` wide with edge gap `s` (mil) on layer `g`.
pub fn modes(g: &LayerGeometry, w: f64, s: f64) -> Option<(f64, f64, f64, f64)> {
    if w <= 0.0 || s <= 0.0 {
        return None;
    }
    match (g.above, g.below) {
        (Some(h1), Some(h2)) => {
            // Cohn, edge-coupled stripline between planes b apart.
            let b = h1 + h2 + g.t;
            let a = (PI * w / (2.0 * b)).tanh();
            let c = (PI * (w + s) / (2.0 * b)).tanh();
            let k = 30.0 * PI / g.er.sqrt();
            Some((k * kk(a * c), k * kk(a / c), g.er, g.er))
        }
        (None, Some(h)) | (Some(h), None) => {
            // Garg and Bahl, coupled microstrip.
            let caps = |er: f64| -> (f64, f64) {
                let z0 = microstrip(w, 0.0, h, er);
                let ee = microstrip_eeff(w, h, er);
                let cp = E0 * er * w / h;
                let cf = ee.sqrt() / (2.0 * C0 * z0) - cp / 2.0;
                let a = (-0.1 * (2.33 - 2.53 * w / h).exp()).exp();
                let cf2 = cf / (1.0 + a * (h / s) * (8.0 * s / h).tanh()) * (er / ee).sqrt();
                let k = (s / h) / (s / h + 2.0 * w / h);
                let cga = E0 * kk(k);
                let coth = 1.0 / (PI * s / (4.0 * h)).tanh();
                let cgd = E0 * er / PI * coth.ln()
                    + 0.65 * cf * (0.02 * er.sqrt() / (s / h) + 1.0 - 1.0 / (er * er));
                (cp + cf + cf2, cp + cf + cga + cgd)
            };
            let (ce, co) = caps(g.er);
            let (cea, coa) = caps(1.0);
            let z = |c: f64, ca: f64| 1.0 / (C0 * (c * ca).sqrt());
            Some((z(ce, cea), z(co, coa), ce / cea, co / coa))
        }
        (None, None) => None,
    }
}

/// Backward and forward coupling of two lines from their modes:
/// Kb = (Lm/L + Cm/C) / 4 and Kf = (Cm/C − Lm/L) / 2.
pub fn coefficients(ze: f64, zo: f64, ee: f64, eo: f64) -> (f64, f64) {
    let (ve, vo) = (C0 / ee.sqrt(), C0 / eo.sqrt());
    let (le, lo) = (ze / ve, zo / vo);
    let (ce, co) = (1.0 / (ze * ve), 1.0 / (zo * vo));
    let lm = (le - lo) / (le + lo);
    let cm = (co - ce) / (co + ce);
    ((lm + cm) / 4.0, (cm - lm) / 2.0)
}

/// A stretch where a victim track runs beside an aggressor track.
#[derive(Clone, Debug)]
pub struct Raw {
    pub victim: u32,
    pub vtrack: usize,
    /// Where the stretch starts and ends along the victim track (0 at its
    /// start, 1 at its end).
    pub x: (f64, f64),
    pub aggressor: u32,
    pub atrack: usize,
    /// The same two points along the aggressor track.
    pub r: (f64, f64),
    pub kb: f64,
    pub kf: f64,
    /// Length of the stretch, mil.
    pub len: f64,
}

/// Kb and Kf of two tracks `w` wide `s` apart on layer `g`, from the
/// field solver: solved at a few gaps from 0.1 to 6 dielectric heights
/// (kept for the next call) and interpolated on the log of the gap.
pub fn field_coupling(g: &LayerGeometry, w: f64, s: f64) -> Option<(f64, f64)> {
    let h = match (g.above, g.below) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        _ => return None,
    };
    let w = (w * 20.0).round().max(1.0) / 20.0;
    let gaps: Vec<f64> = (0..8)
        .map(|i| 0.1 * h * 60f64.powf(i as f64 / 7.0))
        .collect();
    let s = s.clamp(gaps[0], gaps[7]);
    let k = gaps.iter().position(|&x| x >= s).unwrap_or(7).max(1);
    let (a, b) = (
        crate::field::pair(g, w, gaps[k - 1])?,
        crate::field::pair(g, w, gaps[k])?,
    );
    let f = (s / gaps[k - 1]).ln() / (gaps[k] / gaps[k - 1]).ln();
    // Kb falls about as a power of the gap: interpolate its log.
    let kb = if a.kb > 0.0 && b.kb > 0.0 {
        (a.kb.ln() * (1.0 - f) + b.kb.ln() * f).exp()
    } else {
        a.kb * (1.0 - f) + b.kb * f
    };
    Some((kb, a.kf * (1.0 - f) + b.kf * f))
}

/// Every stretch where tracks of `victims` run parallel to tracks of
/// other nets within reach (four dielectric heights, at most 50 mil edge
/// to edge). Nets with copper areas (planes) are left out. With `field`,
/// the coupling comes from the field solver.
pub fn find(board: &Board, victims: &[u32], field: bool) -> Vec<Raw> {
    let geo = board.layer_geometry();
    let g = &board.geometry;
    let planes: std::collections::HashSet<u32> = g.polygons.iter().filter_map(|p| p.net).collect();
    let pt = |p: crate::Point| [p.x.mil(), p.y.mil()];
    let mut out = Vec::new();
    for (vi, v) in g.tracks.iter().enumerate() {
        let (Some(vn), &LayerRef::Copper(l)) = (v.net, &v.layer) else {
            continue;
        };
        if v.text || !victims.contains(&vn) {
            continue;
        }
        let Some(lg) = geo.get(l as usize) else {
            continue;
        };
        let h = match (lg.above, lg.below) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            _ => continue,
        };
        let reach = (4.0 * h).min(50.0);
        let (a, b) = (pt(v.start), pt(v.end));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 5.0 {
            continue;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let wv = v.width.mil();
        let (minx, maxx) = (a[0].min(b[0]) - reach - wv, a[0].max(b[0]) + reach + wv);
        let (miny, maxy) = (a[1].min(b[1]) - reach - wv, a[1].max(b[1]) + reach + wv);
        for (oi, o) in g.tracks.iter().enumerate() {
            let Some(on) = o.net else { continue };
            if o.layer != v.layer || o.text || victims.contains(&on) || planes.contains(&on) {
                continue;
            }
            let (p, q) = (pt(o.start), pt(o.end));
            if p[0].max(q[0]) < minx
                || p[0].min(q[0]) > maxx
                || p[1].max(q[1]) < miny
                || p[1].min(q[1]) > maxy
            {
                continue;
            }
            let olen = (q[0] - p[0]).hypot(q[1] - p[1]);
            if olen < 5.0 {
                continue;
            }
            let uo = [(q[0] - p[0]) / olen, (q[1] - p[1]) / olen];
            if (u[0] * uo[1] - u[1] * uo[0]).abs() > 0.1 {
                continue;
            }
            let along = |x: [f64; 2]| (x[0] - a[0]) * u[0] + (x[1] - a[1]) * u[1];
            let across = |x: [f64; 2]| u[0] * (x[1] - a[1]) - u[1] * (x[0] - a[0]);
            let (p1, p2) = (along(p), along(q));
            let (d1, d2) = (across(p), across(q));
            if d1.signum() != d2.signum() {
                continue;
            }
            let lo = p1.min(p2).max(0.0);
            let hi = p1.max(p2).min(len);
            if hi - lo < 5.0 {
                continue;
            }
            let w = (wv + o.width.mil()) / 2.0;
            let gap = (d1.abs() + d2.abs()) / 2.0 - w;
            if gap <= 0.1 || gap > reach {
                continue;
            }
            let Some((ze, zo, ee, eo)) = modes(lg, w, gap) else {
                continue;
            };
            if !(ze.is_finite() && zo.is_finite() && ze > zo) {
                continue;
            }
            let (kb, kf) = match field.then(|| field_coupling(lg, w, gap)).flatten() {
                Some(k) => k,
                None => coefficients(ze, zo, ee, eo),
            };
            let r = |x: f64| (x - p1) / (p2 - p1);
            out.push(Raw {
                victim: vn,
                vtrack: vi,
                x: (lo / len, hi / len),
                aggressor: on,
                atrack: oi,
                r: (r(lo), r(hi)),
                kb,
                kf,
                len: hi - lo,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(above: Option<f64>, below: Option<f64>) -> LayerGeometry {
        LayerGeometry {
            copper: 1,
            name: "L".into(),
            t: 0.0,
            above,
            below,
            er: 4.2,
            tan_d: None,
            rough_um: None,
            slabs_above: Vec::new(),
            slabs_below: Vec::new(),
            mask: None,
        }
    }

    #[test]
    fn modes_split_close_and_meet_far_apart() {
        for g in [layer(None, Some(4.0)), layer(Some(5.0), Some(5.0))] {
            let z0 = g.z0(5.0);
            let (ze, zo, _, _) = modes(&g, 5.0, 5.0).unwrap();
            assert!(zo < z0 && z0 < ze, "{zo} {z0} {ze}");
            // Zdiff against the IPC-2141 estimate, within 15 %.
            let zd = g.zdiff(5.0, 5.0);
            assert!((2.0 * zo - zd).abs() < 0.15 * zd, "{} {zd}", 2.0 * zo);
            let (ze, zo, _, _) = modes(&g, 5.0, 80.0).unwrap();
            assert!((ze - zo) / z0 < 0.03, "{ze} {zo}");
        }
    }

    #[test]
    fn stripline_has_no_far_end_crosstalk_and_microstrip_has_negative() {
        let (ze, zo, ee, eo) = modes(&layer(Some(5.0), Some(5.0)), 5.0, 5.0).unwrap();
        let (kb, kf) = coefficients(ze, zo, ee, eo);
        assert!(kb > 0.01 && kb < 0.1, "{kb}");
        assert!(kf.abs() < 1e-9);
        let (ze, zo, ee, eo) = modes(&layer(None, Some(4.0)), 5.0, 5.0).unwrap();
        let (kb, kf) = coefficients(ze, zo, ee, eo);
        assert!(kb > 0.01 && kb < 0.15, "{kb}");
        assert!(kf < 0.0, "{kf}");
    }
}
