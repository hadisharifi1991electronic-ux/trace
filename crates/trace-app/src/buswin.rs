//! The bus simulation window: a parallel bus's nets, its strobe, the
//! driver and receiver, the rate and the receiver's timing; each bit's
//! eye and valid window against the strobe, as a table and a chart.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::bussim::{self, BusResult, BusSetup};
use trace_core::Board;

type Solved = Result<BusResult, String>;

pub struct BusWindow {
    pub open: bool,
    nets: String,
    strobe: String,
    from: String,
    to: String,
    mts: f64,
    swing: f64,
    rise: f64,
    setup_ps: f64,
    hold_ps: f64,
    use_mask: bool,
    mask_mv: f64,
    mask_ui: f64,
    centred: bool,
    xtalk: bool,
    vias_3d: bool,
    error: Option<String>,
    running: Option<mpsc::Receiver<Solved>>,
    result: Option<BusResult>,
    save_path: String,
    note: Option<String>,
}

impl Default for BusWindow {
    fn default() -> Self {
        BusWindow {
            open: false,
            nets: "DDR_DQ*".into(),
            strobe: String::new(),
            from: String::new(),
            to: String::new(),
            mts: 1600.0,
            swing: 0.8,
            rise: 0.0,
            setup_ps: 0.0,
            hold_ps: 0.0,
            use_mask: false,
            mask_mv: 136.0,
            mask_ui: 0.2,
            centred: true,
            xtalk: true,
            vias_3d: false,
            error: None,
            running: None,
            result: None,
            save_path: String::new(),
            note: None,
        }
    }
}

