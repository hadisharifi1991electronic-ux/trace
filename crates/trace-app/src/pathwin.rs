//! The whole-path loss window: the routed path between two pins, every
//! trace, via and part on it cascaded into one insertion and return loss.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::pathloss::{self, Options, Report};
use trace_core::touchstone::Network;
use trace_core::{Board, Stackup};

type Run = (
    mpsc::Receiver<Result<Report, String>>,
    Arc<Mutex<(f64, String)>>,
);

pub struct PathWindow {
    pub open: bool,
    from: String,
    to: String,
    ghz: f64,
    points: usize,
    vias_3d: bool,
    tan_d: f64,
    /// Vendor models: pattern, file, the file read.
    models: Vec<(String, String, Result<Network, String>)>,
    new_pattern: String,
    load: Option<(String, Result<Network, String>)>,
    running: Option<Run>,
    last: Option<Result<Report, String>>,
}

impl Default for PathWindow {
    fn default() -> Self {
        PathWindow {
            open: false,
            from: String::new(),
            to: String::new(),
            ghz: 6.0,
            points: 200,
            vias_3d: true,
            tan_d: 0.02,
            models: Vec::new(),
            new_pattern: String::new(),
            load: None,
            running: None,
            last: None,
        }
    }
}

fn pick(filter: &[&str]) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Touchstone", filter)
        .pick_file()
}

