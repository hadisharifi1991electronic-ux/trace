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
    opt_running: Option<OptRun>,
    opt: Option<Optimised>,
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
            opt_running: None,
            opt: None,
        }
    }
}

impl ViaWindow {
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
                ui.checkbox(&mut self.is_pair, "differential, vias at");
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
            });
        ui.horizontal(|ui| {
            let ok = self.setup.from != self.setup.to && self.running.is_none();
            if ui.add_enabled(ok, egui::Button::new("Solve")).clicked() {
                let st = stackup.clone();
                let setup = self.setup.clone();
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(AtomicU32::new(0));
                let p2 = prog.clone();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let r = fdtd::solve_with(&st, &setup, &|f| {
                        p2.store((f * 1000.0) as u32, Ordering::Relaxed);
                    });
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.result = Some((self.setup.clone(), Err(String::new())));
                self.running = Some((rx, prog));
            }
            if let Some((_, p)) = &self.running {
                ui.spinner();
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
        ui.label(RichText::new(line).strong());
        if let Some((f, db)) = r.notch() {
            ui.colored_label(
                Color32::from_rgb(230, 180, 90),
                format!("Stub resonance: {kind}21 dips to {db:.1} dB at {f:.2} GHz. Backdrilling removes it."),
            );
        }
        ui.weak(format!("{} cells, {} time steps", r.cells, r.steps));
        for n in &r.notes {
            ui.weak(n);
        }
        plot(ui, r, kind);
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
                    "Sweeps the antipad (round or oval), the return vias and, if allowed, the \
                     pair's pitch with quick 3D solves, for the best return loss up to the \
                     frequency given. Every setup keeps the copper clearance buildable.",
                )
                .clicked()
            {
                let st = stackup.clone();
                let base = self.setup.clone();
                let mut goal = viaopt::ViaGoal::around(&base, self.opt_f);
                if let (Some(p), true) = (base.pair, self.free_pitch) {
                    let pad = base.pad.max(base.drill);
                    goal.pitch = ((pad + goal.clearance).min(p), (2.5 * pad).max(p));
                }
                let (tx, rx) = mpsc::channel();
                let prog = Arc::new(Mutex::new((0.0, String::new())));
                let p2 = prog.clone();
                let c = ctx.clone();
                std::thread::spawn(move || {
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
            ui.label("for the best return loss up to");
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
            if let Some((_, p)) = &self.opt_running {
                ui.spinner();
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
                        "Best: {}. Worst return loss to {:.1} GHz {:.1} dB (was {:.1} dB); {:.2} dB through at {:.1} GHz.",
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
                                        ui.label(format!("{:.2} dB", t.il));
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
fn plot(ui: &mut egui::Ui, r: &ViaResult, kind: &str) {
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
                        "{:.2} GHz  {kind}21 {:.2} dB  {kind}11 {:.1} dB",
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