impl BusWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    match r {
                        Ok(res) => {
                            self.result = Some(res);
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("Bus simulation (DDR, parallel buses)")
            .open(&mut open)
            .default_width(860.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(760.0)
                    .show(ui, |ui| self.body(ui, ctx, board));
            });
        self.open = open;
    }

    fn setup(&self) -> BusSetup {
        let opt = |s: &str| {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        };
        BusSetup {
            nets: vec![self.nets.clone()],
            strobe: opt(&self.strobe),
            from: self.from.trim().to_string(),
            to: opt(&self.to),
            mts: self.mts,
            swing: self.swing,
            rise_ps: self.rise,
            setup_ps: self.setup_ps,
            hold_ps: self.hold_ps,
            mask: self.use_mask.then_some((self.mask_mv / 1e3, self.mask_ui)),
            strobe_offset_ui: if self.centred { 0.5 } else { 0.0 },
            xtalk: self.xtalk,
            vias_3d: self.vias_3d,
            ..Default::default()
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, board: &Board) {
        ui.label(
            "Each bit's routed path is cascaded from the board and its eye built with the nets \
             beside it switching; its valid window is then set against the strobe's edge.",
        );
        egui::Grid::new("bus-in")
            .num_columns(4)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label("Nets");
                ui.add_sized(
                    [240.0, 20.0],
                    egui::TextEdit::singleline(&mut self.nets)
                        .hint_text("DDR_DQ0, DDR_DQ1… or DDR_DQ*")
                        .desired_width(240.0),
                )
                .on_hover_text("Names or patterns (* and ?), split by commas");
                ui.label("Strobe or clock");
                ui.add_sized(
                    [160.0, 20.0],
                    egui::TextEdit::singleline(&mut self.strobe)
                        .hint_text("DQS0 (optional)")
                        .desired_width(160.0),
                )
                .on_hover_text("A net, or a pair by its name or either line");
                ui.end_row();
                ui.label("From (driver)");
                ui.add_sized(
                    [240.0, 20.0],
                    egui::TextEdit::singleline(&mut self.from)
                        .hint_text("U1")
                        .desired_width(240.0),
                )
                .on_hover_text("The controller for writes, the memory for reads");
                ui.label("To (receiver)");
                ui.add_sized(
                    [160.0, 20.0],
                    egui::TextEdit::singleline(&mut self.to)
                        .hint_text("U5 (optional)")
                        .desired_width(160.0),
                );
                ui.end_row();
                ui.label("Rate");
                ui.add(
                    egui::DragValue::new(&mut self.mts)
                        .speed(10.0)
                        .range(10.0..=20000.0)
                        .suffix(" MT/s"),
                );
                ui.label("Swing / edge");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.swing)
                            .speed(0.01)
                            .range(0.05..=5.0)
                            .suffix(" V"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.rise)
                            .speed(1.0)
                            .range(0.0..=5000.0)
                            .suffix(" ps"),
                    )
                    .on_hover_text("20 to 80 %; 0 takes 30 % of a bit");
                });
                ui.end_row();
                ui.label("Setup / hold");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.setup_ps)
                            .speed(1.0)
                            .range(0.0..=2000.0)
                            .suffix(" ps"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.hold_ps)
                            .speed(1.0)
                            .range(0.0..=2000.0)
                            .suffix(" ps"),
                    );
                });
                ui.checkbox(&mut self.use_mask, "Receiver mask")
                    .on_hover_text("DDR4 and later: VdIVW and TdIVW from the datasheet (DDR4-2400: 136 mV, 0.2 UI)");
                ui.add_enabled_ui(self.use_mask, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.mask_mv)
                                .speed(1.0)
                                .range(1.0..=2000.0)
                                .suffix(" mV"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.mask_ui)
                                .speed(0.01)
                                .range(0.01..=0.9)
                                .suffix(" UI"),
                        );
                    });
                });
                ui.end_row();
            });
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.centred, true, "strobe centred in the data")
                .on_hover_text("DDR writes send it so; DDR reads delay it a quarter clock");
            ui.radio_value(&mut self.centred, false, "edge aligned");
            ui.checkbox(&mut self.xtalk, "crosstalk");
            ui.checkbox(&mut self.vias_3d, "vias in 3D").on_hover_text(
                "Solve each kind of via full-wave (slower); else a coaxial estimate",
            );
        });
        ui.horizontal(|ui| {
            let ok = self.running.is_none()
                && !self.from.trim().is_empty()
                && !self.nets.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new("Simulate")).clicked() {
                let setup = self.setup();
                let b = board.clone();
                let (tx, rx) = mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let r = bussim::run(&b, &setup, &|_, _| {});
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.running = Some(rx);
                self.error = None;
            }
            if self.running.is_some() {
                ui.spinner();
                ui.label("Simulating…");
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 110, 90), e);
        }
        let Some(r) = &self.result else {
            return;
        };
        ui.separator();
        let pass = r.passes();
        let head = match &r.routed {
            Some(m) => format!(
                "As routed: setup {:+.1} ps, hold {:+.1} ps · trained {:+.1} ps each side · per-bit deskew {:+.1} ps",
                m.setup, m.hold, r.trained.setup, r.deskewed.setup
            ),
            None => format!(
                "Every bit valid for {:.1} ps of {:.1} · trained {:+.1} ps each side · per-bit deskew {:+.1} ps",
                r.common[1] - r.common[0],
                r.ui_ps,
                r.trained.setup,
                r.deskewed.setup
            ),
        };
        ui.label(
            RichText::new(format!("{} · {head}", if pass { "PASS" } else { "FAIL" }))
                .strong()
                .color(if pass {
                    Color32::from_rgb(90, 190, 130)
                } else {
                    Color32::from_rgb(230, 100, 80)
                }),
        );
        if let Some(s) = &r.strobe {
            ui.weak(format!(
                "Strobe {}: {:.0} mil, flight {:.1} ps, edge {:.1} ps after the data launch, jitter {:.1} ps",
                s.net, s.length, s.flight_ps, s.edge_ps, s.jitter_ps
            ));
        }
        chart(ui, r);
        let refd = r.strobe.as_ref().map(|s| s.flight_ps).unwrap_or_else(|| {
            r.bits.iter().map(|b| b.flight_ps).sum::<f64>() / r.bits.len().max(1) as f64
        });
        let probe = BusSetup {
            mask: r.mask,
            ..Default::default()
        };
        egui::Grid::new("bus-res")
            .num_columns(10)
            .striped(true)
            .spacing([10.0, 2.0])
            .show(ui, |ui| {
                for h in [
                    "Net",
                    "Length",
                    "Skew",
                    "Height",
                    "Width",
                    "Crosstalk",
                    "Setup",
                    "Hold",
                    "Mask",
                    "",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                let opt = |v: Option<f64>, k: f64, u: &str| {
                    v.map_or("-".to_string(), |x| format!("{:+.1} {u}", x * k))
                };
                for b in &r.bits {
                    ui.label(&b.net);
                    ui.label(format!("{:.0} mil", b.length));
                    ui.label(format!("{:+.1} ps", b.flight_ps - refd));
                    ui.label(format!("{:.0} mV", b.height * 1e3));
                    ui.label(format!("{:.1} ps", b.width_ps));
                    ui.label(format!("{:.1} mV", b.xtalk * 1e3))
                        .on_hover_text(&b.worst_aggressor);
                    ui.label(opt(b.setup, 1.0, "ps"));
                    ui.label(opt(b.hold, 1.0, "ps"));
                    ui.label(
                        b.mask_height
                            .map_or("-".into(), |m| format!("{:.0} mV", m * 1e3)),
                    );
                    if b.passes(&probe) {
                        ui.colored_label(Color32::from_rgb(90, 190, 130), "ok");
                    } else {
                        ui.colored_label(Color32::from_rgb(230, 100, 80), "fail");
                    }
                    ui.end_row();
                }
            });
        for (n, e) in &r.skipped {
            ui.weak(format!("skipped {n}: {e}"));
        }
        for n in &r.notes {
            ui.weak(format!("note: {n}"));
        }
        ui.horizontal(|ui| {
            ui.label("Save to");
            ui.add(
                egui::TextEdit::singleline(&mut self.save_path)
                    .hint_text("folder\\name (timing.svg, bits.csv, report.txt)")
                    .desired_width(320.0),
            );
            if ui.button("Save").clicked() && !self.save_path.trim().is_empty() {
                let base = self.save_path.trim().trim_end_matches(['/', '\\']);
                let base = base
                    .trim_end_matches(".svg")
                    .trim_end_matches(".csv")
                    .trim_end_matches(".txt")
                    .to_string();
                let w = |ext: &str, text: String| std::fs::write(format!("{base}{ext}"), text);
                let res = w(".svg", r.svg("Bus timing"))
                    .and_then(|_| w(".csv", r.csv()))
                    .and_then(|_| w(".txt", r.report()));
                self.note = Some(match res {
                    Ok(()) => format!("Wrote {base}.svg, .csv and .txt"),
                    Err(e) => format!("Could not save: {e}"),
                });
            }
        });
        if let Some(n) = &self.note {
            ui.weak(n);
        }
    }
}

