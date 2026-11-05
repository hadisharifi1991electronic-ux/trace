//! Trace desktop app: open Altium and Allegro boards, inspect nets, measure,
//! check pairs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod canvas;
mod cm;
mod imp;
mod scene;
mod schview;
mod view3d;

use eframe::egui::{self, Color32, RichText};
use scene::{Scene, Slot};
use std::path::PathBuf;
use trace_core::Side;

fn main() -> eframe::Result {
    let start_file = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(concat!("Trace ", env!("CARGO_PKG_VERSION")))
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../assets/icon-128.rgba").to_vec(),
                width: 128,
                height: 128,
            })),
        depth_buffer: 24,
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
    mode3d: bool,
    view3d: view3d::View3d,
    cm: cm::Cm,
    imp: imp::Imp,
    mode_sch: bool,
    schem: schview::Schematics,
    ctx: egui::Context,
    /// Where the board view should go on its next frame: a point and zoom,
    /// or an area to fit.
    focus: Option<Focus>,
}

#[derive(Clone, Copy)]
pub enum Focus {
    Point([f32; 2], f32),
    Area([f32; 4]),
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
            mode3d: false,
            view3d: view3d::View3d::default(),
            cm: cm::Cm::new(cc.storage.and_then(|s| s.get_string(cm::STORE_KEY))),
            imp: imp::Imp::new(cc.storage.and_then(|s| s.get_string(imp::STORE_KEY))),
            mode_sch: false,
            schem: schview::Schematics::default(),
            ctx: cc.egui_ctx.clone(),
            focus: None,
        };
        if let Some(f) = start_file {
            app.open(f);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "schdoc" || ext == "prjpcb" {
            self.schem.open_path(&path, &self.ctx);
            self.mode_sch = true;
            self.mode3d = false;
            self.message = None;
            self.recent.retain(|p| p != &path);
            self.recent.insert(0, path.clone());
            self.recent.truncate(8);
            if ext == "prjpcb" {
                if let Some(board) = schview::project_boards(&path)
                    .into_iter()
                    .find(|b| b.exists())
                {
                    self.load_board(board);
                }
            }
            return;
        }
        if self.load_board(path.clone()) {
            self.mode_sch = false;
            self.recent.retain(|p| p != &path);
            self.recent.insert(0, path.clone());
            self.recent.truncate(8);
            // Bring in the schematic of the board's project, when there is one.
            if let Some(prj) = schview::find_project(&path) {
                if self.schem.project.as_ref() != Some(&prj) {
                    self.schem.open_path(&prj, &self.ctx);
                }
            }
        }
    }

    /// Loads a board; true when it opened.
    fn load_board(&mut self, path: PathBuf) -> bool {
        match Scene::load(path.clone()) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.view3d.reset();
                self.load_constraints();
                self.selected = None;
                self.marker = None;
                self.measure = None;
                self.active = Slot::Copper(0);
                self.needs_fit = true;
                self.message = None;
                true
            }
            Err(e) => {
                self.message = Some((format!("Could not open {}: {e}", path.display()), true));
                false
            }
        }
    }

    /// Selects a part on the board by designator, optionally zooming to it.
    fn select_part(&mut self, designator: &str, zoom: bool) -> bool {
        let Some(scene) = &self.scene else {
            return false;
        };
        let Some(c) = scene
            .board
            .components
            .iter()
            .find(|c| c.designator.eq_ignore_ascii_case(designator))
        else {
            return false;
        };
        let p = [c.x.mil() as f32, c.y.mil() as f32];
        if zoom {
            self.focus = Some(Focus::Point(p, 3.0));
            self.needs_fit = false;
        }
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
        true
    }

    fn open_dialog(&mut self) {
        let gerber = [
            "zip", "tgz", "gbr", "ger", "gbx", "pho", "art", "gtl", "gbl", "gto", "gbo", "gts",
            "gbs", "gtp", "gbp", "gko", "gm1", "g1", "g2", "g3", "g4", "drl", "xln", "txt",
        ];
        let mut boards: Vec<&str> = vec![
            "PcbDoc", "pcbdoc", "brd", "BRD", "tracepcb", "SchDoc", "schdoc", "PrjPcb", "prjpcb",
        ];
        boards.extend(gerber);
        if let Some(mut picked) = rfd::FileDialog::new()
            .add_filter("Boards, schematics and fabrication data", &boards)
            .add_filter("Altium project", &["PrjPcb", "prjpcb"])
            .add_filter("Altium PCB", &["PcbDoc", "pcbdoc"])
            .add_filter("Altium schematic", &["SchDoc", "schdoc"])
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
                self.view3d.reset();
                self.load_constraints();
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

    fn save_kicad(&mut self) {
        let Some(scene) = &self.scene else { return };
        let stem = scene
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "board".into());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("KiCad board", &["kicad_pcb"])
            .set_file_name(format!("{stem}.kicad_pcb"))
            .save_file()
        else {
            return;
        };
        let text = trace_export::to_kicad(&scene.board);
        self.message = Some(match std::fs::write(&path, text) {
            Ok(()) => (
                format!("Saved {} (opens in KiCad 7 or later)", path.display()),
                false,
            ),
            Err(e) => (format!("Could not save: {e}"), true),
        });
    }

    fn save_altium(&mut self) {
        let Some(scene) = &self.scene else { return };
        let stem = scene
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "board".into());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Altium board", &["PcbDoc"])
            .set_file_name(format!("{stem}.PcbDoc"))
            .save_file()
        else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = trace_export::to_altium(&scene.board, &name)
            .and_then(|bytes| std::fs::write(&path, bytes));
        self.message = Some(match result {
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

    fn find_in_schematic(&mut self) {
        let q = self.search.trim().to_string();
        if q.is_empty() {
            return;
        }
        if self.schem.reveal_part(&q) {
            self.select_part(&q, true);
            return;
        }
        let net = self.scene.as_ref().and_then(|scene| {
            scene.net_by_name.get(&q).copied().or_else(|| {
                scene
                    .board
                    .nets
                    .iter()
                    .position(|n| n.name.eq_ignore_ascii_case(&q))
                    .map(|i| i as u32)
            })
        });
        if let (Some(n), Some(scene)) = (net, self.scene.as_ref()) {
            if self.schem.reveal(n, scene) {
                self.selected = Some(n);
                self.marker = None;
                self.message = None;
                return;
            }
        }
        self.message = Some((
            format!("Nothing called \u{201c}{q}\u{201d} on the sheets."),
            true,
        ));
    }

    fn load_constraints(&mut self) {
        if let Some(s) = &self.scene {
            self.cm.load(s.path.display().to_string(), &s.board);
            self.imp.load(s.path.display().to_string(), &s.board);
        }
    }

    fn select_net(&mut self, net: u32, zoom: bool) {
        self.selected = Some(net);
        self.marker = None;
        self.message = None;
        if zoom {
            if let Some(b) = self.scene.as_ref().and_then(|s| s.net_bounds(net)) {
                self.focus = Some(Focus::Area(b));
                self.needs_fit = false;
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
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Save as Trace board…"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.save_as();
                }
                if ui
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Save as KiCad board…"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.save_kicad();
                }
                if ui
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Save as Altium board…"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.save_altium();
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
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new(if self.mode3d { "2D view" } else { "3D view" })
                            .shortcut_text("3"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.mode3d = !self.mode3d;
                }
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
                if ui
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Constraint Manager…").shortcut_text("C"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.cm.open = true;
                }
                if ui
                    .add_enabled(
                        self.scene.is_some(),
                        egui::Button::new("Impedance and stack-up…").shortcut_text("I"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.imp.open = true;
                }
            });
            ui.menu_button("Help", |ui| {
                ui.label(concat!("Trace ", env!("CARGO_PKG_VERSION"), ", open-source PCB design"));
                ui.label("Opens Altium .PcbDoc and Allegro .brd (16.0 and newer) boards,\nGerber and drill files and ODB++ jobs. Saves KiCad and Altium\nboards. 3D view (3) and Constraint Manager (C).");
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
            let has_board = self.scene.is_some();
            if !self.schem.is_empty()
                && ui
                    .selectable_label(self.mode_sch, "Schematic")
                    .on_hover_text("Schematic sheets (S switches)")
                    .clicked()
            {
                self.mode_sch = true;
                self.mode3d = false;
            }
            if has_board
                && ui
                    .selectable_label(!self.mode3d && !self.mode_sch, "Board")
                    .on_hover_text("Board view")
                    .clicked()
            {
                self.mode3d = false;
                self.mode_sch = false;
            }
            if has_board
                && ui
                    .selectable_label(self.mode3d && !self.mode_sch, "3D")
                    .on_hover_text("3D view of the assembled board (3 switches)")
                    .clicked()
            {
                self.mode3d = true;
                self.mode_sch = false;
            }
            if !has_board {
                ui.separator();
                ui.label(
                    RichText::new(
                        "Click a wire or pin to select its net. Double-click a sheet symbol to open its sheet. F fits.",
                    )
                    .weak(),
                );
                return;
            }
            ui.separator();
            if ui
                .selectable_label(self.cm.open, "Constraints")
                .on_hover_text("Constraint Manager: length, match, skew and via limits (C)")
                .clicked()
            {
                self.cm.open = !self.cm.open;
            }
            if ui
                .selectable_label(self.imp.open, "Impedance")
                .on_hover_text("Stack-up, widths for target impedance, and each pair's impedance (I)")
                .clicked()
            {
                self.imp.open = !self.imp.open;
            }
            ui.separator();
            if self.mode_sch {
                ui.label(
                    RichText::new(
                        "Click a wire or pin to select its net on the board too. Double-click a sheet symbol to open its sheet. F fits.",
                    )
                    .weak(),
                );
                return;
            }
            if self.mode3d {
                ui.label(
                    RichText::new(
                        "Drag to turn, right-drag to pan, scroll to zoom. T top, B flip, F reset.",
                    )
                    .weak(),
                );
                return;
            }
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
        if self.mode_sch {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Find a part or net (Enter)")
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.find_in_schematic();
            }
            self.schem
                .side_panel(ui, self.selected, self.scene.as_ref());
            if let Some((text, true)) = &self.message {
                ui.colored_label(Color32::from_rgb(240, 96, 96), text.clone());
            }
            if self.scene.is_some() {
                ui.separator();
                if let Some(n) = self.selected {
                    let name = self
                        .scene
                        .as_ref()
                        .and_then(|s| s.board.nets.get(n as usize))
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    ui.label(format!("Board net {name}"));
                    if ui.button("Show on board").clicked() {
                        self.mode_sch = false;
                        self.mode3d = false;
                        self.select_net(n, true);
                    }
                } else if let Some((text, false)) = &self.message {
                    ui.label(text.clone());
                    if self.marker.is_some() && ui.button("Show on board").clicked() {
                        self.mode_sch = false;
                        self.mode3d = false;
                        if let Some(p) = self.marker {
                            self.focus = Some(Focus::Point(p, 3.0));
                            self.needs_fit = false;
                        }
                    }
                }
            }
            return;
        }
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
                RichText::new(
                    "Or drop a board, a Gerber .zip, a folder or several Gerber files here.",
                )
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
                ui.label(
                    RichText::new(format!("Nets: {}", imp.nets_from))
                        .small()
                        .weak(),
                );
                egui::Grid::new("import_files")
                    .num_columns(2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        for (file, role, why) in &imp.files {
                            let short = file.rsplit('/').next().unwrap_or(file);
                            ui.label(RichText::new(short).small()).on_hover_text(file);
                            ui.label(RichText::new(role).small().weak())
                                .on_hover_text(why);
                            ui.end_row();
                        }
                    });
                for w in imp.warnings.iter().take(5) {
                    ui.label(
                        RichText::new(w)
                            .small()
                            .color(Color32::from_rgb(255, 180, 84)),
                    );
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(&net.name).strong().size(15.0));
            if !self.schem.is_empty() && ui.small_button("Show in schematic").clicked() {
                if self.schem.reveal(id, scene) {
                    self.mode_sch = true;
                    self.mode3d = false;
                } else {
                    self.message = Some(("This net is not on any sheet.".into(), true));
                }
            }
        });
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
        let board2d = !self.mode3d && !self.mode_sch;
        let (open_shortcut, fit, flip, esc, measure_key, toggle3d, cm_key, sch_key) = ctx
            .input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::O),
                    !typing && board2d && i.key_pressed(egui::Key::F),
                    !typing && board2d && i.key_pressed(egui::Key::B),
                    i.key_pressed(egui::Key::Escape),
                    !typing && board2d && i.key_pressed(egui::Key::M),
                    !typing && i.key_pressed(egui::Key::Num3),
                    !typing && i.key_pressed(egui::Key::C),
                    !typing && i.key_pressed(egui::Key::S),
                )
            });
        if !typing && ctx.input(|i| i.key_pressed(egui::Key::I)) && self.scene.is_some() {
            self.imp.open = !self.imp.open;
        }
        if sch_key && !self.schem.is_empty() {
            self.mode_sch = !self.mode_sch || self.scene.is_none();
            if self.mode_sch {
                self.mode3d = false;
            }
        }
        if cm_key && self.scene.is_some() {
            self.cm.open = !self.cm.open;
        }
        if toggle3d && self.scene.is_some() {
            self.mode3d = !self.mode3d || self.mode_sch;
            self.mode_sch = false;
        }
        if self.scene.is_none() && !self.schem.is_empty() {
            self.mode_sch = true;
        }
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
        if self.scene.is_some() || !self.schem.is_empty() {
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
                if self.mode_sch {
                    if let Some(h) = &self.schem.hover {
                        ui.label(RichText::new(h).strong());
                    }
                } else if let Some(text) = self.measure.and_then(|m| measure_text(m.a, m.b?)) {
                    ui.label(RichText::new(text).strong().color(MEASURE));
                } else if let Some(h) = &self.hover {
                    ui.label(RichText::new(h).strong());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(scene) = &self.scene {
                        let off = self.imp.failing(&scene.board);
                        if off > 0
                            && ui
                                .link(
                                    RichText::new(format!("{off} pair nets off impedance"))
                                        .color(Color32::from_rgb(255, 180, 84)),
                                )
                                .clicked()
                        {
                            self.imp.open = true;
                        }
                        let failing = self.cm.failing(&scene.board);
                        if failing > 0
                            && ui
                                .link(
                                    RichText::new(format!("{failing} nets fail constraints"))
                                        .color(Color32::from_rgb(240, 96, 96)),
                                )
                                .clicked()
                        {
                            self.cm.open = true;
                        }
                    }
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
            && !self.mode_sch
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
            if self.mode_sch && !self.schem.is_empty() {
                match self.schem.show(ui, self.selected, self.scene.as_ref()) {
                    Some(schview::Pick::BoardNet(n)) => {
                        self.selected = Some(n);
                        self.marker = None;
                        self.message = None;
                    }
                    Some(schview::Pick::Part(d)) => {
                        self.select_part(&d, true);
                    }
                    None => {}
                }
                return;
            }
            if self.scene.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Open a board or schematic with File → Open…, or drop a .PcbDoc, .brd, .SchDoc or .PrjPcb file here.").size(16.0).weak());
                });
                return;
            }
            if self.mode3d {
                if let Some(scene) = &self.scene {
                    view3d::show(&mut self.view3d, scene, ui);
                }
            } else {
                canvas::show(self, ui);
            }
        });
        let pick = match &self.scene {
            Some(scene) => self
                .cm
                .show(ctx, &scene.board)
                .or(self.imp.show(ctx, &scene.board)),
            None => None,
        };
        if let Some(n) = pick {
            self.mode3d = false;
            self.select_net(n, true);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = gl {
            self.view3d.destroy(gl);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let list: Vec<String> = self
            .recent
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        storage.set_string(RECENT_KEY, list.join("\n"));
        storage.set_string(cm::STORE_KEY, self.cm.to_store());
        storage.set_string(imp::STORE_KEY, self.imp.to_store());
    }
}
