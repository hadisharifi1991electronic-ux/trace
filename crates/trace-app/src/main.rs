//! Trace desktop app: open Altium and Allegro boards, inspect nets, measure,
//! check pairs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod canvas;
mod scene;

use eframe::egui::{self, Color32, RichText};
use scene::{Scene, Slot};
use std::path::PathBuf;
use trace_core::Side;

fn main() -> eframe::Result {
    let start_file = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Trace")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Trace",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, start_file)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Measure,
}

/// A distance measurement: first point, and second once placed.
#[derive(Clone, Copy)]
pub struct Measure {
    pub a: [f32; 2],
    pub b: Option<[f32; 2]>,
}

pub struct App {
    scene: Option<Scene>,
    tool: Tool,
    measure: Option<Measure>,
    view: canvas::View,
    active: Slot,
    selected: Option<u32>,
    marker: Option<[f32; 2]>,
    search: String,
    message: Option<(String, bool)>, // text, is_error
    recent: Vec<PathBuf>,
    show_pairs: bool,
    pair_sort_skew: bool,
    hover: Option<String>,
    cursor: Option<[f32; 2]>,
    needs_fit: bool,
}

const RECENT_KEY: &str = "recent_files";

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start_file: Option<PathBuf>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        cc.egui_ctx.set_style(style);
        let recent = cc
            .storage
            .and_then(|s| s.get_string(RECENT_KEY))
            .map(|s| {
                s.lines()
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut app = App {
            scene: None,
            tool: Tool::Select,
            measure: None,
            view: canvas::View::default(),
            active: Slot::Copper(0),
            selected: None,
            marker: None,
            search: String::new(),
            message: None,
            recent,
            show_pairs: true,
            pair_sort_skew: true,
            hover: None,
            cursor: None,
            needs_fit: false,
        };
        if let Some(f) = start_file {
            app.open(f);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        match Scene::load(path.clone()) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.selected = None;
                self.marker = None;
                self.measure = None;
                self.active = Slot::Copper(0);
                self.needs_fit = true;
                self.message = None;
                self.recent.retain(|p| p != &path);
                self.recent.insert(0, path);
                self.recent.truncate(8);
            }
            Err(e) => {
                self.message = Some((format!("Could not open {}: {e}", path.display()), true))
            }
        }
    }

    fn open_dialog(&mut self) {
        let gerber = [
            "zip", "tgz", "gbr", "ger", "gbx", "pho", "art", "gtl", "gbl", "gto", "gbo", "gts",
            "gbs", "gtp", "gbp", "gko", "gm1", "g1", "g2", "g3", "g4", "drl", "xln", "txt",
        ];
        let mut boards: Vec<&str> = vec!["PcbDoc", "pcbdoc", "brd", "BRD", "tracepcb"];
        boards.extend(gerber);
        if let Some(mut picked) = rfd::FileDialog::new()
            .add_filter("Boards and fabrication data", &boards)
            .add_filter("Altium PCB", &["PcbDoc", "pcbdoc"])
            .add_filter("Allegro board", &["brd", "BRD"])
            .add_filter("Gerber, drill or ODB++", &gerber)
            .add_filter("All files", &["*"])
            .set_title("Open a board (pick several Gerber files to load them together)")
            .pick_files()
        {
            if picked.len() == 1 {
                self.open(picked.remove(0));
            } else if !picked.is_empty() {
                self.open_files(picked);
            }
        }
    }

    fn open_folder_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open a folder of Gerber and drill files, or an ODB++ folder")
            .pick_folder()
        {
            self.open(p);
        }
    }

    fn open_files(&mut self, paths: Vec<PathBuf>) {
        match Scene::load_files(paths) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.selected = None;
                self.marker = None;
                self.measure = None;
                self.active = Slot::Copper(0);
                self.needs_fit = true;
                self.message = None;
            }
            Err(e) => self.message = Some((format!("Could not open these files: {e}"), true)),
        }
    }

    fn save_as(&mut self) {
        let Some(scene) = &self.scene else { return };
        let stem = scene
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "board".into());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Trace board", &[trace_core::file::EXTENSION])
            .set_file_name(format!("{stem}.{}", trace_core::file::EXTENSION))
            .save_file()
        else {
            return;
        };
        self.message = Some(match trace_core::file::save(&scene.board, &path) {
            Ok(()) => (format!("Saved {}", path.display()), false),
            Err(e) => (format!("Could not save: {e}"), true),
        });
    }

    fn export_pairs(&mut self) {
        let Some(scene) = &self.scene else { return };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .set_file_name("pairs.csv")
            .save_file()
        else {
            return;
        };
        let mut out =
            String::from("pair,positive,negative,p_mil,n_mil,skew_mil,p_vias,n_vias,status\n");
        for r in pair_rows(scene) {
            out += &format!(
                "{},{},{},{:.1},{:.1},{:.1},{},{},{}\n",
                r.name,
                r.pos,
                r.neg,
                r.p_len,
                r.n_len,
                r.skew,
                r.p_vias,
                r.n_vias,
                r.status.label()
            );
        }
        self.message = Some(match std::fs::write(&path, out) {
            Ok(()) => (format!("Saved {}", path.display()), false),
            Err(e) => (format!("Could not save: {e}"), true),
        });
    }

    fn find(&mut self) {
        let Some(scene) = &self.scene else { return };
        let q = self.search.trim();
        if q.is_empty() {
            return;
        }
        if let Some(c) = scene
            .board
            .components
            .iter()
            .find(|c| c.designator.eq_ignore_ascii_case(q))
        {
            let p = [c.x.mil() as f32, c.y.mil() as f32];
            self.view.center_on(p, 3.0);
            self.marker = Some(p);
            self.selected = None;
            self.message = Some((
                format!(
                    "{} · {} · {}",
                    c.designator,
                    c.footprint,
                    if c.side == Side::Top { "top" } else { "bottom" }
                ),
                false,
            ));
            return;
        }
        let up = q.to_ascii_uppercase();
        let net = scene.net_by_name.get(q).copied().or_else(|| {
            let names = &scene.board.nets;
            names
                .iter()
                .position(|n| n.name.to_ascii_uppercase() == up)
                .or_else(|| {
                    names
                        .iter()
                        .position(|n| n.name.to_ascii_uppercase().contains(&up))
                })
                .map(|i| i as u32)
        });
        match net {
            Some(n) => self.select_net(n, true),
            None => self.message = Some((format!("Nothing called \u{201c}{q}\u{201d}."), true)),
        }
    }

    fn select_net(&mut self, net: u32, zoom: bool) {
        self.selected = Some(net);
        self.marker = None;
        self.message = None;
        if zoom {
            if let Some(b) = self.scene.as_ref().and_then(|s| s.net_bounds(net)) {
                self.view.fit_rect(b, 0.7);
            }
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add(egui::Button::new("Open board…").shortcut_text("Ctrl+O"))
                    .clicked()
                {
                    ui.close_menu();
                    self.open_dialog();
                }
                if ui.button("Open Gerber or ODB++ folder…").clicked() {
                    ui.close_menu();
                    self.open_folder_dialog();
                }
                ui.menu_button("Open recent", |ui| {
                    if self.recent.is_empty() {
                        ui.label(RichText::new("No recent boards").weak());
                    }
                    for p in self.recent.clone() {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if ui
                            .button(name)
                            .on_hover_text(p.display().to_string())
                            .clicked()
                        {
                            ui.close_menu();
                            self.open(p);
                        }
                    }
                });
                ui.separator();
                if ui
                    .add_enabled(self.scene.is_some(), egui::Button::new("Save as Trace board…"))
                    .clicked()
                {
                    ui.close_menu();
                    self.save_as();
                }
                if ui
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Export pair report…"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.export_pairs();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui
                    .add(egui::Button::new("Fit board").shortcut_text("F"))
                    .clicked()
                {
                    ui.close_menu();
                    self.needs_fit = true;
                }
                if ui
                    .add(
                        egui::Button::new(if self.view.flip {
                            "View from top"
                        } else {
                            "View from bottom"
                        })
                        .shortcut_text("B"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.view.flip = !self.view.flip;
                    self.needs_fit = true;
                }
                ui.checkbox(&mut self.show_pairs, "Differential pairs panel");
            });
            ui.menu_button("Help", |ui| {
                ui.label("Trace 0.1, open-source PCB design");
                ui.label("Opens Altium .PcbDoc and Allegro .brd (16.0 and newer) boards,\nand rebuilds boards from Gerber and drill files (a folder, a .zip,\nor several files picked or dropped together). Each file's layer is\nrecognised automatically and nets are traced through the copper.");
                ui.label(RichText::new("Scroll to zoom, drag to pan, click to select a net.\nM measures: click two points (they snap to pads, vias and track ends).\nF fits the board, B flips to the bottom, Esc clears.").weak());
            });
        });
    }

    fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.measure = None;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.tool == Tool::Select, "Select")
                .on_hover_text("Click copper to select its net")
                .clicked()
            {
                self.set_tool(Tool::Select);
            }
            if ui
                .selectable_label(self.tool == Tool::Measure, "Measure (M)")
                .on_hover_text(
                    "Click two points to measure; they snap to pads, vias and track ends",
                )
                .clicked()
            {
                self.set_tool(Tool::Measure);
            }
            ui.separator();
            let hint = match (self.tool, self.measure) {
                (Tool::Measure, None) => "Click the first point.",
                (Tool::Measure, Some(Measure { b: None, .. })) => "Click the second point.",
                (Tool::Measure, Some(_)) => "Click to start a new measurement, Esc to clear.",
                _ => "Drag to pan, scroll to zoom, click copper to select a net.",
            };
            ui.label(RichText::new(hint).weak());
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else {
            ui.add_space(12.0);
            ui.label("No board open.");
            if ui.button("Open board…").clicked() {
                self.open_dialog();
            }
            if ui.button("Open Gerber or ODB++ folder…").clicked() {
                self.open_folder_dialog();
            }
            ui.label(
                RichText::new("Or drop a board, a Gerber .zip, a folder or several Gerber files here.")
                    .weak(),
            );
            return;
        };
        let b = &scene.board;
        let name = scene
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.add_space(6.0);
        ui.heading(name);
        let open = b
            .nets
            .iter()
            .filter(|n| n.pads >= 2 && !n.has_pour && !n.is_complete())
            .count();
        let thickness = b.stackup.total_thickness();
        ui.label(
            RichText::new(format!(
                "{} copper layers{}\n{} parts · {} nets · {} pairs\n{} nets still open",
                scene.copper_count,
                if thickness.0 > 0 {
                    format!(" · {:.2} mm", thickness.mm())
                } else {
                    String::new()
                },
                b.components.len(),
                b.nets.len(),
                b.diff_pairs.len(),
                open
            ))
            .weak(),
        );
        if let Some(imp) = &scene.import {
            egui::CollapsingHeader::new(
                RichText::new(format!("Rebuilt from {} files", imp.files.len())).small(),
            )
            .id_salt("import")
            .show(ui, |ui| {
                ui.label(RichText::new(format!("Nets: {}", imp.nets_from)).small().weak());
                egui::Grid::new("import_files")
                    .num_columns(2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        for (file, role, why) in &imp.files {
                            let short = file.rsplit('/').next().unwrap_or(file);
                            ui.label(RichText::new(short).small()).on_hover_text(file);
                            ui.label(RichText::new(role).small().weak()).on_hover_text(why);
                            ui.end_row();
                        }
                    });
                for w in imp.warnings.iter().take(5) {
                    ui.label(RichText::new(w).small().color(Color32::from_rgb(255, 180, 84)));
                }
            });
        }
        ui.add_space(6.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Find a net or part")
                .desired_width(f32::INFINITY),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.find();
        }
        if let Some((m, err)) = &self.message {
            ui.label(RichText::new(m).color(if *err {
                Color32::from_rgb(255, 180, 84)
            } else {
                ui.visuals().text_color()
            }));
        }
        if let Some(m) = self.measure {
            ui.separator();
            ui.label(RichText::new("MEASURE").small().weak());
            let b = m.b.or(self.cursor).unwrap_or(m.a);
            let (dx, dy) = (b[0] - m.a[0], b[1] - m.a[1]);
            let d = dx.hypot(dy);
            egui::Grid::new("measure")
                .num_columns(2)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    for (k, v) in [("Distance", d), ("Δx", dx.abs()), ("Δy", dy.abs())] {
                        ui.label(RichText::new(k).weak());
                        ui.label(format!("{v:.2} mil · {:.4} mm", v * 0.0254));
                        ui.end_row();
                    }
                });
        }
        ui.separator();
        self.net_info(ui);
        ui.separator();
        ui.label(RichText::new("LAYERS").small().weak());
        let Some(scene) = &mut self.scene else { return };
        let mut new_active = None;
        egui::ScrollArea::vertical()
            .id_salt("layers")
            .show(ui, |ui| {
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
                        let r = ui.selectable_label(active, text).on_hover_text(
                            "Click to make active. Double-click to show only this layer.",
                        );
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
            });
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

    fn net_info(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else { return };
        let Some(id) = self.selected else {
            ui.label(RichText::new("Click a track, pad or via to select its net.").weak());
            return;
        };
        let Some(net) = scene.board.nets.get(id as usize) else {
            return;
        };
        ui.label(RichText::new(&net.name).strong().size(15.0));
        let warn = Color32::from_rgb(255, 180, 84);
        egui::Grid::new("net")
            .num_columns(2)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Length").weak());
                ui.label(format!(
                    "{:.1} mil · {:.2} mm",
                    net.total_length().mil(),
                    net.total_length().mm()
                ));
                ui.end_row();
                if scene.board.stackup.total_thickness().0 > 0 {
                    ui.label(RichText::new("Via barrels").weak());
                    ui.label(format!(
                        "{:.1} mil ({} vias)",
                        net.via_length.mil(),
                        net.vias
                    ));
                } else {
                    ui.label(RichText::new("Vias").weak());
                    ui.label(net.vias.to_string());
                }
                ui.end_row();
                ui.label(RichText::new("Routing").weak());
                let (text, bad) = if net.pads < 2 {
                    ("single pad".to_string(), false)
                } else if net.has_pour {
                    ("uses a pour, not checked".to_string(), false)
                } else if net.routed_length.0 == 0 {
                    ("not routed".to_string(), true)
                } else if net.pieces <= 1 {
                    (format!("complete, {} pads", net.pads), false)
                } else {
                    (format!("open: {} separate pieces", net.pieces), true)
                };
                ui.label(if bad {
                    RichText::new(text).color(warn)
                } else {
                    RichText::new(text)
                });
                ui.end_row();
                if let Some((pair, other)) = scene.pair_of.get(&net.name) {
                    if let Some(o) = scene.board.net(other) {
                        let skew = (net.total_length().mil() - o.total_length().mil()).abs();
                        ui.label(RichText::new("Pair").weak());
                        ui.label(pair);
                        ui.end_row();
                        ui.label(RichText::new("Partner").weak());
                        ui.label(format!("{other} ({:.1} mil)", o.total_length().mil()));
                        ui.end_row();
                        ui.label(RichText::new("Skew").weak());
                        let t = format!("{skew:.1} mil");
                        ui.label(if skew > 5.0 {
                            RichText::new(t).color(warn)
                        } else {
                            RichText::new(t)
                        });
                        ui.end_row();
                    }
                }
            });
    }

    fn pairs_panel(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else { return };
        let mut rows = pair_rows(scene);
        if self.pair_sort_skew {
            rows.sort_by(|a, b| {
                (a.status.rank(), b.skew)
                    .partial_cmp(&(b.status.rank(), a.skew))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            rows.sort_by(|a, b| a.name.cmp(&b.name));
        }
        let done = rows
            .iter()
            .filter(|r| r.status != PairStatus::Open && r.status != PairStatus::Unrouted)
            .count();
        let over = rows.iter().filter(|r| r.status == PairStatus::Over).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Differential pairs").strong());
            ui.label(
                RichText::new(format!(
                    "{} fully routed ({} over 5 mil) · {} open",
                    done,
                    over,
                    rows.len() - done
                ))
                .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(&mut self.pair_sort_skew, false, "By name");
                ui.selectable_value(&mut self.pair_sort_skew, true, "By problem");
            });
        });
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("pairs")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("pairs_grid")
                    .striped(true)
                    .num_columns(6)
                    .spacing([18.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Pair", "P (mil)", "N (mil)", "Skew", "Vias", "Status"] {
                            ui.label(RichText::new(h).weak());
                        }
                        ui.end_row();
                        for r in &rows {
                            let selected = self.selected.is_some_and(|s| {
                                scene
                                    .board
                                    .nets
                                    .get(s as usize)
                                    .is_some_and(|n| n.name == r.pos || n.name == r.neg)
                            });
                            if ui.selectable_label(selected, &r.name).clicked() {
                                clicked = scene.net_by_name.get(&r.pos).copied();
                            }
                            ui.label(format!("{:.1}", r.p_len));
                            ui.label(format!("{:.1}", r.n_len));
                            ui.label(format!("{:.1}", r.skew));
                            ui.label(format!("{}/{}", r.p_vias, r.n_vias));
                            ui.label(RichText::new(r.status.label()).color(r.status.color(ui)));
                            ui.end_row();
                        }
                    });
            });
        if let Some(n) = clicked {
            self.select_net(n, true);
        }
    }
}

