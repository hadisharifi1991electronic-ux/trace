//! The Shape tool: draw pours, copper regions, keepouts, pour cutouts and
//! the board outline, and edit them: drag a corner, drag the middle of an
//! edge to add a corner, Ctrl+click a corner to take it away, drag the
//! shape to move it, Delete to remove it. A pour's net, dead copper and
//! connection style (direct, thermal relief, none) are set in its window.

use crate::canvas::View;
use eframe::egui::{self, Color32, Pos2, RichText, Shape, Stroke};
use trace_core::edit::Edit;
use trace_core::shape::{self, Kind, ShapeRef};
use trace_core::{Board, Join, PourConnect};

type P = [f64; 2];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Edit,
    Draw,
}

/// What a drag in Edit mode holds.
#[derive(Clone, Copy, Debug)]
pub enum Grab {
    Corner(usize),
    /// The middle of edge k: dragging it adds a corner there.
    NewCorner(usize),
    Whole,
}

pub struct ShapeTool {
    pub mode: Mode,
    pub kind: Kind,
    /// Draw a rectangle from two corners.
    pub rect: bool,
    /// Keep drawn edges to 45°.
    pub snap45: bool,
    /// Corners drawn so far.
    pub pts: Vec<P>,
    /// Net of a new pour or region, by name.
    pub net: String,
    pub picked: Option<ShapeRef>,
    /// A drag under way: what it holds and where it started.
    pub grab: Option<(Grab, P)>,
    /// The picked pour's net, as typed in its window.
    edit_net: String,
    edit_for: Option<ShapeRef>,
}

impl Default for ShapeTool {
    fn default() -> Self {
        ShapeTool {
            mode: Mode::Draw,
            kind: Kind::Pour,
            rect: false,
            snap45: true,
            pts: Vec::new(),
            net: String::new(),
            picked: None,
            grab: None,
            edit_net: String::new(),
            edit_for: None,
        }
    }
}

const PICK: Color32 = Color32::from_rgb(255, 196, 40);
const DRAW: Color32 = Color32::from_rgb(120, 220, 255);

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Pour => Color32::from_rgb(140, 200, 255),
        Kind::Region => Color32::from_rgb(120, 230, 160),
        Kind::Keepout => Color32::from_rgb(240, 90, 90),
        Kind::Cutout => Color32::from_rgb(180, 180, 190),
        Kind::Outline => Color32::from_rgb(217, 194, 122),
    }
}

