//! Icon toolbars in the manner of Altium Designer: a standard bar under the
//! menu, the floating Active Bar over the board and the layer tabs under it.
//! The icons are drawn here as vector shapes.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Open,
    Save,
    Undo,
    Redo,
    Fit,
    ZoomIn,
    ZoomOut,
    Flip,
    Board,
    View3d,
    Schematic,
    Footprint,
    Constraints,
    Impedance,
    Drc,
    Select,
    Measure,
    Route,
    Pair,
    Tune,
    Via,
    Unroute,
}

const RED: Color32 = Color32::from_rgb(236, 84, 84);
const GOLD: Color32 = Color32::from_rgb(222, 184, 74);
const BLUE: Color32 = Color32::from_rgb(84, 156, 236);
const GREEN: Color32 = Color32::from_rgb(96, 200, 120);
const ON_BG: Color32 = Color32::from_rgb(38, 79, 120);
const ON_EDGE: Color32 = Color32::from_rgb(86, 150, 214);

/// One icon button. `on` shows it pressed; a disabled button still shows
/// its tooltip.
pub fn button(ui: &mut egui::Ui, icon: Icon, on: bool, enabled: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::splat(28.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let p = ui.painter_at(rect.expand(1.0));
    if on {
        p.rect(rect, 4.0, ON_BG, Stroke::new(1.0f32, ON_EDGE));
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let fg = if !enabled {
        Color32::from_gray(90)
    } else if on {
        Color32::WHITE
    } else {
        Color32::from_gray(214)
    };
    draw(&p, icon, rect.shrink(6.0), fg, enabled);
    if enabled {
        resp.on_hover_text(tip)
    } else {
        resp.on_hover_text(format!("{tip}\n(not available now)"))
    }
}

/// A thin vertical divider between groups of buttons.
pub fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 22.0), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0f32, Color32::from_gray(70)),
    );
}

