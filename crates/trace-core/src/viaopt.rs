//! Via optimiser: tunes a via's (or a differential pair's) antipad, return
//! vias and pitch with the 3D solver, for the best return loss up to a
//! chosen frequency.
//!
//! It is a coordinate search: each setting in turn is swept with the
//! others held, keeping the best, on a coarse quick solve; a parabola
//! through the best value and its neighbours then tries the point between
//! them, and the passes repeat while they still gain. The winner is then
//! solved again in full, with the next best few, on a grid 0.7 times as
//! fine, and the best there wins. Return loss below the solver's accuracy (about
//! −45 dB) counts as equal, so the search stops chasing noise there and
//! keeps the simpler setup.

use crate::fdtd::{self, ViaResult, ViaSetup};
use crate::Stackup;

/// What to tune, and within what limits (mil).
#[derive(Clone, Debug)]
pub struct ViaGoal {
    /// Judge the return loss from DC up to this frequency, GHz.
    pub f_ghz: f64,
    /// Antipad diameters to try (from, to).
    pub antipad: (f64, f64),
    /// Pair pitch (from, to); ignored for a single via. Equal ends keep
    /// the pitch (a part's pin pitch usually sets it).
    pub pitch: (f64, f64),
    /// Return via distance (from, to).
    pub return_dist: (f64, f64),
    /// Return via counts to try.
    pub returns: Vec<usize>,
    /// Try one oval antipad around a pair.
    pub try_oval: bool,
    /// Copper to copper clearance the layout must keep (from a pad to the
    /// plane around it, between a pair's pads, and from a return via's pad
    /// to the antipad).
    pub clearance: f64,
    /// Pad diameters to try (from, to); equal ends keep the pad.
    pub pad: (f64, f64),
    /// Drill diameters to try (from, to); equal ends keep the drill.
    pub drill: (f64, f64),
    /// Smallest annular ring (pad over drill, each side), mil.
    pub ring: f64,
    /// Weight of the through loss in the score, per dB.
    pub il_weight: f64,
    /// Try backdrilling the stub away.
    pub try_backdrill: bool,
    /// Passes over all the settings, at most.
    pub passes: usize,
    /// Front runners to solve again in full on a finer grid.
    pub confirm: usize,
    /// Values per setting.
    pub steps: usize,
}

/// Return loss better than this is under the 3D solver's accuracy, dB.
pub const FLOOR_DB: f64 = -45.0;

impl ViaGoal {
    /// Sensible limits around a via.
    pub fn around(v: &ViaSetup, f_ghz: f64) -> ViaGoal {
        let pad = v.pad.max(v.drill);
        let pitch = v.pair.unwrap_or(2.0 * pad);
        // 4 mil of copper clearance, or the setup's own if it is tighter
        // (the starting setup is then always one the search may keep).
        let have = ((v.antipad - pad) / 2.0)
            .min(v.pair.map_or(f64::INFINITY, |p| p - pad))
            .min(v.return_dist - v.antipad / 2.0 - pad / 2.0);
        let clearance = have.clamp(0.5, 4.0);
        ViaGoal {
            f_ghz,
            antipad: (pad + 2.0 * clearance, pad + 3.0 * pad.max(8.0)),
            pitch: (pitch, pitch),
            return_dist: (0.0, v.return_dist.max(3.0 * pad)),
            returns: if v.pair.is_some() {
                vec![2, 4, 6]
            } else {
                vec![2, 4]
            },
            try_oval: v.pair.is_some(),
            clearance,
            // Down to a 4 mil annular ring (or the pad's own, if smaller).
            pad: (
                (v.drill + 2.0 * ((pad - v.drill) / 2.0).min(4.0)).min(pad),
                pad,
            ),
            drill: (v.drill, v.drill),
            ring: ((pad - v.drill) / 2.0).min(4.0),
            il_weight: 0.0,
            try_backdrill: false,
            passes: 3,
            confirm: 3,
            steps: 4,
        }
    }
}

/// What the board house can make, mil.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FabLimits {
    /// Finished drill (smallest, largest).
    pub drill: (f64, f64),
    /// Smallest annular ring, each side of the drill.
    pub ring: f64,
    /// Antipad diameter (smallest, largest).
    pub antipad: (f64, f64),
    /// Smallest copper to copper gap (pad to plane, pad to pad).
    pub clearance: f64,
}

