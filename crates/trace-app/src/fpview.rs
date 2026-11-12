//! The footprint editor: one library of footprints (an Altium `.PcbLib`, a
//! Trace `.tracelib`, or the footprints of the open board), one footprint
//! drawn at a time in its own frame, with tools to place and change pads,
//! draw lines, and carry the result back to the board.

use crate::schview::triangulate;
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Shape, Stroke, Vec2};
use std::path::{Path, PathBuf};
use trace_core::footprint::{Footprint, FpText, Library};
use trace_core::{Board, LayerRef, Length, Pad, PadShape, Point, Track};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pad,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    Pad(usize),
    Track(usize),
    Arc(usize),
    Region(usize),
    Text(usize),
}

/// What the editor asks of the app.
pub enum Pick {
    /// Give the board's parts using footprint `.0` (as named on the board)
    /// the footprint `.1`.
    Update(String, Footprint),
    /// Put a new part with this footprint on the board, under the cursor.
    Place(Footprint),
    Message(String, bool),
}

const GRIDS: [(&str, f64); 10] = [
    ("1 mil", 1.0),
    ("5 mil", 5.0),
    ("10 mil", 10.0),
    ("25 mil", 25.0),
    ("50 mil", 50.0),
    ("0.01 mm", 0.01 / 0.0254),
    ("0.05 mm", 0.05 / 0.0254),
    ("0.1 mm", 0.1 / 0.0254),
    ("0.25 mm", 0.25 / 0.0254),
    ("0.5 mm", 0.5 / 0.0254),
];

/// Layers lines can be drawn on.
const LINE_LAYERS: [(&str, LayerRef); 7] = [
    ("Top silk", LayerRef::TopSilk),
    ("Bottom silk", LayerRef::BottomSilk),
    ("Top copper", LayerRef::Copper(0)),
    ("Mechanical 1", LayerRef::Mechanical(1)),
    ("Mechanical 2", LayerRef::Mechanical(2)),
    ("Mechanical 4", LayerRef::Mechanical(4)),
    ("Mechanical 13", LayerRef::Mechanical(13)),
];

pub struct FpEditor {
    pub lib: Library,
    /// Name of each footprint when it was read: on the board, parts use it.
    orig: Vec<String>,
    /// Where Save writes; `None` asks.
    path: Option<PathBuf>,
    /// What the library is, for the panel.
    pub source: String,
    pub from_board: bool,
    current: usize,
    filter: String,
    scale: f32,
    center: [f64; 2],
    needs_fit: bool,
    tool: Tool,
    sel: Option<Sel>,
    drag: Option<[f64; 2]>,
    line_pts: Vec<[f64; 2]>,
    line_layer: usize,
    line_width: Length,
    grid: usize,
    mm: bool,
    undo: Vec<(usize, Footprint)>,
    redo: Vec<(usize, Footprint)>,
    last_field: Option<egui::Id>,
    pub dirty: bool,
    note: Option<(String, bool)>,
    array: (usize, f64, f64),
    /// The footprint and pad the row step was last guessed for.
    array_for: Option<(usize, usize)>,
    hover: Option<[f64; 2]>,
}

impl Default for FpEditor {
    fn default() -> Self {
        FpEditor {
            lib: Library::default(),
            orig: Vec::new(),
            path: None,
            source: String::new(),
            from_board: false,
            current: 0,
            filter: String::new(),
            scale: 2.0,
            center: [0.0, 0.0],
            needs_fit: true,
            tool: Tool::Select,
            sel: None,
            drag: None,
            line_pts: Vec::new(),
            line_layer: 0,
            line_width: Length::from_mm(0.15),
            grid: 7,
            mm: true,
            undo: Vec::new(),
            redo: Vec::new(),
            last_field: None,
            dirty: false,
            note: None,
            array: (4, 0.0, -50.0),
            array_for: None,
            hover: None,
        }
    }
}

fn mil(p: Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
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

fn inside(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut c = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            c = !c;
        }
    }
    c
}

fn shift(p: &mut Point, d: [f64; 2]) {
    p.x = Length(p.x.0 + Length::from_mil(d[0]).0);
    p.y = Length(p.y.0 + Length::from_mil(d[1]).0);
}

/// The corner radius of a rounded-rectangle pad, from its outline.
fn corner_radius(p: &Pad) -> Option<f64> {
    if p.outline.is_empty() || p.shape == PadShape::Octagon {
        return None;
    }
    let c = mil(p.at);
    let (s, co) = (-p.rotation_deg).to_radians().sin_cos();
    let local: Vec<[f64; 2]> = p
        .outline
        .iter()
        .map(|&q| {
            let (dx, dy) = (q.x.mil() - c[0], q.y.mil() - c[1]);
            [dx * co - dy * s, dx * s + dy * co]
        })
        .collect();
    let top = local.iter().map(|q| q[1]).fold(f64::MIN, f64::max);
    let flat = local
        .iter()
        .filter(|q| (q[1] - top).abs() < 0.01)
        .map(|q| q[0])
        .fold(f64::MIN, f64::max);
    let r = p.size_x.mil() / 2.0 - flat;
    (r.is_finite() && r >= 0.0).then_some(r)
}

/// Draws the pad outline again after its size, turn or place changed.
fn reshape(p: &mut Pad, radius: Option<f64>) {
    if let Some(r) = radius {
        p.outline = trace_core::geometry::round_rect(
            p.at,
            p.size_x,
            p.size_y,
            p.rotation_deg,
            Length::from_mil(r),
        );
    }
}

/// The copper shape of a pad, in mil.
fn pad_ring(p: &Pad) -> Vec<[f64; 2]> {
    if !p.outline.is_empty() {
        return p.outline.iter().map(|&q| mil(q)).collect();
    }
    let c = mil(p.at);
    let (hx, hy) = (p.size_x.mil() / 2.0, p.size_y.mil() / 2.0);
    let local: Vec<[f64; 2]> = match p.shape {
        PadShape::Rect => vec![[hx, hy], [-hx, hy], [-hx, -hy], [hx, -hy]],
        PadShape::Octagon => {
            let k = hx.min(hy) * 0.5;
            vec![
                [hx, hy - k],
                [hx - k, hy],
                [-(hx - k), hy],
                [-hx, hy - k],
                [-hx, -(hy - k)],
                [-(hx - k), -hy],
                [hx - k, -hy],
                [hx, -(hy - k)],
            ]
        }
        PadShape::Round => {
            // A stadium: half circles on the short sides.
            let r = hx.min(hy);
            let (ax, ay) = (hx - r, hy - r);
            let mut v = Vec::new();
            for (cx, cy, a0) in [
                (ax, ay, 0.0),
                (-ax, ay, 90.0),
                (-ax, -ay, 180.0),
                (ax, -ay, 270.0),
            ] {
                for k in 0..=6 {
                    let a: f64 = (a0 + 15.0 * k as f64).to_radians();
                    v.push([cx + r * a.cos(), cy + r * a.sin()]);
                }
            }
            v
        }
    };
    let (s, co) = p.rotation_deg.to_radians().sin_cos();
    local
        .iter()
        .map(|q| [c[0] + q[0] * co - q[1] * s, c[1] + q[0] * s + q[1] * co])
        .collect()
}