impl PathWindow {
    /// Returns a spot (mil) and its text to show on the board.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        stackup: &Stackup,
        selected: Option<u32>,
    ) -> Option<([f64; 2], String)> {
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
        egui::Window::new("Path loss")
            .open(&mut open)
            .default_width(760.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "The routed path from one pin to another: each trace from the field solver, each via in 3D, \
                         and the parts on it (series and shunt), cascaded into one insertion and return loss.",
                    )
                    .small()
                    .weak(),
                );
                egui::Grid::new("path_ends").num_columns(2).show(ui, |ui| {
                    ui.label("From");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.from)
                                .hint_text("J503.1, J503 or a net name")
                                .desired_width(220.0),
                        );
                        if let Some(n) = selected.and_then(|n| board.nets.get(n as usize)) {
                            if ui
                                .small_button("Selected net")
                                .on_hover_text(format!("Start on {} (at a connector's pin where it has one)", n.name))
                                .clicked()
                            {
                                self.from = n.name.clone();
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("To");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.to)
                            .hint_text("U101.A5; empty: the farthest chip pin")
                            .desired_width(220.0),
                    );
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.label("Up to");
                    ui.add(
                        egui::DragValue::new(&mut self.ghz)
                            .speed(0.1)
                            .range(0.1..=100.0)
                            .suffix(" GHz"),
                    );
                    ui.label("points");
                    ui.add(egui::DragValue::new(&mut self.points).range(20..=2000));
                    ui.checkbox(&mut self.vias_3d, "Vias in 3D")
                        .on_hover_text("A minute or so per kind of via; off: a coaxial estimate from drill, antipad and barrel");
                    ui.label("Df where the stack-up gives none");
                    ui.add(
                        egui::DragValue::new(&mut self.tan_d)
                            .speed(0.001)
                            .range(0.0..=0.1),
                    );
                });
                egui::CollapsingHeader::new(format!("Part models ({})", self.models.len()))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Vendor Touchstone files by designator, value or part number (* and ?): .s2p for a series \
                                 part, .s1p for a shunt part, .s3p for a balun (port 1 unbalanced, 2 and 3 balanced). \
                                 Parts without one take a model from their value and case size.",
                            )
                            .small()
                            .weak(),
                        );
                        let mut drop = None;
                        for (i, (pat, file, net)) in self.models.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(pat).desired_width(120.0));
                                let name = std::path::Path::new(file.as_str())
                                    .file_name()
                                    .map_or(file.clone(), |n| n.to_string_lossy().to_string());
                                match net {
                                    Ok(n) => ui.label(format!(
                                        "{name}: {} port(s), {:.2} to {:.2} GHz",
                                        n.ports,
                                        n.f.first().unwrap_or(&0.0),
                                        n.f.last().unwrap_or(&0.0)
                                    )),
                                    Err(e) => ui.colored_label(Color32::LIGHT_RED, format!("{name}: {e}")),
                                };
                                if ui.small_button("Remove").clicked() {
                                    drop = Some(i);
                                }
                            });
                        }
                        if let Some(i) = drop {
                            let _ = self.models.remove(i);
                        }
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_pattern)
                                    .hint_text("C340, T30?, 100pF")
                                    .desired_width(120.0),
                            );
                            if ui
                                .add_enabled(
                                    !self.new_pattern.trim().is_empty(),
                                    egui::Button::new("Add file…"),
                                )
                                .clicked()
                            {
                                if let Some(p) = pick(&["s1p", "s2p", "s3p", "s4p"]) {
                                    let net = trace_core::touchstone::read(&p);
                                    self.models.push((
                                        self.new_pattern.trim().to_string(),
                                        p.display().to_string(),
                                        net,
                                    ));
                                    self.new_pattern.clear();
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Load at the end");
                            match &self.load {
                                Some((f, Ok(_))) => {
                                    ui.label(f.as_str());
                                }
                                Some((f, Err(e))) => {
                                    ui.colored_label(Color32::LIGHT_RED, format!("{f}: {e}"));
                                }
                                None => {
                                    ui.label(RichText::new("none (the path between 50/100 Ω ports)").weak());
                                }
                            }
                            if ui
                                .small_button("Choose .s1p…")
                                .on_hover_text("A chip's input as a one-port (such as a vendor's package model): return loss into the path with it at the end")
                                .clicked()
                            {
                                if let Some(p) = pick(&["s1p"]) {
                                    self.load = Some((p.display().to_string(), trace_core::touchstone::read(&p)));
                                }
                            }
                            if self.load.is_some() && ui.small_button("Clear").clicked() {
                                self.load = None;
                            }
                        });
                    });
                ui.horizontal(|ui| {
                    let busy = self.running.is_some();
                    if ui
                        .add_enabled(!busy && !self.from.trim().is_empty(), egui::Button::new("Run"))
                        .clicked()
                    {
                        let opt = Options {
                            from: self.from.trim().to_string(),
                            to: Some(self.to.trim().to_string()).filter(|t| !t.is_empty()),
                            ghz: self.ghz,
                            points: self.points,
                            vias_3d: self.vias_3d,
                            models: self
                                .models
                                .iter()
                                .filter_map(|(p, _, n)| n.as_ref().ok().map(|n| (p.clone(), n.clone())))
                                .collect(),
                            load: self.load.as_ref().and_then(|l| l.1.as_ref().ok().cloned()),
                            tan_d: self.tan_d,
                        };
                        let b = board.clone();
                        let st = stackup.clone();
                        let (tx, rx) = mpsc::channel();
                        let prog = Arc::new(Mutex::new((0.0, String::new())));
                        let p2 = prog.clone();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let r = pathloss::run(&b, &st, &opt, &|f, what| {
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
                    if let Some((_, p)) = &self.running {
                        let (f, what) = p.lock().map(|p| p.clone()).unwrap_or_default();
                        ui.spinner();
                        ui.label(format!("{:.0} %: {what}", f * 100.0));
                    }
                });
                match &self.last {
                    Some(Err(e)) => {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    Some(Ok(r)) => {
                        ui.separator();
                        ui.label(RichText::new(r.summary()).strong());
                        ui.label(format!(
                            "Ports: {} ({} Ω) to {} ({} Ω)",
                            r.start, r.z_ref[0], r.end, r.z_ref[1]
                        ));
                        let top = r.f.last().copied().unwrap_or(0.0);
                        let marks: Vec<String> = [0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0]
                            .into_iter()
                            .filter(|f| *f < top - 1e-9)
                            .chain([top])
                            .filter_map(|f| r.il_at(f).map(|d| format!("{f} GHz {d:.3} dB")))
                            .collect();
                        ui.label(format!("Insertion loss: {}", marks.join(", ")));
                        plot(ui, r);
                        for n in &r.notes {
                            ui.label(RichText::new(n).small().weak());
                        }
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                egui::Grid::new("path_sections")
                                    .striped(true)
                                    .num_columns(6)
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("Section").strong());
                                        ui.label(RichText::new("Z, Ω").strong());
                                        ui.label(RichText::new("Length, mil").strong());
                                        ui.label(RichText::new(format!("Own loss at {top} GHz")).strong());
                                        ui.label(RichText::new("From the start").strong());
                                        ui.label(RichText::new("Model").strong());
                                        ui.end_row();
                                        for s in &r.sections {
                                            let text = format!("{} [{}]", s.what, s.net);
                                            if ui.link(&text).on_hover_text("Show it on the board").clicked() {
                                                out = Some((s.at, text.clone()));
                                            }
                                            ui.label(s.z.map_or(String::new(), |z| {
                                                format!("{z:.1}{}", if s.diff { " diff" } else { "" })
                                            }));
                                            ui.label(if s.length > 0.0 {
                                                format!("{:.0}", s.length)
                                            } else {
                                                String::new()
                                            });
                                            ui.label(format!("{:.3} dB", s.loss_db));
                                            ui.label(format!("{:.3} dB", s.total_db));
                                            ui.label(RichText::new(&s.model).small());
                                            ui.end_row();
                                        }
                                    });
                            });
                        ui.horizontal(|ui| {
                            let save = |name: &str, ext: &str, text: String| {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter(ext, &[ext])
                                    .set_file_name(name)
                                    .save_file()
                                {
                                    let _ = std::fs::write(&path, text);
                                }
                            };
                            let base = format!("{}-{}", r.start, r.end).replace(['.', '/', '\\'], "_");
                            if ui.button("Save Touchstone…").clicked() {
                                save(&format!("{base}.s2p"), "s2p", r.touchstone());
                            }
                            if ui.button("Save plot (SVG)…").clicked() {
                                save(
                                    &format!("{base}.svg"),
                                    "svg",
                                    r.svg(&format!("{} to {}", r.start, r.end)),
                                );
                            }
                            if ui.button("Save sections (CSV)…").clicked() {
                                save(&format!("{base}.csv"), "csv", r.csv());
                            }
                        });
                    }
                    None => {}
                }
            });
        self.open = open;
        out
    }
}

