//! Coupled electrical and thermal analysis: the heat that a supply's
//! current gives off in the copper (its IR drop) warms the board, and the
//! warmer copper conducts less, so it drops more and heats more.
//!
//! Each rail's DC drop is solved on its own grid (`pi`), every grid
//! node's I²R loss is put into the copper slab of its layer in the 3D
//! thermal grid (`thermal`) along with the parts' own power, and the
//! board's temperature then sets each copper link's resistivity
//! (ρ(T) = ρ20 · (1 + 0.00393 · (T − 20 °C))) for the next DC solve. The
//! passes repeat until the hottest point moves less than `tol_k`.

use crate::{pi, thermal, Board};

#[derive(Clone, Debug)]
pub struct Setup {
    /// The supplies whose copper loss heats the board.
    pub rails: Vec<pi::Setup>,
    /// The thermal problem (parts, air, grid); its `joule` is filled in.
    pub thermal: thermal::Setup,
    /// Feed the temperature back into the copper's resistivity (false:
    /// one pass, copper at 20 °C).
    pub couple: bool,
    pub max_passes: usize,
    /// Stop when the hottest board point moves less than this, K.
    pub tol_k: f64,
}

impl Setup {
    pub fn new(rails: Vec<pi::Setup>, thermal: thermal::Setup) -> Setup {
        Setup {
            rails,
            thermal,
            couple: true,
            max_passes: 12,
            tol_k: 0.05,
        }
    }
}

/// One pass of the loop.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    /// Copper loss of all rails, W.
    pub joule_w: f64,
    /// Hottest board point, °C.
    pub board_max_c: f64,
    /// Largest drop of any rail, V.
    pub worst_drop: f64,
}

#[derive(Clone, Debug)]
pub struct Result {
    /// The rails' nets, in the order of `cold` and `rails`.
    pub nets: Vec<u32>,
    /// The rails with the copper at 20 °C (the plain IR drop).
    pub cold: Vec<pi::Result>,
    /// The rails at the board's final temperature.
    pub rails: Vec<pi::Result>,
    /// The temperatures with the copper loss as a heat source.
    pub thermal: thermal::Result,
    pub passes: Vec<Pass>,
    pub converged: bool,
}

/// The board's temperature as a resistivity map for `pi`.
pub fn temp_map(t: &thermal::Result) -> pi::TempMap {
    pi::TempMap {
        origin: t.origin,
        cell: t.cell,
        cols: t.cols,
        rows: t.rows,
        layers: t.maps.iter().map(|m| m.t.clone()).collect(),
    }
}

/// The rails' copper loss as heat sources (mil to mm).
pub fn heat_of(rails: &[pi::Result]) -> Vec<thermal::Heat> {
    rails
        .iter()
        .flat_map(|r| r.heat.iter())
        .map(|&(l, p, w)| thermal::Heat {
            layer: l as usize,
            at: [p[0] * 0.0254, p[1] * 0.0254],
            watts: w,
        })
        .collect()
}

/// A rail drawing `amps` in all, shared among the net's other parts by
/// their pad count (as `trace ir` does); the source is guessed unless
/// named, the volts read from the net's name unless given (else 1 V).
pub fn rail(
    board: &Board,
    net: &str,
    amps: f64,
    source: Option<&str>,
    volts: Option<f64>,
) -> std::result::Result<pi::Setup, String> {
    let net = net.trim();
    let id = board
        .nets
        .iter()
        .position(|n| n.name == net || n.name.eq_ignore_ascii_case(net))
        .ok_or(format!("no net {net}"))? as u32;
    let source = match source.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => board
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?,
        None => pi::guess_source(board, id)
            .ok_or(format!("{net}: no likely source on the net, name one"))?,
    };
    let volts = volts
        .filter(|v| *v != 0.0)
        .or_else(|| crate::pdn::volts_in_name(&board.nets[id as usize].name))
        .unwrap_or(1.0);
    let parts = pi::default_loads(board, id, source);
    let pads: usize = parts.iter().map(|p| p.1).sum();
    if pads == 0 {
        return Err(format!("{net}: no other parts on the net to load it"));
    }
    let mut s = pi::Setup::new(id, source, volts);
    s.loads = parts
        .iter()
        .map(|p| (p.0, amps * p.1 as f64 / pads as f64))
        .collect();
    Ok(s)
}

