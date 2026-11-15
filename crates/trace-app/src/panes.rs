//! What each docked panel shows: Projects, PCB, Layers, Properties and
//! Messages, in the manner of Altium Designer's panels. Navigator is the
//! side panel Trace always had.

use crate::scene::{Slot, Source};
use crate::studio::Pane;
use crate::toolbar::{self, Icon};
use crate::{App, Tool};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use std::collections::HashMap;
use trace_core::edit::Edit;
use trace_core::geometry::LayerRef;
use trace_core::place::Placement;
use trace_core::{Length, Side};

/// The PCB panel's views.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PcbView {
    #[default]
    Nets,
    Components,
    Classes,
    Pairs,
}

/// A Find Similar attribute: name, value shown, query term, ticked.
pub type Attr = (&'static str, String, String, bool);

/// State of the PCB and Properties panels.
#[derive(Default)]
pub struct PaneState {
    pub pcb_view: PcbView,
    pub filter: String,
    /// Typed values of the Properties fields, by field, until committed;
    /// cleared when the selection changes.
    fields: HashMap<&'static str, String>,
    fields_for: String,
    /// PCB Filter: the query, its error or result, recent queries.
    pub query: String,
    query_err: Option<String>,
    query_res: Option<String>,
    no_zoom: bool,
    history: Vec<String>,
    /// Selection memory, Ctrl+1..8 stores and Alt+1..8 recalls.
    pub memory: [Option<trace_core::select::Sel>; 8],
    /// Find Similar Objects: the object and each attribute, ticked to match.
    pub similar: Option<(trace_core::query::Obj, Vec<Attr>)>,
}

/// Example queries for the PCB Filter panel: (label, query).
const EXAMPLES: &[(&str, &str)] = &[
    ("Vias with small holes", "IsVia And HoleSize < 8mil"),
    ("Thin tracks", "IsTrack And Width < 5mil"),
    ("Ground copper", "InNet('GND*')"),
    ("Parts on the bottom", "IsComponent And OnBottom"),
    ("Capacitors", "IsComponent And Designator Like 'C*'"),
    ("Tracks without a net", "IsTrack And Not HasNet"),
    ("Pair copper", "InDifferentialPair('*')"),
];

/// A row with a small icon and a selectable name.
fn icon_row(ui: &mut egui::Ui, icon: Icon, text: &str, on: bool) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        toolbar::draw(ui.painter(), icon, r, Color32::from_gray(214), true);
        ui.selectable_label(on, text)
    })
    .inner
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text)
            .small()
            .strong()
            .color(crate::studio::ACCENT),
    );
}

fn row(ui: &mut egui::Ui, k: &str, v: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(k).weak());
    ui.label(v);
    ui.end_row();
}

impl App {
    pub(crate) fn pane_ui(&mut self, pane: Pane, ui: &mut egui::Ui) {
        match pane {
            Pane::Projects => self.projects_pane(ui),
            Pane::Navigator => self.side_panel(ui),
            Pane::Pcb => self.pcb_pane(ui),
            Pane::Filter => self.filter_pane(ui),
            Pane::Layers => self.layers_pane(ui),
            Pane::Properties => self.properties_pane(ui),
            Pane::Messages => self.messages_pane(ui),
        }
    }