fn layer_color(l: &LayerRef) -> Color32 {
    match l {
        LayerRef::Copper(0) => Color32::from_rgb(214, 70, 70),
        LayerRef::Copper(_) => Color32::from_rgb(70, 120, 214),
        LayerRef::AllCopper => Color32::from_rgb(214, 176, 70),
        LayerRef::TopSilk => Color32::from_rgb(236, 236, 236),
        LayerRef::BottomSilk => Color32::from_rgb(190, 190, 120),
        LayerRef::Mechanical(n) => match n % 6 {
            0 => Color32::from_rgb(200, 120, 220),
            1 => Color32::from_rgb(220, 160, 90),
            2 => Color32::from_rgb(170, 110, 230),
            3 => Color32::from_rgb(90, 200, 200),
            4 => Color32::from_rgb(110, 200, 110),
            _ => Color32::from_rgb(200, 200, 90),
        },
        LayerRef::Other(37 | 38) => Color32::from_rgba_unmultiplied(160, 80, 160, 90),
        LayerRef::Other(35 | 36) => Color32::from_rgba_unmultiplied(150, 150, 150, 90),
        _ => Color32::from_gray(150),
    }
}

fn layer_name(l: &LayerRef) -> String {
    match l {
        LayerRef::Copper(0) => "Top copper".into(),
        LayerRef::Copper(_) => "Bottom copper".into(),
        LayerRef::AllCopper => "All copper (through)".into(),
        LayerRef::TopSilk => "Top silk".into(),
        LayerRef::BottomSilk => "Bottom silk".into(),
        LayerRef::Mechanical(n) => format!("Mechanical {n}"),
        LayerRef::Keepout => "Keep-out".into(),
        LayerRef::Other(35) => "Top paste".into(),
        LayerRef::Other(36) => "Bottom paste".into(),
        LayerRef::Other(37) => "Top solder mask".into(),
        LayerRef::Other(38) => "Bottom solder mask".into(),
        LayerRef::Other(n) => format!("Layer {n}"),
    }
}

/// Pad names counted up: the next free number after the highest.
fn next_name(fp: &Footprint) -> String {
    let n = fp
        .pads
        .iter()
        .filter_map(|p| p.name.trim().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    (n + 1).to_string()
}

/// The step that carries a row on past pad `i`: away from its nearest pad
/// of the same size and shape, in mils.
fn row_step(pads: &[Pad], i: usize) -> Option<[f64; 2]> {
    let base = pads.get(i)?;
    let at = mil(base.at);
    pads.iter()
        .enumerate()
        .filter(|(j, q)| {
            *j != i
                && q.size_x == base.size_x
                && q.size_y == base.size_y
                && q.shape == base.shape
                && q.at != base.at
        })
        .map(|(_, q)| {
            let b = mil(q.at);
            [at[0] - b[0], at[1] - b[1]]
        })
        .min_by(|a, b| a[0].hypot(a[1]).total_cmp(&b[0].hypot(b[1])))
}

fn bump(name: &str, by: u32) -> String {
    // The trailing number goes up: A1 -> A2, 7 -> 8.
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return format!("{name}{by}");
    }
    let (head, num) = name.split_at(name.len() - digits);
    format!("{head}{}", num.parse::<u32>().unwrap_or(0) + by)
}

fn default_pad() -> Pad {
    Pad {
        layer: LayerRef::Copper(0),
        net: None,
        component: None,
        name: "1".into(),
        at: Point::default(),
        size_x: Length::from_mm(0.6),
        size_y: Length::from_mm(0.9),
        hole: Length(0),
        shape: PadShape::Rect,
        rotation_deg: 0.0,
        outline: Vec::new(),
        stack: Vec::new(),
        fab: Default::default(),
    }
}

impl FpEditor {
    pub fn is_empty(&self) -> bool {
        self.lib.footprints.is_empty()
    }

    fn set_lib(&mut self, lib: Library, source: String, path: Option<PathBuf>, from_board: bool) {
        *self = FpEditor {
            orig: lib.footprints.iter().map(|f| f.name.clone()).collect(),
            lib,
            source,
            path,
            from_board,
            mm: self.mm,
            grid: self.grid,
            ..Default::default()
        };
    }

