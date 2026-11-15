//! Trace Studio's workspace, laid out the way Altium Designer users know:
//! panels docked left and right with a tab strip, a collapsed dock shown
//! as a strip of upright tabs, document tabs over the canvas, a Home page,
//! a Panels button and key sequences such as P then T. The colours are
//! Trace's own navy and teal.

use crate::toolbar::{self, Icon};
use crate::{App, Tool};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

/// Trace's accent: the teal of the logo.
pub const ACCENT: Color32 = Color32::from_rgb(38, 150, 172);
/// Panel title bars.
pub const HEADER: Color32 = Color32::from_rgb(24, 70, 92);

/// The dockable panels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Projects,
    Navigator,
    Pcb,
    Layers,
    Properties,
    Messages,
}

impl Pane {
    pub const ALL: [Pane; 6] = [
        Pane::Projects,
        Pane::Navigator,
        Pane::Pcb,
        Pane::Layers,
        Pane::Properties,
        Pane::Messages,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Pane::Projects => "Projects",
            Pane::Navigator => "Navigator",
            Pane::Pcb => "PCB",
            Pane::Layers => "Layers",
            Pane::Properties => "Properties",
            Pane::Messages => "Messages",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Pane::Projects => "The open project: its board, schematic sheets and libraries",
            Pane::Navigator => "Board summary, find a net or part, the selected net, measuring",
            Pane::Pcb => "Every net, part, class and differential pair of the board; click to select and zoom",
            Pane::Layers => "Show and hide layers, pick the current layer",
            Pane::Properties => "What is selected, with its settings to change",
            Pane::Messages => "What Trace has said this session",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Pane::Projects => "projects",
            Pane::Navigator => "navigator",
            Pane::Pcb => "pcb",
            Pane::Layers => "layers",
            Pane::Properties => "properties",
            Pane::Messages => "messages",
        }
    }

    fn from_key(k: &str) -> Option<Pane> {
        Pane::ALL.into_iter().find(|p| p.key() == k)
    }

    /// The dock it goes to when opened from the Panels menu.
    fn home_side(self) -> DockSide {
        match self {
            Pane::Properties | Pane::Messages => DockSide::Right,
            _ => DockSide::Left,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockSide {
    Left,
    Right,
}

/// One dock: its panels in tab order, the one showing, and whether it is
/// open or collapsed to a strip.
#[derive(Clone, Debug)]
pub struct Dock {
    pub panes: Vec<Pane>,
    pub on: Option<Pane>,
    pub open: bool,
}

impl Dock {
    fn shown(&self) -> Option<Pane> {
        self.on
            .filter(|p| self.panes.contains(p))
            .or(self.panes.first().copied())
    }
}

pub const STORE_KEY: &str = "studio_layout";

/// The workspace state that is not about the design.
pub struct Studio {
    pub left: Dock,
    pub right: Dock,
    /// The Home page is the document showing.
    pub home: bool,
    /// The Home page has a tab.
    pub home_tab: bool,
    pub mm: bool,
    pub dark: bool,
    /// Messages shown this session, newest last.
    pub log: Vec<(String, bool)>,
    last_msg: Option<(String, bool)>,
    /// Board, schematic, footprint and 3D modes last frame, to leave the
    /// Home page when one changes.
    last_modes: (bool, bool, bool),
    /// A menu key (P, D, T...) is waiting for its second key.
    pub accel: Option<Accel>,
    pub keys_open: bool,
}

impl Default for Studio {
    fn default() -> Self {
        Studio {
            left: Dock {
                panes: vec![Pane::Projects, Pane::Navigator, Pane::Pcb, Pane::Layers],
                on: Some(Pane::Projects),
                open: true,
            },
            right: Dock {
                panes: vec![Pane::Properties, Pane::Messages],
                on: Some(Pane::Properties),
                open: true,
            },
            home: true,
            home_tab: true,
            mm: false,
            dark: true,
            log: Vec::new(),
            last_msg: None,
            last_modes: (false, false, false),
            accel: None,
            keys_open: false,
        }
    }
}

impl Studio {
    pub fn from_store(s: Option<String>) -> Studio {
        let mut st = Studio::default();
        let Some(s) = s else { return st };
        for part in s.split(';') {
            let Some((k, v)) = part.split_once('=') else { continue };
            let panes = || -> Vec<Pane> { v.split(',').filter_map(Pane::from_key).collect() };
            match k {
                "L" => st.left.panes = panes(),
                "R" => st.right.panes = panes(),
                "l" => st.left.on = Pane::from_key(v),
                "r" => st.right.on = Pane::from_key(v),
                "lo" => st.left.open = v == "1",
                "ro" => st.right.open = v == "1",
                "mm" => st.mm = v == "1",
                "dark" => st.dark = v != "0",
                _ => {}
            }
        }
        st
    }

    pub fn to_store(&self) -> String {
        let list = |d: &Dock| d.panes.iter().map(|p| p.key()).collect::<Vec<_>>().join(",");
        let on = |d: &Dock| d.on.map(|p| p.key()).unwrap_or("");
        format!(
            "L={};R={};l={};r={};lo={};ro={};mm={};dark={}",
            list(&self.left),
            list(&self.right),
            on(&self.left),
            on(&self.right),
            self.left.open as u8,
            self.right.open as u8,
            self.mm as u8,
            self.dark as u8
        )
    }

    pub fn dock(&mut self, side: DockSide) -> &mut Dock {
        match side {
            DockSide::Left => &mut self.left,
            DockSide::Right => &mut self.right,
        }
    }

    pub fn is_open(&self, p: Pane) -> bool {
        self.left.panes.contains(&p) || self.right.panes.contains(&p)
    }

    /// Shows a panel: brings it to the front of its dock, opening the dock,
    /// or adds it to its usual dock.
    pub fn show(&mut self, p: Pane) {
        let side = if self.left.panes.contains(&p) {
            DockSide::Left
        } else if self.right.panes.contains(&p) {
            DockSide::Right
        } else {
            let s = p.home_side();
            self.dock(s).panes.push(p);
            s
        };
        let d = self.dock(side);
        d.on = Some(p);
        d.open = true;
    }

    pub fn close(&mut self, p: Pane) {
        for d in [&mut self.left, &mut self.right] {
            d.panes.retain(|&q| q != p);
            if d.on == Some(p) {
                d.on = d.panes.first().copied();
            }
        }
    }

    fn move_to(&mut self, p: Pane, side: DockSide) {
        self.close(p);
        let d = self.dock(side);
        d.panes.push(p);
        d.on = Some(p);
        d.open = true;
    }

    pub fn reset_layout(&mut self) {
        let d = Studio::default();
        self.left = d.left;
        self.right = d.right;
    }

    /// Format a length given in mil in the chosen units.
    pub fn len(&self, mil: f64) -> String {
        if self.mm {
            format!("{:.4} mm", mil * 0.0254)
        } else {
            format!("{mil:.2} mil")
        }
    }
}

/// Trace Studio's look: dark (the default) or light chrome, teal accent.
pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut style = (*ctx.style()).clone();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        v.panel_fill = Color32::from_rgb(40, 42, 47);
        v.window_fill = Color32::from_rgb(44, 46, 52);
        v.extreme_bg_color = Color32::from_rgb(27, 29, 33);
        v.faint_bg_color = Color32::from_rgb(47, 49, 55);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, Color32::from_rgb(60, 63, 70));
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(54, 57, 64);
        v.widgets.inactive.bg_fill = Color32::from_rgb(54, 57, 64);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(62, 72, 82);
        v.widgets.hovered.bg_fill = Color32::from_rgb(62, 72, 82);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(28, 96, 116);
        v.widgets.active.bg_fill = Color32::from_rgb(28, 96, 116);
        v.hyperlink_color = Color32::from_rgb(96, 196, 216);
    } else {
        v.panel_fill = Color32::from_rgb(236, 238, 241);
        v.window_fill = Color32::from_rgb(246, 247, 249);
        v.faint_bg_color = Color32::from_rgb(228, 231, 235);
        v.hyperlink_color = Color32::from_rgb(16, 110, 132);
    }
    v.selection.bg_fill = if dark {
        Color32::from_rgb(28, 104, 124)
    } else {
        Color32::from_rgb(150, 210, 222)
    };
    v.selection.stroke = Stroke::new(1.0f32, if dark { Color32::WHITE } else { Color32::BLACK });
    v.window_rounding = egui::Rounding::same(4.0);
    v.menu_rounding = egui::Rounding::same(3.0);
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    ctx.set_style(style);
}