/// Insertion loss (upper) and return loss (lower) against frequency.
fn plot(ui: &mut egui::Ui, r: &Report) {
    let w = ui.available_width().max(320.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 360.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let fmax = r.f.last().copied().unwrap_or(1.0).max(1e-9);
    let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text = ui.visuals().weak_text_color();
    let blue = Color32::from_rgb(70, 140, 240);
    let orange = Color32::from_rgb(235, 110, 50);
    let violet = Color32::from_rgb(150, 110, 240);
    let il: Vec<f64> = r.s.iter().map(|s| s[2].db()).collect();
    let rl: Vec<f64> = r.s.iter().map(|s| s[0].db()).collect();
    let loaded: Vec<f64> = r.s11_loaded.iter().map(|s| s.db()).collect();
    let nice = |v: f64| {
        let s = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0]
            .into_iter()
            .find(|s| v / s <= 6.0)
            .unwrap_or(50.0);
        ((v / s).ceil().max(1.0) * s, s)
    };
    let half = (rect.height() - 24.0) / 2.0;
    let inner = |top: f32| {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + 46.0, rect.top() + top + 6.0),
            egui::pos2(rect.right() - 8.0, rect.top() + top + half - 10.0),
        )
    };
    let (il_span, il_step) = nice(-il.iter().copied().fold(0.0, f64::min).max(-60.0));
    let rl_lo = rl
        .iter()
        .chain(&loaded)
        .copied()
        .fold(0.0, f64::min)
        .max(-60.0);
    let (rl_span, rl_step) = nice(-rl_lo);
    let panels = [
        (
            inner(0.0),
            il_span,
            il_step,
            "IL",
            vec![(&il, blue, "|S21|")],
        ),
        (
            inner(half),
            rl_span,
            rl_step,
            "RL",
            if loaded.is_empty() {
                vec![(&rl, orange, "|S11|")]
            } else {
                vec![(&rl, orange, "|S11|"), (&loaded, violet, "with the load")]
            },
        ),
    ];
    let x = |plot: &egui::Rect, f: f64| plot.left() + plot.width() * (f / fmax) as f32;
    for (plot, span, step, name, series) in &panels {
        let y = |d: f64| plot.top() + plot.height() * ((-d).clamp(0.0, *span) / span) as f32;
        let mut d = 0.0;
        while d <= span + 1e-9 {
            p.line_segment(
                [
                    egui::pos2(plot.left(), y(-d)),
                    egui::pos2(plot.right(), y(-d)),
                ],
                Stroke::new(1.0_f32, grid),
            );
            p.text(
                egui::pos2(plot.left() - 4.0, y(-d)),
                egui::Align2::RIGHT_CENTER,
                if d < 1e-9 {
                    "0".to_string()
                } else if *step < 1.0 {
                    format!("{:.1}", -d)
                } else {
                    format!("{:.0}", -d)
                },
                egui::FontId::proportional(11.0),
                text,
            );
            d += step;
        }
        let fs = [0.5, 1.0, 2.0, 5.0, 10.0, 20.0]
            .into_iter()
            .find(|s| fmax / s <= 12.0)
            .unwrap_or(50.0);
        let mut f = 0.0;
        while f <= fmax + 1e-9 {
            p.line_segment(
                [
                    egui::pos2(x(plot, f), plot.top()),
                    egui::pos2(x(plot, f), plot.bottom()),
                ],
                Stroke::new(1.0_f32, grid),
            );
            p.text(
                egui::pos2(x(plot, f), plot.bottom() + 1.0),
                egui::Align2::CENTER_TOP,
                format!("{f}"),
                egui::FontId::proportional(10.0),
                text,
            );
            f += fs;
        }
        for (i, (data, c, label)) in series.iter().enumerate() {
            let pts: Vec<egui::Pos2> =
                r.f.iter()
                    .zip(data.iter())
                    .map(|(f, v)| egui::pos2(x(plot, *f), y(*v)))
                    .collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.8_f32, *c)));
            p.text(
                egui::pos2(plot.right() - 4.0, plot.top() + 4.0 + 15.0 * i as f32),
                egui::Align2::RIGHT_TOP,
                format!("{name} {label}"),
                egui::FontId::proportional(12.0),
                *c,
            );
        }
    }
    if let Some(pos) = resp.hover_pos() {
        let plot = panels[0].0;
        if pos.x >= plot.left() && pos.x <= plot.right() {
            let fq = ((pos.x - plot.left()) / plot.width()) as f64 * fmax;
            if let Some(k) =
                (0..r.f.len()).min_by(|&a, &b| (r.f[a] - fq).abs().total_cmp(&(r.f[b] - fq).abs()))
            {
                for (pl, ..) in &panels {
                    p.line_segment(
                        [
                            egui::pos2(x(pl, r.f[k]), pl.top()),
                            egui::pos2(x(pl, r.f[k]), pl.bottom()),
                        ],
                        Stroke::new(1.0_f32, text),
                    );
                }
                p.text(
                    egui::pos2(x(&plot, r.f[k]) + 6.0, plot.bottom() - 6.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{:.2} GHz  IL {:.3} dB  RL {:.1} dB", r.f[k], -il[k], rl[k]),
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
        }
    }
}
