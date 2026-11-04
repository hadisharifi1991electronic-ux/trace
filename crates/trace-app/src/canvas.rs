//! The board canvas: draws the scene and handles zoom, pan and picking.

use crate::scene::{Geom, Prim, Scene, Slot};
use crate::{App, Measure, Tool, MEASURE};
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};

/// Maps board mil (y up) to screen points.
#[derive(Clone, Copy)]
pub struct View {
    pub scale: f32,
    pub tx: f32,
    pub ty: f32,
    pub flip: bool,
    rect: Rect,
}

impl Default for View {
    fn default() -> Self {
        View {
            scale: 0.1,
            tx: 0.0,
            ty: 0.0,
            flip: false,
            rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)),
        }
    }
}

impl View {
    fn to_screen(self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.rect.min.x + self.tx + if self.flip { -x } else { x } * self.scale,
            self.rect.min.y + self.ty - y * self.scale,
        )
    }
    fn to_board(self, p: Pos2) -> [f32; 2] {
        let x = (p.x - self.rect.min.x - self.tx) / self.scale;
        [
            if self.flip { -x } else { x },
            -(p.y - self.rect.min.y - self.ty) / self.scale,
        ]
    }
    pub fn fit_rect(&mut self, b: [f32; 4], fill: f32) {
        let (w, h) = ((b[2] - b[0]).max(50.0), (b[3] - b[1]).max(50.0));
        self.scale = (self.rect.width() / w).min(self.rect.height() / h) * fill;
        self.center_on([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0], self.scale);
    }
    pub fn center_on(&mut self, [x, y]: [f32; 2], scale: f32) {
        self.scale = scale;
        self.tx = self.rect.width() / 2.0 - if self.flip { -x } else { x } * scale;
        self.ty = self.rect.height() / 2.0 + y * scale;
    }
    /// Visible area in board coordinates.
    fn visible(&self) -> [f32; 4] {
        let a = self.to_board(self.rect.min);
        let b = self.to_board(self.rect.max);
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ]
    }
}

