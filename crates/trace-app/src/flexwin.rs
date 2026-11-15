//! The Rigid-flex window: zones with their own layers, bend areas, and
//! the flex checks (IPC-2223).

use eframe::egui::{self, Color32, RichText};
use trace_core::edit::Edit;
use trace_core::flex::{self, Bend, Issue, Limits, Zone};
use trace_core::{Board, Length, Point};

pub struct FlexWindow {
    pub open: bool,
    /// Draw zones, bends and issues on the board.
    pub overlay: bool,
    zones: Vec<Zone>,
    bends: Vec<Bend>,
    /// The board's zones and bends the draft was taken from.
    taken: Option<(Vec<Zone>, Vec<Bend>)>,
    /// Box for a new zone or bend, mil.
    new_box: [f64; 4],
    lim: Limits,
    pub issues: Vec<Issue>,
    checked: bool,
    /// Fold line of each of the board's bends, for drawing.
    pub axes: Vec<Option<[f64; 2]>>,
}

impl Default for FlexWindow {
    fn default() -> Self {
        FlexWindow {
            open: false,
            overlay: true,
            zones: Vec::new(),
            bends: Vec::new(),
            taken: None,
            new_box: [0.0; 4],
            lim: Limits::default(),
            issues: Vec::new(),
            checked: false,
            axes: Vec::new(),
        }
    }
}

/// What the window asks the app to do.
pub enum FlexAct {
    Edit(String, Vec<Edit>),
    Show([f64; 2], String),
}

fn bbox(r: &[Point]) -> [f64; 4] {
    r.iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            let (x, y) = (p.x.mil(), p.y.mil());
            [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
        })
}

fn mil_value(ui: &mut egui::Ui, v: &mut f64) -> bool {
    ui.add(egui::DragValue::new(v).speed(1.0).suffix(" mil"))
        .changed()
}

