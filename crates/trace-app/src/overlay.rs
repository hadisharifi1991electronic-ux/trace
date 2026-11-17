//! Analysis results drawn on the board: EMI findings, a thermal map with
//! each part's junction temperature, a bus's nets coloured by whether
//! they meet timing. Each analysis window hands its overlay over when a
//! result arrives; the canvas draws every overlay that is switched on,
//! and a small legend in the corner switches them off.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Marker {
    /// Board position, mil.
    pub at: [f64; 2],
    pub color: Color32,
    /// Shown beside the marker when zoomed in enough.
    pub label: String,
}

#[derive(Clone, Default)]
pub struct Overlay {
    pub title: String,
    pub markers: Vec<Marker>,
    /// Nets drawn over in a colour (with a short note in the legend).
    pub nets: Vec<(u32, Color32)>,
    /// An image over the board: texture and its board rectangle
    /// (x0, y0, x1, y1), mil, the image's top row at y1.
    pub image: Option<(egui::TextureHandle, [f32; 4])>,
    pub legend: Vec<(Color32, String)>,
    pub on: bool,
}

/// The overlays by source ("emi", "thermal", "bus").
#[derive(Default)]
pub struct Overlays {
    pub list: BTreeMap<&'static str, Overlay>,
}

impl Overlays {
    pub fn set(&mut self, key: &'static str, o: Overlay) {
        self.list.insert(key, o);
    }

    /// The nets to draw over, with their colours.
    pub fn net_colours(&self) -> Vec<(u32, Color32)> {
        self.list
            .values()
            .filter(|o| o.on)
            .flat_map(|o| o.nets.iter().copied())
            .collect()
    }

    /// Draws the images and markers (nets are drawn by the canvas with
    /// the board's tracks).
    pub fn draw(
        &self,
        shapes: &mut Vec<Shape>,
        painter: &egui::Painter,
        view: crate::canvas::View,
    ) {
        let rect = view.rect;
        for o in self.list.values().filter(|o| o.on) {
            if let Some((tex, b)) = &o.image {
                let a = view.to_screen(b[0], b[3]);
                let c = view.to_screen(b[2], b[1]);
                let r = Rect::from_two_pos(a, c);
                let uv = if view.flip {
                    Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0))
                } else {
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
                };
                shapes.push(Shape::image(
                    tex.id(),
                    r,
                    uv,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 170),
                ));
            }
        }
        let font = egui::FontId::proportional(12.0);
        // Labels that would cover one already drawn are left out (the
        // marker stays; zooming in separates them).
        let mut placed: Vec<Rect> = Vec::new();
        for o in self.list.values().filter(|o| o.on) {
            let labels = o.markers.len() <= 60 || view.scale > 0.6;
            for m in &o.markers {
                let p = view.to_screen(m.at[0] as f32, m.at[1] as f32);
                if !rect.expand(40.0).contains(p) {
                    continue;
                }
                shapes.push(Shape::circle_filled(p, 6.0, m.color));
                shapes.push(Shape::circle_stroke(
                    p,
                    6.0,
                    Stroke::new(1.5_f32, Color32::BLACK),
                ));
                if labels && !m.label.is_empty() {
                    let g = painter.layout_no_wrap(m.label.clone(), font.clone(), Color32::WHITE);
                    let at = p + egui::vec2(9.0, -g.size().y / 2.0);
                    let r = Rect::from_min_size(at, g.size()).expand(2.0);
                    if placed.iter().any(|q| q.intersects(r)) {
                        continue;
                    }
                    placed.push(r);
                    shapes.push(Shape::rect_filled(
                        Rect::from_min_size(at, g.size()).expand(2.0),
                        3.0,
                        Color32::from_black_alpha(190),
                    ));
                    shapes.push(Shape::galley(at, g, Color32::WHITE));
                }
            }
        }
    }

    /// The legend in the canvas's top right corner: each overlay with a
    /// switch, and its colours.
    pub fn legend(&mut self, ui: &mut egui::Ui, rect: Rect) {
        if self.list.is_empty() {
            return;
        }
        let mut remove: Option<&'static str> = None;
        // Drawn in the canvas itself (not as a floating area) so that the
        // analysis windows stay on top of it.
        let at = Rect::from_min_size(
            rect.right_top() + egui::vec2(-250.0, 46.0),
            egui::vec2(240.0, rect.height() - 60.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(at), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(230.0);
                ui.label(egui::RichText::new("Results on the board").strong());
                for (k, o) in self.list.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut o.on, &o.title);
                        if ui.small_button("✖").on_hover_text("Remove").clicked() {
                            remove = Some(k);
                        }
                    });
                    if o.on {
                        for (c, t) in &o.legend {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(r, 2.0, *c);
                                ui.label(t);
                            });
                        }
                    }
                }
            });
        });
        if let Some(k) = remove {
            self.list.remove(k);
        }
    }
}