impl Default for FabLimits {
    /// A common standard process: 8 mil drills and up, 4 mil rings and
    /// gaps.
    fn default() -> FabLimits {
        FabLimits {
            drill: (8.0, 20.0),
            ring: 4.0,
            antipad: (20.0, 80.0),
            clearance: 4.0,
        }
    }
}

impl ViaGoal {
    /// Everything the limits allow, judged up to `f_ghz`, with the through
    /// loss counted too.
    pub fn within(v: &ViaSetup, f_ghz: f64, fab: &FabLimits) -> ViaGoal {
        let mut g = ViaGoal::around(v, f_ghz);
        g.drill = fab.drill;
        g.ring = fab.ring;
        g.clearance = fab.clearance;
        let pmin = fab.drill.0 + 2.0 * fab.ring;
        g.pad = (pmin, (fab.drill.1 + 2.0 * fab.ring).max(pmin));
        g.antipad = (
            fab.antipad.0.max(pmin + 2.0 * fab.clearance),
            fab.antipad.1.max(fab.antipad.0),
        );
        g.return_dist = (0.0, g.return_dist.1.max(fab.antipad.1 / 2.0 + pmin));
        g.il_weight = 4.0;
        g
    }
}

/// One setup tried.
#[derive(Clone, Debug)]
pub struct ViaTrial {
    pub setup: ViaSetup,
    /// Worst return loss up to the goal frequency, dB (negative; lower is
    /// better).
    pub worst_rl: f64,
    /// Insertion loss at the goal frequency, dB.
    pub il: f64,
    /// What the search minimises: the worst return loss, plus a tenth of
    /// the mean, each no better than the solver's accuracy.
    pub score: f64,
    /// The smallest cell of its (quick) solve, mil.
    pub cell: f64,
    /// Why the setup could not be built or solved, if so (not scored).
    pub error: Option<String>,
}

/// The search's outcome.
#[derive(Clone, Debug)]
pub struct ViaOpt {
    /// Every setup tried (quick solves), in the order tried.
    pub trials: Vec<ViaTrial>,
    /// The starting setup and the best one, each solved in full.
    pub start: ViaResult,
    pub best_setup: ViaSetup,
    pub best: ViaResult,
    /// The front runners solved again in full on the finer grid, with
    /// their worst return loss there, dB (the best is among them).
    pub confirmed: Vec<(ViaSetup, f64)>,
}

/// Worst |S11| (dB) up to `f`, and |S21| (dB) at `f`.
pub fn judge(r: &ViaResult, f: f64) -> (f64, f64) {
    let mut worst = f64::MIN;
    let mut il = 0.0;
    for k in 0..r.f.len() {
        if r.f[k] <= f * 1.0001 {
            worst = worst.max(r.s11[k].db());
            il = r.s21[k].db();
        }
    }
    (worst, il)
}

/// The search's measure of a result (lower is better).
pub fn score(r: &ViaResult, f: f64) -> f64 {
    score_with(r, f, 0.0)
}

/// The same, plus `il_weight` times the mean through loss (dB) in band.
pub fn score_with(r: &ViaResult, f: f64, il_weight: f64) -> f64 {
    let il: Vec<f64> =
        r.f.iter()
            .zip(&r.s21)
            .filter(|(x, _)| **x <= f * 1.0001)
            .map(|(_, s)| -s.db().min(0.0))
            .collect();
    let il = il.iter().sum::<f64>() / il.len().max(1) as f64;
    rl_score(r, f) + il_weight * il
}

fn rl_score(r: &ViaResult, f: f64) -> f64 {
    let (worst, _) = judge(r, f);
    let band: Vec<f64> =
        r.f.iter()
            .zip(&r.s11)
            .filter(|(x, _)| **x <= f * 1.0001)
            .map(|(_, s)| s.db().max(FLOOR_DB))
            .collect();
    let mean = band.iter().sum::<f64>() / band.len().max(1) as f64;
    worst.max(FLOOR_DB) + 0.1 * mean
}

fn span(a: f64, b: f64, n: usize) -> Vec<f64> {
    let (a, b) = (a.min(b), a.max(b));
    if n < 2 || b - a < 0.5 {
        return vec![a];
    }
    (0..n)
        .map(|i| ((a + (b - a) * i as f64 / (n - 1) as f64) * 2.0).round() / 2.0)
        .collect()
}

