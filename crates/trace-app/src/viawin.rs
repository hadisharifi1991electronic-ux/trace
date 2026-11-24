//! The Via S-parameters window: a 3D full-wave (FDTD) solve of a via (or
//! a pair of vias) from one layer to another, with its plot.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::fdtd::{self, ViaResult, ViaSetup};
use trace_core::viaopt::{self, ViaOpt};
use trace_core::{Board, Stackup};

type Solved = Result<ViaResult, String>;
type Optimised = Result<ViaOpt, String>;
type OptRun = (mpsc::Receiver<Optimised>, Arc<Mutex<(f64, String)>>);
type SizeRun = (
    mpsc::Receiver<Result<viaopt::Sized, String>>,
    Arc<Mutex<(f64, String)>>,
);

pub struct ViaWindow {
    pub open: bool,
    setup: ViaSetup,
    auto_width: bool,
    is_pair: bool,
    pitch: f64,
    /// The board the defaults were taken from.
    board_key: (usize, usize),
    running: Option<(mpsc::Receiver<Solved>, Arc<AtomicU32>)>,
    result: Option<(ViaSetup, Result<ViaResult, String>)>,
    /// The optimiser: goal frequency, run, outcome.
    opt_f: f64,
    /// Let the optimiser move a pair's pitch too.
    free_pitch: bool,
    /// The optimiser may shrink the pads, and try backdrilling.
    opt_pad: bool,
    opt_backdrill: bool,
    opt_running: Option<OptRun>,
    opt: Option<Optimised>,
    /// What the board house can make; the target impedance and whether
    /// Solve sizes the via to it.
    fab: viaopt::FabLimits,
    target_z: f64,
    to_target: bool,
    sizing: Option<SizeRun>,
    sized_note: Option<String>,
    /// Er and Df of one's own instead of the stack-up's.
    own_mat: bool,
    er: f64,
    df: f64,
    /// Solve on finer grids until converged, with error bars.
    accurate: bool,
    /// Field maps of the solve.
    fields: crate::fieldview::FieldView,
    /// The same transition in openEMS: run, outcome and its folder.
    oems_running: Option<(mpsc::Receiver<Solved>, Arc<AtomicU32>)>,
    oems: Option<Solved>,
    oems_dir: Option<std::path::PathBuf>,
}

impl Default for ViaWindow {
    fn default() -> Self {
        ViaWindow {
            open: false,
            setup: ViaSetup::new(0, 1),
            auto_width: true,
            is_pair: false,
            pitch: 40.0,
            board_key: (0, 0),
            running: None,
            result: None,
            opt_f: 10.0,
            free_pitch: false,
            opt_pad: true,
            opt_backdrill: false,
            opt_running: None,
            opt: None,
            fab: viaopt::FabLimits::default(),
            target_z: 50.0,
            to_target: true,
            sizing: None,
            sized_note: None,
            own_mat: false,
            er: 4.0,
            df: 0.02,
            accurate: true,
            fields: Default::default(),
            oems_running: None,
            oems: None,
            oems_dir: None,
        }
    }
}

impl ViaWindow {
    /// Opens the window on a via as built (from the impedance check), with
    /// its target.
    /// The last solve's S11 (Sdd11 for a pair) as a Smith chart curve,
    /// with its reference impedance.
    pub fn smith_curve(&self) -> Option<(trace_core::smith::Curve, f64)> {
        let (_, Ok(r)) = self.result.as_ref()? else {
            return None;
        };
        if r.f.is_empty() {
            return None;
        }
        let name = if self.is_pair {
            "Via pair Sdd11"
        } else {
            "Via S11"
        };
        Some((
            trace_core::smith::Curve {
                name: name.into(),
                points: r.f.iter().copied().zip(r.s11.iter().copied()).collect(),
            },
            r.z_ref,
        ))
    }

