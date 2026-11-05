//! The schematic viewer: draws Altium sheets, and cross-probes nets and
//! parts with the board.

use crate::canvas::View;
use crate::scene::Scene;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Shape, Stroke};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use trace_core::{SchGeom, Sheet};

/// (designator, pad name) -> board net.
type PadNets = HashMap<(String, String), u32>;

/// Net highlight colour on the sheet.
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 120, 0);

enum Load {
    Sheet(usize, Result<Sheet, String>),
}

/// One sheet slot: the file, and the sheet once read.
pub struct Slot {
    pub path: PathBuf,
    pub name: String,
    pub sheet: Option<Sheet>,
    pub error: Option<String>,
    /// Box of each shape, for skipping what is off screen.
    boxes: Vec<[f32; 4]>,
}

/// What the person picked on a sheet, for the app to act on.
pub enum Pick {
    BoardNet(u32),
    Part(String),
}

#[derive(Default)]
pub struct Schematics {
    pub slots: Vec<Slot>,
    pub project: Option<PathBuf>,
    pub current: usize,
    view: View,
    needs_fit: bool,
    loader: Option<Receiver<Load>>,
    textures: HashMap<(usize, usize), Option<egui::TextureHandle>>,
    /// Selected sheet net (on the current sheet) and part.
    sel_net: Option<usize>,
    sel_part: Option<usize>,
    /// Board net the highlight was worked out for, and the sheet nets it covers.
    hl_for: Option<(u32, usize)>,
    highlight: Vec<HashSet<usize>>,
    /// (designator, pin) -> board net, for the open board.
    pad_nets: Option<(PathBuf, PadNets)>,
    pub hover: Option<String>,
    /// The board net picked from the sheet, so a pick made elsewhere clears
    /// the sheet selection.
    picked: Option<u32>,
    /// Area to frame on the next frame, once the view knows its size.
    focus: Option<[f32; 4]>,
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn key(d: &str, p: &str) -> (String, String) {
    (d.to_ascii_uppercase(), p.to_ascii_uppercase())
}

/// The documents an Altium project lists, in order.
fn project_docs(prj: &Path) -> Vec<PathBuf> {
    let dir = prj.parent().unwrap_or(Path::new("."));
    std::fs::read(prj)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().strip_prefix("DocumentPath="))
        .map(|p| dir.join(p.replace('\\', std::path::MAIN_SEPARATOR_STR)))
        .collect()
}

fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The project that lists this board, looked for next to it and one folder up.
pub fn find_project(board: &Path) -> Option<PathBuf> {
    let name = board.file_name()?.to_string_lossy().to_ascii_lowercase();
    let dir = board.parent()?;
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if ext(&p) == "prjpcb"
                && project_docs(&p).iter().any(|doc| {
                    doc.file_name()
                        .is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == name)
                })
            {
                return Some(p);
            }
        }
    }
    None
}

/// The boards a project lists.
pub fn project_boards(prj: &Path) -> Vec<PathBuf> {
    project_docs(prj)
        .into_iter()
        .filter(|p| ext(p) == "pcbdoc")
        .collect()
}

fn shape_box(g: &SchGeom, w: f64) -> [f32; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut add = |p: [f64; 2]| {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    };
    match g {
        SchGeom::Line(v) | SchGeom::Poly(v) => v.iter().for_each(|&p| add(p)),
        SchGeom::Rect { min, max, .. } => {
            add(*min);
            add(*max);
        }
        SchGeom::Arc { c, r, .. } => {
            add([c[0] - r[0], c[1] - r[1]]);
            add([c[0] + r[0], c[1] + r[1]]);
        }
    }
    let h = w / 2.0;
    [
        (b[0] - h) as f32,
        (b[1] - h) as f32,
        (b[2] + h) as f32,
        (b[3] + h) as f32,
    ]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    };
    ((a[0] + t * dx - p[0]).powi(2) + (a[1] + t * dy - p[1]).powi(2)).sqrt()
}

