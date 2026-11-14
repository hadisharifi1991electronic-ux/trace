//! Differential pair cross-sections for a target impedance: every width
//! the rules allow, the gap that gives the target with the 2D field
//! solver, and what each costs in loss and crosstalk, best first.

use crate::field;
use crate::impedance::LayerGeometry;

const C0: f64 = 299_792_458.0;

/// What to look for.
#[derive(Clone, Debug)]
pub struct PairGoal {
    /// Differential impedance, Ω.
    pub zdiff: f64,
    /// Narrowest track and gap allowed, widest track wanted, mil.
    pub w_min: f64,
    pub s_min: f64,
    pub w_max: f64,
    /// Frequency for the loss, Hz (the link's Nyquist frequency).
    pub f: f64,
    /// Loss tangent and roughness where the stack-up gives none.
    pub tan_d: f64,
    pub rough_um: Option<f64>,
    /// Edge-to-edge distance to the next pair's nearest track, mil (None:
    /// three times the height over the nearest plane).
    pub neighbour: Option<f64>,
}

impl PairGoal {
    pub fn new(zdiff: f64) -> PairGoal {
        PairGoal {
            zdiff,
            w_min: 3.0,
            s_min: 3.0,
            w_max: 20.0,
            f: 8e9,
            tan_d: 0.02,
            rough_um: None,
            neighbour: None,
        }
    }
}

/// One cross-section that meets the target.
#[derive(Clone, Debug)]
pub struct PairOption {
    pub w: f64,
    pub s: f64,
    pub zdiff: f64,
    pub zcomm: f64,
    /// Differential loss at the goal's frequency, dB per inch, and its
    /// copper and dielectric parts.
    pub loss: f64,
    pub loss_cu: f64,
    pub loss_diel: f64,
    /// Delay, ps per inch (odd mode).
    pub delay: f64,
    /// Coupling inside the pair: |Zcomm − Zdiff/4| over Zdiff/4 tells
    /// how tightly the two tracks hold each other; 0 is uncoupled.
    pub coupling: f64,
    /// Near-end crosstalk from the next pair at the goal's distance, %.
    pub next: f64,
    /// Far-end coupling (microstrip only), %.
    pub fext: f64,
    /// Space the pair takes: two tracks and the gap, mil.
    pub span: f64,
    /// How much Zdiff moves for 0.5 mil of etch on the width, Ω.
    pub etch: f64,
}

/// Every width from the goal's narrowest to its widest in about twenty
/// steps, with the gap for the target, scored. Sorted by loss.
pub fn pair_options(g: &LayerGeometry, goal: &PairGoal) -> Vec<PairOption> {
    let h = match (g.above, g.below) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        _ => return Vec::new(),
    };
    let neighbour = goal.neighbour.unwrap_or(3.0 * h);
    let steps = 20;
    let lo = goal.w_min.max(0.5);
    let hi = goal.w_max.max(lo);
    let mut widths: Vec<f64> = (0..=steps)
        .map(|k| lo + (hi - lo) * k as f64 / steps as f64)
        .collect();
    widths.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    let solve = |w: f64| -> Option<PairOption> {
        let s = g.gap_for_field(w, goal.zdiff)?;
        if s < goal.s_min || !s.is_finite() {
            return None;
        }
        let p = field::pair(g, w, s)?;
        if (p.zdiff - goal.zdiff).abs() > goal.zdiff * 0.03 {
            return None;
        }
        let f = goal.f;
        let k = goal.rough_um.map_or(1.0, |r| crate::sim::loss::rough(f, r));
        let r = (p.r_odd_ghz * (f / 1e9).sqrt() * k).hypot(p.single.r_dc);
        let tan = if p.tan_odd > 0.0 {
            p.tan_odd
        } else {
            goal.tan_d * p.single.fill.max(0.5)
        };
        let alpha_c = r / (2.0 * p.z_odd);
        let alpha_d = std::f64::consts::PI * f * p.e_odd.sqrt() / C0 * tan;
        let to_db = 8.686 * 0.0254;
        let (kb, kf) = crate::sim::xtalk::field_coupling(g, w, neighbour).unwrap_or((0.0, 0.0));
        let etch =
            field::pair(g, (w - 0.5).max(0.1), s + 0.5).map_or(0.0, |q| (q.zdiff - p.zdiff).abs());
        Some(PairOption {
            w,
            s,
            zdiff: p.zdiff,
            zcomm: p.zcomm,
            loss: (alpha_c + alpha_d) * to_db,
            loss_cu: alpha_c * to_db,
            loss_diel: alpha_d * to_db,
            delay: p.e_odd.sqrt() / C0 * 0.0254 * 1e12,
            coupling: ((p.zcomm - p.zdiff / 4.0) / (p.zdiff / 4.0)).abs(),
            next: kb.abs() * 100.0,
            fext: kf.abs() * 100.0,
            span: 2.0 * w + s,
            etch,
        })
    };
    // Solve in parallel: each width is a few field solves.
    let mut out: Vec<PairOption> = std::thread::scope(|sc| {
        let hs: Vec<_> = widths.iter().map(|&w| sc.spawn(move || solve(w))).collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    out.sort_by(|a, b| a.loss.total_cmp(&b.loss));
    out
}

/// The pick among `options` (sorted by loss): the lowest loss whose
/// crosstalk stays under `next_max` % and whose pair is no wider than
/// `span_max` mil, if any.
pub fn best(options: &[PairOption], next_max: f64, span_max: Option<f64>) -> Option<usize> {
    options
        .iter()
        .position(|o| o.next <= next_max && span_max.is_none_or(|m| o.span <= m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripline() -> LayerGeometry {
        LayerGeometry {
            copper: 2,
            name: "L3".into(),
            t: 0.6,
            above: Some(5.0),
            below: Some(5.0),
            er: 3.6,
            tan_d: Some(0.004),
            rough_um: None,
            slabs_above: vec![[5.0, 3.6, 0.004]],
            slabs_below: vec![[5.0, 3.6, 0.004]],
            mask: None,
        }
    }

    #[test]
    fn options_hit_the_target_and_wider_loses_less_copper() {
        let g = stripline();
        let mut goal = PairGoal::new(100.0);
        goal.w_min = 3.0;
        goal.w_max = 7.0;
        let o = pair_options(&g, &goal);
        assert!(o.len() >= 5, "{} options", o.len());
        for p in &o {
            assert!((p.zdiff - 100.0).abs() < 3.0, "{p:?}");
            assert!(p.s >= goal.s_min);
        }
        let narrow = o.iter().min_by(|a, b| a.w.total_cmp(&b.w)).unwrap();
        let wide = o.iter().max_by(|a, b| a.w.total_cmp(&b.w)).unwrap();
        assert!(wide.loss_cu < narrow.loss_cu);
        // Wider tracks need a wider gap for the same Zdiff.
        assert!(wide.s > narrow.s);
        assert!(o.windows(2).all(|p| p[0].loss <= p[1].loss));
    }
}
