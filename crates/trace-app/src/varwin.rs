//! The Assembly variants window: variants of the board with parts left
//! off or changed, and the variant the outputs are made for.

use eframe::egui::{self, RichText};
use trace_core::variant::{natural, Variant};
use trace_core::Board;

#[derive(Default)]
pub struct VariantWindow {
    pub open: bool,
    /// The variant being edited.
    pick: usize,
    new_name: String,
}

/// A change to the board's variants.
pub enum VarAct {
    Add(String),
    Delete(usize),
    /// The variant outputs are made for.
    Active(Option<usize>),
    /// Parts (designators) fitted or not in a variant.
    Fit(usize, Vec<String>, bool),
}

impl VarAct {
    pub fn apply(self, board: &mut Board) -> String {
        match self {
            VarAct::Add(name) => {
                board.variants.push(Variant::new(&name));
                format!("Added the variant {name}")
            }
            VarAct::Delete(i) => {
                if i >= board.variants.len() {
                    return String::new();
                }
                let v = board.variants.remove(i);
                board.variant = match board.variant {
                    Some(a) if a == i => None,
                    Some(a) if a > i => Some(a - 1),
                    a => a,
                };
                format!("Deleted the variant {}", v.name)
            }
            VarAct::Active(a) => {
                board.variant = a.filter(|&i| i < board.variants.len());
                match trace_core::variant::name(board) {
                    "" => "Outputs are for the board as designed".into(),
                    n => format!("Outputs are for the variant {n}"),
                }
            }
            VarAct::Fit(i, parts, fit) => {
                let Some(v) = board.variants.get_mut(i) else {
                    return String::new();
                };
                for d in &parts {
                    if fit {
                        v.not_fitted.remove(d);
                    } else {
                        v.not_fitted.insert(d.clone());
                    }
                }
                format!(
                    "{} {} {} in {}",
                    parts.len(),
                    if parts.len() == 1 { "part" } else { "parts" },
                    if fit { "fitted" } else { "not fitted" },
                    v.name
                )
            }
        }
    }
}

impl VariantWindow {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected_parts: &[usize],
    ) -> Option<VarAct> {
        if !self.open {
            return None;
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Assembly variants")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let label = |i: Option<usize>| match i.and_then(|i| board.variants.get(i)) {
                    Some(v) => v.name.clone(),
                    None => "Board as designed".to_string(),
                };
                ui.horizontal(|ui| {
                    ui.label("Outputs for");
                    egui::ComboBox::from_id_salt("variant_active")
                        .selected_text(label(board.variant))
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(board.variant.is_none(), label(None))
                                .clicked()
                            {
                                out = Some(VarAct::Active(None));
                            }
                            for i in 0..board.variants.len() {
                                if ui
                                    .selectable_label(board.variant == Some(i), label(Some(i)))
                                    .clicked()
                                {
                                    out = Some(VarAct::Active(Some(i)));
                                }
                            }
                        });
                })
                .response
                .on_hover_text("BOM, pick and place, assembly drawing, IPC-2581 and STEP leave off the parts this variant does not fit");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.new_name);
                    let name = self.new_name.trim().to_string();
                    let taken = board
                        .variants
                        .iter()
                        .any(|v| v.name.eq_ignore_ascii_case(&name));
                    if ui
                        .add_enabled(!name.is_empty() && !taken, egui::Button::new("New variant"))
                        .clicked()
                    {
                        self.pick = board.variants.len();
                        self.new_name.clear();
                        out = Some(VarAct::Add(name));
                    }
                });
                if board.variants.is_empty() {
                    ui.label("No variants yet. A PcbDoc brings those of the Altium project beside it.");
                    return;
                }
                self.pick = self.pick.min(board.variants.len() - 1);
                ui.horizontal(|ui| {
                    ui.label("Edit");
                    egui::ComboBox::from_id_salt("variant_pick")
                        .selected_text(board.variants[self.pick].name.clone())
                        .show_ui(ui, |ui| {
                            for (i, v) in board.variants.iter().enumerate() {
                                ui.selectable_value(&mut self.pick, i, v.name.clone());
                            }
                        });
                    if ui.button("Delete").clicked() {
                        out = Some(VarAct::Delete(self.pick));
                    }
                });
                let v = &board.variants[self.pick];
                let names: Vec<String> = selected_parts
                    .iter()
                    .filter_map(|&i| board.components.get(i))
                    .map(|c| c.designator.clone())
                    .collect();
                ui.horizontal(|ui| {
                    let n = names.len();
                    if ui
                        .add_enabled(n > 0, egui::Button::new(format!("Selected parts ({n}) not fitted")))
                        .clicked()
                    {
                        out = Some(VarAct::Fit(self.pick, names.clone(), false));
                    }
                    if ui
                        .add_enabled(n > 0, egui::Button::new("Fitted"))
                        .clicked()
                    {
                        out = Some(VarAct::Fit(self.pick, names.clone(), true));
                    }
                });
                let mut off: Vec<&String> = v.not_fitted.iter().collect();
                off.sort_by(|a, b| natural(a, b));
                ui.label(RichText::new(format!("Not fitted: {}", off.len())).strong());
                egui::ScrollArea::vertical()
                    .id_salt("variant_off")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for d in off {
                                if ui
                                    .small_button(d)
                                    .on_hover_text("Click to fit it again")
                                    .clicked()
                                {
                                    out = Some(VarAct::Fit(self.pick, vec![d.clone()], true));
                                }
                            }
                        });
                    });
                if !v.params.is_empty() {
                    ui.label(RichText::new(format!("Changed: {}", v.params.len())).strong());
                    egui::ScrollArea::vertical()
                        .id_salt("variant_changed")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (d, p) in &v.params {
                                let value = p
                                    .iter()
                                    .find(|(k, _)| k.eq_ignore_ascii_case("Value"))
                                    .map(|(_, x)| x.as_str())
                                    .unwrap_or("");
                                ui.label(format!("{d}: {value} ({} parameters)", p.len()));
                            }
                        });
                }
            });
        self.open = open;
        out
    }
}