fn rgba(c: [u8; 3], a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (a * 255.0) as u8)
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn draw(shapes: &mut Vec<Shape>, view: &View, g: &Geom, color: Color32) {
    match *g {
        Geom::Seg { x1, y1, x2, y2, w } => {
            let (a, b) = (view.to_screen(x1, y1), view.to_screen(x2, y2));
            let px = (w * view.scale).max(1.0);
            shapes.push(Shape::line_segment([a, b], Stroke::new(px, color)));
            if px > 3.0 {
                // Round ends, as copper is drawn by a round aperture.
                shapes.push(Shape::circle_filled(a, px / 2.0, color));
                shapes.push(Shape::circle_filled(b, px / 2.0, color));
            }
        }
        Geom::Circle { x, y, r } => shapes.push(Shape::circle_filled(
            view.to_screen(x, y),
            (r * view.scale).max(0.75),
            color,
        )),
        Geom::Rect { x, y, sx, sy, rot } => {
            let (s, c) = rot.to_radians().sin_cos();
            let pts = [(-sx, -sy), (sx, -sy), (sx, sy), (-sx, sy)].map(|(u, v)| {
                let (u, v) = (u / 2.0, v / 2.0);
                view.to_screen(x + u * c - v * s, y + u * s + v * c)
            });
            shapes.push(Shape::convex_polygon(pts.to_vec(), color, Stroke::NONE));
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    app.view.rect = rect;
    let Some(scene) = &app.scene else { return };
    if app.needs_fit {
        app.view.fit_rect(scene.bounds, 0.92);
        app.needs_fit = false;
    }

    // Zoom about the cursor, pan by dragging.
    if resp.hovered() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let k = zoom * (scroll * 0.0025).exp();
        if (k - 1.0).abs() > 1e-4 {
            if let Some(p) = resp.hover_pos() {
                let (px, py) = (p.x - rect.min.x, p.y - rect.min.y);
                app.view.tx = px - (px - app.view.tx) * k;
                app.view.ty = py - (py - app.view.ty) * k;
                app.view.scale *= k;
            }
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        app.view.tx += d.x;
        app.view.ty += d.y;
    }

    let view = app.view;
    let pointer = resp.hover_pos().map(|p| view.to_board(p));
    app.cursor = pointer;
    app.hover = None;
    let tol = 3.0 / view.scale;
    // The net under the pointer: copper first, then pours.
    let net_at = |x: f32, y: f32| -> (Option<u32>, Option<u32>) {
        match scene.pick(x, y, tol, app.active) {
            Some(i) => (scene.prims[i].net, scene.prims[i].component),
            None => (
                scene
                    .pick_fill(x, y, app.active)
                    .and_then(|f| scene.fills[f].net),
                None,
            ),
        }
    };
    if let Some([x, y]) = pointer {
        let (net, component) = net_at(x, y);
        app.hover = net
            .and_then(|n| scene.board.nets.get(n as usize))
            .map(|n| n.name.clone());
        if let Some(c) = component.and_then(|c| scene.board.components.get(c as usize)) {
            app.hover = Some(format!(
                "{}  ·  {}",
                app.hover.clone().unwrap_or_else(|| "no net".into()),
                c.designator
            ));
        }
    }
    // Measuring snaps to pad and via centres and track ends within 8 px.
    let snapped = pointer.map(|[x, y]| {
        scene
            .snap(x, y, 8.0 / view.scale, None)
            .map(|(p, _)| p)
            .unwrap_or([x, y])
    });
    if resp.clicked() {
        if let (Some([x, y]), Some(at)) = (pointer, snapped) {
            match app.tool {
                Tool::Measure => {
                    app.measure = match app.measure {
                        Some(Measure { a, b: None }) => Some(Measure { a, b: Some(at) }),
                        _ => Some(Measure { a: at, b: None }),
                    };
                }
                Tool::Select => {
                    app.marker = None;
                    match net_at(x, y).0 {
                        Some(n) => {
                            app.selected = Some(n);
                            app.message = None;
                        }
                        None => app.selected = None,
                    }
                }
            }
        }
    }

    let painter = ui.painter_at(rect);
    let visible = view.visible();
    let mut shapes: Vec<Shape> = Vec::with_capacity(60_000);

    // Board body.
    if scene.outline.len() >= 3 {
        let pts: Vec<Pos2> = scene
            .outline
            .iter()
            .map(|p| view.to_screen(p[0], p[1]))
            .collect();
        shapes.push(Shape::closed_line(
            pts,
            Stroke::new(1.5_f32, Color32::from_rgb(217, 194, 122)),
        ));
    }

    // Layers back to front: bottom copper first unless viewing from below.
    let mut order: Vec<(Slot, [u8; 3])> = scene
        .layers
        .iter()
        .filter(|l| l.visible)
        .map(|l| (l.slot, l.color))
        .collect();
    let rank = |s: Slot| -> i32 {
        match s {
            Slot::Copper(i) => -(i as i32),
            Slot::Vias => 50,
            Slot::TopSilk => 100,
            Slot::BottomSilk => -100,
            Slot::Pours | Slot::Other => -1000,
        }
    };
    order.sort_by_key(|(s, _)| if view.flip { -rank(*s) } else { rank(*s) });
    let dim = if app.selected.is_some() { 0.2 } else { 1.0 };
    let tiny = 0.6 / view.scale; // skip shapes smaller than a fraction of a pixel
    let any_copper = scene
        .layers
        .iter()
        .any(|l| matches!(l.slot, Slot::Copper(_)) && l.visible);
    for (slot, color) in &order {
        let alpha = match slot {
            Slot::Copper(_) if *slot == app.active => 0.95,
            Slot::Copper(_) => 0.6,
            _ => 0.9,
        } * dim;
        let col = rgba(*color, alpha);
        if matches!(slot, Slot::Copper(_)) {
            draw_fills(&mut shapes, scene, &view, visible, |f| {
                // Pours sit under the tracks; planes on other layers stay faint.
                let a = if *slot == app.active { 0.5 } else { 0.22 };
                (f.slot == *slot).then_some(rgba(*color, a * dim))
            });
        }
        for p in scene
            .prims
            .iter()
            .filter(|p| p.slot == *slot && !(p.all_layers && p.slot != Slot::Vias))
        {
            let b = p.geom.bbox();
            if !overlaps(b, visible) || (b[2] - b[0] < tiny && b[3] - b[1] < tiny) {
                continue;
            }
            draw(&mut shapes, &view, &p.geom, col);
        }
        // Through-hole pads sit with the outermost visible copper, drawn once.
        let last_copper = order
            .iter()
            .rev()
            .find(|(s, _)| matches!(s, Slot::Copper(_)))
            .map(|(s, _)| *s);
        if Some(*slot) == last_copper {
            let th = rgba([201, 180, 88], 0.95 * dim);
            for p in scene
                .prims
                .iter()
                .filter(|p: &&Prim| p.all_layers && p.slot != Slot::Vias && any_copper)
            {
                if overlaps(p.geom.bbox(), visible) {
                    draw(&mut shapes, &view, &p.geom, th);
                }
            }
        }
    }

    // Selected net on top, in white.
    if let Some(n) = app.selected {
        draw_fills(&mut shapes, scene, &view, visible, |f| {
            (f.net == Some(n)).then_some(Color32::from_white_alpha(70))
        });
        if let Some(list) = scene.by_net.get(&n) {
            for &i in list {
                let p = &scene.prims[i];
                if scene.shows(p) || p.slot == Slot::Vias {
                    draw(&mut shapes, &view, &p.geom, Color32::WHITE);
                }
            }
        }
    }

    // Part designators once zoomed in far enough to read them.
    if view.scale > 0.35 {
        let font = egui::FontId::proportional((view.scale * 22.0).clamp(10.0, 14.0));
        let show_top = scene.layer_visible(Slot::Copper(0));
        let show_bottom = scene.layer_visible(Slot::Copper(scene.copper_count.saturating_sub(1)));
        for c in &scene.board.components {
            let top = c.side == trace_core::Side::Top;
            if (top && !show_top) || (!top && !show_bottom) {
                continue;
            }
            let p = view.to_screen(c.x.mil() as f32, c.y.mil() as f32);
            if !rect.expand(40.0).contains(p) {
                continue;
            }
            let color = if top {
                Color32::WHITE
            } else {
                Color32::from_rgb(199, 184, 255)
            };
            shapes.push(Shape::text(
                &ui.fonts(|f| f.clone()),
                p + egui::vec2(1.0, 1.0),
                egui::Align2::CENTER_CENTER,
                &c.designator,
                font.clone(),
                Color32::BLACK,
            ));
            shapes.push(Shape::text(
                &ui.fonts(|f| f.clone()),
                p,
                egui::Align2::CENTER_CENTER,
                &c.designator,
                font.clone(),
                color,
            ));
        }
    }

    if app.tool == Tool::Measure || app.measure.is_some() {
        draw_measure(ui, &mut shapes, &view, app.measure, snapped);
    }

    if let Some([x, y]) = app.marker {
        shapes.push(Shape::circle_stroke(
            view.to_screen(x, y),
            18.0,
            Stroke::new(2.0_f32, Color32::WHITE),
        ));
    }
    painter.extend(shapes);
}

/// Adds the visible pours that `color_of` gives a colour to, as one mesh.
fn draw_fills(
    shapes: &mut Vec<Shape>,
    scene: &Scene,
    view: &View,
    visible: [f32; 4],
    color_of: impl Fn(&crate::scene::Fill) -> Option<Color32>,
) {
    let mut mesh = egui::Mesh::default();
    for f in &scene.fills {
        if !overlaps(f.bbox, visible) || !scene.fill_shows(f) {
            continue;
        }
        let Some(color) = color_of(f) else { continue };
        let base = mesh.vertices.len() as u32;
        for v in &scene.fill_verts[f.verts.clone()] {
            mesh.colored_vertex(view.to_screen(v[0], v[1]), color);
        }
        mesh.indices
            .extend(scene.fill_tris[f.tris.clone()].iter().map(|&i| base + i));
    }
    if !mesh.indices.is_empty() {
        shapes.push(Shape::mesh(mesh));
    }
}

/// The measurement line, its end markers and a label with the distance.
fn draw_measure(
    ui: &egui::Ui,
    shapes: &mut Vec<Shape>,
    view: &View,
    measure: Option<Measure>,
    cursor: Option<[f32; 2]>,
) {
    let stroke = Stroke::new(1.5_f32, MEASURE);
    let mark = |shapes: &mut Vec<Shape>, p: Pos2| {
        shapes.push(Shape::circle_stroke(p, 5.0, stroke));
        shapes.push(Shape::line_segment(
            [p - egui::vec2(8.0, 0.0), p + egui::vec2(8.0, 0.0)],
            stroke,
        ));
        shapes.push(Shape::line_segment(
            [p - egui::vec2(0.0, 8.0), p + egui::vec2(0.0, 8.0)],
            stroke,
        ));
    };
    let Some(m) = measure else {
        if let Some([x, y]) = cursor {
            mark(shapes, view.to_screen(x, y));
        }
        return;
    };
    let Some(b) = m.b.or(cursor) else { return };
    let (pa, pb) = (view.to_screen(m.a[0], m.a[1]), view.to_screen(b[0], b[1]));
    // Dashed guides for the x and y legs, then the direct line.
    let corner = view.to_screen(b[0], m.a[1]);
    let guide = Stroke::new(1.0_f32, MEASURE.gamma_multiply(0.5));
    shapes.extend(Shape::dashed_line(&[pa, corner, pb], guide, 5.0, 4.0));
    shapes.push(Shape::line_segment([pa, pb], stroke));
    mark(shapes, pa);
    mark(shapes, pb);
    let (dx, dy) = (b[0] - m.a[0], b[1] - m.a[1]);
    let d = dx.hypot(dy);
    let text = format!(
        "{d:.2} mil  ·  {:.4} mm\nΔx {:.2}   Δy {:.2} mil",
        d * 0.0254,
        dx.abs(),
        dy.abs()
    );
    let font = egui::FontId::proportional(13.0);
    let galley = ui.fonts(|f| f.layout_no_wrap(text, font, Color32::BLACK));
    let size = galley.size();
    // Beside the middle of the line, kept inside the canvas.
    let mid = pa + (pb - pa) / 2.0 + egui::vec2(12.0, -size.y - 12.0);
    let area = view.rect.shrink(8.0);
    let at = Pos2::new(
        mid.x
            .clamp(area.min.x, (area.max.x - size.x).max(area.min.x)),
        mid.y
            .clamp(area.min.y, (area.max.y - size.y).max(area.min.y)),
    );
    shapes.push(Shape::rect_filled(
        Rect::from_min_size(at, size).expand(5.0),
        4.0,
        MEASURE,
    ));
    shapes.push(Shape::galley(at, galley, Color32::BLACK));
}
