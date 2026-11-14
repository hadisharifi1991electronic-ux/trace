//! Via optimiser: tunes a via's (or a differential pair's) antipad, return
//! vias and pitch with the 3D solver, for the best return loss up to a
//! chosen frequency.
//!
//! It is a coordinate search: each setting in turn is swept with the
//! others held, keeping the best, on a coarse quick solve; the winner is
//! then solved again in full.

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
    /// Values per setting.
    pub steps: usize,
}

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
            steps: 4,
        }
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
        "{:.1}|{}|{:.1}|{}|{:.1}",
        v.antipad,
        v.antipad_oval,
        v.pair.unwrap_or(0.0),
        v.returns,
        v.return_dist
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
    // Each setting: its values and how to apply one.
    type Apply = fn(&mut ViaSetup, f64);
    let mut axes: Vec<(&str, Vec<f64>, Apply)> = vec![(
        "antipad",
        span(goal.antipad.0, goal.antipad.1, goal.steps + 1),
        |v, x| v.antipad = x,
    )];
    if pair && goal.try_oval {
        axes.push(("oval", vec![0.0, 1.0], |v, x| v.antipad_oval = x > 0.5));
    }
    if pair && goal.pitch.1 - goal.pitch.0 > 0.5 {
        axes.push((
            "pitch",
            span(goal.pitch.0, goal.pitch.1, goal.steps),
            |v, x| v.pair = Some(x),
        ));
    }
    if !goal.returns.is_empty() {
        axes.push((
            "returns",
            goal.returns.iter().map(|&n| n as f64).collect(),
            |v, x| v.returns = x as usize,
        ));
    }
    axes.push((
        "return distance",
        span(goal.return_dist.0, goal.return_dist.1, goal.steps - 1),
        |v, x| v.return_dist = x,
    ));
    // The antipad again, finer, once the rest has moved.
    let total_guess: usize = axes.iter().map(|a| a.1.len()).sum::<usize>() + goal.steps + 2;
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
        let tg = total_guess as f64;
        progress(n0 / tg, what);
        let r = fdtd::solve_with(stackup, &quick(&v), &|p| {
            progress((n0 + p * 0.95) / tg, what)
        });
        done.set(done.get() + 1);
        match r {
            Ok(r) => {
                let (worst_rl, il) = judge(&r, f);
                trials.push(ViaTrial {
                    setup: v,
                    worst_rl,
                    il,
                    error: None,
                });
            }
            // A setup that cannot be built (return vias on the traces,
            // say) is listed but never wins.
            Err(e) => trials.push(ViaTrial {
                setup: v,
                worst_rl: f64::INFINITY,
                il: f64::NAN,
                error: Some(e),
            }),
        }
        seen.insert(k, trials.len() - 1);
        Ok(trials.len() - 1)
    };
    let mut best = run(base.clone(), "the starting setup", &mut trials)?;
    for (name, values, apply) in axes.iter() {
        for &x in values {
            let mut v = trials[best].setup.clone();
            apply(&mut v, x);
            let i = run(v, &format!("{name} {x}"), &mut trials)?;
            if trials[i].worst_rl < trials[best].worst_rl - 0.05 {
                best = i;
            }
        }
    }
    // Refine the antipad about the best.
    let a = trials[best].setup.antipad;
    let step = ((goal.antipad.1 - goal.antipad.0) / goal.steps.max(1) as f64 / 2.0).max(1.0);
    for x in [a - step, a + step] {
        if x >= goal.antipad.0 - 1e-9 && x <= goal.antipad.1 + 1e-9 {
            let mut v = trials[best].setup.clone();
            v.antipad = x;
            let i = run(v, &format!("antipad {x}"), &mut trials)?;
            if trials[i].worst_rl < trials[best].worst_rl - 0.05 {
                best = i;
            }
        }
    }
    if let Some(e) = &trials[best].error {
        return Err(format!("no setup could be solved: {e}"));
    }
    let best_setup = trials[best].setup.clone();
    progress(0.96, "the best setup in full");
    let start = fdtd::solve(stackup, base)?;
    let full = fdtd::solve(stackup, &best_setup)?;
    progress(1.0, "done");
    Ok(ViaOpt {
        trials,
        start,
        best_setup,
        best: full,
    })
}

/// A one-line description of a setup.
pub fn describe(v: &ViaSetup) -> String {
    let mut s = format!(
        "antipad {:.1} mil{}",
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
}