impl FlexWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<FlexAct> {
        if !self.open {
            return None;
        }
        // Take the board's zones afresh when they changed under us (undo,
        // another file) and nothing is being edited here.
        let now = (board.zones.clone(), board.bends.clone());
        if self.taken.as_ref() != Some(&now) {
            let edited = self
                .taken
                .as_ref()
                .is_some_and(|t| t.0 != self.zones || t.1 != self.bends);
            if !edited {
                self.zones = now.0.clone();
                self.bends = now.1.clone();
                self.issues.clear();
                self.checked = false;
            }
            if self.taken.is_none() {
                let b = bbox(&board.geometry.outline);
                if b[0] <= b[2] {
                    let (cx, w) = ((b[0] + b[2]) / 2.0, (b[2] - b[0]) / 6.0);
                    self.new_box = [cx - w, b[1], cx + w, b[3]];
                }
            }
            self.axes = board
                .bends
                .iter()
                .map(|b| flex::fold_axis(board, b))
                .collect();
            self.taken = Some(now);
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Rigid-flex")
            .open(&mut open)
            .default_width(560.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Zones are areas with only some of the stack-up's layers: a flex zone keeps its \
                         polyimide and copper. Bends sit in flex zones. The check follows IPC-2223.",
                    )
                    .small()
                    .weak(),
                );
                ui.checkbox(&mut self.overlay, "Show zones, bends and issues on the board");
                ui.separator();
                let names: Vec<String> = board.stackup.layers.iter().map(|l| l.name.clone()).collect();
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .id_salt("flex_list")
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Zones ({})", self.zones.len())).strong());
                        let mut drop = None;
                        for (i, z) in self.zones.iter_mut().enumerate() {
                            let (t, n) = flex::zone_thickness(board, z);
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut z.name).desired_width(110.0));
                                ui.checkbox(&mut z.flex, "flex");
                                ui.checkbox(&mut z.stiffener, "stiffened")
                                    .on_hover_text("Parts may sit on it although it is flex");
                                ui.label(format!("{t:.1} mil, {n} Cu"));
                                if ui.small_button("Show").clicked() {
                                    out = Some(FlexAct::Show(
                                        flex::centroid(&z.outline),
                                        format!("Zone {}", z.name),
                                    ));
                                }
                                if ui.small_button("Delete").clicked() {
                                    drop = Some(i);
                                }
                            });
                            egui::CollapsingHeader::new(format!("Layers in {}", z.name))
                                .id_salt(("zl", i))
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for name in &names {
                                            let mut on = z.layers.contains(name);
                                            if ui.checkbox(&mut on, name).changed() {
                                                if on {
                                                    // Keep the stack-up's order.
                                                    z.layers.push(name.clone());
                                                    z.layers.sort_by_key(|l| {
                                                        names.iter().position(|n| n == l)
                                                    });
                                                } else {
                                                    z.layers.retain(|l| l != name);
                                                }
                                            }
                                        }
                                    });
                                });
                        }
                        if let Some(i) = drop {
                            self.zones.remove(i);
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("Bends ({})", self.bends.len())).strong());
                        let mut drop = None;
                        for (i, b) in self.bends.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(70.0));
                                ui.label("radius");
                                let mut r = b.radius.mil();
                                if mil_value(ui, &mut r) {
                                    b.radius = Length::from_mil(r.max(0.0));
                                }
                                ui.label("angle");
                                ui.add(
                                    egui::DragValue::new(&mut b.angle_deg)
                                        .speed(1.0)
                                        .range(0.0..=360.0)
                                        .suffix("°"),
                                );
                                ui.checkbox(&mut b.dynamic, "dynamic")
                                    .on_hover_text("Flexes in use (hinge, cable): needs 100x the thickness");
                                let mut fixed = b.axis_deg.is_some();
                                if ui
                                    .checkbox(&mut fixed, "fold at")
                                    .on_hover_text("Off: the fold line is square to the tracks crossing the bend")
                                    .changed()
                                {
                                    b.axis_deg = fixed.then_some(0.0);
                                }
                                if let Some(a) = b.axis_deg.as_mut() {
                                    ui.add(egui::DragValue::new(a).speed(1.0).range(-180.0..=180.0).suffix("°"));
                                }
                                if ui.small_button("Show").clicked() {
                                    out = Some(FlexAct::Show(
                                        flex::centroid(&b.area),
                                        format!("Bend {}", b.name),
                                    ));
                                }
                                if ui.small_button("Delete").clicked() {
                                    drop = Some(i);
                                }
                            });
                            if let Some(z) = flex::zone_at(board, flex::centroid(&b.area)).and_then(|z| board.zones.get(z)) {
                                let (t, n) = flex::zone_thickness(board, z);
                                let need = flex::min_radius(t, n, b.dynamic);
                                let ok = b.radius.mil() >= need;
                                ui.label(
                                    RichText::new(format!(
                                        "    in {}: needs at least {need:.0} mil radius{}",
                                        z.name,
                                        if ok { "" } else { " (too tight)" }
                                    ))
                                    .small()
                                    .color(if ok { Color32::GRAY } else { Color32::LIGHT_RED }),
                                );
                            }
                        }
                        if let Some(i) = drop {
                            self.bends.remove(i);
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Box");
                    for v in self.new_box.iter_mut() {
                        mil_value(ui, v);
                    }
                })
                .response
                .on_hover_text("Left, bottom, right, top of a new zone or bend, mil");
                ui.horizontal(|ui| {
                    let [x0, y0, x1, y1] = self.new_box;
                    let good = x1 > x0 && y1 > y0;
                    if ui
                        .add_enabled(!board.geometry.outline.is_empty(), egui::Button::new("Add rigid zone (whole board)"))
                        .clicked()
                    {
                        self.zones.push(Zone {
                            name: format!("Rigid {}", self.zones.len() + 1),
                            flex: false,
                            outline: board.geometry.outline.clone(),
                            layers: names.clone(),
                            stiffener: false,
                        });
                    }
                    if ui.add_enabled(good, egui::Button::new("Add flex zone")).clicked() {
                        // Flex keeps the two outer-most copper layers of the
                        // middle of the stack and what lies between them.
                        let cu: Vec<usize> = board
                            .stackup
                            .layers
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| l.kind == trace_core::LayerKind::Copper)
                            .map(|(i, _)| i)
                            .collect();
                        let layers = if cu.len() >= 2 {
                            let mid = cu.len() / 2;
                            let (a, b) = (cu[mid - 1], cu[mid]);
                            names[a..=b].to_vec()
                        } else {
                            names.clone()
                        };
                        self.zones.push(Zone {
                            name: format!("Flex {}", self.zones.len() + 1),
                            flex: true,
                            outline: flex::rect(x0, y0, x1, y1),
                            layers,
                            stiffener: false,
                        });
                    }
                    if ui.add_enabled(good, egui::Button::new("Add bend")).clicked() {
                        self.bends.push(Bend {
                            name: format!("B{}", self.bends.len() + 1),
                            area: flex::rect(x0, y0, x1, y1),
                            radius: Length::from_mil(100.0),
                            angle_deg: 90.0,
                            dynamic: false,
                            axis_deg: None,
                        });
                    }
                });
                let taken = self.taken.clone().unwrap_or_default();
                let edited = taken.0 != self.zones || taken.1 != self.bends;
                ui.horizontal(|ui| {
                    if ui.add_enabled(edited, egui::Button::new("Apply to the board")).clicked() {
                        out = Some(FlexAct::Edit(
                            "Changed the rigid-flex zones".into(),
                            vec![Edit::Flex {
                                before: Box::new(taken.clone()),
                                after: Box::new((self.zones.clone(), self.bends.clone())),
                            }],
                        ));
                    }
                    if ui.add_enabled(edited, egui::Button::new("Revert")).clicked() {
                        self.zones = taken.0.clone();
                        self.bends = taken.1.clone();
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Keep vias from rigid edges");
                    mil_value(ui, &mut self.lim.edge_keepout);
                    ui.label("tracks across bends within");
                    ui.add(
                        egui::DragValue::new(&mut self.lim.cross_angle)
                            .speed(0.5)
                            .range(0.0..=45.0)
                            .suffix("°"),
                    );
                    if ui
                        .add_enabled(!board.zones.is_empty(), egui::Button::new("Check"))
                        .on_hover_text("Checks the zones and bends on the board (apply edits first)")
                        .clicked()
                    {
                        self.issues = flex::check(board, &self.lim);
                        self.checked = true;
                    }
                });
                if self.checked {
                    if self.issues.is_empty() {
                        ui.label(RichText::new("No rigid-flex issues.").strong().color(Color32::from_rgb(90, 200, 120)));
                    } else {
                        let mut kinds: Vec<(&str, usize)> = Vec::new();
                        for i in &self.issues {
                            match kinds.iter_mut().find(|k| k.0 == i.kind) {
                                Some(k) => k.1 += 1,
                                None => kinds.push((i.kind, 1)),
                            }
                        }
                        ui.label(
                            RichText::new(format!(
                                "{} issues: {}",
                                self.issues.len(),
                                kinds
                                    .iter()
                                    .map(|(k, n)| format!("{n} {k}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                            .strong(),
                        );
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .id_salt("flex_issues")
                            .show(ui, |ui| {
                                for i in self.issues.iter().take(1000) {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(i.kind).color(Color32::from_rgb(230, 180, 70)));
                                        if ui.link(&i.message).clicked() {
                                            out = Some(FlexAct::Show(i.at, i.message.clone()));
                                        }
                                    });
                                }
                            });
                    }
                    if ui.button("Save report…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("txt", &["txt"])
                            .set_file_name("rigid-flex.txt")
                            .save_file()
                        {
                            let _ = std::fs::write(&path, flex::report(board, &self.issues));
                        }
                    }
                }
            });
        self.open = open;
        out
    }
}
