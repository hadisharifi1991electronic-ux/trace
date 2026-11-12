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
use trace_core::sch_edit::{snap, WIRE_COLOR};
use trace_core::{Drawing, SavedSheet, SchFile, SchGeom, Sheet};

/// Grid the editing tools snap to, in mil.
const GRID: f64 = 50.0;

/// What a click on the sheet does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchTool {
    #[default]
    Select,
    Wire,
    Label,
}

/// An edit worked out while drawing, made once the drawing is done with.
enum Act {
    Move(usize, [f64; 2]),
    Turn(usize),
    DeletePart(usize),
    CopyPart(usize),
    DeleteWire(usize),
    DeleteTag(usize),
    Wire(Vec<[f64; 2]>),
    Label([f64; 2], String),
    Undo,
    Redo,
    Save,
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
    undo: Vec<(usize, Sheet)>,
    redo: Vec<(usize, Sheet)>,
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
    /// A part being dragged, and the grid point it was last moved to.
    drag: Option<(usize, [f64; 2])>,
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
    let mut best: Option<([f64; 2], f64)> = None;
    for w in &d.wires {
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
        *self = Schematics {
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
        for s in f.sheets {
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
        };
        file.save(&path).map_err(|e| e.to_string())?;
        self.saved_to = Some(path.clone());
        self.dirty = false;
        Ok(path)
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
        let sources = Sources {
            parts: Some(&parts),
            library,
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
            self.sel_part = Some(n);
            self.sel_wire = None;
            self.sel_tag = None;
            self.note = Some(format!(
                "Copied {name} as {new}. Wire it up, then Update PCB adds it to the board."
            ));
        }
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
        self.undo.push((cur, sheet.clone()));
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
    }

    fn step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some((i, sheet)) = from.pop() else { return };
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        if let Some(now) = slot.sheet.replace(sheet) {
            to.push((i, now));
        }
        self.select_sheet(i);
        self.sel_part = None;
        self.sel_wire = None;
        self.sel_tag = None;
        self.after_edit(i);
    }

