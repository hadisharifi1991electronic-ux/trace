//! The schematic editor: draws Altium sheets, cross-probes nets and parts
//! with the board, edits the sheets (move, turn and delete parts, wires,
//! net labels, names and values), saves them as a Trace file, and carries
//! the edits to the board.

use crate::canvas::View;
use crate::scene::Scene;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Shape, Stroke};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use trace_core::eco::{Change, PartInfo, Plan, Sources};
use trace_core::footprint::{designator_prefix, next_designator, Footprint};
use trace_core::hier::{self, Scope};
use trace_core::sch_edit::{snap, Xf, HARNESS_LINE, WIRE_COLOR};
use trace_core::sch_select::SchSel;
use trace_core::schematic::{Pt, TagKind};
use trace_core::{Drawing, SavedSheet, SchFile, SchGeom, Sheet};

mod erc;

/// The one part, tag and wire a selection holds alone, for the side panel.
fn singles(sel: &SchSel) -> (Option<usize>, Option<usize>, Option<usize>) {
    let only = |v: &[usize], rest: usize| (v.len() == 1 && rest == 0).then(|| v[0]);
    (
        only(&sel.parts, sel.len() - sel.parts.len()),
        only(&sel.tags, sel.len() - sel.tags.len() - sel.wires.len()),
        only(&sel.wires, sel.len() - sel.wires.len() - sel.tags.len()),
    )
}

/// Sets the selection field by field, so it works while a sheet is
/// borrowed.
macro_rules! set_sel {
    ($s:ident, $sel:expr) => {{
        let x = $sel;
        ($s.sel_part, $s.sel_tag, $s.sel_wire) = singles(&x);
        $s.sel = x;
    }};
}

/// What a press at `p` takes hold of: a power port (by its symbol too),
/// else a part, a wire, a junction or a No ERC marker.
fn hit_sel(d: &Drawing, boxes: &[[f32; 4]], p: [f64; 2], tol: f64) -> SchSel {
    let near = |a: [f64; 2], r: f64| (a[0] - p[0]).hypot(a[1] - p[1]) < r;
    let tag = d.tags.iter().position(|t| {
        t.kind != TagKind::SheetEntry
            && (t.at.iter().any(|&a| near(a, tol * 2.0))
                || (t.kind == TagKind::Port
                    && t.at.len() == 2
                    && seg_dist(p, t.at[0], t.at[1]) < tol * 2.0))
    });
    let tag = tag.or_else(|| {
        let (x, y) = (p[0] as f32, p[1] as f32);
        d.shapes.iter().enumerate().find_map(|(si, s)| {
            let b = boxes.get(si)?;
            let t = s.tag?;
            let kind = d.tags.get(t as usize)?.kind;
            (kind == TagKind::Power && x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3])
                .then_some(t as usize)
        })
    });
    if let Some(t) = tag {
        return SchSel::tag(t);
    }
    if let Some(i) = part_at(d, p) {
        return SchSel::part(i);
    }
    if let Some(h) = d.harnesses.iter().position(|h| {
        p[0] >= h.min[0] - tol
            && p[0] <= h.max[0] + tol
            && p[1] >= h.min[1] - tol
            && p[1] <= h.max[1] + tol
    }) {
        return SchSel {
            harnesses: vec![h],
            ..Default::default()
        };
    }
    if let Some(e) = d
        .bus_entries
        .iter()
        .position(|e| seg_dist(p, e[0], e[1]) < tol)
    {
        return SchSel {
            bus_entries: vec![e],
            ..Default::default()
        };
    }
    if let Some(m) = d.no_erc.iter().position(|&a| near(a, tol * 2.0)) {
        return SchSel {
            no_erc: vec![m],
            ..Default::default()
        };
    }
    if let Some(w) = d
        .wires
        .iter()
        .position(|w| w.windows(2).any(|s| seg_dist(p, s[0], s[1]) < tol))
    {
        return SchSel::wire(w);
    }
    if let Some(j) = d.junctions.iter().position(|&a| near(a, tol * 2.0)) {
        return SchSel {
            junctions: vec![j],
            ..Default::default()
        };
    }
    let on_line = |w: &Vec<[f64; 2]>, t: f64| w.windows(2).any(|s| seg_dist(p, s[0], s[1]) < t);
    if let Some(b) = d
        .signal_harnesses
        .iter()
        .position(|w| on_line(w, tol * 2.0))
    {
        return SchSel {
            signal_harnesses: vec![b],
            ..Default::default()
        };
    }
    if let Some(b) = d.buses.iter().position(|w| on_line(w, tol * 1.5)) {
        return SchSel {
            buses: vec![b],
            ..Default::default()
        };
    }
    SchSel::default()
}

/// Draws the lines and circles of `g` as a preview in one colour.
fn paint_ghost(painter: &egui::Painter, view: View, g: &Drawing, c: Color32) {
    let to = |p: &[f64; 2]| view.to_screen(p[0] as f32, p[1] as f32);
    let stroke = Stroke::new((10.0 * view.scale).max(1.5), c);
    for s in &g.shapes {
        match &s.geom {
            SchGeom::Line(v) => {
                painter.add(Shape::line(v.iter().map(to).collect(), stroke));
            }
            SchGeom::Poly(v) => {
                painter.add(Shape::closed_line(v.iter().map(to).collect(), stroke));
            }
            SchGeom::Arc {
                c: cc,
                r,
                start,
                end,
            } => {
                let pts: Vec<Pos2> = arc_points(*cc, *r, *start, *end).iter().map(to).collect();
                painter.add(Shape::line(pts, stroke));
            }
            SchGeom::Rect { min, max, .. } => {
                painter.rect_stroke(Rect::from_two_pos(to(min), to(max)), 0.0, stroke);
            }
        }
    }
    for t in &g.texts {
        let size = (t.size as f32 * view.scale).max(9.0);
        let fx = [0.0, 0.5, 1.0][(t.just % 3) as usize];
        let fy = [1.0, 0.5, 0.0][(t.just / 3).min(2) as usize];
        let align = egui::Align2([
            [egui::Align::Min, egui::Align::Center, egui::Align::Max][(fx * 2.0) as usize],
            [egui::Align::Min, egui::Align::Center, egui::Align::Max][(fy * 2.0) as usize],
        ]);
        painter.text(
            to(&t.at),
            align,
            &t.text,
            FontId::proportional(size * 0.92),
            c,
        );
    }
}

/// Grid the editing tools snap to, in mil.
const GRID: f64 = 50.0;

/// What a click on the sheet does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchTool {
    #[default]
    Select,
    Wire,
    Label,
    /// Places power ports (GND, +3V3...).
    Power,
    /// Places No ERC markers.
    NoErc,
    /// Draws buses (thick lines carrying D[0..7] and the like).
    Bus,
    /// Places 45 degree bus entries on a bus (Space turns them).
    BusEntry,
    /// Places harness connectors.
    Harness,
    /// Draws signal harnesses between connectors, ports and sheet entries.
    SignalHarness,
    /// Places ports, for nets, buses and harnesses that leave the sheet.
    Port,
}

/// An edit worked out while drawing, made once the drawing is done with.
enum Act {
    /// Shifts the selection (one undo step covers a whole drag).
    Move([f64; 2]),
    /// Turns or mirrors the selection.
    Xf(XfKind),
    Delete,
    CopyPart(usize),
    Copy,
    Cut,
    Paste,
    /// Copies the selection and pastes it to follow the pointer.
    Duplicate,
    SelectAll,
    Power([f64; 2]),
    NoErc([f64; 2]),
    Wire(Vec<[f64; 2]>),
    Bus(Vec<[f64; 2]>),
    SignalHarness(Vec<[f64; 2]>),
    BusEntry([f64; 2]),
    Harness([f64; 2]),
    Port([f64; 2]),
    /// Drags a wire's segment (or corner, when true) by the whole drag so
    /// far, from the sheet as it was when the drag began.
    DragWire(usize, usize, bool, [f64; 2]),
    Label([f64; 2], String),
    /// Takes back a symbol that was being placed.
    CancelPlace,
    Undo,
    Redo,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum XfKind {
    Turn,
    MirrorX,
    MirrorY,
}

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
    /// Carry the schematic edits to the board, as one step.
    Eco(Vec<trace_core::edit::Edit>),
    /// Something to tell the person.
    Message(String, bool),
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
    pub tool: SchTool,
    /// Each sheet as the board last matched it.
    baseline: Vec<Option<Drawing>>,
    /// Each step: the sheets it changed, as they were.
    undo: Vec<Vec<(usize, Sheet)>>,
    redo: Vec<Vec<(usize, Sheet)>>,
    /// Electrical rule check results, and annotation settings.
    erc: erc::ErcState,
    /// How the sheets' nets join (the project's net identifier scope).
    pub scope: Scope,
    /// Designators renamed on the sheets: now -> as on the board.
    renames: HashMap<String, String>,
    /// The sheets as first read, which the board file as it was matches,
    /// and every rename since.
    original: Vec<Option<Drawing>>,
    original_renames: HashMap<String, String>,
    /// Edited since saved.
    pub dirty: bool,
    /// The Trace schematic file the sheets were read from or saved to.
    pub saved_to: Option<PathBuf>,
    sel_wire: Option<usize>,
    sel_tag: Option<usize>,
    /// Everything selected on the current sheet.
    sel: SchSel,
    /// The selection is being dragged: the grid point it was last moved to.
    drag: Option<[f64; 2]>,
    /// Where a selection box was started, in sheet mil.
    boxing: Option<[f64; 2]>,
    /// What Ctrl+C took, as a sheet of its own.
    clip: Option<Sheet>,
    /// The power port the Power tool places: net, Altium style, quarter
    /// turns from pointing right.
    power_net: String,
    power_style: i64,
    power_orient: i64,
    /// Points of the wire being drawn.
    wire_pts: Vec<[f64; 2]>,
    /// Where a net label goes, while its name is typed.
    label_at: Option<[f64; 2]>,
    label_text: String,
    /// Name and value fields for the selected part: (sheet, part).
    edit_for: Option<(usize, usize)>,
    edit_name: String,
    edit_value: String,
    edit_tag: String,
    /// Changes waiting to go to the board.
    eco: Option<Plan>,
    note: Option<String>,
    /// Symbol libraries, for placing new parts.
    pub libs: crate::symlib::Libraries,
    /// The symbol library editor.
    pub symedit: crate::symedit::SymEditor,
    /// The dragged part follows the pointer until a click drops it (a
    /// symbol just taken from a library).
    floating: bool,
    /// Dragging moves without wires following (Altium's Move) instead of
    /// with them (Drag). Ctrl at the start of a drag does the other one.
    pub move_breaks: bool,
    /// Whether the drag going on drags (wires follow).
    drag_keeps: bool,
    /// A wire segment (or corner) being dragged: wire, segment or point,
    /// corner, where the drag began.
    wire_drag: Option<(usize, usize, bool, [f64; 2])>,
    /// Bus entry tool: quarter turns of the entry, from up and right.
    entry_turn: u8,
    /// Harness connector tool: its type and entries.
    harness_kind: String,
    harness_entries: String,
    harness_left: bool,
    /// Port tool: its name.
    port_name: String,
    /// Bus breakout: the members' bus name, and which side.
    breakout_name: String,
    breakout_flip: bool,
}

/// The drawing to keep as the board's match: no pictures.
fn synced(d: &Drawing) -> Drawing {
    let mut d = d.clone();
    d.images.clear();
    d
}

