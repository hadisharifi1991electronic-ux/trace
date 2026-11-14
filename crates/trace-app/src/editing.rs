//! Selecting many things on the board, and placing text and dimensions:
//! the state behind box selection, Ctrl+click, dragging a selection,
//! copy and paste, and the Text and Dimension tools.

use crate::canvas::{draw, grow, View};
use crate::scene::{Geom, Prim, Scene, Slot, Source};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Shape, Stroke};
use trace_core::select::{self, Clip, Sel, TextStyle, Units};
use trace_core::{LayerRef, Track};

/// Where placed text and dimensions go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    TopSilk,
    BottomSilk,
    /// The current copper layer.
    Copper,
    Mechanical,
}

impl Place {
    const ALL: [Place; 4] = [
        Place::TopSilk,
        Place::BottomSilk,
        Place::Copper,
        Place::Mechanical,
    ];

    fn label(self) -> &'static str {
        match self {
            Place::TopSilk => "Top silk",
            Place::BottomSilk => "Bottom silk",
            Place::Copper => "Current copper layer",
            Place::Mechanical => "Mechanical",
        }
    }
}

pub struct Editing {
    pub sel: Sel,
    pub clip: Option<Clip>,
    /// A box being dragged out (board mil): where it started.
    pub boxing: Option<[f32; 2]>,
    /// The selection being dragged: where the drag started.
    pub dragging: Option<[f32; 2]>,
    pub text: String,
    pub text_on: Place,
    pub dim_on: Place,
    /// Which mechanical layer.
    pub mech: u8,
    pub height: f64,
    pub width: f64,
    pub rotation: f64,
    /// Points of the dimension being placed: two ends, then the offset.
    pub dim: Vec<[f64; 2]>,
    pub units: Units,
    /// Arrow keys move the selection this far (mil); ten times with Shift.
    pub nudge: f64,
}

impl Default for Editing {
    fn default() -> Self {
        Editing {
            sel: Sel::default(),
            clip: None,
            boxing: None,
            dragging: None,
            text: "TEXT".into(),
            text_on: Place::TopSilk,
            dim_on: Place::Mechanical,
            mech: 1,
            height: 60.0,
            width: 6.0,
            rotation: 0.0,
            dim: Vec::new(),
            units: Units::Mil,
            nudge: 5.0,
        }
    }
}

const SEL: Color32 = Color32::from_rgb(90, 220, 255);

/// What a primitive stands for in a selection: its part, or itself.
pub fn item_of(p: &Prim) -> Sel {
    let mut s = Sel::default();
    match (p.component, p.source) {
        (Some(c), _) => {
            s.parts.insert(c as usize);
        }
        (None, Source::Track(i)) => {
            s.tracks.insert(i);
        }
        (None, Source::Arc(i)) => {
            s.arcs.insert(i);
        }
        (None, Source::Via(i)) => {
            s.vias.insert(i);
        }
        (None, Source::Pad(_)) => {}
    }
    s
}

fn holds(sel: &Sel, p: &Prim) -> bool {
    match (p.component, p.source) {
        (Some(c), _) => sel.parts.contains(&(c as usize)),
        (None, Source::Track(i)) => sel.tracks.contains(&i),
        (None, Source::Arc(i)) => sel.arcs.contains(&i),
        (None, Source::Via(i)) => sel.vias.contains(&i),
        (None, Source::Pad(_)) => false,
    }
}

fn shifted(g: &Geom, d: [f32; 2]) -> Geom {
    match *g {
        Geom::Seg { x1, y1, x2, y2, w } => Geom::Seg {
            x1: x1 + d[0],
            y1: y1 + d[1],
            x2: x2 + d[0],
            y2: y2 + d[1],
            w,
        },
        Geom::Circle { x, y, r } => Geom::Circle {
            x: x + d[0],
            y: y + d[1],
            r,
        },
        Geom::Rect { x, y, sx, sy, rot } => Geom::Rect {
            x: x + d[0],
            y: y + d[1],
            sx,
            sy,
            rot,
        },
    }
}

fn seg(t: &Track) -> Geom {
    Geom::Seg {
        x1: t.start.x.mil() as f32,
        y1: t.start.y.mil() as f32,
        x2: t.end.x.mil() as f32,
        y2: t.end.y.mil() as f32,
        w: t.width.mil() as f32,
    }
}

