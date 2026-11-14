//! The schematic's electrical rule check and annotation windows, and the
//! sheet side of pin and gate swaps made on the board.

use super::Schematics;
use crate::canvas::View;
use eframe::egui::{self, Color32, RichText, Stroke};
use trace_core::annotate::{self, AnnotateGoal, Order};
use trace_core::erc::{self, Issue, Level, Rule};
use trace_core::swap;
use trace_core::Sheet;

#[derive(Default)]
pub struct ErcState {
    pub open: bool,
    pub issues: Vec<Issue>,
    /// The sheet slot each issue's sheet index stands for.
    slots: Vec<usize>,
    pub ran: bool,
    /// The sheets changed since the check ran.
    pub stale: bool,
    pub warnings: bool,
    pub picked: Option<usize>,
    pub annotate_open: bool,
    pub goal: AnnotateGoal,
    preview: Option<Vec<(usize, annotate::Renamed)>>,
}

const ERR: Color32 = Color32::from_rgb(235, 80, 70);
const WARN: Color32 = Color32::from_rgb(235, 180, 60);

impl Schematics {
    /// The sheets read so far, with their slot indices.
    pub fn sheets(&self) -> (Vec<usize>, Vec<&Sheet>) {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.sheet.as_ref().map(|sh| (i, sh)))
            .unzip()
    }

    pub(super) fn run_erc(&mut self) {
        let (slots, sheets) = self.sheets();
        let issues = erc::check(&sheets);
        self.note = Some(format!("Electrical rule check: {}", erc::summary(&issues)));
        if !self.erc.ran {
            self.erc.warnings = true;
        }
        self.erc.issues = issues;
        self.erc.slots = slots;
        self.erc.ran = true;
        self.erc.stale = false;
        self.erc.open = true;
        self.erc.picked = None;
    }

    fn go_to_issue(&mut self, k: usize) {
        let Some(i) = self.erc.issues.get(k) else {
            return;
        };
        let Some(&slot) = self.erc.slots.get(i.sheet) else {
            return;
        };
        let (x, y) = (i.at[0] as f32, i.at[1] as f32);
        self.select_sheet(slot);
        self.needs_fit = false;
        self.focus = Some([x - 600.0, y - 400.0, x + 600.0, y + 400.0]);
        self.erc.picked = Some(k);
    }

    pub(super) fn erc_window(&mut self, ctx: &egui::Context) {
        if !self.erc.open {
            return;
        }
        let mut open = true;
        let mut go = None;
        let mut rerun = false;
        egui::Window::new("Electrical rule check")
            .default_width(520.0)
            .default_height(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let e = self
                    .erc
                    .issues
                    .iter()
                    .filter(|i| i.level == Level::Error)
                    .count();
                let w = self.erc.issues.len() - e;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{e} errors")).color(ERR));
                    ui.label(RichText::new(format!("{w} warnings")).color(WARN));
                    ui.checkbox(&mut self.erc.warnings, "show warnings");
                    if ui.button("Check again").clicked() {
                        rerun = true;
                    }
                });
                if self.erc.stale {
                    ui.weak("The sheets changed since this check.");
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for rule in Rule::ALL {
                        let list: Vec<usize> = self
                            .erc
                            .issues
                            .iter()
                            .enumerate()
                            .filter(|(_, i)| i.rule == rule)
                            .filter(|(_, i)| self.erc.warnings || i.level == Level::Error)
                            .map(|(k, _)| k)
                            .collect();
                        if list.is_empty() {
                            continue;
                        }
                        let colour = if rule.level() == Level::Error {
                            ERR
                        } else {
                            WARN
                        };
                        egui::CollapsingHeader::new(
                            RichText::new(format!("{} ({})", rule.label(), list.len()))
                                .color(colour),
                        )
                        .id_salt(rule.label())
                        .default_open(list.len() <= 30)
                        .show(ui, |ui| {
                            for k in list.into_iter().take(500) {
                                let i = &self.erc.issues[k];
                                let sheet = self
                                    .erc
                                    .slots
                                    .get(i.sheet)
                                    .and_then(|&s| self.slots.get(s))
                                    .map_or("", |s| s.name.as_str());
                                if ui
                                    .selectable_label(
                                        self.erc.picked == Some(k),
                                        format!("{sheet}: {}", i.text),
                                    )
                                    .on_hover_text("Show it on the sheet")
                                    .clicked()
                                {
                                    go = Some(k);
                                }
                            }
                        });
                    }
                    if self.erc.issues.is_empty() {
                        ui.label("No errors or warnings.");
                    }
                });
            });
        self.erc.open = open;
        if rerun {
            self.run_erc();
        }
        if let Some(k) = go {
            self.go_to_issue(k);
        }
    }

    /// Rings round the issues on the sheet on view.
    pub(super) fn draw_erc(&self, painter: &egui::Painter, view: View) {
        if !self.erc.open {
            return;
        }
        for (k, i) in self.erc.issues.iter().enumerate() {
            if self.erc.slots.get(i.sheet) != Some(&self.current) {
                continue;
            }
            if i.level == Level::Warning && !self.erc.warnings {
                continue;
            }
            let c = if i.level == Level::Error { ERR } else { WARN };
            let at = view.to_screen(i.at[0] as f32, i.at[1] as f32);
            let r = if self.erc.picked == Some(k) {
                12.0
            } else {
                7.0
            };
            painter.circle_stroke(at, r, Stroke::new(2.0_f32, c));
        }
    }

    pub(super) fn annotate_window(&mut self, ctx: &egui::Context) {
        if !self.erc.annotate_open {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut preview = false;
        egui::Window::new("Annotate")
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let g = &mut self.erc.goal;
                let before = (g.order, g.all, g.start, g.sheet_step);
                ui.horizontal(|ui| {
                    ui.label("Order");
                    ui.radio_value(&mut g.order, Order::AcrossThenDown, "across, then down");
                    ui.radio_value(&mut g.order, Order::DownThenAcross, "down, then across");
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut g.all, false, "Only parts without a number (R?)");
                    ui.radio_value(&mut g.all, true, "Every part again");
                });
                ui.horizontal(|ui| {
                    ui.label("Start at");
                    ui.add(egui::DragValue::new(&mut g.start).range(0..=100000));
                    let mut per = g.sheet_step.is_some();
                    ui.checkbox(&mut per, "a block per sheet of")
                        .on_hover_text("The first sheet's parts start at the start number, the second's that much higher, and so on");
                    let mut step = g.sheet_step.unwrap_or(100);
                    ui.add_enabled(per, egui::DragValue::new(&mut step).range(10..=100000));
                    g.sheet_step = per.then_some(step);
                });
                if before != (g.order, g.all, g.start, g.sheet_step) {
                    self.erc.preview = None;
                }
                ui.weak(
                    "Gates of one part share its number; new gates fill a part of the same kind \
                     on the same sheet before starting another. Renamed parts are carried to the \
                     board by Update PCB.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Preview").clicked() {
                        preview = true;
                    }
                    let n = self.erc.preview.as_ref().map(|p| p.len());
                    if ui
                        .add_enabled(n.is_some_and(|n| n > 0), egui::Button::new("Apply"))
                        .clicked()
                    {
                        apply = true;
                    }
                });
                if let Some(p) = &self.erc.preview {
                    if p.is_empty() {
                        ui.label("Nothing to rename.");
                    } else {
                        ui.label(format!("{} parts get a new designator:", p.len()));
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for (slot, r) in p.iter().take(400) {
                                let sheet = self.slots.get(*slot).map_or("", |s| s.name.as_str());
                                ui.label(format!("{sheet}: {} → {}", r.from, r.to));
                            }
                        });
                    }
                }
            });
        self.erc.annotate_open = open;
        if preview || apply {
            let (slots, sheets) = self.sheets();
            let r = annotate::annotate(&sheets, &self.erc.goal);
            self.erc.preview = Some(r.iter().map(|x| (slots[x.sheet], x.clone())).collect());
        }
        if apply {
            let n = self.apply_annotation();
            self.note = Some(format!("Annotated: {n} parts renamed."));
            self.erc.preview = None;
            self.erc.annotate_open = false;
        }
    }

    /// Applies the previewed renames, as one undo step.
    fn apply_annotation(&mut self) -> usize {
        let Some(p) = self.erc.preview.clone() else {
            return 0;
        };
        if p.is_empty() {
            return 0;
        }
        let mut group = Vec::new();
        let mut touched: Vec<usize> = p.iter().map(|(s, _)| *s).collect();
        touched.sort_unstable();
        touched.dedup();
        for &slot in &touched {
            if let Some(sh) = self.slots[slot].sheet.as_ref() {
                group.push((slot, sh.clone()));
            }
        }
        self.undo.push(group);
        self.redo.clear();
        for (slot, r) in &p {
            if let Some(sh) = self.slots[*slot].sheet.as_mut() {
                let one = annotate::Renamed {
                    sheet: 0,
                    ..r.clone()
                };
                annotate::apply(&mut [sh], &[one]);
            }
        }
        // Numbered parts renamed: now -> as on the board.
        let renamed =
            annotate::component_renames(&p.iter().map(|x| x.1.clone()).collect::<Vec<_>>());
        for map in [&mut self.renames, &mut self.original_renames] {
            let was: Vec<(String, String)> = renamed
                .iter()
                .map(|(from, to)| {
                    let up = from.to_ascii_uppercase();
                    (to.to_ascii_uppercase(), map.get(&up).cloned().unwrap_or(up))
                })
                .collect();
            for from in renamed.keys() {
                map.remove(&from.to_ascii_uppercase());
            }
            map.extend(was);
        }
        for slot in touched {
            self.after_edit(slot);
        }
        self.dirty = true;
        self.erc.stale = true;
        p.len()
    }

    /// The gates drawn for a designator: (slot, part, gate).
    pub fn gates(&self, designator: &str) -> Vec<(usize, usize, u32)> {
        let (slots, sheets) = self.sheets();
        swap::gates_of(&sheets, designator)
            .into_iter()
            .map(|(s, p, g)| (slots[s], p, g))
            .collect()
    }

    /// The board's pad pairs for swapping two gates of one part.
    pub fn gate_pads(
        &self,
        board: &trace_core::Board,
        a: (usize, usize),
        b: (usize, usize),
    ) -> Result<Vec<(swap::PadRef, swap::PadRef)>, String> {
        let (slots, sheets) = self.sheets();
        let idx = |s: usize| slots.iter().position(|&x| x == s).ok_or("sheet not read");
        swap::gate_pads(board, &sheets, (idx(a.0)?, a.1), (idx(b.0)?, b.1))
    }

    /// Trades pin numbers on the sheets after the board swapped them: each
    /// pair of part `designator`, one undo step. The sheets as the board
    /// last matched them change too, so Update PCB sees nothing to undo.
    pub fn swap_numbers(&mut self, designator: &str, pairs: &[(String, String)]) -> usize {
        let mut group = Vec::new();
        let mut n = 0;
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(sh) = slot.sheet.as_mut() else {
                continue;
            };
            let before = sh.clone();
            let k = swap::swap_numbers(&mut sh.drawing, designator, pairs);
            if k > 0 {
                group.push((i, before));
                n += k;
                if let Some(Some(b)) = self.baseline.get_mut(i) {
                    swap::swap_numbers(b, designator, pairs);
                }
            }
        }
        if n > 0 {
            let touched: Vec<usize> = group.iter().map(|g| g.0).collect();
            self.undo.push(group);
            self.redo.clear();
            for i in touched {
                self.after_edit(i);
            }
            self.dirty = true;
            self.erc.stale = true;
        }
        n
    }

    /// Trades two drawn gates of one part (letters and pin numbers).
    pub fn swap_gates(&mut self, a: (usize, usize), b: (usize, usize)) -> Result<(), String> {
        let pairs: Vec<(String, String)> = {
            let (slots, sheets) = self.sheets();
            let idx = |s: usize| slots.iter().position(|&x| x == s).ok_or("sheet not read");
            let (ia, ib) = ((idx(a.0)?, a.1), (idx(b.0)?, b.1));
            swap::match_gates(&sheets, ia, ib)?
                .into_iter()
                .map(|(x, y)| {
                    (
                        sheets[ia.0].drawing.pins[x].number.clone(),
                        sheets[ib.0].drawing.pins[y].number.clone(),
                    )
                })
                .collect()
        };
        let designator = self.slots[a.0]
            .sheet
            .as_ref()
            .map(|s| s.drawing.parts[a.1].designator.clone())
            .unwrap_or_default();
        let mut group = vec![];
        let mut seen = vec![];
        for s in [a.0, b.0] {
            if !seen.contains(&s) {
                seen.push(s);
                if let Some(sh) = self.slots[s].sheet.as_ref() {
                    group.push((s, sh.clone()));
                }
            }
        }
        if a.0 == b.0 {
            let sh = self.slots[a.0].sheet.as_mut().ok_or("sheet not read")?;
            swap::sheet_swap_gates(&mut [sh], (0, a.1), (0, b.1))?;
        } else {
            let (lo, hi) = (a.0.min(b.0), a.0.max(b.0));
            let (x, y) = self.slots.split_at_mut(hi);
            let sx = x[lo].sheet.as_mut().ok_or("sheet not read")?;
            let sy = y[0].sheet.as_mut().ok_or("sheet not read")?;
            let (ga, gb) = if a.0 < b.0 {
                ((0, a.1), (1, b.1))
            } else {
                ((1, a.1), (0, b.1))
            };
            swap::sheet_swap_gates(&mut [sx, sy], ga, gb)?;
        }
        for (i, b) in self.baseline.iter_mut().enumerate() {
            if let Some(b) = b {
                if seen.contains(&i) {
                    swap::swap_numbers(b, &designator, &pairs);
                }
            }
        }
        self.undo.push(group);
        self.redo.clear();
        for i in seen {
            self.after_edit(i);
        }
        self.dirty = true;
        self.erc.stale = true;
        Ok(())
    }
}