fn draw(p: &egui::Painter, icon: Icon, r: Rect, fg: Color32, enabled: bool) {
    // Points in a 0..1 box.
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * r.width(), r.top() + y * r.height());
    let s = |w: f32, c: Color32| Stroke::new(w, c);
    let tint = |c: Color32| if enabled { c } else { fg };
    let line = |pts: &[(f32, f32)], st: Stroke| {
        p.add(Shape::line(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            st,
        ));
    };
    let poly = |pts: &[(f32, f32)], fill: Color32, st: Stroke| {
        p.add(Shape::convex_polygon(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            fill,
            st,
        ));
    };
    let arc = |cx: f32, cy: f32, rad: f32, a0: f32, a1: f32, st: Stroke| {
        let n = 16;
        let pts = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                at(cx + rad * a.cos(), cy - rad * a.sin())
            })
            .collect();
        p.add(Shape::line(pts, st));
    };
    let w = r.width();
    match icon {
        Icon::Open => {
            poly(
                &[
                    (0.0, 0.15),
                    (0.38, 0.15),
                    (0.48, 0.28),
                    (0.95, 0.28),
                    (0.95, 0.88),
                    (0.0, 0.88),
                ],
                tint(Color32::from_rgb(150, 120, 50)),
                s(1.0, fg),
            );
            poly(
                &[(0.12, 0.42), (1.0, 0.42), (0.88, 0.88), (0.0, 0.88)],
                tint(GOLD),
                s(1.0, fg),
            );
        }
        Icon::Save => {
            poly(
                &[
                    (0.05, 0.05),
                    (0.8, 0.05),
                    (0.95, 0.2),
                    (0.95, 0.95),
                    (0.05, 0.95),
                ],
                tint(BLUE),
                s(1.0, fg),
            );
            p.rect_filled(Rect::from_min_max(at(0.22, 0.05), at(0.72, 0.36)), 0.0, fg);
            p.rect_filled(
                Rect::from_min_max(at(0.2, 0.56), at(0.8, 0.95)),
                1.0,
                Color32::from_gray(235),
            );
        }
        Icon::Undo | Icon::Redo => {
            let m = |x: f32| if icon == Icon::Redo { 1.0 - x } else { x };
            let st = s(2.0, fg);
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = (180.0 - 180.0 * i as f32 / 16.0f32).to_radians();
                    at(m(0.55 - 0.32 * a.cos()), 0.62 - 0.36 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, st));
            poly(
                &[(m(0.0), 0.5), (m(0.46), 0.5), (m(0.23), 0.85)],
                fg,
                Stroke::NONE,
            );
        }
        Icon::Fit => {
            let st = s(2.0, fg);
            for (x, y, dx, dy) in [
                (0.0, 0.0, 1.0, 1.0),
                (1.0, 0.0, -1.0, 1.0),
                (0.0, 1.0, 1.0, -1.0),
                (1.0, 1.0, -1.0, -1.0),
            ] {
                line(&[(x + dx * 0.32, y), (x, y), (x, y + dy * 0.32)], st);
            }
            p.rect_stroke(
                Rect::from_min_max(at(0.32, 0.32), at(0.68, 0.68)),
                1.0,
                s(1.5, tint(GREEN)),
            );
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(at(0.42, 0.42), w * 0.33, s(2.0, fg));
            line(&[(0.66, 0.66), (0.98, 0.98)], s(3.0, fg));
            line(&[(0.26, 0.42), (0.58, 0.42)], s(2.0, fg));
            if icon == Icon::ZoomIn {
                line(&[(0.42, 0.26), (0.42, 0.58)], s(2.0, fg));
            }
        }
        Icon::Flip => {
            line(&[(0.5, 0.0), (0.5, 1.0)], s(1.0, Color32::from_gray(120)));
            poly(
                &[(0.42, 0.15), (0.42, 0.85), (0.0, 0.85)],
                tint(RED),
                Stroke::NONE,
            );
            poly(
                &[(0.58, 0.15), (0.58, 0.85), (1.0, 0.85)],
                Color32::TRANSPARENT,
                s(1.5, tint(BLUE)),
            );
        }
        Icon::Board => {
            p.rect(
                Rect::from_min_max(at(0.0, 0.1), at(1.0, 0.9)),
                2.0,
                tint(Color32::from_rgb(30, 70, 40)),
                s(1.0, fg),
            );
            line(
                &[(0.15, 0.7), (0.45, 0.7), (0.65, 0.35), (0.88, 0.35)],
                s(2.0, tint(RED)),
            );
            p.circle_filled(at(0.15, 0.7), w * 0.08, tint(GOLD));
            p.circle_filled(at(0.88, 0.35), w * 0.08, tint(GOLD));
        }
        Icon::View3d => {
            let st = s(1.5, fg);
            poly(
                &[(0.5, 0.0), (1.0, 0.25), (0.5, 0.5), (0.0, 0.25)],
                tint(GREEN),
                st,
            );
            poly(
                &[(0.0, 0.25), (0.5, 0.5), (0.5, 1.0), (0.0, 0.75)],
                tint(Color32::from_rgb(50, 120, 70)),
                st,
            );
            poly(
                &[(0.5, 0.5), (1.0, 0.25), (1.0, 0.75), (0.5, 1.0)],
                tint(Color32::from_rgb(70, 150, 90)),
                st,
            );
        }
        Icon::Schematic => {
            let st = s(1.5, fg);
            line(
                &[
                    (0.0, 0.5),
                    (0.2, 0.5),
                    (0.27, 0.3),
                    (0.4, 0.7),
                    (0.53, 0.3),
                    (0.66, 0.7),
                    (0.73, 0.5),
                    (1.0, 0.5),
                ],
                s(1.5, tint(BLUE)),
            );
            line(&[(0.0, 0.15), (0.0, 0.85)], st);
            line(&[(1.0, 0.15), (1.0, 0.85)], st);
        }
        Icon::Footprint => {
            // A part outline with a row of pads on each side.
            p.rect_stroke(
                Rect::from_min_max(at(0.22, 0.1), at(0.78, 0.9)),
                1.0,
                s(1.2, fg),
            );
            for k in 0..3 {
                let y = 0.22 + 0.28 * k as f32;
                p.rect_filled(
                    Rect::from_min_max(at(0.0, y - 0.07), at(0.16, y + 0.07)),
                    0.0,
                    tint(RED),
                );
                p.rect_filled(
                    Rect::from_min_max(at(0.84, y - 0.07), at(1.0, y + 0.07)),
                    0.0,
                    tint(RED),
                );
            }
        }
        Icon::Constraints => {
            let st = s(1.0, fg);
            p.rect_stroke(Rect::from_min_max(at(0.0, 0.05), at(1.0, 0.95)), 1.0, st);
            p.rect_filled(
                Rect::from_min_max(at(0.0, 0.05), at(1.0, 0.3)),
                1.0,
                tint(BLUE),
            );
            for y in [0.55, 0.75] {
                line(&[(0.0, y), (1.0, y)], st);
            }
            line(&[(0.38, 0.05), (0.38, 0.95)], st);
        }
        Icon::Impedance => {
            line(
                &[(0.05, 0.1), (0.62, 0.1), (0.05, 0.9), (0.62, 0.9)],
                s(2.0, fg),
            );
            line(
                &[(0.7, 0.55), (0.78, 0.35), (0.86, 0.75), (0.94, 0.55)],
                s(1.5, tint(GOLD)),
            );
        }
        Icon::Drc => {
            // A board with a check mark: design rule check.
            let st = s(1.0, fg);
            p.rect_stroke(Rect::from_min_max(at(0.0, 0.1), at(0.8, 0.9)), 1.0, st);
            line(&[(0.15, 0.3), (0.45, 0.3)], st);
            line(&[(0.15, 0.5), (0.35, 0.5)], st);
            line(
                &[(0.42, 0.62), (0.62, 0.85), (1.0, 0.3)],
                s(2.0, tint(GREEN)),
            );
        }
        Icon::Select => {
            poly(
                &[
                    (0.15, 0.0),
                    (0.15, 0.85),
                    (0.36, 0.66),
                    (0.5, 1.0),
                    (0.64, 0.94),
                    (0.5, 0.6),
                    (0.8, 0.6),
                ],
                Color32::TRANSPARENT,
                s(1.5, fg),
            );
            // The outline above is not convex; fill its convex body.
            poly(
                &[(0.15, 0.0), (0.8, 0.6), (0.36, 0.66), (0.15, 0.85)],
                fg,
                Stroke::NONE,
            );
            poly(
                &[(0.36, 0.62), (0.52, 0.6), (0.64, 0.94), (0.5, 1.0)],
                fg,
                Stroke::NONE,
            );
        }
        Icon::Measure => {
            poly(
                &[(0.0, 0.7), (0.7, 0.0), (1.0, 0.3), (0.3, 1.0)],
                tint(GOLD),
                s(1.0, fg),
            );
            for i in 1..6 {
                let t = i as f32 / 6.0;
                let (x, y) = (0.0 + 0.7 * t, 0.7 - 0.7 * t);
                let l = if i % 2 == 0 { 0.2 } else { 0.12 };
                line(&[(x, y), (x + l, y + l)], s(1.0, Color32::from_gray(40)));
            }
        }
        Icon::Route => {
            let st = Stroke::new(w * 0.16, tint(RED));
            line(&[(0.1, 0.88), (0.42, 0.88), (0.88, 0.42), (0.88, 0.1)], st);
            p.circle_filled(at(0.1, 0.88), w * 0.12, tint(GOLD));
            p.circle_filled(at(0.88, 0.1), w * 0.12, tint(GOLD));
        }
        Icon::Pair => {
            let st = Stroke::new(w * 0.11, tint(RED));
            line(&[(0.0, 0.72), (0.34, 0.72), (0.72, 0.34), (0.72, 0.0)], st);
            line(&[(0.0, 1.0), (0.46, 1.0), (1.0, 0.46), (1.0, 0.0)], st);
        }
        Icon::Tune => {
            let st = Stroke::new(w * 0.11, tint(GOLD));
            line(
                &[
                    (0.0, 0.7),
                    (0.15, 0.7),
                    (0.15, 0.25),
                    (0.35, 0.25),
                    (0.35, 0.7),
                    (0.55, 0.7),
                    (0.55, 0.25),
                    (0.75, 0.25),
                    (0.75, 0.7),
                    (1.0, 0.7),
                ],
                st,
            );
        }
        Icon::Via => {
            p.circle_filled(at(0.5, 0.5), w * 0.46, tint(GOLD));
            p.circle_filled(at(0.5, 0.5), w * 0.2, Color32::from_gray(25));
        }
        Icon::Unroute => {
            line(&[(0.0, 0.5), (1.0, 0.5)], Stroke::new(w * 0.14, tint(RED)));
            line(&[(0.2, 0.15), (0.8, 0.85)], s(2.5, fg));
            line(&[(0.8, 0.15), (0.2, 0.85)], s(2.5, fg));
        }
    }
    let _ = arc;
}

