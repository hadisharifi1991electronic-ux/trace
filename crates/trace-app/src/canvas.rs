//! The board canvas: draws the scene and handles zoom, pan and picking.

use crate::scene::{Geom, Prim, Scene, Slot, Source};
use crate::{App, Measure, Tool, MEASURE};
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};
use trace_core::LayerRef;

/// Maps board mil (y up) to screen points.
#[derive(Clone, Copy)]
pub struct View {
    pub scale: f32,
    pub tx: f32,
    pub ty: f32,
    pub flip: bool,
    pub rect: Rect,
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
    pub fn to_screen(self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.rect.min.x + self.tx + if self.flip { -x } else { x } * self.scale,
            self.rect.min.y + self.ty - y * self.scale,
        )
    }
    pub fn to_board(self, p: Pos2) -> [f32; 2] {
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
    pub fn visible(&self) -> [f32; 4] {
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

pub(crate) fn draw(shapes: &mut Vec<Shape>, view: &View, g: &Geom, color: Color32) {
    match *g {
        Geom::Seg { x1, y1, x2, y2, w } => {
            let (a, b) = (view.to_screen(x1, y1), view.to_screen(x2, y2));
            let px = (w * view.scale).max(1.0);
            shapes.push(Shape::line_segment([a, b], Stroke::new(px, color)));
            if px > 1.5 {
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

/// A shape made larger by `e` on every side.
pub(crate) fn grow(g: &Geom, e: f32) -> Geom {
    match *g {
        Geom::Seg { x1, y1, x2, y2, w } => Geom::Seg {
            x1,
            y1,
            x2,
            y2,
            w: w + 2.0 * e,
        },
        Geom::Circle { x, y, r } => Geom::Circle { x, y, r: r + e },
        Geom::Rect { x, y, sx, sy, rot } => Geom::Rect {
            x,
            y,
            sx: sx + 2.0 * e,
            sy: sy + 2.0 * e,
            rot,
        },
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
    match app.focus.take() {
        Some(crate::Focus::Point(p, s)) => app.view.center_on(p, s),
        Some(crate::Focus::Area(b)) => app.view.fit_rect(b, 0.7),
        None => {}
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
    // Pressing on a part and dragging moves it, as in Altium; anywhere
    // else (or with the right button) dragging pans.
    // Shift+drag draws a box to select with; pressing on a selection of
    // more than one part drags all of it.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && app.tool == Tool::Select
        && app.router.active.is_none()
        && app.moving.is_none()
    {
        if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
            let [x, y] = app.view.to_board(o);
            let tol = 3.0 / app.view.scale;
            let hit = scene
                .pick(x, y, tol, app.active, true)
                .or_else(|| scene.pick(x, y, tol, app.active, false));
            let item = hit.map(|i| crate::editing::item_of(&scene.prims[i]));
            let in_sel = item.as_ref().is_some_and(|s| {
                !s.is_empty() && {
                    let mut both = app.edit.sel.clone();
                    both.add(s);
                    both == app.edit.sel
                }
            });
            let single_part = app.edit.sel.parts.len() == 1 && app.edit.sel.len() == 1;
            if ui.input(|i| i.modifiers.shift) {
                app.edit.boxing = Some([x, y]);
            } else if in_sel && !single_part {
                app.edit.dragging = Some([x, y]);
            } else if let Some(c) = hit.and_then(|i| scene.prims[i].component) {
                app.moving =
                    crate::placing::Moving::start(&scene.board, c as usize, [x as f64, y as f64]);
                if let Some(d) = scene.board.components.get(c as usize) {
                    app.message = Some((
                        format!(
                            "Moving {}: Space turns it, L flips it to the other side, Escape puts it back.",
                            d.designator
                        ),
                        false,
                    ));
                }
            }
        }
    }
    let mut placed: Option<(String, Vec<trace_core::edit::Edit>)> = None;
    if app.moving.is_some() {
        if let Some(p) = resp.interact_pointer_pos().or(resp.hover_pos()) {
            let [x, y] = app.view.to_board(p);
            if let Some(m) = app.moving.as_mut() {
                m.follow([x as f64, y as f64]);
            }
        }
        let floating = app.moving.as_ref().is_some_and(|m| m.floating);
        let done = if floating {
            resp.clicked() || resp.drag_stopped()
        } else {
            resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down())
        };
        if done {
            if let Some(m) = app.moving.take() {
                if floating {
                    app.message = None;
                }
                if m.moved() {
                    let name = scene
                        .board
                        .components
                        .get(m.index)
                        .map(|c| c.designator.clone())
                        .unwrap_or_default();
                    let edits = m.edits(&scene.board);
                    // One undo takes back placing and moving alike.
                    app.join_next = floating && !edits.is_empty();
                    let verb = if floating { "Placed" } else { "Moved" };
                    placed = Some((format!("{verb} {name}"), edits));
                }
                app.moving = None;
            }
        }
    } else if app.edit.boxing.is_some() || app.edit.dragging.is_some() {
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            let end = resp
                .interact_pointer_pos()
                .or(resp.hover_pos())
                .map(|p| app.view.to_board(p));
            if let (Some(a), Some(b)) = (app.edit.boxing.take(), end) {
                let bx = [
                    a[0].min(b[0]) as f64,
                    a[1].min(b[1]) as f64,
                    a[0].max(b[0]) as f64,
                    a[1].max(b[1]) as f64,
                ];
                let found = trace_core::select::Sel::in_box(&scene.board, bx, b[0] < a[0]);
                if ui.input(|i| i.modifiers.command) {
                    app.edit.sel.add(&found);
                } else {
                    app.edit.sel = found;
                }
                app.picked = None;
                app.message = Some((select_hint(&app.edit.sel), false));
            }
            if let (Some(a), Some(b)) = (app.edit.dragging.take(), end) {
                let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
                let edits = trace_core::select::move_by(&scene.board, &app.edit.sel, dx, dy);
                if !edits.is_empty() {
                    placed = Some((format!("Moved {}", app.edit.sel.describe()), edits));
                }
            }
            app.edit.boxing = None;
            app.edit.dragging = None;
        }
    } else if resp.dragged() {
        let d = resp.drag_delta();
        app.view.tx += d.x;
        app.view.ty += d.y;
    }

    let view = app.view;
    if let Some(layer) = app.router.active.as_ref().map(|a| a.layer()) {
        app.active = Slot::Copper(layer);
    }
    let pointer = resp.hover_pos().map(|p| view.to_board(p));
    app.cursor = pointer;
    app.hover = None;
    let tol = 3.0 / view.scale;
    // The net under the pointer, as drawn: the active layer first.
    let net_at = |x: f32, y: f32| scene.net_at(x, y, tol, app.active);
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
    // Routing follows the cursor, landing on copper of its own net.
    let route_snap = |x: f32, y: f32, layer: u8| -> Option<([f64; 2], Option<u32>)> {
        let slot = Slot::Copper(layer);
        scene
            .snap(x, y, 8.0 / view.scale, Some(slot))
            .map(|(p, n)| ([p[0] as f64, p[1] as f64], n))
            .or_else(|| {
                scene
                    .pick(x, y, tol, slot, true)
                    .map(|i| ([x as f64, y as f64], scene.prims[i].net))
            })
    };
    if let (Some(layer), Some([x, y])) = (app.router.active.as_ref().map(|a| a.layer()), pointer) {
        let snap = route_snap(x, y, layer);
        let board = &scene.board;
        // A bus's far pads can spread wider than a pair's.
        let reach = if app.router.active.as_ref().is_some_and(|a| a.is_bus()) {
            2000.0
        } else {
            400.0
        };
        app.router
            .update(board, [x as f64, y as f64], snap, |p, net| {
                partner_pad(board, p, net, layer, reach)
            });
    }
    let mut pending: Option<Vec<trace_core::edit::Edit>> = None;
    let mut pending_label = "Routed a track".to_string();
    if let Some((label, edits)) = placed {
        pending_label = label;
        pending = Some(edits);
    }
    // Length tuning: meanders on the straight track under the cursor.
    if app.tool == Tool::Tune {
        app.router.tune.preview = None;
        if let Some([x, y]) = pointer {
            let picked = scene
                .pick(x, y, tol, app.active, true)
                .map(|i| &scene.prims[i])
                .and_then(|p| match (p.source, p.slot) {
                    (Source::Track(t), Slot::Copper(l)) => Some((t, l)),
                    _ => None,
                });
            if let Some((ti, layer)) = picked {
                tune_preview(&mut app.router, &mut app.cm, scene, ti, layer);
            }
        }
    }
    if resp.clicked() {
        if let (Some([x, y]), Some(at)) = (pointer, snapped) {
            match app.tool {
                Tool::Tune => {
                    if let Some(p) = &app.router.tune.preview {
                        if p.hit.is_none() && p.added > 0.0 {
                            let name = scene.board.nets[p.net as usize].name.clone();
                            pending_label = format!("Tuned {name} (+{:.1} mil)", p.added);
                            pending = Some(p.edits(&scene.board));
                        }
                    }
                }
                Tool::Route => {
                    let ctrl = ui.input(|i| i.modifiers.command);
                    if app.router.active.is_some() {
                        pending = app.router.click(&scene.board);
                    } else if let (true, Slot::Copper(layer)) = (ctrl, app.active) {
                        // Ctrl+click gathers a bus.
                        match route_snap(x, y, layer) {
                            Some((p, Some(net))) => {
                                let n = app.router.pick_bus(p, net);
                                app.router.message = None;
                                app.message = Some((format!("Bus: {n} nets"), false));
                            }
                            _ => {
                                app.router.message = Some(
                                    "Ctrl+click a pad or track end of a net to add it to the bus."
                                        .into(),
                                )
                            }
                        }
                    } else if let (true, Slot::Copper(layer)) =
                        (app.router.bus.len() >= 2, app.active)
                    {
                        let n = app.router.bus.len();
                        if app.router.start_bus(layer) {
                            app.message = Some((format!("Routing a bus of {n} nets"), false));
                        }
                    } else if let Slot::Copper(layer) = app.active {
                        match route_snap(x, y, layer) {
                            Some((p, Some(net))) => {
                                let pair = if app.router.pairs {
                                    pair_start(&app.router, &mut app.imp, scene, p, net, layer)
                                } else {
                                    None
                                };
                                if let Some((info, _)) = &pair {
                                    app.message = Some((
                                        format!("Routing pair {} together", info.name),
                                        false,
                                    ));
                                }
                                app.router.start(p, Some(net), layer, pair);
                                app.selected = Some(net);
                            }
                            Some((_, None)) => {
                                app.router.message = Some("That copper has no net to route.".into())
                            }
                            None => {
                                app.router.message = Some(
                                    "Start on a pad, via or track of the layer you route on."
                                        .into(),
                                )
                            }
                        }
                    } else {
                        app.router.message = Some("Choose a copper layer to route on.".into());
                    }
                }
                Tool::Measure => {
                    app.measure = match app.measure {
                        Some(Measure { a, b: None }) => Some(Measure { a, b: Some(at) }),
                        _ => Some(Measure { a: at, b: None }),
                    };
                }
                Tool::Text => {
                    let t =
                        app.edit
                            .text_tracks([x as f64, y as f64], app.active, scene.copper_count);
                    if !t.is_empty() {
                        pending_label =
                            format!("Placed text \u{201c}{}\u{201d}", app.edit.text.trim());
                        pending = Some(trace_core::select::add_tracks(&scene.board, t).0);
                    }
                }
                Tool::Dimension => {
                    if let Some(t) = app.edit.dim_click(
                        [at[0] as f64, at[1] as f64],
                        app.active,
                        scene.copper_count,
                    ) {
                        if !t.is_empty() {
                            pending_label = "Placed a dimension".into();
                            pending = Some(trace_core::select::add_tracks(&scene.board, t).0);
                            app.show_mechanical = true;
                        }
                    }
                }
                Tool::Select => {
                    app.marker = None;
                    let hit = scene
                        .pick(x, y, tol, app.active, true)
                        .or_else(|| scene.pick(x, y, tol, app.active, false));
                    let item = hit
                        .map(|i| crate::editing::item_of(&scene.prims[i]))
                        .unwrap_or_default();
                    if ui.input(|i| i.modifiers.command) {
                        // Ctrl+click adds to the selection or takes away.
                        app.edit.sel.toggle(&item);
                        app.message = Some((select_hint(&app.edit.sel), false));
                        app.picked = None;
                        app.edit.boxing = None;
                    } else {
                        app.edit.sel = item;
                    }
                    app.picked = hit.map(|i| scene.prims[i].source);
                    match net_at(x, y).0 {
                        Some(n) => {
                            app.selected = Some(n);
                            app.message = matches!(
                                app.picked,
                                Some(Source::Track(_) | Source::Arc(_) | Source::Via(_))
                            )
                            .then(|| {
                                (
                                    "Delete removes just the segment in yellow; Shift+Delete unroutes the whole net.".to_string(),
                                    false,
                                )
                            });
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
    for slot in scene.slots.iter().filter(|s| s.len() >= 3) {
        let pts: Vec<Pos2> = slot.iter().map(|p| view.to_screen(p[0], p[1])).collect();
        shapes.push(Shape::closed_line(
            pts,
            Stroke::new(1.0_f32, Color32::from_rgb(217, 194, 122)),
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
            Slot::Mechanical => 120,
            Slot::BottomSilk => -100,
            Slot::Pours | Slot::Other => -1000,
        }
    };
    order.sort_by_key(|(s, _)| if view.flip { -rank(*s) } else { rank(*s) });
    let dim = if app.selected.is_some() { 0.2 } else { 1.0 };
    // Tracks the route is pushing aside are drawn where they will go.
    let shoved_away: Vec<usize> = app
        .router
        .active
        .as_ref()
        .and_then(|a| a.shove.as_ref())
        .map(|s| s.remove.clone())
        .unwrap_or_default();
    // A part being dragged, and the tracks stretching to it, are drawn
    // where they go instead.
    let moving = app.moving.as_ref();
    let lifted = |p: &Prim| match moving {
        Some(m) => {
            p.component == Some(m.index as u32)
                || matches!(p.source, Source::Track(t) if m.ends.iter().any(|e| e.0 == t))
        }
        None => false,
    };
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
            .filter(|p| !matches!(p.source, Source::Track(t) if shoved_away.contains(&t)))
            .filter(|p| !lifted(p))
        {
            let b = p.geom.bbox();
            if !overlaps(b, visible) || (b[2] - b[0] < tiny && b[3] - b[1] < tiny) {
                continue;
            }
            let c = match p.source {
                Source::Via(vi) => rgba(via_color(scene, vi), 0.95 * dim),
                _ => col,
            };
            draw(&mut shapes, &view, &p.geom, c);
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
                .filter(|p| !lifted(p))
            {
                if overlaps(p.geom.bbox(), visible) {
                    draw(&mut shapes, &view, &p.geom, th);
                }
            }
        }
    }

    if let Some(m) = moving {
        draw_moving(&mut shapes, scene, &view, m);
    }

    // Selected net on top, in white with a dark edge, over a veil that
    // darkens the rest; stacked pours on many-layer boards can make the
    // board so pale that white alone would not show.
    if let Some(n) = app.selected {
        shapes.push(Shape::rect_filled(
            rect,
            0.0,
            Color32::from_black_alpha(150),
        ));
        draw_fills(&mut shapes, scene, &view, visible, |f| {
            (f.net == Some(n)).then_some(Color32::from_rgba_unmultiplied(255, 255, 255, 110))
        });
        if let Some(list) = scene.by_net.get(&n) {
            let shown: Vec<&Geom> = list
                .iter()
                .map(|&i| &scene.prims[i])
                .filter(|p| scene.shows(p) || p.slot == Slot::Vias)
                .map(|p| &p.geom)
                .collect();
            let edge = 1.5 / view.scale;
            for g in &shown {
                draw(
                    &mut shapes,
                    &view,
                    &grow(g, edge),
                    Color32::from_rgb(20, 20, 28),
                );
            }
            for g in &shown {
                draw(&mut shapes, &view, g, Color32::WHITE);
            }
        }
    }
    // The clicked segment, arc or via: what Delete removes. Drawn apart
    // from its net, with handles at segment ends, as Altium shows it.
    if let Some(src @ (Source::Track(_) | Source::Arc(_) | Source::Via(_))) = app.picked {
        let pick = Color32::from_rgb(255, 196, 40);
        for p in scene.prims.iter().filter(|p| p.source == src) {
            draw(
                &mut shapes,
                &view,
                &grow(&p.geom, 1.5 / view.scale),
                Color32::from_rgb(20, 20, 28),
            );
            draw(&mut shapes, &view, &p.geom, pick);
            if let (Source::Track(_), Geom::Seg { x1, y1, x2, y2, .. }) = (src, &p.geom) {
                for (x, y) in [(*x1, *y1), (*x2, *y2)] {
                    let c = view.to_screen(x, y);
                    shapes.push(Shape::rect_filled(
                        Rect::from_center_size(c, egui::vec2(7.0, 7.0)),
                        0.0,
                        Color32::WHITE,
                    ));
                    shapes.push(Shape::rect_stroke(
                        Rect::from_center_size(c, egui::vec2(7.0, 7.0)),
                        0.0,
                        Stroke::new(1.0f32, Color32::from_rgb(20, 20, 28)),
                    ));
                }
            }
        }
    }

    // The selection, where it is or where it is being dragged.
    let drag_offset = app
        .edit
        .dragging
        .zip(pointer)
        .map(|(a, b)| [b[0] - a[0], b[1] - a[1]]);
    app.edit
        .draw(&mut shapes, scene, &view, drag_offset, app.picked.is_some());
    if let Some(p) = pointer {
        app.edit.draw_box(&mut shapes, &view, p);
    }
    match app.tool {
        Tool::Text => {
            if let Some([x, y]) = pointer {
                let t = app
                    .edit
                    .text_tracks([x as f64, y as f64], app.active, scene.copper_count);
                crate::editing::Editing::draw_preview(&mut shapes, &view, &t);
            }
        }
        Tool::Dimension => {
            let t = app.edit.dim_tracks(
                snapped.map(|p| [p[0] as f64, p[1] as f64]),
                app.active,
                scene.copper_count,
            );
            crate::editing::Editing::draw_preview(&mut shapes, &view, &t);
            app.edit.draw_dim_points(&mut shapes, &view);
        }
        _ => {}
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
            let (x, y) = match moving.filter(|m| std::ptr::eq(&scene.board.components[m.index], c))
            {
                Some(m) => (m.to.x.mil(), m.to.y.mil()),
                None => (c.x.mil(), c.y.mil()),
            };
            let p = view.to_screen(x as f32, y as f32);
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

    let lifted_part = moving.map(|m| m.index as u32);
    draw_holes(&mut shapes, scene, &view, visible, lifted_part);
    if let Some(m) = moving {
        // The moving part's holes, where it goes.
        let t = m.transform(&scene.board);
        for p in scene.board.geometry.pads.iter() {
            if p.component != lifted_part || p.hole.0 <= 0 {
                continue;
            }
            let c = t.xy([p.at.x.mil(), p.at.y.mil()]);
            shapes.push(Shape::circle_filled(
                view.to_screen(c[0] as f32, c[1] as f32),
                (p.hole.mil() / 2.0) as f32 * view.scale,
                Color32::from_rgb(24, 26, 31),
            ));
        }
    }
    draw_labels(
        ui,
        &mut shapes,
        scene,
        &view,
        visible,
        app.active,
        lifted_part,
    );

    if app.tool == Tool::Measure || app.tool == Tool::Dimension || app.measure.is_some() {
        draw_measure(ui, &mut shapes, &view, app.measure, snapped);
    }
    if resp.double_clicked() && app.tool == Tool::Route && pending.is_none() {
        pending = app.router.finish(&scene.board);
    }
    if let Some(a) = &app.router.active {
        draw_route(&mut shapes, &view, &app.router, a);
    }
    // Pads gathered for a bus, ringed.
    for (p, _) in &app.router.bus {
        shapes.push(Shape::circle_stroke(
            view.to_screen(p[0] as f32, p[1] as f32),
            7.0,
            Stroke::new(2.0_f32, Color32::from_rgb(255, 220, 120)),
        ));
    }
    if let Some(p) = &app.router.tune.preview {
        let color = if p.hit.is_some() {
            Color32::from_rgb(240, 70, 70)
        } else {
            Color32::from_rgb(255, 220, 120)
        };
        for piece in &p.pieces {
            let (a, b) = (piece.start(), piece.end());
            draw(
                &mut shapes,
                &view,
                &Geom::Seg {
                    x1: a[0] as f32,
                    y1: a[1] as f32,
                    x2: b[0] as f32,
                    y2: b[1] as f32,
                    w: p.width as f32,
                },
                color,
            );
        }
    }

    // Rooms: a dashed outline with the room's name at its top left.
    let room_color = Color32::from_rgba_unmultiplied(200, 160, 255, 170);
    for (name, ring) in &scene.rooms {
        let mut pts: Vec<Pos2> = ring.iter().map(|p| view.to_screen(p[0], p[1])).collect();
        if pts.len() < 3 {
            continue;
        }
        let (min, max) = pts.iter().fold(
            (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN)),
            |(a, b), p| (a.min(*p), b.max(*p)),
        );
        if !rect.intersects(egui::Rect::from_min_max(min, max)) {
            continue;
        }
        pts.push(pts[0]);
        shapes.extend(Shape::dashed_line(
            &pts,
            Stroke::new(1.2_f32, room_color),
            8.0,
            5.0,
        ));
        shapes.push(Shape::text(
            &ui.fonts(|f| f.clone()),
            min + egui::vec2(4.0, 3.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::proportional(12.0),
            room_color,
        ));
    }

    // Design rule violations on the layers shown: an X in the colour of
    // its kind, the one picked in the list larger.
    if app.drc.markers {
        for (i, v) in app.drc.list.iter().enumerate() {
            if v.layer
                .is_some_and(|l| !scene.layer_visible(Slot::Copper(l)))
            {
                continue;
            }
            let p = view.to_screen(v.at[0] as f32, v.at[1] as f32);
            if !rect.expand(10.0).contains(p) {
                continue;
            }
            let big = app.drc.current == Some(i);
            let r = if big { 9.0 } else { 5.0 };
            let st = Stroke::new(
                if big { 2.5_f32 } else { 1.8_f32 },
                crate::drc::color(v.kind),
            );
            shapes.push(Shape::line_segment(
                [p + egui::vec2(-r, -r), p + egui::vec2(r, r)],
                st,
            ));
            shapes.push(Shape::line_segment(
                [p + egui::vec2(-r, r), p + egui::vec2(r, -r)],
                st,
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
    if let Some(edits) = pending {
        if app.tool == Tool::Route {
            app.selected = None;
        }
        app.router.tune.preview = None;
        app.router.shoved = None;
        app.apply_edits(&pending_label, edits);
        if std::mem::take(&mut app.join_next) {
            app.router.history.merge_last();
        }
        if std::mem::take(&mut app.show_mechanical) {
            if let Some(scene) = app.scene.as_mut() {
                for l in scene
                    .layers
                    .iter_mut()
                    .filter(|l| l.slot == Slot::Mechanical)
                {
                    l.visible = true;
                }
            }
        }
    } else if let Some(edits) = app.router.shoved.take() {
        let n = edits
            .iter()
            .filter(|e| matches!(e, trace_core::edit::Edit::RemoveTrack { .. }))
            .count();
        let label = if n == 1 {
            "Pushed a track aside".to_string()
        } else {
            format!("Pushed {n} tracks aside")
        };
        app.apply_edits(&label, edits);
    }
}

/// The route being drawn: fixed corners in amber, the part following the
/// cursor lighter (red where it breaks clearance), with its clearance halo.
fn draw_route(
    shapes: &mut Vec<Shape>,
    view: &View,
    r: &crate::router::Router,
    a: &crate::router::Active,
) {
    let amber = Color32::from_rgb(255, 196, 64);
    let seg = |shapes: &mut Vec<Shape>, p: [f64; 2], q: [f64; 2], w: f64, c: Color32| {
        draw(
            shapes,
            view,
            &Geom::Seg {
                x1: p[0] as f32,
                y1: p[1] as f32,
                x2: q[0] as f32,
                y2: q[1] as f32,
                w: w as f32,
            },
            c,
        );
    };
    let halo = Color32::from_white_alpha(28);
    let bad = a.hit.is_some();
    let live = if bad {
        Color32::from_rgb(240, 70, 70)
    } else {
        Color32::from_rgb(255, 220, 120)
    };
    let layout = a.layout(r.width);
    // Clearance halo around the part following the cursor, as joined lines
    // so overlaps do not show.
    for p in layout.iter().filter(|p| p.live) {
        let ch = p.piece.chords();
        let Some(first) = ch.first() else { continue };
        let mut pts = vec![view.to_screen(first.0[0] as f32, first.0[1] as f32)];
        pts.extend(
            ch.iter()
                .map(|(_, q)| view.to_screen(q[0] as f32, q[1] as f32)),
        );
        let w = ((p.width + 2.0 * r.clearance) as f32 * view.scale).max(1.0);
        shapes.push(Shape::line(pts, Stroke::new(w, halo)));
    }
    for p in &layout {
        for (x, y) in p.piece.chords() {
            seg(shapes, x, y, p.width, if p.live { live } else { amber });
        }
    }
    // Tracks pushed aside, where they will go.
    if let Some(s) = &a.shove {
        let pushed = Color32::from_rgb(110, 200, 255);
        for t in &s.add {
            seg(
                shapes,
                [t.start.x.mil(), t.start.y.mil()],
                [t.end.x.mil(), t.end.y.mil()],
                t.width.mil(),
                pushed,
            );
        }
    }
    for (v, d, hole) in a.via_shapes() {
        let c = view.to_screen(v[0] as f32, v[1] as f32);
        shapes.push(Shape::circle_filled(
            c,
            (d / 2.0) as f32 * view.scale,
            amber,
        ));
        shapes.push(Shape::circle_filled(
            c,
            (hole / 2.0) as f32 * view.scale,
            Color32::from_rgb(20, 20, 24),
        ));
    }
    if let Some(h) = &a.hit {
        shapes.push(Shape::circle_stroke(
            view.to_screen(h.near[0] as f32, h.near[1] as f32),
            14.0,
            Stroke::new(2.0_f32, Color32::from_rgb(240, 70, 70)),
        ));
    }
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

/// Colour of a via: gold for through vias, and one colour per layer span
/// for blind and buried ones, so staggered vias are told apart at a glance.
fn via_color(scene: &Scene, vi: usize) -> [u8; 3] {
    const SPANS: [[u8; 3]; 8] = [
        [236, 112, 99],
        [93, 173, 226],
        [88, 214, 141],
        [245, 176, 65],
        [175, 122, 197],
        [72, 201, 176],
        [240, 98, 146],
        [149, 165, 166],
    ];
    let v = &scene.board.geometry.vias[vi];
    let (a, z) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
    let last = scene.copper_count.saturating_sub(1);
    if a == 0 && z >= last {
        [201, 180, 88]
    } else {
        SPANS[(a as usize * 7 + z as usize * 3) % SPANS.len()]
    }
}

/// Drill holes of vias and through-hole pads, once they are a few pixels across.
fn draw_holes(
    shapes: &mut Vec<Shape>,
    scene: &Scene,
    view: &View,
    visible: [f32; 4],
    lifted: Option<u32>,
) {
    let hole_color = Color32::from_rgb(24, 26, 31);
    let g = &scene.board.geometry;
    let vias_on = scene.layer_visible(Slot::Vias);
    for p in scene
        .prims
        .iter()
        .filter(|p| lifted.is_none() || p.component != lifted)
    {
        let (at, hole) = match p.source {
            Source::Via(vi) if vias_on => (g.vias[vi].at, g.vias[vi].hole),
            Source::Pad(pi) if p.all_layers && scene.shows(p) => (g.pads[pi].at, g.pads[pi].hole),
            _ => continue,
        };
        let r = hole.mil() as f32 / 2.0;
        if r * view.scale < 1.2 {
            continue;
        }
        let (x, y) = (at.x.mil() as f32, at.y.mil() as f32);
        if !overlaps([x - r, y - r, x + r, y + r], visible) {
            continue;
        }
        shapes.push(Shape::circle_filled(
            view.to_screen(x, y),
            r * view.scale,
            hole_color,
        ));
        // A backdrilled via: a ring the size of the backdrill.
        if let Source::Via(vi) = p.source {
            let f = &g.vias[vi].fab;
            if let Some(d) = f
                .back_top
                .or(f.back_bottom)
                .map(|b| b.diameter.mil() as f32)
            {
                shapes.push(Shape::circle_stroke(
                    view.to_screen(x, y),
                    d / 2.0 * view.scale,
                    Stroke::new(1.5_f32, BACKDRILL),
                ));
            }
        }
    }
}

/// Ring drawn round backdrilled vias.
const BACKDRILL: Color32 = Color32::from_rgb(150, 230, 150);

/// Text centred on `at`, turned by `angle` radians (screen).
fn label(
    ui: &egui::Ui,
    shapes: &mut Vec<Shape>,
    at: Pos2,
    text: &str,
    px: f32,
    angle: f32,
    color: Color32,
) {
    let galley =
        ui.fonts(|f| f.layout_no_wrap(text.to_string(), egui::FontId::proportional(px), color));
    let half = galley.size() / 2.0;
    let (s, c) = angle.sin_cos();
    let off = egui::vec2(-half.x * c + half.y * s, -half.x * s - half.y * c);
    shapes.push(Shape::Text(
        egui::epaint::TextShape::new(at + off, galley, color).with_angle(angle),
    ));
}

/// Net names on tracks and pads, and layer span plus net on vias, once
/// zoomed in far enough to read them (as in Allegro).
fn draw_labels(
    ui: &egui::Ui,
    shapes: &mut Vec<Shape>,
    scene: &Scene,
    view: &View,
    visible: [f32; 4],
    active: Slot,
    lifted: Option<u32>,
) {
    let g = &scene.board.geometry;
    let name = |n: Option<u32>| {
        n.and_then(|n| scene.board.nets.get(n as usize))
            .map(|n| n.name.as_str())
    };
    let text = Color32::from_rgb(250, 250, 250);
    let shadow = Color32::from_black_alpha(200);
    let mut budget = 1500;
    let vias_on = scene.layer_visible(Slot::Vias);
    for p in scene
        .prims
        .iter()
        .filter(|p| lifted.is_none() || p.component != lifted)
    {
        if budget == 0 {
            break;
        }
        let b = p.geom.bbox();
        if !overlaps(b, visible) {
            continue;
        }
        match (p.source, p.geom) {
            (Source::Via(vi), Geom::Circle { x, y, r }) if vias_on => {
                let d = 2.0 * r * view.scale;
                if d < 26.0 {
                    continue;
                }
                let v = &g.vias[vi];
                let span = format!(
                    "{}-{}",
                    v.from_layer.min(v.to_layer) + 1,
                    v.from_layer.max(v.to_layer) + 1
                );
                let px = (d * 0.26).min(16.0);
                let c = view.to_screen(x, y);
                label(
                    ui,
                    shapes,
                    c + egui::vec2(1.0, -px * 0.45 + 1.0),
                    &span,
                    px,
                    0.0,
                    shadow,
                );
                label(
                    ui,
                    shapes,
                    c + egui::vec2(0.0, -px * 0.45),
                    &span,
                    px,
                    0.0,
                    text,
                );
                if let Some(n) = name(p.net) {
                    let px = (d * 0.85 / (n.chars().count().max(3) as f32 * 0.55)).min(px);
                    if px >= 5.0 {
                        label(
                            ui,
                            shapes,
                            c + egui::vec2(1.0, px * 0.6 + 1.0),
                            n,
                            px,
                            0.0,
                            shadow,
                        );
                        label(
                            ui,
                            shapes,
                            c + egui::vec2(0.0, px * 0.6),
                            n,
                            px,
                            0.0,
                            Color32::from_rgb(120, 230, 230),
                        );
                    }
                }
                budget -= 1;
            }
            (Source::Track(_) | Source::Arc(_), Geom::Seg { x1, y1, x2, y2, w })
                if p.slot == active =>
            {
                let Some(n) = name(p.net) else { continue };
                let wpx = w * view.scale;
                if wpx < 8.0 {
                    continue;
                }
                let (a, bb) = (view.to_screen(x1, y1), view.to_screen(x2, y2));
                let len = (bb - a).length();
                let px = (wpx * 0.75).min(14.0);
                if len < px * 0.6 * n.chars().count() as f32 + 2.0 * wpx {
                    continue;
                }
                let mut ang = (bb.y - a.y).atan2(bb.x - a.x);
                if ang > std::f32::consts::FRAC_PI_2 {
                    ang -= std::f32::consts::PI;
                } else if ang < -std::f32::consts::FRAC_PI_2 {
                    ang += std::f32::consts::PI;
                }
                label(
                    ui,
                    shapes,
                    a + (bb - a) / 2.0,
                    n,
                    px,
                    ang,
                    Color32::from_black_alpha(230),
                );
                budget -= 1;
            }
            (Source::Pad(_), geom) if scene.shows(p) && (p.slot == active || p.all_layers) => {
                let Some(n) = name(p.net) else { continue };
                let (cx, cy, short) = match geom {
                    Geom::Circle { x, y, r } => (x, y, 2.0 * r),
                    Geom::Rect { x, y, sx, sy, .. } => (x, y, sx.min(sy)),
                    Geom::Seg { x1, y1, x2, y2, w } => ((x1 + x2) / 2.0, (y1 + y2) / 2.0, w),
                };
                let spx = short * view.scale;
                if spx < 22.0 {
                    continue;
                }
                let px = (spx * 0.3).min(14.0);
                let c = view.to_screen(cx, cy);
                let off = if p.all_layers {
                    egui::vec2(0.0, spx * 0.32)
                } else {
                    egui::Vec2::ZERO
                };
                label(
                    ui,
                    shapes,
                    c + off + egui::vec2(1.0, 1.0),
                    n,
                    px,
                    0.0,
                    shadow,
                );
                label(ui, shapes, c + off, n, px, 0.0, text);
                budget -= 1;
            }
            _ => {}
        }
    }
}

/// The partner's pad nearest a pad of a pair, on the routing layer.
fn partner_pad(
    board: &trace_core::Board,
    at: [f64; 2],
    net: Option<u32>,
    layer: u8,
    reach: f64,
) -> Option<[f64; 2]> {
    let net = net?;
    board
        .geometry
        .pads
        .iter()
        .filter(|p| {
            p.net == Some(net)
                && match p.layer {
                    trace_core::LayerRef::Copper(l) => l == layer,
                    trace_core::LayerRef::AllCopper => true,
                    _ => false,
                }
        })
        .map(|p| [p.at.x.mil(), p.at.y.mil()])
        .map(|p| ((p[0] - at[0]).hypot(p[1] - at[1]), p))
        .filter(|(d, _)| *d < reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// When `net` belongs to a differential pair whose partner has a pad next
/// to `at`, the pair's sizes per layer and where the partner starts.
fn pair_start(
    router: &crate::router::Router,
    imp: &mut crate::imp::Imp,
    scene: &Scene,
    at: [f64; 2],
    net: u32,
    layer: u8,
) -> Option<(crate::router::PairInfo, [f64; 2])> {
    let name = &scene.board.nets.get(net as usize)?.name;
    let (pair, partner) = scene.pair_of.get(name)?;
    let partner = *scene.net_by_name.get(partner)?;
    let b = partner_pad(&scene.board, at, Some(partner), layer, 400.0)?;
    let fallback = (router.width, router.width.max(router.clearance));
    let sizes = imp
        .pair_sizes(name, scene.copper_count as usize)
        .into_iter()
        .map(|s| s.unwrap_or(fallback))
        .collect();
    Some((
        crate::router::PairInfo {
            name: pair.clone(),
            partner: Some(partner),
            sizes,
        },
        b,
    ))
}

/// Works out the meander for track `ti` that brings its net to the target.
fn tune_preview(
    router: &mut crate::router::Router,
    cm: &mut crate::cm::Cm,
    scene: &Scene,
    ti: usize,
    layer: u8,
) {
    let board = &scene.board;
    let Some(t) = board.geometry.tracks.get(ti) else {
        return;
    };
    let Some(net) = t.net else {
        router.message = Some("That track has no net.".into());
        return;
    };
    let length = board.net_length(net).mil();
    let (target, why) = match router.tune.target {
        Some(v) => (v, "your target".to_string()),
        None => match cm.tune_target(board, net, t).or_else(|| {
            // A pair net with no rule: match its partner.
            let (_, partner) = scene.pair_of.get(&board.nets[net as usize].name)?;
            let p = *scene.net_by_name.get(partner)?;
            Some((board.net_length(p).mil(), format!("partner {partner}")))
        }) {
            Some(t) => t,
            None => {
                router.message = Some(format!(
                    "No Constraint Manager group gives {} a length; tick Target and type one.",
                    board.nets[net as usize].name
                ));
                return;
            }
        },
    };
    let extra = target - length;
    if extra <= 0.5 {
        router.message = Some(format!(
            "{} is {:.1} mil, already at its target of {:.1} mil ({why}).",
            board.nets[net as usize].name, length, target
        ));
        return;
    }
    router.message = None;
    let width = t.width.mil();
    let tune = &router.tune;
    let space = if tune.space > 0.0 {
        tune.space
    } else {
        4.0 * width
    };
    let (a, b) = (
        [t.start.x.mil(), t.start.y.mil()],
        [t.end.x.mil(), t.end.y.mil()],
    );
    let (pieces, added) = trace_core::route::meander(a, b, extra, tune.amp, space, tune.flip);
    let clearance = router.clearance;
    let obstacles = router.obstacles(board);
    let mut hit = None;
    for p in &pieces {
        if let Some(h) =
            obstacles.check_track(layer, p.start(), p.end(), width, Some(net), clearance)
        {
            hit = Some(h);
            break;
        }
    }
    router.tune.preview = Some(crate::router::TunePreview {
        track: ti,
        layer,
        net,
        width,
        pieces,
        added,
        length,
        target,
        why,
        hit,
    });
}

/// The part being dragged where it will go, the tracks that stretch to
/// it, and its ratsnest.
fn draw_moving(shapes: &mut Vec<Shape>, scene: &Scene, view: &View, m: &crate::placing::Moving) {
    let board = &scene.board;
    let t = m.transform(board);
    let color_of = |slot: Slot| -> Option<Color32> {
        let l = scene.layers.iter().find(|l| l.slot == slot)?;
        l.visible.then(|| rgba(l.color, 0.95))
    };
    let slot_after = |s: Slot| match s {
        Slot::Copper(i) => Slot::Copper(t.copper(i)),
        Slot::TopSilk if t.flips() => Slot::BottomSilk,
        Slot::BottomSilk if t.flips() => Slot::TopSilk,
        other => other,
    };
    let at = |x: f32, y: f32| {
        let q = t.xy([x as f64, y as f64]);
        (q[0] as f32, q[1] as f32)
    };
    for p in scene
        .prims
        .iter()
        .filter(|p| p.component == Some(m.index as u32))
    {
        let geom = match p.geom {
            Geom::Seg { x1, y1, x2, y2, w } => {
                let ((x1, y1), (x2, y2)) = (at(x1, y1), at(x2, y2));
                Geom::Seg { x1, y1, x2, y2, w }
            }
            Geom::Circle { x, y, r } => {
                let (x, y) = at(x, y);
                Geom::Circle { x, y, r }
            }
            Geom::Rect { x, y, sx, sy, rot } => {
                let (x, y) = at(x, y);
                let rot = t.angle(rot as f64) as f32;
                Geom::Rect { x, y, sx, sy, rot }
            }
        };
        let color = if p.all_layers {
            Some(rgba([201, 180, 88], 0.95))
        } else {
            color_of(slot_after(p.slot))
        };
        if let Some(c) = color {
            draw(shapes, view, &geom, c);
        }
    }
    for (_, tr) in m.stretched(board) {
        let LayerRef::Copper(l) = tr.layer else {
            continue;
        };
        if let Some(c) = color_of(Slot::Copper(l)) {
            let g = Geom::Seg {
                x1: tr.start.x.mil() as f32,
                y1: tr.start.y.mil() as f32,
                x2: tr.end.x.mil() as f32,
                y2: tr.end.y.mil() as f32,
                w: tr.width.mil() as f32,
            };
            draw(shapes, view, &g, c);
        }
    }
    let rats = Stroke::new(1.0_f32, Color32::from_rgb(235, 235, 120));
    for (a, b) in m.ratsnest(board) {
        shapes.push(Shape::line_segment(
            [
                view.to_screen(a[0] as f32, a[1] as f32),
                view.to_screen(b[0] as f32, b[1] as f32),
            ],
            rats,
        ));
    }
}

/// What the selection holds, and what can be done with it.
pub fn select_hint(sel: &trace_core::select::Sel) -> String {
    if sel.is_empty() {
        return "Nothing selected.".into();
    }
    format!(
        "Selected {}. Drag to move, arrows nudge, Ctrl+R turns, Delete removes, Ctrl+C copies; Edit > Align lines them up.",
        sel.describe()
    )
}