pub const MEASURE: Color32 = Color32::from_rgb(255, 214, 10);

/// One-line summary of a measurement.
pub fn measure_text(a: [f32; 2], b: [f32; 2]) -> Option<String> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d = dx.hypot(dy);
    Some(format!(
        "Measured {d:.2} mil · {:.4} mm   (Δx {:.2}, Δy {:.2} mil)",
        d * 0.0254,
        dx.abs(),
        dy.abs()
    ))
}

#[derive(PartialEq, Clone, Copy)]
enum PairStatus {
    Over,
    Ok,
    Open,
    Unrouted,
}

impl PairStatus {
    fn label(self) -> &'static str {
        match self {
            PairStatus::Over => "over 5 mil",
            PairStatus::Ok => "ok",
            PairStatus::Open => "open",
            PairStatus::Unrouted => "not routed",
        }
    }
    fn rank(self) -> u8 {
        match self {
            PairStatus::Over => 0,
            PairStatus::Open => 1,
            PairStatus::Ok => 2,
            PairStatus::Unrouted => 3,
        }
    }
    fn color(self, ui: &egui::Ui) -> Color32 {
        match self {
            PairStatus::Over | PairStatus::Open => Color32::from_rgb(255, 180, 84),
            PairStatus::Ok => Color32::from_rgb(127, 209, 127),
            PairStatus::Unrouted => ui.visuals().weak_text_color(),
        }
    }
}