/// Ear-clipping for small polygons (port shapes, arrows, glyphs).
fn triangulate(pts: &[Pos2]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let ccw = area > 0.0;
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut cut = None;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            let cr = cross(a, b, c);
            if (cr > 0.0) != ccw || cr == 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = pts[j];
                let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
                if ccw {
                    d1 > 0.0 && d2 > 0.0 && d3 > 0.0
                } else {
                    d1 < 0.0 && d2 < 0.0 && d3 < 0.0
                }
            });
            if !inside {
                cut = Some(i);
                out.push([ia, ib, ic]);
                break;
            }
        }
        match cut {
            Some(i) => {
                idx.remove(i);
            }
            None => break,
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

fn arc_points(c: [f64; 2], r: [f64; 2], start: f64, end: f64) -> Vec<[f64; 2]> {
    let steps = (((end - start).abs() / 360.0) * 48.0).ceil().max(6.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = (start + (end - start) * i as f64 / steps as f64).to_radians();
            [c[0] + r[0] * a.cos(), c[1] + r[1] * a.sin()]
        })
        .collect()
}

impl Schematics {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn loading(&self) -> Option<(usize, usize)> {
        self.loader.as_ref()?;
        let done = self
            .slots
            .iter()
            .filter(|s| s.sheet.is_some() || s.error.is_some())
            .count();
        Some((done, self.slots.len()))
    }

