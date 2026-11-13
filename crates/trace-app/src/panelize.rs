//! The Panelize window: how many copies, how they come apart, rails.
//! Making the panel opens it in place of the board, unsaved.

use eframe::egui;
use trace_core::panel::{PanelSpec, Separation};

pub struct PanelWindow {
    pub open: bool,
    cols: u32,
    rows: u32,
    vscore: bool,
    gap_mm: f64,
    rail_mm: f64,
    /// 0 picks one per two inches of edge.
    tabs: u32,
    tab_mm: f64,
    fiducials: bool,
    tooling: bool,
}

impl Default for PanelWindow {
    fn default() -> Self {
        PanelWindow {
            open: false,
            cols: 2,
            rows: 2,
            vscore: false,
            gap_mm: 2.0,
            rail_mm: 5.0,
            tabs: 0,
            tab_mm: 5.0,
            fiducials: true,
            tooling: true,
        }
    }
}

impl PanelWindow {
    /// Shows the window; returns the panel to make when the button is
    /// pressed. `board_mm` is the board's size, for the panel size shown.
    pub fn show(&mut self, ctx: &egui::Context, board_mm: Option<[f64; 2]>) -> Option<PanelSpec> {
        let mut make = None;
        let mut open = self.open;
        egui::Window::new("Panelize")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("panel").num_columns(2).show(ui, |ui| {
                    ui.label("Columns × rows");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.cols).range(1..=20));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.rows).range(1..=20));
                    });
                    ui.end_row();
                    ui.label("Separate by");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.vscore, false, "Tabs with mouse bites");
                        ui.radio_value(&mut self.vscore, true, "V-score");
                    });
                    ui.end_row();
                    if !self.vscore {
                        ui.label("Milled gap");
                        ui.add(
                            egui::DragValue::new(&mut self.gap_mm)
                                .range(0.5..=10.0)
                                .speed(0.1)
                                .suffix(" mm"),
                        );
                        ui.end_row();
                        ui.label("Tabs per edge");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.tabs).range(0..=8));
                            ui.weak(if self.tabs == 0 { "auto: one per 2 in" } else { "" });
                        });
                        ui.end_row();
                        ui.label("Tab width");
                        ui.add(
                            egui::DragValue::new(&mut self.tab_mm)
                                .range(1.0..=20.0)
                                .speed(0.1)
                                .suffix(" mm"),
                        );
                        ui.end_row();
                    }
                    ui.label("Rails (top and bottom)");
                    ui.add(
                        egui::DragValue::new(&mut self.rail_mm)
                            .range(0.0..=20.0)
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                    ui.end_row();
                    ui.label("On the rails");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.fiducials, "Fiducials");
                        ui.checkbox(&mut self.tooling, "Tooling holes");
                    });
                    ui.end_row();
                });
                let spec = self.spec();
                if let Some([w, h]) = board_mm {
                    let gap = if self.vscore { 0.0 } else { self.gap_mm };
                    let rail_gap = if spec.rail > 0.0 { gap } else { 0.0 };
                    let pw = self.cols as f64 * w + (self.cols - 1) as f64 * gap;
                    let ph = self.rows as f64 * h
                        + (self.rows - 1) as f64 * gap
                        + 2.0 * (self.rail_mm + rail_gap);
                    ui.label(format!("Panel {pw:.1} × {ph:.1} mm"));
                }
                ui.weak("The panel opens in place of the board; save it, then export fab outputs or ODB++.");
                if ui.button("Make panel").clicked() {
                    make = Some(spec);
                }
            });
        self.open = open && make.is_none();
        make
    }

    fn spec(&self) -> PanelSpec {
        let mil = |mm: f64| mm / 0.0254;
        PanelSpec {
            cols: self.cols,
            rows: self.rows,
            gap: mil(self.gap_mm),
            rail: mil(self.rail_mm),
            separation: if self.vscore {
                Separation::VScore
            } else {
                Separation::MouseBites
            },
            tabs: self.tabs,
            tab_width: mil(self.tab_mm),
            fiducials: self.fiducials,
            tooling: self.tooling,
            ..Default::default()
        }
    }
}
