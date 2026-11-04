//! The board canvas: draws the scene and handles zoom, pan and picking.

use crate::scene::{Geom, Prim, Slot};
use crate::App;
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
    if let Some([x, y]) = pointer {
        if let Some(i) = scene.pick(x, y, 3.0 / view.scale, app.active) {
            let p = &scene.prims[i];
            app.hover = p
                .net
                .and_then(|n| scene.board.nets.get(n as usize))
                .map(|n| n.name.clone());
            if let Some(c) = p
                .component
                .and_then(|c| scene.board.components.get(c as usize))
            {
                app.hover = Some(format!(
                    "{}  ·  {}",
                    app.hover.clone().unwrap_or_else(|| "no net".into()),
                    c.designator
                ));
            }
        }
    }
    if resp.clicked() {
        if let Some([x, y]) = pointer {
            app.marker = None;
            match scene
                .pick(x, y, 3.0 / view.scale, app.active)
                .and_then(|i| scene.prims[i].net)
            {
                Some(n) => {
                    app.selected = Some(n);
                    app.message = None;
                }
                None => app.selected = None,
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
            Slot::Other => -1000,
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

    if let Some([x, y]) = app.marker {
        shapes.push(Shape::circle_stroke(
            view.to_screen(x, y),
            18.0,
            Stroke::new(2.0_f32, Color32::WHITE),
        ));
    }
    painter.extend(shapes);
}
