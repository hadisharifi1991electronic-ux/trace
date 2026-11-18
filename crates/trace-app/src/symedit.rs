//! The schematic symbol editor: a library (`.SchLib`) of components, each
//! drawn on a canvas with body graphics and pins, a pin table, the
//! component's properties and parameters, parts for multi-part
//! components, and a wizard that lays a symbol out from a pin list.
//! Saved libraries reload in Place symbol, and placed parts can be
//! redrawn from the edited component.

use crate::canvas::View;
use crate::symlib::paint_drawing;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke};
use std::path::{Path, PathBuf};
use trace_altium::SchLibFile;
use trace_core::schematic::PinKind;
use trace_core::symbol::{
    on_part, parse_pin_list, SymHit, SymParam, SymPin, SymShape, SymText, SymbolDef, BODY_YELLOW,
    LINE_BLUE, TEXT_BLUE,
};
use trace_core::{LibSymbol, Pt, SchGeom};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tool {
    #[default]
    Select,
    Pin,
    Line,
    Rect,
    Ellipse,
    Polygon,
    Text,
}

impl Tool {
    const ALL: [(Tool, &'static str, &'static str); 7] = [
        (
            Tool::Select,
            "Select",
            "Pick, drag and edit what is drawn (S)",
        ),
        (
            Tool::Pin,
            "Pin",
            "Place pins: click on the body edge; Space turns the pin; numbers count up (P)",
        ),
        (
            Tool::Line,
            "Line",
            "Click the points of a line; double-click or Enter ends it (L)",
        ),
        (
            Tool::Rect,
            "Rectangle",
            "Click two corners: a body rectangle (R)",
        ),
        (
            Tool::Ellipse,
            "Ellipse",
            "Click two corners of the box it fills (E)",
        ),
        (
            Tool::Polygon,
            "Polygon",
            "Click the corners of a filled polygon; double-click or Enter ends it",
        ),
        (
            Tool::Text,
            "Text",
            "Click to place the text typed beside the tools (T)",
        ),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Draw,
    Pins,
    Component,
    Wizard,
}

/// What the editor asks the rest of the app to do.
pub enum SymAct {
    /// The library was saved here: read it again for placing.
    Saved(PathBuf),
    /// Redraw placed parts of this component (all its parts).
    Update(Vec<LibSymbol>),
    /// Place this part on the current sheet.
    Place(Box<LibSymbol>),
}

pub struct SymEditor {
    pub open: bool,
    lib: SchLibFile,
    path: Option<PathBuf>,
    /// The file had a backup made this session (saving over it the first
    /// time keeps the old one as .bak).
    backed_up: bool,
    cur: usize,
    part: u32,
    tool: Tool,
    tab: Tab,
    sel: Option<SymHit>,
    /// Where a drag was last moved to.
    drag: Option<Pt>,
    /// Points of the shape being drawn.
    pts: Vec<Pt>,
    view: View,
    fit: bool,
    grid: f64,
    undo: Vec<(usize, SymbolDef)>,
    redo: Vec<(usize, SymbolDef)>,
    /// The placeable parts of the current component, and the change count
    /// they were drawn for.
    cache: Option<(u64, usize, Vec<LibSymbol>)>,
    gen: u64,
    /// A text field is being typed into: one undo step for all of it.
    typing: bool,
    filter: String,
    pin_filter: String,
    pin_part_only: bool,
    pub note: Option<String>,
    pub dirty: bool,
    wizard: String,
    wizard_name: String,
    wizard_split: bool,
    pin_orient: u8,
    text_buf: String,
    footprints: String,
    footprints_for: Option<(usize, u64)>,
}

impl Default for SymEditor {
    fn default() -> Self {
        SymEditor {
            open: false,
            lib: SchLibFile::new("NewLibrary"),
            path: None,
            backed_up: false,
            cur: 0,
            part: 1,
            tool: Tool::Select,
            tab: Tab::Draw,
            sel: None,
            drag: None,
            pts: Vec::new(),
            view: View::default(),
            fit: true,
            grid: 100.0,
            undo: Vec::new(),
            redo: Vec::new(),
            cache: None,
            gen: 0,
            typing: false,
            filter: String::new(),
            pin_filter: String::new(),
            pin_part_only: true,
            note: None,
            dirty: false,
            wizard: "1, VDD, power, L\n2, IN, input, L\n3, OUT, output, R\n4, GND, power, R\n"
                .into(),
            wizard_name: "NEW_PART".into(),
            wizard_split: false,
            pin_orient: 2,
            text_buf: "Text".into(),
            footprints: String::new(),
            footprints_for: None,
        }
    }
}

fn snap(p: Pt, g: f64) -> Pt {
    [(p[0] / g).round() * g, (p[1] / g).round() * g]
}

fn kind_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, k: &mut PinKind, w: f32) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(k.label())
        .show_ui(ui, |ui| {
            for o in PinKind::ALL {
                changed |= ui.selectable_value(k, o, o.label()).changed();
            }
        });
    changed
}

const ORIENTS: [&str; 4] = ["right", "up", "left", "down"];

fn orient_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, o: &mut u8, w: f32) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(ORIENTS[(*o % 4) as usize])
        .show_ui(ui, |ui| {
            for (i, n) in ORIENTS.iter().enumerate() {
                changed |= ui.selectable_value(o, i as u8, *n).changed();
            }
        });
    changed
}

impl SymEditor {
    /// Opens library `path` in the editor, at component `name` if given.
    pub fn open_path(&mut self, path: &Path, name: Option<&str>) -> Result<(), String> {
        let lib = trace_altium::read_schlib_file(path).map_err(|e| e.to_string())?;
        self.lib = lib;
        self.path = Some(path.to_path_buf());
        self.backed_up = false;
        self.cur = name
            .and_then(|n| {
                self.lib
                    .symbols
                    .iter()
                    .position(|d| d.name.eq_ignore_ascii_case(n))
            })
            .unwrap_or(0);
        self.after_switch();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.open = true;
        self.note = Some(format!(
            "Opened {} ({} components).",
            path.display(),
            self.lib.symbols.len()
        ));
        Ok(())
    }

    /// Shows a tab by name (draw, pins, component, wizard), for
    /// screenshots.
    pub fn show_tab(&mut self, name: &str) {
        self.tab = match name {
            "pins" => Tab::Pins,
            "component" => Tab::Component,
            "wizard" => Tab::Wizard,
            _ => Tab::Draw,
        };
    }