    /// Opens sheets in the background, keeping their order.
    pub fn open(&mut self, files: Vec<PathBuf>, project: Option<PathBuf>, ctx: &egui::Context) {
        *self = Schematics {
            project,
            needs_fit: true,
            ..Default::default()
        };
        self.slots = files
            .iter()
            .map(|p| Slot {
                path: p.clone(),
                name: p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                sheet: None,
                error: None,
                boxes: Vec::new(),
            })
            .collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (i, f) in files.into_iter().enumerate() {
                let r = trace_altium::read_schdoc(&f).map_err(|e| e.to_string());
                if tx.send(Load::Sheet(i, r)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        self.loader = Some(rx);
    }

    /// Opens a `.SchDoc` or every sheet of a `.PrjPcb`.
    pub fn open_path(&mut self, path: &Path, ctx: &egui::Context) {
        if ext(path) == "prjpcb" {
            let sheets = project_docs(path)
                .into_iter()
                .filter(|p| ext(p) == "schdoc" && p.exists())
                .collect();
            self.open(sheets, Some(path.to_path_buf()), ctx);
        } else {
            self.open(vec![path.to_path_buf()], None, ctx);
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loader else { return };
        let mut done = false;
        loop {
            match rx.try_recv() {
                Ok(Load::Sheet(i, r)) => {
                    let s = &mut self.slots[i];
                    match r {
                        Ok(sheet) => {
                            s.boxes = sheet
                                .drawing
                                .shapes
                                .iter()
                                .map(|sh| shape_box(&sh.geom, sh.width))
                                .collect();
                            s.sheet = Some(sheet);
                        }
                        Err(e) => s.error = Some(e),
                    }
                    self.hl_for = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        if done {
            self.loader = None;
        }
    }

    pub fn select_sheet(&mut self, i: usize) {
        if i < self.slots.len() && i != self.current {
            self.current = i;
            self.sel_net = None;
            self.sel_part = None;
            self.needs_fit = true;
        }
    }

    fn pad_nets(&mut self, scene: &Scene) -> &HashMap<(String, String), u32> {
        if self.pad_nets.as_ref().map(|p| &p.0) != Some(&scene.path) {
            let b = &scene.board;
            let mut m = HashMap::new();
            for p in &b.geometry.pads {
                if let (Some(c), Some(n)) = (p.component, p.net) {
                    if let Some(comp) = b.components.get(c as usize) {
                        m.entry(key(&comp.designator, &p.name)).or_insert(n);
                    }
                }
            }
            self.pad_nets = Some((scene.path.clone(), m));
        }
        &self.pad_nets.as_ref().unwrap().1
    }

    /// The board net a sheet net is, found through its pins, else its name.
    fn board_net(&mut self, sheet: usize, net: usize, scene: &Scene) -> Option<u32> {
        let pins: Vec<(String, String)> = {
            let d = &self.slots[sheet].sheet.as_ref()?.drawing;
            d.nets[net]
                .pins
                .iter()
                .map(|&p| {
                    let pin = &d.pins[p as usize];
                    key(&d.parts[pin.part as usize].designator, &pin.number)
                })
                .collect()
        };
        let map = self.pad_nets(scene);
        if let Some(n) = pins.iter().find_map(|k| map.get(k).copied()) {
            return Some(n);
        }
        let d = &self.slots[sheet].sheet.as_ref()?.drawing;
        let name = d.nets[net].name.as_ref()?;
        scene.net_by_name.get(name).copied().or_else(|| {
            scene
                .board
                .nets
                .iter()
                .position(|n| n.name.eq_ignore_ascii_case(name))
                .map(|i| i as u32)
        })
    }

    /// Which sheet nets belong to a board net, on every sheet.
    fn update_highlight(&mut self, board_net: Option<u32>, scene: Option<&Scene>) {
        let loaded = self.slots.iter().filter(|s| s.sheet.is_some()).count();
        let want = board_net.map(|n| (n, loaded));
        if self.hl_for == want && self.highlight.len() == self.slots.len() {
            return;
        }
        self.hl_for = want;
        self.highlight = vec![HashSet::new(); self.slots.len()];
        let (Some(n), Some(scene)) = (board_net, scene) else {
            return;
        };
        let b = &scene.board;
        let pads: HashSet<(String, String)> = b
            .geometry
            .pads
            .iter()
            .filter(|p| p.net == Some(n))
            .filter_map(|p| {
                let c = b.components.get(p.component? as usize)?;
                Some(key(&c.designator, &p.name))
            })
            .collect();
        let name = b.nets.get(n as usize).map(|x| x.name.to_ascii_uppercase());
        for (si, slot) in self.slots.iter().enumerate() {
            let Some(sheet) = &slot.sheet else { continue };
            let d = &sheet.drawing;
            for (ni, net) in d.nets.iter().enumerate() {
                let by_pin = net.pins.iter().any(|&p| {
                    let pin = &d.pins[p as usize];
                    pads.contains(&key(&d.parts[pin.part as usize].designator, &pin.number))
                });
                let by_name = net.pins.is_empty()
                    && net
                        .name
                        .as_ref()
                        .is_some_and(|x| Some(x.to_ascii_uppercase()) == name);
                if by_pin || by_name {
                    self.highlight[si].insert(ni);
                }
            }
        }
    }

    /// Sheets where the board net appears.
    pub fn sheets_with(&mut self, board_net: u32, scene: &Scene) -> Vec<usize> {
        self.update_highlight(Some(board_net), Some(scene));
        (0..self.slots.len())
            .filter(|&i| !self.highlight[i].is_empty())
            .collect()
    }

    /// Goes to the first sheet with the board net and frames it.
    pub fn reveal(&mut self, board_net: u32, scene: &Scene) -> bool {
        let sheets = self.sheets_with(board_net, scene);
        let Some(&first) = sheets
            .iter()
            .find(|&&i| i == self.current)
            .or(sheets.first())
        else {
            return false;
        };
        self.select_sheet(first);
        self.needs_fit = false;
        let Some(d) = self.slots[first].sheet.as_ref().map(|s| &s.drawing) else {
            return false;
        };
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut grow = |p: [f64; 2]| {
            b = [
                b[0].min(p[0] as f32),
                b[1].min(p[1] as f32),
                b[2].max(p[0] as f32),
                b[3].max(p[1] as f32),
            ];
        };
        for &ni in &self.highlight[first] {
            let net = &d.nets[ni];
            for &w in &net.wires {
                d.wires[w as usize].iter().for_each(|&p| grow(p));
            }
            for &p in &net.pins {
                grow(d.pins[p as usize].at);
            }
        }
        if b[0] <= b[2] {
            // At least a quarter of an A4 sheet, so there is context around.
            let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            let (hw, hh) = (
                ((b[2] - b[0]) / 2.0).max(2400.0),
                ((b[3] - b[1]) / 2.0).max(1600.0),
            );
            let b = [cx - hw, cy - hh, cx + hw, cy + hh];
            let pad = 300.0;
            self.focus = Some([b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]);
        }
        true
    }

    /// Goes to the sheet with this part and frames it.
    pub fn reveal_part(&mut self, designator: &str) -> bool {
        for (si, slot) in self.slots.iter().enumerate() {
            let Some(sheet) = &slot.sheet else { continue };
            if let Some(pi) = sheet
                .drawing
                .parts
                .iter()
                .position(|p| p.designator.eq_ignore_ascii_case(designator))
            {
                let p = &sheet.drawing.parts[pi];
                let b = [
                    p.min[0] as f32 - 800.0,
                    p.min[1] as f32 - 800.0,
                    p.max[0] as f32 + 800.0,
                    p.max[1] as f32 + 800.0,
                ];
                self.select_sheet(si);
                self.needs_fit = false;
                self.focus = Some(b);
                self.sel_part = Some(pi);
                return true;
            }
        }
        false
    }

    /// The sheet list for the side panel.
    pub fn side_panel(&mut self, ui: &mut egui::Ui, board_net: Option<u32>, scene: Option<&Scene>) {
        self.poll();
        self.update_highlight(board_net, scene);
        if let Some(p) = &self.project {
            ui.label(
                RichText::new(
                    p.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
                .strong(),
            );
        }
        if let Some((done, total)) = self.loading() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Reading sheets: {done} of {total}"));
            });
        }
        ui.label(RichText::new("Sheets").weak());
        let mut go = None;
        egui::ScrollArea::vertical()
            .id_salt("sheets")
            .max_height(ui.available_height() * 0.6)
            .show(ui, |ui| {
                for (i, s) in self.slots.iter().enumerate() {
                    let mut text = RichText::new(&s.name);
                    if self.highlight.get(i).is_some_and(|h| !h.is_empty()) {
                        text = text.color(HIGHLIGHT);
                    }
                    if s.error.is_some() {
                        text = text.strikethrough();
                    }
                    let r = ui.selectable_label(i == self.current, text);
                    let r = match &s.error {
                        Some(e) => r.on_hover_text(e),
                        None => r,
                    };
                    if r.clicked() {
                        go = Some(i);
                    }
                }
            });
        if let Some(i) = go {
            self.select_sheet(i);
        }
        if board_net.is_some() {
            ui.label(
                RichText::new("Sheets in orange have the selected net.")
                    .weak()
                    .small(),
            );
        }
        ui.separator();
        if let Some(sheet) = self.slots.get(self.current).and_then(|s| s.sheet.as_ref()) {
            if let Some(pi) = self.sel_part {
                if let Some(p) = sheet.drawing.parts.get(pi) {
                    let sym = &sheet.symbols[p.symbol];
                    ui.label(RichText::new(&p.designator).strong().size(16.0));
                    if let Some(v) = &sym.value {
                        ui.label(v);
                    }
                    ui.label(RichText::new(&sym.library_ref).weak());
                    if !sym.description.is_empty() {
                        ui.label(RichText::new(&sym.description).weak().small());
                    }
                }
            } else if let Some(ni) = self.sel_net {
                if let Some(n) = sheet.drawing.nets.get(ni) {
                    ui.label(
                        RichText::new(n.name.clone().unwrap_or_else(|| "Unnamed net".into()))
                            .strong()
                            .size(16.0),
                    );
                    ui.label(format!("{} pins on this sheet", n.pins.len()));
                }
            } else {
                ui.label(
                    RichText::new(format!(
                        "{} parts, {} nets",
                        sheet.drawing.parts.len(),
                        sheet.drawing.nets.len()
                    ))
                    .weak(),
                );
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        board_net: Option<u32>,
        scene: Option<&Scene>,
    ) -> Option<Pick> {
        self.poll();
        self.update_highlight(board_net, scene);
        if self.sel_net.is_some() && board_net != self.picked {
            self.sel_net = None;
        }
        let (rect, resp) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        self.view.rect = rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(40, 42, 48));
        let cur = self.current;
        let Some(sheet) = self.slots.get(cur).and_then(|s| s.sheet.as_ref()) else {
            let msg = match self.slots.get(cur).and_then(|s| s.error.clone()) {
                Some(e) => format!("Could not read this sheet: {e}"),
                None => "Reading the sheet…".into(),
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                FontId::proportional(16.0),
                Color32::GRAY,
            );
            return None;
        };
        let size = sheet.drawing.size;
        if self.needs_fit {
            self.view
                .fit_rect([0.0, 0.0, size[0] as f32, size[1] as f32], 0.96);
            self.needs_fit = false;
        }
        if let Some(b) = self.focus.take() {
            self.view.fit_rect(b, 0.9);
        }
        // Zoom about the cursor, pan by dragging.
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
        if resp.dragged() {
            let d = resp.drag_delta();
            self.view.tx += d.x;
            self.view.ty += d.y;
        }
        if ui.input(|i| i.key_pressed(egui::Key::F)) && !ui.ctx().wants_keyboard_input() {
            self.needs_fit = true;
        }
        let view = self.view;
        let vis = view.visible();
        let ctx = ui.ctx().clone();
        let paper = Rect::from_two_pos(
            view.to_screen(0.0, 0.0),
            view.to_screen(size[0] as f32, size[1] as f32),
        );
        painter.rect_filled(paper, 0.0, rgb(sheet.drawing.paper));
        // Pictures next, under the drawing.
        for (ii, im) in sheet.drawing.images.iter().enumerate() {
            let tex = self.textures.entry((cur, ii)).or_insert_with(|| {
                let img = image::load_from_memory(&im.data).ok()?.to_rgba8();
                let (w, h) = img.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    img.as_raw(),
                );
                Some(ctx.load_texture(
                    format!("sheet-{cur}-{ii}"),
                    ci,
                    egui::TextureOptions::LINEAR,
                ))
            });
            let a = view.to_screen(im.min[0] as f32, im.max[1] as f32);
            let b = view.to_screen(im.max[0] as f32, im.min[1] as f32);
            let mut r = Rect::from_two_pos(a, b);
            if let Some(t) = tex {
                if im.keep_aspect {
                    let [tw, th] = t.size().map(|v| v as f32);
                    let k = (r.width() / tw).min(r.height() / th);
                    r = Rect::from_center_size(r.center(), egui::vec2(tw * k, th * k));
                }
                painter.image(
                    t.id(),
                    r,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        let d = &sheet.drawing;
        let slot = &self.slots[cur];
        let mut shapes: Vec<Shape> = Vec::new();
        let hl = self.highlight.get(cur).cloned().unwrap_or_default();
        let sel_net = self.sel_net;
        // Highlighted nets: a wide glow under their wires and pins.
        let net_of_wire: HashMap<u32, usize> = d
            .nets
            .iter()
            .enumerate()
            .flat_map(|(ni, n)| n.wires.iter().map(move |&w| (w, ni)))
            .collect();
        let lit = |ni: usize| hl.contains(&ni) || sel_net == Some(ni);
        let glow = Stroke::new((45.0 * view.scale).max(6.0), HIGHLIGHT.gamma_multiply(0.55));
        for (ni, n) in d.nets.iter().enumerate() {
            if !lit(ni) {
                continue;
            }
            for &w in &n.wires {
                let pts: Vec<Pos2> = d.wires[w as usize]
                    .iter()
                    .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                    .collect();
                shapes.push(Shape::line(pts, glow));
            }
            for &p in &n.pins {
                let pin = &d.pins[p as usize];
                shapes.push(Shape::line_segment(
                    [
                        view.to_screen(pin.at[0] as f32, pin.at[1] as f32),
                        view.to_screen(pin.base[0] as f32, pin.base[1] as f32),
                    ],
                    glow,
                ));
            }
        }
        if let Some(p) = self.sel_part.and_then(|i| d.parts.get(i)) {
            let r = Rect::from_two_pos(
                view.to_screen(p.min[0] as f32 - 30.0, p.min[1] as f32 - 30.0),
                view.to_screen(p.max[0] as f32 + 30.0, p.max[1] as f32 + 30.0),
            );
            shapes.push(Shape::rect_filled(r, 2.0, HIGHLIGHT.gamma_multiply(0.25)));
        }
        let px = |w: f64| ((w as f32) * view.scale).max(1.0);
        for (si, sh) in d.shapes.iter().enumerate() {
            if !overlaps(slot.boxes[si], vis) {
                continue;
            }
            let mut stroke = sh
                .stroke
                .map(|c| Stroke::new(px(sh.width), rgb(c)))
                .unwrap_or(Stroke::NONE);
            if let Some(w) = sh.wire {
                if net_of_wire.get(&w).is_some_and(|&ni| lit(ni)) {
                    stroke.color = HIGHLIGHT;
                }
            }
            let fill = sh.fill.map(rgb);
            let to = |p: &[f64; 2]| view.to_screen(p[0] as f32, p[1] as f32);
            match &sh.geom {
                SchGeom::Line(v) => {
                    let pts: Vec<Pos2> = v.iter().map(to).collect();
                    if sh.dashed {
                        shapes.extend(Shape::dashed_line(
                            &pts,
                            stroke,
                            (40.0 * view.scale).max(3.0),
                            (25.0 * view.scale).max(2.0),
                        ));
                    } else {
                        shapes.push(Shape::line(pts, stroke));
                    }
                }
                SchGeom::Poly(v) => {
                    let pts: Vec<Pos2> = v.iter().map(to).collect();
                    if let Some(f) = fill {
                        let mut mesh = egui::epaint::Mesh::default();
                        for &p in &pts {
                            mesh.colored_vertex(p, f);
                        }
                        for t in triangulate(&pts) {
                            mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
                        }
                        shapes.push(Shape::mesh(mesh));
                    }
                    if sh.dashed {
                        let mut closed = pts.clone();
                        closed.push(pts[0]);
                        shapes.extend(Shape::dashed_line(
                            &closed,
                            stroke,
                            (40.0 * view.scale).max(3.0),
                            (25.0 * view.scale).max(2.0),
                        ));
                    } else if stroke != Stroke::NONE {
                        shapes.push(Shape::closed_line(pts, stroke));
                    }
                }
                SchGeom::Rect { min, max, r } => {
                    let rr = Rect::from_two_pos(to(min), to(max));
                    let round = (r[0].max(r[1]) as f32 * view.scale).min(rr.height() / 2.0);
                    if let Some(f) = fill {
                        shapes.push(Shape::rect_filled(rr, round, f));
                    }
                    if stroke != Stroke::NONE {
                        shapes.push(Shape::rect_stroke(rr, round, stroke));
                    }
                }
                SchGeom::Arc { c, r, start, end } => {
                    let full = end - start >= 359.99;
                    let pts: Vec<Pos2> = arc_points(*c, *r, *start, *end).iter().map(to).collect();
                    if let Some(f) = fill {
                        let mut fan = pts.clone();
                        if !full {
                            fan.insert(0, to(c));
                        }
                        let mut mesh = egui::epaint::Mesh::default();
                        let center = to(c);
                        mesh.colored_vertex(center, f);
                        for &p in &fan {
                            mesh.colored_vertex(p, f);
                        }
                        for i in 1..fan.len() as u32 {
                            mesh.add_triangle(0, i, i + 1);
                        }
                        mesh.indices.truncate(mesh.indices.len().saturating_sub(3));
                        shapes.push(Shape::mesh(mesh));
                    }
                    if stroke != Stroke::NONE {
                        if full {
                            shapes.push(Shape::closed_line(pts, stroke));
                        } else {
                            shapes.push(Shape::line(pts, stroke));
                        }
                    }
                }
            }
        }
        // Text, skipped when too small to read.
        for t in &d.texts {
            let size = t.size as f32 * view.scale;
            if size < 3.5 {
                continue;
            }
            let reach = (t.size * (t.text.len() as f64 + 2.0)) as f32;
            let (ax, ay) = (t.at[0] as f32, t.at[1] as f32);
            if !overlaps([ax - reach, ay - reach, ax + reach, ay + reach], vis) {
                continue;
            }
            let font = FontId::proportional(size * 0.92);
            let galley = match t.wrap {
                Some(w) => {
                    painter.layout(t.text.clone(), font, rgb(t.color), w as f32 * view.scale)
                }
                None => painter.layout_no_wrap(t.text.clone(), font, rgb(t.color)),
            };
            let (w, h) = (galley.size().x, galley.size().y);
            let fx = [0.0, 0.5, 1.0][(t.just % 3) as usize];
            let fy = [1.0, 0.5, 0.0][(t.just / 3).min(2) as usize];
            let (ox, oy) = (-w * fx, -h * fy);
            let angle = -(t.rot as f32) * std::f32::consts::FRAC_PI_2;
            let (s, c) = angle.sin_cos();
            let anchor = view.to_screen(ax, ay);
            let pos = anchor + egui::vec2(ox * c - oy * s, ox * s + oy * c);
            shapes.push(Shape::Text(
                egui::epaint::TextShape::new(pos, galley, rgb(t.color)).with_angle(angle),
            ));
        }
        painter.extend(shapes);

        // Hover and pick.
        let pointer = resp.hover_pos().map(|p| view.to_board(p));
        self.hover = None;
        let mut pick = None;
        if let Some(p) = pointer {
            let p = [p[0] as f64, p[1] as f64];
            let tol = (5.0 / view.scale as f64).max(8.0);
            let hit_pin = d
                .pins
                .iter()
                .position(|pin| seg_dist(p, pin.at, pin.base) < tol);
            let hit_wire = d
                .wires
                .iter()
                .position(|w| w.windows(2).any(|s| seg_dist(p, s[0], s[1]) < tol));
            let hit_tag = d.tags.iter().position(|t| {
                t.at.iter()
                    .any(|&a| ((a[0] - p[0]).powi(2) + (a[1] - p[1]).powi(2)).sqrt() < tol * 2.0)
            });
            let net_of = |kind: u8, i: u32| {
                d.nets.iter().position(|n| match kind {
                    0 => n.pins.contains(&i),
                    1 => n.wires.contains(&i),
                    _ => n.tags.contains(&i),
                })
            };
            let hit_net = hit_pin
                .and_then(|i| net_of(0, i as u32))
                .or_else(|| hit_wire.and_then(|i| net_of(1, i as u32)))
                .or_else(|| hit_tag.and_then(|i| net_of(2, i as u32)));
            let hit_part = d
                .parts
                .iter()
                .enumerate()
                .filter(|(_, q)| {
                    p[0] >= q.min[0] && p[0] <= q.max[0] && p[1] >= q.min[1] && p[1] <= q.max[1]
                })
                .min_by(|a, b| {
                    let area =
                        |q: &trace_core::SchPart| (q.max[0] - q.min[0]) * (q.max[1] - q.min[1]);
                    area(a.1).total_cmp(&area(b.1))
                })
                .map(|(i, _)| i);
            let hit_link = d.links.iter().position(|l| {
                p[0] >= l.min[0] && p[0] <= l.max[0] && p[1] >= l.min[1] && p[1] <= l.max[1]
            });
            let net_name = |ni: usize| -> String {
                d.nets[ni]
                    .name
                    .clone()
                    .unwrap_or_else(|| "unnamed net".into())
            };
            self.hover = if let Some(i) = hit_pin {
                let pin = &d.pins[i];
                Some(format!(
                    "Pin {}.{} {}{}",
                    d.parts[pin.part as usize].designator,
                    pin.number,
                    pin.name.replace('\\', ""),
                    hit_net
                        .map(|n| format!(" · {}", net_name(n)))
                        .unwrap_or_default()
                ))
            } else if let Some(n) = hit_net {
                Some(format!("Net {}", net_name(n)))
            } else if let Some(i) = hit_link {
                Some(format!("Sheet {} (double-click to open)", d.links[i].file))
            } else {
                hit_part.map(|i| {
                    let q = &d.parts[i];
                    let s = &sheet.symbols[q.symbol];
                    format!(
                        "{} {}",
                        q.designator,
                        s.value.clone().unwrap_or_else(|| s.library_ref.clone())
                    )
                })
            };
            if resp.double_clicked() {
                if let Some(i) = hit_link {
                    let want = d.links[i].file.to_ascii_lowercase();
                    let target = self.slots.iter().position(|s| {
                        s.path
                            .file_name()
                            .is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == want)
                    });
                    if let Some(t) = target {
                        self.select_sheet(t);
                        return None;
                    }
                }
            } else if resp.clicked() {
                if let Some(n) = hit_net {
                    self.sel_net = Some(n);
                    self.sel_part = None;
                    if let Some(scene) = scene {
                        if let Some(bn) = self.board_net(cur, n, scene) {
                            self.picked = Some(bn);
                            pick = Some(Pick::BoardNet(bn));
                        }
                    }
                } else if let Some(i) = hit_part {
                    self.sel_part = Some(i);
                    self.sel_net = None;
                    let des = self.slots[cur].sheet.as_ref().unwrap().drawing.parts[i]
                        .designator
                        .clone();
                    pick = Some(Pick::Part(des));
                } else {
                    self.sel_net = None;
                    self.sel_part = None;
                }
            }
        }
        pick
    }
}
