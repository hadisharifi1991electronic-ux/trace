//! The Copy block layout window (design reuse): a laid-out block's
//! placement and routing copied onto another instance of it.

use eframe::egui::{self, RichText};
use trace_core::edit::Edit;
use trace_core::reuse::{match_block, replicate, BlockMatch};
use trace_core::{Board, Length};

pub struct ReuseWindow {
    pub open: bool,
    /// The block's parts by designator, the anchor first.
    block: Vec<String>,
    anchor: usize,
    found: Option<Result<BlockMatch, String>>,
    clear: bool,
    margin_mm: f64,
    last: Option<String>,
}

impl Default for ReuseWindow {
    fn default() -> Self {
        ReuseWindow {
            open: false,
            block: Vec::new(),
            anchor: 0,
            found: None,
            clear: true,
            margin_mm: 1.0,
            last: None,
        }
    }
}

fn index(board: &Board, d: &str) -> Option<usize> {
    board.components.iter().position(|c| c.designator == d)
}

impl ReuseWindow {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: &[usize],
    ) -> Option<(String, Vec<Edit>)> {
        if !self.open {
            return None;
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Copy block layout")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("1. Select the parts of the block that is laid out, then take them.");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !selected.is_empty(),
                            egui::Button::new(format!(
                                "Take the {} selected parts",
                                selected.len()
                            )),
                        )
                        .clicked()
                    {
                        // The part with the most pads anchors the block.
                        let mut v: Vec<(usize, usize)> = selected
                            .iter()
                            .map(|&i| {
                                let n = board
                                    .geometry
                                    .pads
                                    .iter()
                                    .filter(|p| p.component == Some(i as u32))
                                    .count();
                                (n, i)
                            })
                            .collect();
                        v.sort_by(|a, b| b.cmp(a));
                        self.block = v
                            .iter()
                            .map(|&(_, i)| board.components[i].designator.clone())
                            .collect();
                        self.anchor = 0;
                        self.found = None;
                    }
                    ui.label(format!("Block: {} parts", self.block.len()));
                });
                if !self.block.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Anchor");
                        egui::ComboBox::from_id_salt("reuse_anchor")
                            .selected_text(
                                self.block[self.anchor.min(self.block.len() - 1)].clone(),
                            )
                            .show_ui(ui, |ui| {
                                for (i, d) in self.block.iter().enumerate() {
                                    if ui.selectable_value(&mut self.anchor, i, d).changed() {
                                        self.found = None;
                                    }
                                }
                            });
                    });
                }
                ui.separator();
                ui.label("2. Select the copy's part that matches the anchor, then match.");
                let copy_anchor = (selected.len() == 1)
                    .then(|| selected[0])
                    .filter(|&i| !self.block.contains(&board.components[i].designator));
                if ui
                    .add_enabled(
                        copy_anchor.is_some() && !self.block.is_empty(),
                        egui::Button::new(match copy_anchor {
                            Some(i) => format!("Match with {}", board.components[i].designator),
                            None => "Match with the selected part".into(),
                        }),
                    )
                    .clicked()
                {
                    let mut idx: Vec<usize> =
                        self.block.iter().filter_map(|d| index(board, d)).collect();
                    let a = self.anchor.min(idx.len().saturating_sub(1));
                    if a < idx.len() {
                        let first = idx.remove(a);
                        idx.insert(0, first);
                    }
                    self.found = Some(match_block(board, &idx, copy_anchor.unwrap_or(0)));
                }
                if let Some(f) = &self.found {
                    match f {
                        Err(e) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, e);
                        }
                        Ok(m) => {
                            ui.label(
                                RichText::new(format!(
                                    "{} parts matched{}",
                                    m.pairs.len(),
                                    if m.unmatched.is_empty() {
                                        String::new()
                                    } else {
                                        format!(", {} without a counterpart", m.unmatched.len())
                                    }
                                ))
                                .strong(),
                            );
                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for &(s, t) in &m.pairs {
                                            ui.label(format!(
                                                "{}→{}",
                                                board.components[s].designator,
                                                board.components[t].designator
                                            ));
                                        }
                                        for &s in &m.unmatched {
                                            ui.colored_label(
                                                egui::Color32::LIGHT_RED,
                                                format!("{}→?", board.components[s].designator),
                                            );
                                        }
                                    });
                                });
                            ui.separator();
                            ui.label("3. Copy.");
                            ui.checkbox(&mut self.clear, "Take up the copy's own routing first");
                            ui.horizontal(|ui| {
                                ui.label("Routing within");
                                ui.add(
                                    egui::DragValue::new(&mut self.margin_mm)
                                        .speed(0.1)
                                        .range(0.0..=20.0)
                                        .suffix(" mm"),
                                )
                                .on_hover_text("of the block's pads is copied");
                            });
                            if ui.button("Copy placement and routing").clicked() {
                                let (edits, r) = replicate(
                                    board,
                                    m,
                                    Length::from_mm(self.margin_mm),
                                    self.clear,
                                );
                                self.last = Some(r.summary());
                                if !edits.is_empty() {
                                    out = Some(("Copied the block layout".to_string(), edits));
                                }
                            }
                        }
                    }
                }
                if let Some(s) = &self.last {
                    ui.separator();
                    ui.label(s);
                }
            });
        self.open = open;
        out
    }
}
