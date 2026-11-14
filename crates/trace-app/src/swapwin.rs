//! The Pin and gate swap window: two pins of a part trade nets on the
//! board (with the routing that leaves each pad), or every pin of two
//! gates of one part does; the open schematic is changed to match.

use eframe::egui::{self, RichText};
use trace_core::Board;

/// The gates drawn for a designator: (sheet slot, part, gate).
pub type GateList<'a> = dyn Fn(&str) -> Vec<(usize, usize, u32)> + 'a;

pub enum SwapAct {
    /// Part index, pad names.
    Pins(usize, String, String),
    /// Two drawn gates: (sheet slot, part on the sheet).
    Gates((usize, usize), (usize, usize)),
}

#[derive(Default)]
pub struct SwapWindow {
    pub open: bool,
    part: Option<usize>,
    find: String,
    a: Option<String>,
    b: Option<String>,
    ga: Option<usize>,
    gb: Option<usize>,
}

impl SwapWindow {
    /// `selected`: the part selected on the board, if just one;
    /// `gates`: the gates of the chosen part drawn on the sheets, as
    /// (sheet slot, part, gate letter number).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<usize>,
        gates: &GateList<'_>,
        sheets_open: bool,
    ) -> Option<SwapAct> {
        if !self.open {
            return None;
        }
        if let Some(s) = selected {
            if self.part != Some(s) {
                self.part = Some(s);
                self.find = board.components[s].designator.clone();
                self.a = None;
                self.b = None;
                self.ga = None;
                self.gb = None;
            }
        }
        let mut act = None;
        let mut open = true;
        egui::Window::new("Pin and gate swap")
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Part");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.find).desired_width(100.0));
                    if r.changed() {
                        self.part = board
                            .components
                            .iter()
                            .position(|c| c.designator.eq_ignore_ascii_case(self.find.trim()));
                        self.a = None;
                        self.b = None;
                        self.ga = None;
                        self.gb = None;
                    }
                    if self.part.is_none() {
                        ui.weak("type a designator, or click a part");
                    }
                });
                let Some(part) = self.part.filter(|&p| p < board.components.len()) else {
                    return;
                };
                let des = board.components[part].designator.clone();
                let mut pads: Vec<(String, String)> = board
                    .geometry
                    .pads
                    .iter()
                    .filter(|p| p.component == Some(part as u32) && !p.name.is_empty())
                    .map(|p| {
                        let net = p
                            .net
                            .and_then(|n| board.nets.get(n as usize))
                            .map_or("no net".to_string(), |n| n.name.clone());
                        (p.name.clone(), net)
                    })
                    .collect();
                pads.sort_by_key(|x| natural(&x.0));
                pads.dedup_by(|x, y| x.0 == y.0);
                ui.separator();
                ui.strong("Pins");
                let label = |n: &Option<String>| {
                    n.as_ref()
                        .and_then(|n| pads.iter().find(|p| &p.0 == n))
                        .map_or("pick a pin".to_string(), |p| format!("{} ({})", p.0, p.1))
                };
                ui.horizontal(|ui| {
                    for (id, v) in [("swap-a", &mut self.a), ("swap-b", &mut self.b)] {
                        let text = label(v);
                        egui::ComboBox::from_id_salt(id)
                            .selected_text(text)
                            .width(170.0)
                            .height(400.0)
                            .show_ui(ui, |ui| {
                                for (name, net) in &pads {
                                    ui.selectable_value(v, Some(name.clone()), format!("{name} ({net})"));
                                }
                            });
                    }
                });
                let ok = matches!((&self.a, &self.b), (Some(a), Some(b)) if a != b);
                if ui
                    .add_enabled(ok, egui::Button::new(RichText::new("Swap pins").strong()))
                    .on_hover_text("The two pads trade nets; tracks and vias leading only to one of them go with it")
                    .clicked()
                {
                    if let (Some(a), Some(b)) = (&self.a, &self.b) {
                        act = Some(SwapAct::Pins(part, a.clone(), b.clone()));
                    }
                }
                ui.separator();
                ui.strong("Gates");
                if !sheets_open {
                    ui.weak("Open the schematic (View > Schematic) to swap gates; it says which pins make each gate.");
                    return;
                }
                let gs = gates(&des);
                if gs.len() < 2 {
                    ui.weak(format!("{des} has no other gate drawn on the sheets."));
                    return;
                }
                let name = |k: Option<usize>| {
                    k.and_then(|k| gs.get(k)).map_or("pick a gate".into(), |g| {
                        format!("{des}{}", (b'A' + (g.2.clamp(1, 26) - 1) as u8) as char)
                    })
                };
                ui.horizontal(|ui| {
                    for (id, v) in [("gate-a", &mut self.ga), ("gate-b", &mut self.gb)] {
                        egui::ComboBox::from_id_salt(id)
                            .selected_text(name(*v))
                            .show_ui(ui, |ui| {
                                for k in 0..gs.len() {
                                    ui.selectable_value(v, Some(k), name(Some(k)));
                                }
                            });
                    }
                });
                let ok = matches!((self.ga, self.gb), (Some(a), Some(b)) if a != b);
                if ui
                    .add_enabled(ok, egui::Button::new(RichText::new("Swap gates").strong()))
                    .on_hover_text("Every pin of one gate trades nets with the matching pin of the other; the sheets swap the gates' letters and pin numbers")
                    .clicked()
                {
                    if let (Some(a), Some(b)) = (self.ga, self.gb) {
                        act = Some(SwapAct::Gates((gs[a].0, gs[a].1), (gs[b].0, gs[b].1)));
                    }
                }
            });
        self.open = open;
        act
    }
}

/// Sorts pad names as people read them: 2 before 10, A2 before A10.
fn natural(s: &str) -> (String, u64, String) {
    let head: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &s[head.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let tail = rest[digits.len()..].to_string();
    (head, digits.parse().unwrap_or(0), tail)
}
