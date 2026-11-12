//! Schematic symbol libraries: the window that lists the symbols of the
//! open `.SchLib` files, with a preview, for placing one on a sheet.

use crate::canvas::View;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke};
use std::path::{Path, PathBuf};
use trace_core::footprint::Footprint;
use trace_core::{Drawing, LibSymbol, SchGeom, SymbolLib};

/// Where the list of opened libraries is kept between runs.
pub const STORE_KEY: &str = "symbol_libraries";

#[derive(Default)]
pub struct Libraries {
    pub open: bool,
    pub libs: Vec<(PathBuf, SymbolLib)>,
    /// Footprints of the open integrated libraries, by library.
    footprints: Vec<(PathBuf, Vec<Footprint>)>,
    /// Libraries to read the first time the window opens.
    pending: Vec<PathBuf>,
    filter: String,
    /// The picked symbol: (library, symbol).
    pick: Option<(usize, usize)>,
    note: Option<String>,
}

impl Libraries {
    pub fn new(stored: Option<String>) -> Self {
        Libraries {
            pending: stored
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(PathBuf::from)
                .collect(),
            ..Default::default()
        }
    }

    pub fn to_store(&self) -> String {
        self.libs
            .iter()
            .map(|(p, _)| p.display().to_string())
            .chain(self.pending.iter().map(|p| p.display().to_string()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Reads a library (`.SchLib`, or `.IntLib` with its footprints), or
    /// reads it again if it is open already.
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let int = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("intlib"));
        let lib = if int {
            let il = trace_altium::read_intlib(path).map_err(|e| e.to_string())?;
            self.footprints.retain(|(p, _)| p != path);
            if !il.footprints.footprints.is_empty() {
                self.footprints
                    .push((path.to_path_buf(), il.footprints.footprints));
            }
            il.symbols
        } else {
            trace_altium::read_schlib(path).map_err(|e| e.to_string())?
        };
        match self.libs.iter().position(|(p, _)| p == path) {
            Some(i) => self.libs[i].1 = lib,
            None => self.libs.push((path.to_path_buf(), lib)),
        }
        Ok(())
    }

    /// Reads the libraries an Altium project lists, quietly.
    pub fn load_quietly(&mut self, paths: &[PathBuf]) {
        for p in paths {
            if p.exists() && !self.libs.iter().any(|(q, _)| q == p) {
                let _ = self.load(p);
            }
        }
    }

    /// Whether an open integrated library brought footprints.
    pub fn has_footprints(&self) -> bool {
        !self.footprints.is_empty()
    }

    /// The footprints of the open integrated libraries.
    pub fn footprints(&self) -> impl Iterator<Item = &Footprint> {
        self.footprints.iter().flat_map(|(_, f)| f)
    }

    pub fn symbol(&self, at: (usize, usize)) -> Option<&LibSymbol> {
        self.libs.get(at.0)?.1.symbols.get(at.1)
    }

    /// The window; returns the symbol to place, when Place is pressed.
    pub fn window(&mut self, ctx: &egui::Context) -> Option<(usize, usize)> {
        if !self.open {
            return None;
        }
        if !self.pending.is_empty() {
            for p in std::mem::take(&mut self.pending) {
                if p.exists() {
                    let _ = self.load(&p);
                }
            }
        }
        let mut open = self.open;
        let mut place = None;
        egui::Window::new("Place symbol")
            .open(&mut open)
            .default_size([560.0, 420.0])
            .resizable(true)
            .show(ctx, |ui| place = self.contents(ui));
        self.open = open && place.is_none();
        place
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<(usize, usize)> {
        let mut place = None;
        ui.horizontal(|ui| {
            if ui
                .button("Open library…")
                .on_hover_text(
                    "Add an Altium schematic library (.SchLib) or integrated library (.IntLib)",
                )
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter(
                        "Symbol libraries",
                        &["SchLib", "schlib", "IntLib", "intlib"],
                    )
                    .pick_files()
                {
                    for p in paths {
                        if let Err(e) = self.load(&p) {
                            self.note = Some(format!("Could not read {}: {e}", p.display()));
                        }
                    }
                }
            }
            ui.label("Find");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(160.0));
        });
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).weak().small());
        }
        if self.libs.is_empty() {
            ui.label(
                RichText::new(
                    "No libraries yet. Open a .SchLib or .IntLib; the libraries of an Altium project open by themselves.",
                )
                .weak(),
            );
            return None;
        }
        let want = self.filter.trim().to_ascii_lowercase();
        let mut close_lib = None;
        ui.columns(2, |cols| {
            egui::ScrollArea::vertical()
                .id_salt("symbols")
                .auto_shrink([false, false])
                .show(&mut cols[0], |ui| {
                    for (li, (path, lib)) in self.libs.iter().enumerate() {
                        let head = egui::CollapsingHeader::new(
                            RichText::new(format!("{} ({})", lib.name, lib.symbols.len())).strong(),
                        )
                        .id_salt(path)
                        .default_open(true);
                        let r = head.show(ui, |ui| {
                            for (si, s) in lib.symbols.iter().enumerate() {
                                let title = s.title();
                                if !want.is_empty()
                                    && !title.to_ascii_lowercase().contains(&want)
                                    && !s.symbol.description.to_ascii_lowercase().contains(&want)
                                {
                                    continue;
                                }
                                let r = ui
                                    .selectable_label(self.pick == Some((li, si)), title)
                                    .on_hover_text(&s.symbol.description);
                                if r.clicked() {
                                    self.pick = Some((li, si));
                                }
                                if r.double_clicked() {
                                    place = Some((li, si));
                                }
                            }
                        });
                        r.header_response.context_menu(|ui| {
                            if ui.button("Close library").clicked() {
                                close_lib = Some(li);
                                ui.close_menu();
                            }
                        });
                    }
                });
            let ui = &mut cols[1];
            match self.pick.and_then(|p| self.symbol(p)) {
                Some(s) => {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 220.0),
                        Sense::hover(),
                    );
                    paint_preview(ui.painter_at(rect), rect, &s.drawing);
                    ui.label(RichText::new(s.title()).strong());
                    if !s.symbol.description.is_empty() {
                        ui.label(RichText::new(&s.symbol.description).weak().small());
                    }
                    ui.label(format!(
                        "{} pins · footprint {}",
                        s.drawing.pins.len(),
                        if s.symbol.footprint.is_empty() {
                            "none"
                        } else {
                            &s.symbol.footprint
                        }
                    ));
                    if ui
                        .button("Place")
                        .on_hover_text("The symbol follows the pointer: click to drop it, Space turns it, Esc cancels")
                        .clicked()
                    {
                        place = self.pick;
                    }
                }
                None => {
                    ui.label(RichText::new("Pick a symbol to see it.").weak());
                }
            }
        });
        if let Some(li) = close_lib {
            let (path, _) = self.libs.remove(li);
            self.footprints.retain(|(p, _)| p != &path);
            self.pick = None;
        }
        place
    }
}