    fn after_switch(&mut self) {
        self.part = 1;
        self.sel = None;
        self.pts.clear();
        self.fit = true;
        self.cache = None;
        self.gen += 1;
    }

    fn def(&self) -> Option<&SymbolDef> {
        self.lib.symbols.get(self.cur)
    }

    /// One undoable change to the current component.
    fn edit(&mut self, f: impl FnOnce(&mut SymbolDef)) {
        let Some(d) = self.lib.symbols.get_mut(self.cur) else {
            return;
        };
        self.undo.push((self.cur, d.clone()));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        f(d);
        d.touch();
        self.gen += 1;
        self.dirty = true;
    }

    fn undo_step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        if let Some((i, d)) = from.pop() {
            if let Some(slot) = self.lib.symbols.get_mut(i) {
                to.push((i, std::mem::replace(slot, d)));
                self.cur = i;
                self.sel = None;
                self.gen += 1;
                self.dirty = true;
            }
        }
    }

    /// The placeable parts of the current component.
    fn parts_drawn(&mut self) -> &[LibSymbol] {
        let fresh = matches!(&self.cache, Some((g, c, _)) if *g == self.gen && *c == self.cur);
        if !fresh {
            let v = self.lib.lib_symbols(self.cur);
            self.cache = Some((self.gen, self.cur, v));
        }
        &self.cache.as_ref().unwrap().2
    }

    fn save(&mut self, ask: bool) -> Option<PathBuf> {
        let path = match (&self.path, ask) {
            (Some(p), false) => p.clone(),
            _ => rfd::FileDialog::new()
                .add_filter("Altium schematic library", &["SchLib"])
                .set_file_name(format!("{}.SchLib", self.lib.name))
                .save_file()?,
        };
        let problems: Vec<String> = self
            .lib
            .symbols
            .iter()
            .flat_map(|d| {
                d.problems()
                    .into_iter()
                    .map(move |p| format!("{}: {p}", d.name))
            })
            .collect();
        let bytes = match self.lib.write() {
            Ok(b) => b,
            Err(e) => {
                self.note = Some(format!("Could not save: {e}"));
                return None;
            }
        };
        // The first save over a file this session keeps the old one.
        let same = self.path.as_ref() == Some(&path);
        if path.exists() && (!same || !self.backed_up) {
            let bak = path.with_extension("SchLib.bak");
            let _ = std::fs::copy(&path, &bak);
            self.backed_up = true;
        }
        if let Err(e) = std::fs::write(&path, bytes) {
            self.note = Some(format!("Could not save {}: {e}", path.display()));
            return None;
        }
        if let Some(stem) = path.file_stem() {
            self.lib.name = stem.to_string_lossy().into_owned();
        }
        self.path = Some(path.clone());
        self.dirty = false;
        self.note = Some(if problems.is_empty() {
            format!("Saved {}.", path.display())
        } else {
            format!(
                "Saved {}. Check: {}",
                path.display(),
                problems.into_iter().take(3).collect::<Vec<_>>().join(" ")
            )
        });
        Some(path)
    }

    /// The window. Returns what the app should do, when asked.
    pub fn window(&mut self, ctx: &egui::Context) -> Option<SymAct> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut act = None;
        let title = format!(
            "Symbol editor: {}{}",
            self.lib.name,
            if self.dirty { " (not saved)" } else { "" }
        );
        egui::Window::new(title)
            .id(egui::Id::new("symbol_editor"))
            .open(&mut open)
            .default_size([1080.0, 680.0])
            .resizable(true)
            .show(ctx, |ui| act = self.contents(ui));
        self.open = open;
        act
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<SymAct> {
        let mut act = None;
        ui.horizontal(|ui| {
            if ui.button("New library").clicked() {
                *self = SymEditor {
                    open: true,
                    ..Default::default()
                };
                let mut d = SymbolDef::new("Component_1");
                d.touch();
                self.lib.symbols.push(d);
                self.dirty = true;
            }
            if ui
                .button("Open…")
                .on_hover_text("Open an Altium schematic library (.SchLib) to edit")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Altium schematic library", &["SchLib", "schlib"])
                    .pick_file()
                {
                    if let Err(e) = self.open_path(&p, None) {
                        self.note = Some(format!("Could not read {}: {e}", p.display()));
                    }
                }
            }
            let save = ui
                .add_enabled(self.dirty || self.path.is_none(), egui::Button::new("Save"))
                .on_hover_text("Save the library (Ctrl+S). Saving over a file the first time keeps the old one as .SchLib.bak");
            if save.clicked() {
                if let Some(p) = self.save(false) {
                    act = Some(SymAct::Saved(p));
                }
            }
            if ui.button("Save as…").clicked() {
                if let Some(p) = self.save(true) {
                    act = Some(SymAct::Saved(p));
                }
            }
            ui.separator();
            let has = self.def().is_some();
            if ui
                .add_enabled(has, egui::Button::new("Place"))
                .on_hover_text("Place the part shown on the current sheet")
                .clicked()
            {
                let part = self.part;
                if let Some(s) = self.parts_drawn().iter().find(|s| s.part == part).cloned() {
                    act = Some(SymAct::Place(Box::new(s)));
                }
            }
            if ui
                .add_enabled(has, egui::Button::new("Update sheets"))
                .on_hover_text("Redraw every placed part of this component from it, on all open sheets: body, pins and texts change, designators and values stay, wires follow moved pins")
                .clicked()
            {
                act = Some(SymAct::Update(self.parts_drawn().to_vec()));
            }
            ui.separator();
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                self.undo_step(true);
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).clicked() {
                self.undo_step(false);
            }
        });
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).weak().small());
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            if let Some(p) = self.save(false) {
                act = Some(SymAct::Saved(p));
            }
        }
        ui.separator();
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(200.0, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.component_list(ui),
            );
            ui.separator();
            ui.vertical(|ui| {
                if self.def().is_none() {
                    ui.label(
                        RichText::new("No component. Add one with New, or open a library.").weak(),
                    );
                    return;
                }
                ui.horizontal(|ui| {
                    for (t, n) in [
                        (Tab::Draw, "Draw"),
                        (Tab::Pins, "Pins"),
                        (Tab::Component, "Component"),
                        (Tab::Wizard, "Symbol wizard"),
                    ] {
                        ui.selectable_value(&mut self.tab, t, n);
                    }
                    ui.separator();
                    self.part_bar(ui);
                });
                ui.separator();
                match self.tab {
                    Tab::Draw => self.draw_tab(ui),
                    Tab::Pins => self.pins_tab(ui),
                    Tab::Component => self.component_tab(ui),
                    Tab::Wizard => self.wizard_tab(ui),
                }
            });
        });
        act
    }

    fn component_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(140.0));
        });
        ui.horizontal(|ui| {
            if ui
                .button("New")
                .on_hover_text("Add an empty component")
                .clicked()
            {
                let mut n = self.lib.symbols.len() + 1;
                while self
                    .lib
                    .symbols
                    .iter()
                    .any(|d| d.name == format!("Component_{n}"))
                {
                    n += 1;
                }
                let mut d = SymbolDef::new(&format!("Component_{n}"));
                d.touch();
                self.lib.symbols.push(d);
                self.cur = self.lib.symbols.len() - 1;
                self.after_switch();
                self.dirty = true;
            }
            if ui
                .button("Copy")
                .on_hover_text("Add a copy of this component")
                .clicked()
            {
                if let Some(d) = self.def().cloned() {
                    let mut d = d;
                    d.name = format!("{}_copy", d.name);
                    d.touch();
                    self.lib.symbols.insert(self.cur + 1, d);
                    self.cur += 1;
                    self.after_switch();
                    self.dirty = true;
                }
            }
            if ui
                .add_enabled(self.def().is_some(), egui::Button::new("Delete"))
                .on_hover_text("Remove this component from the library")
                .clicked()
            {
                self.lib.symbols.remove(self.cur);
                self.undo.clear();
                self.redo.clear();
                self.cur = self.cur.min(self.lib.symbols.len().saturating_sub(1));
                self.after_switch();
                self.dirty = true;
            }
        });
        let want = self.filter.trim().to_ascii_lowercase();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("sym_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, d) in self.lib.symbols.iter().enumerate() {
                    if !want.is_empty()
                        && !d.name.to_ascii_lowercase().contains(&want)
                        && !d.description.to_ascii_lowercase().contains(&want)
                    {
                        continue;
                    }
                    let label = if d.parts > 1 {
                        format!("{} ({} parts)", d.name, d.parts)
                    } else {
                        d.name.clone()
                    };
                    if ui
                        .selectable_label(i == self.cur, label)
                        .on_hover_text(&d.description)
                        .clicked()
                    {
                        pick = Some(i);
                    }
                }
            });
        if let Some(i) = pick {
            if i != self.cur {
                self.cur = i;
                self.after_switch();
            }
        }
    }

    fn part_bar(&mut self, ui: &mut egui::Ui) {
        let parts = self.def().map(|d| d.parts).unwrap_or(1);
        ui.label("Part");
        for p in 1..=parts {
            if ui
                .selectable_label(self.part == p, SymbolDef::letter(p).to_string())
                .clicked()
            {
                self.part = p;
                self.sel = None;
                self.fit = true;
            }
        }
        if ui
            .small_button("+")
            .on_hover_text("Add an empty part")
            .clicked()
        {
            let mut n = 0;
            self.edit(|d| n = d.add_part());
            self.part = n;
            self.fit = true;
        }
        if ui
            .small_button("Duplicate")
            .on_hover_text(
                "Add a part drawn like this one, with new pin numbers (supply pins keep theirs)",
            )
            .clicked()
        {
            let part = self.part;
            let mut n = part;
            self.edit(|d| n = d.duplicate_part(part));
            self.part = n;
        }
        if ui
            .add_enabled(parts > 1, egui::Button::new("Remove part").small())
            .on_hover_text("Remove this part and its pins")
            .clicked()
        {
            let part = self.part;
            self.edit(|d| d.remove_part(part));
            self.part = self.part.min(parts - 1).max(1);
            self.sel = None;
        }
    }

    // ---- drawing -------------------------------------------------------

    fn draw_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (t, n, tip) in Tool::ALL {
                if ui
                    .selectable_label(self.tool == t, n)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.tool = t;
                    self.pts.clear();
                }
            }
            if self.tool == Tool::Text {
                ui.add(egui::TextEdit::singleline(&mut self.text_buf).desired_width(100.0));
            }
            if self.tool == Tool::Pin {
                ui.label("points");
                orient_combo(ui, "new_pin_orient", &mut self.pin_orient, 60.0);
            }
            ui.separator();
            ui.label("Grid");
            egui::ComboBox::from_id_salt("sym_grid")
                .width(60.0)
                .selected_text(format!("{} mil", self.grid))
                .show_ui(ui, |ui| {
                    for g in [10.0, 50.0, 100.0] {
                        ui.selectable_value(&mut self.grid, g, format!("{g} mil"));
                    }
                });
            if ui.button("Fit").clicked() {
                self.fit = true;
            }
        });
        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2((avail.x - 280.0).max(200.0), avail.y.max(200.0)),
                Sense::click_and_drag(),
            );
            self.canvas(ui, rect, &resp);
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(260.0);
                egui::ScrollArea::vertical()
                    .id_salt("sym_props")
                    .show(ui, |ui| self.properties(ui));
            });
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response) {
        self.view.rect = rect;
        let part = self.part;
        if self.fit {
            if let Some(d) = self.def() {
                let (a, b) = d.bounds(part);
                let mut bb = [a[0].min(0.0), a[1].min(0.0), b[0].max(0.0), b[1].max(0.0)];
                for p in [d.designator_at, d.comment_at] {
                    bb = [
                        bb[0].min(p[0]),
                        bb[1].min(p[1]),
                        bb[2].max(p[0]),
                        bb[3].max(p[1]),
                    ];
                }
                self.view.fit_rect(
                    [
                        bb[0] as f32 - 200.0,
                        bb[1] as f32 - 200.0,
                        bb[2] as f32 + 200.0,
                        bb[3] as f32 + 200.0,
                    ],
                    0.9,
                );
            }
            self.fit = false;
        }
        // Zoom about the pointer; the right or middle button pans.
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let k = zoom * (scroll * 0.0025).exp();
            if (k - 1.0).abs() > 1e-4 {
                if let Some(p) = resp.hover_pos() {
                    let (px, py) = (p.x - rect.min.x, p.y - rect.min.y);
                    self.view.tx = px - (px - self.view.tx) * k;
                    self.view.ty = py - (py - self.view.ty) * k;
                    self.view.scale *= k;
                }
            }
        }
        if resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
        {
            self.view.tx += resp.drag_delta().x;
            self.view.ty += resp.drag_delta().y;
        }
        let board = |v: &View, p: Pos2| {
            let b = v.to_board(p);
            [b[0] as f64, b[1] as f64]
        };
        let hover = resp.hover_pos().map(|p| board(&self.view, p));
        let g = self.grid;
        let tol = (6.0 / self.view.scale as f64).max(5.0);

        // Keys.
        let focus_free = ui.ctx().memory(|m| m.focused().is_none());
        if resp.hovered() || resp.has_focus() || focus_free {
            let (del, space, esc, enter, undo, redo) = ui.input_mut(|i| {
                (
                    focus_free
                        && (i.key_pressed(egui::Key::Delete)
                            || i.key_pressed(egui::Key::Backspace)),
                    focus_free && i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Enter),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y),
                )
            });
            if focus_free && resp.hovered() {
                let keys = ui.input(|i| {
                    [
                        (egui::Key::S, Tool::Select),
                        (egui::Key::P, Tool::Pin),
                        (egui::Key::L, Tool::Line),
                        (egui::Key::R, Tool::Rect),
                        (egui::Key::E, Tool::Ellipse),
                        (egui::Key::T, Tool::Text),
                    ]
                    .into_iter()
                    .find(|(k, _)| i.key_pressed(*k) && !i.modifiers.command)
                    .map(|(_, t)| t)
                });
                if let Some(t) = keys {
                    self.tool = t;
                    self.pts.clear();
                }
            }
            if undo {
                self.undo_step(true);
            }
            if redo {
                self.undo_step(false);
            }
            if del {
                if let Some(h) = self.sel.take() {
                    self.edit(|d| d.delete_hit(h));
                }
            }
            if space {
                if self.tool == Tool::Pin {
                    self.pin_orient = (self.pin_orient + 1) % 4;
                } else if let Some(h) = self.sel {
                    self.edit(|d| d.turn_hit(h));
                }
            }
            if esc {
                if !self.pts.is_empty() {
                    self.pts.clear();
                } else {
                    self.tool = Tool::Select;
                    self.sel = None;
                }
            }
            if enter {
                self.finish_shape();
            }
        }

        // Clicks and drags.
        if let Some(p) = resp
            .interact_pointer_pos()
            .filter(|_| resp.clicked_by(egui::PointerButton::Primary))
        {
            let b = board(&self.view, p);
            let s = snap(b, g);
            match self.tool {
                Tool::Select => self.sel = self.def().and_then(|d| d.hit(b, part, tol)),
                Tool::Pin => {
                    let o = self.pin_orient;
                    let mut i = 0;
                    self.edit(|d| i = d.add_pin(part, s, o));
                    self.sel = Some(SymHit::Pin(i));
                }
                Tool::Text => {
                    let text = self.text_buf.clone();
                    self.edit(|d| {
                        d.texts.push(SymText {
                            part,
                            at: s,
                            text,
                            size: 100.0,
                            rot: 0,
                            just: 0,
                            color: TEXT_BLUE,
                        })
                    });
                }
                Tool::Rect | Tool::Ellipse => {
                    if self.pts.is_empty() {
                        self.pts.push(s);
                    } else {
                        self.pts.push(s);
                        self.finish_shape();
                    }
                }
                Tool::Line | Tool::Polygon => {
                    if self.pts.last() != Some(&s) {
                        self.pts.push(s);
                    }
                }
            }
        }
        if resp.double_clicked() && matches!(self.tool, Tool::Line | Tool::Polygon) {
            self.finish_shape();
        }
        if self.tool == Tool::Select && resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
                let b = board(&self.view, o);
                let hit = self.def().and_then(|d| d.hit(b, part, tol));
                self.sel = hit;
                if hit.is_some() {
                    self.drag = Some(snap(b, g));
                    // One undo step for the whole drag.
                    if let Some(d) = self.def().cloned() {
                        self.undo.push((self.cur, d));
                        self.redo.clear();
                    }
                }
            }
        }
        if let (Some(from), Some(h), Some(p)) = (self.drag, self.sel, hover) {
            if resp.dragged_by(egui::PointerButton::Primary) {
                let to = snap(p, g);
                let dd = [to[0] - from[0], to[1] - from[1]];
                if dd != [0.0, 0.0] {
                    if let Some(d) = self.lib.symbols.get_mut(self.cur) {
                        d.move_hit(h, dd);
                        self.gen += 1;
                        self.dirty = true;
                    }
                    self.drag = Some(to);
                }
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }

        // Paint.
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(255, 252, 240));
        let v = self.view;
        let to = |p: Pt| v.to_screen(p[0] as f32, p[1] as f32);
        // Grid dots where they are not too dense.
        let step = if g * v.scale as f64 >= 8.0 { g } else { 100.0 };
        if step * v.scale as f64 >= 6.0 {
            let vis = v.visible();
            let (x0, x1) = (
                (vis[0] as f64 / step).floor() as i64,
                (vis[2] as f64 / step).ceil() as i64,
            );
            let (y0, y1) = (
                (vis[1] as f64 / step).floor() as i64,
                (vis[3] as f64 / step).ceil() as i64,
            );
            if (x1 - x0) * (y1 - y0) < 40_000 {
                let c = Color32::from_rgb(210, 205, 190);
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        painter.circle_filled(to([x as f64 * step, y as f64 * step]), 0.8, c);
                    }
                }
            }
        }
        // Origin.
        let o = to([0.0, 0.0]);
        let oc = Stroke::new(1.0_f32, Color32::from_rgb(200, 60, 60));
        painter.line_segment([o - egui::vec2(8.0, 0.0), o + egui::vec2(8.0, 0.0)], oc);
        painter.line_segment([o - egui::vec2(0.0, 8.0), o + egui::vec2(0.0, 8.0)], oc);
        let drawn = self.parts_drawn().iter().find(|s| s.part == part).cloned();
        if let Some(s) = &drawn {
            paint_drawing(&painter, &v, &s.drawing);
        }
        let Some(d) = self.def().cloned() else { return };
        // Hidden pins are left out of the drawing: shown faint here.
        for q in d.pins.iter().filter(|q| q.hidden && on_part(q.part, part)) {
            painter.line_segment(
                [to(q.base), to(q.hot())],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 0, 0, 70)),
            );
            painter.text(
                to(q.hot()),
                egui::Align2::CENTER_BOTTOM,
                format!("{} {} (hidden)", q.number, q.name),
                FontId::proportional(10.0),
                Color32::from_gray(140),
            );
        }
        // Hot ends: where wires connect.
        for q in d.pins.iter().filter(|q| on_part(q.part, part)) {
            let h = to(q.hot());
            let c = Color32::from_rgb(170, 0, 0);
            painter.line_segment(
                [h - egui::vec2(3.0, 3.0), h + egui::vec2(3.0, 3.0)],
                Stroke::new(1.0_f32, c),
            );
            painter.line_segment(
                [h - egui::vec2(3.0, -3.0), h + egui::vec2(3.0, -3.0)],
                Stroke::new(1.0_f32, c),
            );
        }
        // The selection.
        let hl = Stroke::new(3.0_f32, Color32::from_rgba_unmultiplied(255, 120, 0, 200));
        match self.sel {
            Some(SymHit::Pin(i)) => {
                if let Some(q) = d.pins.get(i) {
                    painter.line_segment([to(q.base), to(q.hot())], hl);
                }
            }
            Some(SymHit::Shape(i)) => {
                if let Some(s) = d.shapes.get(i) {
                    outline(&painter, &v, &s.geom, hl);
                }
            }
            Some(SymHit::Text(i)) => {
                if let Some(t) = d.texts.get(i) {
                    painter.circle_stroke(to(t.at), 5.0, hl);
                }
            }
            Some(SymHit::Designator) => {
                painter.circle_stroke(to(d.designator_at), 5.0, hl);
            }
            Some(SymHit::Comment) => {
                painter.circle_stroke(to(d.comment_at), 5.0, hl);
            }
            None => {}
        }
        // What is being drawn.
        if let Some(p) = hover {
            let s = snap(p, g);
            let ghost = Stroke::new(1.5_f32, Color32::from_rgb(0, 120, 200));
            match self.tool {
                Tool::Pin => {
                    let q = SymPin::new(part, s, self.pin_orient, "", "");
                    painter.line_segment([to(q.base), to(q.hot())], ghost);
                    painter.circle_filled(to(q.base), 2.5, ghost.color);
                }
                Tool::Rect | Tool::Ellipse if !self.pts.is_empty() => {
                    let a = self.pts[0];
                    let g = if self.tool == Tool::Rect {
                        rect_geom(a, s)
                    } else {
                        ellipse_geom(a, s)
                    };
                    outline(&painter, &v, &g, ghost);
                }
                Tool::Line | Tool::Polygon if !self.pts.is_empty() => {
                    let mut pts: Vec<Pos2> = self.pts.iter().map(|p| to(*p)).collect();
                    pts.push(to(s));
                    if self.tool == Tool::Polygon {
                        pts.push(to(self.pts[0]));
                    }
                    painter.add(egui::Shape::line(pts, ghost));
                }
                _ => {}
            }
            painter.text(
                rect.left_bottom() + egui::vec2(6.0, -6.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.0}, {:.0} mil", s[0], s[1]),
                FontId::monospace(11.0),
                Color32::from_gray(90),
            );
        }
        let hint = match self.tool {
            Tool::Select => {
                "Click to pick, drag to move; Space turns, Delete removes; right-drag pans"
            }
            Tool::Pin => {
                "Click to place a pin; Space turns it; the hot end (x) is where wires connect"
            }
            Tool::Line | Tool::Polygon => "Click points; double-click or Enter ends; Esc drops",
            Tool::Rect | Tool::Ellipse => "Click two corners",
            Tool::Text => "Click to place the text",
        };
        painter.text(
            rect.left_top() + egui::vec2(6.0, 6.0),
            egui::Align2::LEFT_TOP,
            hint,
            FontId::proportional(11.0),
            Color32::from_gray(110),
        );
    }

    fn finish_shape(&mut self) {
        let pts = std::mem::take(&mut self.pts);
        let part = self.part;
        let geom = match self.tool {
            Tool::Rect if pts.len() >= 2 => rect_geom(pts[0], pts[1]),
            Tool::Ellipse if pts.len() >= 2 => ellipse_geom(pts[0], pts[1]),
            Tool::Line if pts.len() >= 2 => SchGeom::Line(pts),
            Tool::Polygon if pts.len() >= 3 => SchGeom::Poly(pts),
            _ => return,
        };
        let filled = matches!(self.tool, Tool::Rect | Tool::Polygon);
        let mut i = 0;
        self.edit(|d| {
            d.shapes.push(SymShape {
                part,
                geom,
                width: 10.0,
                stroke: LINE_BLUE,
                fill: filled.then_some(BODY_YELLOW),
            });
            i = d.shapes.len() - 1;
        });
        self.sel = Some(SymHit::Shape(i));
    }

    // ---- properties ----------------------------------------------------

    /// Ends one undo step per edit: `before` is pushed the first frame a
    /// field changes, and typing on in it stays the same step.
    fn changed(&mut self, before: SymbolDef, changed: bool, ui: &egui::Ui) {
        if changed {
            if !self.typing {
                self.undo.push((self.cur, before));
                self.redo.clear();
            }
            self.typing = true;
            if let Some(d) = self.lib.symbols.get_mut(self.cur) {
                d.touch();
            }
            self.gen += 1;
            self.dirty = true;
        }
        if ui.ctx().memory(|m| m.focused().is_none()) && !ui.ctx().input(|i| i.pointer.any_down()) {
            self.typing = false;
        }
    }

    fn properties(&mut self, ui: &mut egui::Ui) {
        let Some(before) = self.def().cloned() else {
            return;
        };
        let sel = self.sel;
        let parts = before.parts;
        let mut ch = false;
        let d = &mut self.lib.symbols[self.cur];
        match sel {
            Some(SymHit::Pin(i)) if i < d.pins.len() => {
                ui.label(RichText::new("Pin").strong());
                let p = &mut d.pins[i];
                egui::Grid::new("pin_props").num_columns(2).show(ui, |ui| {
                    ui.label("Number");
                    ch |= ui.text_edit_singleline(&mut p.number).changed();
                    ui.end_row();
                    ui.label("Name");
                    ch |= ui
                        .text_edit_singleline(&mut p.name)
                        .on_hover_text(r"A backslash after a letter draws a bar over it: R\S\T\ for an active-low RST")
                        .changed();
                    ui.end_row();
                    ui.label("Type");
                    ch |= kind_combo(ui, "pin_kind", &mut p.kind, 140.0);
                    ui.end_row();
                    ui.label("Points");
                    ch |= orient_combo(ui, "pin_orient", &mut p.orient, 140.0);
                    ui.end_row();
                    ui.label("Length (mil)");
                    ch |= ui
                        .add(egui::DragValue::new(&mut p.len).range(0.0..=2000.0).speed(10.0))
                        .changed();
                    ui.end_row();
                    ui.label("At (mil)");
                    ui.horizontal(|ui| {
                        ch |= ui.add(egui::DragValue::new(&mut p.base[0]).speed(10.0)).changed();
                        ch |= ui.add(egui::DragValue::new(&mut p.base[1]).speed(10.0)).changed();
                    });
                    ui.end_row();
                    ui.label("Part");
                    let mut part = p.part;
                    egui::ComboBox::from_id_salt("pin_part")
                        .width(140.0)
                        .selected_text(if part == 0 { "every part".to_string() } else { SymbolDef::letter(part).to_string() })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut part, 0, "every part");
                            for k in 1..=parts {
                                ui.selectable_value(&mut part, k, SymbolDef::letter(k).to_string());
                            }
                        });
                    if part != p.part {
                        p.part = part;
                        ch = true;
                    }
                    ui.end_row();
                    ui.label("Description");
                    ch |= ui.text_edit_singleline(&mut p.description).changed();
                    ui.end_row();
                });
                ch |= ui.checkbox(&mut p.show_name, "Show name").changed();
                ch |= ui.checkbox(&mut p.show_number, "Show number").changed();
                ch |= ui
                    .checkbox(&mut p.hidden, "Hidden (still connects by name)")
                    .changed();
                ch |= ui.checkbox(&mut p.dot, "Negated (bubble)").changed();
                ch |= ui.checkbox(&mut p.clock, "Clock (wedge inside)").changed();
            }
            Some(SymHit::Shape(i)) if i < d.shapes.len() => {
                let s = &mut d.shapes[i];
                let kind = match &s.geom {
                    SchGeom::Line(_) => "Line",
                    SchGeom::Poly(_) => "Polygon",
                    SchGeom::Rect { .. } => "Rectangle",
                    SchGeom::Arc { .. } => "Ellipse or arc",
                };
                ui.label(RichText::new(kind).strong());
                egui::Grid::new("shape_props")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Line");
                        egui::ComboBox::from_id_salt("shape_w")
                            .selected_text(match s.width as i64 {
                                0 => "smallest",
                                1..=15 => "small",
                                16..=40 => "medium",
                                _ => "large",
                            })
                            .show_ui(ui, |ui| {
                                for (w, n) in [
                                    (0.0, "smallest"),
                                    (10.0, "small"),
                                    (30.0, "medium"),
                                    (50.0, "large"),
                                ] {
                                    ch |= ui.selectable_value(&mut s.width, w, n).changed();
                                }
                            });
                        ui.end_row();
                        ui.label("Colour");
                        ch |= ui.color_edit_button_srgb(&mut s.stroke).changed();
                        ui.end_row();
                        if !matches!(s.geom, SchGeom::Line(_)) {
                            ui.label("Fill");
                            ui.horizontal(|ui| {
                                let mut on = s.fill.is_some();
                                if ui.checkbox(&mut on, "").changed() {
                                    s.fill = on.then_some(BODY_YELLOW);
                                    ch = true;
                                }
                                if let Some(f) = &mut s.fill {
                                    ch |= ui.color_edit_button_srgb(f).changed();
                                }
                            });
                            ui.end_row();
                        }
                        if let SchGeom::Rect { min, max, .. } = &mut s.geom {
                            ui.label("From");
                            ui.horizontal(|ui| {
                                ch |= ui
                                    .add(egui::DragValue::new(&mut min[0]).speed(10.0))
                                    .changed();
                                ch |= ui
                                    .add(egui::DragValue::new(&mut min[1]).speed(10.0))
                                    .changed();
                            });
                            ui.end_row();
                            ui.label("To");
                            ui.horizontal(|ui| {
                                ch |= ui
                                    .add(egui::DragValue::new(&mut max[0]).speed(10.0))
                                    .changed();
                                ch |= ui
                                    .add(egui::DragValue::new(&mut max[1]).speed(10.0))
                                    .changed();
                            });
                            ui.end_row();
                        }
                        ui.label("Part");
                        let mut part = s.part;
                        egui::ComboBox::from_id_salt("shape_part")
                            .selected_text(if part == 0 {
                                "every part".to_string()
                            } else {
                                SymbolDef::letter(part).to_string()
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut part, 0, "every part");
                                for k in 1..=parts {
                                    ui.selectable_value(
                                        &mut part,
                                        k,
                                        SymbolDef::letter(k).to_string(),
                                    );
                                }
                            });
                        if part != s.part {
                            s.part = part;
                            ch = true;
                        }
                        ui.end_row();
                    });
            }
            Some(SymHit::Text(i)) if i < d.texts.len() => {
                ui.label(RichText::new("Text").strong());
                let t = &mut d.texts[i];
                ch |= ui.text_edit_singleline(&mut t.text).changed();
                ui.horizontal(|ui| {
                    ui.label("Size (mil)");
                    ch |= ui
                        .add(
                            egui::DragValue::new(&mut t.size)
                                .range(20.0..=500.0)
                                .speed(10.0),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Colour");
                    ch |= ui.color_edit_button_srgb(&mut t.color).changed();
                });
            }
            Some(SymHit::Designator) => {
                ui.label(RichText::new("Designator").strong());
                ch |= ui.text_edit_singleline(&mut d.designator).changed();
            }
            Some(SymHit::Comment) => {
                ui.label(RichText::new("Comment").strong());
                ch |= ui
                    .text_edit_singleline(&mut d.comment)
                    .on_hover_text("* shows the component's name")
                    .changed();
                ch |= ui.checkbox(&mut d.show_comment, "Shown").changed();
            }
            _ => {
                ui.label(RichText::new(&d.name).strong());
                if !d.description.is_empty() {
                    ui.label(RichText::new(&d.description).weak().small());
                }
                let n = d.pins.iter().filter(|p| on_part(p.part, self.part)).count();
                ui.label(format!(
                    "Part {} of {} · {} pins here, {} in all",
                    SymbolDef::letter(self.part),
                    d.parts,
                    n,
                    d.pins.len()
                ));
                ui.label(RichText::new("Pick something to edit it here.").weak());
            }
        }
        let probs = d.problems();
        if !probs.is_empty() {
            ui.separator();
            for p in probs.iter().take(6) {
                ui.label(
                    RichText::new(p)
                        .color(Color32::from_rgb(190, 90, 0))
                        .small(),
                );
            }
        }
        self.changed(before, ch, ui);
    }

    // ---- pin table -----------------------------------------------------

    fn pins_tab(&mut self, ui: &mut egui::Ui) {
        let Some(before) = self.def().cloned() else {
            return;
        };
        let part = self.part;
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.add(egui::TextEdit::singleline(&mut self.pin_filter).desired_width(120.0));
            ui.checkbox(
                &mut self.pin_part_only,
                format!("Part {} only", SymbolDef::letter(part)),
            );
        });
        let mut ch = false;
        let mut remove = None;
        let mut add = false;
        let mut sort = false;
        ui.horizontal(|ui| {
            add = ui
                .button("Add pin")
                .on_hover_text("A new pin below the lowest on the left edge")
                .clicked();
            sort = ui.button("Sort by number").clicked();
            ui.label(RichText::new(format!("{} pins", before.pins.len())).weak());
        });
        let want = self.pin_filter.trim().to_ascii_lowercase();
        let rows: Vec<usize> = before
            .pins
            .iter()
            .enumerate()
            .filter(|(_, p)| !self.pin_part_only || on_part(p.part, part))
            .filter(|(_, p)| {
                want.is_empty()
                    || p.number.to_ascii_lowercase().contains(&want)
                    || p.name.to_ascii_lowercase().contains(&want)
            })
            .map(|(i, _)| i)
            .collect();
        let widths = [
            36.0, 60.0, 150.0, 110.0, 64.0, 60.0, 40.0, 40.0, 40.0, 40.0, 40.0, 24.0,
        ];
        let heads = [
            "Part", "Number", "Name", "Type", "Points", "Length", "Name", "No.", "Hide", "Dot",
            "Clk", "",
        ];
        ui.horizontal(|ui| {
            for (w, h) in widths.iter().zip(heads) {
                ui.add_sized([*w, 16.0], egui::Label::new(RichText::new(h).strong()));
            }
        });
        let parts = before.parts;
        let sel_pin = match self.sel {
            Some(SymHit::Pin(i)) => Some(i),
            _ => None,
        };
        let mut pick = None;
        let d = &mut self.lib.symbols[self.cur];
        egui::ScrollArea::vertical()
            .id_salt("pin_table")
            .auto_shrink([false, false])
            .show_rows(ui, 22.0, rows.len(), |ui, range| {
                for &i in &rows[range] {
                    let p = &mut d.pins[i];
                    ui.horizontal(|ui| {
                        let mut part_txt = if p.part == 0 {
                            "all".to_string()
                        } else {
                            SymbolDef::letter(p.part).to_string()
                        };
                        let r = ui.add_sized(
                            [widths[0], 20.0],
                            egui::SelectableLabel::new(sel_pin == Some(i), &part_txt),
                        );
                        if r.clicked() {
                            pick = Some(i);
                        }
                        r.context_menu(|ui| {
                            for k in 0..=parts {
                                let n = if k == 0 {
                                    "every part".to_string()
                                } else {
                                    format!("Part {}", SymbolDef::letter(k))
                                };
                                if ui.button(n).clicked() {
                                    p.part = k;
                                    ch = true;
                                    ui.close_menu();
                                }
                            }
                        });
                        part_txt.clear();
                        ch |= ui
                            .add_sized([widths[1], 20.0], egui::TextEdit::singleline(&mut p.number))
                            .changed();
                        ch |= ui
                            .add_sized([widths[2], 20.0], egui::TextEdit::singleline(&mut p.name))
                            .changed();
                        ch |= kind_combo(ui, ("pk", i), &mut p.kind, widths[3] - 10.0);
                        ch |= orient_combo(ui, ("po", i), &mut p.orient, widths[4] - 10.0);
                        ch |= ui
                            .add_sized(
                                [widths[5], 20.0],
                                egui::DragValue::new(&mut p.len)
                                    .range(0.0..=2000.0)
                                    .speed(10.0),
                            )
                            .changed();
                        for (k, v) in [
                            &mut p.show_name,
                            &mut p.show_number,
                            &mut p.hidden,
                            &mut p.dot,
                            &mut p.clock,
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            ch |= ui
                                .add_sized([widths[6 + k], 20.0], egui::Checkbox::without_text(v))
                                .changed();
                        }
                        if ui
                            .small_button("✕")
                            .on_hover_text("Remove this pin")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
            });
        if let Some(i) = pick {
            self.sel = Some(SymHit::Pin(i));
        }
        let d = &mut self.lib.symbols[self.cur];
        if let Some(i) = remove {
            d.pins.remove(i);
            self.sel = None;
            ch = true;
        }
        if add {
            let low = d
                .pins
                .iter()
                .filter(|p| on_part(p.part, part) && p.orient == 2)
                .map(|p| p.base[1])
                .fold(f64::MAX, f64::min);
            let y = if low == f64::MAX { -100.0 } else { low - 100.0 };
            let n = d.next_number();
            d.pins.push(SymPin::new(part, [0.0, y], 2, &n, &n));
            ch = true;
        }
        if sort {
            d.sort_pins();
            self.sel = None;
            ch = true;
        }
        self.changed(before, ch, ui);
    }

    // ---- component -----------------------------------------------------

    fn component_tab(&mut self, ui: &mut egui::Ui) {
        let Some(before) = self.def().cloned() else {
            return;
        };
        let mut ch = false;
        if self.footprints_for != Some((self.cur, self.gen)) && !self.typing {
            self.footprints = before.footprints.join(", ");
        }
        let d = &mut self.lib.symbols[self.cur];
        egui::Grid::new("comp_props").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ch |= ui
                .add(egui::TextEdit::singleline(&mut d.name).desired_width(300.0))
                .on_hover_text("The library reference placed parts point back to")
                .changed();
            ui.end_row();
            ui.label("Description");
            ch |= ui
                .add(egui::TextEdit::singleline(&mut d.description).desired_width(300.0))
                .changed();
            ui.end_row();
            ui.label("Designator");
            ch |= ui
                .add(egui::TextEdit::singleline(&mut d.designator).desired_width(80.0))
                .changed();
            ui.end_row();
            ui.label("Comment");
            ui.horizontal(|ui| {
                ch |= ui
                    .add(egui::TextEdit::singleline(&mut d.comment).desired_width(160.0))
                    .on_hover_text("The value placed parts show; * shows the name")
                    .changed();
                ch |= ui.checkbox(&mut d.show_comment, "shown").changed();
            });
            ui.end_row();
            ui.label("Footprints");
            let r = ui
                .add(egui::TextEdit::singleline(&mut self.footprints).desired_width(300.0))
                .on_hover_text("PCB footprints, comma separated; the first is the one used");
            if r.changed() {
                d.footprints = self
                    .footprints
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                ch = true;
            }
            ui.end_row();
            ui.label("Parts");
            ui.label(d.parts.to_string());
            ui.end_row();
        });
        ui.separator();
        ui.label(RichText::new("Parameters").strong());
        let mut remove = None;
        egui::Grid::new("params")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                ui.label("Name");
                ui.label("Value");
                ui.label("Shown");
                ui.label("");
                ui.end_row();
                for (i, p) in d.params.iter_mut().enumerate() {
                    ch |= ui
                        .add(egui::TextEdit::singleline(&mut p.name).desired_width(160.0))
                        .changed();
                    ch |= ui
                        .add(egui::TextEdit::singleline(&mut p.value).desired_width(260.0))
                        .changed();
                    ch |= ui.checkbox(&mut p.visible, "").changed();
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            d.params.remove(i);
            ch = true;
        }
        ui.horizontal(|ui| {
            for name in [
                "Manufacturer",
                "Manufacturer Part Number",
                "Datasheet",
                "Value",
            ] {
                if !d.params.iter().any(|p| p.name == name)
                    && ui.small_button(format!("+ {name}")).clicked()
                {
                    d.params.push(SymParam {
                        name: name.into(),
                        value: String::new(),
                        visible: false,
                        at: [0.0, 0.0],
                        raw: Vec::new(),
                    });
                    ch = true;
                }
            }
            if ui.small_button("+ Parameter").clicked() {
                d.params.push(SymParam {
                    name: format!("Param{}", d.params.len() + 1),
                    value: String::new(),
                    visible: false,
                    at: [0.0, 0.0],
                    raw: Vec::new(),
                });
                ch = true;
            }
        });
        let probs = d.problems();
        ui.separator();
        if probs.is_empty() {
            ui.label(RichText::new("No problems found.").weak());
        }
        for p in probs {
            ui.label(RichText::new(p).color(Color32::from_rgb(190, 90, 0)));
        }
        if ch {
            self.footprints_for = Some((self.cur, self.gen + 1));
        }
        self.changed(before, ch, ui);
    }

    // ---- wizard --------------------------------------------------------

    fn wizard_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Paste a pin list, one pin a line: number, name, type, side (L, R, T or B), split by commas or tabs, as copied from a datasheet table or spreadsheet. Type and side may be left out.");
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.wizard_name);
            ui.checkbox(&mut self.wizard_split, "Power pins on a part of their own");
        });
        let pins = parse_pin_list(&self.wizard);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!pins.is_empty(), egui::Button::new("Add as new component"))
                .clicked()
            {
                let mut d = SymbolDef::from_pin_list(self.wizard_name.trim(), &pins);
                if self.wizard_split {
                    d.split_power_part();
                }
                d.touch();
                self.lib.symbols.push(d);
                self.cur = self.lib.symbols.len() - 1;
                self.after_switch();
                self.dirty = true;
                self.tab = Tab::Draw;
            }
            if ui
                .add_enabled(
                    !pins.is_empty() && self.def().is_some(),
                    egui::Button::new("Replace this component's body and pins"),
                )
                .on_hover_text("Keeps its name, parameters and footprints")
                .clicked()
            {
                let split = self.wizard_split;
                self.edit(|d| {
                    let mut n = SymbolDef::from_pin_list(&d.name, &pins);
                    if split {
                        n.split_power_part();
                    }
                    d.parts = n.parts;
                    d.pins = n.pins;
                    d.shapes = n.shapes;
                    d.texts.clear();
                    d.designator_at = n.designator_at;
                    d.comment_at = n.comment_at;
                });
                self.after_switch();
                self.tab = Tab::Draw;
            }
            ui.label(RichText::new(format!("{} pins read", pins.len())).weak());
        });
        egui::ScrollArea::vertical()
            .id_salt("wizard_text")
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.wizard)
                        .code_editor()
                        .desired_rows(24)
                        .desired_width(f32::INFINITY),
                );
            });
    }
}

