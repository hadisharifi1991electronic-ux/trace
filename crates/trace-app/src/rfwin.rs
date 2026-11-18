//! The whole-board RF report window: every RF path found from the
//! board's coaxial connectors and run through the path loss, in one
//! table, with where each one reflects and loses most.

use eframe::egui::{self, Color32, RichText};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::rfreport::{self, RfReport, Setup, Status};
use trace_core::touchstone::{self, Network};
use trace_core::{Board, Stackup};

type Outcome = Result<RfReport, String>;
type Run = (mpsc::Receiver<Outcome>, Arc<Mutex<(f64, String)>>);

/// What the window asks the board view to do.
pub enum RfAct {
    /// Show a spot (mil) with a note, the net selected.
    Show(Option<u32>, [f64; 2], String),
    /// Open the path in the Path loss window: from, to, band, load file.
    Open {
        from: String,
        to: String,
        band: (f64, f64),
        load: Option<(String, Result<Network, String>)>,
    },
}

pub struct RfWindow {
    pub open: bool,
    band: (f64, f64),
    rl_goal: f64,
    budget_on: bool,
    budget: f64,
    vias_3d: bool,
    only: String,
    /// Chip port models: pattern, file, the file read.
    loads: Vec<(String, String, Result<Network, String>)>,
    new_pattern: String,
    running: Option<Run>,
    last: Option<Outcome>,
    picked: Option<usize>,
    saved: Option<String>,
}