fn worst(rails: &[pi::Result]) -> f64 {
    rails.iter().map(|r| r.max_drop).fold(0.0, f64::max)
}

pub fn solve(board: &Board, setup: &Setup) -> std::result::Result<Result, String> {
    if setup.rails.is_empty() {
        return Err("no rail: give at least one supply net".into());
    }
    let solve_rails = |temp: Option<pi::TempMap>| -> std::result::Result<Vec<pi::Result>, String> {
        setup
            .rails
            .iter()
            .map(|r| {
                let mut s = r.clone();
                s.temp = temp.clone();
                pi::solve(board, &s).map_err(|e| {
                    let name = board
                        .nets
                        .get(r.net as usize)
                        .map_or("?", |n| n.name.as_str());
                    format!("{name}: {e}")
                })
            })
            .collect()
    };
    let cold = solve_rails(None)?;
    let mut rails = cold.clone();
    let mut passes = Vec::new();
    let mut converged = !setup.couple;
    let mut th;
    loop {
        crate::cancel::check()?;
        let mut ts = setup.thermal.clone();
        ts.joule.extend(heat_of(&rails));
        th = thermal::solve(board, &ts)?;
        let pass = Pass {
            joule_w: rails.iter().map(|r| r.loss_w).sum(),
            board_max_c: th.board_max.0,
            worst_drop: worst(&rails),
        };
        let moved = passes
            .last()
            .map(|p: &Pass| (p.board_max_c - pass.board_max_c).abs());
        passes.push(pass);
        if !setup.couple {
            break;
        }
        if moved.is_some_and(|d| d < setup.tol_k) {
            converged = true;
            break;
        }
        if passes.len() >= setup.max_passes.max(1) {
            break;
        }
        rails = solve_rails(Some(temp_map(&th)))?;
    }
    Ok(Result {
        nets: setup.rails.iter().map(|r| r.net).collect(),
        cold,
        rails,
        thermal: th,
        passes,
        converged,
    })
}

impl Result {
    /// The rails cold and hot, the loop's passes, then the thermal report.
    pub fn report(&self, board: &Board) -> String {
        let des = |i: usize| {
            board
                .components
                .get(i)
                .map_or("?", |c| c.designator.as_str())
        };
        let mut s = String::new();
        s += &format!(
            "{:<16} {:>7} {:>9} {:>9} {:>10} {:>10} {:>9}\n",
            "Rail", "V", "A", "loss W", "hot loss W", "drop mV", "hot mV"
        );
        for ((c, h), &net) in self.cold.iter().zip(&self.rails).zip(&self.nets) {
            s += &format!(
                "{:<16} {:>7.3} {:>9.3} {:>9.4} {:>10.4} {:>10.2} {:>9.2}\n",
                board
                    .nets
                    .get(net as usize)
                    .map_or("?", |n| n.name.as_str()),
                c.volts,
                c.loads.iter().map(|l| l.amps).sum::<f64>(),
                c.loss_w,
                h.loss_w,
                c.max_drop * 1000.0,
                h.max_drop * 1000.0
            );
            // The loads that sag most once the copper is hot.
            let mut worst: Vec<(usize, f64, f64)> = c
                .loads
                .iter()
                .zip(&h.loads)
                .filter(|(a, b)| a.connected && b.connected)
                .map(|(a, b)| (a.part, a.v_min, b.v_min))
                .collect();
            worst.sort_by(|a, b| a.2.total_cmp(&b.2));
            for (p, vc, vh) in worst.into_iter().take(3) {
                s += &format!(
                    "    {:<12} V min {:.4} -> {:.4} V ({:+.2} mV)\n",
                    des(p),
                    vc,
                    vh,
                    (vh - vc) * 1000.0
                );
            }
        }
        s += "Pass  copper loss W  hottest °C  worst drop mV\n";
        for (k, p) in self.passes.iter().enumerate() {
            s += &format!(
                "{:>4} {:>14.4} {:>11.2} {:>14.2}\n",
                k + 1,
                p.joule_w,
                p.board_max_c,
                p.worst_drop * 1000.0
            );
        }
        if self.passes.len() > 1 {
            s += &format!(
                "{} after {} passes (resistivity +0.393 %/K)\n",
                if self.converged {
                    "Settled"
                } else {
                    "Did not settle"
                },
                self.passes.len()
            );
        }
        s += &self.thermal.report();
        s
    }
}
