//! The Smith chart window: S11 (or any Sij) of Touchstone files and of
//! the via window's last solve on the impedance chart. Hover reads Z,
//! VSWR and return loss; a click drops a marker there.

use eframe::egui::{self, Color32, Pos2, RichText, Stroke};
use trace_core::smith::{self, Curve, Reading};

pub struct SmithWindow {
    pub open: bool,
    curves: Vec<Curve>,
    z0: f64,
    param: String,
    /// Markers: (curve, GHz).
    markers: Vec<(usize, f64)>,
    error: Option<String>,
}

impl Default for SmithWindow {
    fn default() -> Self {
        SmithWindow {
            open: false,
            curves: Vec::new(),
            z0: 50.0,
            param: "S11".into(),
            markers: Vec::new(),
            error: None,
        }
    }
}

fn colour(k: usize) -> Color32 {
    let h = smith::COLOURS[k % smith::COLOURS.len()].trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl SmithWindow {
    /// Adds a curve (and opens the window), for other windows' results.
    pub fn add(&mut self, c: Curve, z0: f64) {
        self.z0 = z0;
        self.curves.push(c);
        self.open = true;
    }

    /// `via`: the via window's last result as (name, curve, Z ref), if any.
    pub fn show(&mut self, ctx: &egui::Context, via: Option<(Curve, f64)>) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Smith chart")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("S-parameter");
                    ui.add(egui::TextEdit::singleline(&mut self.param).desired_width(40.0))
                        .on_hover_text("S11, S22, S33 ... (the reflection at a port)");
                    if ui.button("Add Touchstone…").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("Touchstone", &["s1p", "s2p", "s3p", "s4p", "s6p", "s8p"])
                            .pick_file()
                        {
                            self.error = self.load(&p).err();
                        }
                    }
                    let via_ok = via.is_some();
                    if ui
                        .add_enabled(via_ok, egui::Button::new("Add via result"))
                        .on_hover_text("S11 (Sdd11 for a pair) of the via window's last solve")
                        .on_disabled_hover_text("Solve a via in Analysis > Via S-parameters first")
                        .clicked()
                    {
                        if let Some((c, z)) = via.clone() {
                            self.add(c, z);
                        }
                    }
                    if ui.button("Clear").clicked() {
                        self.curves.clear();
                        self.markers.clear();
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(Color32::RED, e);
                }
                self.chart(ui);
            });
        self.open = open;
    }

    fn load(&mut self, p: &std::path::Path) -> Result<(), String> {
        let n = trace_core::touchstone::read(p)?;
        let (i, j) = smith::parse_param(&self.param)
            .ok_or_else(|| format!("not an S-parameter: {}", self.param))?;
        let name = format!(
            "{} S{i}{j}",
            p.file_name()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned())
        );
        let c = smith::from_network(&n, i, j, &name)?;
        self.add(c, n.z_ref);
        Ok(())
    }

    fn chart(&mut self, ui: &mut egui::Ui) {
        let side = ui.available_width().clamp(300.0, 700.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
        let p = ui.painter_at(rect);
        let c = rect.center();
        let r = side * 0.46;
        let to = |g: [f64; 2]| Pos2::new(c.x + g[0] as f32 * r, c.y - g[1] as f32 * r);
        let dark = ui.visuals().dark_mode;
        let (grid, strong, text) = if dark {
            (
                Color32::from_gray(70),
                Color32::from_gray(120),
                Color32::from_gray(150),
            )
        } else {
            (
                Color32::from_gray(205),
                Color32::from_gray(150),
                Color32::from_gray(110),
            )
        };
        p.circle_stroke(c, r, Stroke::new(1.2_f32, strong));
        for (v, line) in smith::grid() {
            let pts: Vec<Pos2> = line.iter().map(|&g| to(g)).collect();
            let s = if (v.abs() - 1.0).abs() < 1e-9 {
                strong
            } else {
                grid
            };
            p.add(egui::Shape::line(pts, Stroke::new(0.8_f32, s)));
        }
        for (g, t) in smith::labels() {
            p.text(
                to(g),
                egui::Align2::CENTER_BOTTOM,
                t,
                egui::FontId::proportional(9.0),
                text,
            );
        }
        for (k, cv) in self.curves.iter().enumerate() {
            let pts: Vec<Pos2> = cv.points.iter().map(|q| to([q.1.re, q.1.im])).collect();
            p.add(egui::Shape::line(pts, Stroke::new(2.0_f32, colour(k))));
            if let Some(q) = cv.points.first() {
                p.circle_filled(to([q.1.re, q.1.im]), 3.0, colour(k));
            }
        }
        // Nearest point to the pointer: hover reading, click marker.
        let near = resp.hover_pos().and_then(|m| {
            let mut best: Option<(f32, usize, f64)> = None;
            for (k, cv) in self.curves.iter().enumerate() {
                for q in &cv.points {
                    let d = to([q.1.re, q.1.im]).distance(m);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, k, q.0));
                    }
                }
            }
            best.filter(|b| b.0 < 20.0)
        });
        let reading = |k: usize, f: f64| -> Option<Reading> {
            smith::at(&self.curves[k], f).map(|g| smith::read(g, f, self.z0))
        };
        if let Some((_, k, f)) = near {
            if let Some(rd) = reading(k, f) {
                p.circle_stroke(
                    to([rd.gamma.re, rd.gamma.im]),
                    5.0,
                    Stroke::new(1.5_f32, colour(k)),
                );
                resp.clone().on_hover_text(rd.text());
                if resp.clicked() {
                    self.markers.push((k, f));
                }
            }
        }
        for (n, &(k, f)) in self.markers.iter().enumerate() {
            if let Some(rd) = reading(k, f) {
                let q = to([rd.gamma.re, rd.gamma.im]);
                p.circle_filled(q, 4.0, colour(k));
                p.text(
                    q + egui::vec2(5.0, -5.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("m{}", n + 1),
                    egui::FontId::proportional(11.0),
                    colour(k),
                );
            }
        }
        ui.label(
            RichText::new(format!(
                "Z0 = {} Ω. Hover a curve to read it; click to drop a marker.",
                self.z0
            ))
            .small()
            .weak(),
        );
        for (k, cv) in self.curves.iter().enumerate() {
            ui.colored_label(colour(k), &cv.name);
        }
        let mut drop = None;
        for (n, &(k, f)) in self.markers.iter().enumerate() {
            if let Some(rd) = reading(k, f) {
                ui.horizontal(|ui| {
                    ui.colored_label(colour(k), format!("m{} {}", n + 1, rd.text()));
                    if ui.small_button("✕").clicked() {
                        drop = Some(n);
                    }
                });
            }
        }
        if let Some(n) = drop {
            self.markers.remove(n);
        }
    }
}