/// Each bit's valid window against the strobe's edge (or the centre of
/// the common window), with the receiver's need shaded.
fn chart(ui: &mut egui::Ui, r: &BusResult) {
    let n = r.bits.len().max(1);
    let row = 16.0;
    let label_w = 120.0;
    let w = ui.available_width().min(820.0);
    let h = row * n as f32 + 26.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let centre = r
        .strobe
        .as_ref()
        .map_or((r.common[0] + r.common[1]) / 2.0, |s| s.edge_ps);
    let span = 1.25 * r.ui_ps;
    let (x0, x1) = (rect.left() + label_w, rect.right() - 8.0);
    let x = |ps: f64| x0 + (x1 - x0) * ((((ps - centre) / (2.0 * span)) + 0.5) as f32);
    let top = rect.top() + 2.0;
    let bottom = top + row * n as f32;
    let weak = ui.visuals().weak_text_color();
    let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
    for k in -5..=5 {
        let ps = centre + k as f64 * r.ui_ps / 4.0;
        let xi = x(ps);
        p.line_segment(
            [egui::pos2(xi, top), egui::pos2(xi, bottom)],
            egui::Stroke::new(if k == 0 { 1.5_f32 } else { 0.5_f32 }, grid),
        );
        if k % 2 == 0 {
            p.text(
                egui::pos2(xi, bottom + 3.0),
                egui::Align2::CENTER_TOP,
                format!("{:.0}", ps - centre),
                egui::FontId::proportional(11.0),
                weak,
            );
        }
    }
    let js = r.strobe.as_ref().map_or(0.0, |s| s.jitter_ps);
    let need = (
        centre - js / 2.0 - r.setup_ps,
        centre + js / 2.0 + r.hold_ps,
    );
    if need.1 - need.0 > 0.1 {
        p.rect_filled(
            egui::Rect::from_x_y_ranges(x(need.0)..=x(need.1), top..=bottom),
            0.0,
            Color32::from_rgba_unmultiplied(212, 56, 13, 40),
        );
    }
    if let Some((_, mw)) = r.mask {
        let half = mw * r.ui_ps / 2.0 + js / 2.0;
        p.rect_stroke(
            egui::Rect::from_x_y_ranges(x(centre - half)..=x(centre + half), top..=bottom),
            0.0,
            egui::Stroke::new(1.0_f32, Color32::from_rgb(212, 56, 13)),
        );
    }
    let probe = BusSetup {
        mask: r.mask,
        ..Default::default()
    };
    let text = ui.visuals().text_color();
    for (i, b) in r.bits.iter().enumerate() {
        let y = top + row * i as f32;
        p.text(
            egui::pos2(x0 - 6.0, y + row / 2.0),
            egui::Align2::RIGHT_CENTER,
            &b.net,
            egui::FontId::proportional(12.0),
            text,
        );
        let (a, z) = (x(b.window[0]).max(x0), x(b.window[1]).min(x1));
        if z > a {
            p.rect_filled(
                egui::Rect::from_x_y_ranges(a..=z, y + 3.0..=y + row - 3.0),
                3.0,
                if b.passes(&probe) {
                    Color32::from_rgb(47, 158, 110)
                } else {
                    Color32::from_rgb(212, 56, 13)
                },
            );
        }
    }
    p.text(
        egui::pos2(rect.left(), bottom + 3.0),
        egui::Align2::LEFT_TOP,
        if r.strobe.is_some() {
            "ps from the strobe"
        } else {
            "ps from the centre"
        },
        egui::FontId::proportional(11.0),
        weak,
    );
}