    pub fn load(&mut self, board: &Board, s: ViaSetup, target: f64) {
        self.board_key = (board as *const Board as usize, board.geometry.vias.len());
        self.is_pair = s.pair.is_some();
        if let Some(p) = s.pair {
            self.pitch = p;
        }
        self.auto_width = s.w_from.is_none() && s.w_to.is_none();
        let mut s = s;
        // The check's quick grid; the window chooses its own.
        s.cell = 0.0;
        s.fmax_ghz = self.setup.fmax_ghz.max(s.fmax_ghz);
        s.points = self.setup.points.max(s.points);
        self.setup = s;
        self.target_z = target;
        self.result = None;
        self.opt = None;
        self.sized_note = None;
        self.open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, board: &Board, stackup: &Stackup) {
        if !self.open {
            return;
        }
        if let Some((rx, _)) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    if let Some(res) = &mut self.result {
                        res.1 = r;
                    }
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        if let Some((rx, _)) = &self.opt_running {
            match rx.try_recv() {
                Ok(r) => {
                    self.opt = Some(r);
                    self.opt_running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.opt_running = None,
            }
        }
        if let Some((rx, _)) = &self.sizing {
            match rx.try_recv() {
                Ok(Ok(sz)) => {
                    self.is_pair = sz.setup.pair.is_some();
                    if let Some(p) = sz.setup.pair {
                        self.pitch = p;
                    }
                    self.sized_note = Some(match &sz.note {
                        Some(n) => format!("Closest: {}; {n}.", viaopt::describe(&sz.setup)),
                        None if sz.accurate.is_empty() => format!(
                            "Sized to {:.1} Ω in {} solves: {}.",
                            sz.result.z_via.unwrap_or(f64::NAN),
                            sz.steps.len(),
                            viaopt::describe(&sz.setup)
                        ),
                        None => format!(
                            "Sized to {:.1} Ω on the converged grid ({} accurate and {} quick solves): {}.",
                            sz.result.z_via.unwrap_or(f64::NAN),
                            sz.accurate.len(),
                            sz.steps.len(),
                            viaopt::describe(&sz.setup)
                        ),
                    });
                    self.setup = sz.setup.clone();
                    self.result = Some((sz.setup, Ok(sz.result)));
                    self.sizing = None;
                }
                Ok(Err(e)) => {
                    self.result = Some((self.setup.clone(), Err(e)));
                    self.sizing = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.sizing = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("Via S-parameters (3D)")
            .open(&mut open)
            .default_width(760.0)
            .show(ctx, |ui| self.body(ui, ctx, board, stackup));
        self.open = open;
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, board: &Board, stackup: &Stackup) {
        let geo = trace_core::impedance::layer_geometry(stackup);
        if geo.len() < 2 {
            ui.label("The stack-up needs two or more copper layers.");
            return;
        }
        let key = (board as *const Board as usize, board.geometry.vias.len());
        if key != self.board_key {
            self.board_key = key;
            let mut s = ViaSetup::new(0, (geo.len() - 1) as u8);
            if let Some((d, p, a)) = fdtd::board_via(board) {
                s.drill = d;
                s.pad = p;
                s.antipad = a;
            }
            self.setup = s;
            self.result = None;
        }
        let n = geo.len();
        let name = |l: u8| {
            geo.get(l as usize)
                .map_or(String::new(), |g| g.name.clone())
        };
        let s = &mut self.setup;
        s.from = s.from.min(n as u8 - 1);
        s.to = s.to.min(n as u8 - 1);
        egui::Grid::new("via-setup")
            .num_columns(4)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("From layer");
                egui::ComboBox::from_id_salt("via-from")
                    .selected_text(name(s.from))
                    .show_ui(ui, |ui| {
                        for l in 0..n as u8 {
                            ui.selectable_value(&mut s.from, l, name(l));
                        }
                    });
                ui.label("To layer");
                egui::ComboBox::from_id_salt("via-to")
                    .selected_text(name(s.to))
                    .show_ui(ui, |ui| {
                        for l in 0..n as u8 {
                            ui.selectable_value(&mut s.to, l, name(l));
                        }
                    });
                ui.end_row();
                let mil = |ui: &mut egui::Ui, v: &mut f64, lo: f64, hi: f64| {
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.25)
                            .range(lo..=hi)
                            .max_decimals(2)
                            .suffix(" mil"),
                    );
                };
                ui.label("Drill");
                mil(ui, &mut s.drill, 2.0, 60.0);
                ui.label("Pad");
                mil(ui, &mut s.pad, s.drill, 120.0);
                ui.end_row();
                ui.label("Antipad")
                    .on_hover_text("Diameter of the clearance hole in the planes");
                mil(ui, &mut s.antipad, s.pad, 200.0);
                ui.label("Return vias");
                ui.horizontal(|ui| {
                    let mut k = s.returns as u32;
                    ui.add(egui::DragValue::new(&mut k).range(0..=6));
                    s.returns = k as usize;
                    ui.label("at");
                    mil(ui, &mut s.return_dist, 5.0, 200.0);
                });
                ui.end_row();
                ui.label("Traces");
                ui.checkbox(&mut self.auto_width, "50 Ω from the field solver");
                if !self.auto_width {
                    let mut w = s.w_from.unwrap_or(5.0);
                    let mut w2 = s.w_to.unwrap_or(5.0);
                    mil(ui, &mut w, 1.0, 100.0);
                    mil(ui, &mut w2, 1.0, 100.0);
                    s.w_from = Some(w);
                    s.w_to = Some(w2);
                } else {
                    s.w_from = None;
                    s.w_to = None;
                }
                ui.end_row();
                ui.label("Pair");
                if ui
                    .checkbox(&mut self.is_pair, "differential, vias at")
                    .changed()
                {
                    self.target_z = match (self.is_pair, self.target_z) {
                        (true, z) if (z - 50.0).abs() < 0.01 => 100.0,
                        (false, z) if (z - 100.0).abs() < 0.01 => 50.0,
                        (_, z) => z,
                    };
                }
                mil(ui, &mut self.pitch, s.pad + 1.0, 300.0);
                s.pair = self.is_pair.then_some(self.pitch);
                ui.add_enabled(
                    self.is_pair,
                    egui::Checkbox::new(&mut s.antipad_oval, "one oval antipad"),
                )
                .on_hover_text("One clearance around both vias instead of a round one each");
                ui.end_row();
                ui.label("Stub");
                ui.checkbox(&mut s.backdrill, "backdrilled (no stub)");
                ui.label("Up to");
                ui.add(
                    egui::DragValue::new(&mut s.fmax_ghz)
                        .speed(0.5)
                        .range(2.0..=100.0)
                        .suffix(" GHz"),
                );
                ui.end_row();
                // Materials: the stack-up's between the two layers, or
                // values of one's own for every dielectric layer.
                let stack = fdtd::materials_between(stackup, s.from, s.to);
                ui.label("Dielectric");
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut self.own_mat, "own")
                        .on_hover_text(
                            "Off: each layer's Er and Df from the stack-up. On: these for every \
                             dielectric layer (traces and via alike)",
                        )
                        .changed()
                        && self.own_mat
                    {
                        if let Some((e, d)) = stack {
                            self.er = (e * 100.0).round() / 100.0;
                            self.df = (d * 10000.0).round() / 10000.0;
                        }
                    }
                    if self.own_mat {
                        ui.label("Er");
                        ui.add(
                            egui::DragValue::new(&mut self.er)
                                .speed(0.01)
                                .range(1.0..=15.0)
                                .max_decimals(2),
                        );
                        ui.label("Df");
                        ui.add(
                            egui::DragValue::new(&mut self.df)
                                .speed(0.0002)
                                .range(0.0..=0.1)
                                .max_decimals(4),
                        );
                    } else if let Some((e, d)) = stack {
                        ui.label(RichText::new(format!("Er {e:.2}, Df {d:.4} (stack-up)")).weak());
                    }
                });
                s.er = self.own_mat.then_some(self.er);
                s.df = self.own_mat.then_some(self.df);
                ui.label("Loss");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.metal_loss, "copper and Df")
                        .on_hover_text(
                            "The barrel's skin-effect copper loss (with roughness) and its \
                             dielectric loss; off: perfect copper, no Df (the grid alone)",
                        );
                    ui.add_enabled(
                        s.metal_loss,
                        egui::DragValue::new(&mut s.rough_um)
                            .speed(0.05)
                            .range(0.0..=10.0)
                            .max_decimals(2)
                            .prefix("rough ")
                            .suffix(" µm"),
                    );
                });
                ui.end_row();
            });
        egui::CollapsingHeader::new("Manufacturing limits and target")
            .id_salt("via-fab")
            .default_open(true)
            .show(ui, |ui| {
                let f = &mut self.fab;
                let mil = |ui: &mut egui::Ui, v: &mut f64| {
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.25)
                            .range(1.0..=200.0)
                            .max_decimals(2)
                            .suffix(" mil"),
                    );
                };
                egui::Grid::new("via-fab-grid")
                    .num_columns(4)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Drill").on_hover_text("Finished hole sizes the board house drills");
                        ui.horizontal(|ui| {
                            mil(ui, &mut f.drill.0);
                            ui.label("to");
                            mil(ui, &mut f.drill.1);
                        });
                        ui.label("Annular ring").on_hover_text("Smallest pad copper each side of the hole");
                        mil(ui, &mut f.ring);
                        ui.end_row();
                        ui.label("Antipad").on_hover_text("Clearance hole in the planes, smallest and largest diameter");
                        ui.horizontal(|ui| {
                            mil(ui, &mut f.antipad.0);
                            ui.label("to");
                            mil(ui, &mut f.antipad.1);
                        });
                        ui.label("Copper gap").on_hover_text("Smallest copper to copper gap (pad to plane, pad to pad)");
                        mil(ui, &mut f.clearance);
                        ui.end_row();
                        f.drill.1 = f.drill.1.max(f.drill.0);
                        f.antipad.1 = f.antipad.1.max(f.antipad.0);
                        ui.label("Target");
                        ui.add(
                            egui::DragValue::new(&mut self.target_z)
                                .speed(0.5)
                                .range(10.0..=200.0)
                                .suffix(" Ω"),
                        )
                        .on_hover_text("The via's own impedance (differential for a pair)");
                        ui.checkbox(&mut self.to_target, "Solve sizes the via to it")
                            .on_hover_text("Solve moves the antipad (then the drill) within the limits until the 3D solve gives the target; off: Solve takes the via as set");
                        ui.end_row();
                    });
            });
        ui.horizontal(|ui| {
            let ok =
                self.setup.from != self.setup.to && self.running.is_none() && self.sizing.is_none();
            let sizing = ok
                && self.to_target
                && ui
                    .add_enabled(ok, egui::Button::new("Solve"))
                    .on_hover_text(
                        "Sizes the via to the target impedance within the limits, with 3D solves",
                    )
                    .clicked();
            if sizing {
                let st = stackup.clone();
                let base = self.setup.clone();
                let mut goal = viaopt::ViaGoal::within(&base, base.fmax_ghz, &self.fab);
                if !self.accurate {
                    goal.accurate = None;
                }
                let target = self.target_z;
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(Mutex::new((0.0, String::new())));
                let p2 = prog.clone();
                let c = ctx.clone();
                let token = crate::jobs::start(ctx, "via-size");
                std::thread::spawn(move || {
                    let _g = token.install();
                    let r = viaopt::size_to(&st, &base, target, &goal, &|f, what| {
                        if let Ok(mut p) = p2.lock() {
                            *p = (f, what.to_string());
                        }
                    });
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.sized_note = None;
                self.sizing = Some((rx, prog));
            }
            if let Some(p) = self.sizing.as_ref().map(|(_, p)| p.clone()) {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "via-size") {
                    self.sizing = None;
                }
                if let Ok(p) = p.lock() {
                    ui.label(format!("{:.0} %: sizing, {}", p.0 * 100.0, p.1));
                }
            }
            ui.checkbox(&mut self.accurate, "Accurate").on_hover_text(
                "Solves on finer and finer grids until the S-parameters stop moving, \
                 extrapolates to a zero cell size and gives every point an error bar. \
                 Sizing and the optimiser then judge by that converged grid too \
                 (tens of minutes; the quick grid can be several ohm and several dB off \
                 on fine vias). Off: one quick grid.",
            );
            if !self.to_target {
                self.fields.setup_row(ui);
            }
            if !self.to_target && ui.add_enabled(ok, egui::Button::new("Solve")).clicked() {
                let st = stackup.clone();
                let mut setup = self.setup.clone();
                setup.fields = self.fields.freqs();
                let accurate = self.accurate;
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(AtomicU32::new(0));
                let p2 = prog.clone();
                let c = ctx.clone();
                let token = crate::jobs::start(ctx, "via");
                std::thread::spawn(move || {
                    let _g = token.install();
                    let progress = |f: f64| {
                        p2.store((f * 1000.0) as u32, Ordering::Relaxed);
                    };
                    let r = if accurate {
                        fdtd::converge(&st, &setup, &fdtd::Tolerance::default(), &progress).map(
                            |c| {
                                let mut best = c.best.clone();
                                for (k, line) in c.verdict().lines().enumerate() {
                                    best.notes.insert(k, line.trim().to_string());
                                }
                                best
                            },
                        )
                    } else {
                        fdtd::solve_with(&st, &setup, &progress)
                    };
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.result = Some((self.setup.clone(), Err(String::new())));
                self.oems = None;
                self.running = Some((rx, prog));
            }
            if let Some(p) = self.running.as_ref().map(|(_, p)| p.clone()) {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "via") {
                    self.running = None;
                    self.result = None;
                }
                // Steps are bounded generously, so most runs end early.
                ui.label(format!(
                    "Solving… ({} % of the step limit)",
                    p.load(Ordering::Relaxed) / 10
                ));
            } else {
                ui.weak("A 3D solve takes from seconds to a few minutes.");
            }
            if let Some((setup, Ok(r))) = &self.result {
                let title = format!("Via {} to {}", name(setup.from), name(setup.to));
                if ui.button("Save Touchstone…").clicked() {
                    let ext = "s2p";
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Touchstone", &[ext])
                        .set_file_name("via.s2p")
                        .save_file()
                    {
                        let _ = std::fs::write(p, r.touchstone(&title));
                    }
                }
                if ui.button("Save plot (SVG)…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("SVG", &["svg"])
                        .set_file_name("via.svg")
                        .save_file()
                    {
                        let _ = std::fs::write(p, r.svg(&title));
                    }
                }
            }
        });
        self.openems_row(ui, ctx, stackup);
        self.optimiser(ui, ctx, stackup);
        let Some((_, res)) = &self.result else {
            return;
        };
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                if !e.is_empty() {
                    ui.colored_label(Color32::from_rgb(230, 110, 90), e);
                }
                return;
            }
        };
        ui.separator();
        let kind = if r.z_ref > 60.0 { "Sdd" } else { "S" };
        let worst = (0..r.f.len())
            .max_by(|&a, &b| r.s11[a].abs().total_cmp(&r.s11[b].abs()))
            .unwrap_or(0);
        let mut line =
            format!(
            "Traces {:.2} / {:.2} mil ({:.1} / {:.1} Ω); worst return loss {:.1} dB at {:.1} GHz",
            r.w[0], r.w[1], r.z0[0], r.z0[1], r.s11[worst].db(), r.f[worst]
        );
        if r.stub > 0.5 {
            line += &format!("; stub {:.1} mil", r.stub);
        }
        if let Some(z) = r.z_via {
            line += &format!("; via {z:.1} Ω");
        }
        ui.label(RichText::new(line).strong());
        if let Some(n) = &self.sized_note {
            ui.label(n);
        }
        if let Some((f, db)) = r.notch() {
            ui.colored_label(
                Color32::from_rgb(230, 180, 90),
                format!("Stub resonance: {kind}21 dips to {db:.1} dB at {f:.2} GHz. Backdrilling removes it."),
            );
        }
        ui.weak(format!("{} cells, {} time steps", r.cells, r.steps));
        if r.err11.len() == r.f.len() && !r.f.is_empty() {
            // The error bar at the worst return loss and at the top.
            let n = r.f.len() - 1;
            let (_, a, b) = fdtd::db_range(r.s11[worst], r.err11[worst]);
            let (_, c, d) = fdtd::db_range(r.s21[n], r.err21[n]);
            ui.label(format!(
                "Error bars: worst {kind}11 between {a:.1} and {b:.1} dB; {kind}21 at {:.1} GHz between {c:.3} and {d:.3} dB",
                r.f[n]
            ));
        }
        let q = fdtd::quality(r);
        let col = if q.passive() && q.causal() {
            Color32::from_rgb(120, 190, 120)
        } else {
            Color32::from_rgb(230, 110, 90)
        };
        ui.colored_label(col, q.summary());
        for n in &r.notes {
            ui.weak(n);
        }
        let other = match &self.oems {
            Some(Ok(o)) if o.f.len() == r.f.len() => {
                let c = trace_core::openems::compare(r, o);
                ui.label(
                    RichText::new(format!("Trace vs openEMS (dashed): {}", c.summary()))
                        .color(Color32::from_rgb(120, 170, 230)),
                );
                Some(o)
            }
            Some(Err(e)) if !e.is_empty() => {
                ui.colored_label(Color32::from_rgb(230, 110, 90), format!("openEMS: {e}"));
                None
            }
            _ => None,
        };
        plot(ui, r, other, kind);
        self.fields.show(ui, ctx, &r.fields);
    }
}