impl Editing {
    /// The layer text or a dimension goes on; `active` is the current layer.
    fn layer(&self, on: Place, active: Slot) -> LayerRef {
        match on {
            Place::TopSilk => LayerRef::TopSilk,
            Place::BottomSilk => LayerRef::BottomSilk,
            Place::Copper => match active {
                Slot::Copper(l) => LayerRef::Copper(l),
                _ => LayerRef::Copper(0),
            },
            Place::Mechanical => LayerRef::Mechanical(self.mech),
        }
    }

    /// How text sits there: mirrored on the bottom layers.
    fn style(&self, on: Place, active: Slot, copper: u8) -> TextStyle {
        let layer = self.layer(on, active);
        let mirror = match layer {
            LayerRef::BottomSilk => true,
            LayerRef::Copper(l) => copper > 1 && l + 1 == copper,
            _ => false,
        };
        TextStyle {
            layer,
            height: self.height,
            width: self.width,
            rotation_deg: self.rotation,
            mirror,
        }
    }

    pub fn text_tracks(&self, at: [f64; 2], active: Slot, copper: u8) -> Vec<Track> {
        if self.text.trim().is_empty() {
            return Vec::new();
        }
        select::text(
            &self.text,
            at,
            &self.style(self.text_on, active, copper),
            false,
        )
    }

    /// The dimension so far, with the cursor standing in for the next point.
    pub fn dim_tracks(&self, cursor: Option<[f64; 2]>, active: Slot, copper: u8) -> Vec<Track> {
        let mut pts = self.dim.clone();
        pts.extend(cursor);
        if pts.len() < 2 {
            return Vec::new();
        }
        let (a, b) = (pts[0], pts[1]);
        let offset = match pts.get(2) {
            Some(c) => {
                let d = [b[0] - a[0], b[1] - a[1]];
                let len = d[0].hypot(d[1]).max(1e-9);
                // Signed distance to the left of a→b.
                (-(d[1]) * (c[0] - a[0]) + d[0] * (c[1] - a[1])) / len
            }
            None => 0.0,
        };
        let offset = if offset.abs() < 1e-6 { 1e-6 } else { offset };
        select::dimension(
            a,
            b,
            offset,
            &self.style(self.dim_on, active, copper),
            self.units,
        )
    }

    /// A click of the Dimension tool: the dimension once it has three
    /// points.
    pub fn dim_click(&mut self, p: [f64; 2], active: Slot, copper: u8) -> Option<Vec<Track>> {
        if self.dim.len() == 1 && (self.dim[0][0] - p[0]).hypot(self.dim[0][1] - p[1]) < 1e-3 {
            return None;
        }
        if self.dim.len() < 2 {
            self.dim.push(p);
            return None;
        }
        let t = self.dim_tracks(Some(p), active, copper);
        self.dim.clear();
        Some(t)
    }

    /// The selection, highlighted, and shifted by `offset` while dragged.
    pub fn draw(
        &self,
        shapes: &mut Vec<Shape>,
        scene: &Scene,
        view: &View,
        offset: Option<[f32; 2]>,
        picked: bool,
    ) {
        // One clicked track, arc or via shows as picked (in yellow) alone.
        let lone = self.sel.parts.is_empty() && self.sel.len() == 1 && picked;
        if self.sel.is_empty() || (lone && offset.is_none()) {
            return;
        }
        let d = offset.unwrap_or([0.0, 0.0]);
        let vis = view.visible();
        let pad = 40.0 / view.scale;
        for p in scene.prims.iter().filter(|p| holds(&self.sel, p)) {
            if !scene.shows(p) && p.slot != Slot::Mechanical {
                continue;
            }
            let g = shifted(&p.geom, d);
            let b = g.bbox();
            if b[2] < vis[0] - pad
                || b[0] > vis[2] + pad
                || b[3] < vis[1] - pad
                || b[1] > vis[3] + pad
            {
                continue;
            }
            if offset.is_none() {
                draw(
                    shapes,
                    view,
                    &grow(&g, 1.5 / view.scale),
                    Color32::from_rgb(20, 20, 28),
                );
                draw(shapes, view, &g, SEL);
            } else {
                draw(shapes, view, &g, SEL.gamma_multiply(0.7));
            }
        }
        // A dashed frame round each selected part.
        if offset.is_none() && !self.sel.parts.is_empty() {
            let all = select::all_part_bounds(&scene.board);
            for &i in &self.sel.parts {
                if let Some(b) = all.get(i).copied().flatten() {
                    let r = Rect::from_two_pos(
                        view.to_screen(b[0] as f32, b[1] as f32),
                        view.to_screen(b[2] as f32, b[3] as f32),
                    )
                    .expand(3.0);
                    let c = [
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                        r.left_top(),
                    ];
                    shapes.extend(Shape::dashed_line(&c, Stroke::new(1.0f32, SEL), 4.0, 3.0));
                }
            }
        }
    }