/// A panel's title bar: its name and the menu, collapse and close buttons.
enum HeaderAct {
    Collapse,
    Close,
    Move,
}

#[derive(Clone, Copy)]
enum Glyph {
    Close,
    Collapse,
    ToRight,
    ToLeft,
}

/// A small drawn button for a panel's title bar.
fn flat(ui: &mut egui::Ui, g: Glyph, tip: &str) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, 3.0, Color32::from_white_alpha(40));
    }
    let s = Stroke::new(1.4f32, Color32::from_gray(230));
    let k = r.shrink(5.0);
    match g {
        Glyph::Close => {
            p.line_segment([k.left_top(), k.right_bottom()], s);
            p.line_segment([k.right_top(), k.left_bottom()], s);
        }
        Glyph::Collapse => {
            p.line_segment([k.left_bottom(), k.right_bottom()], s);
        }
        Glyph::ToRight | Glyph::ToLeft => {
            let (a, b) = if matches!(g, Glyph::ToRight) {
                (k.left_center(), k.right_center())
            } else {
                (k.right_center(), k.left_center())
            };
            p.line_segment([a, b], s);
            let d = if matches!(g, Glyph::ToRight) { -3.5 } else { 3.5 };
            p.line_segment([b, b + Vec2::new(d, -3.5)], s);
            p.line_segment([b, b + Vec2::new(d, 3.5)], s);
        }
    }
    resp.on_hover_text(tip).clicked()
}