fn mid(a: P, b: P) -> P {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

impl ShapeTool {
    pub fn reset(&mut self) {
        self.pts.clear();
        self.grab = None;
    }

    /// Drops a picked shape the board no longer has.
    pub fn keep_valid(&mut self, board: &Board) {
        if self.picked.is_some_and(|r| !r.exists(board)) {
            self.picked = None;
        }
    }

    /// Where the next corner goes for a pointer at `p`.
    pub fn next_point(&self, p: P) -> P {
        match self.pts.last() {
            Some(&last) if self.snap45 && !self.rect => shape::snap45(last, p),
            _ => p,
        }
    }

    /// The ring being drawn, with the pointer as its next corner.
    fn drawn_ring(&self, cursor: Option<P>) -> Vec<P> {
        let mut pts = self.pts.clone();
        if let Some(c) = cursor {
            let c = self.next_point(c);
            if pts.last() != Some(&c) {
                pts.push(c);
            }
        }
        if self.rect && pts.len() >= 2 {
            return shape::rect_ring(pts[0], pts[1]);
        }
        pts
    }

    /// A click while drawing. Returns the shape's ring once it is closed:
    /// a rectangle's second corner, or a click back on the first corner.
    pub fn draw_click(&mut self, p: P, tol: f64) -> Option<Vec<P>> {
        let p = self.next_point(p);
        if self.rect {
            self.pts.push(p);
            if self.pts.len() >= 2 {
                let r = shape::rect_ring(self.pts[0], self.pts[1]);
                self.pts.clear();
                return Some(r);
            }
            return None;
        }
        if let Some(&first) = self.pts.first() {
            if self.pts.len() >= 3 && (first[0] - p[0]).hypot(first[1] - p[1]) <= tol {
                return Some(std::mem::take(&mut self.pts));
            }
        }
        if self.pts.last() != Some(&p) {
            self.pts.push(p);
        }
        None
    }

    /// Closes the shape being drawn (Enter, double-click).
    pub fn finish(&mut self) -> Option<Vec<P>> {
        let mut pts = std::mem::take(&mut self.pts);
        pts.dedup();
        (pts.len() >= 3).then_some(pts)
    }

    /// What a press at `p` in Edit mode would take hold of.
    pub fn grab_at(&self, board: &Board, p: P, tol: f64, layer: Option<u8>) -> Option<Grab> {
        let r = self.picked?;
        let ring = r.ring(board)?;
        if let Some(k) = shape::corner_at(&ring, p, tol) {
            return Some(Grab::Corner(k));
        }
        let n = ring.len();
        if let Some(k) = (0..n).find(|&k| {
            let m = mid(ring[k], ring[(k + 1) % n]);
            (m[0] - p[0]).hypot(m[1] - p[1]) <= tol
        }) {
            return Some(Grab::NewCorner(k));
        }
        (shape::pick(board, p, tol, layer) == Some(r) && r != ShapeRef::Outline)
            .then_some(Grab::Whole)
    }

    /// The picked shape's ring as the drag under way leaves it.
    fn dragged_ring(&self, board: &Board, to: P) -> Option<Vec<P>> {
        let (g, from) = self.grab?;
        let ring = self.picked?.ring(board)?;
        Some(match g {
            Grab::Corner(k) => shape::with_corner(&ring, k, to),
            Grab::NewCorner(k) => shape::with_new_corner(&ring, k, to),
            Grab::Whole => ring
                .iter()
                .map(|q| [q[0] + to[0] - from[0], q[1] + to[1] - from[1]])
                .collect(),
        })
    }

    /// Edits for a drag that ended at `to`.
    pub fn drop_at(&mut self, board: &Board, to: P) -> Option<(String, Vec<Edit>)> {
        let (g, from) = self.grab?;
        let r = self.picked?;
        let edits = match g {
            Grab::Whole => shape::move_by(board, r, to[0] - from[0], to[1] - from[1]),
            _ => shape::set_ring(board, r, &self.dragged_ring(board, to)?),
        };
        self.grab = None;
        let what = r.kind(board).label().to_lowercase();
        let label = match g {
            Grab::Whole => format!("Moved a {what}"),
            Grab::NewCorner(_) => format!("Added a corner to a {what}"),
            Grab::Corner(_) => format!("Moved a corner of a {what}"),
        };
        (!edits.is_empty()).then_some((label, edits))
    }

    /// Ctrl+click on a corner of the picked shape: takes it away.
    pub fn remove_corner(&self, board: &Board, p: P, tol: f64) -> Option<(String, Vec<Edit>)> {
        let r = self.picked?;
        let ring = r.ring(board)?;
        let k = shape::corner_at(&ring, p, tol)?;
        let fewer = shape::without_corner(&ring, k)?;
        let e = shape::set_ring(board, r, &fewer);
        (!e.is_empty()).then(|| ("Took a corner away".to_string(), e))
    }

    /// Outlines of the shapes on the current layer, the picked one with its
    /// handles, and the shape being drawn.
    pub fn draw(
        &self,
        shapes: &mut Vec<Shape>,
        board: &Board,
        view: &View,
        layer: Option<u8>,
        cursor: Option<P>,
    ) {
        let scr = |p: P| view.to_screen(p[0] as f32, p[1] as f32);
        let ring_pts = |r: &[P]| -> Vec<Pos2> {
            let mut v: Vec<Pos2> = r.iter().map(|&p| scr(p)).collect();
            if let Some(&f) = v.first() {
                v.push(f);
            }
            v
        };
        for s in shape::shapes(board, layer) {
            if Some(s) == self.picked {
                continue;
            }
            let Some(r) = s.ring(board) else { continue };
            let c = kind_color(s.kind(board)).gamma_multiply(0.75);
            shapes.extend(Shape::dashed_line(
                &ring_pts(&r),
                Stroke::new(1.0f32, c),
                6.0,
                4.0,
            ));
        }
        if let Some(r) = self.picked {
            let ring = match (self.grab, cursor) {
                (Some(_), Some(c)) => self.dragged_ring(board, c),
                _ => r.ring(board),
            };
            if let Some(ring) = ring {
                shapes.push(Shape::line(ring_pts(&ring), Stroke::new(2.0f32, PICK)));
                let n = ring.len();
                for k in 0..n {
                    let c = scr(ring[k]);
                    let rect = egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0));
                    shapes.push(Shape::rect_filled(rect, 0.0, Color32::WHITE));
                    shapes.push(Shape::rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.0f32, Color32::from_rgb(20, 20, 28)),
                    ));
                    let m = scr(mid(ring[k], ring[(k + 1) % n]));
                    shapes.push(Shape::circle_filled(m, 3.5, PICK));
                }
            }
        }
        if self.mode == Mode::Draw && (!self.pts.is_empty() || cursor.is_some()) {
            let ring = self.drawn_ring(cursor);
            let c = kind_color(self.kind);
            if ring.len() >= 2 {
                let pts: Vec<Pos2> = ring.iter().map(|&p| scr(p)).collect();
                shapes.push(Shape::line(pts.clone(), Stroke::new(1.8f32, c)));
                if ring.len() >= 3 {
                    shapes.extend(Shape::dashed_line(
                        &[pts[pts.len() - 1], pts[0]],
                        Stroke::new(1.0f32, c),
                        5.0,
                        4.0,
                    ));
                }
            }
            for &p in &self.pts {
                shapes.push(Shape::circle_stroke(scr(p), 4.0, Stroke::new(1.5f32, DRAW)));
            }
        }
    }

    /// Options in the tool bar.
    pub fn bar(&mut self, ui: &mut egui::Ui) {
        ui.selectable_value(&mut self.mode, Mode::Draw, "Draw");
        ui.selectable_value(&mut self.mode, Mode::Edit, "Edit");
        ui.separator();
        if self.mode == Mode::Draw {
            egui::ComboBox::from_id_salt("shape_kind")
                .selected_text(self.kind.label())
                .show_ui(ui, |ui| {
                    for k in Kind::ALL {
                        ui.selectable_value(&mut self.kind, k, k.label());
                    }
                });
            if matches!(self.kind, Kind::Pour | Kind::Region) {
                ui.label("Net");
                ui.add(
                    egui::TextEdit::singleline(&mut self.net)
                        .hint_text("selected net")
                        .desired_width(110.0),
                );
            }
            ui.checkbox(&mut self.rect, "Rectangle");
            if !self.rect {
                ui.checkbox(&mut self.snap45, "45°");
            }
            let hint = if self.rect {
                "Click two opposite corners."
            } else if self.pts.len() < 3 {
                "Click each corner on the current layer."
            } else {
                "Click the first corner, double-click or Enter to close; Backspace takes the last corner back."
            };
            ui.label(RichText::new(hint).weak());
        } else {
            ui.label(
                RichText::new(
                    "Click a shape to pick it. Drag a corner, or an edge's middle to add one; Ctrl+click a corner to take it away; drag inside to move; Delete removes it.",
                )
                .weak(),
            );
        }
    }

    /// The picked shape's settings, in a small window. Returns edits.
    pub fn window(&mut self, ctx: &egui::Context, board: &Board) -> Option<(String, Vec<Edit>)> {
        let r = self.picked?;
        if !r.exists(board) {
            self.picked = None;
            return None;
        }
        if self.edit_for != Some(r) {
            self.edit_for = Some(r);
            self.edit_net = r
                .net(board)
                .and_then(|n| board.nets.get(n as usize))
                .map(|n| n.name.clone())
                .unwrap_or_default();
        }
        let mut out = None;
        let mut open = true;
        egui::Window::new("Shape")
            .open(&mut open)
            .default_width(330.0)
            .default_pos(egui::pos2(300.0, 140.0))
            .show(ctx, |ui| {
                ui.strong(r.describe(board));
                if let Some(ring) = r.ring(board) {
                    ui.label(RichText::new(format!("{} corners", ring.len())).weak());
                }
                let kind = r.kind(board);
                if matches!(kind, Kind::Pour | Kind::Region) {
                    ui.horizontal(|ui| {
                        ui.label("Net");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.edit_net).desired_width(150.0),
                        );
                        let enter =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Set").clicked() || enter {
                            let name = self.edit_net.trim();
                            let net = if name.is_empty() {
                                Some(None)
                            } else {
                                trace_core::stitch::net_named(board, name).map(Some)
                            };
                            match (net, r) {
                                (Some(n), ShapeRef::Pour(i)) => {
                                    out = Some((
                                        "Changed a pour's net".into(),
                                        shape::set_pour(board, i, |p| p.net = n),
                                    ))
                                }
                                (Some(n), ShapeRef::Area(i)) => {
                                    out = Some((
                                        "Changed a region's net".into(),
                                        shape::set_area_net(board, i, n),
                                    ))
                                }
                                (None, _) => {
                                    out = Some((
                                        format!("No net called \u{201c}{name}\u{201d}"),
                                        Vec::new(),
                                    ))
                                }
                                _ => {}
                            }
                        }
                    });
                }
                if let ShapeRef::Pour(i) = r {
                    if let Some(p) = board.geometry.pours.get(i) {
                        let mut np = p.clone();
                        ui.checkbox(&mut np.remove_dead, "Remove dead copper")
                            .on_hover_text("Leave out islands that touch none of the pour's net");
                        ui.horizontal(|ui| {
                            ui.label("Pour order");
                            ui.add(egui::DragValue::new(&mut np.order).range(-100..=1000))
                                .on_hover_text("Lower pours first; later ones keep clear of them");
                        });
                        ui.separator();
                        ui.strong("Connection to its net's pads and vias");
                        let mut own = np.connect.is_some();
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut own, false, "Board rules");
                            ui.radio_value(&mut own, true, "Set here");
                        });
                        if own && np.connect.is_none() {
                            np.connect = Some(PourConnect::USUAL);
                        }
                        if !own {
                            np.connect = None;
                        }
                        if let Some(c) = &mut np.connect {
                            egui::Grid::new("joins").num_columns(2).show(ui, |ui| {
                                for (label, j) in [
                                    ("Through-hole pads", &mut c.th_pads),
                                    ("Surface pads", &mut c.smd_pads),
                                    ("Vias", &mut c.vias),
                                ] {
                                    ui.label(label);
                                    join_ui(ui, label, j);
                                    ui.end_row();
                                }
                            });
                        }
                        if format!("{np:?}") != format!("{p:?}") {
                            let label = if np.connect != p.connect {
                                "Changed a pour's connection style"
                            } else {
                                "Changed a pour"
                            };
                            out = Some((label.into(), shape::set_pour(board, i, |q| *q = np)));
                        }
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if r != ShapeRef::Outline && ui.button("Delete").clicked() {
                        out = Some((
                            format!("Deleted a {}", kind.label().to_lowercase()),
                            shape::delete(board, r),
                        ));
                    }
                    if let ShapeRef::Pour(i) = r {
                        if ui.button("Pour again").clicked() {
                            out = Some(("Poured again".into(), shape::pour_again(board, i)));
                        }
                    }
                });
            });
        if !open {
            self.picked = None;
        }
        out
    }
}