impl Default for RfWindow {
    fn default() -> Self {
        RfWindow {
            open: false,
            band: (0.5, 6.0),
            rl_goal: 10.0,
            budget_on: false,
            budget: 2.0,
            vias_3d: true,
            only: String::new(),
            loads: Vec::new(),
            new_pattern: "RX*".into(),
            running: None,
            last: None,
            picked: None,
            saved: None,
        }
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn net_id(board: &Board, name: &str) -> Option<u32> {
    board
        .nets
        .iter()
        .position(|n| n.name == name)
        .map(|i| i as u32)
}

impl RfWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, stackup: &Stackup) -> Option<RfAct> {
        if !self.open {
            return None;
        }
        if let Some((rx, _)) = &self.running {
            if let Ok(r) = rx.try_recv() {
                self.last = Some(r);
                self.running = None;
                self.picked = None;
            }
        }
        let mut act = None;
        let mut open = self.open;
        egui::Window::new("RF report")
            .open(&mut open)
            .default_width(900.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Every RF path on the board: each coaxial connector's signal pin followed through series parts and \
                         baluns to the chip, traces from the field solver, each kind of via solved in 3D once, parts from \
                         their value and case. Insertion loss in the band and the worst return loss from its bottom.",
                    )
                    .small()
                    .weak(),
                );
                egui::Grid::new("rf_setup").num_columns(2).show(ui, |ui| {
                    ui.label("Band, GHz");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.band.0).speed(0.05).range(0.0..=100.0));
                        ui.label("to");
                        ui.add(egui::DragValue::new(&mut self.band.1).speed(0.1).range(0.1..=100.0));
                        if self.band.1 <= self.band.0 {
                            self.band.1 = self.band.0 + 0.1;
                        }
                    });
                    ui.end_row();
                    ui.label("Goal");
                    ui.horizontal(|ui| {
                        ui.label("return loss");
                        ui.add(egui::DragValue::new(&mut self.rl_goal).speed(0.5).range(1.0..=40.0).suffix(" dB"));
                        ui.checkbox(&mut self.budget_on, "insertion loss under");
                        ui.add_enabled(
                            self.budget_on,
                            egui::DragValue::new(&mut self.budget).speed(0.05).range(0.05..=40.0).suffix(" dB"),
                        );
                    });
                    ui.end_row();
                    ui.label("Connectors");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.only)
                                .hint_text("all (or J3*, J5)")
                                .desired_width(160.0),
                        );
                        ui.checkbox(&mut self.vias_3d, "vias in 3D")
                            .on_hover_text("Each kind of via solved once with the 3D solver; off: a coaxial estimate");
                    });
                    ui.end_row();
                    ui.label("Chip models");
                    ui.vertical(|ui| {
                        let mut drop = None;
                        for (i, (p, f, n)) in self.loads.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(p).monospace());
                                ui.label(f);
                                if let Err(e) = n {
                                    ui.colored_label(Color32::LIGHT_RED, e);
                                }
                                if ui.small_button("✖").clicked() {
                                    drop = Some(i);
                                }
                            });
                        }
                        if let Some(i) = drop {
                            let _ = self.loads.remove(i);
                        }
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_pattern)
                                    .hint_text("RX*, U101.H17, TX1*")
                                    .desired_width(140.0),
                            )
                            .on_hover_text("Matched against the end pin, its net, the path's name or its class (RX, ORX, TX, LO)");
                            if ui.button("Add .s1p…").clicked() {
                                if let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Touchstone", &["s1p", "s2p"])
                                    .pick_file()
                                {
                                    let name = p
                                        .file_name()
                                        .map_or(String::new(), |n| n.to_string_lossy().to_string());
                                    self.loads.push((
                                        self.new_pattern.trim().to_string(),
                                        name,
                                        touchstone::read(&p).map_err(|e| e.to_string()),
                                    ));
                                }
                            }
                        });
                    });
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    let busy = self.running.is_some();
                    if ui.add_enabled(!busy, egui::Button::new("Run")).clicked() {
                        let setup = Setup {
                            band: self.band,
                            vias_3d: self.vias_3d,
                            loads: self
                                .loads
                                .iter()
                                .filter_map(|(p, f, n)| n.as_ref().ok().map(|n| (p.clone(), f.clone(), n.clone())))
                                .collect(),
                            rl_goal: self.rl_goal,
                            il_budget: self.budget_on.then_some(self.budget),
                            only: self
                                .only
                                .split([',', ' ', ';'])
                                .map(str::trim)
                                .filter(|x| !x.is_empty())
                                .map(String::from)
                                .collect(),
                            ..Default::default()
                        };
                        let b = board.clone();
                        let st = stackup.clone();
                        let (tx, rx) = mpsc::channel();
                        let prog = Arc::new(Mutex::new((0.0, String::new())));
                        let p2 = prog.clone();
                        let c = ctx.clone();
                        let token = crate::jobs::start(ui.ctx(), "rf");
                        std::thread::spawn(move || {
                            let _g = token.install();
                            let pr = |f: f64, what: &str| {
                                if let Ok(mut p) = p2.lock() {
                                    *p = (f, what.to_string());
                                }
                                c.request_repaint();
                            };
                            let _ = tx.send(rfreport::run(&b, &st, &setup, &pr));
                            c.request_repaint();
                        });
                        self.running = Some((rx, prog));
                    }
                    if let Some((_, p)) = &self.running {
                        let (f, what) = p.lock().map(|p| p.clone()).unwrap_or_default();
                        ui.spinner();
                        if crate::jobs::cancel_button(ui, "rf") {
                            self.running = None;
                        }
                        ui.label(format!("{:.0} %: {what}", f * 100.0));
                    }
                    if let Some(Ok(r)) = &self.last {
                        if ui.button("Save…").on_hover_text("Report, CSV, chart, and each path's .s2p and .svg into a folder").clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                self.saved = Some(match save(r, &d) {
                                    Ok(n) => format!("Saved {n} file(s) to {}", d.display()),
                                    Err(e) => format!("Could not save: {e}"),
                                });
                            }
                        }
                    }
                });
                if let Some(s) = &self.saved {
                    ui.label(RichText::new(s).small());
                }
                match &self.last {
                    None => {}
                    Some(Err(e)) => {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    Some(Ok(r)) => {
                        ui.separator();
                        let fails = r.failing();
                        ui.label(RichText::new(r.summary()).strong().color(if fails == 0 {
                            Color32::from_rgb(90, 200, 120)
                        } else {
                            Color32::from_rgb(235, 170, 60)
                        }));
                        let marks = rfreport::marks(r.band);
                        egui::ScrollArea::vertical().max_height(300.0).id_salt("rf_rows").show(ui, |ui| {
                            egui::Grid::new("rf_table").striped(true).show(ui, |ui| {
                                for h in ["", "path", "from", "to"] {
                                    ui.label(RichText::new(h).strong());
                                }
                                for f in &marks {
                                    ui.label(RichText::new(format!("IL {f}")).strong());
                                }
                                for h in ["worst RL", "length", "vias", "parts", ""] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for (i, p) in r.paths.iter().enumerate() {
                                    ui.label(RichText::new(p.class.name()).color(rgb(p.class.rgb())).strong());
                                    if ui
                                        .selectable_label(self.picked == Some(i), &p.name)
                                        .on_hover_text("Details below; shows the path's start on the board")
                                        .clicked()
                                    {
                                        self.picked = Some(i);
                                        if let Ok(rep) = &p.report {
                                            let first = rep.sections.first();
                                            act = Some(RfAct::Show(
                                                first.and_then(|s| net_id(board, &s.net)),
                                                first.map_or([0.0, 0.0], |s| s.at),
                                                format!("{} {}: {}", p.class.name(), p.name, rep.summary()),
                                            ));
                                        }
                                    }
                                    let (from, to) = p
                                        .report
                                        .as_ref()
                                        .map_or((p.from.clone(), "-".to_string()), |r| (r.start.clone(), r.end.clone()));
                                    ui.label(from);
                                    ui.label(to);
                                    for f in &marks {
                                        ui.label(p.il_at(*f).map_or("-".into(), |d| format!("{d:.2}")));
                                    }
                                    ui.label(p.worst_rl().map_or("-".into(), |(d, f)| {
                                        format!("{d:.1} @ {f:.2}{}", if p.load.is_some() { " *" } else { "" })
                                    }));
                                    ui.label(
                                        p.report
                                            .as_ref()
                                            .map_or("-".into(), |r| format!("{:.2} in", r.length / 1000.0)),
                                    );
                                    let (v, n) = p.counts();
                                    ui.label(v.to_string());
                                    ui.label(n.to_string());
                                    let col = match p.status {
                                        Status::Pass => Color32::from_rgb(90, 200, 120),
                                        Status::Check => Color32::from_rgb(235, 170, 60),
                                        Status::Failed => Color32::LIGHT_RED,
                                        Status::NotRf => Color32::GRAY,
                                    };
                                    ui.label(RichText::new(p.status.name()).color(col).strong());
                                    ui.end_row();
                                }
                            });
                        });
                        ui.label(
                            RichText::new(format!(
                                "IL in dB at GHz; worst RL in dB @ GHz from {} GHz (* into the chip's model).",
                                r.band.0
                            ))
                            .small()
                            .weak(),
                        );
                        for n in &r.notes {
                            ui.label(RichText::new(n).small().weak());
                        }
                        if let Some(p) = self.picked.and_then(|i| r.paths.get(i)) {
                            ui.separator();
                            ui.label(RichText::new(format!("{} {}", p.class.name(), p.name)).strong());
                            match &p.report {
                                Err(e) => {
                                    ui.colored_label(Color32::LIGHT_RED, format!("not followed from {}: {e}", p.from));
                                }
                                Ok(rep) => {
                                    ui.label(rep.summary());
                                    if let Some(l) = &p.load {
                                        ui.label(format!("Into {l}"));
                                    }
                                    for f in &p.findings {
                                        ui.colored_label(Color32::from_rgb(235, 170, 60), f);
                                    }
                                    if !p.reflections.is_empty() {
                                        ui.label(RichText::new("Reflects most (click to show):").small());
                                        for (w, g, at) in &p.reflections {
                                            let net = w
                                                .split_once('[')
                                                .and_then(|(_, r)| r.split_once(']'))
                                                .and_then(|(n, _)| net_id(board, n));
                                            if ui.link(format!("|Γ| {g:.2}  {w}")).clicked() {
                                                act = Some(RfAct::Show(net, *at, w.clone()));
                                            }
                                        }
                                    }
                                    if !p.losses.is_empty() {
                                        ui.label(RichText::new(format!("Loses most at {} GHz:", r.band.1)).small());
                                        for (w, d, at) in &p.losses {
                                            if ui.link(format!("{d:.3} dB  {w}")).clicked() {
                                                act = Some(RfAct::Show(None, *at, w.clone()));
                                            }
                                        }
                                    }
                                    for n in &rep.notes {
                                        ui.label(RichText::new(n).small().weak());
                                    }
                                    if ui
                                        .button("Open in Path loss…")
                                        .on_hover_text("Section table, plot, tuner and eye for this path")
                                        .clicked()
                                    {
                                        let load = p.load.as_ref().and_then(|l| {
                                            self.loads
                                                .iter()
                                                .find(|x| &x.1 == l)
                                                .map(|x| (x.1.clone(), x.2.clone()))
                                        });
                                        act = Some(RfAct::Open {
                                            from: rep.start.clone(),
                                            to: rep.end.clone(),
                                            band: r.band,
                                            load,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            });
        self.open = open;
        act
    }
}

fn save(r: &RfReport, d: &std::path::Path) -> Result<usize, String> {
    let w = |n: &str, t: String| std::fs::write(d.join(n), t).map_err(|e| e.to_string());
    w("rf-report.txt", r.text())?;
    w("rf-report.csv", r.csv())?;
    w("rf-report.svg", r.svg("RF paths"))?;
    let mut n = 3;
    for p in &r.paths {
        if let Ok(rep) = &p.report {
            let s = p.file_stem();
            w(&format!("{s}.s2p"), rep.touchstone())?;
            w(
                &format!("{s}.svg"),
                rep.svg(&format!(
                    "{} {}: {} to {}",
                    p.class.name(),
                    p.name,
                    rep.start,
                    rep.end
                )),
            )?;
            n += 2;
        }
    }
    Ok(n)
}