fn header(ui: &mut egui::Ui, title: &str, side: DockSide) -> Option<HeaderAct> {
    let mut act = None;
    egui::Frame::none()
        .fill(HEADER)
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong().color(Color32::WHITE).size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if flat(ui, Glyph::Close, "Close this panel (the Panels button brings it back)") {
                        act = Some(HeaderAct::Close);
                    }
                    if flat(ui, Glyph::Collapse, "Collapse the dock to a strip of tabs") {
                        act = Some(HeaderAct::Collapse);
                    }
                    let other = match side {
                        DockSide::Left => Glyph::ToRight,
                        DockSide::Right => Glyph::ToLeft,
                    };
                    if flat(ui, other, "Move this panel to the other side") {
                        act = Some(HeaderAct::Move);
                    }
                });
            });
        });
    act
}

/// A tab in a dock's strip along its bottom.
fn pane_tab(ui: &mut egui::Ui, text: &str, on: bool, tip: &str) -> egui::Response {
    let font = FontId::proportional(13.0);
    let fg = if on {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 24.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 0.0, ui.visuals().window_fill);
        p.rect_filled(
            Rect::from_min_size(Pos2::new(rect.left(), rect.bottom() - 2.5), Vec2::new(rect.width(), 2.5)),
            0.0,
            ACCENT,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    p.galley(
        Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp.on_hover_text(tip)
}

/// An upright tab in a collapsed dock's strip, text reading top to bottom.
fn strip_tab(ui: &mut egui::Ui, text: &str, tip: &str) -> egui::Response {
    let font = FontId::proportional(13.0);
    let fg = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = galley.size();
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(size.y + 8.0, size.x + 18.0), Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        2.0,
        if resp.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().faint_bg_color
        },
        Stroke::new(1.0f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    let pos = Pos2::new(rect.right() - 4.0, rect.top() + 9.0);
    p.add(egui::epaint::TextShape::new(pos, galley, fg).with_angle(std::f32::consts::FRAC_PI_2));
    resp.on_hover_text(tip)
}

/// A document tab over the canvas.
fn doc_tab(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    on: bool,
    closable: bool,
) -> (bool, bool) {
    let font = FontId::proportional(13.0);
    let fg = if on {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let w = 34.0 + galley.size().x + if closable { 22.0 } else { 10.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let p = ui.painter().clone();
    let bg = if on {
        Color32::from_rgb(15, 17, 21)
    } else if resp.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        ui.visuals().faint_bg_color
    };
    p.rect_filled(
        rect,
        egui::Rounding {
            nw: 3.0,
            ne: 3.0,
            sw: 0.0,
            se: 0.0,
        },
        bg,
    );
    if on {
        p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(rect.width(), 2.5)), 0.0, ACCENT);
    }
    let ir = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.center().y - 8.0), Vec2::splat(16.0));
    toolbar::draw(&p, icon, ir, Color32::from_gray(214), true);
    let text_fg = if on { Color32::WHITE } else { fg };
    p.galley_with_override_text_color(
        Pos2::new(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text_fg,
    );
    let mut closed = false;
    if closable {
        let cr = Rect::from_center_size(Pos2::new(rect.right() - 12.0, rect.center().y), Vec2::splat(14.0));
        let hot = resp.hover_pos().is_some_and(|h| cr.contains(h));
        if hot {
            p.rect_filled(cr, 2.0, Color32::from_gray(80));
        }
        let c = if hot || on { Color32::from_gray(230) } else { Color32::from_gray(140) };
        let s = Stroke::new(1.3f32, c);
        let k = cr.shrink(3.5);
        p.line_segment([k.left_top(), k.right_bottom()], s);
        p.line_segment([k.right_top(), k.left_bottom()], s);
        if resp.clicked() && hot {
            closed = true;
        }
    }
    (resp.clicked() && !closed, closed)
}

