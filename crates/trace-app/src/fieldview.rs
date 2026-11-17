//! A field map viewer: one cut plane of a 3D solve's fields, a quantity
//! on a dB colour scale, its phase through the cycle (animated or by a
//! slider), with saving as an SVG page or an animated GIF.

use eframe::egui::{self, RichText};
use trace_core::emfield::{self, FieldMap, Quantity};

/// What a drawn picture was made from: map, plane, quantity, scale, phase.
type ViewKey = (usize, usize, usize, i32, i32);

pub struct FieldView {
    /// Keep fields at this frequency on the next solve.
    pub on: bool,
    pub ghz: f64,
    plane: usize,
    q: Quantity,
    range: f64,
    /// None: the peak; else the phase, degrees.
    phase: Option<f64>,
    animate: bool,
    map_idx: usize,
    tex: Option<(ViewKey, egui::TextureHandle)>,
    note: Option<String>,
}

impl Default for FieldView {
    fn default() -> Self {
        FieldView {
            on: false,
            ghz: 6.0,
            plane: 0,
            q: Quantity::E,
            range: 40.0,
            phase: None,
            animate: false,
            map_idx: 0,
            tex: None,
            note: None,
        }
    }
}

impl FieldView {
    /// The checkbox and frequency, beside the Solve button.
    pub fn setup_row(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.on, "Field maps at").on_hover_text(
            "Keep |E|, |H|, energy density and surface current density on cut planes \
             (each trace layer, mid-way between the planes the via crosses, side views) \
             at this frequency, per 1 W in. Adds a little to the solve time.",
        );
        ui.add_enabled(
            self.on,
            egui::DragValue::new(&mut self.ghz)
                .speed(0.1)
                .range(0.01..=200.0)
                .suffix(" GHz"),
        );
    }

    /// The frequencies to ask the solver for.
    pub fn freqs(&self) -> Vec<f64> {
        if self.on {
            vec![self.ghz]
        } else {
            Vec::new()
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, maps: &[FieldMap]) {
        if maps.is_empty() {
            return;
        }
        ui.separator();
        ui.label(RichText::new("Field maps").strong());
        self.map_idx = self.map_idx.min(maps.len() - 1);
        let m = &maps[self.map_idx];
        self.plane = self.plane.min(m.planes.len() - 1);
        ui.horizontal_wrapped(|ui| {
            if maps.len() > 1 {
                egui::ComboBox::from_id_salt("field-f")
                    .selected_text(format!("{:.2} GHz", maps[self.map_idx].f_ghz))
                    .show_ui(ui, |ui| {
                        for (i, mm) in maps.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.map_idx,
                                i,
                                format!("{:.2} GHz", mm.f_ghz),
                            );
                        }
                    });
            } else {
                ui.label(format!("{:.2} GHz", m.f_ghz));
            }
            egui::ComboBox::from_id_salt("field-plane")
                .width(300.0)
                .selected_text(&m.planes[self.plane].name)
                .show_ui(ui, |ui| {
                    for (i, p) in m.planes.iter().enumerate() {
                        ui.selectable_value(&mut self.plane, i, &p.name);
                    }
                });
            egui::ComboBox::from_id_salt("field-q")
                .selected_text(self.q.label())
                .show_ui(ui, |ui| {
                    for q in Quantity::ALL {
                        ui.selectable_value(&mut self.q, q, q.label());
                    }
                });
            ui.label("scale");
            ui.add(
                egui::DragValue::new(&mut self.range)
                    .speed(1.0)
                    .range(10.0..=80.0)
                    .suffix(" dB"),
            );
        });
        ui.horizontal(|ui| {
            let mut inst = self.phase.is_some();
            if ui
                .checkbox(&mut inst, "at an instant")
                .on_hover_text("Off: the peak over the cycle (energy: its average). On: the field at one phase of the cycle.")
                .changed()
            {
                self.phase = inst.then_some(0.0);
                if !inst {
                    self.animate = false;
                }
            }
            if let Some(ph) = &mut self.phase {
                ui.add(egui::Slider::new(ph, 0.0..=359.0).suffix("°"));
                ui.checkbox(&mut self.animate, "animate");
                if self.animate {
                    *ph = (*ph + 7.5) % 360.0;
                    ctx.request_repaint_after(std::time::Duration::from_millis(60));
                }
            } else if ui.button("Animate").clicked() {
                self.phase = Some(0.0);
                self.animate = true;
            }
        });
        let p = &m.planes[self.plane];
        if self.q == Quantity::Current && !p.has_copper() {
            ui.weak("No copper on this plane.");
            return;
        }
        // Shared scale: the quantity's peak over every plane of this map
        // when showing the peak, this plane's peak when animating (so the
        // wave fills the scale).
        let top = if self.phase.is_some() {
            p.values(self.q, None)
                .iter()
                .filter(|v| v.is_finite())
                .fold(0.0f64, |a, &b| a.max(b))
                * if self.q == Quantity::Energy { 2.0 } else { 1.0 }
        } else {
            m.top(self.q) / if self.q == Quantity::Energy { 2.0 } else { 1.0 }
        };
        let key = (
            self.map_idx,
            self.plane,
            Quantity::ALL.iter().position(|&q| q == self.q).unwrap_or(0),
            self.range as i32,
            self.phase.map_or(-1, |p| (p * 10.0) as i32),
        );
        if self.tex.as_ref().is_none_or(|(k, _)| *k != key) {
            let fr = p.frame(720, 200);
            let vals = p.values(self.q, self.phase.map(f64::to_radians));
            let img = p.draw(self.q, &vals, top, self.range, fr, &m.outlines[self.plane]);
            let ci = egui::ColorImage::from_rgb([img.w, img.h], &img.rgb);
            let tex = ctx.load_texture("field-map", ci, egui::TextureOptions::NEAREST);
            self.tex = Some((key, tex));
        }
        if let Some((_, tex)) = &self.tex {
            let size = tex.size_vec2();
            let w = ui.available_width().min(size.x).max(200.0);
            ui.image((tex.id(), egui::vec2(w, size.y * w / size.x)));
        }
        // The scale, in words and a strip.
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 12.0), egui::Sense::hover());
            let painter = ui.painter();
            for k in 0..60 {
                let t = k as f32 / 59.0;
                let c = emfield::colour(t as f64);
                let x0 = rect.left() + rect.width() * k as f32 / 60.0;
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, rect.top()),
                        egui::pos2(x0 + rect.width() / 60.0 + 0.5, rect.bottom()),
                    ),
                    0.0,
                    egui::Color32::from_rgb(c[0], c[1], c[2]),
                );
            }
            ui.label(format!(
                "−{:.0} dB to 0 dB = {} {}",
                self.range,
                emfield::si(top),
                self.q.unit()
            ));
        });
        let peak = p
            .values(self.q, None)
            .into_iter()
            .filter(|v| v.is_finite())
            .fold(0.0f64, f64::max);
        ui.weak(format!(
            "Peak on this plane {} {}; per 1 W into port 1. Copper grey (coloured by its current in the current map).",
            emfield::si(peak),
            self.q.unit()
        ));
        ui.horizontal(|ui| {
            if ui.button("Save maps (SVG)…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .set_file_name("fields.svg")
                    .save_file()
                {
                    self.note = Some(
                        match std::fs::write(&path, m.svg(&Quantity::ALL, self.range)) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Could not save: {e}"),
                        },
                    );
                }
            }
            if ui.button("Save animation (GIF)…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("GIF", &["gif"])
                    .set_file_name("fields.gif")
                    .save_file()
                {
                    self.note = Some(
                        match std::fs::write(&path, m.gif(self.plane, self.q, self.range, 24)) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Could not save: {e}"),
                        },
                    );
                }
            }
        });
        if let Some(n) = &self.note {
            ui.weak(n);
        }
    }
}
