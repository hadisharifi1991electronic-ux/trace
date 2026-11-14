//! The Test points window: one probe point per net for bed-of-nails or
//! flying-probe test, and the nets left without one.

use eframe::egui::{self, RichText};
use trace_core::edit::Edit;
use trace_core::testpoint::{self, Sides, TestGoal, TestReport};
use trace_core::{Board, Length};

#[derive(Default)]
pub struct TestWindow {
    pub open: bool,
    goal: TestGoal,
    last: Option<TestReport>,
}

/// What the window asks the app to do.
pub enum TestAct {
    Edit(String, Vec<Edit>),
    ShowNet(u32),
    Report(String),
}

fn mm_value(ui: &mut egui::Ui, label: &str, v: &mut Length, hint: &str) {
    let mut x = v.mm();
    ui.label(label);
    if ui
        .add(
            egui::DragValue::new(&mut x)
                .speed(0.01)
                .range(0.0..=20.0)
                .suffix(" mm"),
        )
        .on_hover_text(hint)
        .changed()
    {
        *v = Length::from_mm(x);
    }
}

impl TestWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<TestAct> {
        if !self.open {
            return None;
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Test points")
            .open(&mut open)
            .default_width(440.0)
            .show(ctx, |ui| {
                let marked = testpoint::existing(board);
                ui.label(format!("{} test points marked on the board.", marked.len()));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Probe from");
                    ui.radio_value(&mut self.goal.sides, Sides::Bottom, "bottom");
                    ui.radio_value(&mut self.goal.sides, Sides::Top, "top");
                    ui.radio_value(&mut self.goal.sides, Sides::Both, "both sides");
                });
                ui.horizontal(|ui| {
                    ui.label("Use");
                    ui.checkbox(&mut self.goal.vias, "vias");
                    ui.checkbox(&mut self.goal.th_pads, "through-hole pads");
                    ui.checkbox(&mut self.goal.smd_pads, "surface pads")
                        .on_hover_text("Pads of test point parts (TP…) are always used");
                });
                egui::Grid::new("tp_sizes").num_columns(4).show(ui, |ui| {
                    mm_value(
                        ui,
                        "Smallest land",
                        &mut self.goal.min_size,
                        "Pad or via a probe can hit",
                    );
                    mm_value(
                        ui,
                        "Probe spacing",
                        &mut self.goal.spacing,
                        "Closest two probes on one side, centre to centre",
                    );
                    ui.end_row();
                    mm_value(
                        ui,
                        "From parts",
                        &mut self.goal.part_clearance,
                        "Room between a probe point and a part body on its side",
                    );
                    mm_value(
                        ui,
                        "From the edge",
                        &mut self.goal.edge,
                        "Room from the board edge",
                    );
                    ui.end_row();
                });
                ui.checkbox(&mut self.goal.keep, "Keep the test points already marked");
                ui.checkbox(
                    &mut self.goal.tented,
                    "Probe vias under solder mask (opens the mask)",
                );
                ui.horizontal(|ui| {
                    if ui.button("Assign test points").clicked() {
                        let r = testpoint::assign(board, &self.goal);
                        if let Some(e) = testpoint::mark(board, &r.points) {
                            out = Some(TestAct::Edit(
                                format!(
                                    "Test points on {} of {} nets",
                                    r.nets - r.missing.len(),
                                    r.nets
                                ),
                                vec![e],
                            ));
                        }
                        self.last = Some(r);
                    }
                    if ui
                        .add_enabled(!marked.is_empty(), egui::Button::new("Clear all"))
                        .clicked()
                    {
                        if let Some(e) = testpoint::mark(board, &[]) {
                            out = Some(TestAct::Edit("Cleared the test points".into(), vec![e]));
                        }
                        self.last = None;
                    }
                    if ui
                        .add_enabled(!marked.is_empty(), egui::Button::new("Save report…"))
                        .on_hover_text("CSV: net, test point, side, place and size")
                        .clicked()
                    {
                        let missing = self
                            .last
                            .as_ref()
                            .map(|r| r.missing.clone())
                            .unwrap_or_default();
                        out = Some(TestAct::Report(testpoint::report_csv(
                            board, &marked, &missing,
                        )));
                    }
                });
                if let Some(r) = &self.last {
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} of {} nets have a probe point ({:.1} %)",
                            r.nets - r.missing.len(),
                            r.nets,
                            r.coverage()
                        ))
                        .strong(),
                    );
                    if !r.missing.is_empty() {
                        ui.label("Nets without one (click to show):");
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .show(ui, |ui| {
                                for (net, why) in &r.missing {
                                    let name = board
                                        .nets
                                        .get(*net as usize)
                                        .map_or("?", |n| n.name.as_str());
                                    if ui.link(format!("{name}: {why}")).clicked() {
                                        out = Some(TestAct::ShowNet(*net));
                                    }
                                }
                            });
                    }
                }
            });
        self.open = open;
        out
    }
}