    /// Opens a `.PcbLib` or `.tracelib`.
    pub fn open_file(&mut self, path: &Path) -> Result<(), String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lib = if ext == "tracelib" {
            Library::load(path).map_err(|e| e.to_string())?
        } else {
            trace_altium::read_pcblib(path).map_err(|e| e.to_string())?
        };
        if lib.footprints.is_empty() {
            return Err(format!("{name} holds no footprints."));
        }
        let save = (ext == "tracelib").then(|| path.to_path_buf());
        self.set_lib(lib, name, save, false);
        Ok(())
    }

    /// The footprints of the open board, one per name, each from its first
    /// part.
    pub fn open_board(&mut self, board: &Board, title: &str) {
        let mut seen = std::collections::HashSet::new();
        let mut lib = Library::default();
        for (i, c) in board.components.iter().enumerate() {
            if c.footprint.is_empty() || !seen.insert(c.footprint.to_ascii_uppercase()) {
                continue;
            }
            if let Some(fp) = Footprint::from_part(board, i) {
                lib.footprints.push(fp);
            }
        }
        lib.footprints.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        self.set_lib(lib, format!("Footprints on {title}"), None, true);
    }

    fn select(&mut self, i: usize) {
        self.current = i;
        self.sel = None;
        self.line_pts.clear();
        self.needs_fit = true;
    }

    fn fp(&self) -> Option<&Footprint> {
        self.lib.footprints.get(self.current)
    }

    /// One undoable change to the current footprint.
    fn change(&mut self, f: impl FnOnce(&mut Footprint)) {
        let cur = self.current;
        let Some(fp) = self.lib.footprints.get_mut(cur) else {
            return;
        };
        self.undo.push((cur, fp.clone()));
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
        f(fp);
        self.dirty = true;
        self.last_field = None;
    }

    fn step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some((i, fp)) = from.pop() else { return };
        if let Some(now) = self.lib.footprints.get_mut(i) {
            to.push((i, std::mem::replace(now, fp)));
        }
        self.current = i;
        self.sel = None;
        self.dirty = true;
        self.last_field = None;
    }

    fn save_to(&mut self, path: &Path) -> Result<(), String> {
        self.lib.version = env!("CARGO_PKG_VERSION").to_string();
        let altium = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pcblib"));
        if altium {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bytes = trace_export::to_pcblib(&self.lib, &name).map_err(|e| e.to_string())?;
            std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        } else {
            self.lib.save(path).map_err(|e| e.to_string())?;
        }
        self.path = Some(path.to_path_buf());
        self.source = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.from_board = false;
        self.dirty = false;
        Ok(())
    }

    /// Saves as an Altium `.PcbLib`, asking where.
    fn save_altium(&mut self) -> Option<Pick> {
        let stem = self
            .source
            .trim_start_matches("Footprints on ")
            .split('.')
            .next()
            .unwrap_or("footprints")
            .to_string();
        let path = rfd::FileDialog::new()
            .add_filter("Altium footprint library", &["PcbLib"])
            .set_file_name(format!("{stem}.PcbLib"))
            .save_file()?;
        Some(match self.save_to(&path) {
            Ok(()) => Pick::Message(format!("Saved {}", path.display()), false),
            Err(e) => Pick::Message(format!("Could not save {}: {e}", path.display()), true),
        })
    }

    fn save(&mut self, ask: bool) -> Option<Pick> {
        let path = match (&self.path, ask) {
            (Some(p), false) => p.clone(),
            _ => {
                let stem = self
                    .source
                    .trim_start_matches("Footprints on ")
                    .split('.')
                    .next()
                    .unwrap_or("footprints")
                    .to_string();
                rfd::FileDialog::new()
                    .add_filter("Trace footprint library", &["tracelib"])
                    .set_file_name(format!("{stem}.tracelib"))
                    .save_file()?
            }
        };
        Some(match self.save_to(&path) {
            Ok(()) => Pick::Message(format!("Saved {}", path.display()), false),
            Err(e) => Pick::Message(format!("Could not save {}: {e}", path.display()), true),
        })
    }

    /// Moves what is selected by `d` mil.
    fn move_sel(fp: &mut Footprint, sel: Sel, d: [f64; 2]) {
        match sel {
            Sel::Pad(i) => {
                if let Some(p) = fp.pads.get_mut(i) {
                    shift(&mut p.at, d);
                    for q in p.outline.iter_mut() {
                        shift(q, d);
                    }
                    for l in &mut p.stack {
                        for q in l.outline.iter_mut() {
                            shift(q, d);
                        }
                    }
                }
            }
            Sel::Track(i) => {
                if let Some(t) = fp.tracks.get_mut(i) {
                    shift(&mut t.start, d);
                    shift(&mut t.end, d);
                }
            }
            Sel::Arc(i) => {
                if let Some(a) = fp.arcs.get_mut(i) {
                    shift(&mut a.center, d);
                }
            }
            Sel::Region(i) => {
                if let Some(r) = fp.regions.get_mut(i) {
                    for q in r.outline.iter_mut().chain(r.holes.iter_mut().flatten()) {
                        shift(q, d);
                    }
                }
            }
            Sel::Text(i) => {
                if let Some(t) = fp.texts.get_mut(i) {
                    shift(&mut t.at, d);
                }
            }
        }
    }

    fn delete_sel(fp: &mut Footprint, sel: Sel) {
        match sel {
            Sel::Pad(i) if i < fp.pads.len() => {
                fp.pads.remove(i);
            }
            Sel::Track(i) if i < fp.tracks.len() => {
                fp.tracks.remove(i);
            }
            Sel::Arc(i) if i < fp.arcs.len() => {
                fp.arcs.remove(i);
            }
            Sel::Region(i) if i < fp.regions.len() => {
                fp.regions.remove(i);
            }
            Sel::Text(i) if i < fp.texts.len() => {
                fp.texts.remove(i);
            }
            _ => {}
        }
    }

    /// What is under `p` (mil), pads first; `tol` in mil.
    fn hit(fp: &Footprint, p: [f64; 2], tol: f64) -> Option<Sel> {
        if let Some(i) = fp.pads.iter().rposition(|q| inside(p, &pad_ring(q))) {
            return Some(Sel::Pad(i));
        }
        if let Some(i) = fp.texts.iter().position(|t| {
            fp_strokes(t)
                .iter()
                .any(|s| seg_dist(p, mil(s.start), mil(s.end)) < s.width.mil() / 2.0 + tol)
        }) {
            return Some(Sel::Text(i));
        }
        if let Some(i) = fp
            .tracks
            .iter()
            .position(|t| seg_dist(p, mil(t.start), mil(t.end)) < t.width.mil() / 2.0 + tol)
        {
            return Some(Sel::Track(i));
        }
        if let Some(i) = fp.arcs.iter().position(|a| {
            let c = mil(a.center);
            let d = (p[0] - c[0]).hypot(p[1] - c[1]);
            if (d - a.radius.mil()).abs() > a.width.mil() / 2.0 + tol {
                return false;
            }
            let ang = (p[1] - c[1])
                .atan2(p[0] - c[0])
                .to_degrees()
                .rem_euclid(360.0);
            let span = (a.end_deg - a.start_deg).rem_euclid(360.0);
            let span = if span == 0.0 { 360.0 } else { span };
            (ang - a.start_deg).rem_euclid(360.0) <= span
        }) {
            return Some(Sel::Arc(i));
        }
        fp.regions
            .iter()
            .rposition(|r| inside(p, &r.outline.iter().map(|&q| mil(q)).collect::<Vec<_>>()))
            .map(Sel::Region)
    }

    fn fit(&mut self, rect: egui::Rect) {
        let Some(fp) = self.fp() else { return };
        let b = fp.bounds();
        let (w, h) = ((b[2] - b[0]).max(20.0), (b[3] - b[1]).max(20.0));
        self.scale = ((rect.width() as f64 * 0.8 / w).min(rect.height() as f64 * 0.8 / h)) as f32;
        self.center = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
    }

    fn fmt(&self, l: f64) -> String {
        if self.mm {
            format!("{:.3} mm", l * 0.0254)
        } else {
            format!("{l:.2} mil")
        }
    }

    /// A length field in the units shown. True when it changed.
    fn len_field(&mut self, ui: &mut egui::Ui, label: &str, v: &mut Length) -> bool {
        let mut x = if self.mm { v.mm() } else { v.mil() };
        let r = ui
            .horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new(label));
                ui.add(
                    egui::DragValue::new(&mut x)
                        .speed(if self.mm { 0.01 } else { 0.5 })
                        .fixed_decimals(if self.mm { 3 } else { 2 })
                        .suffix(if self.mm { " mm" } else { " mil" }),
                )
            })
            .inner;
        if r.changed() {
            *v = if self.mm {
                Length::from_mm(x)
            } else {
                Length::from_mil(x)
            };
            self.note_field(r.id);
            true
        } else {
            false
        }
    }

    /// Changes to one field in a row make one undo step.
    fn note_field(&mut self, id: egui::Id) {
        if self.last_field != Some(id) {
            if let Some(fp) = self.lib.footprints.get(self.current) {
                self.undo.push((self.current, fp.clone()));
                self.redo.clear();
            }
            self.last_field = Some(id);
        }
        self.dirty = true;
    }

    pub fn side_panel(&mut self, ui: &mut egui::Ui, board: Option<&Board>) -> Option<Pick> {
        let mut pick = None;
        ui.label(
            RichText::new(if self.source.is_empty() {
                "Footprints"
            } else {
                &self.source
            })
            .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open library…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Footprint libraries", &["PcbLib", "pcblib", "tracelib"])
                    .pick_file()
                {
                    pick = Some(match self.open_file(&p) {
                        Ok(()) => Pick::Message(format!("Opened {}", p.display()), false),
                        Err(e) => {
                            Pick::Message(format!("Could not open {}: {e}", p.display()), true)
                        }
                    });
                }
            }
            if ui
                .add_enabled(!self.is_empty(), egui::Button::new("Save"))
                .on_hover_text("Save the library as a Trace footprint library (.tracelib). Ctrl+S")
                .clicked()
            {
                pick = self.save(false);
            }
            if ui
                .add_enabled(!self.is_empty(), egui::Button::new("Save as…"))
                .clicked()
            {
                pick = self.save(true);
            }
            if ui
                .add_enabled(!self.is_empty(), egui::Button::new("Save as PcbLib…"))
                .on_hover_text("Write an Altium footprint library (.PcbLib): pads, lines, arcs, text and regions. 3D bodies are not kept, so save under a new name rather than over a library made in Altium.")
                .clicked()
            {
                pick = self.save_altium();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("New footprint").clicked() {
                let mut fp = Footprint {
                    name: format!("NEW_{}", self.lib.footprints.len() + 1),
                    ..Default::default()
                };
                fp.texts.push(FpText {
                    layer: LayerRef::TopSilk,
                    at: Point::mil(-20.0, 40.0),
                    height: Length::from_mm(1.0),
                    width: Length::from_mm(0.15),
                    rotation_deg: 0.0,
                    mirror: false,
                    text: ".Designator".into(),
                });
                self.lib.footprints.push(fp);
                self.orig.push(String::new());
                let i = self.lib.footprints.len() - 1;
                self.select(i);
                self.dirty = true;
            }
            if ui
                .add_enabled(self.fp().is_some(), egui::Button::new("Copy"))
                .on_hover_text("A copy of this footprint under a new name")
                .clicked()
            {
                if let Some(mut fp) = self.fp().cloned() {
                    fp.name = format!("{}_COPY", fp.name);
                    self.lib.footprints.insert(self.current + 1, fp);
                    self.orig.insert(self.current + 1, String::new());
                    self.select(self.current + 1);
                    self.dirty = true;
                }
            }
            if ui
                .add_enabled(self.fp().is_some(), egui::Button::new("Remove"))
                .on_hover_text("Take this footprint out of the library")
                .clicked()
            {
                let i = self.current;
                self.lib.footprints.remove(i);
                self.orig.remove(i);
                self.undo.clear();
                self.redo.clear();
                self.select(i.min(self.lib.footprints.len().saturating_sub(1)));
                self.dirty = true;
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Find a footprint")
                .desired_width(f32::INFINITY),
        );
        let f = self.filter.to_ascii_lowercase();
        let shown: Vec<usize> = (0..self.lib.footprints.len())
            .filter(|&i| {
                f.is_empty()
                    || self.lib.footprints[i]
                        .name
                        .to_ascii_lowercase()
                        .contains(&f)
            })
            .collect();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("fp_list")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show_rows(ui, 18.0, shown.len(), |ui, rows| {
                for &i in &shown[rows] {
                    let fp = &self.lib.footprints[i];
                    let users = board
                        .filter(|_| self.from_board)
                        .map(|b| trace_core::footprint::parts_using(b, &self.orig[i]).len());
                    let text = match users {
                        Some(n) if n > 0 => format!("{}  ({n})", fp.name),
                        _ => fp.name.clone(),
                    };
                    if ui.selectable_label(i == self.current, text).clicked() {
                        picked = Some(i);
                    }
                }
            });
        if let Some(i) = picked {
            self.select(i);
        }
        ui.separator();
        if self.fp().is_none() {
            ui.label(
                RichText::new("Open a .PcbLib or .tracelib, or a board to edit its footprints.")
                    .weak(),
            );
            return pick;
        }

        // The footprint itself.
        let cur = self.current;
        let mut name = self.lib.footprints[cur].name.clone();
        let r = ui
            .horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Name"));
                ui.text_edit_singleline(&mut name)
            })
            .inner;
        if r.changed() {
            self.note_field(r.id);
            self.lib.footprints[cur].name = name;
        }
        let mut desc = self.lib.footprints[cur].description.clone();
        let r = ui
            .horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Description"));
                ui.text_edit_singleline(&mut desc)
            })
            .inner;
        if r.changed() {
            self.note_field(r.id);
            self.lib.footprints[cur].description = desc;
        }
        {
            let fp = &self.lib.footprints[cur];
            let b = fp.bounds();
            ui.label(
                RichText::new(format!(
                    "{} pads · {} × {}",
                    fp.pads.len(),
                    self.fmt(b[2] - b[0]),
                    self.fmt(b[3] - b[1])
                ))
                .weak(),
            );
        }

        // Tools.
        ui.horizontal(|ui| {
            for (t, label, tip) in [
                (
                    Tool::Select,
                    "Select",
                    "Click to select, drag to move (Esc)",
                ),
                (Tool::Pad, "Pad", "Click to place pads, numbered on (P)"),
                (
                    Tool::Line,
                    "Line",
                    "Click the corners of a line; Esc or double-click ends (L)",
                ),
            ] {
                if ui
                    .selectable_label(self.tool == t, label)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.tool = t;
                    self.line_pts.clear();
                }
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("⟲"))
                .on_hover_text("Undo (Ctrl+Z)")
                .clicked()
            {
                self.step(true);
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("⟳"))
                .on_hover_text("Redo (Ctrl+Y)")
                .clicked()
            {
                self.step(false);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Grid");
            egui::ComboBox::from_id_salt("fp_grid")
                .selected_text(GRIDS[self.grid].0)
                .show_ui(ui, |ui| {
                    for (i, (n, _)) in GRIDS.iter().enumerate() {
                        ui.selectable_value(&mut self.grid, i, *n);
                    }
                });
            ui.selectable_value(&mut self.mm, true, "mm");
            ui.selectable_value(&mut self.mm, false, "mil");
        });
        if self.tool == Tool::Line {
            ui.horizontal(|ui| {
                ui.label("Layer");
                egui::ComboBox::from_id_salt("fp_line_layer")
                    .selected_text(LINE_LAYERS[self.line_layer].0)
                    .show_ui(ui, |ui| {
                        for (i, (n, _)) in LINE_LAYERS.iter().enumerate() {
                            ui.selectable_value(&mut self.line_layer, i, *n);
                        }
                    });
            });
            let mut w = self.line_width;
            if self.len_field(ui, "Width", &mut w) {
                self.line_width = w;
                // A tool setting, not a change to the footprint.
                if let Some((_, _)) = self.undo.last() {
                    self.undo.pop();
                }
                self.last_field = None;
            }
        }
        ui.separator();

        // What is selected.
        match self.sel {
            Some(Sel::Pad(i)) if i < self.lib.footprints[cur].pads.len() => self.pad_panel(ui, i),
            Some(Sel::Track(i)) if i < self.lib.footprints[cur].tracks.len() => {
                let mut t = self.lib.footprints[cur].tracks[i].clone();
                ui.label(RichText::new(format!("Line on {}", layer_name(&t.layer))).strong());
                let mut ch = self.len_field(ui, "Width", &mut t.width);
                ch |= self.len_field(ui, "Start x", &mut t.start.x);
                ch |= self.len_field(ui, "Start y", &mut t.start.y);
                ch |= self.len_field(ui, "End x", &mut t.end.x);
                ch |= self.len_field(ui, "End y", &mut t.end.y);
                if ch {
                    self.lib.footprints[cur].tracks[i] = t;
                }
            }
            Some(Sel::Arc(i)) if i < self.lib.footprints[cur].arcs.len() => {
                let mut a = self.lib.footprints[cur].arcs[i].clone();
                ui.label(RichText::new(format!("Arc on {}", layer_name(&a.layer))).strong());
                let mut ch = self.len_field(ui, "Width", &mut a.width);
                ch |= self.len_field(ui, "Radius", &mut a.radius);
                ch |= self.len_field(ui, "Centre x", &mut a.center.x);
                ch |= self.len_field(ui, "Centre y", &mut a.center.y);
                if ch {
                    self.lib.footprints[cur].arcs[i] = a;
                }
            }
            Some(Sel::Text(i)) if i < self.lib.footprints[cur].texts.len() => {
                let mut t = self.lib.footprints[cur].texts[i].clone();
                ui.label(RichText::new(format!("Text on {}", layer_name(&t.layer))).strong());
                let r = ui
                    .horizontal(|ui| {
                        ui.add_sized([86.0, 18.0], egui::Label::new("Text"));
                        ui.text_edit_singleline(&mut t.text)
                    })
                    .inner;
                let mut ch = r.changed();
                if ch {
                    self.note_field(r.id);
                }
                ch |= self.len_field(ui, "Height", &mut t.height);
                ch |= self.len_field(ui, "Stroke", &mut t.width);
                if ch {
                    self.lib.footprints[cur].texts[i] = t;
                }
                ui.label(
                    RichText::new(".Designator and .Comment become the part's own text.")
                        .weak()
                        .small(),
                );
            }
            Some(Sel::Region(i)) if i < self.lib.footprints[cur].regions.len() => {
                let r = &self.lib.footprints[cur].regions[i];
                ui.label(
                    RichText::new(format!("Filled shape on {}", layer_name(&r.layer))).strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "{} corners. Drag to move, Delete removes.",
                        r.outline.len()
                    ))
                    .weak(),
                );
            }
            _ => {
                ui.label(
                    RichText::new(
                        "Click a pad or line to change it. P places pads, L draws lines, Delete removes, arrows nudge by the grid.",
                    )
                    .weak(),
                );
            }
        }

        // Back to the board.
        if let Some(b) = board {
            ui.separator();
            let key = if self.from_board {
                self.orig.get(cur).cloned().unwrap_or_default()
            } else {
                self.lib.footprints[cur].name.clone()
            };
            if ui
                .button("Place on board")
                .on_hover_text("A new part with this footprint follows the cursor on the board: click to drop it, Space turns, L flips, Esc takes it away")
                .clicked()
            {
                pick = Some(Pick::Place(self.lib.footprints[cur].clone()));
            }
            let users = trace_core::footprint::parts_using(b, &key);
            if users.is_empty() {
                ui.label(RichText::new("No part on the board uses this footprint.").weak());
            } else {
                let names: Vec<&str> = users
                    .iter()
                    .take(6)
                    .filter_map(|&i| b.components.get(i).map(|c| c.designator.as_str()))
                    .collect();
                let more = if users.len() > 6 { " …" } else { "" };
                ui.label(format!(
                    "Used by {} on the board: {}{more}",
                    users.len(),
                    names.join(", ")
                ));
                if ui
                    .button(format!("Update {} part{} on the board", users.len(), if users.len() == 1 { "" } else { "s" }))
                    .on_hover_text("Each part gets this footprint where it sits; pads keep their nets by number. Ctrl+Z on the board undoes it.")
                    .clicked()
                {
                    pick = Some(Pick::Update(key, self.lib.footprints[cur].clone()));
                    if let Some(o) = self.orig.get_mut(cur) {
                        *o = self.lib.footprints[cur].name.clone();
                    }
                }
            }
        }
        if let Some((text, err)) = &self.note {
            ui.separator();
            if *err {
                ui.colored_label(Color32::from_rgb(240, 96, 96), text);
            } else {
                ui.label(text);
            }
        }
        pick
    }

    fn pad_panel(&mut self, ui: &mut egui::Ui, i: usize) {
        let cur = self.current;
        let mut p = self.lib.footprints[cur].pads[i].clone();
        let radius = corner_radius(&p);
        ui.label(RichText::new(format!("Pad {} on {}", p.name, layer_name(&p.layer))).strong());
        let mut ch = false;
        let r = ui
            .horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Number"));
                ui.text_edit_singleline(&mut p.name)
            })
            .inner;
        if r.changed() {
            self.note_field(r.id);
            ch = true;
        }
        let before = p.at;
        ch |= self.len_field(ui, "x", &mut p.at.x);
        ch |= self.len_field(ui, "y", &mut p.at.y);
        if p.at != before {
            let d = [p.at.x.mil() - before.x.mil(), p.at.y.mil() - before.y.mil()];
            for q in p.outline.iter_mut() {
                shift(q, d);
            }
        }
        let mut sized = self.len_field(ui, "Width", &mut p.size_x);
        sized |= self.len_field(ui, "Height", &mut p.size_y);
        let r = ui
            .horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new("Turn"));
                ui.add(
                    egui::DragValue::new(&mut p.rotation_deg)
                        .speed(1.0)
                        .suffix("°"),
                )
            })
            .inner;
        if r.changed() {
            self.note_field(r.id);
            sized = true;
        }
        // Shape, with rounded rectangles as their own choice.
        let mut kind = match (p.shape, radius) {
            (PadShape::Octagon, _) => 3,
            (_, Some(_)) => 2,
            (PadShape::Rect, None) => 1,
            (PadShape::Round, None) => 0,
        };
        let names = ["Round", "Rectangle", "Rounded rectangle", "Octagon"];
        let was = kind;
        ui.horizontal(|ui| {
            ui.add_sized([86.0, 18.0], egui::Label::new("Shape"));
            egui::ComboBox::from_id_salt("fp_pad_shape")
                .selected_text(names[kind])
                .show_ui(ui, |ui| {
                    for (k, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut kind, k, *n);
                    }
                });
        });
        let mut radius = radius;
        if kind != was {
            self.note_field(egui::Id::new(("fp_shape", i, kind)));
            p.shape = match kind {
                0 => PadShape::Round,
                3 => PadShape::Octagon,
                _ => PadShape::Rect,
            };
            p.outline.clear();
            radius = (kind == 2).then(|| p.size_x.mil().min(p.size_y.mil()) * 0.25);
            sized = true;
        }
        if let Some(r0) = radius {
            let mut pct =
                (r0 / (p.size_x.mil().min(p.size_y.mil()) / 2.0) * 100.0).clamp(0.0, 100.0);
            let r = ui
                .horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new("Corners"));
                    ui.add(
                        egui::DragValue::new(&mut pct)
                            .range(0.0..=100.0)
                            .speed(1.0)
                            .suffix(" %"),
                    )
                })
                .inner;
            if r.changed() {
                self.note_field(r.id);
                radius = Some(p.size_x.mil().min(p.size_y.mil()) / 2.0 * pct / 100.0);
                sized = true;
            }
        }
        if sized {
            reshape(&mut p, radius);
            ch = true;
        }
        // Copper: one side, or through with a hole.
        let mut side = match p.layer {
            LayerRef::AllCopper => 2,
            LayerRef::Copper(0) => 0,
            _ => 1,
        };
        let sides = ["Top", "Bottom", "Through hole"];
        let was = side;
        ui.horizontal(|ui| {
            ui.add_sized([86.0, 18.0], egui::Label::new("Layer"));
            egui::ComboBox::from_id_salt("fp_pad_side")
                .selected_text(sides[side])
                .show_ui(ui, |ui| {
                    for (k, n) in sides.iter().enumerate() {
                        ui.selectable_value(&mut side, k, *n);
                    }
                });
        });
        if side != was {
            self.note_field(egui::Id::new(("fp_side", i, side)));
            let copper = self.lib.footprints[cur].copper.max(2);
            p.layer = match side {
                0 => LayerRef::Copper(0),
                1 => LayerRef::Copper(copper - 1),
                _ => LayerRef::AllCopper,
            };
            if side == 2 && p.hole.0 == 0 {
                p.hole = Length(p.size_x.0.min(p.size_y.0) / 2);
            }
            if side != 2 {
                p.hole = Length(0);
            }
            p.stack.clear();
            ch = true;
        }
        if p.layer == LayerRef::AllCopper {
            ch |= self.len_field(ui, "Hole", &mut p.hole);
            let mut plated = !p.fab.unplated;
            if ui.checkbox(&mut plated, "Plated").changed() {
                self.note_field(egui::Id::new(("fp_plated", i)));
                p.fab.unplated = !plated;
                ch = true;
            }
        }
        if ch {
            self.lib.footprints[cur].pads[i] = p;
        }

        // Copies in a row.
        ui.separator();
        ui.label(RichText::new("Copies in a row").weak());
        if self.array_for != Some((cur, i)) {
            self.array_for = Some((cur, i));
            if let Some(d) = row_step(&self.lib.footprints[cur].pads, i) {
                self.array.1 = d[0];
                self.array.2 = d[1];
            }
        }
        let k = if self.mm { 0.0254 } else { 1.0 };
        let unit = if self.mm { " mm" } else { " mil" };
        let (mut n, mut dx, mut dy) = (self.array.0, self.array.1 * k, self.array.2 * k);
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut n).range(1..=200).prefix("× "));
            ui.add(
                egui::DragValue::new(&mut dx)
                    .speed(0.05)
                    .prefix("dx ")
                    .suffix(unit),
            );
            ui.add(
                egui::DragValue::new(&mut dy)
                    .speed(0.05)
                    .prefix("dy ")
                    .suffix(unit),
            );
        });
        self.array = (n, dx / k, dy / k);
        if ui
            .button(format!("Add {n} copies"))
            .on_hover_text("Each copy steps by dx, dy and counts the number up")
            .clicked()
        {
            let (n, dx, dy) = self.array;
            self.change(|fp| {
                let Some(base) = fp.pads.get(i).cloned() else {
                    return;
                };
                for c in 1..=n {
                    let mut q = base.clone();
                    Self::move_pad(&mut q, [dx * c as f64, dy * c as f64]);
                    q.name = bump(&base.name, c as u32);
                    fp.pads.push(q);
                }
            });
        }
    }

    fn move_pad(p: &mut Pad, d: [f64; 2]) {
        shift(&mut p.at, d);
        for q in p.outline.iter_mut() {
            shift(q, d);
        }
        for l in &mut p.stack {
            for q in l.outline.iter_mut() {
                shift(q, d);
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Pick> {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 20, 24));
        if self.fp().is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No footprint open",
                egui::FontId::proportional(16.0),
                Color32::GRAY,
            );
            return None;
        }
        if self.needs_fit {
            self.fit(rect);
            self.needs_fit = false;
        }
        let (scale, center) = (self.scale, self.center);
        let to_mil = |s: Pos2| -> [f64; 2] {
            [
                center[0] + ((s.x - rect.center().x) / scale) as f64,
                center[1] - ((s.y - rect.center().y) / scale) as f64,
            ]
        };
        let grid = GRIDS[self.grid].1;
        let snap = |p: [f64; 2]| [(p[0] / grid).round() * grid, (p[1] / grid).round() * grid];

        // Zoom about the pointer, pan with the right or middle button.
        if let Some(hp) = resp.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = to_mil(hp);
                self.scale = (self.scale * (scroll / 300.0).exp()).clamp(0.02, 400.0);
                let s = self.scale;
                self.center = [
                    before[0] - ((hp.x - rect.center().x) / s) as f64,
                    before[1] + ((hp.y - rect.center().y) / s) as f64,
                ];
            }
        }
        if resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
        {
            let d = resp.drag_delta();
            self.center[0] -= (d.x / self.scale) as f64;
            self.center[1] += (d.y / self.scale) as f64;
        }
        let (scale, center) = (self.scale, self.center);
        let to_screen = |p: [f64; 2]| -> Pos2 {
            rect.center()
                + Vec2::new(
                    ((p[0] - center[0]) as f32) * scale,
                    -((p[1] - center[1]) as f32) * scale,
                )
        };
        let to_mil = |s: Pos2| -> [f64; 2] {
            [
                center[0] + ((s.x - rect.center().x) / scale) as f64,
                center[1] - ((s.y - rect.center().y) / scale) as f64,
            ]
        };

        // Grid dots, when they are not too close.
        let step_px = grid as f32 * scale;
        let mut g = grid;
        let mut sp = step_px;
        while sp < 12.0 {
            g *= 5.0;
            sp *= 5.0;
        }
        let lo = to_mil(rect.left_bottom());
        let hi = to_mil(rect.right_top());
        let (x0, y0) = ((lo[0] / g).floor() as i64, (lo[1] / g).floor() as i64);
        let (x1, y1) = ((hi[0] / g).ceil() as i64, (hi[1] / g).ceil() as i64);
        if (x1 - x0) * (y1 - y0) < 40_000 {
            for xi in x0..=x1 {
                for yi in y0..=y1 {
                    painter.circle_filled(
                        to_screen([xi as f64 * g, yi as f64 * g]),
                        0.8,
                        Color32::from_gray(60),
                    );
                }
            }
        }
        // The origin.
        let o = to_screen([0.0, 0.0]);
        let cross = Stroke::new(1.0_f32, Color32::from_gray(140));
        painter.line_segment([o - Vec2::new(10.0, 0.0), o + Vec2::new(10.0, 0.0)], cross);
        painter.line_segment([o - Vec2::new(0.0, 10.0), o + Vec2::new(0.0, 10.0)], cross);
        painter.circle_stroke(o, 5.0, cross);

        let fp = &self.lib.footprints[self.current];
        let px = |w: f64| (w as f32 * scale).max(1.0);
        let fill = |ring: &[[f64; 2]], color: Color32| {
            let pts: Vec<Pos2> = ring.iter().map(|&q| to_screen(q)).collect();
            if pts.len() < 3 {
                return;
            }
            let mut mesh = egui::epaint::Mesh::default();
            for &q in &pts {
                mesh.colored_vertex(q, color);
            }
            for t in triangulate(&pts) {
                mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
            }
            painter.add(Shape::mesh(mesh));
        };
        let line = |a: [f64; 2], b: [f64; 2], w: f64, color: Color32| {
            let (a, b) = (to_screen(a), to_screen(b));
            let w = px(w);
            painter.line_segment([a, b], Stroke::new(w, color));
            if w > 2.5 {
                painter.circle_filled(a, w / 2.0, color);
                painter.circle_filled(b, w / 2.0, color);
            }
        };
        let arc_pts = |c: [f64; 2], r: f64, s: f64, e: f64| -> Vec<[f64; 2]> {
            let span = (e - s).rem_euclid(360.0);
            let span = if span == 0.0 { 360.0 } else { span };
            let n = ((span / 6.0).ceil() as usize).max(2);
            (0..=n)
                .map(|k| {
                    let a = (s + span * k as f64 / n as f64).to_radians();
                    [c[0] + r * a.cos(), c[1] + r * a.sin()]
                })
                .collect()
        };
        // Order: mechanical and mask shapes, bottom copper, top copper, silk.
        let rank = |l: &LayerRef| match l {
            LayerRef::Mechanical(_) | LayerRef::Other(_) | LayerRef::Keepout => 0,
            LayerRef::BottomSilk => 1,
            LayerRef::Copper(n) if *n > 0 => 2,
            LayerRef::Copper(_) | LayerRef::AllCopper => 3,
            LayerRef::TopSilk => 4,
        };
        for pass in 0..5 {
            for r in fp.regions.iter().filter(|r| rank(&r.layer) == pass) {
                let ring: Vec<[f64; 2]> = r.outline.iter().map(|&q| mil(q)).collect();
                let c = layer_color(&r.layer);
                fill(&ring, if pass == 3 { c } else { c.gamma_multiply(0.6) });
            }
            if pass == 2 || pass == 3 {
                for p in fp.pads.iter().filter(|p| rank(&p.layer) == pass) {
                    fill(&pad_ring(p), layer_color(&p.layer));
                }
            }
            for t in fp.tracks.iter().filter(|t| rank(&t.layer) == pass) {
                line(
                    mil(t.start),
                    mil(t.end),
                    t.width.mil(),
                    layer_color(&t.layer),
                );
            }
            for a in fp.arcs.iter().filter(|a| rank(&a.layer) == pass) {
                let pts = arc_pts(mil(a.center), a.radius.mil(), a.start_deg, a.end_deg);
                for w in pts.windows(2) {
                    line(w[0], w[1], a.width.mil(), layer_color(&a.layer));
                }
            }
            for t in fp.texts.iter().filter(|t| rank(&t.layer) == pass) {
                for s in fp_strokes(t) {
                    line(
                        mil(s.start),
                        mil(s.end),
                        s.width.mil(),
                        layer_color(&t.layer),
                    );
                }
            }
        }
        // Holes and pad numbers on top.
        for p in &fp.pads {
            if p.hole.0 > 0 {
                let c = to_screen(mil(p.at));
                if p.fab.slot.0 > p.hole.0 {
                    let (s, co) = p.fab.slot_deg.to_radians().sin_cos();
                    let h = (p.fab.slot.mil() - p.hole.mil()) / 2.0;
                    let q = mil(p.at);
                    line(
                        [q[0] - h * co, q[1] - h * s],
                        [q[0] + h * co, q[1] + h * s],
                        p.hole.mil(),
                        Color32::from_rgb(30, 30, 34),
                    );
                } else {
                    painter.circle_filled(c, px(p.hole.mil() / 2.0), Color32::from_rgb(30, 30, 34));
                }
            }
            let size = (p.size_x.mil().min(p.size_y.mil()) as f32 * scale * 0.5).min(18.0);
            if size >= 7.0 {
                painter.text(
                    to_screen(mil(p.at)),
                    egui::Align2::CENTER_CENTER,
                    &p.name,
                    egui::FontId::proportional(size),
                    Color32::WHITE,
                );
            }
        }
        // Selection.
        let hl = Stroke::new(2.0_f32, Color32::from_rgb(255, 200, 60));
        let outline = |ring: &[[f64; 2]]| {
            let pts: Vec<Pos2> = ring.iter().map(|&q| to_screen(q)).collect();
            painter.add(Shape::closed_line(pts, hl));
        };
        match self.sel {
            Some(Sel::Pad(i)) => {
                if let Some(p) = fp.pads.get(i) {
                    outline(&pad_ring(p));
                }
            }
            Some(Sel::Track(i)) => {
                if let Some(t) = fp.tracks.get(i) {
                    painter.line_segment(
                        [to_screen(mil(t.start)), to_screen(mil(t.end))],
                        Stroke::new(px(t.width.mil()) + 3.0, hl.color.gamma_multiply(0.6)),
                    );
                }
            }
            Some(Sel::Arc(i)) => {
                if let Some(a) = fp.arcs.get(i) {
                    let pts: Vec<Pos2> =
                        arc_pts(mil(a.center), a.radius.mil(), a.start_deg, a.end_deg)
                            .into_iter()
                            .map(to_screen)
                            .collect();
                    painter.add(Shape::line(
                        pts,
                        Stroke::new(px(a.width.mil()) + 3.0, hl.color.gamma_multiply(0.6)),
                    ));
                }
            }
            Some(Sel::Region(i)) => {
                if let Some(r) = fp.regions.get(i) {
                    outline(&r.outline.iter().map(|&q| mil(q)).collect::<Vec<_>>());
                }
            }
            Some(Sel::Text(i)) => {
                if let Some(t) = fp.texts.get(i) {
                    for s in fp_strokes(t) {
                        painter.line_segment(
                            [to_screen(mil(s.start)), to_screen(mil(s.end))],
                            Stroke::new(px(s.width.mil()) + 3.0, hl.color.gamma_multiply(0.6)),
                        );
                    }
                }
            }
            None => {}
        }

        // Pointer.
        let pointer = resp.hover_pos().map(to_mil);
        self.hover = pointer.map(snap);
        let tol = 4.0 / scale as f64;
        let mut pick = None;
        if let Some(c) = self.hover {
            if self.tool != Tool::Select {
                let s = to_screen(c);
                let st = Stroke::new(1.0_f32, Color32::from_rgb(120, 200, 255));
                painter.line_segment([s - Vec2::new(8.0, 0.0), s + Vec2::new(8.0, 0.0)], st);
                painter.line_segment([s - Vec2::new(0.0, 8.0), s + Vec2::new(0.0, 8.0)], st);
            }
            if let (Tool::Line, Some(&last)) = (self.tool, self.line_pts.last()) {
                line(
                    last,
                    c,
                    self.line_width.mil(),
                    layer_color(&LINE_LAYERS[self.line_layer].1).gamma_multiply(0.7),
                );
            }
            if let (Tool::Pad, Some(Sel::Pad(i))) = (self.tool, self.sel) {
                if let Some(p) = fp.pads.get(i) {
                    let mut q = p.clone();
                    Self::move_pad(&mut q, [c[0] - p.at.x.mil(), c[1] - p.at.y.mil()]);
                    fill(&pad_ring(&q), layer_color(&q.layer).gamma_multiply(0.4));
                }
            }
            painter.text(
                rect.left_bottom() + Vec2::new(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                format!("x {}   y {}", self.fmt(c[0]), self.fmt(c[1])),
                egui::FontId::monospace(12.0),
                Color32::from_gray(170),
            );
        }

        // Clicks and drags.
        let hit_now = pointer.and_then(|p| Self::hit(fp, p, tol));
        match self.tool {
            Tool::Select => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let start = resp.interact_pointer_pos().map(to_mil);
                    if let Some(p) = start {
                        self.sel = Self::hit(fp, p, tol);
                        if self.sel.is_some() {
                            self.drag = Some(snap(p));
                            let cur = self.current;
                            self.undo.push((cur, self.lib.footprints[cur].clone()));
                            self.redo.clear();
                            self.last_field = None;
                        }
                    }
                } else if let (Some(last), Some(p), Some(sel)) = (self.drag, pointer, self.sel) {
                    if resp.dragged_by(egui::PointerButton::Primary) {
                        let now = snap(p);
                        let d = [now[0] - last[0], now[1] - last[1]];
                        if d != [0.0, 0.0] {
                            Self::move_sel(&mut self.lib.footprints[self.current], sel, d);
                            self.drag = Some(now);
                            self.dirty = true;
                        }
                    }
                }
                if resp.drag_stopped() {
                    self.drag = None;
                }
                if resp.clicked() {
                    self.sel = hit_now;
                    self.last_field = None;
                }
            }
            Tool::Pad => {
                if resp.clicked() {
                    if let Some(c) = self.hover {
                        let template = match self.sel {
                            Some(Sel::Pad(i)) => {
                                self.lib.footprints[self.current].pads.get(i).cloned()
                            }
                            _ => self.lib.footprints[self.current].pads.last().cloned(),
                        }
                        .unwrap_or_else(default_pad);
                        let name = next_name(&self.lib.footprints[self.current]);
                        let mut new_index = 0;
                        self.change(|fp| {
                            let mut q = template.clone();
                            Self::move_pad(
                                &mut q,
                                [c[0] - template.at.x.mil(), c[1] - template.at.y.mil()],
                            );
                            q.name = name;
                            q.net = None;
                            fp.pads.push(q);
                            new_index = fp.pads.len() - 1;
                        });
                        self.sel = Some(Sel::Pad(new_index));
                    }
                }
            }
            Tool::Line => {
                if resp.clicked() {
                    if let Some(c) = self.hover {
                        if let Some(&last) = self.line_pts.last() {
                            if last != c {
                                let (layer, width) =
                                    (LINE_LAYERS[self.line_layer].1.clone(), self.line_width);
                                self.change(|fp| {
                                    fp.tracks.push(Track {
                                        layer,
                                        net: None,
                                        start: Point::mil(last[0], last[1]),
                                        end: Point::mil(c[0], c[1]),
                                        width,
                                        component: None,
                                        text: false,
                                    });
                                });
                            }
                        }
                        self.line_pts.push(c);
                    }
                }
                // A double click ends the line where it was clicked.
                if resp.double_clicked() || resp.secondary_clicked() {
                    self.line_pts.clear();
                }
            }
        }

        // Keys, when the pointer is over the drawing and nothing is typed in.
        let typing = ui.ctx().wants_keyboard_input();
        if resp.hovered() && !typing {
            let plain = |i: &egui::InputState, k: egui::Key| {
                i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == k && !modifiers.command && !modifiers.ctrl && !modifiers.alt)
                })
            };
            let (esc, del, undo, redo, save, pad, line_key, fit, arrows) = ui.input_mut(|i| {
                let arrows = [
                    (egui::Key::ArrowLeft, [-1.0, 0.0]),
                    (egui::Key::ArrowRight, [1.0, 0.0]),
                    (egui::Key::ArrowUp, [0.0, 1.0]),
                    (egui::Key::ArrowDown, [0.0, -1.0]),
                ]
                .into_iter()
                .filter(|(k, _)| i.key_pressed(*k))
                .map(|(_, d)| d)
                .fold([0.0, 0.0], |a: [f64; 2], d: [f64; 2]| {
                    [a[0] + d[0], a[1] + d[1]]
                });
                (
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                        || i.consume_key(
                            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                            egui::Key::Z,
                        ),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                    plain(i, egui::Key::P),
                    plain(i, egui::Key::L),
                    plain(i, egui::Key::F),
                    arrows,
                )
            });
            if esc {
                if self.line_pts.is_empty() {
                    self.tool = Tool::Select;
                } else {
                    self.line_pts.clear();
                }
            }
            if pad {
                self.tool = Tool::Pad;
            }
            if line_key {
                self.tool = Tool::Line;
                self.line_pts.clear();
            }
            if fit {
                self.needs_fit = true;
            }
            if del {
                if let Some(sel) = self.sel.take() {
                    self.change(|fp| Self::delete_sel(fp, sel));
                }
            }
            if arrows != [0.0, 0.0] {
                if let Some(sel) = self.sel {
                    self.change(|fp| Self::move_sel(fp, sel, [arrows[0] * grid, arrows[1] * grid]));
                }
            }
            if undo {
                self.step(true);
            }
            if redo {
                self.step(false);
            }
            if save {
                pick = self.save(false);
            }
        }
        if let Some(Pick::Message(text, err)) = &pick {
            self.note = Some((text.clone(), *err));
        }
        pick
    }

    /// Notes a result from the app (an update on the board, a save).
    pub fn set_note(&mut self, text: String, err: bool) {
        self.note = Some((text, err));
    }
}

/// A footprint text's strokes, with its special strings shown as they are.
fn fp_strokes(t: &FpText) -> Vec<Track> {
    let one = Footprint {
        texts: vec![t.clone()],
        ..Default::default()
    };
    one.text_strokes(None)
}