impl ViaWindow {
    fn optimiser(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, stackup: &Stackup) {
        ui.horizontal(|ui| {
            let busy = self.opt_running.is_some() || self.running.is_some();
            let ok = self.setup.from != self.setup.to && !busy;
            if ui
                .add_enabled(ok, egui::Button::new("Optimise"))
                .on_hover_text(
                    "Sweeps the antipad (round or oval), the return vias, the pad and, if allowed, \
                     the pair's pitch and backdrilling with quick 3D solves, for the best return \
                     loss up to the frequency given, in repeated passes with in-between values. \
                     Every setup keeps the copper clearance buildable.",
                )
                .clicked()
            {
                let st = stackup.clone();
                let base = self.setup.clone();
                let mut goal = viaopt::ViaGoal::within(&base, self.opt_f, &self.fab);
                goal.try_backdrill = self.opt_backdrill && !base.backdrill;
                if !self.accurate {
                    goal.accurate = None;
                }
                if !self.opt_pad {
                    goal.pad = (base.pad, base.pad);
                }
                if let (Some(p), true) = (base.pair, self.free_pitch) {
                    let pad = base.pad.max(base.drill);
                    goal.pitch = ((pad + goal.clearance).min(p), (2.5 * pad).max(p));
                }
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(Mutex::new((0.0, String::new())));
                let p2 = prog.clone();
                let c = ctx.clone();
                let token = crate::jobs::start(ctx, "via-opt");
                std::thread::spawn(move || {
                    let _g = token.install();
                    let r = viaopt::optimise(&st, &base, &goal, &|f, what| {
                        if let Ok(mut p) = p2.lock() {
                            *p = (f, what.to_string());
                        }
                    });
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.opt = None;
                self.opt_running = Some((rx, prog));
            }
            ui.label("for the best return and insertion loss up to");
            ui.add(
                egui::DragValue::new(&mut self.opt_f)
                    .speed(0.5)
                    .range(1.0..=self.setup.fmax_ghz.max(1.0))
                    .suffix(" GHz"),
            );
            if self.is_pair {
                ui.checkbox(&mut self.free_pitch, "pitch too")
                    .on_hover_text("Off: the pair keeps its pitch (a part's pins usually set it)");
            }
            ui.checkbox(&mut self.opt_pad, "pad").on_hover_text(
                "Also try smaller pads, down to a 4 mil annular ring (less pad capacitance)",
            );
            ui.checkbox(&mut self.opt_backdrill, "backdrill")
                .on_hover_text("Also try backdrilling the stub away (costs a fab step)");
            if let Some(p) = self.opt_running.as_ref().map(|(_, p)| p.clone()) {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "via-opt") {
                    self.opt_running = None;
                }
                if let Ok(p) = p.lock() {
                    ui.label(format!("{:.0} %: trying {}", p.0 * 100.0, p.1));
                }
            }
        });
        let mut use_best = None;
        match &self.opt {
            Some(Err(e)) => {
                ui.colored_label(Color32::from_rgb(230, 110, 90), e);
            }
            Some(Ok(o)) => {
                let (s0, _) = viaopt::judge(&o.start, self.opt_f);
                let (s1, il) = viaopt::judge(&o.best, self.opt_f);
                ui.label(
                    RichText::new(format!(
                        "Best: {}. Worst return loss to {:.1} GHz {:.1} dB (was {:.1} dB); {:.3} dB through at {:.1} GHz.",
                        viaopt::describe(&o.best_setup),
                        self.opt_f,
                        s1,
                        s0,
                        il,
                        self.opt_f
                    ))
                    .strong(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Use this setup").clicked() {
                        use_best = Some((o.best_setup.clone(), o.best.clone()));
                    }
                    ui.weak(format!("{} setups tried", o.trials.len()));
                });
                egui::CollapsingHeader::new("Setups tried")
                    .id_salt("via-trials")
                    .show(ui, |ui| {
                        egui::Grid::new("via-trials-grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("Setup");
                                ui.strong("Worst RL");
                                ui.strong("Through");
                                ui.end_row();
                                for t in &o.trials {
                                    ui.label(viaopt::describe(&t.setup));
                                    if let Some(e) = &t.error {
                                        ui.weak("not buildable").on_hover_text(e);
                                        ui.label("");
                                    } else {
                                        ui.label(format!("{:.1} dB", t.worst_rl));
                                        ui.label(format!("{:.3} dB", t.il));
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
            None => {}
        }
        if let Some((s, r)) = use_best {
            self.is_pair = s.pair.is_some();
            if let Some(p) = s.pair {
                self.pitch = p;
            }
            self.setup = s.clone();
            self.result = Some((s, Ok(r)));
        }
    }
}

/// |S11| and |S21| in dB against frequency.
impl ViaWindow {
    /// Check with openEMS: the transition last solved, run in openEMS
    /// (found in C:\openEMS, or TRACE_OPENEMS), its result dashed on the
    /// plot; or only its model written to a folder.
    fn openems_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, stackup: &Stackup) {
        use trace_core::openems;
        if let Some((rx, _)) = &self.oems_running {
            if let Ok(r) = rx.try_recv() {
                self.oems = Some(r);
                self.oems_running = None;
            }
        }
        let Some((setup, Ok(_))) = &self.result else {
            return;
        };
        let setup = setup.clone();
        ui.horizontal(|ui| {
            let exe = openems::find_exe(None);
            let idle = self.oems_running.is_none();
            let tip = match &exe {
                Some(e) => format!(
                    "Runs the same transition in openEMS ({}), an independent FDTD solver, \
                     on half this grid's smallest cell, and draws its result dashed over \
                     Trace's with how far apart they are. Large grids take a while.",
                    e.display()
                ),
                None => "openEMS was not found (looked in C:\\openEMS, TRACE_OPENEMS and the PATH)"
                    .into(),
            };
            if ui
                .add_enabled(
                    idle && exe.is_some(),
                    egui::Button::new("Check with openEMS"),
                )
                .on_hover_text(tip)
                .clicked()
            {
                let exe = exe.unwrap();
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let dir = std::env::temp_dir().join(format!("trace-openems-{stamp}"));
                self.oems_dir = Some(dir.clone());
                let st = stackup.clone();
                let setup = setup.clone();
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(AtomicU32::new(0));
                let p2 = prog.clone();
                let c = ctx.clone();
                let token = crate::jobs::start(ctx, "via-openems");
                std::thread::spawn(move || {
                    let _g = token.install();
                    let progress = |f: f64| p2.store((f * 1000.0) as u32, Ordering::Relaxed);
                    let r = openems::check(&st, &setup, 0.0, &dir, &exe, &progress).map(|(r, _)| r);
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.oems = None;
                self.oems_running = Some((rx, prog));
            }
            if ui
                .add_enabled(idle, egui::Button::new("Export openEMS model…"))
                .on_hover_text(
                    "Writes the transition as an openEMS model (trace_via.xml) to run yourself",
                )
                .clicked()
            {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    let r = openems::setup_for(stackup, &setup, 0.0)
                        .and_then(|s| openems::export(stackup, &s, &dir));
                    self.oems = match r {
                        Ok(_) => {
                            self.oems_dir = Some(dir);
                            None
                        }
                        Err(e) => Some(Err(e)),
                    };
                }
            }
            if let Some(p) = self.oems_running.as_ref().map(|(_, p)| p.clone()) {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "via-openems") {
                    self.oems_running = None;
                }
                ui.label(format!(
                    "openEMS running… {} %",
                    p.load(Ordering::Relaxed) / 10
                ));
            }
            if let Some(d) = &self.oems_dir {
                ui.weak(format!("{}", d.display()));
            }
        });
    }
}

fn plot(ui: &mut egui::Ui, r: &ViaResult, other: Option<&ViaResult>, kind: &str) {
    let w = ui.available_width().max(300.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 300.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let plot = rect
        .shrink2(egui::vec2(36.0, 18.0))
        .translate(egui::vec2(14.0, -4.0));
    let fmax = r.f.last().copied().unwrap_or(1.0);
    let lo = (r
        .s11
        .iter()
        .chain(&r.s21)
        .map(|s| s.db())
        .fold(0.0f64, f64::min)
        .max(-60.0)
        / 10.0)
        .floor()
        * 10.0;
    let x = |f: f64| plot.left() + plot.width() * (f / fmax) as f32;
    let y = |d: f64| plot.top() + plot.height() * (d.clamp(lo, 0.0) / lo) as f32;
    let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text = ui.visuals().weak_text_color();
    let mut d = 0.0;
    while d >= lo - 1e-9 {
        p.line_segment(
            [
                egui::pos2(plot.left(), y(d)),
                egui::pos2(plot.right(), y(d)),
            ],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(plot.left() - 4.0, y(d)),
            egui::Align2::RIGHT_CENTER,
            format!("{d:.0}"),
            egui::FontId::proportional(11.0),
            text,
        );
        d -= 10.0;
    }
    let step = [1.0, 2.0, 5.0, 10.0, 20.0]
        .into_iter()
        .find(|s| fmax / s <= 10.0)
        .unwrap_or(50.0);
    let mut f = 0.0;
    while f <= fmax + 1e-9 {
        p.line_segment(
            [
                egui::pos2(x(f), plot.top()),
                egui::pos2(x(f), plot.bottom()),
            ],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(x(f), plot.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            format!("{f:.0}"),
            egui::FontId::proportional(11.0),
            text,
        );
        f += step;
    }
    let blue = Color32::from_rgb(70, 140, 240);
    let orange = Color32::from_rgb(235, 110, 50);
    for (s, c) in [(&r.s21, blue), (&r.s11, orange)] {
        let pts: Vec<egui::Pos2> =
            r.f.iter()
                .zip(s.iter())
                .map(|(f, v)| egui::pos2(x(*f), y(v.db())))
                .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8_f32, c)));
    }
    // A second solver's result, dashed.
    if let Some(o) = other {
        for (s, c) in [(&o.s21, blue), (&o.s11, orange)] {
            let pts: Vec<egui::Pos2> =
                o.f.iter()
                    .zip(s.iter())
                    .map(|(f, v)| egui::pos2(x(*f), y(v.db())))
                    .collect();
            p.extend(egui::Shape::dashed_line(
                &pts,
                Stroke::new(1.6_f32, c),
                6.0,
                4.0,
            ));
        }
    }
    p.text(
        egui::pos2(plot.right() - 4.0, plot.top() + 4.0),
        egui::Align2::RIGHT_TOP,
        format!("{kind}21"),
        egui::FontId::proportional(12.0),
        blue,
    );
    p.text(
        egui::pos2(plot.right() - 4.0, plot.top() + 20.0),
        egui::Align2::RIGHT_TOP,
        format!("{kind}11"),
        egui::FontId::proportional(12.0),
        orange,
    );
    p.text(
        egui::pos2(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "GHz",
        egui::FontId::proportional(11.0),
        text,
    );
    // Read-out under the pointer.
    if let Some(pos) = resp.hover_pos() {
        if plot.contains(pos) {
            let fq = ((pos.x - plot.left()) / plot.width()) as f64 * fmax;
            if let Some(k) =
                (0..r.f.len()).min_by(|&a, &b| (r.f[a] - fq).abs().total_cmp(&(r.f[b] - fq).abs()))
            {
                p.line_segment(
                    [
                        egui::pos2(x(r.f[k]), plot.top()),
                        egui::pos2(x(r.f[k]), plot.bottom()),
                    ],
                    Stroke::new(1.0_f32, text),
                );
                p.text(
                    egui::pos2(x(r.f[k]) + 6.0, plot.bottom() - 6.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "{:.2} GHz  {kind}21 {:.3} dB  {kind}11 {:.1} dB",
                        r.f[k],
                        r.s21[k].db(),
                        r.s11[k].db()
                    ),
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
        }
    }
}