/// One row of connection settings: direct, relief (gap, spoke width and
/// count, angle) or none.
fn join_ui(ui: &mut egui::Ui, id: &str, j: &mut Join) {
    ui.horizontal(|ui| {
        let name = match j {
            Join::Direct => "Direct",
            Join::Relief { .. } => "Relief",
            Join::None { .. } => "None",
        };
        egui::ComboBox::from_id_salt(("join", id))
            .selected_text(name)
            .width(70.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(matches!(j, Join::Direct), "Direct")
                    .clicked()
                {
                    *j = Join::Direct;
                }
                if ui
                    .selectable_label(matches!(j, Join::Relief { .. }), "Relief")
                    .clicked()
                    && !matches!(j, Join::Relief { .. })
                {
                    *j = Join::RELIEF;
                }
                if ui
                    .selectable_label(matches!(j, Join::None { .. }), "None")
                    .clicked()
                {
                    *j = Join::None { gap: 10.0 };
                }
            });
        match j {
            Join::Direct => {}
            Join::Relief {
                gap,
                width,
                spokes,
                diagonal,
            } => {
                ui.add(
                    egui::DragValue::new(gap)
                        .speed(0.5)
                        .range(1.0..=100.0)
                        .prefix("gap ")
                        .suffix(" mil"),
                );
                ui.add(
                    egui::DragValue::new(width)
                        .speed(0.5)
                        .range(2.0..=100.0)
                        .prefix("spoke ")
                        .suffix(" mil"),
                );
                egui::ComboBox::from_id_salt(("spokes", id))
                    .selected_text(format!("{spokes}×"))
                    .width(40.0)
                    .show_ui(ui, |ui| {
                        for n in [2u32, 4] {
                            ui.selectable_value(spokes, n, format!("{n}×"));
                        }
                    });
                ui.checkbox(diagonal, "45°");
            }
            Join::None { gap } => {
                ui.add(
                    egui::DragValue::new(gap)
                        .speed(0.5)
                        .range(1.0..=100.0)
                        .prefix("gap ")
                        .suffix(" mil"),
                );
            }
        }
    });
}

/// The copper layer the tool works on, from the current layer.
pub fn layer_of(slot: crate::scene::Slot) -> Option<u8> {
    match slot {
        crate::scene::Slot::Copper(l) => Some(l),
        _ => None,
    }
}

/// The net a new pour or region gets: the one typed, else the selected one.
pub fn net_for(board: &Board, typed: &str, selected: Option<u32>) -> Result<Option<u32>, String> {
    let t = typed.trim();
    if t.is_empty() {
        return Ok(selected);
    }
    trace_core::stitch::net_named(board, t)
        .map(Some)
        .ok_or_else(|| format!("No net called \u{201c}{t}\u{201d}."))
}
