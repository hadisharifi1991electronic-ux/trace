//! The whole-path loss window: the routed path between two pins, every
//! trace, via and part on it cascaded into one insertion and return loss.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::eye::{EyeResult, EyeSetup};
use trace_core::pathloss::{self, Options, Report, TuneGoal, Tuned};
use trace_core::touchstone::Network;
use trace_core::{Board, Stackup};

type Outcome = Result<(Report, Option<Tuned>), String>;
type Run = (mpsc::Receiver<Outcome>, Arc<Mutex<(f64, String)>>);

pub struct PathWindow {
    pub open: bool,
    from: String,
    to: String,
    ghz: f64,
    points: usize,
    vias_3d: bool,
    tan_d: f64,
    fmin: f64,
    /// Vendor models: pattern, file, the file read.
    models: Vec<(String, String, Result<Network, String>)>,
    new_pattern: String,
    load: Option<(String, Result<Network, String>)>,
    /// Footprints fitted where several share pads, and fitted parts the
    /// tuner may choose again (comma-separated patterns).
    fit: String,
    free: String,
    any_part: bool,
    band: (f64, f64),
    il_weight: f64,
    running: Option<Run>,
    last: Option<Outcome>,
    eye: EyeSetup,
    /// The eye of the last result, once asked for.
    eye_result: Option<Result<EyeResult, String>>,
    /// Let the picker choose the taps and the CTLE.
    pick_ffe: bool,
    pick_ctle: bool,
    picked: Option<trace_core::eye::EqChoice>,
    /// Nets beside the path switch too.
    xtalk: bool,
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
            fmin: 0.0,
            models: Vec::new(),
            new_pattern: String::new(),
            load: None,
            fit: String::new(),
            free: String::new(),
            any_part: false,
            band: (1.0, 6.0),
            il_weight: 1.0,
            running: None,
            last: None,
            eye: EyeSetup::default(),
            eye_result: None,
            pick_ffe: true,
            pick_ctle: true,
            picked: None,
            xtalk: false,
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
                // An eye shown before is drawn again for the new path.
                if self.eye_result.is_some() {
                    self.eye_result = match &r {
                        Ok((rep, _)) => {
                            self.eye.aggressors = if self.xtalk {
                                trace_core::eye::aggressors(board, rep, self.eye.amplitude)
                            } else {
                                Vec::new()
                            };
                            Some(trace_core::eye::eye(rep, &self.eye))
                        }
                        Err(_) => None,
                    };
                }
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
                    ui.label("RL from");
                    ui.add(
                        egui::DragValue::new(&mut self.fmin)
                            .speed(0.05)
                            .range(0.0..=100.0)
                            .suffix(" GHz"),
                    )
                    .on_hover_text("The worst return loss is taken from here up (a DC block's corner lies below the band)");
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
                    ui.label("Fitted where footprints share pads");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fit)
                            .hint_text("R302, C30?")
                            .desired_width(120.0),
                    )
                    .on_hover_text("Of parts stacked on the same pads only one is fitted; without a name a capacitor is taken, then an inductor, then a resistor");
                });
                egui::CollapsingHeader::new("Tune the match")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Chooses E12 capacitors and inductors (with their case's parasitics) for the empty sites on the path \
                                 (TBD, DNI, or the parts named below) for the best worst-case return loss over the band, into the \
                                 load when one is given, plus the mean insertion loss times the weight.",
                            )
                            .small()
                            .weak(),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Band");
                            ui.add(egui::DragValue::new(&mut self.band.0).speed(0.05).range(0.0..=100.0).suffix(" GHz"));
                            ui.label("to");
                            ui.add(egui::DragValue::new(&mut self.band.1).speed(0.05).range(0.0..=100.0).suffix(" GHz"));
                            ui.label("IL weight");
                            ui.add(egui::DragValue::new(&mut self.il_weight).speed(0.1).range(0.0..=20.0));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Also choose again");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.free)
                                    .hint_text("C30?, L301")
                                    .desired_width(120.0),
                            );
                            ui.checkbox(&mut self.any_part, "C or L in any site")
                                .on_hover_text("Off: C sites take capacitors and L sites inductors");
                        });
                    });
                egui::CollapsingHeader::new("Eye diagram")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "A PRBS pattern through the path (into the load when one is given), built from the path's pulse response: height, width and jitter of the eye, and the worst case from the cursors. The edge's content reaches about 2.5 × the bit rate: sweep that high.",
                            )
                            .small()
                            .weak(),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Bit rate");
                            ui.add(egui::DragValue::new(&mut self.eye.gbps).speed(0.05).range(0.01..=112.0).suffix(" Gbit/s"));
                            ui.label("swing");
                            ui.add(egui::DragValue::new(&mut self.eye.amplitude).speed(0.01).range(0.01..=5.0).suffix(" V p-p"))
                                .on_hover_text("What the driver puts into a matched load (differential for a pair)");
                            ui.label("edge");
                            ui.add(
                                egui::DragValue::new(&mut self.eye.rise_ps)
                                    .speed(0.5)
                                    .range(0.0..=5000.0)
                                    .custom_formatter(|v, _| if v <= 0.0 { "auto".into() } else { format!("{v:.1} ps") }),
                            )
                                .on_hover_text("20 to 80 %; 0: 30 % of a bit");
                            egui::ComboBox::from_id_salt("eye_prbs")
                                .selected_text(format!("PRBS{}", self.eye.prbs))
                                .show_ui(ui, |ui| {
                                    for n in [7u32, 9, 11, 15] {
                                        ui.selectable_value(&mut self.eye.prbs, n, format!("PRBS{n}"));
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("Transmit taps: pre");
                            ui.add(egui::DragValue::new(&mut self.eye.eq.pre).speed(0.005).range(-0.3..=0.0).fixed_decimals(3));
                            ui.label("post");
                            ui.add(egui::DragValue::new(&mut self.eye.eq.post).speed(0.005).range(-0.45..=0.0).fixed_decimals(3))
                                .on_hover_text("De-emphasis: the bit after a change is sent this much weaker");
                            ui.label("CTLE");
                            ui.add(egui::DragValue::new(&mut self.eye.eq.ctle_db).speed(0.1).range(0.0..=24.0).suffix(" dB"))
                                .on_hover_text("Receiver peaking: DC this far below the peak (0: none)");
                            ui.label("at");
                            ui.add(
                                egui::DragValue::new(&mut self.eye.eq.ctle_ghz)
                                    .speed(0.05)
                                    .range(0.0..=60.0)
                                    .custom_formatter(|v, _| if v <= 0.0 { "half the rate".into() } else { format!("{v:.2} GHz") }),
                            );
                        });
                        ui.horizontal(|ui| {
                            let rep = match &self.last {
                                Some(Ok((r, _))) => Some(r),
                                _ => None,
                            };
                            ui.checkbox(&mut self.pick_ffe, "taps");
                            ui.checkbox(&mut self.pick_ctle, "CTLE");
                            if ui
                                .add_enabled(rep.is_some() && (self.pick_ffe || self.pick_ctle), egui::Button::new("Pick best"))
                                .on_hover_text("Chooses the ticked ones for the widest worst case eye (opening summed across the bit), then draws it")
                                .clicked()
                            {
                                if let Some(r) = rep {
                                    self.eye.aggressors = if self.xtalk {
                                        trace_core::eye::aggressors(board, r, self.eye.amplitude)
                                    } else {
                                        Vec::new()
                                    };
                                    match trace_core::eye::optimise(r, &self.eye, self.pick_ffe, self.pick_ctle) {
                                        Ok(c) => {
                                            self.eye.eq = c.eq;
                                            self.picked = Some(c);
                                            self.eye_result = Some(trace_core::eye::eye(r, &self.eye));
                                        }
                                        Err(e) => self.eye_result = Some(Err(e)),
                                    }
                                }
                            }
                            ui.checkbox(&mut self.xtalk, "crosstalk")
                                .on_hover_text("Nets beside the path's traces switch too (at the same rate, their own pattern, near or far end whichever is worse)");
                            if ui.small_button("None").on_hover_text("No equalisation").clicked() {
                                self.eye.eq = Default::default();
                                self.picked = None;
                            }
                            if let Some(c) = &self.picked {
                                ui.label(
                                    RichText::new(format!(
                                        "picked: worst case {:.1} -> {:.1} mV",
                                        c.before * 1e3,
                                        c.after * 1e3
                                    ))
                                    .small(),
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            let need = (2.5 * self.eye.gbps).ceil().max(1.0);
                            if ui
                                .button(format!("Sweep for it ({} GHz)", need))
                                .on_hover_text("Sets the top of the sweep and a fine step for this bit rate; then press Run")
                                .clicked()
                            {
                                self.ghz = need;
                                self.points = ((need * 40.0).ceil() as usize).clamp(200, 2000);
                            }
                            let rep = match &self.last {
                                Some(Ok((r, _))) => Some(r),
                                _ => None,
                            };
                            if ui
                                .add_enabled(rep.is_some(), egui::Button::new("Draw eye"))
                                .on_hover_text("From the last run")
                                .clicked()
                            {
                                if let Some(r) = rep {
                                    self.eye.aggressors = if self.xtalk {
                                        trace_core::eye::aggressors(board, r, self.eye.amplitude)
                                    } else {
                                        Vec::new()
                                    };
                                    self.eye_result = Some(trace_core::eye::eye(r, &self.eye));
                                }
                            }
                        });
                    });
                ui.horizontal(|ui| {
                    let busy = self.running.is_some();
                    let run = ui
                        .add_enabled(!busy && !self.from.trim().is_empty(), egui::Button::new("Run"))
                        .clicked();
                    let tune = ui
                        .add_enabled(!busy && !self.from.trim().is_empty(), egui::Button::new("Tune match"))
                        .on_hover_text("Choose parts for the empty sites (see Tune the match)")
                        .clicked();
                    if run || tune {
                        let list = |t: &str| -> Vec<String> {
                            t.split([',', ' ', ';'])
                                .map(str::trim)
                                .filter(|x| !x.is_empty())
                                .map(String::from)
                                .collect()
                        };
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
                            fmin: self.fmin,
                            free: if tune { list(&self.free) } else { Vec::new() },
                            any_part: self.any_part,
                            fit: list(&self.fit),
                        };
                        let goal = tune.then(|| TuneGoal {
                            band: self.band,
                            il_weight: self.il_weight,
                            ..Default::default()
                        });
                        let b = board.clone();
                        let st = stackup.clone();
                        let (tx, rx) = mpsc::channel();
                        let prog = Arc::new(Mutex::new((0.0, String::new())));
                        let p2 = prog.clone();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let pr = |f: f64, what: &str| {
                                if let Ok(mut p) = p2.lock() {
                                    *p = (f, what.to_string());
                                }
                                c.request_repaint();
                            };
                            let r: Outcome = match goal {
                                None => pathloss::run(&b, &st, &opt, &pr).map(|r| (r, None)),
                                Some(g) => pathloss::tune(&b, &st, &opt, &g, &pr)
                                    .map(|t| (t.after.clone(), Some(t))),
                            };
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
                    Some(Ok((r, tuned))) => {
                        ui.separator();
                        if let Some(t) = tuned {
                            ui.label(
                                RichText::new(format!(
                                    "Tuned for {}-{} GHz: worst return loss {:.1} -> {:.1} dB, mean insertion loss {:.3} -> {:.3} dB",
                                    t.band.0, t.band.1, t.worst.0, t.worst.1, t.il.0, t.il.1
                                ))
                                .strong()
                                .color(Color32::from_rgb(90, 200, 120)),
                            );
                            egui::Grid::new("path_tuned").striped(true).show(ui, |ui| {
                                for c in &t.choices {
                                    if ui.link(&c.site).on_hover_text("Show it on the board").clicked() {
                                        out = Some((c.at, format!("{}: {}", c.site, c.part)));
                                    }
                                    ui.label(RichText::new(&c.part).strong());
                                    ui.label(if c.series {
                                        "in series"
                                    } else if c.across {
                                        "across the pair"
                                    } else {
                                        "to ground"
                                    });
                                    ui.end_row();
                                }
                            });
                        }
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
                        if let Some((d, f)) = r.worst_loaded() {
                            ui.label(format!(
                                "Into the path with the load at the end: worst {d:.1} dB at {f:.2} GHz"
                            ));
                        }
                        match &self.eye_result {
                            Some(Ok(e)) => {
                                ui.label(
                                    RichText::new(format!(
                                        "Eye at {} Gbit/s: height {:.1} mV ({:.0} %), width {:.1} ps ({:.0} %), jitter {:.1} ps p-p, worst case {:.1} mV",
                                        e.gbps,
                                        e.height * 1e3,
                                        100.0 * e.height_share(),
                                        e.width_ps,
                                        100.0 * e.width_share(),
                                        e.jitter_ps,
                                        e.worst_height * 1e3
                                    ))
                                    .strong(),
                                );
                                if !e.eq.is_none() {
                                    ui.label(RichText::new(format!("With {}", e.eq.describe(e.gbps))).small());
                                }
                                eye_plot(ui, e);
                                if !e.xtalk.is_empty() {
                                    let total: f64 = e.xtalk.iter().map(|x| x.closure).sum();
                                    ui.label(format!(
                                        "Crosstalk from {} net(s): worst case {:.1} mV less, to {:.1} mV",
                                        e.xtalk.len(),
                                        total * 1e3,
                                        e.worst_with_xtalk * 1e3
                                    ));
                                    egui::Grid::new("eye_xtalk").striped(true).show(ui, |ui| {
                                        for x in e.xtalk.iter().take(12) {
                                            if ui.link(&x.name).on_hover_text("Show where it runs beside the path").clicked() {
                                                out = Some((x.at, format!("{}: {:.1} mV {}", x.name, x.closure * 1e3, x.kind)));
                                            }
                                            ui.label(x.kind);
                                            ui.label(format!("{:.0} mil beside", x.length));
                                            ui.label(format!("closure {:.1} mV", x.closure * 1e3));
                                            ui.label(format!("peak {:.1} mV", x.peak * 1e3));
                                            ui.end_row();
                                        }
                                    });
                                }
                                for n in &e.notes {
                                    ui.label(RichText::new(n).small().color(Color32::from_rgb(230, 170, 60)));
                                }
                            }
                            Some(Err(e)) => {
                                ui.colored_label(Color32::LIGHT_RED, format!("Eye: {e}"));
                            }
                            None => {}
                        }
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
                            if let Some(Ok(e)) = &self.eye_result {
                                if ui.button("Save eye (SVG)…").clicked() {
                                    save(
                                        &format!("{base}-eye.svg"),
                                        "svg",
                                        e.svg(&format!("{} to {} at {} Gbit/s", r.start, r.end, e.gbps)),
                                    );
                                }
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

/// The eye: every trace over two bits, the opening marked at the centre.
fn eye_plot(ui: &mut egui::Ui, e: &EyeResult) {
    let w = ui.available_width().max(320.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 300.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 56.0, rect.top() + 6.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 18.0),
    );
    let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text = ui.visuals().weak_text_color();
    let vmax = e
        .traces
        .iter()
        .flatten()
        .map(|v| (*v as f64).abs())
        .fold(e.amplitude / 2.0, f64::max)
        * 1.08;
    let n = e.traces.first().map_or(65, |t| t.len()).max(2);
    let x = |i: f64| plot.left() + plot.width() * (i / (n - 1) as f64) as f32;
    let y = |v: f64| plot.top() + plot.height() * (0.5 - v / (2.0 * vmax)) as f32;
    let step = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0,
    ]
    .into_iter()
    .find(|s| vmax / s <= 4.0)
    .unwrap_or(5.0);
    let mut v = -(vmax / step).floor() * step;
    while v <= vmax + 1e-12 {
        p.line_segment(
            [
                egui::pos2(plot.left(), y(v)),
                egui::pos2(plot.right(), y(v)),
            ],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(plot.left() - 4.0, y(v)),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0} mV", v * 1e3),
            egui::FontId::proportional(10.0),
            text,
        );
        v += step;
    }
    for k in 0..=4 {
        let xi = x(k as f64 * (n - 1) as f64 / 4.0);
        p.line_segment(
            [egui::pos2(xi, plot.top()), egui::pos2(xi, plot.bottom())],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(xi, plot.bottom() + 1.0),
            egui::Align2::CENTER_TOP,
            format!("{:.0} ps", (k as f64 / 2.0 - 1.0) * e.ui_ps),
            egui::FontId::proportional(10.0),
            text,
        );
    }
    let col = Color32::from_rgba_unmultiplied(70, 140, 240, 70);
    for t in &e.traces {
        let pts: Vec<egui::Pos2> = t
            .iter()
            .enumerate()
            .map(|(i, v)| egui::pos2(x(i as f64), y(*v as f64)))
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.0_f32, col)));
    }
    if e.height > 0.0 {
        let xc = x((n - 1) as f64 / 2.0);
        p.line_segment(
            [egui::pos2(xc, y(e.high)), egui::pos2(xc, y(e.low))],
            Stroke::new(2.0_f32, Color32::from_rgb(235, 110, 50)),
        );
    }
    if let Some(pos) = resp.hover_pos() {
        if plot.contains(pos) {
            let t = ((pos.x - plot.left()) / plot.width()) as f64 * 2.0 - 1.0;
            let v = (0.5 - ((pos.y - plot.top()) / plot.height()) as f64) * 2.0 * vmax;
            p.text(
                pos + egui::vec2(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.1} ps, {:.1} mV", t * e.ui_ps, v * 1e3),
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
        }
    }
}