/// The setup made legal: return vias clear of the antipad, a pair's pads
/// apart.
fn legal(mut v: ViaSetup, g: &ViaGoal) -> ViaSetup {
    if g.drill.1 > g.drill.0 {
        v.drill = v.drill.clamp(g.drill.0, g.drill.1);
    }
    v.pad = v
        .pad
        .max(g.pad.0.min(g.pad.1))
        .max(v.drill + 2.0 * g.ring)
        .max(v.drill);
    if g.antipad.1 > g.antipad.0 {
        v.antipad = v.antipad.min(g.antipad.1);
    }
    let pad = v.pad.max(v.drill);
    v.antipad = v.antipad.max(pad + 2.0 * g.clearance);
    if let Some(p) = v.pair {
        v.pair = Some(p.max(pad + g.clearance));
    }
    v.return_dist = v.return_dist.max(v.antipad / 2.0 + pad / 2.0 + g.clearance);
    v
}

fn key(v: &ViaSetup) -> String {
    format!(
        "{:.1}|{:.1}|{}|{:.1}|{}|{:.1}|{:.1}|{}",
        v.drill,
        v.antipad,
        v.antipad_oval,
        v.pair.unwrap_or(0.0),
        v.returns,
        v.return_dist,
        v.pad,
        v.backdrill
    )
}