/// The smallest part box holding `p`.
fn part_at(d: &Drawing, p: [f64; 2]) -> Option<usize> {
    d.parts
        .iter()
        .enumerate()
        .filter(|(_, q)| {
            p[0] >= q.min[0] && p[0] <= q.max[0] && p[1] >= q.min[1] && p[1] <= q.max[1]
        })
        .min_by(|a, b| {
            let area = |q: &trace_core::SchPart| (q.max[0] - q.min[0]) * (q.max[1] - q.min[1]);
            area(a.1).total_cmp(&area(b.1))
        })
        .map(|(i, _)| i)
}

/// The point on the wire nearest `p`, if one runs within `catch`: on the
/// grid along the wire where that stays on it.
fn on_wire_near(d: &Drawing, p: [f64; 2], catch: f64) -> Option<[f64; 2]> {
    on_line_near(&d.wires, p, catch)
}

/// The nearest point on one of `lines` within `catch` of `p`, on the
/// grid along level and upright runs.
fn on_line_near(lines: &[Vec<[f64; 2]>], p: [f64; 2], catch: f64) -> Option<[f64; 2]> {
    let mut best: Option<([f64; 2], f64)> = None;
    for w in lines {
        for s in w.windows(2) {
            let dist = seg_dist(p, s[0], s[1]);
            if dist < catch && best.is_none_or(|b| dist < b.1) {
                let (a, b) = (s[0], s[1]);
                let q = if (a[0] - b[0]).abs() < 1e-6 {
                    let y = snap(p, GRID)[1];
                    let y = if y >= a[1].min(b[1]) && y <= a[1].max(b[1]) {
                        y
                    } else {
                        p[1]
                    };
                    [a[0], y]
                } else if (a[1] - b[1]).abs() < 1e-6 {
                    let x = snap(p, GRID)[0];
                    let x = if x >= a[0].min(b[0]) && x <= a[0].max(b[0]) {
                        x
                    } else {
                        p[0]
                    };
                    [x, a[1]]
                } else {
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy))
                        .clamp(0.0, 1.0);
                    [a[0] + t * dx, a[1] + t * dy]
                };
                best = Some((q, dist));
            }
        }
    }
    best.map(|b| b.0)
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

/// Keeps a file about to be replaced as `name.bak`, the first time only.
fn backup(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Ok(());
    }
    let mut bak = p.as_os_str().to_owned();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    if !bak.exists() {
        std::fs::copy(p, &bak).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    Ok(())
}