fn rect_geom(a: Pt, b: Pt) -> SchGeom {
    SchGeom::Rect {
        min: [a[0].min(b[0]), a[1].min(b[1])],
        max: [a[0].max(b[0]), a[1].max(b[1])],
        r: [0.0, 0.0],
    }
}

fn ellipse_geom(a: Pt, b: Pt) -> SchGeom {
    SchGeom::Arc {
        c: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
        r: [
            ((a[0] - b[0]) / 2.0).abs().max(5.0),
            ((a[1] - b[1]) / 2.0).abs().max(5.0),
        ],
        start: 0.0,
        end: 360.0,
    }
}

/// Draws the outline of a shape (for the selection and shapes being drawn).
fn outline(painter: &egui::Painter, v: &View, g: &SchGeom, s: Stroke) {
    let to = |p: &Pt| v.to_screen(p[0] as f32, p[1] as f32);
    match g {
        SchGeom::Line(pts) => {
            painter.add(egui::Shape::line(pts.iter().map(to).collect(), s));
        }
        SchGeom::Poly(pts) => {
            painter.add(egui::Shape::closed_line(pts.iter().map(to).collect(), s));
        }
        SchGeom::Rect { min, max, .. } => {
            painter.rect_stroke(Rect::from_two_pos(to(min), to(max)), 0.0, s);
        }
        SchGeom::Arc { c, r, start, end } => {
            let n = 48;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = (start + (end - start) * i as f64 / n as f64).to_radians();
                    to(&[c[0] + r[0] * a.cos(), c[1] + r[1] * a.sin()])
                })
                .collect();
            painter.add(egui::Shape::line(pts, s));
        }
    }
}