/// Searches for the best setup near `base`. `progress` gets the share
/// done (0 to 1) and what is being tried.
pub fn optimise(
    stackup: &Stackup,
    base: &ViaSetup,
    goal: &ViaGoal,
    progress: &dyn Fn(f64, &str),
) -> Result<ViaOpt, String> {
    let f = goal.f_ghz.max(0.5);
    // Quick solves: up to a little past the goal, few points.
    let quick = |v: &ViaSetup| {
        let mut q = v.clone();
        q.fmax_ghz = f * 1.2;
        q.points = 48;
        q
    };
    let pair = base.pair.is_some();
    // Each setting: its values, how to apply one, and whether values
    // between them mean anything (for the parabola step).
    type Apply = fn(&mut ViaSetup, f64);
    let mut axes: Vec<(&str, Vec<f64>, Apply, bool)> = vec![(
        "antipad",
        span(goal.antipad.0, goal.antipad.1, goal.steps + 1),
        |v, x| v.antipad = x,
        true,
    )];
    if goal.drill.1 - goal.drill.0 > 0.5 {
        axes.push((
            "drill",
            span(goal.drill.0, goal.drill.1, goal.steps.min(3)),
            |v, x| {
                let ring = (v.pad - v.drill) / 2.0;
                v.drill = x;
                v.pad = x + 2.0 * ring;
            },
            true,
        ));
    }
    if goal.pad.1 - goal.pad.0 > 0.5 {
        axes.push((
            "pad",
            span(goal.pad.0, goal.pad.1, goal.steps.min(3)),
            |v, x| v.pad = x,
            true,
        ));
    }
    if pair && goal.try_oval {
        axes.push((
            "oval",
            vec![0.0, 1.0],
            |v, x| v.antipad_oval = x > 0.5,
            false,
        ));
    }
    if pair && goal.pitch.1 - goal.pitch.0 > 0.5 {
        axes.push((
            "pitch",
            span(goal.pitch.0, goal.pitch.1, goal.steps),
            |v, x| v.pair = Some(x),
            true,
        ));
    }
    if !goal.returns.is_empty() {
        axes.push((
            "returns",
            goal.returns.iter().map(|&n| n as f64).collect(),
            |v, x| v.returns = x as usize,
            false,
        ));
    }
    axes.push((
        "return distance",
        span(goal.return_dist.0, goal.return_dist.1, goal.steps - 1),
        |v, x| v.return_dist = x,
        true,
    ));
    if goal.try_backdrill && !base.backdrill {
        axes.push((
            "backdrill",
            vec![0.0, 1.0],
            |v, x| v.backdrill = x > 0.5,
            false,
        ));
    }
    let per_pass: usize = axes.iter().map(|a| a.1.len() + 1).sum();
    let total_guess = 1 + per_pass * goal.passes.clamp(1, 2) + 2;
    let mut trials: Vec<ViaTrial> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    let done = std::cell::Cell::new(0usize);
    let mut run = |v: ViaSetup, what: &str, trials: &mut Vec<ViaTrial>| -> Result<usize, String> {
        let v = legal(v, goal);
        let k = key(&v);
        if let Some(&i) = seen.get(&k) {
            return Ok(i);
        }
        let n0 = done.get() as f64;
        let tg = (total_guess as f64).max(n0 + 1.0);
        progress((n0 / tg).min(0.95), what);
        let r = fdtd::solve_with(stackup, &quick(&v), &|p| {
            progress(((n0 + p * 0.95) / tg).min(0.95), what)
        });
        done.set(done.get() + 1);
        match r {
            Ok(r) => {
                let (worst_rl, il) = judge(&r, f);
                trials.push(ViaTrial {
                    setup: v,
                    worst_rl,
                    il,
                    score: score_with(&r, f, goal.il_weight),
                    cell: r.cell,
                    error: None,
                });
            }
            // A setup that cannot be built (return vias on the traces,
            // say) is listed but never wins.
            Err(e) => trials.push(ViaTrial {
                setup: v,
                worst_rl: f64::INFINITY,
                il: f64::NAN,
                score: f64::INFINITY,
                cell: 0.0,
                error: Some(e),
            }),
        }
        seen.insert(k, trials.len() - 1);
        Ok(trials.len() - 1)
    };
    // Better by a clear margin (equal within it keeps the earlier, which
    // is the simpler or the one already chosen).
    let gain = 0.05;
    let mut best = run(base.clone(), "the starting setup", &mut trials)?;
    for pass in 0..goal.passes.max(1) {
        let before = trials[best].score;
        for (name, values, apply, smooth) in axes.iter() {
            let mut tried: Vec<(f64, f64)> = Vec::new();
            for &x in values {
                let mut v = trials[best].setup.clone();
                apply(&mut v, x);
                let i = run(v, &format!("{name} {x}"), &mut trials)?;
                tried.push((x, trials[i].score));
                if trials[i].score < trials[best].score - gain {
                    best = i;
                }
            }
            // The parabola through the best value and its neighbours.
            if *smooth && tried.len() >= 3 {
                tried.sort_by(|a, b| a.0.total_cmp(&b.0));
                let m = (0..tried.len())
                    .min_by(|&a, &b| tried[a].1.total_cmp(&tried[b].1))
                    .unwrap_or(0);
                if m > 0 && m + 1 < tried.len() {
                    let ((x0, y0), (x1, y1), (x2, y2)) = (tried[m - 1], tried[m], tried[m + 1]);
                    let den = (x1 - x0) * (y1 - y2) - (x1 - x2) * (y1 - y0);
                    if den.abs() > 1e-12 && y0.is_finite() && y2.is_finite() {
                        let num = (x1 - x0).powi(2) * (y1 - y2) - (x1 - x2).powi(2) * (y1 - y0);
                        let x = ((x1 - 0.5 * num / den).clamp(x0, x2) * 2.0).round() / 2.0;
                        if (x - x1).abs() >= 0.5 {
                            let mut v = trials[best].setup.clone();
                            apply(&mut v, x);
                            let i = run(v, &format!("{name} {x} (between)"), &mut trials)?;
                            if trials[i].score < trials[best].score - gain {
                                best = i;
                            }
                        }
                    }
                }
            }
        }
        if pass > 0 && trials[best].score > before - gain {
            break;
        }
        if trials[best].score >= before - gain && pass == 0 && best == 0 {
            break;
        }
    }
    if let Some(e) = &trials[best].error {
        return Err(format!("no setup could be solved: {e}"));
    }
    // The quick solves differ by a few dB at very good return loss, so
    // the front runners are solved again in full on a grid 0.7 times as
    // fine (unless a cell was given), and the best there wins.
    let fine_cell = if base.cell > 0.0 {
        base.cell
    } else {
        trials[best].cell * 0.7
    };
    let fine = |v: &ViaSetup| {
        let mut v = v.clone();
        v.cell = fine_cell;
        v
    };
    let mut order: Vec<usize> = (0..trials.len())
        .filter(|&i| trials[i].error.is_none())
        .collect();
    order.sort_by(|&a, &b| trials[a].score.total_cmp(&trials[b].score));
    let mut front: Vec<usize> = vec![best];
    for i in order {
        if front.len() >= goal.confirm.max(1) {
            break;
        }
        // Only those near the best: within 3 dB, as the grid can tell.
        if !front.contains(&i) && trials[i].score < trials[best].score + 3.0 {
            front.push(i);
        }
    }
    let n = front.len() as f64 + 1.0;
    progress(0.86, "the starting setup in full");
    let start = fdtd::solve_with(stackup, &fine(base), &|p| {
        progress(0.86 + 0.14 * p / n, "the starting setup in full")
    })?;
    let mut confirmed = Vec::new();
    let mut winner: Option<(f64, ViaSetup, ViaResult)> = None;
    for (k, &i) in front.iter().enumerate() {
        let v = trials[i].setup.clone();
        let what = format!("front runner {} of {} in full", k + 1, front.len());
        let r = fdtd::solve_with(stackup, &fine(&v), &|p| {
            progress(0.86 + 0.14 * (k as f64 + 1.0 + p) / n, &what)
        })?;
        let sc = score_with(&r, f, goal.il_weight);
        confirmed.push((v.clone(), judge(&r, f).0));
        if winner.as_ref().is_none_or(|w| sc < w.0 - 0.05) {
            winner = Some((sc, v, r));
        }
    }
    let (_, best_setup, full) = winner.ok_or("nothing to confirm")?;
    progress(1.0, "done");
    Ok(ViaOpt {
        trials,
        start,
        best_setup,
        best: full,
        confirmed,
    })
}