struct PairRow {
    name: String,
    pos: String,
    neg: String,
    p_len: f64,
    n_len: f64,
    skew: f64,
    p_vias: usize,
    n_vias: usize,
    status: PairStatus,
}

fn pair_rows(scene: &Scene) -> Vec<PairRow> {
    let b = &scene.board;
    b.diff_pairs
        .iter()
        .filter_map(|p| {
            let (pn, nn) = (b.net(&p.positive)?, b.net(&p.negative)?);
            let (p_len, n_len) = (pn.total_length().mil(), nn.total_length().mil());
            let skew = (p_len - n_len).abs();
            let status = if pn.routed_length.0 == 0 && nn.routed_length.0 == 0 {
                PairStatus::Unrouted
            } else if !pn.is_complete()
                || !nn.is_complete()
                || pn.routed_length.0 == 0
                || nn.routed_length.0 == 0
            {
                PairStatus::Open
            } else if skew > 5.0 {
                PairStatus::Over
            } else {
                PairStatus::Ok
            };
            Some(PairRow {
                name: p.name.clone(),
                pos: p.positive.clone(),
                neg: p.negative.clone(),
                p_len,
                n_len,
                skew,
                p_vias: pn.vias,
                n_vias: nn.vias,
                status,
            })
        })
        .collect()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Files dropped on the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.len() > 1 {
            self.open_files(dropped);
        } else if let Some(p) = dropped.into_iter().next() {
            self.open(p);
        }
        let typing = ctx.wants_keyboard_input();
        let (open_shortcut, fit, flip, esc, measure_key) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::O),
                !typing && i.key_pressed(egui::Key::F),
                !typing && i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::Escape),
                !typing && i.key_pressed(egui::Key::M),
            )
        });
        if measure_key {
            self.set_tool(if self.tool == Tool::Measure {
                Tool::Select
            } else {
                Tool::Measure
            });
        }
        if open_shortcut {
            self.open_dialog();
        }
        if fit {
            self.needs_fit = true;
        }
        if flip {
            self.view.flip = !self.view.flip;
            self.needs_fit = true;
        }
        if esc {
            if self.measure.is_some() {
                self.measure = None;
            } else if self.tool == Tool::Measure {
                self.set_tool(Tool::Select);
            } else {
                self.selected = None;
                self.marker = None;
            }
        }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui, ctx));
        if self.scene.is_some() {
            egui::TopBottomPanel::top("tools").show(ctx, |ui| self.toolbar(ui));
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some([x, y]) = self.cursor {
                    ui.label(
                        RichText::new(format!(
                            "x {x:.1}  y {y:.1} mil   ·   {:.3}, {:.3} mm",
                            x * 0.0254,
                            y * 0.0254
                        ))
                        .monospace()
                        .weak(),
                    );
                }
                if let Some(text) = self.measure.and_then(|m| measure_text(m.a, m.b?)) {
                    ui.label(RichText::new(text).strong().color(MEASURE));
                } else if let Some(h) = &self.hover {
                    ui.label(RichText::new(h).strong());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "zoom {:.0}%{}",
                            self.view.scale * 100.0,
                            if self.view.flip {
                                " · bottom view"
                            } else {
                                ""
                            }
                        ))
                        .weak(),
                    );
                });
            });
        });
        egui::SidePanel::left("side")
            .resizable(true)
            .default_width(270.0)
            .show(ctx, |ui| self.side_panel(ui));
        if self.show_pairs
            && self
                .scene
                .as_ref()
                .is_some_and(|s| !s.board.diff_pairs.is_empty())
        {
            egui::TopBottomPanel::bottom("pairs")
                .resizable(true)
                .default_height(220.0)
                .show(ctx, |ui| self.pairs_panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::none().fill(Color32::from_rgb(15, 17, 21))).show(ctx, |ui| {
            if self.scene.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Open a board with File → Open board…, or drop a .PcbDoc or .brd file here.").size(16.0).weak());
                });
                return;
            }
            canvas::show(self, ui);
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let list: Vec<String> = self
            .recent
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        storage.set_string(RECENT_KEY, list.join("\n"));
    }
}
