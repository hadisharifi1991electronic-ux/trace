//! The Fanout window: dogbone or via-in-pad fanout of BGA parts.

use eframe::egui::{self, Color32, RichText};
use trace_core::edit::Edit;
use trace_core::fanout::{self, Direction, FanoutGoal, Style};
use trace_core::{Board, LayerRef};

pub struct FanoutWindow {
    pub open: bool,
    goal: FanoutGoal,
    /// Parts with many surface pads, most first, and the one picked when
    /// nothing is selected.
    candidates: Vec<(usize, usize)>,
    picked: Option<usize>,
    own_width: bool,
    width: f64,
    board_key: usize,
    last: Option<String>,
}

impl Default for FanoutWindow {
    fn default() -> Self {
        FanoutWindow {
            open: false,
            goal: FanoutGoal::of(&Board::default(), Vec::new()),
            candidates: Vec::new(),
            picked: None,
            own_width: false,
            width: 5.0,
            board_key: 0,
            last: None,
        }
    }
}

/// Parts with at least 16 surface pads, by pad count.
fn candidates(board: &Board) -> Vec<(usize, usize)> {
    let mut n = vec![0usize; board.components.len()];
    for p in &board.geometry.pads {
        if let (Some(c), LayerRef::Copper(_)) = (p.component, &p.layer) {
            if p.hole.0 == 0 {
                if let Some(x) = n.get_mut(c as usize) {
                    *x += 1;
                }
            }
        }
    }
    let mut out: Vec<(usize, usize)> = n
        .into_iter()
        .enumerate()
        .filter(|(_, k)| *k >= 16)
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.1));
    out
}