/// Draws a symbol's graphics fitted into `rect`.
pub fn paint_preview(painter: egui::Painter, rect: Rect, d: &Drawing) {
    painter.rect_filled(rect, 4.0, Color32::from_rgb(255, 252, 240));
    let Some(q) = d.parts.first() else { return };
    let mut b = [q.min[0], q.min[1], q.max[0], q.max[1]];
    for t in &d.texts {
        b = [
            b[0].min(t.at[0]),
            b[1].min(t.at[1]),
            b[2].max(t.at[0]),
            b[3].max(t.at[1]),
        ];
    }
    let mut view = View {
        rect,
        ..Default::default()
    };
    view.fit_rect(
        [
            b[0] as f32 - 60.0,
            b[1] as f32 - 60.0,
            b[2] as f32 + 60.0,
            b[3] as f32 + 60.0,
        ],
        0.9,
    );
    let to = |p: &[f64; 2]| view.to_screen(p[0] as f32, p[1] as f32);
    let px = |w: f64| ((w as f32) * view.scale).max(1.0);
    let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    let mut shapes = Vec::new();
    for sh in &d.shapes {
        let stroke = sh
            .stroke
            .map(|c| Stroke::new(px(sh.width), rgb(c)))
            .unwrap_or(Stroke::NONE);
        let fill = sh.fill.map(rgb);
        match &sh.geom {
            SchGeom::Line(v) => shapes.push(Shape::line(v.iter().map(to).collect(), stroke)),
            SchGeom::Poly(v) => {
                let pts: Vec<Pos2> = v.iter().map(to).collect();
                if let Some(f) = fill {
                    let mut mesh = egui::epaint::Mesh::default();
                    for &p in &pts {
                        mesh.colored_vertex(p, f);
                    }
                    for t in crate::schview::triangulate(&pts) {
                        mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
                    }
                    shapes.push(Shape::mesh(mesh));
                }
                shapes.push(Shape::closed_line(pts, stroke));
            }
            SchGeom::Rect { min, max, .. } => {
                let r = Rect::from_two_pos(to(min), to(max));
                if let Some(f) = fill {
                    shapes.push(Shape::rect_filled(r, 0.0, f));
                }
                shapes.push(Shape::rect_stroke(r, 0.0, stroke));
            }
            SchGeom::Arc { c, r, start, end } => {
                let n = 32;
                let pts: Vec<Pos2> = (0..=n)
                    .map(|i| {
                        let a = (start + (end - start) * i as f64 / n as f64).to_radians();
                        to(&[c[0] + r[0] * a.cos(), c[1] + r[1] * a.sin()])
                    })
                    .collect();
                shapes.push(Shape::line(pts, stroke));
            }
        }
    }
    for t in &d.texts {
        let size = t.size as f32 * view.scale;
        if size < 3.0 {
            continue;
        }
        let galley = painter.layout_no_wrap(
            t.text.clone(),
            FontId::proportional(size * 0.92),
            rgb(t.color),
        );
        let (w, h) = (galley.size().x, galley.size().y);
        let fx = [0.0, 0.5, 1.0][(t.just % 3) as usize];
        let fy = [1.0, 0.5, 0.0][(t.just / 3).min(2) as usize];
        let (ox, oy) = (-w * fx, -h * fy);
        let angle = -(t.rot as f32) * std::f32::consts::FRAC_PI_2;
        let (s, c) = angle.sin_cos();
        let pos = to(&t.at) + egui::vec2(ox * c - oy * s, ox * s + oy * c);
        shapes.push(Shape::Text(
            egui::epaint::TextShape::new(pos, galley, rgb(t.color)).with_angle(angle),
        ));
    }
    painter.extend(shapes);
}