/// Second keys of the menu keys, as in Altium: P then T places a track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accel {
    Place,
    Route,
}

impl Accel {
    fn title(self) -> &'static str {
        match self {
            Accel::Place => "Place",
            Accel::Route => "Route",
        }
    }

    /// (key, label) of each item.
    fn items(self) -> &'static [(egui::Key, &'static str, &'static str)] {
        use egui::Key::*;
        match self {
            Accel::Place => &[
                (T, "T", "Track (interactive routing)"),
                (I, "I", "Differential pair routing"),
                (V, "V", "Via"),
                (S, "S", "String (text)"),
                (D, "D", "Dimension"),
                (G, "G", "Polygon pour and shapes"),
                (F, "F", "Fanout…"),
                (C, "C", "Component from a footprint library"),
            ],
            Accel::Route => &[
                (T, "T", "Interactive routing"),
                (I, "I", "Differential pair routing"),
                (R, "R", "Interactive length tuning"),
                (A, "A", "Autoroute…"),
                (F, "F", "Fanout…"),
                (U, "U", "Unroute the selected net"),
            ],
        }
    }
}

impl App {
    /// The docks on both sides, before the document area is laid out.
    pub(crate) fn show_docks(&mut self, ctx: &egui::Context) {
        for side in [DockSide::Left, DockSide::Right] {
            let d = self.studio.dock(side).clone();
            if d.panes.is_empty() {
                continue;
            }
            let id = match side {
                DockSide::Left => "dock_left",
                DockSide::Right => "dock_right",
            };
            let frame = egui::Frame::side_top_panel(&ctx.style()).inner_margin(0.0);
            if d.open {
                let panel = match side {
                    DockSide::Left => egui::SidePanel::left(id),
                    DockSide::Right => egui::SidePanel::right(id),
                };
                panel
                    .resizable(true)
                    .default_width(if side == DockSide::Left { 290.0 } else { 270.0 })
                    .width_range(200.0..=640.0)
                    .frame(frame)
                    .show(ctx, |ui| self.dock_ui(ui, side));
            } else {
                let strip = format!("{id}_strip");
                let panel = match side {
                    DockSide::Left => egui::SidePanel::left(strip),
                    DockSide::Right => egui::SidePanel::right(strip),
                };
                panel
                    .resizable(false)
                    .exact_width(26.0)
                    .frame(frame.inner_margin(egui::Margin::symmetric(2.0, 6.0)))
                    .show(ctx, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for p in d.panes.iter().copied() {
                            if strip_tab(ui, p.label(), p.tip()).clicked() {
                                let dk = self.studio.dock(side);
                                dk.on = Some(p);
                                dk.open = true;
                            }
                        }
                    });
            }
        }
    }

    fn dock_ui(&mut self, ui: &mut egui::Ui, side: DockSide) {
        let d = self.studio.dock(side).clone();
        let Some(pane) = d.shown() else { return };
        let act = egui::TopBottomPanel::top(format!("{side:?}_head"))
            .frame(egui::Frame::none())
            .show_inside(ui, |ui| header(ui, pane.label(), side))
            .inner;
        match act {
            Some(HeaderAct::Close) => self.studio.close(pane),
            Some(HeaderAct::Collapse) => self.studio.dock(side).open = false,
            Some(HeaderAct::Move) => self.studio.move_to(
                pane,
                match side {
                    DockSide::Left => DockSide::Right,
                    DockSide::Right => DockSide::Left,
                },
            ),
            None => {}
        }
        if d.panes.len() > 1 {
            egui::TopBottomPanel::bottom(format!("{side:?}_tabs"))
                .frame(
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .inner_margin(egui::Margin::symmetric(2.0, 0.0)),
                )
                .show_inside(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(1.0, 1.0);
                        for p in d.panes.iter().copied() {
                            if pane_tab(ui, p.label(), p == pane, p.tip()).clicked() {
                                self.studio.dock(side).on = Some(p);
                            }
                        }
                    });
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(8.0, 6.0)))
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(format!("{side:?}_{}", pane.label()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.pane_ui(pane, ui));
            });
    }

    /// The Panels menu: every panel with a tick when it is open.
    pub(crate) fn panels_menu(&mut self, ui: &mut egui::Ui) {
        for p in Pane::ALL {
            let mut on = self.studio.is_open(p);
            if ui.checkbox(&mut on, p.label()).on_hover_text(p.tip()).changed() {
                if on {
                    self.studio.show(p);
                } else {
                    self.studio.close(p);
                }
            }
        }
        ui.separator();
        if ui.button("Reset panel layout").clicked() {
            ui.close_menu();
            self.studio.reset_layout();
        }
    }

    /// Per-frame bookkeeping: the message log and leaving the Home page when
    /// a view mode changes.
    pub(crate) fn studio_tick(&mut self) {
        if self.message != self.studio.last_msg {
            if let Some(m) = &self.message {
                self.studio.log.push(m.clone());
                if self.studio.log.len() > 500 {
                    self.studio.log.remove(0);
                }
            }
            self.studio.last_msg = self.message.clone();
        }
        let modes = (self.mode_sch, self.mode_fp, self.mode3d);
        if modes != self.studio.last_modes {
            self.studio.last_modes = modes;
            self.studio.home = false;
        }
        if self.scene.is_none() && self.schem.is_empty() && self.fped.is_empty() {
            self.studio.home = true;
            self.studio.home_tab = true;
        }
    }

    /// The document tabs over the canvas.
    pub(crate) fn doc_tabs(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("doc_tabs")
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.extreme_bg_color)
                    .inner_margin(egui::Margin {
                        left: 4.0,
                        right: 4.0,
                        top: 4.0,
                        bottom: 0.0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let home = self.studio.home;
                    if self.studio.home_tab {
                        let (click, close) = doc_tab(ui, Icon::Home, "Home Page", home, true);
                        if click {
                            self.studio.home = true;
                        }
                        if close {
                            self.studio.home_tab = false;
                            self.studio.home = false;
                        }
                    }
                    let name = self
                        .scene
                        .as_ref()
                        .and_then(|s| s.path.file_name())
                        .map(|n| n.to_string_lossy().into_owned());
                    if let Some(name) = name {
                        let on = !home && !self.mode_sch && !self.mode_fp;
                        let (icon, label) = if self.mode3d {
                            (Icon::View3d, format!("{name} (3D)"))
                        } else {
                            (Icon::Board, name)
                        };
                        let label = if self.dirty { format!("{label} *") } else { label };
                        if doc_tab(ui, icon, &label, on, false).0 {
                            self.go_board();
                        }
                    }
                    if !self.schem.is_empty() {
                        let on = !home && self.mode_sch && !self.mode_fp;
                        let label = self
                            .schem
                            .project
                            .as_ref()
                            .and_then(|p| p.file_stem())
                            .map(|s| format!("{} (schematic)", s.to_string_lossy()))
                            .or_else(|| self.schem.slots.get(self.schem.current).map(|s| s.name.clone()))
                            .unwrap_or_else(|| "Schematic".into());
                        let label = if self.schem.dirty { format!("{label} *") } else { label };
                        if doc_tab(ui, Icon::Schematic, &label, on, false).0 {
                            self.go_schematic();
                        }
                    }
                    if !self.fped.is_empty() {
                        let on = !home && self.mode_fp;
                        let label = if self.fped.source.is_empty() {
                            "Footprints".to_string()
                        } else {
                            self.fped.source.clone()
                        };
                        let label = if self.fped.dirty { format!("{label} *") } else { label };
                        let (click, close) = doc_tab(ui, Icon::Footprint, &label, on, true);
                        if click {
                            self.studio.home = false;
                            self.mode_fp = true;
                            self.studio.last_modes = (self.mode_sch, self.mode_fp, self.mode3d);
                        }
                        if close {
                            self.mode_fp = false;
                            if !self.fped.dirty {
                                self.fped = Default::default();
                            }
                        }
                    }
                });
            });
    }

    pub(crate) fn go_board(&mut self) {
        self.studio.home = false;
        self.mode_sch = false;
        self.mode_fp = false;
        self.studio.last_modes = (self.mode_sch, self.mode_fp, self.mode3d);
    }

    pub(crate) fn go_schematic(&mut self) {
        if self.schem.is_empty() {
            return;
        }
        self.studio.home = false;
        self.mode_sch = true;
        self.mode_fp = false;
        self.mode3d = false;
        self.studio.last_modes = (self.mode_sch, self.mode_fp, self.mode3d);
    }

    pub(crate) fn go_home(&mut self) {
        self.studio.home = true;
        self.studio.home_tab = true;
    }

    /// The Home page: open a design, recent files, and the keys.
    pub(crate) fn home_page(&mut self, ui: &mut egui::Ui) {
        let dark = Color32::from_rgb(15, 17, 21);
        ui.painter().rect_filled(ui.max_rect(), 0.0, dark);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(36.0);
            ui.horizontal(|ui| {
                ui.add_space(48.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                        logo(ui.painter(), r);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Trace Studio").size(30.0).strong().color(Color32::WHITE));
                            ui.label(
                                RichText::new(concat!(
                                    "Version ",
                                    env!("CARGO_PKG_VERSION"),
                                    " · open-source PCB design for high-speed boards"
                                ))
                                .color(Color32::from_gray(160)),
                            );
                        });
                    });
                    ui.add_space(24.0);
                    ui.columns(2, |cols| {
                        let ui = &mut cols[0];
                        section(ui, "Start");
                        let big = |ui: &mut egui::Ui, text: &str, tip: &str| {
                            ui.add_sized(
                                [280.0, 30.0],
                                egui::Button::new(RichText::new(text).size(14.0)),
                            )
                            .on_hover_text(tip)
                            .clicked()
                        };
                        if big(ui, "Open project or document…", "Altium .PrjPcb, .PcbDoc, .SchDoc, .PcbLib, .IntLib, Allegro .brd, KiCad, Trace files (Ctrl+O)") {
                            self.open_dialog();
                        }
                        if big(ui, "Open Gerber or ODB++ folder…", "Rebuild a board from fabrication data") {
                            self.open_folder_dialog();
                        }
                        if self.scene.is_some() && big(ui, "Go to the open board", "") {
                            self.go_board();
                        }
                        ui.add_space(16.0);
                        section(ui, "Recent documents");
                        if self.recent.is_empty() {
                            ui.label(RichText::new("Nothing opened yet.").weak());
                        }
                        for p in self.recent.clone().into_iter().take(12) {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let dir = p
                                .parent()
                                .map(|d| d.display().to_string())
                                .unwrap_or_default();
                            ui.horizontal(|ui| {
                                if ui.link(RichText::new(name).size(14.0)).clicked() {
                                    self.open(p.clone());
                                }
                                ui.label(RichText::new(dir).small().weak());
                            });
                        }
                        let ui = &mut cols[1];
                        section(ui, "Keys");
                        egui::Grid::new("home_keys").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                            for (k, what) in KEYS {
                                ui.label(RichText::new(*k).monospace().color(ACCENT));
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                    });
                });
            });
        });
    }

    /// The pop-up of a menu key's second keys, and acting on the second key.
    pub(crate) fn accel_keys(&mut self, ctx: &egui::Context, board2d: bool, typing: bool) {
        if typing || !board2d {
            self.studio.accel = None;
            return;
        }
        let Some(a) = self.studio.accel else {
            let first = ctx.input(|i| {
                use egui::Key::*;
                [(P, Accel::Place), (U, Accel::Route)]
                    .into_iter()
                    .find(|(k, _)| i.key_pressed(*k) && !i.modifiers.any())
                    .map(|x| x.1)
            });
            if let Some(a) = first {
                ctx.input_mut(|i| {
                    let k = match a {
                        Accel::Place => egui::Key::P,
                        Accel::Route => egui::Key::U,
                    };
                    i.consume_key(egui::Modifiers::NONE, k);
                });
                self.studio.accel = Some(a);
            }
            return;
        };
        let hit = ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                return Some(None);
            }
            for (k, _, label) in a.items() {
                if i.consume_key(egui::Modifiers::NONE, *k) {
                    return Some(Some(*label));
                }
            }
            None
        });
        let mut pick = hit;
        let pos = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(self.view.rect.center());
        egui::Area::new(egui::Id::new("accel"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos + Vec2::new(12.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(a.title()).strong().color(ACCENT));
                    for (_, key, label) in a.items() {
                        let r = ui.add(
                            egui::Button::new(format!("{key}   {label}")).frame(false),
                        );
                        if r.clicked() {
                            pick = Some(Some(*label));
                        }
                    }
                    ui.label(RichText::new("Press a key, or Esc").small().weak());
                });
            });
        if let Some(choice) = pick {
            self.studio.accel = None;
            if let Some(label) = choice {
                self.run_command(label);
            }
        }
    }

    /// Carries out a command named as in the menus and key pop-ups.
    pub(crate) fn run_command(&mut self, label: &str) {
        let has = self.scene.is_some();
        match label {
            "Track (interactive routing)" | "Interactive routing" => {
                self.set_tool(Tool::Route);
                self.router.pairs = false;
            }
            "Differential pair routing" => {
                self.set_tool(Tool::Route);
                self.router.pairs = true;
            }
            "Interactive length tuning" => self.set_tool(Tool::Tune),
            "Via" => {
                if self.router.active.is_some() {
                    self.place_via(false);
                } else {
                    self.message = Some((
                        "Start a track first (P then T), then press V where the via goes.".into(),
                        false,
                    ));
                }
            }
            "String (text)" => self.set_tool(Tool::Text),
            "Dimension" => self.set_tool(Tool::Dimension),
            "Polygon pour and shapes" => self.set_tool(Tool::Shape),
            "Fanout…" => self.fan.open = true,
            "Component from a footprint library" => self.open_footprints(),
            "Rules: Constraint Manager" => self.cm.open = true,
            "Layer stack and impedance" => self.imp.open = true,
            "Copy block layout (reuse)…" => self.reuse.open = true,
            "Panelize…" => self.panel.open = true,
            "Assembly variants…" => self.variants.open = true,
            "Design Rule Check" => self.drc.open = true,
            "Teardrops and gloss…" => self.tidy.open = true,
            "3D clearance…" => self.clear3d.open = true,
            "Via stitching and fences…" => self.stitch.open = true,
            "Test points…" => self.tests.open = true,
            "Pin and gate swap…" => self.swap.open = true,
            "Autoroute…" => self.autoroute.open = true,
            "Unroute the selected net" => self.unroute_selected(),
            "Fit board" => self.needs_fit = true,
            "View from the other side" => {
                self.view.flip = !self.view.flip;
                self.needs_fit = true;
            }
            "3D view" if has => {
                self.go_board();
                self.mode3d = true;
            }
            "2D view" => {
                self.go_board();
                self.mode3d = false;
            }
            "Home page" => self.go_home(),
            _ => {}
        }
    }

    /// Help > Keyboard shortcuts.
    pub(crate) fn keys_window(&mut self, ctx: &egui::Context) {
        let mut open = self.studio.keys_open;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("keys_win").num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (k, what) in KEYS {
                        ui.label(RichText::new(*k).monospace().color(ACCENT));
                        ui.label(*what);
                        ui.end_row();
                    }
                });
            });
        self.studio.keys_open = open;
    }
}