impl FanoutWindow {
    /// `selected`: the parts selected on the board, which it fans out
    /// when there are any.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: &[usize],
    ) -> Option<(String, Vec<Edit>)> {
        if !self.open {
            return None;
        }
        let key = board as *const Board as usize;
        if key != self.board_key {
            // A board loaded, or edited: keep the user's settings, but take
            // the board's via and clearance the first time.
            if self.board_key == 0 || self.candidates.is_empty() {
                let parts = std::mem::take(&mut self.goal.parts);
                self.goal = FanoutGoal::of(board, parts);
            }
            self.board_key = key;
            self.candidates = candidates(board);
            if self
                .picked
                .is_none_or(|p| !self.candidates.iter().any(|c| c.0 == p))
            {
                self.picked = self.candidates.first().map(|c| c.0);
            }
        }
        let mut out = None;
        let mut open = self.open;
        let names: Vec<String> = board
            .layer_geometry()
            .iter()
            .map(|g| g.name.clone())
            .collect();
        let n_layers = names.len().max(2) as u8;
        egui::Window::new("Fanout")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let mil = |ui: &mut egui::Ui, v: &mut f64, lo: f64, hi: f64| {
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.5)
                            .range(lo..=hi)
                            .max_decimals(1)
                            .suffix(" mil"),
                    );
                };
                let des = |i: usize| {
                    board
                        .components
                        .get(i)
                        .map_or(String::new(), |c| c.designator.clone())
                };
                egui::Grid::new("fanout-grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Parts");
                        if selected.is_empty() {
                            let text =
                                self.picked.map_or("none with 16 or more pads".into(), |p| {
                                    let n = self
                                        .candidates
                                        .iter()
                                        .find(|c| c.0 == p)
                                        .map_or(0, |c| c.1);
                                    format!("{} ({n} pads)", des(p))
                                });
                            egui::ComboBox::from_id_salt("fanout-part")
                                .selected_text(text)
                                .height(360.0)
                                .show_ui(ui, |ui| {
                                    for &(i, n) in &self.candidates {
                                        ui.selectable_value(
                                            &mut self.picked,
                                            Some(i),
                                            format!("{} ({n} pads)", des(i)),
                                        );
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Or select parts on the board (click, Ctrl+click, Shift+drag)",
                                );
                        } else {
                            let list: Vec<String> =
                                selected.iter().take(6).map(|&i| des(i)).collect();
                            let more = selected.len().saturating_sub(6);
                            ui.label(if more > 0 {
                                format!("{} and {more} more (selected)", list.join(", "))
                            } else {
                                format!("{} (selected)", list.join(", "))
                            });
                        }
                        ui.end_row();
                        ui.label("Style");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.goal.style, Style::Dogbone, "Dogbone")
                                .on_hover_text("A short track to a via between the balls");
                            ui.radio_value(&mut self.goal.style, Style::InPad, "Via in pad")
                                .on_hover_text("Filled and capped vias in the ball pads");
                        });
                        ui.end_row();
                        if self.goal.style == Style::Dogbone {
                            ui.label("Direction");
                            ui.horizontal(|ui| {
                                ui.radio_value(
                                    &mut self.goal.direction,
                                    Direction::Out,
                                    "Away from the centre",
                                );
                                ui.radio_value(
                                    &mut self.goal.direction,
                                    Direction::In,
                                    "Towards it",
                                );
                            });
                            ui.end_row();
                            ui.label("Neck width");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut self.own_width, false, "Width rule");
                                ui.radio_value(&mut self.own_width, true, "");
                                ui.add_enabled_ui(self.own_width, |ui| {
                                    mil(ui, &mut self.width, 1.0, 50.0)
                                });
                            });
                            ui.end_row();
                        }
                        ui.label("Via pad / drill");
                        ui.horizontal(|ui| {
                            mil(ui, &mut self.goal.via.pad, 4.0, 200.0);
                            mil(ui, &mut self.goal.via.drill, 2.0, self.goal.via.pad);
                        });
                        ui.end_row();
                        ui.label("Layers");
                        ui.horizontal(|ui| {
                            for (id, l) in [
                                ("fo-from", &mut self.goal.via.from),
                                ("fo-to", &mut self.goal.via.to),
                            ] {
                                egui::ComboBox::from_id_salt(id)
                                    .selected_text(
                                        names.get(*l as usize).cloned().unwrap_or_default(),
                                    )
                                    .show_ui(ui, |ui| {
                                        for (i, n) in names.iter().enumerate() {
                                            ui.selectable_value(l, i as u8, n);
                                        }
                                    });
                            }
                        });
                        ui.end_row();
                        ui.label("Clearance");
                        mil(ui, &mut self.goal.clearance, 1.0, 100.0);
                        ui.end_row();
                        ui.label("Outer rows left");
                        ui.add(egui::DragValue::new(&mut self.goal.skip_rows).range(0..=10))
                            .on_hover_text(
                                "Rows round the edge kept for routing out on the surface",
                            );
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.goal.no_net, "Pads without a net too");
                        ui.end_row();
                    });
                if self.goal.via.from > self.goal.via.to {
                    std::mem::swap(&mut self.goal.via.from, &mut self.goal.via.to);
                }
                self.goal.via.to = self.goal.via.to.min(n_layers - 1);
                if self.goal.via.from == self.goal.via.to {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 90),
                        "The vias need two different layers.",
                    );
                }
                ui.weak(
                    "Each via keeps clear of the tracks, pads, vias and other nets' copper; \
                     planes it passes through get a clearance hole, pours are poured again.",
                );
                let parts: Vec<usize> = if selected.is_empty() {
                    self.picked.into_iter().collect()
                } else {
                    selected.to_vec()
                };
                let can = !parts.is_empty() && self.goal.via.from < self.goal.via.to;
                if ui
                    .add_enabled(can, egui::Button::new(RichText::new("Fan out").strong()))
                    .clicked()
                {
                    let mut g = self.goal.clone();
                    g.parts = parts.clone();
                    g.width = self.own_width.then_some(self.width);
                    let f = fanout::fanout(board, &g);
                    self.last = Some(f.summary());
                    if f.pads > 0 {
                        let who: Vec<String> = parts.iter().map(|&i| des(i)).collect();
                        out = Some((
                            format!("Fanned out {} ({} vias)", who.join(", "), f.vias.len()),
                            f.edits(board),
                        ));
                    }
                }
                if let Some(s) = &self.last {
                    ui.label(s);
                }
            });
        self.open = open;
        out
    }
}