/// A via sized to a target impedance.
#[derive(Clone, Debug)]
pub struct Sized {
    pub setup: ViaSetup,
    pub result: ViaResult,
    /// Each solve on the way: the setup and the impedance it gave.
    pub steps: Vec<(ViaSetup, Option<f64>)>,
    /// Within 2 % (or 1 Ω) of the target.
    pub reached: bool,
    pub note: Option<String>,
}

/// Sizes the antipad (and, when the antipad reaches a limit, the drill)
/// so that the via's own impedance, from the 3D solve, meets `target` Ω
/// (differential for a pair), within the goal's limits.
pub fn size_to(
    stackup: &Stackup,
    base: &ViaSetup,
    target: f64,
    goal: &ViaGoal,
    progress: &dyn Fn(f64, &str),
) -> Result<Sized, String> {
    // Coaxial start: Z = 60/√εr · ln(D/d), D the antipad, d the drill.
    let er = {
        let (mut t, mut w) = (0.0, 0.0);
        for l in stackup
            .layers
            .iter()
            .filter(|l| l.kind == crate::LayerKind::Dielectric)
        {
            let h = l.thickness.mil();
            t += h * l.dielectric_constant.unwrap_or(4.0);
            w += h;
        }
        if w > 0.0 {
            t / w
        } else {
            4.0
        }
    };
    let single = if base.pair.is_some() {
        target / 2.0
    } else {
        target
    };
    let mut v = legal(base.clone(), goal);
    v.antipad = v.drill * (single * er.sqrt() / 60.0).exp();
    v = legal(v, goal);
    let tries = 6;
    let mut steps: Vec<(ViaSetup, Option<f64>)> = Vec::new();
    let mut last: Option<(f64, f64)> = None; // (ln antipad, z)
    let mut best: Option<(f64, ViaSetup, ViaResult)> = None;
    for k in 0..tries {
        let what = format!("antipad {:.1} mil, drill {:.1} mil", v.antipad, v.drill);
        let r = fdtd::solve_with(stackup, &v, &|p| {
            progress((k as f64 + p) / tries as f64, &what)
        })?;
        let z = r.z_via;
        steps.push((v.clone(), z));
        let Some(z) = z else {
            return Err("the via is too short to tell its impedance; widen the band".into());
        };
        let miss = (z - target).abs();
        if best.as_ref().is_none_or(|b| miss < b.0) {
            best = Some((miss, v.clone(), r));
        }
        if miss <= (0.02 * target).max(1.0) {
            break;
        }
        // Next antipad: the secant on ln(antipad) once there are two
        // points, else the coaxial slope.
        let x = v.antipad.ln();
        let slope = match last {
            Some((x0, z0)) if (x - x0).abs() > 1e-6 && (z - z0).abs() > 1e-6 => (z - z0) / (x - x0),
            _ => 60.0 / er.sqrt() * if base.pair.is_some() { 2.0 } else { 1.0 },
        };
        last = Some((x, z));
        let mut n = v.clone();
        n.antipad = (x + (target - z) / slope.max(1.0)).exp();
        let n = legal(n, goal);
        if (n.antipad - v.antipad).abs() < 0.25 {
            // The antipad is at a limit: move the drill instead (a thinner
            // drill raises the impedance).
            let mut d = v.clone();
            let ring = (d.pad - d.drill) / 2.0;
            let step = if z < target { -2.0 } else { 2.0 };
            d.drill = (d.drill + step).clamp(goal.drill.0, goal.drill.1.max(goal.drill.0));
            d.pad = d.drill + 2.0 * ring;
            let d = legal(d, goal);
            if (d.drill - v.drill).abs() < 0.25 {
                break;
            }
            v = d;
            last = None;
        } else {
            v = n;
        }
    }
    let (miss, setup, result) = best.ok_or("nothing solved")?;
    let reached = miss <= (0.02 * target).max(1.0);
    let note = (!reached).then(|| {
        format!(
            "the limits allow {:.1} Ω at best ({:.1} Ω off the {:.0} Ω target)",
            result.z_via.unwrap_or(0.0),
            miss,
            target
        )
    });
    progress(1.0, "done");
    Ok(Sized {
        setup,
        result,
        steps,
        reached,
        note,
    })
}