/// The keys shown on the Home page and in Help.
pub const KEYS: &[(&str, &str)] = &[
    ("P, T", "Place a track: interactive routing"),
    ("P, I", "Differential pair routing"),
    ("P, S / P, D", "Place text / a dimension"),
    ("P, G", "Polygon pours and shapes"),
    ("U, A / U, F", "Autoroute / fanout"),
    ("D", "Design Rule Check"),
    ("Ctrl+W", "Interactive routing"),
    ("Ctrl+M", "Measure distance"),
    ("X", "Route on and off"),
    ("V / Shift+V", "Via down / up while routing"),
    ("T", "Length tuning"),
    ("Shift+S", "Single layer mode"),
    ("+ / - / *", "Next / previous copper layer"),
    ("Q", "Units: mil or mm"),
    ("F / B / 3", "Fit / flip / 3D"),
    ("S", "Schematic sheets"),
    ("C / I", "Constraints / impedance"),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
    ("Ctrl+A, C, X, V", "Select all, copy, cut, paste"),
    ("Ctrl+R", "Turn the selection 90°"),
    ("Esc", "Cancel, then clear the selection"),
];

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(17.0).strong().color(Color32::from_gray(230)));
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [Pos2::new(r.left(), r.top()), Pos2::new(r.left() + 280.0, r.top())],
        Stroke::new(1.5f32, ACCENT),
    );
    ui.add_space(8.0);
}

/// The Trace mark: a navy square with a teal T trace ending in two pads.
pub fn logo(p: &egui::Painter, r: Rect) {
    p.rect_filled(r, 7.0, Color32::from_rgb(20, 40, 72));
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * r.width(), r.top() + y * r.height());
    let s = Stroke::new(r.width() * 0.11, ACCENT);
    p.line_segment([at(0.2, 0.28), at(0.8, 0.28)], s);
    p.line_segment([at(0.5, 0.28), at(0.5, 0.8)], s);
    p.circle_filled(at(0.2, 0.28), r.width() * 0.08, Color32::from_rgb(222, 184, 74));
    p.circle_filled(at(0.8, 0.28), r.width() * 0.08, Color32::from_rgb(222, 184, 74));
    p.circle_filled(at(0.5, 0.8), r.width() * 0.09, Color32::from_rgb(222, 184, 74));
}