    fn projects_pane(&mut self, ui: &mut egui::Ui) {
        let board = self.scene.as_ref().map(|s| s.path.clone());
        let project = self
            .schem
            .project
            .clone()
            .or_else(|| board.as_deref().and_then(crate::schview::find_project));
        let title = project
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Free Documents".into());
        if board.is_none() && self.schem.is_empty() && self.fped.is_empty() {
            ui.label(RichText::new("No project open.").weak());
            if ui.button("Open project or document…").clicked() {
                self.open_dialog();
            }
            return;
        }
        let home = self.studio.home;
        egui::CollapsingHeader::new(RichText::new(title).strong())
            .id_salt("proj_root")
            .default_open(true)
            .show(ui, |ui| {
                egui::CollapsingHeader::new("Source Documents")
                    .id_salt("proj_src")
                    .default_open(true)
                    .show(ui, |ui| {
                        if let Some(b) = &board {
                            let name = b
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let on = !home && !self.mode_sch && !self.mode_fp;
                            if icon_row(ui, Icon::Board, &name, on)
                                .on_hover_text(b.display().to_string())
                                .clicked()
                            {
                                self.go_board();
                            }
                        }
                        let current = self.schem.current;
                        let names: Vec<(usize, String)> = self
                            .schem
                            .slots
                            .iter()
                            .enumerate()
                            .map(|(i, s)| (i, s.name.clone()))
                            .collect();
                        for (i, name) in names {
                            let on = !home && self.mode_sch && !self.mode_fp && i == current;
                            if icon_row(ui, Icon::Schematic, &name, on).clicked() {
                                self.go_schematic();
                                self.schem.select_sheet(i);
                            }
                        }
                    });
                if !self.fped.is_empty() {
                    egui::CollapsingHeader::new("Libraries")
                        .id_salt("proj_libs")
                        .default_open(true)
                        .show(ui, |ui| {
                            let label = if self.fped.source.is_empty() {
                                "Footprints".to_string()
                            } else {
                                self.fped.source.clone()
                            };
                            if icon_row(ui, Icon::Footprint, &label, !home && self.mode_fp)
                                .clicked()
                            {
                                self.studio.home = false;
                                self.mode_fp = true;
                            }
                        });
                }
            });
        if let Some((done, total)) = self.schem.loading() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Reading sheets: {done} of {total}"));
            });
        }
    }

    pub(crate) fn layers_pane(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &mut self.scene else {
            ui.label(RichText::new("No board open.").weak());
            return;
        };
        let mut new_active = None;
        for l in scene.layers.iter_mut() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut l.visible, "");
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    3.0,
                    Color32::from_rgb(l.color[0], l.color[1], l.color[2]),
                );
                let active = l.slot == self.active;
                let text = if active {
                    RichText::new(&l.name).strong()
                } else {
                    RichText::new(&l.name)
                };
                let r = ui
                    .selectable_label(active, text)
                    .on_hover_text("Click to make active. Double-click to show only this layer.");
                if r.clicked() && matches!(l.slot, Slot::Copper(_)) {
                    new_active = Some((l.slot, false));
                }
                if r.double_clicked() {
                    new_active = Some((l.slot, true));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(l.count.to_string()).small().weak());
                });
            });
        }
        if let Some((slot, solo)) = new_active {
            if matches!(slot, Slot::Copper(_)) {
                self.active = slot;
            }
            for l in scene.layers.iter_mut() {
                if solo {
                    l.visible = l.slot == slot || l.slot == Slot::Vias || l.slot == Slot::Pours;
                } else if l.slot == slot {
                    l.visible = true;
                }
            }
        }
    }

    fn pcb_pane(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else {
            ui.label(RichText::new("No board open.").weak());
            return;
        };
        let b = &scene.board;
        ui.horizontal(|ui| {
            let v = &mut self.panes.pcb_view;
            ui.selectable_value(v, PcbView::Nets, format!("Nets {}", b.nets.len()));
            ui.selectable_value(
                v,
                PcbView::Components,
                format!("Parts {}", b.components.len()),
            );
        });
        ui.horizontal(|ui| {
            let v = &mut self.panes.pcb_view;
            ui.selectable_value(
                v,
                PcbView::Classes,
                format!(
                    "Classes {}",
                    b.net_classes.len() + b.component_classes.len()
                ),
            );
            ui.selectable_value(v, PcbView::Pairs, format!("Pairs {}", b.diff_pairs.len()));
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.panes.filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY),
        );
        let f = self.panes.filter.to_ascii_lowercase();
        let hit = |s: &str| f.is_empty() || s.to_ascii_lowercase().contains(&f);
        let mut pick_net = None;
        let mut pick_part = None;
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        match self.panes.pcb_view {
            PcbView::Nets => {
                let rows: Vec<usize> = (0..b.nets.len())
                    .filter(|&i| hit(&b.nets[i].name))
                    .collect();
                egui::Grid::new("pcb_nets_head")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Net").small().weak());
                        ui.label(
                            RichText::new(format!(
                                "Length ({})",
                                if self.studio.mm { "mm" } else { "mil" }
                            ))
                            .small()
                            .weak(),
                        );
                    });
                egui::ScrollArea::vertical()
                    .id_salt("pcb_nets")
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, rows.len(), |ui, range| {
                        for &i in &rows[range] {
                            let n = &b.nets[i];
                            ui.horizontal(|ui| {
                                let open = n.pads >= 2 && !n.has_pour && !n.is_complete();
                                let text = if open {
                                    RichText::new(&n.name).color(Color32::from_rgb(255, 180, 84))
                                } else {
                                    RichText::new(&n.name)
                                };
                                if ui
                                    .selectable_label(self.selected == Some(i as u32), text)
                                    .clicked()
                                {
                                    pick_net = Some(i as u32);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let mil = n.total_length().mil();
                                        let v = if self.studio.mm { mil * 0.0254 } else { mil };
                                        ui.label(RichText::new(format!("{v:.1}")).small().weak());
                                    },
                                );
                            });
                        }
                    });
            }
            PcbView::Components => {
                let rows: Vec<usize> = (0..b.components.len())
                    .filter(|&i| {
                        let c = &b.components[i];
                        hit(&c.designator) || hit(&c.footprint)
                    })
                    .collect();
                egui::ScrollArea::vertical()
                    .id_salt("pcb_parts")
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, rows.len(), |ui, range| {
                        for &i in &rows[range] {
                            let c = &b.components[i];
                            ui.horizontal(|ui| {
                                let on = self.edit.sel.parts.len() == 1
                                    && self.edit.sel.parts.contains(&i);
                                if ui.selectable_label(on, &c.designator).clicked() {
                                    pick_part = Some(i);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}",
                                                c.footprint,
                                                if c.side == Side::Top { "top" } else { "bot" }
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    },
                                );
                            });
                        }
                    });
            }
            PcbView::Classes => {
                egui::ScrollArea::vertical()
                    .id_salt("pcb_classes")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (kind, list) in [
                            ("Net classes", &b.net_classes),
                            ("Pair classes", &b.diff_pair_classes),
                            ("Component classes", &b.component_classes),
                        ] {
                            if list.is_empty() {
                                continue;
                            }
                            caption(ui, kind);
                            for c in list.iter().filter(|c| hit(&c.name)) {
                                egui::CollapsingHeader::new(format!(
                                    "{} ({})",
                                    c.name,
                                    c.members.len()
                                ))
                                .id_salt(format!("{kind}{}", c.name))
                                .show(ui, |ui| {
                                    for m in &c.members {
                                        if ui.selectable_label(false, m).clicked() {
                                            if let Some(&n) = scene.net_by_name.get(m) {
                                                pick_net = Some(n);
                                            } else if let Some(i) =
                                                b.components.iter().position(|p| &p.designator == m)
                                            {
                                                pick_part = Some(i);
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    });
            }
            PcbView::Pairs => {
                egui::ScrollArea::vertical()
                    .id_salt("pcb_pairs")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for p in b
                            .diff_pairs
                            .iter()
                            .filter(|p| hit(&p.name) || hit(&p.positive) || hit(&p.negative))
                        {
                            ui.horizontal(|ui| {
                                let pn = scene.net_by_name.get(&p.positive).copied();
                                let on = pn.is_some() && self.selected == pn;
                                if ui.selectable_label(on, &p.name).clicked() {
                                    pick_net = pn;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let skew = match (b.net(&p.positive), b.net(&p.negative)) {
                                            (Some(a), Some(c)) => (a.total_length().mil()
                                                - c.total_length().mil())
                                            .abs(),
                                            _ => 0.0,
                                        };
                                        ui.label(
                                            RichText::new(format!(
                                                "skew {}",
                                                self.studio.len(skew)
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    },
                                );
                            });
                        }
                    });
            }
        }
        if let Some(n) = pick_net {
            self.go_board();
            self.edit.sel.clear();
            self.select_net(n, true);
        }
        if let Some(i) = pick_part {
            let d = self
                .scene
                .as_ref()
                .map(|s| s.board.components[i].designator.clone());
            if let Some(d) = d {
                self.go_board();
                self.select_part(&d, true);
                self.edit.sel.clear();
                self.edit.sel.parts.insert(i);
            }
        }
    }

    fn filter_pane(&mut self, ui: &mut egui::Ui) {
        if self.scene.is_none() {
            ui.label(RichText::new("No board open.").weak());
            return;
        }
        let id = egui::Id::new("pcb_filter_query");
        let enter = ui.memory(|m| m.has_focus(id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        ui.add(
            egui::TextEdit::multiline(&mut self.panes.query)
                .id(id)
                .hint_text("IsTrack And InNet('GND') And Width < 10mil")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );
        let mut run = enter;
        ui.horizontal(|ui| {
            if ui
                .button("Apply")
                .on_hover_text("Select what the query picks (Ctrl+Enter)")
                .clicked()
            {
                run = true;
            }
            if ui.button("Clear").clicked() {
                self.panes.query.clear();
                self.panes.query_err = None;
                self.panes.query_res = None;
                self.edit.sel.clear();
            }
            let mut zoom = !self.panes.no_zoom;
            if ui.checkbox(&mut zoom, "Zoom").changed() {
                self.panes.no_zoom = !zoom;
            }
        });
        if run {
            let q = self.panes.query.clone();
            self.run_query(&q);
        }
        if let Some(e) = &self.panes.query_err {
            ui.label(RichText::new(e).color(Color32::from_rgb(255, 150, 84)));
        } else if let Some(r) = &self.panes.query_res {
            ui.label(RichText::new(r).color(crate::studio::ACCENT));
        }
        caption(ui, "Examples");
        for (label, q) in EXAMPLES {
            if ui.link(*label).on_hover_text(*q).clicked() {
                self.panes.query = q.to_string();
                self.run_query(q);
            }
        }
        if !self.panes.history.is_empty() {
            caption(ui, "Recent");
            for q in self.panes.history.clone().iter().rev() {
                if ui.link(RichText::new(q).monospace().small()).clicked() {
                    self.panes.query = q.clone();
                    self.run_query(q);
                }
            }
        }
        egui::CollapsingHeader::new("Query words")
            .id_salt("query_help")
            .show(ui, |ui| {
                ui.label(RichText::new(
                    "Kinds: IsTrack, IsArc, IsVia, IsComponent\n\
                     Tests: InNet('n*'), InNetClass('c'), InDifferentialPair('p'), OnLayer('Top*'), OnTop, OnBottom, InComponent('U1'), InComponentClass('c'), HasNet\n\
                     Values: Net, Layer, Width, HoleSize, Diameter, Length, X, Y, Designator, Footprint, Value, Rotation, Side\n\
                     Compare: = <> < > <= >= Like ('*' and '?')\n\
                     Combine: And, Or, Not, ( )\n\
                     Lengths: 5mil, 0.2mm, 0.01in (mil if no unit)",
                ).small());
            });
    }

    /// Runs a PCB Filter query: selects what it picks and zooms to it.
    pub(crate) fn run_query(&mut self, q: &str) {
        let Some(scene) = &self.scene else { return };
        match trace_core::query::parse(q) {
            Err(e) => {
                self.panes.query_err = Some(e);
                self.panes.query_res = None;
            }
            Ok(expr) => {
                let sel = trace_core::query::run(&scene.board, &expr);
                let bounds = sel.bounds(&scene.board);
                self.panes.query_err = None;
                let mut parts = Vec::new();
                for (n, k) in [
                    (sel.parts.len(), "parts"),
                    (sel.tracks.len(), "tracks"),
                    (sel.arcs.len(), "arcs"),
                    (sel.vias.len(), "vias"),
                ] {
                    if n > 0 {
                        parts.push(format!("{n} {k}"));
                    }
                }
                self.panes.query_res = Some(if parts.is_empty() {
                    "Nothing matches.".into()
                } else {
                    format!("Selected {}.", parts.join(", "))
                });
                let q = q.trim().to_string();
                self.panes.history.retain(|h| h != &q);
                self.panes.history.push(q);
                if self.panes.history.len() > 10 {
                    self.panes.history.remove(0);
                }
                self.selected = None;
                self.picked = None;
                self.edit.sel = sel;
                self.go_board();
                self.mode3d = false;
                if let (Some(b), false) = (bounds, self.panes.no_zoom) {
                    let m = 0.1 * (b[2] - b[0]).max(b[3] - b[1]).max(50.0);
                    self.focus = Some(crate::Focus::Area([
                        (b[0] - m) as f32,
                        (b[1] - m) as f32,
                        (b[2] + m) as f32,
                        (b[3] + m) as f32,
                    ]));
                    self.needs_fit = false;
                }
            }
        }
    }

    /// The one object selected or last clicked, for Find Similar Objects.
    pub(crate) fn single_object(&self) -> Option<trace_core::query::Obj> {
        use trace_core::query::Obj;
        let s = &self.edit.sel;
        if s.len() == 1 {
            if let Some(&i) = s.parts.iter().next() {
                return Some(Obj::Part(i));
            }
            if let Some(&i) = s.tracks.iter().next() {
                return Some(Obj::Track(i));
            }
            if let Some(&i) = s.arcs.iter().next() {
                return Some(Obj::Arc(i));
            }
            if let Some(&i) = s.vias.iter().next() {
                return Some(Obj::Via(i));
            }
        }
        if s.is_empty() {
            return match self.picked {
                Some(Source::Track(i)) => Some(Obj::Track(i)),
                Some(Source::Arc(i)) => Some(Obj::Arc(i)),
                Some(Source::Via(i)) => Some(Obj::Via(i)),
                _ => None,
            };
        }
        None
    }

    /// Opens Find Similar Objects for the selected object.
    pub(crate) fn find_similar(&mut self) {
        let (Some(o), Some(scene)) = (self.single_object(), &self.scene) else {
            self.message = Some((
                "Select one part, track, arc or via first, then Find Similar Objects (Shift+F)."
                    .into(),
                true,
            ));
            return;
        };
        let attrs = trace_core::query::attributes(&scene.board, o)
            .into_iter()
            .enumerate()
            .map(|(i, (k, v, q))| (k, v, q, i == 0))
            .collect();
        self.panes.similar = Some((o, attrs));
    }

    /// The Find Similar Objects window.
    pub(crate) fn similar_window(&mut self, ctx: &egui::Context) {
        let Some((_, attrs)) = &mut self.panes.similar else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Find Similar Objects")
            .open(&mut open)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Tick what must be the same; the rest may be anything.").weak(),
                );
                egui::Grid::new("similar")
                    .num_columns(3)
                    .striped(true)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for (k, v, _, on) in attrs.iter_mut() {
                            ui.label(RichText::new(*k).weak());
                            ui.label(v.as_str());
                            ui.checkbox(on, "Same");
                            ui.end_row();
                        }
                    });
                ui.add_space(4.0);
                if ui.button("Select matching").clicked() {
                    apply = true;
                }
            });
        if apply {
            let q: Vec<String> = attrs.iter().filter(|a| a.3).map(|a| a.2.clone()).collect();
            let q = if q.is_empty() {
                "All".to_string()
            } else {
                q.join(" And ")
            };
            self.panes.query = q.clone();
            self.panes.similar = None;
            self.run_query(&q);
            self.studio.show(Pane::Filter);
            return;
        }
        if !open {
            self.panes.similar = None;
        }
    }

    /// Ctrl+1..8 stores the selection, Alt+1..8 brings it back.
    pub(crate) fn selection_memory(&mut self, ctx: &egui::Context) {
        use egui::Key::*;
        let keys = [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8];
        for (n, k) in keys.into_iter().enumerate() {
            let (store, recall) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, k),
                    i.consume_key(egui::Modifiers::ALT, k),
                )
            });
            if store {
                if self.edit.sel.is_empty() {
                    self.panes.memory[n] = None;
                    self.message = Some((format!("Memory {} cleared", n + 1), false));
                } else {
                    self.panes.memory[n] = Some(self.edit.sel.clone());
                    self.message = Some((
                        format!(
                            "Stored {} in memory {} (Alt+{} brings it back)",
                            self.edit.sel.describe(),
                            n + 1,
                            n + 1
                        ),
                        false,
                    ));
                }
            }
            if recall {
                match &self.panes.memory[n] {
                    Some(s) => {
                        self.edit.sel = s.clone();
                        if let Some(scene) = &self.scene {
                            self.edit.sel.keep_valid(&scene.board);
                        }
                        self.selected = None;
                        self.message = Some((
                            format!("Memory {}: {}", n + 1, self.edit.sel.describe()),
                            false,
                        ));
                    }
                    None => {
                        self.message = Some((
                            format!(
                                "Memory {} is empty (Ctrl+{} stores the selection)",
                                n + 1,
                                n + 1
                            ),
                            true,
                        ));
                    }
                }
            }
        }
    }

    fn messages_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} messages", self.studio.log.len())).weak());
            if ui.small_button("Clear").clicked() {
                self.studio.log.clear();
            }
        });
        for (text, err) in self.studio.log.iter().rev() {
            if *err {
                ui.label(RichText::new(text).color(Color32::from_rgb(255, 150, 84)));
            } else {
                ui.label(text);
            }
            ui.separator();
        }
    }

    /// The name of a board layer.
    fn layer_name(&self, l: &LayerRef) -> String {
        if let (LayerRef::Copper(i), Some(s)) = (l, &self.scene) {
            let i = *i;
            if let Some(x) = s.layers.iter().find(|x| x.slot == Slot::Copper(i)) {
                return x.name.clone();
            }
        }
        format!("{l:?}")
    }

    /// A length field in the chosen units: shows `mil`, and gives the new
    /// value in mil once typed and committed with Enter or by leaving it.
    fn len_field(&mut self, ui: &mut egui::Ui, key: &'static str, mil: f64) -> Option<f64> {
        let mm = self.studio.mm;
        let shown = if mm {
            format!("{:.4}", mil * 0.0254)
        } else {
            format!("{mil:.2}")
        };
        let buf = self
            .panes
            .fields
            .entry(key)
            .or_insert_with(|| shown.clone());
        let r = ui.add(egui::TextEdit::singleline(buf).desired_width(90.0));
        ui.label(RichText::new(if mm { "mm" } else { "mil" }).weak());
        if r.lost_focus() {
            let text = buf.trim().to_string();
            self.panes.fields.remove(key);
            let v: f64 = text.parse().ok()?;
            let v = if mm { v / 0.0254 } else { v };
            if (v - mil).abs() > 1e-6 && v.is_finite() {
                return Some(v);
            }
        } else if !r.has_focus() && *buf != shown {
            *buf = shown;
        }
        None
    }

    /// A plain number field (degrees).
    fn num_field(
        &mut self,
        ui: &mut egui::Ui,
        key: &'static str,
        v: f64,
        unit: &str,
    ) -> Option<f64> {
        let shown = format!("{v:.2}");
        let buf = self
            .panes
            .fields
            .entry(key)
            .or_insert_with(|| shown.clone());
        let r = ui.add(egui::TextEdit::singleline(buf).desired_width(90.0));
        ui.label(RichText::new(unit).weak());
        if r.lost_focus() {
            let text = buf.trim().to_string();
            self.panes.fields.remove(key);
            let n: f64 = text.parse().ok()?;
            if (n - v).abs() > 1e-9 && n.is_finite() {
                return Some(n);
            }
        } else if !r.has_focus() && *buf != shown {
            *buf = shown;
        }
        None
    }

    /// Whether the Properties panel is the one showing in an open dock.
    pub(crate) fn props_visible(&self) -> bool {
        [&self.studio.left, &self.studio.right].iter().any(|d| {
            d.open
                && d.panes.contains(&Pane::Properties)
                && d.on
                    .filter(|p| d.panes.contains(p))
                    .or(d.panes.first().copied())
                    == Some(Pane::Properties)
        })
    }

    fn properties_pane(&mut self, ui: &mut egui::Ui) {
        if self.mode_sch || self.mode_fp || self.studio.home {
            let what = if self.studio.home {
                "the Home page"
            } else if self.mode_sch {
                "the schematic; its tools are in the Navigator panel"
            } else {
                "the footprint editor; its settings are in the Navigator panel"
            };
            ui.label(RichText::new(format!("Showing {what}.")).weak());
            return;
        }
        if self.scene.is_none() {
            ui.label(RichText::new("No board open.").weak());
            return;
        }
        // The current tool's options, as Altium shows them here.
        let tool = match self.tool {
            Tool::Route if self.router.pairs => Some("Differential Pair Routing"),
            Tool::Route => Some("Interactive Routing"),
            Tool::Tune => Some("Length Tuning"),
            Tool::Text => Some("Text"),
            Tool::Dimension => Some("Dimension"),
            Tool::Shape => Some("Shapes and Pours"),
            _ => None,
        };
        if let Some(t) = tool {
            caption(ui, t);
            ui.horizontal_wrapped(|ui| match self.tool {
                Tool::Route => self.route_bar(ui),
                Tool::Tune => self.tune_bar(ui),
                Tool::Text => self.edit.text_bar(ui),
                Tool::Dimension => self.edit.dim_bar(ui),
                Tool::Shape => self.shape.bar(ui),
                _ => {}
            });
            ui.separator();
        }
        let Some(scene) = &self.scene else { return };
        let b = &scene.board;
        // What is selected: the box selection, else the last clicked item.
        let mut sel = self.edit.sel.clone();
        if sel.is_empty() {
            match self.picked {
                Some(Source::Track(i)) => {
                    sel.tracks.insert(i);
                }
                Some(Source::Via(i)) => {
                    sel.vias.insert(i);
                }
                Some(Source::Arc(i)) => {
                    sel.arcs.insert(i);
                }
                _ => {}
            }
        }
        let sig = format!(
            "{:?}{:?}{:?}{:?}{}",
            sel.parts,
            sel.tracks,
            sel.vias,
            sel.arcs,
            self.router.history.undo_label().unwrap_or_default()
        );
        if sig != self.panes.fields_for {
            self.panes.fields.clear();
            self.panes.fields_for = sig;
        }
        let g = &b.geometry;
        let mut edits: Vec<Edit> = Vec::new();
        let mut label = String::new();
        if sel.is_empty() {
            if self.selected.is_some() {
                caption(ui, "Net");
                self.net_info(ui);
                return;
            }
            caption(ui, "Board");
            let name = scene
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (w, h) = {
                let r = scene.bounds;
                ((r[2] - r[0]) as f64, (r[3] - r[1]) as f64)
            };
            let open = b
                .nets
                .iter()
                .filter(|n| n.pads >= 2 && !n.has_pour && !n.is_complete())
                .count();
            egui::Grid::new("prop_board")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "File", name);
                    row(
                        ui,
                        "Size",
                        format!("{} × {}", self.studio.len(w), self.studio.len(h)),
                    );
                    row(ui, "Copper layers", scene.copper_count.to_string());
                    let t = b.stackup.total_thickness();
                    if t.0 > 0 {
                        row(ui, "Thickness", self.studio.len(t.mil()));
                    }
                    row(ui, "Parts", b.components.len().to_string());
                    row(ui, "Nets", format!("{} ({} open)", b.nets.len(), open));
                    row(ui, "Pairs", b.diff_pairs.len().to_string());
                    row(ui, "Tracks", g.tracks.len().to_string());
                    row(ui, "Vias", g.vias.len().to_string());
                    row(ui, "Rules", b.rules.len().to_string());
                });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Layer stack…").clicked() {
                    self.imp.open = true;
                }
                if ui.button("Rules…").clicked() {
                    self.cm.open = true;
                }
                if ui.button("Check rules").clicked() {
                    self.drc.open = true;
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Click a part, track or via, or Shift+drag a box, to see and change its settings here.").weak());
            return;
        }
        let one = sel.parts.len() + sel.tracks.len() + sel.vias.len() + sel.arcs.len() == 1;
        if one && sel.parts.len() == 1 {
            let i = *sel.parts.iter().next().unwrap();
            let Some(c) = b.components.get(i).cloned() else {
                return;
            };
            caption(ui, "Component");
            let from = Placement::of(b, i);
            let mut to = from;
            egui::Grid::new("prop_part")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "Designator", RichText::new(&c.designator).strong());
                    row(ui, "Footprint", c.footprint.clone());
                    if let Some(v) = c.param(&["Value", "Comment"]) {
                        row(ui, "Value", v.to_string());
                    }
                    if let Some(v) = c.param(&[
                        "Manufacturer Part Number",
                        "MPN",
                        "Part Number",
                        "PartNumber",
                    ]) {
                        row(ui, "Part number", v.to_string());
                    }
                    if let Some(s) = &c.sheet {
                        row(ui, "Sheet", s.clone());
                    }
                    ui.label(RichText::new("X").weak());
                    ui.horizontal(|ui| {
                        if let (Some(v), Some(t)) =
                            (self.len_field(ui, "part_x", c.x.mil()), to.as_mut())
                        {
                            t.x = Length::from_mil(v);
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Y").weak());
                    ui.horizontal(|ui| {
                        if let (Some(v), Some(t)) =
                            (self.len_field(ui, "part_y", c.y.mil()), to.as_mut())
                        {
                            t.y = Length::from_mil(v);
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Rotation").weak());
                    ui.horizontal(|ui| {
                        if let (Some(v), Some(t)) = (
                            self.num_field(ui, "part_rot", c.rotation_deg, "°"),
                            to.as_mut(),
                        ) {
                            t.rotation_deg = v.rem_euclid(360.0);
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Side").weak());
                    ui.horizontal(|ui| {
                        let top = c.side == Side::Top;
                        if ui.selectable_label(top, "Top").clicked() && !top {
                            if let Some(t) = to.as_mut() {
                                t.side = Side::Top;
                            }
                        }
                        if ui.selectable_label(!top, "Bottom").clicked() && top {
                            if let Some(t) = to.as_mut() {
                                t.side = Side::Bottom;
                            }
                        }
                    });
                    ui.end_row();
                });
            let others: Vec<(String, String)> = c
                .params
                .iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !others.is_empty() {
                egui::CollapsingHeader::new(format!("Parameters ({})", others.len()))
                    .id_salt("prop_params")
                    .show(ui, |ui| {
                        egui::Grid::new("prop_params_g")
                            .num_columns(2)
                            .spacing([10.0, 2.0])
                            .show(ui, |ui| {
                                for (k, v) in others {
                                    row(ui, &k, v);
                                }
                            });
                    });
            }
            if let (Some(f), Some(t)) = (from, to) {
                if f != t {
                    edits.push(Edit::PlaceComponent {
                        index: i,
                        from: f,
                        to: t,
                    });
                    label = format!("Changed {}", c.designator);
                }
            }
        } else if one && sel.tracks.len() == 1 {
            let i = *sel.tracks.iter().next().unwrap();
            let Some(t) = g.tracks.get(i).cloned() else {
                return;
            };
            caption(ui, "Track");
            let net = t
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| n.name.clone());
            let len = ((t.end.x.mil() - t.start.x.mil()).powi(2)
                + (t.end.y.mil() - t.start.y.mil()).powi(2))
            .sqrt();
            egui::Grid::new("prop_track")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "Net", net.unwrap_or_else(|| "No net".into()));
                    row(ui, "Layer", self.layer_name(&t.layer));
                    row(
                        ui,
                        "Start",
                        format!(
                            "{}, {}",
                            self.studio.len(t.start.x.mil()),
                            self.studio.len(t.start.y.mil())
                        ),
                    );
                    row(
                        ui,
                        "End",
                        format!(
                            "{}, {}",
                            self.studio.len(t.end.x.mil()),
                            self.studio.len(t.end.y.mil())
                        ),
                    );
                    row(ui, "Length", self.studio.len(len));
                    ui.label(RichText::new("Width").weak());
                    ui.horizontal(|ui| {
                        if let Some(w) = self.len_field(ui, "track_w", t.width.mil()) {
                            if w > 0.0 {
                                let mut n = t.clone();
                                n.width = Length::from_mil(w);
                                edits.push(Edit::RemoveTrack {
                                    index: i,
                                    track: t.clone(),
                                });
                                edits.push(Edit::InsertTrack { index: i, track: n });
                                label = "Changed a track's width".into();
                            }
                        }
                    });
                    ui.end_row();
                });
        } else if one && sel.vias.len() == 1 {
            let i = *sel.vias.iter().next().unwrap();
            let Some(v) = g.vias.get(i).cloned() else {
                return;
            };
            caption(ui, "Via");
            let net = v
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| n.name.clone());
            egui::Grid::new("prop_via")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "Net", net.unwrap_or_else(|| "No net".into()));
                    row(
                        ui,
                        "Location",
                        format!(
                            "{}, {}",
                            self.studio.len(v.at.x.mil()),
                            self.studio.len(v.at.y.mil())
                        ),
                    );
                    row(
                        ui,
                        "Span",
                        format!(
                            "{} to {}",
                            self.layer_name(&LayerRef::Copper(v.from_layer)),
                            self.layer_name(&LayerRef::Copper(v.to_layer))
                        ),
                    );
                    let mut n = v.clone();
                    let mut changed = false;
                    ui.label(RichText::new("Diameter").weak());
                    ui.horizontal(|ui| {
                        if let Some(d) = self.len_field(ui, "via_d", v.diameter.mil()) {
                            if d > v.hole.mil() {
                                n.diameter = Length::from_mil(d);
                                n.rings.clear();
                                changed = true;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Hole").weak());
                    ui.horizontal(|ui| {
                        if let Some(h) = self.len_field(ui, "via_h", v.hole.mil()) {
                            if h > 0.0 && h < n.diameter.mil() {
                                n.hole = Length::from_mil(h);
                                changed = true;
                            }
                        }
                    });
                    ui.end_row();
                    if changed {
                        edits.push(Edit::RemoveVia {
                            index: i,
                            via: v.clone(),
                        });
                        edits.push(Edit::InsertVia { index: i, via: n });
                        label = "Changed a via".into();
                    }
                });
        } else if one && sel.arcs.len() == 1 {
            let i = *sel.arcs.iter().next().unwrap();
            let Some(a) = g.arcs.get(i).cloned() else {
                return;
            };
            caption(ui, "Arc");
            let net = a
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| n.name.clone());
            egui::Grid::new("prop_arc")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "Net", net.unwrap_or_else(|| "No net".into()));
                    row(ui, "Layer", self.layer_name(&a.layer));
                    row(ui, "Radius", self.studio.len(a.radius.mil()));
                    row(
                        ui,
                        "Angles",
                        format!("{:.1}° to {:.1}°", a.start_deg, a.end_deg),
                    );
                    row(ui, "Width", self.studio.len(a.width.mil()));
                });
        } else {
            caption(ui, "Selection");
            egui::Grid::new("prop_many")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (k, n) in [
                        ("Parts", sel.parts.len()),
                        ("Tracks", sel.tracks.len()),
                        ("Arcs", sel.arcs.len()),
                        ("Vias", sel.vias.len()),
                    ] {
                        if n > 0 {
                            row(ui, k, n.to_string());
                        }
                    }
                });
            if !sel.tracks.is_empty() {
                let tracks: Vec<(usize, trace_core::Track)> = sel
                    .tracks
                    .iter()
                    .filter_map(|&i| g.tracks.get(i).map(|t| (i, t.clone())))
                    .collect();
                let widths: Vec<f64> = tracks.iter().map(|(_, t)| t.width.mil()).collect();
                let w0 = widths.first().copied().unwrap_or(0.0);
                let same = widths.iter().all(|w| (w - w0).abs() < 1e-6);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if same {
                            "Track width"
                        } else {
                            "Track width (mixed)"
                        })
                        .weak(),
                    );
                    if let Some(w) = self.len_field(ui, "many_w", w0) {
                        if w > 0.0 {
                            for (i, t) in &tracks {
                                let mut n = t.clone();
                                n.width = Length::from_mil(w);
                                edits.push(Edit::RemoveTrack {
                                    index: *i,
                                    track: t.clone(),
                                });
                                edits.push(Edit::InsertTrack {
                                    index: *i,
                                    track: n,
                                });
                            }
                            label = format!("Changed the width of {} tracks", sel.tracks.len());
                        }
                    }
                });
            }
        }
        if !edits.is_empty() {
            self.apply_edits(&label, edits);
        }
    }
}
