//! The impedance check window: every controlled trace run and via kind
//! against its target impedance, the worst first.

use eframe::egui::{self, Color32, RichText};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::fdtd::ViaSetup;
use trace_core::zcheck::{self, Goal, Kind, Report, Rule};
use trace_core::{Board, Stackup};

type Run = (mpsc::Receiver<Report>, Arc<Mutex<(f64, String)>>);

/// What the window asks of the app.
pub enum ZAct {
    /// Select a net and show a spot (mil).
    Show(u32, [f64; 2], String),
    /// Open a via in the via window, with its target.
    Via(ViaSetup, f64),
}

pub struct ZWindow {
    pub open: bool,
    goal: Goal,
    tol_pct: f64,
    running: Option<Run>,
    last: Option<Report>,
    show_ok: bool,
    /// A rule being typed: pattern, differential, ohms.
    new_rule: (String, bool, f64),
}

impl Default for ZWindow {
    fn default() -> Self {
        ZWindow {
            open: false,
            goal: Goal::default(),
            tol_pct: 10.0,
            running: None,
            last: None,
            show_ok: false,
            new_rule: (String::new(), false, 50.0),
        }
    }
}

impl ZWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, stackup: &Stackup) -> Option<ZAct> {
        if !self.open {
            return None;
        }
        if let Some((rx, _)) = &self.running {
            if let Ok(r) = rx.try_recv() {
                self.last = Some(r);
                self.running = None;
            }
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Impedance check")
            .open(&mut open)
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Every controlled trace (2D field solver, pour beside it counted) and every \
                         kind of via (3D) against its target.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.goal.pairs, "Differential pairs")
                        .on_hover_text("100 Ω, 90 Ω for USB, 85 Ω for PCIe and SATA");
                    ui.add_enabled(
                        self.goal.pairs,
                        egui::Checkbox::new(&mut self.goal.pairs_by_name, "also by name"),
                    )
                    .on_hover_text(
                        "Nets named X_P/X_N, X+/X-, XP/XN that run side by side, where the board defines no pair",
                    );
                    ui.checkbox(&mut self.goal.rf, "Coax connector nets, 50 Ω");
                });
                ui.horizontal(|ui| {
                    ui.label("Off by more than");
                    ui.add(
                        egui::DragValue::new(&mut self.tol_pct)
                            .speed(0.5)
                            .range(1.0..=50.0)
                            .suffix(" %"),
                    );
                    ui.label("fails, up to");
                    ui.add(
                        egui::DragValue::new(&mut self.goal.ghz)
                            .speed(0.1)
                            .range(0.1..=100.0)
                            .suffix(" GHz"),
                    )
                    .on_hover_text(
                        "Runs shorter than a twentieth of a wavelength here are electrically short: shown, not failed",
                    );
                    ui.label("Shortest run");
                    ui.add(
                        egui::DragValue::new(&mut self.goal.min_length)
                            .speed(1.0)
                            .range(0.0..=1000.0)
                            .suffix(" mil"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.goal.vias, "Vias in 3D, up to");
                    ui.add_enabled(
                        self.goal.vias,
                        egui::DragValue::new(&mut self.goal.via_kinds).range(1..=40),
                    );
                    ui.label("kinds (the commonest first; each takes a minute or so)");
                });
                egui::CollapsingHeader::new(format!("Rules ({})", self.goal.rules.len()))
                    .default_open(self.goal.rules.is_empty())
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Net or pair names with * and ?; the first rule that matches wins.",
                            )
                            .small()
                            .weak(),
                        );
                        let mut drop = None;
                        for (i, r) in self.goal.rules.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut r.pattern);
                                ui.checkbox(&mut r.diff, "differential");
                                ui.add(
                                    egui::DragValue::new(&mut r.ohms)
                                        .speed(0.5)
                                        .range(5.0..=200.0)
                                        .suffix(" Ω"),
                                );
                                if ui.small_button("Remove").clicked() {
                                    drop = Some(i);
                                }
                            });
                        }
                        if let Some(i) = drop {
                            self.goal.rules.remove(i);
                        }
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_rule.0)
                                    .hint_text("e.g. *CLK* or DDR_DQ*"),
                            );
                            ui.checkbox(&mut self.new_rule.1, "differential");
                            ui.add(
                                egui::DragValue::new(&mut self.new_rule.2)
                                    .speed(0.5)
                                    .range(5.0..=200.0)
                                    .suffix(" Ω"),
                            );
                            if ui
                                .add_enabled(
                                    !self.new_rule.0.trim().is_empty(),
                                    egui::Button::new("Add"),
                                )
                                .clicked()
                            {
                                self.goal.rules.push(Rule {
                                    pattern: self.new_rule.0.trim().to_string(),
                                    diff: self.new_rule.1,
                                    ohms: self.new_rule.2,
                                });
                                self.new_rule.0.clear();
                            }
                        });
                    });
                // Named pairs are found when checking (too slow to count
                // each frame).
                let nets = zcheck::controlled(board, &self.goal).len();
                ui.horizontal(|ui| {
                    let busy = self.running.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Check"))
                        .clicked()
                    {
                        let mut goal = self.goal.clone();
                        goal.tolerance = self.tol_pct / 100.0;
                        let b = board.clone();
                        let st = stackup.clone();
                        let (tx, rx) = mpsc::channel();
                        let prog = Arc::new(Mutex::new((0.0, String::new())));
                        let p2 = prog.clone();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let r = zcheck::check(&b, &st, &goal, &|f, what| {
                                if let Ok(mut p) = p2.lock() {
                                    *p = (f, what.to_string());
                                }
                                c.request_repaint();
                            });
                            let _ = tx.send(r);
                            c.request_repaint();
                        });
                        self.running = Some((rx, prog));
                    }
                    ui.label(if self.goal.pairs && self.goal.pairs_by_name {
                        format!("{nets} controlled net(s), and pairs found by name")
                    } else {
                        format!("{nets} controlled net(s)")
                    });
                    if let Some((_, p)) = &self.running {
                        let (f, what) = p.lock().map(|p| p.clone()).unwrap_or_default();
                        ui.spinner();
                        ui.label(format!("{:.0} %: {what}", f * 100.0));
                    }
                });
                let geo = trace_core::impedance::layer_geometry(stackup);
                if let Some(r) = &self.last {
                    ui.separator();
                    ui.label(RichText::new(r.summary()).strong());
                    for n in &r.notes {
                        ui.label(RichText::new(n).small().weak());
                    }
                    if r.vias_skipped.0 > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{} more via kind(s) ({} vias) not solved; raise the kinds to include them.",
                                r.vias_skipped.0, r.vias_skipped.1
                            ))
                            .small()
                            .weak(),
                        );
                    }
                    ui.checkbox(&mut self.show_ok, "Show those on target too");
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for f in &r.findings {
                                let off = f.off().abs() > r.tolerance;
                                let bad = off && !f.short;
                                if !off && !self.show_ok {
                                    continue;
                                }
                                let colour = if bad {
                                    Color32::LIGHT_RED
                                } else if off {
                                    Color32::from_rgb(230, 170, 60)
                                } else {
                                    ui.visuals().text_color()
                                };
                                let text = f.describe(&geo);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.link(RichText::new(&text).color(colour)).clicked() {
                                        out = Some(ZAct::Show(f.net, f.at, text.clone()));
                                    }
                                    if let Kind::Via { setup, .. } = &f.kind {
                                        if ui
                                            .small_button("Via window")
                                            .on_hover_text(
                                                "Open this via, as built, in the 3D via window with its target: Solve sizes it, Optimise tunes it",
                                            )
                                            .clicked()
                                        {
                                            out = Some(ZAct::Via(setup.clone(), f.target));
                                        }
                                    }
                                });
                            }
                        });
                    if ui.button("Save report (CSV)…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("CSV", &["csv"])
                            .set_file_name("impedance-check.csv")
                            .save_file()
                        {
                            if let Err(e) = std::fs::write(&path, r.csv(&geo)) {
                                ui.label(format!("Could not save: {e}"));
                            }
                        }
                    }
                }
            });
        self.open = open;
        out
    }
}
