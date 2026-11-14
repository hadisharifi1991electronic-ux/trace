//! The 3D clearance window: part bodies that run into each other or come
//! too close, parts taller than the board's Height rules or a limit, and
//! bodies over the board edge.

use crate::view3d::View3d;
use eframe::egui::{self, RichText};
use trace_core::clear3d::{check, Goal, Issue, ModelMesh, Report};
use trace_core::Board;

pub struct ClearWindow {
    pub open: bool,
    goal: Goal,
    top: (bool, f64),
    bottom: (bool, f64),
    /// Waiting for the part models to be read.
    pending: bool,
    last: Option<Report>,
}

impl Default for ClearWindow {
    fn default() -> Self {
        ClearWindow {
            open: false,
            goal: Goal::default(),
            top: (false, 10.0),
            bottom: (false, 3.0),
            pending: false,
            last: None,
        }
    }
}

/// Parts to show, and where (mm).
pub struct ShowParts(pub Vec<usize>, pub [f64; 2], pub String);

fn limit(ui: &mut egui::Ui, label: &str, v: &mut (bool, f64)) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut v.0, label);
        ui.add_enabled(
            v.0,
            egui::DragValue::new(&mut v.1)
                .speed(0.05)
                .range(0.0..=100.0)
                .suffix(" mm"),
        );
    });
}

impl ClearWindow {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        v3: &mut View3d,
    ) -> Option<ShowParts> {
        if !self.open {
            return None;
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("3D clearance")
            .open(&mut open)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Room between bodies");
                    ui.add(
                        egui::DragValue::new(&mut self.goal.clearance)
                            .speed(0.01)
                            .range(0.0..=10.0)
                            .suffix(" mm"),
                    )
                    .on_hover_text("0 checks only that bodies do not touch");
                });
                limit(ui, "Tallest on top", &mut self.top);
                limit(ui, "Tallest on bottom", &mut self.bottom);
                ui.label(
                    RichText::new("The board's Height rules apply as well.")
                        .small()
                        .weak(),
                );
                ui.checkbox(&mut self.goal.edge, "Bodies over the board edge");
                ui.checkbox(
                    &mut self.goal.estimate,
                    "Boxes for parts without a 3D model",
                )
                .on_hover_text("Sized from the pads, as the 3D view draws them");
                if ui
                    .add_enabled(!self.pending, egui::Button::new("Check"))
                    .clicked()
                {
                    self.pending = true;
                }
                if self.pending {
                    match v3.meshes(&board.geometry.models) {
                        Err((done, of)) => {
                            ui.label(format!("Reading the part models: {done} of {of}…"));
                            ctx.request_repaint_after(std::time::Duration::from_millis(200));
                        }
                        Ok(meshes) => {
                            let refs: Vec<Option<ModelMesh>> = meshes
                                .iter()
                                .map(|m| {
                                    m.as_ref().map(|m| ModelMesh {
                                        positions: &m.positions,
                                        indices: &m.indices,
                                    })
                                })
                                .collect();
                            let goal = Goal {
                                max_top: self.top.0.then_some(self.top.1),
                                max_bottom: self.bottom.0.then_some(self.bottom.1),
                                ..self.goal.clone()
                            };
                            self.last = Some(check(board, &refs, &goal));
                            self.pending = false;
                        }
                    }
                }
                if let Some(r) = &self.last {
                    ui.separator();
                    ui.label(RichText::new(r.summary()).strong());
                    if r.issues.is_empty() {
                        ui.label("Nothing found.");
                    } else {
                        ui.label("Click one to show it:");
                    }
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for issue in &r.issues {
                                let colour = match issue {
                                    Issue::Collision { .. } | Issue::TooTall { .. } => {
                                        egui::Color32::LIGHT_RED
                                    }
                                    _ => ui.visuals().text_color(),
                                };
                                let text = issue.describe(board);
                                if ui.link(RichText::new(&text).color(colour)).clicked() {
                                    out = Some(ShowParts(issue.parts(), issue.at(board), text));
                                }
                            }
                        });
                }
            });
        self.open = open;
        out
    }
}