    /// The box being dragged out: blue picks what is wholly inside (dragged
    /// to the right), green what it touches (dragged to the left).
    pub fn draw_box(&self, shapes: &mut Vec<Shape>, view: &View, to: [f32; 2]) {
        let Some(a) = self.boxing else { return };
        let touching = to[0] < a[0];
        let c = if touching {
            Color32::from_rgb(110, 220, 120)
        } else {
            Color32::from_rgb(90, 160, 255)
        };
        let r = Rect::from_two_pos(view.to_screen(a[0], a[1]), view.to_screen(to[0], to[1]));
        shapes.push(Shape::rect_filled(r, 0.0, c.gamma_multiply(0.12)));
        let pts = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        shapes.extend(Shape::dashed_line(&pts, Stroke::new(1.0f32, c), 5.0, 3.0));
    }

    /// Text or a dimension not yet placed, drawn where it would go.
    pub fn draw_preview(shapes: &mut Vec<Shape>, view: &View, tracks: &[Track]) {
        let c = Color32::from_rgb(255, 220, 120);
        for t in tracks {
            draw(shapes, view, &seg(t), c);
        }
    }

    /// Mark for a dimension point placed so far.
    pub fn draw_dim_points(&self, shapes: &mut Vec<Shape>, view: &View) {
        for p in &self.dim {
            let c: Pos2 = view.to_screen(p[0] as f32, p[1] as f32);
            shapes.push(Shape::circle_stroke(
                c,
                5.0,
                Stroke::new(1.5f32, Color32::from_rgb(255, 220, 120)),
            ));
        }
    }

    fn layer_picker(&mut self, ui: &mut egui::Ui, dim: bool) {
        let id = if dim { "dim_layer" } else { "text_layer" };
        let on = if dim {
            &mut self.dim_on
        } else {
            &mut self.text_on
        };
        egui::ComboBox::from_id_salt(id)
            .selected_text(on.label())
            .show_ui(ui, |ui| {
                for p in Place::ALL {
                    ui.selectable_value(on, p, p.label());
                }
            });
        if *on == Place::Mechanical {
            ui.add(
                egui::DragValue::new(&mut self.mech)
                    .range(1..=32)
                    .prefix("Mech "),
            );
        }
    }

    fn sizes(&mut self, ui: &mut egui::Ui) {
        ui.label("Height");
        ui.add(
            egui::DragValue::new(&mut self.height)
                .speed(1.0)
                .range(5.0..=2000.0)
                .suffix(" mil"),
        );
        ui.label("Stroke");
        ui.add(
            egui::DragValue::new(&mut self.width)
                .speed(0.2)
                .range(1.0..=200.0)
                .suffix(" mil"),
        );
    }

    /// Options of the Text tool, in the tool bar.
    pub fn text_bar(&mut self, ui: &mut egui::Ui) {
        ui.label("Text");
        ui.add(egui::TextEdit::singleline(&mut self.text).desired_width(160.0));
        self.layer_picker(ui, false);
        self.sizes(ui);
        ui.label("Angle");
        egui::ComboBox::from_id_salt("text_rot")
            .selected_text(format!("{}°", self.rotation))
            .width(60.0)
            .show_ui(ui, |ui| {
                for a in [0.0, 90.0, 180.0, 270.0] {
                    ui.selectable_value(&mut self.rotation, a, format!("{a}°"));
                }
            });
        ui.label(RichText::new("Click to place. Esc ends.").weak());
    }

    /// Options of the Dimension tool, in the tool bar.
    pub fn dim_bar(&mut self, ui: &mut egui::Ui) {
        self.layer_picker(ui, true);
        self.sizes(ui);
        egui::ComboBox::from_id_salt("dim_units")
            .selected_text(match self.units {
                Units::Mil => "mil",
                Units::Mm => "mm",
            })
            .width(50.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.units, Units::Mil, "mil");
                ui.selectable_value(&mut self.units, Units::Mm, "mm");
            });
        let hint = match self.dim.len() {
            0 => "Click the first point (snaps to pads, vias and track ends).",
            1 => "Click the second point.",
            _ => "Move to set where the line sits, then click.",
        };
        ui.label(RichText::new(hint).weak());
    }
}