    pub fn select_sheet(&mut self, i: usize) {
        if i < self.slots.len() && i != self.current {
            self.current = i;
            self.sel_net = None;
            self.sel_part = None;
            self.sel_wire = None;
            self.sel_tag = None;
            self.wire_pts.clear();
            self.drag = None;
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
                    "Click to select, drag parts to move them",
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
            let save = ui
                .add_enabled(self.dirty, egui::Button::new("Save"))
                .on_hover_text("Save the sheets as one Trace schematic file next to the project (Ctrl+S)");
            if save.clicked() {
                self.note = Some(match self.save() {
                    Ok(p) => format!("Saved {}", p.display()),
                    Err(e) => e,
                });
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
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).weak().small());
        }
        ui.separator();
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
        // Pressing on a part with the Select tool drags it; anywhere else
        // pans.
        if resp.drag_started() && self.tool == SchTool::Select {
            let origin = ui.input(|i| i.pointer.press_origin());
            if let Some(o) = origin {
                let b = self.view.to_board(o);
                let p = [b[0] as f64, b[1] as f64];
                if let Some(i) = part_at(&sheet.drawing, p) {
                    self.undo.push((cur, sheet.clone()));
                    self.redo.clear();
                    self.drag = Some((i, snap(p, GRID)));
                    self.sel_part = Some(i);
                    self.sel_net = None;
                    self.sel_wire = None;
                    self.sel_tag = None;
                }
            }
        }
        if resp.dragged() && self.drag.is_none() {
            let d = resp.drag_delta();
            self.view.tx += d.x;
            self.view.ty += d.y;
        }
        if !resp.dragged() && !resp.drag_started() {
            self.drag = None;
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

        // Selected wire and label, the wire being drawn.
        let sel_stroke = Stroke::new((30.0 * view.scale).max(4.0), HIGHLIGHT.gamma_multiply(0.5));
        if let Some(w) = self.sel_wire.and_then(|w| d.wires.get(w)) {
            let pts: Vec<Pos2> = w
                .iter()
                .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                .collect();
            painter.add(Shape::line(pts, sel_stroke));
        }
        if let Some(t) = self.sel_tag.and_then(|t| d.tags.get(t)) {
            for a in &t.at {
                painter.circle_stroke(
                    view.to_screen(a[0] as f32, a[1] as f32),
                    (40.0 * view.scale).max(6.0),
                    sel_stroke,
                );
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
                .map(|q| (q, (q[0] - p[0]).hypot(q[1] - p[1])))
                .filter(|(_, dist)| *dist < catch)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(q, _)| q)
                .or_else(|| on_wire_near(d, p, catch))
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
        if self.tool == SchTool::Wire {
            if let (Some(&last), Some(c)) = (self.wire_pts.last(), snapped) {
                let mut pts = self.wire_pts.clone();
                pts.push([c[0], last[1]]);
                pts.push(c);
                let pts: Vec<Pos2> = pts
                    .iter()
                    .map(|p| view.to_screen(p[0] as f32, p[1] as f32))
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(px(10.0).max(2.0), wire_color)));
            }
        }
        if matches!(self.tool, SchTool::Wire | SchTool::Label) {
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
                SchTool::Label => {
                    if resp.clicked() {
                        self.label_at = Some(c);
                        self.label_text.clear();
                    }
                }
                SchTool::Select => {
                    if let Some((i, last)) = self.drag {
                        if c != last {
                            act = Some(Act::Move(i, [c[0] - last[0], c[1] - last[1]]));
                            self.drag = Some((i, c));
                        }
                    }
                    if resp.double_clicked() {
                        if let Some(i) = hit_link {
                            let want = d.links[i].file.to_ascii_lowercase();
                            let target = self.slots.iter().position(|s| {
                                s.path.file_name().is_some_and(|f| {
                                    f.to_string_lossy().to_ascii_lowercase() == want
                                })
                            });
                            if let Some(t) = target {
                                self.select_sheet(t);
                                return None;
                            }
                        }
                    } else if resp.clicked() {
                        self.sel_wire = if hit_pin.is_none() { hit_wire } else { None };
                        self.sel_tag = hit_tag;
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
            }
        }

        // Keys, while the pointer is over the sheet and no text is typed.
        let typing = ui.ctx().wants_keyboard_input();
        if resp.hovered() && !typing && act.is_none() {
            let copy = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D));
            if copy && self.tool == SchTool::Select {
                if let Some(i) = self.sel_part {
                    act = Some(Act::CopyPart(i));
                }
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
            if esc {
                if self.wire_pts.len() >= 2 {
                    act = Some(Act::Wire(std::mem::take(&mut self.wire_pts)));
                } else {
                    self.wire_pts.clear();
                    self.tool = SchTool::Select;
                }
            }
            if space {
                if let Some(i) = self.drag.map(|d| d.0).or(self.sel_part) {
                    act = Some(Act::Turn(i));
                }
            }
            if del && self.tool == SchTool::Select {
                act = if let Some(t) = self.sel_tag {
                    Some(Act::DeleteTag(t))
                } else if let Some(w) = self.sel_wire {
                    Some(Act::DeleteWire(w))
                } else {
                    self.sel_part.map(Act::DeletePart)
                };
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
            Some(Act::Move(i, dv)) => {
                // One undo step covers the whole drag.
                if let Some(s) = self.slots.get_mut(cur).and_then(|s| s.sheet.as_mut()) {
                    s.drawing.move_part(i, dv);
                }
                self.after_edit(cur);
                self.sel_part = Some(i);
            }
            Some(Act::Turn(i)) => {
                if self.drag.is_some() {
                    if let Some(s) = self.slots.get_mut(cur).and_then(|s| s.sheet.as_mut()) {
                        s.drawing.turn_part(i);
                    }
                    self.after_edit(cur);
                } else {
                    self.edit(|s| s.drawing.turn_part(i));
                }
                self.sel_part = Some(i);
            }
            Some(Act::DeletePart(i)) => {
                self.edit(|s| s.drawing.delete_part(i));
                self.sel_part = None;
            }
            Some(Act::CopyPart(i)) => self.copy_part(i, scene),
            Some(Act::DeleteWire(w)) => {
                self.edit(|s| s.drawing.delete_wire(w));
                self.sel_wire = None;
            }
            Some(Act::DeleteTag(t)) => {
                self.edit(|s| s.drawing.delete_tag(t));
                self.sel_tag = None;
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
        pick
    }
}