/// One layer tab: its colour, its name, raised when it is the current layer
/// and greyed when hidden. Returns the response for clicks.
pub fn layer_tab(
    ui: &mut egui::Ui,
    name: &str,
    color: [u8; 3],
    current: bool,
    visible: bool,
) -> egui::Response {
    let c = Color32::from_rgb(color[0], color[1], color[2]);
    let font = egui::FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), font, Color32::WHITE);
    let size = Vec2::new(galley.size().x + 28.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let bg = if current {
        c.linear_multiply(0.45)
    } else if resp.hovered() {
        Color32::from_gray(52)
    } else {
        Color32::from_gray(34)
    };
    p.rect(
        rect,
        egui::Rounding {
            nw: 0.0,
            ne: 0.0,
            sw: 4.0,
            se: 4.0,
        },
        bg,
        Stroke::new(1.0f32, if current { c } else { Color32::from_gray(58) }),
    );
    if current {
        p.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), 2.5)),
            0.0,
            c,
        );
    }
    let sw = Rect::from_center_size(
        Pos2::new(rect.left() + 11.0, rect.center().y),
        Vec2::splat(10.0),
    );
    if visible {
        p.rect_filled(sw, 2.0, c);
    } else {
        p.rect_stroke(sw, 2.0, Stroke::new(1.0f32, c));
    }
    let text = if !visible {
        Color32::from_gray(110)
    } else if current {
        Color32::WHITE
    } else {
        Color32::from_gray(200)
    };
    p.galley_with_override_text_color(
        Pos2::new(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    resp
}

/// The "LS" button that turns single layer mode on and off.
pub fn text_toggle(ui: &mut egui::Ui, text: &str, on: bool, tip: &str) -> egui::Response {
    let font = egui::FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 14.0, 22.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(rect, 4.0, ON_BG, Stroke::new(1.0f32, ON_EDGE));
    } else {
        p.rect(
            rect,
            4.0,
            if resp.hovered() {
                Color32::from_gray(52)
            } else {
                Color32::from_gray(34)
            },
            Stroke::new(1.0f32, Color32::from_gray(58)),
        );
    }
    p.galley_with_override_text_color(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        if on {
            Color32::WHITE
        } else {
            Color32::from_gray(200)
        },
    );
    resp.on_hover_text(tip)
}
