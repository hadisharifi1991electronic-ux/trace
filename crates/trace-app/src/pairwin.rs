//! The Pair cross-sections window: every width and gap on a layer that
//! gives the target differential impedance (2D field solver), ranked by
//! loss, with crosstalk to the next pair and etch sensitivity.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::impedance::layer_geometry;
use trace_core::pairopt::{self, PairGoal, PairOption};
use trace_core::{Board, Stackup};

pub struct PairWindow {
    pub open: bool,
    layer: usize,
    goal: PairGoal,
    f_ghz: f64,
    own_neighbour: bool,
    neighbour: f64,
    next_max: f64,
    board_key: usize,
    running: Option<mpsc::Receiver<Vec<PairOption>>>,
    result: Option<(usize, Vec<PairOption>)>,
}

impl Default for PairWindow {
    fn default() -> Self {
        PairWindow {
            open: false,
            layer: 0,
            goal: PairGoal::new(100.0),
            f_ghz: 8.0,
            own_neighbour: false,
            neighbour: 15.0,
            next_max: 2.0,
            board_key: 0,
            running: None,
            result: None,
        }
    }
}

impl PairWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, stackup: &Stackup) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    if let Some(res) = &mut self.result {
                        res.1 = r;
                    }
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let key = board as *const Board as usize;
        if key != self.board_key {
            self.board_key = key;
            if let Some(w) = trace_core::route::board_min_width(board) {
                self.goal.w_min = w.max(1.0);
            }
            self.goal.s_min = trace_core::route::board_clearance(board).clamp(2.0, 20.0);
            self.result = None;
        }
        let mut open = self.open;
        egui::Window::new("Pair cross-sections")
            .open(&mut open)
            .default_width(720.0)
            .show(ctx, |ui| self.body(ui, ctx, stackup));
        self.open = open;
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, stackup: &Stackup) {
        let geo = layer_geometry(stackup);
        if geo.is_empty() {
            ui.label("The stack-up has no copper layers.");
            return;
        }
        self.layer = self.layer.min(geo.len() - 1);
        let g = &mut self.goal;
        egui::Grid::new("pair-goal")
            .num_columns(4)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Layer");
                egui::ComboBox::from_id_salt("pair-layer")
                    .selected_text(geo[self.layer].name.clone())
                    .show_ui(ui, |ui| {
                        for (i, l) in geo.iter().enumerate() {
                            ui.selectable_value(&mut self.layer, i, l.name.clone());
                        }
                    });
                ui.label("Zdiff");
                ui.add(
                    egui::DragValue::new(&mut g.zdiff)
                        .speed(0.5)
                        .range(30.0..=200.0)
                        .suffix(" Ω"),
                );
                ui.end_row();
                let mil = |ui: &mut egui::Ui, v: &mut f64, lo: f64, hi: f64| {
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.1)
                            .range(lo..=hi)
                            .max_decimals(2)
                            .suffix(" mil"),
                    );
                };
                ui.label("Width from");
                ui.horizontal(|ui| {
                    mil(ui, &mut g.w_min, 0.5, 50.0);
                    ui.label("to");
                    mil(ui, &mut g.w_max, g.w_min, 60.0);
                });
                ui.label("Smallest gap");
                mil(ui, &mut g.s_min, 0.5, 50.0);
                ui.end_row();
                ui.label("Loss at");
                ui.add(
                    egui::DragValue::new(&mut self.f_ghz)
                        .speed(0.25)
                        .range(0.1..=100.0)
                        .suffix(" GHz"),
                )
                .on_hover_text("The link's Nyquist frequency: half the bit rate");
                ui.label("Next pair at");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.own_neighbour, "")
                        .on_hover_text("Off: three times the height over the plane");
                    ui.add_enabled_ui(self.own_neighbour, |ui| {
                        mil(ui, &mut self.neighbour, 1.0, 200.0)
                    });
                });
                ui.end_row();
                ui.label("Crosstalk under");
                ui.add(
                    egui::DragValue::new(&mut self.next_max)
                        .speed(0.1)
                        .range(0.1..=20.0)
                        .suffix(" %"),
                );
                ui.end_row();
            });
        g.f = self.f_ghz * 1e9;
        g.neighbour = self.own_neighbour.then_some(self.neighbour);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.running.is_none(),
                    egui::Button::new("Find cross-sections"),
                )
                .clicked()
            {
                let l = geo[self.layer].clone();
                let goal = self.goal.clone();
                let (tx, rx) = mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(pairopt::pair_options(&l, &goal));
                    c.request_repaint();
                });
                self.result = Some((self.layer, Vec::new()));
                self.running = Some(rx);
            }
            if self.running.is_some() {
                ui.spinner();
                ui.label("Solving…");
            }
        });
        let Some((layer, opts)) = &self.result else {
            return;
        };
        if self.running.is_some() {
            return;
        }
        ui.separator();
        if opts.is_empty() {
            ui.colored_label(
                Color32::from_rgb(230, 110, 90),
                "No width in that range reaches the target with the smallest gap allowed.",
            );
            return;
        }
        let best = pairopt::best(opts, self.next_max, None);
        if let Some(b) = best.map(|b| &opts[b]) {
            ui.label(
                RichText::new(format!(
                    "Best on {}: {:.2} mil tracks, {:.2} mil gap: {:.1} Ω, {:.3} dB/in at {:.1} GHz, {:.2} % crosstalk.",
                    geo.get(*layer).map_or("", |g| g.name.as_str()),
                    b.w,
                    b.s,
                    b.zdiff,
                    b.loss,
                    self.f_ghz,
                    b.next
                ))
                .strong(),
            );
        } else {
            ui.colored_label(
                Color32::from_rgb(230, 180, 90),
                "None stays under the crosstalk limit; the table is ranked by loss.",
            );
        }
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                egui::Grid::new("pair-options")
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for h in [
                            "Width",
                            "Gap",
                            "Zdiff",
                            "Loss",
                            "Copper",
                            "Dielectric",
                            "Delay",
                            "Crosstalk",
                            "Far end",
                            "Span",
                            "Etch ±0.5",
                        ] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for (i, o) in opts.iter().enumerate() {
                            let t = |s: String| {
                                if Some(i) == best {
                                    RichText::new(s)
                                        .strong()
                                        .color(Color32::from_rgb(120, 200, 140))
                                } else {
                                    RichText::new(s)
                                }
                            };
                            ui.label(t(format!("{:.2}", o.w)));
                            ui.label(t(format!("{:.2}", o.s)));
                            ui.label(t(format!("{:.1} Ω", o.zdiff)));
                            ui.label(t(format!("{:.3} dB/in", o.loss)));
                            ui.label(t(format!("{:.3}", o.loss_cu)));
                            ui.label(t(format!("{:.3}", o.loss_diel)));
                            ui.label(t(format!("{:.1} ps/in", o.delay)));
                            ui.label(t(format!("{:.2} %", o.next)));
                            ui.label(t(format!("{:.2} %", o.fext)));
                            ui.label(t(format!("{:.1} mil", o.span)));
                            ui.label(t(format!("{:.1} Ω", o.etch)));
                            ui.end_row();
                        }
                    });
            });
        ui.weak("Lengths in mil. Loss is differential, per inch. Crosstalk is near-end, from the next pair at the distance given. Etch: how far Zdiff moves if the tracks come out 0.5 mil narrower.");
    }
}