/// A project's text with its sheets pointing at the files written (by
/// name, next to it) and its other documents at where they are.
fn retarget_project(text: &str, from_dir: &Path, sheets: &[(PathBuf, String)]) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let Some(p) = line.trim_start().strip_prefix("DocumentPath=") else {
            out.push_str(line);
            out.push_str("\r\n");
            continue;
        };
        let full = from_dir.join(p.replace('\\', std::path::MAIN_SEPARATOR_STR));
        let same = |a: &Path| {
            a.file_name().map(|f| f.to_ascii_lowercase())
                == full.file_name().map(|f| f.to_ascii_lowercase())
        };
        let new = match sheets.iter().find(|(src, _)| same(src)) {
            Some((_, name)) => name.clone(),
            None => full.display().to_string(),
        };
        out.push_str(&format!("DocumentPath={new}\r\n"));
    }
    out
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
pub(crate) fn triangulate(pts: &[Pos2]) -> Vec<[usize; 3]> {
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
        let libs = std::mem::take(&mut self.libs);
        *self = Schematics {
            project,
            needs_fit: true,
            libs,
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

    /// Opens a `.SchDoc`, a Trace schematic, or every sheet of a `.PrjPcb`
    /// (from its Trace schematic, when it has been saved as one).
    pub fn open_path(&mut self, path: &Path, ctx: &egui::Context) {
        if ext(path) == "tracesch" {
            if let Err(e) = self.open_saved(path, None) {
                self.note = Some(format!("Could not read {}: {e}", path.display()));
            }
            return;
        }
        if ext(path) == "prjpcb" {
            let saved = path.with_extension("tracesch");
            if saved.exists() && self.open_saved(&saved, Some(path.to_path_buf())).is_ok() {
                return;
            }
            let sheets = project_docs(path)
                .into_iter()
                .filter(|p| ext(p) == "schdoc" && p.exists())
                .collect();
            self.open(sheets, Some(path.to_path_buf()), ctx);
            self.scope = trace_altium::project_scope(path);
            self.project_libs();
        } else {
            self.open(vec![path.to_path_buf()], None, ctx);
        }
    }

    pub fn with_libs(libs: crate::symlib::Libraries) -> Self {
        Schematics {
            libs,
            ..Default::default()
        }
    }

    /// Opens the symbol libraries the project lists.
    fn project_libs(&mut self) {
        if let Some(prj) = &self.project {
            let libs: Vec<PathBuf> = project_docs(prj)
                .into_iter()
                .filter(|p| ext(p) == "schlib" || ext(p) == "intlib")
                .collect();
            self.libs.load_quietly(&libs);
        }
    }

    /// The designator for a new unit of library symbol `lib`: for a later
    /// unit of a multi-part component (U1B), the first placed component of
    /// the same kind that lacks that unit; otherwise the next free one.
    fn unit_designator(&self, lib: &trace_core::LibSymbol, scene: Option<&Scene>) -> String {
        if lib.parts > 1 {
            let letter = (b'A' + (lib.part.clamp(1, 26) - 1) as u8) as char;
            let mut units: std::collections::BTreeMap<String, HashSet<char>> = Default::default();
            for sheet in self.slots.iter().filter_map(|s| s.sheet.as_ref()) {
                let d = &sheet.drawing;
                for (i, q) in d.parts.iter().enumerate() {
                    let same = sheet
                        .symbols
                        .get(q.symbol)
                        .is_some_and(|s| s.library_ref == lib.name);
                    if !same {
                        continue;
                    }
                    let set = units.entry(q.designator.clone()).or_default();
                    for t in d.texts.iter().filter(|t| t.part == Some(i as u32)) {
                        if let Some(l) = t.text.strip_prefix(q.designator.as_str()) {
                            if l.len() == 1 {
                                set.extend(l.chars());
                            }
                        }
                    }
                }
            }
            if let Some((d, _)) = units.iter().find(|(_, set)| !set.contains(&letter)) {
                return d.clone();
            }
        }
        self.free_designator(&lib.symbol.designator, scene)
    }

    /// Takes symbol `at` of the libraries onto the current sheet, at the
    /// middle of the view, following the pointer until a click drops it.
    fn start_place(&mut self, at: (usize, usize), scene: Option<&Scene>) {
        let Some(lib) = self.libs.symbol(at).cloned() else {
            return;
        };
        self.place_lib(lib, scene);
    }

    /// What the symbol editor asked for.
    pub fn symbol_act(&mut self, act: crate::symedit::SymAct, scene: Option<&Scene>) {
        use crate::symedit::SymAct;
        match act {
            SymAct::Saved(p) => {
                if let Err(e) = self.libs.load(&p) {
                    self.symedit.note = Some(format!("Saved, but could not read it back: {e}"));
                }
            }
            SymAct::Place(lib) => {
                self.place_lib(*lib, scene);
                if let Some(n) = &self.note {
                    self.symedit.note = Some(n.clone());
                }
            }
            SymAct::Update(parts) => {
                let mut step = Vec::new();
                let mut done: Vec<String> = Vec::new();
                let mut moved = 0;
                let mut lost: Vec<String> = Vec::new();
                for i in 0..self.slots.len() {
                    let Some(sheet) = self.slots[i].sheet.as_mut() else {
                        continue;
                    };
                    let before = sheet.clone();
                    let rep = sheet.update_from_library(&parts);
                    if rep.updated.is_empty() {
                        continue;
                    }
                    step.push((i, before));
                    done.extend(rep.updated);
                    moved += rep.wires_moved;
                    lost.extend(rep.lost_pins);
                }
                let name = parts.first().map(|p| p.name.clone()).unwrap_or_default();
                let text = if step.is_empty() {
                    format!("No placed {name} on the open sheets.")
                } else {
                    let changed: Vec<usize> = step.iter().map(|s| s.0).collect();
                    self.undo.push(step);
                    self.redo.clear();
                    for i in changed {
                        self.after_edit(i);
                    }
                    let mut t = format!(
                        "Redrew {} placed part(s) of {name} ({}); wires followed {moved} moved pin(s).",
                        done.len(),
                        done.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
                    );
                    if !lost.is_empty() {
                        t += &format!(
                            " Pins gone from the symbol (their wires are loose): {}.",
                            lost.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
                        );
                    }
                    t
                };
                self.note = Some(text.clone());
                self.symedit.note = Some(text);
            }
        }
    }

    /// Takes a library symbol onto the current sheet, at the middle of the
    /// view, following the pointer until a click drops it.
    fn place_lib(&mut self, lib: trace_core::LibSymbol, scene: Option<&Scene>) {
        if self
            .slots
            .get(self.current)
            .and_then(|s| s.sheet.as_ref())
            .is_none()
        {
            self.note = Some("Open a sheet first.".into());
            return;
        }
        let c = self.view.to_board(self.view.rect.center());
        let spot = snap([c[0] as f64, c[1] as f64], GRID);
        let name = self.unit_designator(&lib, scene);
        let mut made = None;
        let n2 = name.clone();
        self.edit(|s| made = s.place_symbol(&lib, spot, &n2));
        if let Some(n) = made {
            self.tool = SchTool::Select;
            self.pick_sel(SchSel::part(n));
            self.drag = Some(spot);
            self.floating = true;
            self.note = Some(format!(
                "Placing {name} ({}): click to drop it, Space turns it, Esc cancels.",
                lib.title()
            ));
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
                            if self.baseline.len() < i + 1 {
                                self.baseline.resize(i + 1, None);
                            }
                            self.baseline[i] = Some(synced(&sheet.drawing));
                            if self.original.len() < i + 1 {
                                self.original.resize(i + 1, None);
                            }
                            self.original[i] = Some(synced(&sheet.drawing));
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

    /// Reads sheets saved by Trace.
    pub fn open_saved(&mut self, path: &Path, project: Option<PathBuf>) -> Result<(), String> {
        let f = SchFile::load(path).map_err(|e| e.to_string())?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let libs = std::mem::take(&mut self.libs);
        *self = Schematics {
            libs,
            project: project.or_else(|| {
                f.project
                    .as_ref()
                    .map(|p| dir.join(p))
                    .filter(|p| p.exists())
            }),
            needs_fit: true,
            saved_to: Some(path.to_path_buf()),
            ..Default::default()
        };
        for mut s in f.sheets {
            // Harness bundles are worked out, not saved.
            s.sheet.drawing.connect();
            let file = dir.join(&s.file);
            self.slots.push(Slot {
                name: file
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: file,
                boxes: s
                    .sheet
                    .drawing
                    .shapes
                    .iter()
                    .map(|sh| shape_box(&sh.geom, sh.width))
                    .collect(),
                error: None,
                sheet: None,
            });
            let base = s.synced.unwrap_or_else(|| synced(&s.sheet.drawing));
            self.original
                .push(Some(s.original.unwrap_or_else(|| base.clone())));
            self.baseline.push(Some(base));
            self.slots.last_mut().unwrap().sheet = Some(s.sheet);
        }
        self.renames = f.renames.into_iter().collect();
        self.original_renames = f.original_renames.into_iter().collect();
        self.scope = f.scope;
        self.project_libs();
        Ok(())
    }

    /// Where the sheets save to: the file they came from, else next to the
    /// project or first sheet.
    fn save_path(&self) -> Option<PathBuf> {
        self.saved_to
            .clone()
            .or_else(|| self.project.as_ref().map(|p| p.with_extension("tracesch")))
            .or_else(|| {
                self.slots
                    .first()
                    .map(|s| s.path.with_extension("tracesch"))
            })
    }

    /// Saves every sheet as one Trace schematic file.
    pub fn save(&mut self) -> Result<PathBuf, String> {
        if self.loader.is_some() {
            return Err("Wait until every sheet has been read.".into());
        }
        let path = self.save_path().ok_or("Nothing to save.")?;
        let file = SchFile {
            version: env!("CARGO_PKG_VERSION").to_string(),
            project: self
                .project
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned()),
            sheets: self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    Some(SavedSheet {
                        file: s.path.file_name()?.to_string_lossy().into_owned(),
                        sheet: s.sheet.clone()?,
                        synced: self.baseline.get(i).cloned().flatten(),
                        original: self.original.get(i).cloned().flatten(),
                    })
                })
                .collect(),
            renames: self
                .renames
                .iter()
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect(),
            original_renames: self
                .original_renames
                .iter()
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect(),
            scope: self.scope,
        };
        file.save(&path).map_err(|e| e.to_string())?;
        self.saved_to = Some(path.clone());
        self.dirty = false;
        Ok(path)
    }

    /// Writes every sheet into `dir` as an Altium `.SchDoc`, keeping what
    /// each sheet's own `.SchDoc` holds that Trace does not draw. A file
    /// already there is kept as `.bak` first. With a project, a copy of it
    /// that lists the written sheets goes there too. Returns how many
    /// sheets were written.
    pub fn save_altium(&mut self, dir: &Path) -> Result<usize, String> {
        if self.loader.is_some() {
            return Err("Wait until every sheet has been read.".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut written = Vec::new();
        for slot in &self.slots {
            let Some(sheet) = &slot.sheet else { continue };
            let name = format!("{}.SchDoc", slot.name);
            let original =
                (ext(&slot.path) == "schdoc" && slot.path.is_file()).then_some(slot.path.as_path());
            let bytes =
                trace_altium::write_schdoc(sheet, original).map_err(|e| format!("{name}: {e}"))?;
            let to = dir.join(&name);
            backup(&to)?;
            std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))?;
            written.push((slot.path.clone(), name));
        }
        if let Some(prj) = &self.project {
            let from_dir = prj.parent().unwrap_or(Path::new("."));
            if from_dir.canonicalize().ok() != dir.canonicalize().ok() {
                let text = std::fs::read(prj)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .map_err(|e| e.to_string())?;
                let to = dir.join(prj.file_name().unwrap_or_default());
                backup(&to)?;
                std::fs::write(&to, retarget_project(&text, from_dir, &written))
                    .map_err(|e| format!("{}: {e}", to.display()))?;
            }
        }
        Ok(written.len())
    }

    /// The folder of the project, or of the first sheet.
    fn project_dir(&self) -> PathBuf {
        self.project
            .as_ref()
            .or(self.slots.first().map(|s| &s.path))
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    /// What carrying the sheet edits to `board` would change. A board read
    /// fresh from its file is compared with the sheets as first read; one
    /// already updated (or saved by Trace after an update) with the sheets
    /// as it last took them.
    fn plan(&self, scene: &Scene, library: &[Footprint]) -> Plan {
        let fresh = !scene.from_sheets && ext(&scene.path) != "tracepcb";
        let (base, renames) = if fresh {
            (&self.original, &self.original_renames)
        } else {
            (&self.baseline, &self.renames)
        };
        let before: Vec<&Drawing> = base.iter().flatten().collect();
        let after: Vec<&Drawing> = self
            .slots
            .iter()
            .filter_map(|s| s.sheet.as_ref().map(|s| &s.drawing))
            .collect();
        let before_names: Vec<&str> = base
            .iter()
            .zip(&self.slots)
            .filter(|(b, _)| b.is_some())
            .map(|(_, s)| s.name.as_str())
            .collect();
        let after_names: Vec<&str> = self
            .slots
            .iter()
            .filter(|s| s.sheet.is_some())
            .map(|s| s.name.as_str())
            .collect();
        // Renames still standing: the new name is on a sheet, the old one
        // no longer is.
        let on_sheets: HashSet<String> = after
            .iter()
            .flat_map(|d| d.parts.iter().map(|p| p.designator.to_ascii_uppercase()))
            .collect();
        let renames: HashMap<String, String> = renames
            .iter()
            .filter(|(now, was)| on_sheets.contains(*now) && !on_sheets.contains(*was))
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        // What the sheets say about each part, for parts new on them.
        let mut parts: HashMap<String, PartInfo> = HashMap::new();
        for slot in &self.slots {
            let Some(sheet) = &slot.sheet else { continue };
            for p in &sheet.drawing.parts {
                let Some(sym) = sheet.symbols.get(p.symbol) else {
                    continue;
                };
                parts
                    .entry(p.designator.to_ascii_uppercase())
                    .or_insert_with(|| PartInfo {
                        footprint: sym.footprint.clone(),
                        value: sym.value.clone(),
                        library_ref: sym.library_ref.clone(),
                        sheet: slot.name.clone(),
                    });
            }
        }
        // The open footprint library first, then those of open IntLibs.
        let all: Vec<Footprint>;
        let library = if self.libs.has_footprints() {
            all = library
                .iter()
                .chain(self.libs.footprints())
                .cloned()
                .collect();
            &all[..]
        } else {
            library
        };
        let sources = Sources {
            parts: Some(&parts),
            library,
            hier: Some(trace_core::eco::Hier {
                before: &before_names,
                after: &after_names,
                scope: self.scope,
            }),
        };
        trace_core::eco::plan_with(&before, &after, &renames, &scene.board, sources)
    }

    /// The first free designator with the letters of `like` (R for R12),
    /// across every sheet and the board.
    fn free_designator(&self, like: &str, scene: Option<&Scene>) -> String {
        let taken: HashSet<String> = self
            .slots
            .iter()
            .filter_map(|s| s.sheet.as_ref())
            .flat_map(|s| s.drawing.parts.iter())
            .map(|p| p.designator.to_ascii_uppercase())
            .collect();
        let prefix = designator_prefix(like);
        let prefix = if prefix.is_empty() { "U" } else { prefix };
        match scene {
            Some(sc) => next_designator(&sc.board, prefix, &taken),
            None => next_designator(&trace_core::Board::default(), prefix, &taken),
        }
    }

    /// Copies part `i` of the current sheet beside it as a new part with
    /// the next free designator, and selects the copy.
    fn copy_part(&mut self, i: usize, scene: Option<&Scene>) {
        let cur = self.current;
        let Some((name, d)) = self
            .slots
            .get(cur)
            .and_then(|s| s.sheet.as_ref())
            .and_then(|s| {
                Some((
                    s.drawing.parts.get(i)?.designator.clone(),
                    s.drawing.clear_spot(i),
                ))
            })
        else {
            return;
        };
        let new = self.free_designator(&name, scene);
        let mut made = None;
        let n2 = new.clone();
        self.edit(|s| made = s.copy_part(i, d, &n2));
        if let Some(n) = made {
            self.pick_sel(SchSel::part(n));
            self.note = Some(format!(
                "Copied {name} as {new}. Wire it up, then Update PCB adds it to the board."
            ));
        }
    }

    /// Selects `sel`; one part, wire or tag alone is also the one the side
    /// panel edits.
    fn pick_sel(&mut self, sel: SchSel) {
        set_sel!(self, sel);
    }

    /// New designators for the parts of a clipboard sheet: the next free
    /// ones, gates of one part sharing theirs.
    fn paste_names(&self, clip: &Sheet, scene: Option<&Scene>) -> HashMap<String, String> {
        let mut taken: HashSet<String> = self
            .slots
            .iter()
            .filter_map(|s| s.sheet.as_ref())
            .flat_map(|s| s.drawing.parts.iter())
            .map(|p| p.designator.to_ascii_uppercase())
            .collect();
        let empty = trace_core::Board::default();
        let board = scene.map_or(&empty, |sc| &sc.board);
        let mut names = HashMap::new();
        for q in &clip.drawing.parts {
            if names.contains_key(&q.designator) {
                continue;
            }
            let prefix = designator_prefix(&q.designator);
            let prefix = if prefix.is_empty() { "U" } else { prefix };
            let n = next_designator(board, prefix, &taken);
            taken.insert(n.to_ascii_uppercase());
            names.insert(q.designator.clone(), n);
        }
        names
    }

    /// Pastes the clipboard to follow the pointer (at `at`) until a click
    /// drops it.
    fn paste(&mut self, at: [f64; 2], scene: Option<&Scene>) {
        let Some(clip) = self.clip.clone() else {
            self.note = Some("Nothing copied yet: select something and press Ctrl+C.".into());
            return;
        };
        let Some((lo, _)) = clip.drawing.sel_bounds(&clip.drawing.select_all()) else {
            return;
        };
        let names = self.paste_names(&clip, scene);
        let d = snap([at[0] - lo[0], at[1] - lo[1]], GRID);
        let mut made = SchSel::default();
        self.edit(|s| made = s.paste_clip(&clip, d, &names));
        let n = made.parts.len();
        self.pick_sel(made);
        self.tool = SchTool::Select;
        self.drag = Some(snap(at, GRID));
        self.floating = true;
        self.note = Some(if n > 0 {
            format!("Pasted with new designators ({n} parts): click to drop, Space turns, X / Y mirror, Esc cancels.")
        } else {
            "Pasted: click to drop, Space turns, X / Y mirror, Esc cancels.".into()
        });
    }

    /// The board now matches the sheets.
    fn synced_now(&mut self) {
        self.baseline = self
            .slots
            .iter()
            .map(|s| s.sheet.as_ref().map(|s| synced(&s.drawing)))
            .collect();
        self.renames.clear();
        self.dirty = true;
    }

    /// Makes one edit to the current sheet as an undoable step.
    fn edit(&mut self, f: impl FnOnce(&mut Sheet)) {
        let cur = self.current;
        let Some(sheet) = self.slots.get_mut(cur).and_then(|s| s.sheet.as_mut()) else {
            return;
        };
        self.undo.push(vec![(cur, sheet.clone())]);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        f(sheet);
        self.after_edit(cur);
    }

    /// Brings what is worked out from a sheet up to date after it changed.
    fn after_edit(&mut self, i: usize) {
        if let Some(s) = self.slots.get_mut(i) {
            if let Some(sheet) = &s.sheet {
                if i == self.current {
                    self.sel.clamp(&sheet.drawing);
                }
                s.boxes = sheet
                    .drawing
                    .shapes
                    .iter()
                    .map(|sh| shape_box(&sh.geom, sh.width))
                    .collect();
            }
        }
        self.hl_for = None;
        self.sel_net = None;
        self.dirty = true;
        self.erc.stale = true;
    }

    fn step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(group) = from.pop() else { return };
        let mut back = Vec::new();
        let mut touched = Vec::new();
        for (i, sheet) in group {
            let Some(slot) = self.slots.get_mut(i) else {
                continue;
            };
            if let Some(now) = slot.sheet.replace(sheet) {
                back.push((i, now));
            }
            touched.push(i);
        }
        if !back.is_empty() {
            to.push(back);
        }
        if !touched.contains(&self.current) {
            if let Some(&i) = touched.first() {
                self.select_sheet(i);
            }
        }
        self.pick_sel(SchSel::default());
        for i in touched {
            self.after_edit(i);
        }
        self.erc.stale = true;
    }

    pub fn select_sheet(&mut self, i: usize) {
        if i < self.slots.len() && i != self.current {
            self.current = i;
            self.sel_net = None;
            self.pick_sel(SchSel::default());
            self.wire_pts.clear();
            self.drag = None;
            self.boxing = None;
            self.needs_fit = true;
        }
    }

    /// How the sheets nest (sheets not read yet count as empty).
    fn hier_tree(&self) -> hier::Tree {
        let empty = Drawing::default();
        let names: Vec<&str> = self.slots.iter().map(|s| s.name.as_str()).collect();
        let ds: Vec<&Drawing> = self
            .slots
            .iter()
            .map(|s| s.sheet.as_ref().map_or(&empty, |s| &s.drawing))
            .collect();
        hier::tree(&names, &ds)
    }

    /// The port a sheet entry leads to, or the sheet entry above a port.
    fn other_end(&self, sheet: usize, tag: usize) -> Option<(usize, Pt)> {
        let empty = Drawing::default();
        let ds: Vec<&Drawing> = self
            .slots
            .iter()
            .map(|s| s.sheet.as_ref().map_or(&empty, |s| &s.drawing))
            .collect();
        let (s, t) = hier::other_end(&ds, &self.hier_tree(), sheet, tag)?;
        Some((s, ds[s].tags[t].at.first().copied()?))
    }

    /// Goes to sheet `i` and frames `at`.
    fn go_to(&mut self, i: usize, at: Pt) {
        self.select_sheet(i);
        self.needs_fit = false;
        let (x, y) = (at[0] as f32, at[1] as f32);
        self.focus = Some([x - 2400.0, y - 1600.0, x + 2400.0, y + 1600.0]);
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
                self.pick_sel(SchSel::part(pi));
                return true;
            }
        }
        false
    }

    /// The sheet list for the side panel.
    pub fn side_panel(
        &mut self,
        ui: &mut egui::Ui,
        board_net: Option<u32>,
        scene: Option<&Scene>,
        library: &[Footprint],
    ) {
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
        // Editing tools.
        ui.horizontal(|ui| {
            for (t, name, tip) in [
                (
                    SchTool::Select,
                    "Select",
                    "Click to select (Shift or Ctrl+click adds), drag to move, drag on the sheet for a box (left to right: inside; right to left: touching). Right or middle drag pans.",
                ),
                (
                    SchTool::Wire,
                    "Wire",
                    "Draw wires: click each corner, double-click or Esc to end (W)",
                ),
                (
                    SchTool::Label,
                    "Net label",
                    "Click a wire or pin to name its net (N)",
                ),
                (
                    SchTool::Power,
                    "Power port",
                    "Click a pin or wire end to place a power port: GND, +3V3... (G; Space turns it)",
                ),
                (
                    SchTool::NoErc,
                    "No ERC",
                    "Click a pin or wire to mark it as left open on purpose: the ERC does not check it",
                ),
                (
                    SchTool::Port,
                    "Port",
                    "Click a wire, bus or signal harness end to place a port that carries it to other sheets",
                ),
            ] {
                if ui
                    .selectable_label(self.tool == t, name)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.tool = t;
                    self.wire_pts.clear();
                }
            }
        });
        ui.horizontal(|ui| {
            for (t, name, tip) in [
                (
                    SchTool::Bus,
                    "Bus",
                    "Draw a bus: click each corner, double-click or Esc to end (B). Name it with a net label like D[0..7]; its members join by their own labels D0..D7.",
                ),
                (
                    SchTool::BusEntry,
                    "Bus entry",
                    "Click a bus to place a 45 degree entry; Space turns it",
                ),
                (
                    SchTool::Harness,
                    "Harness",
                    "Place a harness connector: its type and entries below, click for its top-left corner",
                ),
                (
                    SchTool::SignalHarness,
                    "Signal harness",
                    "Draw a signal harness from a connector to a port, sheet entry or another connector (double-click or Esc ends)",
                ),
            ] {
                if ui
                    .selectable_label(self.tool == t, name)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.tool = t;
                    self.wire_pts.clear();
                }
            }
        });
        ui.checkbox(&mut self.move_breaks, "Move breaks wires")
            .on_hover_text("Off: dragging parts keeps their wires connected, rubber-banded (Altium's Drag). On: they move alone and the wires stay (Altium's Move). Ctrl at the start of a drag does the other one.");
        if self.tool == SchTool::Harness {
            if self.harness_kind.is_empty() {
                self.harness_kind = "SPI".into();
                self.harness_entries = "MOSI, MISO, SCK, CS".into();
            }
            ui.horizontal(|ui| {
                ui.label("Type");
                ui.add(egui::TextEdit::singleline(&mut self.harness_kind).desired_width(90.0));
                ui.checkbox(&mut self.harness_left, "Entries on the left");
            });
            ui.horizontal(|ui| {
                ui.label("Entries");
                ui.add(egui::TextEdit::singleline(&mut self.harness_entries).desired_width(200.0));
            });
        }
        if self.tool == SchTool::Port {
            if self.port_name.is_empty() {
                self.port_name = "PORT".into();
            }
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.port_name).desired_width(120.0));
            });
            ui.label(
                RichText::new("On a bus name it D[0..7]; on a signal harness its name names the harness (SPI1.MOSI).")
                    .weak()
                    .small(),
            );
        }
        if self.tool == SchTool::BusEntry {
            let way =
                ["up right", "up left", "down left", "down right"][self.entry_turn as usize % 4];
            ui.label(
                RichText::new(format!("Leans {way}: Space turns it."))
                    .weak()
                    .small(),
            );
        }
        if self.tool == SchTool::Power {
            if self.power_net.is_empty() {
                self.power_net = "GND".into();
                self.power_style = 4;
                self.power_orient = 3;
            }
            ui.horizontal(|ui| {
                for (net, style, orient) in [
                    ("GND", 4, 3),
                    ("AGND", 5, 3),
                    ("+3V3", 2, 1),
                    ("+5V", 2, 1),
                    ("VCC", 1, 1),
                ] {
                    if ui.selectable_label(self.power_net == net, net).clicked() {
                        self.power_net = net.into();
                        self.power_style = style;
                        self.power_orient = orient;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Net");
                ui.add(egui::TextEdit::singleline(&mut self.power_net).desired_width(90.0));
                let name = trace_altium::POWER_STYLES
                    .iter()
                    .find(|s| s.0 == self.power_style)
                    .map_or("Bar", |s| s.1);
                egui::ComboBox::from_id_salt("power-style")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for (k, n) in trace_altium::POWER_STYLES {
                            ui.selectable_value(&mut self.power_style, k, n);
                        }
                    });
            });
            let way = ["right", "up", "left", "down"][self.power_orient.rem_euclid(4) as usize];
            ui.label(
                RichText::new(format!("Points {way}: Space turns it."))
                    .weak()
                    .small(),
            );
        }
        // What is selected, and what can be done with it.
        if self.sel.len() > 1 {
            let s = &self.sel;
            let mut what = Vec::new();
            for (n, one, many) in [
                (s.parts.len(), "part", "parts"),
                (s.wires.len(), "wire", "wires"),
                (s.tags.len(), "label or port", "labels and ports"),
                (s.junctions.len(), "junction", "junctions"),
                (s.no_erc.len(), "No ERC marker", "No ERC markers"),
                (s.buses.len(), "bus", "buses"),
                (s.bus_entries.len(), "bus entry", "bus entries"),
                (s.harnesses.len(), "harness connector", "harness connectors"),
                (
                    s.signal_harnesses.len(),
                    "signal harness",
                    "signal harnesses",
                ),
            ] {
                if n > 0 {
                    what.push(format!("{n} {}", if n == 1 { one } else { many }));
                }
            }
            ui.label(RichText::new(format!("Selected: {}", what.join(", "))).strong());
            let mut kind = None;
            let mut del = false;
            let mut copy = false;
            ui.horizontal_wrapped(|ui| {
                if ui.button("Turn").on_hover_text("Space").clicked() {
                    kind = Some(Xf::Turn([0.0, 0.0]));
                }
                if ui
                    .button("Mirror X")
                    .on_hover_text("X: flip left to right")
                    .clicked()
                {
                    kind = Some(Xf::MirrorX(0.0));
                }
                if ui
                    .button("Mirror Y")
                    .on_hover_text("Y: flip top to bottom")
                    .clicked()
                {
                    kind = Some(Xf::MirrorY(0.0));
                }
                if ui
                    .button("Copy")
                    .on_hover_text("Ctrl+C; Ctrl+V pastes")
                    .clicked()
                {
                    copy = true;
                }
                if ui.button("Delete").on_hover_text("Delete").clicked() {
                    del = true;
                }
            });
            let cur = self.current;
            let sel = self.sel.clone();
            let centre = self
                .slots
                .get(cur)
                .and_then(|s| s.sheet.as_ref())
                .and_then(|s| s.drawing.sel_centre(&sel));
            if let (Some(k), Some(c)) = (kind, centre) {
                let xf = match k {
                    Xf::Turn(_) => Xf::Turn(c),
                    Xf::MirrorX(_) => Xf::MirrorX(c[0]),
                    _ => Xf::MirrorY(c[1]),
                };
                self.edit(|s| s.drawing.xf_sel(&sel, xf));
            }
            if copy {
                self.clip = self
                    .slots
                    .get(cur)
                    .and_then(|s| s.sheet.as_ref())
                    .map(|s| s.clip(&sel));
                self.note = Some("Copied: Ctrl+V pastes it here or on another sheet.".into());
            }
            if del {
                self.edit(|s| s.drawing.delete_sel(&sel));
                self.pick_sel(SchSel::default());
            }
        }
        // One bus: break it out into named members.
        if self.sel.len() == 1 && self.sel.buses.len() == 1 {
            let b = self.sel.buses[0];
            ui.label(RichText::new("Bus").strong());
            if self.breakout_name.is_empty() {
                self.breakout_name = "D[0..7]".into();
            }
            let mut go = false;
            ui.horizontal(|ui| {
                ui.label("Members");
                ui.add(egui::TextEdit::singleline(&mut self.breakout_name).desired_width(100.0));
                ui.checkbox(&mut self.breakout_flip, "Other side");
                go = ui
                    .button("Break out")
                    .on_hover_text("An entry, a wire and a net label per member, 100 mil apart from the bus's start, and a label naming the bus")
                    .clicked();
            });
            if go {
                match trace_core::bus_members(self.breakout_name.trim()) {
                    Some(m) => {
                        let (name, flip) =
                            (self.breakout_name.trim().to_string(), self.breakout_flip);
                        let cur = self.current;
                        let start = self
                            .slots
                            .get(cur)
                            .and_then(|s| s.sheet.as_ref())
                            .and_then(|s| s.drawing.buses.get(b))
                            .and_then(|bus| {
                                let (a, c) = (bus[0], bus[1]);
                                let l = (c[0] - a[0]).hypot(c[1] - a[1]);
                                (l > 0.0).then(|| {
                                    [
                                        a[0] + (c[0] - a[0]) / l * 100.0,
                                        a[1] + (c[1] - a[1]) / l * 100.0,
                                    ]
                                })
                            });
                        if let Some(start) = start {
                            let mut n = 0;
                            self.edit(|s| {
                                n = s.drawing.bus_breakout(b, &name, start, 100.0, 300.0, flip)
                            });
                            self.note = Some(format!("Broke out {n} of {} members.", m.len()));
                        }
                    }
                    None => self.note = Some("Write the members as a range, like D[0..7].".into()),
                }
            }
        }
        // One harness connector: its type and entries.
        if self.sel.len() == 1 && self.sel.harnesses.len() == 1 {
            let h = self.sel.harnesses[0];
            let cur = self.current;
            let now = self
                .slots
                .get(cur)
                .and_then(|s| s.sheet.as_ref())
                .and_then(|s| s.drawing.harnesses.get(h).cloned());
            if let Some(c) = now {
                ui.label(RichText::new("Harness connector").strong());
                if self.edit_for != Some((cur, usize::MAX - h)) {
                    self.edit_for = Some((cur, usize::MAX - h));
                    self.harness_kind = c.kind.clone();
                    self.harness_entries = c
                        .entries
                        .iter()
                        .map(|e| e.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                let mut apply = false;
                ui.horizontal(|ui| {
                    ui.label("Type");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.harness_kind).desired_width(90.0),
                    );
                    apply |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                ui.horizontal(|ui| {
                    ui.label("Entries");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.harness_entries).desired_width(200.0),
                    );
                    apply |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    apply |= ui.button("Apply").clicked();
                });
                if apply {
                    let names: Vec<String> = self
                        .harness_entries
                        .split([',', ';'])
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect();
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let top_left = [c.min[0], c.max[1]];
                    let mut n = trace_core::Harness::new(
                        top_left,
                        self.harness_kind.trim(),
                        &refs,
                        !c.entries_right(),
                    );
                    // Keep its size when the entries still fit.
                    if n.max[1] - n.min[1] <= c.max[1] - c.min[1] {
                        n.min[1] = c.min[1];
                        n.primary = c.primary;
                    }
                    n.max[0] = n.max[0].max(c.max[0]);
                    self.edit(|s| {
                        s.drawing.harnesses[h] = n;
                        s.drawing.connect();
                    });
                }
            }
        }
        if ui
            .button("Place symbol…")
            .on_hover_text("Place a new part from a symbol library (.SchLib or .IntLib) (P)")
            .clicked()
        {
            self.libs.open = true;
        }
        if ui
            .button("Symbol editor…")
            .on_hover_text("Draw and edit library symbols (.SchLib): body, pins, pin table, parts; then update the placed parts")
            .clicked()
        {
            self.symedit.open = true;
        }
        if let Some(at) = self.libs.window(ui.ctx()) {
            self.start_place(at, scene);
        }
        if let Some((path, name)) = self.libs.edit_request.take() {
            if let Err(e) = self.symedit.open_path(&path, Some(&name)) {
                self.note = Some(format!("Could not open {}: {e}", path.display()));
            }
        }
        ui.horizontal(|ui| {
            let save = ui
                .add_enabled(self.dirty, egui::Button::new("Save"))
                .on_hover_text("Save the sheets as one Trace schematic file next to the project (Ctrl+S)");
            if save.clicked() {
                self.note = Some(match self.save() {
                    Ok(p) => format!("Saved {}", p.display()),
                    Err(e) => e,
                });
            }
            if ui
                .button("Save as Altium…")
                .on_hover_text("Write each sheet as an Altium .SchDoc (with a copy of the project) into a folder you pick")
                .clicked()
            {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Folder for the Altium sheets")
                    .set_directory(self.project_dir())
                    .pick_folder()
                {
                    self.note = Some(match self.save_altium(&dir) {
                        Ok(n) => format!("Wrote {n} Altium sheets to {}", dir.display()),
                        Err(e) => e,
                    });
                }
            }
            if let Some(scene) = scene {
                if ui
                    .button("Update PCB…")
                    .on_hover_text("Carry the sheet edits to the board: pin nets, new nets, net and part names, new parts")
                    .clicked()
                {
                    let plan = self.plan(scene, library);
                    if plan.changes.is_empty() {
                        self.note = Some("The board already matches the sheets.".into());
                    } else {
                        self.eco = Some(plan);
                    }
                }
            }
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
            if ui
                .button("Annotate…")
                .on_hover_text("Number parts without a designator number (R?, U?A), or every part again, in sheet order")
                .clicked()
            {
                self.erc.annotate_open = true;
            }
            if ui
                .button("Check (ERC)")
                .on_hover_text("Electrical rule check: unconnected pins, one-pin nets, outputs driving each other, undriven inputs, duplicate designators, nets with two names, loose wire ends")
                .clicked()
            {
                self.run_erc();
            }
        });
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).weak().small());
        }
        ui.separator();
        let tree = self.hier_tree();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sheets").weak());
            if let Some((p, li)) = tree.parent.get(self.current).copied().flatten() {
                let name = self.slots[p].name.clone();
                if ui
                    .small_button(format!("⬆ {name}"))
                    .on_hover_text("Up to the sheet that places this one")
                    .clicked()
                {
                    let at = self.slots[p].sheet.as_ref().and_then(|s| {
                        let l = s.drawing.links.get(li)?;
                        Some([(l.min[0] + l.max[0]) / 2.0, (l.min[1] + l.max[1]) / 2.0])
                    });
                    match at {
                        Some(at) => self.go_to(p, at),
                        None => self.select_sheet(p),
                    }
                }
            }
        });
        let before = self.scope;
        ui.horizontal(|ui| {
            ui.label("Nets join");
            egui::ComboBox::from_id_salt("net_scope")
                .selected_text(self.scope.label())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for s in Scope::ALL {
                        ui.selectable_value(&mut self.scope, s, s.label());
                    }
                })
                .response
                .on_hover_text("How nets join across sheets (Altium's net identifier scope, read from the project). Automatic: hierarchical when there are sheet entries, flat when there are ports, global otherwise.");
        });
        if self.scope != before {
            self.dirty = true;
            self.erc.stale = true;
        }
        let mut go = None;
        egui::ScrollArea::vertical()
            .id_salt("sheets")
            .max_height(ui.available_height() * 0.6)
            .show(ui, |ui| {
                // Channels each sheet is placed as.
                let mut placed = vec![0usize; self.slots.len()];
                for (si, kids) in tree.child.iter().enumerate() {
                    let Some(sheet) = &self.slots[si].sheet else {
                        continue;
                    };
                    for (li, c) in kids.iter().enumerate() {
                        if let (Some(c), Some(l)) = (*c, sheet.drawing.links.get(li)) {
                            placed[c] += l.channels().len();
                        }
                    }
                }
                for (i, depth) in tree.order() {
                    let s = &self.slots[i];
                    let name = if placed[i] > 1 {
                        format!("{} ×{}", s.name, placed[i])
                    } else {
                        s.name.clone()
                    };
                    let mut text = RichText::new(name);
                    if self.highlight.get(i).is_some_and(|h| !h.is_empty()) {
                        text = text.color(HIGHLIGHT);
                    }
                    if s.error.is_some() {
                        text = text.strikethrough();
                    }
                    let r = ui
                        .horizontal(|ui| {
                            ui.add_space(depth as f32 * 14.0);
                            ui.selectable_label(i == self.current, text)
                        })
                        .inner;
                    let r = if placed[i] > 1 {
                        r.on_hover_text(format!(
                            "Placed {} times: each copy's parts get the channel's name (R1_CH1)",
                            placed[i]
                        ))
                    } else {
                        r
                    };
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
        let cur = self.current;
        // Name and value of the selected part, which can be changed.
        if let Some(pi) = self.sel_part {
            let found = self
                .slots
                .get(cur)
                .and_then(|s| s.sheet.as_ref())
                .and_then(|sheet| {
                    let p = sheet.drawing.parts.get(pi)?;
                    let sym = sheet.symbols.get(p.symbol)?;
                    Some((
                        p.designator.clone(),
                        sym.value.clone().unwrap_or_default(),
                        sym.library_ref.clone(),
                        sym.description.clone(),
                    ))
                });
            if let Some((name, value, lib, desc)) = found {
                if self.edit_for != Some((cur, pi)) {
                    self.edit_for = Some((cur, pi));
                    self.edit_name = name.clone();
                    self.edit_value = value.clone();
                }
                ui.label(RichText::new(&name).strong().size(16.0));
                let mut apply = false;
                egui::Grid::new("part_fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Designator");
                        let r = ui.text_edit_singleline(&mut self.edit_name);
                        apply |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                        ui.label("Value");
                        let r = ui.text_edit_singleline(&mut self.edit_value);
                        apply |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                    });
                let changed = self.edit_name.trim() != name || self.edit_value.trim() != value;
                let mut copy = false;
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(changed, egui::Button::new("Apply"))
                        .clicked()
                    {
                        apply = true;
                    }
                    copy = ui
                        .button("Copy part")
                        .on_hover_text("A copy beside it with the next free designator and the same footprint (Ctrl+D)")
                        .clicked();
                });
                if copy {
                    self.copy_part(pi, scene);
                }
                if apply && changed {
                    let (new_name, new_value) = (
                        self.edit_name.trim().to_string(),
                        self.edit_value.trim().to_string(),
                    );
                    let taken = new_name != name
                        && self.slots.iter().filter_map(|s| s.sheet.as_ref()).any(|s| {
                            s.drawing
                                .parts
                                .iter()
                                .any(|p| p.designator.eq_ignore_ascii_case(&new_name))
                        });
                    if new_name.is_empty() || taken {
                        self.note = Some(format!(
                            "{new_name} is already used; pick another designator."
                        ));
                    } else {
                        if new_name != name {
                            // Every unit of a multi-part component shares the name.
                            let was = self
                                .renames
                                .remove(&name.to_ascii_uppercase())
                                .unwrap_or_else(|| name.to_ascii_uppercase());
                            self.renames.insert(new_name.to_ascii_uppercase(), was);
                            let first = self
                                .original_renames
                                .remove(&name.to_ascii_uppercase())
                                .unwrap_or_else(|| name.to_ascii_uppercase());
                            self.original_renames
                                .insert(new_name.to_ascii_uppercase(), first);
                        }
                        let (n2, v2, old) = (new_name.clone(), new_value.clone(), name.clone());
                        self.edit(move |sheet| {
                            let units: Vec<usize> = sheet
                                .drawing
                                .parts
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| p.designator == old)
                                .map(|(i, _)| i)
                                .collect();
                            for i in units {
                                if n2 != old {
                                    sheet.drawing.rename_part(i, &n2);
                                }
                                sheet.set_value(i, &v2);
                            }
                        });
                        self.edit_for = None;
                        self.note = Some(
                            "Changed on the sheet. Update PCB carries it to the board.".into(),
                        );
                    }
                }
                ui.label(RichText::new(&lib).weak());
                if !desc.is_empty() {
                    ui.label(RichText::new(&desc).weak().small());
                }
            }
        } else if let Some(t) = self.sel_tag {
            let found = self
                .slots
                .get(cur)
                .and_then(|s| s.sheet.as_ref())
                .and_then(|s| s.drawing.tags.get(t))
                .map(|t| (t.text.clone(), t.kind));
            if let Some((text, kind)) = found {
                ui.label(RichText::new(format!("{kind:?} {text}")).strong());
                if kind == trace_core::TagKind::Label {
                    if self.edit_for != Some((cur, usize::MAX - t)) {
                        self.edit_for = Some((cur, usize::MAX - t));
                        self.edit_tag = text.clone();
                    }
                    let r = ui.text_edit_singleline(&mut self.edit_tag);
                    if r.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.edit_tag.trim().is_empty()
                        && self.edit_tag.trim() != text
                    {
                        let new = self.edit_tag.trim().to_string();
                        self.edit(move |s| s.drawing.rename_tag(t, &new));
                        self.edit_for = None;
                    }
                    ui.label(
                        RichText::new("Enter renames the label. Delete removes it.")
                            .weak()
                            .small(),
                    );
                }
            }
        }
        if let Some(sheet) = self.slots.get(self.current).and_then(|s| s.sheet.as_ref()) {
            if self.sel_part.is_some() || self.sel_tag.is_some() {
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
        // With the Select tool, pressing on something drags it (with the
        // rest of the selection when it is selected), and pressing on the
        // empty sheet draws a selection box: left to right picks what is
        // wholly inside, right to left what it touches. The right or
        // middle button pans, and so does the left with the other tools.
        let primary = resp.drag_started_by(egui::PointerButton::Primary);
        if primary && self.tool == SchTool::Select && !self.floating {
            let origin = ui.input(|i| i.pointer.press_origin());
            if let Some(o) = origin {
                let b = self.view.to_board(o);
                let p = [b[0] as f64, b[1] as f64];
                let tol = (5.0 / self.view.scale as f64).max(8.0);
                let hit = hit_sel(&sheet.drawing, &self.slots[cur].boxes, p, tol);
                let ctrl = ui.input(|i| i.modifiers.command);
                self.drag_keeps = self.move_breaks == ctrl;
                // A wire on its own drags by the segment (or corner) taken
                // hold of, the rest of the wiring staying connected.
                let lone_wire = hit.wires.len() == 1
                    && hit.len() == 1
                    && (self.sel.is_empty() || self.sel == hit);
                if hit.is_empty() {
                    self.boxing = Some(p);
                } else if lone_wire {
                    let w = hit.wires[0];
                    let pts = &sheet.drawing.wires[w];
                    let corner = pts
                        .iter()
                        .position(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < tol * 1.5);
                    let seg = pts
                        .windows(2)
                        .enumerate()
                        .min_by(|a, b| {
                            seg_dist(p, a.1[0], a.1[1]).total_cmp(&seg_dist(p, b.1[0], b.1[1]))
                        })
                        .map(|(k, _)| k)
                        .unwrap_or(0);
                    set_sel!(self, hit);
                    self.undo.push(vec![(cur, sheet.clone())]);
                    self.redo.clear();
                    self.wire_drag = Some(match corner {
                        Some(k) => (w, k, true, snap(p, GRID)),
                        None => (w, seg, false, snap(p, GRID)),
                    });
                    self.drag = Some(snap(p, GRID));
                    self.sel_net = None;
                } else {
                    if !self.sel.holds(&hit) {
                        set_sel!(self, hit);
                    }
                    self.undo.push(vec![(cur, sheet.clone())]);
                    self.redo.clear();
                    self.drag = Some(snap(p, GRID));
                    self.sel_net = None;
                }
            }
        }
        let pan = resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
            || (resp.dragged_by(egui::PointerButton::Primary)
                && self.drag.is_none()
                && self.boxing.is_none()
                && self.tool != SchTool::Select);
        if pan {
            let d = resp.drag_delta();
            self.view.tx += d.x;
            self.view.ty += d.y;
        }
        if !resp.dragged() && !resp.drag_started() && !self.floating {
            self.drag = None;
            self.wire_drag = None;
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
        for p in self.sel.parts.iter().filter_map(|&i| d.parts.get(i)) {
            let r = Rect::from_two_pos(
                view.to_screen(p.min[0] as f32 - 30.0, p.min[1] as f32 - 30.0),
                view.to_screen(p.max[0] as f32 + 30.0, p.max[1] as f32 + 30.0),
            );
            shapes.push(Shape::rect_filled(r, 2.0, HIGHLIGHT.gamma_multiply(0.25)));
        }
        // A selected power port's symbol.
        for &t in &self.sel.tags {
            let mut b: Option<[f32; 4]> = None;
            for (si, sh) in d.shapes.iter().enumerate() {
                if sh.tag == Some(t as u32) {
                    let x = slot.boxes[si];
                    b = Some(b.map_or(x, |o| {
                        [
                            o[0].min(x[0]),
                            o[1].min(x[1]),
                            o[2].max(x[2]),
                            o[3].max(x[3]),
                        ]
                    }));
                }
            }
            if let Some(x) = b {
                let r = Rect::from_two_pos(
                    view.to_screen(x[0] - 20.0, x[1] - 20.0),
                    view.to_screen(x[2] + 20.0, x[3] + 20.0),
                );
                shapes.push(Shape::rect_filled(r, 2.0, HIGHLIGHT.gamma_multiply(0.25)));
            }
        }
        let px = |w: f64| ((w as f32) * view.scale).max(1.0);
        // Buses, harnesses and their kin are drawn from their lists.
        let (xs, xt) = d.bus_harness_graphics();
        let all_shapes = d
            .shapes
            .iter()
            .enumerate()
            .map(|(i, s)| (slot.boxes.get(i).copied(), s))
            .chain(xs.iter().map(|s| (None, s)));
        for (bx, sh) in all_shapes {
            let bx = bx.unwrap_or_else(|| shape_box(&sh.geom, sh.width));
            if !overlaps(bx, vis) {
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
        for t in d.texts.iter().chain(xt.iter()) {
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

        // Selected wire and label, the wire being drawn.
        let sel_stroke = Stroke::new((30.0 * view.scale).max(4.0), HIGHLIGHT.gamma_multiply(0.5));
        for w in self.sel.wires.iter().filter_map(|&w| d.wires.get(w)) {
            let pts: Vec<Pos2> = w
                .iter()
                .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                .collect();
            painter.add(Shape::line(pts, sel_stroke));
        }
        let ring = |a: &[f64; 2]| {
            painter.circle_stroke(
                view.to_screen(a[0] as f32, a[1] as f32),
                (40.0 * view.scale).max(6.0),
                sel_stroke,
            );
        };
        for t in self.sel.tags.iter().filter_map(|&t| d.tags.get(t)) {
            t.at.iter().for_each(ring);
        }
        self.sel
            .junctions
            .iter()
            .filter_map(|&j| d.junctions.get(j))
            .for_each(ring);
        self.sel
            .no_erc
            .iter()
            .filter_map(|&j| d.no_erc.get(j))
            .for_each(ring);
        let sel_line = |pts: &[[f64; 2]]| {
            let pts: Vec<Pos2> = pts
                .iter()
                .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                .collect();
            painter.add(Shape::line(pts, sel_stroke));
        };
        for b in self.sel.buses.iter().filter_map(|&b| d.buses.get(b)) {
            sel_line(b);
        }
        for e in self
            .sel
            .bus_entries
            .iter()
            .filter_map(|&b| d.bus_entries.get(b))
        {
            sel_line(e);
        }
        for b in self
            .sel
            .signal_harnesses
            .iter()
            .filter_map(|&b| d.signal_harnesses.get(b))
        {
            sel_line(b);
        }
        for h in self
            .sel
            .harnesses
            .iter()
            .filter_map(|&h| d.harnesses.get(h))
        {
            let r = Rect::from_two_pos(
                view.to_screen(h.min[0] as f32 - 30.0, h.min[1] as f32 - 30.0),
                view.to_screen(h.max[0] as f32 + 30.0, h.max[1] as f32 + 30.0),
            );
            painter.rect_filled(r, 2.0, HIGHLIGHT.gamma_multiply(0.25));
        }
        // The selection box being drawn.
        if let (Some(a), Some(b)) = (
            self.boxing,
            resp.hover_pos().or(ui.input(|i| i.pointer.latest_pos())),
        ) {
            let pa = view.to_screen(a[0] as f32, a[1] as f32);
            let r = Rect::from_two_pos(pa, b);
            let touching = b.x < pa.x;
            let c = if touching {
                Color32::from_rgb(90, 200, 120)
            } else {
                Color32::from_rgb(90, 150, 255)
            };
            painter.rect_filled(r, 0.0, c.gamma_multiply(0.12));
            if touching {
                let pts = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ];
                painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0_f32, c), 6.0, 4.0));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, c));
            }
        }
        let pointer = resp.hover_pos().map(|p| view.to_board(p));
        // Pin ends and wire ends close to the pointer win over the grid, so a
        // wire lands on them even when they sit off the grid.
        let catch = (10.0 / view.scale as f64).max(1.0);
        let snapped = pointer.map(|p| {
            let p = [p[0] as f64, p[1] as f64];
            d.pins
                .iter()
                .map(|pin| pin.at)
                .chain(d.wires.iter().flat_map(|w| [w[0], w[w.len() - 1]]))
                .chain(d.harnesses.iter().flat_map(|h| {
                    h.entries
                        .iter()
                        .map(|e| h.entry_at(e))
                        .chain([h.bundle_at()])
                        .collect::<Vec<_>>()
                }))
                .chain(
                    d.signal_harnesses
                        .iter()
                        .flat_map(|w| [w[0], w[w.len() - 1]]),
                )
                .chain(d.bus_entries.iter().flat_map(|e| [e[0], e[1]]))
                .map(|q| (q, (q[0] - p[0]).hypot(q[1] - p[1])))
                .filter(|(_, dist)| *dist < catch)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(q, _)| q)
                .or_else(|| {
                    if matches!(self.tool, SchTool::BusEntry | SchTool::Bus | SchTool::Label) {
                        on_line_near(&d.buses, p, catch.max(30.0))
                    } else {
                        None
                    }
                })
                .or_else(|| on_wire_near(d, p, catch))
                .or_else(|| on_line_near(&d.signal_harnesses, p, catch))
                // On a pin's line: its free end, where things connect.
                .or_else(|| {
                    d.pins
                        .iter()
                        .find(|pin| seg_dist(p, pin.at, pin.base) < catch)
                        .map(|pin| pin.at)
                })
                .unwrap_or_else(|| snap(p, GRID))
        });
        let on_pin = snapped.is_some_and(|c| d.pins.iter().any(|pin| pin.at == c));
        let wire_color = rgb(WIRE_COLOR);
        if matches!(
            self.tool,
            SchTool::Wire | SchTool::Bus | SchTool::SignalHarness
        ) {
            let (w, col) = match self.tool {
                SchTool::Bus => (30.0, wire_color),
                SchTool::SignalHarness => (40.0, rgb(HARNESS_LINE)),
                _ => (10.0, wire_color),
            };
            if let (Some(&last), Some(c)) = (self.wire_pts.last(), snapped) {
                let mut pts = self.wire_pts.clone();
                pts.push([c[0], last[1]]);
                pts.push(c);
                let pts: Vec<Pos2> = pts
                    .iter()
                    .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(px(w).max(2.0), col)));
            }
        }
        if self.tool == SchTool::BusEntry {
            if let Some(c) = snapped {
                let (dx, dy) = [
                    (100.0, 100.0),
                    (-100.0, 100.0),
                    (-100.0, -100.0),
                    (100.0, -100.0),
                ][self.entry_turn as usize % 4];
                painter.line_segment(
                    [
                        view.to_screen(c[0] as f32, c[1] as f32),
                        view.to_screen((c[0] + dx) as f32, (c[1] + dy) as f32),
                    ],
                    Stroke::new(px(10.0).max(2.0), wire_color.gamma_multiply(0.7)),
                );
            }
        }
        if self.tool == SchTool::Harness {
            if let Some(c) = snapped {
                let names: Vec<String> = self
                    .harness_entries
                    .split([',', ';'])
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let mut g = Drawing::default();
                g.harnesses.push(trace_core::Harness::new(
                    c,
                    &self.harness_kind,
                    &refs,
                    self.harness_left,
                ));
                let (sh, _) = g.bus_harness_graphics();
                paint_ghost(
                    &painter,
                    view,
                    &Drawing {
                        shapes: sh,
                        ..Default::default()
                    },
                    rgb(HARNESS_LINE),
                );
            }
        }
        if self.tool == SchTool::Port {
            if let Some(c) = snapped {
                let g = trace_altium::port_drawing(c, self.port_name.trim(), 500.0);
                paint_ghost(&painter, view, &g, rgb([128, 0, 0]));
            }
        }
        if self.tool == SchTool::Power {
            if let Some(c) = snapped {
                let g = trace_altium::power_port_drawing(
                    c,
                    &self.power_net,
                    self.power_style,
                    self.power_orient,
                );
                paint_ghost(&painter, view, &g, rgb([128, 0, 0]));
            }
        }
        if self.tool == SchTool::NoErc {
            if let Some(c) = snapped {
                let g = trace_core::sch_select::no_erc_drawing(c);
                paint_ghost(&painter, view, &g, Color32::RED);
            }
        }
        if self.tool != SchTool::Select {
            if let Some(c) = snapped {
                let at = view.to_screen(c[0] as f32, c[1] as f32);
                let r = 6.0;
                if on_pin {
                    painter.circle_stroke(at, 5.0, Stroke::new(1.5_f32, HIGHLIGHT));
                }
                painter.line_segment(
                    [at - egui::vec2(r, 0.0), at + egui::vec2(r, 0.0)],
                    Stroke::new(1.0_f32, wire_color),
                );
                painter.line_segment(
                    [at - egui::vec2(0.0, r), at + egui::vec2(0.0, r)],
                    Stroke::new(1.0_f32, wire_color),
                );
            }
        }

        // Hover and pick.
        self.hover = None;
        let mut pick = None;
        let mut act: Option<Act> = None;
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
            // A port is hit anywhere along its body, between its two ends.
            let hit_tag = d.tags.iter().position(|t| {
                t.at.iter()
                    .any(|&a| ((a[0] - p[0]).powi(2) + (a[1] - p[1]).powi(2)).sqrt() < tol * 2.0)
                    || (t.kind == TagKind::Port
                        && t.at.len() == 2
                        && seg_dist(p, t.at[0], t.at[1]) < tol * 2.0)
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
            let hit_part = part_at(d, p);
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
            } else if let Some(t) =
                hit_tag.filter(|&t| matches!(d.tags[t].kind, TagKind::Port | TagKind::SheetEntry))
            {
                Some(format!(
                    "{} {} (double-click for the other end)",
                    if d.tags[t].kind == TagKind::Port {
                        "Port"
                    } else {
                        "Sheet entry"
                    },
                    d.tags[t].text
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
            let c = snapped.unwrap_or_else(|| snap(p, GRID));
            match self.tool {
                SchTool::Wire => {
                    // Ending on a pin or another wire finishes the wire.
                    let joins = d.pins.iter().any(|pin| pin.at == c)
                        || d.wires
                            .iter()
                            .any(|w| w.windows(2).any(|s| seg_dist(c, s[0], s[1]) < 0.6));
                    if resp.double_clicked() {
                        if self.wire_pts.len() >= 2 {
                            act = Some(Act::Wire(std::mem::take(&mut self.wire_pts)));
                        }
                    } else if resp.clicked() {
                        match self.wire_pts.last().copied() {
                            None => self.wire_pts.push(c),
                            Some(last) => {
                                let corner = [c[0], last[1]];
                                if corner != last && corner != c {
                                    self.wire_pts.push(corner);
                                }
                                if c != last {
                                    self.wire_pts.push(c);
                                }
                                if joins && self.wire_pts.len() >= 2 {
                                    act = Some(Act::Wire(std::mem::take(&mut self.wire_pts)));
                                }
                            }
                        }
                    }
                }
                SchTool::Bus | SchTool::SignalHarness => {
                    if resp.double_clicked() {
                        if self.wire_pts.len() >= 2 {
                            let pts = std::mem::take(&mut self.wire_pts);
                            act = Some(if self.tool == SchTool::Bus {
                                Act::Bus(pts)
                            } else {
                                Act::SignalHarness(pts)
                            });
                        }
                    } else if resp.clicked() {
                        match self.wire_pts.last().copied() {
                            None => self.wire_pts.push(c),
                            Some(last) => {
                                let corner = [c[0], last[1]];
                                if corner != last && corner != c {
                                    self.wire_pts.push(corner);
                                }
                                if c != last {
                                    self.wire_pts.push(c);
                                }
                            }
                        }
                    }
                }
                SchTool::BusEntry => {
                    if resp.clicked() {
                        let on_bus = d
                            .buses
                            .iter()
                            .any(|b| b.windows(2).any(|s| seg_dist(c, s[0], s[1]) < 0.6));
                        if on_bus {
                            act = Some(Act::BusEntry(c));
                        } else {
                            self.note = Some("Click on a bus to place an entry.".into());
                        }
                    }
                }
                SchTool::Harness => {
                    if resp.clicked() {
                        act = Some(Act::Harness(c));
                    }
                }
                SchTool::Port => {
                    if resp.clicked() {
                        act = Some(Act::Port(c));
                    }
                }
                SchTool::Label => {
                    if resp.clicked() {
                        self.label_at = Some(c);
                        self.label_text.clear();
                    }
                }
                SchTool::Power => {
                    if resp.clicked() {
                        act = Some(Act::Power(c));
                    }
                }
                SchTool::NoErc => {
                    if resp.clicked() {
                        act = Some(Act::NoErc(c));
                    }
                }
                SchTool::Select => {
                    if let Some((w, k, corner, from)) = self.wire_drag {
                        let g = snap(p, GRID);
                        if Some(g) != self.drag {
                            act = Some(Act::DragWire(
                                w,
                                k,
                                corner,
                                [g[0] - from[0], g[1] - from[1]],
                            ));
                            self.drag = Some(g);
                        }
                    } else if let Some(last) = self.drag {
                        let g = snap(p, GRID);
                        if g != last {
                            act = Some(Act::Move([g[0] - last[0], g[1] - last[1]]));
                            self.drag = Some(g);
                        }
                    }
                    if self.boxing.is_some() && !resp.dragged() {
                        let a = self.boxing.take().unwrap_or(p);
                        let touching = p[0] < a[0];
                        let mut found = d.select_box(a, p, touching);
                        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
                        if shift || ctrl {
                            let mut all = self.sel.clone();
                            all.add(&found);
                            found = all;
                        }
                        let n = found.len();
                        set_sel!(self, found);
                        self.sel_net = None;
                        if n > 1 {
                            self.note = Some(format!(
                                "{n} selected: drag to move, Space turns, X / Y mirror, Delete, Ctrl+C copies."
                            ));
                        }
                    }
                    if self.floating {
                        if resp.clicked() || resp.drag_started() {
                            self.floating = false;
                            self.drag = None;
                            self.note = Some(
                                "Placed. Wire it up, then Update PCB adds it to the board.".into(),
                            );
                        }
                    } else if resp.double_clicked() {
                        // A sheet entry leads down to its port, a port up
                        // to its sheet entry, a sheet symbol into its sheet.
                        let hop = hit_tag
                            .filter(|&t| {
                                matches!(d.tags[t].kind, TagKind::Port | TagKind::SheetEntry)
                            })
                            .and_then(|t| self.other_end(cur, t));
                        if let Some((s, at)) = hop {
                            self.go_to(s, at);
                            return None;
                        }
                        if let Some(i) = hit_link {
                            let want = hier::sheet_key(&d.links[i].file);
                            let target = self
                                .slots
                                .iter()
                                .position(|s| hier::sheet_key(&s.name) == want);
                            if let Some(t) = target {
                                self.select_sheet(t);
                                return None;
                            }
                        }
                    } else if resp.clicked()
                        && ui.input(|i| i.modifiers.shift || i.modifiers.command)
                    {
                        // Shift or Ctrl+click adds to the selection or takes away.
                        let hit = hit_sel(d, &slot.boxes, p, tol);
                        let mut all = self.sel.clone();
                        all.toggle(&hit);
                        set_sel!(self, all);
                        self.sel_net = None;
                    } else if resp.clicked() {
                        let hit = hit_sel(d, &slot.boxes, p, tol);
                        let nothing = hit.is_empty();
                        set_sel!(self, hit);
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
                            set_sel!(self, SchSel::part(i));
                            self.sel_net = None;
                            let des = self.slots[cur].sheet.as_ref().unwrap().drawing.parts[i]
                                .designator
                                .clone();
                            pick = Some(Pick::Part(des));
                        } else {
                            self.sel_net = None;
                            if nothing {
                                set_sel!(self, SchSel::default());
                            }
                        }
                    }
                }
            }
        }

        // Keys, while the pointer is over the sheet and no text is typed.
        let typing = ui.ctx().wants_keyboard_input();
        if resp.hovered() && !typing && act.is_none() {
            let dup = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D));
            if dup && self.tool == SchTool::Select {
                act = match self.sel_part {
                    Some(i) if self.sel.len() == 1 => Some(Act::CopyPart(i)),
                    _ if !self.sel.is_empty() => Some(Act::Duplicate),
                    _ => None,
                };
            }
            // Ctrl+C, X and V come as clipboard events on most systems.
            let (copy, cut, paste, all, mx, my, g) = ui.input_mut(|i| {
                let mut out = (false, false, false);
                i.events.retain(|e| match e {
                    egui::Event::Copy => {
                        out.0 = true;
                        false
                    }
                    egui::Event::Cut => {
                        out.1 = true;
                        false
                    }
                    egui::Event::Paste(_) => {
                        out.2 = true;
                        false
                    }
                    _ => true,
                });
                (
                    out.0 || i.consume_key(egui::Modifiers::COMMAND, egui::Key::C),
                    out.1 || i.consume_key(egui::Modifiers::COMMAND, egui::Key::X),
                    out.2 || i.consume_key(egui::Modifiers::COMMAND, egui::Key::V),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::X),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Y),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::G),
                )
            });
            let has = !self.sel.is_empty();
            if copy && has {
                act = Some(Act::Copy);
            }
            if cut && has && !self.floating {
                act = Some(Act::Cut);
            }
            if paste && !self.floating {
                act = Some(Act::Paste);
            }
            if all && !self.floating {
                act = Some(Act::SelectAll);
            }
            if mx && has {
                act = Some(Act::Xf(XfKind::MirrorX));
            }
            if my && has {
                act = Some(Act::Xf(XfKind::MirrorY));
            }
            if g {
                self.tool = SchTool::Power;
                self.wire_pts.clear();
            }
            let (w, n, esc, space, del, undo, redo, save) = ui.input_mut(|i| {
                (
                    i.key_pressed(egui::Key::W),
                    i.key_pressed(egui::Key::N),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                        || i.consume_key(
                            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                            egui::Key::Z,
                        ),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                )
            });
            if w {
                self.tool = SchTool::Wire;
                self.wire_pts.clear();
            }
            if n {
                self.tool = SchTool::Label;
            }
            if ui.input(|i| i.key_pressed(egui::Key::B)) {
                self.tool = SchTool::Bus;
                self.wire_pts.clear();
            }
            if ui.input(|i| i.key_pressed(egui::Key::P)) {
                self.libs.open = true;
            }
            if esc && self.floating {
                act = Some(Act::CancelPlace);
            } else if esc && self.tool == SchTool::Select && self.boxing.is_none() {
                set_sel!(self, SchSel::default());
                self.sel_net = None;
            } else if esc {
                self.boxing = None;
                if self.wire_pts.len() >= 2 {
                    let pts = std::mem::take(&mut self.wire_pts);
                    act = Some(match self.tool {
                        SchTool::Bus => Act::Bus(pts),
                        SchTool::SignalHarness => Act::SignalHarness(pts),
                        _ => Act::Wire(pts),
                    });
                } else {
                    self.wire_pts.clear();
                    self.tool = SchTool::Select;
                }
            }
            if space {
                if self.tool == SchTool::BusEntry {
                    self.entry_turn = (self.entry_turn + 1) % 4;
                } else if self.tool == SchTool::Power {
                    self.power_orient = (self.power_orient + 1).rem_euclid(4);
                } else if !self.sel.is_empty() {
                    act = Some(Act::Xf(XfKind::Turn));
                }
            }
            if del && self.tool == SchTool::Select && !self.sel.is_empty() && !self.floating {
                act = Some(Act::Delete);
            }
            if undo {
                act = Some(Act::Undo);
            }
            if redo {
                act = Some(Act::Redo);
            }
            if save {
                act = Some(Act::Save);
            }
        }

        self.draw_erc(&painter, view);
        // The name for a new net label.
        if let Some(at) = self.label_at {
            let mut open = true;
            let mut done = false;
            egui::Window::new("Net label")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .fixed_pos(view.to_screen(at[0] as f32, at[1] as f32) + egui::vec2(16.0, 16.0))
                .show(ui.ctx(), |ui| {
                    let r = ui.text_edit_singleline(&mut self.label_text);
                    // Asked after reading the response: focusing first would
                    // hide the Enter that just let go of it.
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = true;
                    } else {
                        r.request_focus();
                    }
                    ui.label(
                        RichText::new("Enter places it, Esc cancels.")
                            .weak()
                            .small(),
                    );
                });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
            if done && !self.label_text.trim().is_empty() {
                act = Some(Act::Label(at, self.label_text.trim().to_string()));
                self.label_at = None;
            } else if !open {
                self.label_at = None;
            }
        }

        // Changes waiting to go to the board.
        if let Some(plan) = &self.eco {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            egui::Window::new("Update PCB from schematic")
                .collapsible(false)
                .default_width(460.0)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(match plan.changes.len() {
                        1 => "1 change:".to_string(),
                        n => format!("{n} changes:"),
                    });
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for c in &plan.changes {
                            let text = c.describe();
                            match c {
                                Change::NotOnBoard { .. } | Change::Removed { .. } => {
                                    ui.label(RichText::new(text).color(Color32::from_rgb(240, 180, 90)))
                                }
                                Change::AddPart { .. } => {
                                    ui.label(RichText::new(text).color(Color32::from_rgb(120, 200, 120)))
                                }
                                _ => ui.label(text),
                            };
                        }
                    });
                    ui.separator();
                    ui.label(
                        RichText::new(
                            "Tracks keep their nets: where a pad changes net, the DRC shows what to reroute.",
                        )
                        .weak()
                        .small(),
                    );
                    if plan.changes.iter().any(|c| matches!(c, Change::AddPart { .. })) {
                        ui.label(
                            RichText::new(
                                "New parts go to the right of the board with their nets: drag them into place.",
                            )
                            .weak()
                            .small(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!plan.edits.is_empty(), egui::Button::new("Apply to board"))
                            .clicked()
                        {
                            apply = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if apply {
                let edits = self.eco.take().map(|p| p.edits).unwrap_or_default();
                if !edits.is_empty() {
                    pick = Some(Pick::Eco(edits));
                    self.synced_now();
                    self.note = Some("Board updated. Ctrl+Z on the board takes it back.".into());
                }
            } else if !open || cancel {
                self.eco = None;
            }
        }

        // Make the edit, now the drawing is no longer borrowed.
        match act {
            Some(Act::Move(dv)) => {
                // One undo step covers the whole drag.
                let sel = self.sel.clone();
                let keep = self.drag_keeps || self.floating;
                if let Some(s) = self.slots.get_mut(cur).and_then(|s| s.sheet.as_mut()) {
                    s.drawing.xf_sel_mode(&sel, Xf::Shift(dv), keep);
                }
                self.after_edit(cur);
            }
            Some(Act::DragWire(w, k, corner, dv)) => {
                // Worked again from the sheet as the drag found it, so a
                // leg added on the way is not added twice.
                let start = self
                    .undo
                    .last()
                    .and_then(|g| g.iter().find(|(i, _)| *i == cur))
                    .map(|(_, s)| s.clone());
                if let (Some(mut s), Some(slot)) = (start, self.slots.get_mut(cur)) {
                    if corner {
                        s.drawing.drag_vertex(w, k, dv);
                    } else {
                        s.drawing.drag_segment(w, k, dv);
                    }
                    slot.sheet = Some(s);
                }
                self.after_edit(cur);
            }
            Some(Act::Bus(pts)) => {
                self.edit(|s| {
                    s.drawing.add_bus(pts);
                });
            }
            Some(Act::SignalHarness(pts)) => {
                self.edit(|s| {
                    s.drawing.add_signal_harness(pts);
                });
            }
            Some(Act::BusEntry(at)) => {
                let (dx, dy) = [
                    (100.0, 100.0),
                    (-100.0, 100.0),
                    (-100.0, -100.0),
                    (100.0, -100.0),
                ][self.entry_turn as usize % 4];
                self.edit(|s| {
                    s.drawing.add_bus_entry(at, [at[0] + dx, at[1] + dy]);
                });
            }
            Some(Act::Harness(at)) => {
                let names: Vec<String> = self
                    .harness_entries
                    .split([',', ';'])
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let h = trace_core::Harness::new(
                    at,
                    self.harness_kind.trim(),
                    &refs,
                    self.harness_left,
                );
                self.edit(|s| {
                    s.drawing.add_harness(h);
                });
                self.note = Some(
                    "Placed. Draw a signal harness from its open side to a port or sheet entry."
                        .into(),
                );
            }
            Some(Act::Port(at)) => {
                let name = self.port_name.trim().to_string();
                if !name.is_empty() {
                    let g = trace_altium::port_drawing(at, &name, 500.0);
                    self.edit(|s| {
                        s.drawing.add_drawn_tag(&g);
                    });
                }
            }
            Some(Act::Xf(kind)) => {
                let sel = self.sel.clone();
                let centre = self
                    .slots
                    .get(cur)
                    .and_then(|s| s.sheet.as_ref())
                    .and_then(|s| s.drawing.sel_centre(&sel));
                if let Some(c) = centre {
                    let xf = match kind {
                        XfKind::Turn => Xf::Turn(c),
                        XfKind::MirrorX => Xf::MirrorX(c[0]),
                        XfKind::MirrorY => Xf::MirrorY(c[1]),
                    };
                    if self.drag.is_some() {
                        // Part of the drag's undo step.
                        if let Some(s) = self.slots.get_mut(cur).and_then(|s| s.sheet.as_mut()) {
                            s.drawing.xf_sel(&sel, xf);
                        }
                        self.after_edit(cur);
                    } else {
                        self.edit(|s| s.drawing.xf_sel(&sel, xf));
                    }
                }
            }
            Some(Act::Delete) => {
                let sel = self.sel.clone();
                let n = sel.len();
                self.edit(|s| s.drawing.delete_sel(&sel));
                set_sel!(self, SchSel::default());
                if n > 1 {
                    self.note = Some(format!("Deleted {n} objects. Ctrl+Z puts them back."));
                }
            }
            Some(Act::CopyPart(i)) => self.copy_part(i, scene),
            Some(Act::Copy) | Some(Act::Cut) | Some(Act::Duplicate) => {
                let sel = self.sel.clone();
                let clip = self
                    .slots
                    .get(cur)
                    .and_then(|s| s.sheet.as_ref())
                    .map(|s| s.clip(&sel));
                if let Some(clip) = clip {
                    let n = sel.len();
                    self.clip = Some(clip);
                    match act {
                        Some(Act::Cut) => {
                            self.edit(|s| s.drawing.delete_sel(&sel));
                            set_sel!(self, SchSel::default());
                            self.note = Some(format!("Cut {n} objects: Ctrl+V pastes them."));
                        }
                        Some(Act::Duplicate) => {
                            let at = pointer
                                .map(|p| [p[0] as f64, p[1] as f64])
                                .unwrap_or([0.0, 0.0]);
                            self.paste(at, scene);
                        }
                        _ => {
                            self.note = Some(format!(
                                "Copied {n} objects: Ctrl+V pastes them here or on another sheet."
                            ));
                        }
                    }
                }
            }
            Some(Act::Paste) => {
                let at = pointer
                    .map(|p| [p[0] as f64, p[1] as f64])
                    .unwrap_or_else(|| {
                        let c = self.view.to_board(self.view.rect.center());
                        [c[0] as f64, c[1] as f64]
                    });
                self.paste(at, scene);
            }
            Some(Act::SelectAll) => {
                let all = self
                    .slots
                    .get(cur)
                    .and_then(|s| s.sheet.as_ref())
                    .map(|s| s.drawing.select_all())
                    .unwrap_or_default();
                set_sel!(self, all);
                self.tool = SchTool::Select;
            }
            Some(Act::Power(at)) => {
                if self.power_net.trim().is_empty() {
                    self.power_net = "GND".into();
                }
                let g = trace_altium::power_port_drawing(
                    at,
                    self.power_net.trim(),
                    self.power_style,
                    self.power_orient,
                );
                let mut made = None;
                self.edit(|s| made = s.drawing.add_drawn_tag(&g));
                if let Some(t) = made {
                    set_sel!(self, SchSel::tag(t));
                }
            }
            Some(Act::NoErc(at)) => {
                self.edit(|s| {
                    s.drawing.add_no_erc(at);
                });
            }
            Some(Act::Wire(pts)) => {
                self.edit(|s| {
                    s.drawing.add_wire(pts);
                });
            }
            Some(Act::Label(at, text)) => {
                self.edit(|s| {
                    s.drawing.add_label(at, &text);
                });
            }
            Some(Act::CancelPlace) => {
                self.floating = false;
                self.drag = None;
                self.step(true);
                self.redo.pop();
                self.sel_part = None;
                self.note = Some("Placing cancelled.".into());
            }
            Some(Act::Undo) => self.step(true),
            Some(Act::Redo) => self.step(false),
            Some(Act::Save) => {
                pick = Some(match self.save() {
                    Ok(p) => Pick::Message(format!("Saved {}", p.display()), false),
                    Err(e) => Pick::Message(e, true),
                });
            }
            None => {}
        }
        self.erc_window(ui.ctx());
        self.annotate_window(ui.ctx());
        pick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altium_sheets_save_with_a_project_that_lists_them() {
        let dir = std::env::temp_dir().join(format!("trace-altium-save-{}", std::process::id()));
        let src = dir.join("src");
        let out = dir.join("out");
        std::fs::create_dir_all(&src).unwrap();
        let prj = src.join("Demo.PrjPcb");
        std::fs::write(
            &prj,
            "[Design]\r\nVersion=1.0\r\n[Document1]\r\nDocumentPath=Main.SchDoc\r\n[Document2]\r\nDocumentPath=Board.PcbDoc\r\n",
        )
        .unwrap();
        let mut sheet = Sheet::default();
        sheet.drawing.size = [11000.0, 8500.0];
        sheet.drawing.add_wire(vec![[100.0, 100.0], [500.0, 100.0]]);
        sheet.drawing.add_label([500.0, 100.0], "SDA");
        let mut s = Schematics {
            project: Some(prj.clone()),
            ..Default::default()
        };
        s.slots.push(Slot {
            path: src.join("Main.SchDoc"),
            name: "Main".into(),
            sheet: Some(sheet),
            error: None,
            boxes: Vec::new(),
        });
        assert_eq!(s.save_altium(&out), Ok(1));
        let back = trace_altium::read_schdoc(out.join("Main.SchDoc")).unwrap();
        assert_eq!(back.net_labels, ["SDA"]);
        assert_eq!(back.wires, 1);
        let p = std::fs::read_to_string(out.join("Demo.PrjPcb")).unwrap();
        assert!(p.contains("DocumentPath=Main.SchDoc\r\n"));
        let board = src.join("Board.PcbDoc").display().to_string();
        assert!(p.contains(&format!("DocumentPath={board}\r\n")));
        // Saving again keeps the first file as a backup.
        assert_eq!(s.save_altium(&out), Ok(1));
        assert!(out.join("Main.SchDoc.bak").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