/// A one-line description of a setup.
pub fn describe(v: &ViaSetup) -> String {
    let mut s = format!(
        "drill {:.1}, pad {:.1}, antipad {:.1} mil{}",
        v.drill,
        v.pad,
        v.antipad,
        if v.antipad_oval && v.pair.is_some() {
            " (oval)"
        } else {
            ""
        }
    );
    if let Some(p) = v.pair {
        s += &format!(", pitch {p:.1} mil");
    }
    s += &format!(
        ", {} return via(s) {:.1} mil from {}",
        v.returns,
        v.return_dist,
        if v.pair.is_some() {
            "each via"
        } else {
            "the via"
        }
    );
    if v.backdrill {
        s += ", backdrilled";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerKind, Length, StackLayer};

    fn board() -> Stackup {
        let mut s = Stackup::default();
        for (i, k) in "SGGS".chars().enumerate() {
            if i > 0 {
                s.layers.push(StackLayer {
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
            s.layers.push(StackLayer {
                name: if k == 'G' {
                    format!("GND{i}")
                } else {
                    format!("SIG{i}")
                },
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(0.0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        s
    }

    #[test]
    fn search_never_ends_worse_than_it_starts() {
        let mut v = ViaSetup::new(0, 3);
        v.antipad = 20.0;
        v.cell = 2.0;
        v.fmax_ghz = 15.0;
        v.points = 30;
        let mut g = ViaGoal::around(&v, 12.0);
        g.antipad = (20.0, 40.0);
        g.returns = vec![];
        g.steps = 2;
        let o = optimise(&board(), &v, &g, &|_, _| {}).unwrap();
        assert!(o.trials.len() >= 3);
        let (s, _) = judge(&o.start, 12.0);
        let (b, _) = judge(&o.best, 12.0);
        assert!(b <= s + 0.5, "start {s:.1} dB, best {b:.1} dB");
        // Every setup it tried keeps the return vias clear.
        for t in &o.trials {
            assert!(t.setup.return_dist >= t.setup.antipad / 2.0 + 9.0 - 1e-9);
        }
    }

    #[test]
    fn sizing_gets_closer_to_the_target_and_keeps_the_limits() {
        let mut v = ViaSetup::new(0, 3);
        v.antipad = 20.0;
        v.cell = 2.0;
        v.fmax_ghz = 15.0;
        v.points = 30;
        let fab = FabLimits {
            drill: (8.0, 14.0),
            ring: 4.0,
            antipad: (20.0, 60.0),
            clearance: 4.0,
        };
        let g = ViaGoal::within(&v, 12.0, &fab);
        let s = size_to(&board(), &v, 50.0, &g, &|_, _| {}).unwrap();
        let first = s.steps[0].1.unwrap();
        let got = s.result.z_via.unwrap();
        assert!(
            (got - 50.0).abs() <= (first - 50.0).abs() + 1e-9,
            "first {first:.1} ohm, sized {got:.1} ohm"
        );
        for (st, _) in &s.steps {
            assert!(st.antipad >= 20.0 - 1e-9 && st.antipad <= 60.0 + 1e-9);
            assert!(st.drill >= 8.0 - 1e-9 && st.drill <= 14.0 + 1e-9);
            assert!(st.pad >= st.drill + 8.0 - 1e-9);
        }
    }
}
