//! The Teardrops and gloss window: teardrops at pads and vias, and a
//! tidy-up of routed tracks.

use eframe::egui::{self, RichText};
use trace_core::edit::Edit;
use trace_core::gloss::{self, GlossGoal};
use trace_core::teardrop::{self, TeardropGoal};
use trace_core::Board;

pub struct TidyWindow {
    pub open: bool,
    td: TeardropGoal,
    gl: GlossGoal,
    /// Only the selected net (and its pair partner).
    selected_only: bool,
    board_key: usize,
    last: Option<String>,
}

impl Default for TidyWindow {
    fn default() -> Self {
        let b = Board::default();
        TidyWindow {
            open: false,
            td: TeardropGoal::of(&b),
            gl: GlossGoal::of(&b),
            selected_only: false,
            board_key: 0,
            last: None,
        }
    }
}

fn nets_of(board: &Board, selected: Option<u32>) -> Vec<u32> {
    let Some(s) = selected else {
        return Vec::new();
    };
    let name = &board.nets[s as usize].name;
    let mut out = vec![s];
    for p in &board.diff_pairs {
        let other = if &p.positive == name {
            &p.negative
        } else if &p.negative == name {
            &p.positive
        } else {
            continue;
        };
        out.extend(trace_core::stitch::net_named(board, other));
    }
    out
}

impl TidyWindow {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) -> Option<(String, Vec<Edit>)> {
        if !self.open {
            return None;
        }
        let key = board as *const Board as usize;
        if key != self.board_key {
            self.board_key = key;
            let c = trace_core::route::board_clearance(board);
            self.td.clearance = c;
            self.gl.clearance = c;
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Teardrops and gloss")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let nets = if self.selected_only {
                    nets_of(board, selected)
                } else {
                    Vec::new()
                };
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        selected.is_some(),
                        egui::Checkbox::new(&mut self.selected_only, "Only the selected net"),
                    );
                    ui.label("Clearance");
                    ui.add(
                        egui::DragValue::new(&mut self.td.clearance)
                            .speed(0.5)
                            .range(1.0..=100.0)
                            .suffix(" mil"),
                    );
                    self.gl.clearance = self.td.clearance;
                });
                if self.selected_only && selected.is_none() {
                    self.selected_only = false;
                }
                ui.separator();
                ui.strong("Teardrops");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.td.vias, "vias");
                    ui.checkbox(&mut self.td.th_pads, "through-hole pads");
                    ui.checkbox(&mut self.td.smd_pads, "surface pads");
                });
                ui.horizontal(|ui| {
                    ui.label("Length");
                    ui.add(
                        egui::DragValue::new(&mut self.td.length)
                            .speed(0.05)
                            .range(0.2..=3.0),
                    )
                    .on_hover_text("Beyond the pad's edge, times the pad's width");
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut self.td.width)
                            .speed(0.05)
                            .range(0.3..=1.0),
                    )
                    .on_hover_text("Where it leaves the pad, times the pad's width");
                });
                ui.horizontal(|ui| {
                    if ui.button("Add teardrops").clicked() {
                        let mut g = self.td.clone();
                        g.nets = nets.clone();
                        let t = teardrop::add(board, &g);
                        self.last = Some(t.summary());
                        if !t.added.is_empty() {
                            out = Some((
                                format!("Added {} teardrops", t.added.len()),
                                t.edits(board),
                            ));
                        }
                    }
                    let has = board
                        .geometry
                        .polygons
                        .iter()
                        .filter(|p| p.teardrop)
                        .count();
                    if ui
                        .add_enabled(has > 0, egui::Button::new("Remove teardrops"))
                        .clicked()
                    {
                        if let Some(e) = teardrop::remove(board) {
                            self.last = Some(format!("{has} teardrops removed"));
                            out = Some((format!("Removed {has} teardrops"), vec![e]));
                        }
                    }
                });
                ui.separator();
                ui.strong("Gloss");
                ui.checkbox(
                    &mut self.gl.merge,
                    "Merge tracks that continue in a straight line",
                );
                ui.checkbox(&mut self.gl.dangling, "Cut back dangling tracks")
                    .on_hover_text(
                        "Track ends that touch nothing of their net, as far back as they go",
                    );
                ui.checkbox(&mut self.gl.corners, "Chamfer square corners (45°)");
                if ui
                    .button("Gloss")
                    .on_hover_text(
                        "Also removes zero-length and duplicate tracks. Ctrl+Z undoes it.",
                    )
                    .clicked()
                {
                    let mut g = self.gl.clone();
                    g.nets = nets.clone();
                    let r = gloss::gloss(board, &g);
                    self.last = Some(r.summary());
                    if !r.is_empty() {
                        out = Some(("Glossed".to_string(), r.edits(board)));
                    }
                }
                if let Some(s) = &self.last {
                    ui.separator();
                    ui.label(RichText::new(s).strong());
                }
            });
        self.open = open;
        out
    }
}
