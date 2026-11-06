//! A loaded board turned into drawable primitives (in mil, y up), with an
//! index per net and a grid for picking.

use std::collections::HashMap;
use std::path::PathBuf;
use trace_core::{Board, LayerRef, PadShape};

/// Where a primitive is drawn. Copper layers are 0 (top) to n-1 (bottom).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    Copper(u8),
    TopSilk,
    BottomSilk,
    Vias,
    /// Switch for copper pours and planes; they are drawn with their layer.
    Pours,
    Other,
}

/// A copper pour triangulated for drawing and picking.
pub struct Fill {
    pub slot: Slot,
    pub net: Option<u32>,
    pub bbox: [f32; 4],
    /// Range in `Scene::fill_verts`.
    pub verts: std::ops::Range<usize>,
    /// Range in `Scene::fill_tris`; indices are relative to `verts.start`.
    pub tris: std::ops::Range<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum Geom {
    /// Segment with full width.
    Seg {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        w: f32,
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
    },
    /// Rectangle centred on (x, y), rotated by `rot` degrees.
    Rect {
        x: f32,
        y: f32,
        sx: f32,
        sy: f32,
        rot: f32,
    },
}

impl Geom {
    pub fn bbox(&self) -> [f32; 4] {
        match *self {
            Geom::Seg { x1, y1, x2, y2, w } => [
                x1.min(x2) - w / 2.0,
                y1.min(y2) - w / 2.0,
                x1.max(x2) + w / 2.0,
                y1.max(y2) + w / 2.0,
            ],
            Geom::Circle { x, y, r } => [x - r, y - r, x + r, y + r],
            Geom::Rect { x, y, sx, sy, .. } => {
                let r = sx.hypot(sy) / 2.0;
                [x - r, y - r, x + r, y + r]
            }
        }
    }

    /// Distance from a point to the shape's edge (negative inside).
    pub fn distance(&self, px: f32, py: f32) -> f32 {
        match *self {
            Geom::Seg { x1, y1, x2, y2, w } => {
                let (dx, dy) = (x2 - x1, y2 - y1);
                let l = dx * dx + dy * dy;
                let t = if l > 0.0 {
                    (((px - x1) * dx + (py - y1) * dy) / l).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (px - (x1 + t * dx)).hypot(py - (y1 + t * dy)) - w / 2.0
            }
            Geom::Circle { x, y, r } => (px - x).hypot(py - y) - r,
            Geom::Rect { x, y, sx, sy, rot } => {
                let (s, c) = (-rot.to_radians()).sin_cos();
                let (u, v) = ((px - x) * c - (py - y) * s, (px - x) * s + (py - y) * c);
                (u.abs() - sx / 2.0).max(v.abs() - sy / 2.0)
            }
        }
    }
}

/// The board item a primitive was drawn from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Track(usize),
    Arc(usize),
    Pad(usize),
    Via(usize),
}

#[derive(Clone, Copy)]
pub struct Prim {
    pub source: Source,
    pub slot: Slot,
    pub net: Option<u32>,
    pub geom: Geom,
    /// Through-hole pads appear on every copper layer.
    pub all_layers: bool,
    pub component: Option<u32>,
}

pub struct Layer {
    pub slot: Slot,
    pub name: String,
    pub color: [u8; 3],
    pub visible: bool,
    pub count: usize,
}

pub struct Scene {
    pub path: PathBuf,
    pub board: Board,
    pub prims: Vec<Prim>,
    pub layers: Vec<Layer>,
    pub copper_count: u8,
    pub outline: Vec<[f32; 2]>,
    pub bounds: [f32; 4],
    pub by_net: HashMap<u32, Vec<usize>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    pub net_by_name: HashMap<String, u32>,
    pub pair_of: HashMap<String, (String, String)>, // net -> (pair name, partner)
    pub fills: Vec<Fill>,
    pub fill_verts: Vec<[f32; 2]>,
    pub fill_tris: Vec<u32>,
    /// How each file was read, for boards rebuilt from Gerber or ODB++.
    pub import: Option<ImportInfo>,
}

pub struct ImportInfo {
    /// (file, what it was taken as, why)
    pub files: Vec<(String, String, String)>,
    pub nets_from: String,
    pub warnings: Vec<String>,
}

const CELL: f32 = 150.0;

const PALETTE: [[u8; 3]; 16] = [
    [70, 167, 88],
    [245, 165, 36],
    [168, 110, 233],
    [18, 165, 148],
    [233, 61, 130],
    [142, 140, 153],
    [255, 180, 84],
    [95, 184, 240],
    [127, 209, 127],
    [214, 64, 159],
    [158, 123, 240],
    [46, 189, 229],
    [247, 107, 21],
    [194, 166, 121],
    [110, 159, 255],
    [255, 139, 139],
];

fn mil(l: trace_core::Length) -> f32 {
    l.mil() as f32
}

impl Scene {
    /// Opens an Altium `.PcbDoc` or a Trace `.tracepcb` board.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let board = match ext.as_deref() {
            Some("pcbdoc") => trace_altium::read_pcbdoc(&path).map_err(|e| e.to_string())?,
            Some("brd") => trace_allegro::read_brd(&path).map_err(|e| e.to_string())?,
            Some(trace_core::file::EXTENSION) => trace_core::file::load(&path)?,
            _ if path.is_dir()
                || matches!(ext.as_deref(), Some("zip") | Some("tgz") | Some("gz"))
                || trace_gerber::is_fab_file(&name) =>
            {
                let imp = trace_gerber::read_path(&path).map_err(|e| e.to_string())?;
                return Ok(Self::from_import(path, imp));
            }
            _ => {
                return Err(
                    "expected an Altium .PcbDoc, an Allegro .brd, Gerber files (folder or .zip), ODB++ or a Trace .tracepcb board".into(),
                )
            }
        };
        Ok(Self::from_board(path, board))
    }

    /// Several Gerber and drill files read together as one board.
    pub fn load_files(paths: Vec<PathBuf>) -> Result<Self, String> {
        let mut files = Vec::new();
        for p in &paths {
            let data = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push(trace_gerber::SourceFile { name, data });
        }
        let imp = trace_gerber::read_files(files).map_err(|e| e.to_string())?;
        let dir = paths
            .first()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        Ok(Self::from_import(dir, imp))
    }

    fn from_import(path: PathBuf, imp: trace_gerber::Import) -> Self {
        let info = ImportInfo {
            files: imp
                .files
                .iter()
                .map(|f| {
                    let role = match f.copper_index {
                        Some(i) => format!("{} (L{})", f.role, i + 1),
                        None => f.role.clone(),
                    };
                    (f.file.clone(), role, f.reason.clone())
                })
                .collect(),
            nets_from: imp.nets_from.to_string(),
            warnings: imp.warnings.clone(),
        };
        let mut scene = Self::from_board(path, imp.board);
        scene.import = Some(info);
        scene
    }

    /// Rebuilds the drawing after the board changed, keeping layer settings.
    #[allow(dead_code)] // used by editing, paused for now
    pub fn rebuild(&mut self) {
        let board = std::mem::take(&mut self.board);
        let mut next = Self::from_board(std::mem::take(&mut self.path), board);
        for l in &mut next.layers {
            if let Some(old) = self.layers.iter().find(|o| o.slot == l.slot) {
                l.visible = old.visible;
            }
        }
        *self = next;
    }

    pub fn from_board(path: PathBuf, board: Board) -> Self {
        let copper: Vec<String> = board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect();
        let n = copper.len() as u8;
        let slot = |l: &LayerRef| match l {
            LayerRef::Copper(i) => Slot::Copper(*i),
            LayerRef::AllCopper => Slot::Copper(0),
            LayerRef::TopSilk => Slot::TopSilk,
            LayerRef::BottomSilk => Slot::BottomSilk,
            _ => Slot::Other,
        };
        let g = &board.geometry;
        let mut prims =
            Vec::with_capacity(g.tracks.len() + g.arcs.len() * 4 + g.vias.len() + g.pads.len());
        for (i, t) in g.tracks.iter().enumerate() {
            prims.push(Prim {
                source: Source::Track(i),
                slot: slot(&t.layer),
                net: t.net,
                geom: Geom::Seg {
                    x1: mil(t.start.x),
                    y1: mil(t.start.y),
                    x2: mil(t.end.x),
                    y2: mil(t.end.y),
                    w: mil(t.width),
                },
                all_layers: false,
                component: t.component,
            });
        }
        for (ai, a) in g.arcs.iter().enumerate() {
            let (cx, cy, r) = (a.center.x.mil(), a.center.y.mil(), a.radius.mil());
            let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
            if sweep == 0.0 {
                sweep = 360.0;
            }
            let steps = (sweep / 10.0).ceil().max(1.0) as usize;
            let p = |i: usize| {
                let t = (a.start_deg + sweep * i as f64 / steps as f64).to_radians();
                ((cx + r * t.cos()) as f32, (cy + r * t.sin()) as f32)
            };
            for i in 0..steps {
                let ((x1, y1), (x2, y2)) = (p(i), p(i + 1));
                prims.push(Prim {
                    source: Source::Arc(ai),
                    slot: slot(&a.layer),
                    net: a.net,
                    geom: Geom::Seg {
                        x1,
                        y1,
                        x2,
                        y2,
                        w: mil(a.width),
                    },
                    all_layers: false,
                    component: a.component,
                });
            }
        }
        for (pi, p) in g.pads.iter().enumerate() {
            let (x, y, sx, sy) = (mil(p.at.x), mil(p.at.y), mil(p.size_x), mil(p.size_y));
            let geom = if p.shape == PadShape::Round && (sx - sy).abs() < 1e-3 {
                Geom::Circle { x, y, r: sx / 2.0 }
            } else if p.shape == PadShape::Round {
                // Oval: a segment with round ends along the long axis.
                let (long, short) = (sx.max(sy), sx.min(sy));
                let ang = (p.rotation_deg as f32 + if sx >= sy { 0.0 } else { 90.0 }).to_radians();
                let h = (long - short) / 2.0;
                Geom::Seg {
                    x1: x - h * ang.cos(),
                    y1: y - h * ang.sin(),
                    x2: x + h * ang.cos(),
                    y2: y + h * ang.sin(),
                    w: short,
                }
            } else {
                Geom::Rect {
                    x,
                    y,
                    sx,
                    sy,
                    rot: p.rotation_deg as f32,
                }
            };
            prims.push(Prim {
                source: Source::Pad(pi),
                slot: slot(&p.layer),
                net: p.net,
                geom,
                all_layers: p.layer == LayerRef::AllCopper,
                component: p.component,
            });
        }
        for (vi, v) in g.vias.iter().enumerate() {
            prims.push(Prim {
                source: Source::Via(vi),
                slot: Slot::Vias,
                net: v.net,
                geom: Geom::Circle {
                    x: mil(v.at.x),
                    y: mil(v.at.y),
                    r: mil(v.diameter) / 2.0,
                },
                all_layers: true,
                component: None,
            });
        }

        let mut layers: Vec<Layer> = copper
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let color = if i == 0 {
                    [229, 72, 77]
                } else if i as u8 == n - 1 {
                    [62, 156, 240]
                } else {
                    PALETTE[(i - 1) % PALETTE.len()]
                };
                Layer {
                    slot: Slot::Copper(i as u8),
                    name: name.clone(),
                    color,
                    visible: true,
                    count: 0,
                }
            })
            .collect();
        layers.push(Layer {
            slot: Slot::Vias,
            name: "Vias".into(),
            color: [201, 180, 88],
            visible: true,
            count: 0,
        });
        if !g.polygons.is_empty() {
            layers.push(Layer {
                slot: Slot::Pours,
                name: "Copper pours".into(),
                color: [120, 130, 150],
                visible: true,
                count: g.polygons.len(),
            });
        }
        layers.push(Layer {
            slot: Slot::TopSilk,
            name: "Top silk".into(),
            color: [230, 230, 230],
            visible: true,
            count: 0,
        });
        layers.push(Layer {
            slot: Slot::BottomSilk,
            name: "Bottom silk".into(),
            color: [180, 140, 255],
            visible: false,
            count: 0,
        });
        for p in &prims {
            if let Some(l) = layers.iter_mut().find(|l| l.slot == p.slot) {
                l.count += 1;
            }
        }

        let outline: Vec<[f32; 2]> = g.outline.iter().map(|p| [mil(p.x), mil(p.y)]).collect();
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let pts: Vec<[f32; 2]> = if outline.len() >= 3 {
            outline.clone()
        } else {
            prims
                .iter()
                .flat_map(|p| {
                    let b = p.geom.bbox();
                    [[b[0], b[1]], [b[2], b[3]]]
                })
                .collect()
        };
        for [x, y] in pts {
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }

        let mut by_net: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, p) in prims.iter().enumerate() {
            if let Some(n) = p.net {
                by_net.entry(n).or_default().push(i);
            }
            let b = p.geom.bbox();
            for gx in (b[0] / CELL).floor() as i32..=(b[2] / CELL).floor() as i32 {
                for gy in (b[1] / CELL).floor() as i32..=(b[3] / CELL).floor() as i32 {
                    grid.entry((gx, gy)).or_default().push(i);
                }
            }
        }
        let net_by_name = board
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i as u32))
            .collect();
        let mut pair_of = HashMap::new();
        for p in &board.diff_pairs {
            pair_of.insert(p.positive.clone(), (p.name.clone(), p.negative.clone()));
            pair_of.insert(p.negative.clone(), (p.name.clone(), p.positive.clone()));
        }

        let (fills, fill_verts, fill_tris) = triangulate(&board, &slot);

        Scene {
            path,
            fills,
            fill_verts,
            fill_tris,
            import: None,
            board,
            prims,
            layers,
            copper_count: n,
            outline,
            bounds,
            by_net,
            grid,
            net_by_name,
            pair_of,
        }
    }

    pub fn layer_visible(&self, slot: Slot) -> bool {
        self.layers
            .iter()
            .find(|l| l.slot == slot)
            .is_none_or(|l| l.visible)
    }

    /// Whether a primitive shows with the current layer settings.
    pub fn shows(&self, p: &Prim) -> bool {
        if p.slot == Slot::Other {
            return false;
        }
        if p.all_layers && p.slot != Slot::Vias {
            return self
                .layers
                .iter()
                .any(|l| matches!(l.slot, Slot::Copper(_)) && l.visible);
        }
        self.layer_visible(p.slot)
    }

    /// Whether a pour shows with the current layer settings.
    pub fn fill_shows(&self, f: &Fill) -> bool {
        self.layer_visible(Slot::Pours) && self.layer_visible(f.slot)
    }

    /// A visible pour containing a point: on the active layer only, or on
    /// any other layer.
    pub fn pick_fill(&self, x: f32, y: f32, active: Slot, on_active: bool) -> Option<usize> {
        self.fills.iter().position(|f| {
            let b = f.bbox;
            if x < b[0]
                || x > b[2]
                || y < b[1]
                || y > b[3]
                || (f.slot == active) != on_active
                || !self.fill_shows(f)
            {
                return false;
            }
            let v = &self.fill_verts[f.verts.clone()];
            self.fill_tris[f.tris.clone()].chunks_exact(3).any(|t| {
                inside_triangle([x, y], v[t[0] as usize], v[t[1] as usize], v[t[2] as usize])
            })
        })
    }

    /// Whether a primitive is drawn with the active layer: on it, or a via
    /// or through-hole pad, which show on every layer.
    fn on_active(&self, p: &Prim, active: Slot) -> bool {
        p.slot == active || p.slot == Slot::Vias || p.all_layers
    }

    /// The primitive under a board point, on the active layer only or on
    /// any other visible layer. Copper the point is inside of beats copper
    /// merely near it; part pads win ties so a click can select the part.
    pub fn pick(
        &self,
        x: f32,
        y: f32,
        tolerance: f32,
        active: Slot,
        on_active: bool,
    ) -> Option<usize> {
        let cell = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        let mut best: Option<(f32, usize)> = None;
        for &i in self.grid.get(&cell)? {
            let p = &self.prims[i];
            if self.on_active(p, active) != on_active || !self.shows(p) {
                continue;
            }
            let d = p.geom.distance(x, y);
            if d > tolerance {
                continue;
            }
            // Inside counts the same however deep, so a narrow track is
            // not lost to a wide one or a pad it runs into.
            let score = d.max(0.0)
                - if p.component.is_some() && d <= 0.0 {
                    0.01
                } else {
                    0.0
                };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// What a click at a board point lands on, as the person sees it: the
    /// active layer's copper, then its pours, then other layers' copper,
    /// then their pours. Returns the net and the part (for pads).
    pub fn net_at(
        &self,
        x: f32,
        y: f32,
        tolerance: f32,
        active: Slot,
    ) -> (Option<u32>, Option<u32>) {
        for on_active in [true, false] {
            if let Some(i) = self.pick(x, y, tolerance, active, on_active) {
                return (self.prims[i].net, self.prims[i].component);
            }
            if let Some(f) = self.pick_fill(x, y, active, on_active) {
                return (self.fills[f].net, None);
            }
        }
        (None, None)
    }

    /// A point to start or end a route at: the centre of a pad or via, or
    /// the nearer end of a track, on the given copper layer. Also returns
    /// the net found there.
    pub fn snap(
        &self,
        x: f32,
        y: f32,
        tolerance: f32,
        layer: Option<Slot>,
    ) -> Option<([f32; 2], Option<u32>)> {
        let cell = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        let mut best: Option<(f32, [f32; 2], Option<u32>)> = None;
        for &i in self.grid.get(&cell)? {
            let p = &self.prims[i];
            let on_layer = match layer {
                Some(l) => p.slot == l || p.all_layers || p.slot == Slot::Vias,
                None => self.shows(p),
            };
            if !on_layer || p.geom.distance(x, y) > tolerance {
                continue;
            }
            let target = match p.geom {
                Geom::Circle { x, y, .. } | Geom::Rect { x, y, .. } => [x, y],
                Geom::Seg { x1, y1, x2, y2, .. } => match p.source {
                    // Oval pads snap to their centre.
                    Source::Pad(_) => [(x1 + x2) / 2.0, (y1 + y2) / 2.0],
                    _ if (x - x1).hypot(y - y1) <= (x - x2).hypot(y - y2) => [x1, y1],
                    _ => [x2, y2],
                },
            };
            let d = (target[0] - x).hypot(target[1] - y);
            if best.is_none_or(|(b, _, _)| d < b) {
                best = Some((d, target, p.net));
            }
        }
        best.map(|(_, t, n)| (t, n))
    }

    /// Primitives belonging to a part (its pads).
    #[allow(dead_code)]
    pub fn component_prims(&self, component: usize) -> impl Iterator<Item = usize> + '_ {
        self.prims
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.component == Some(component as u32))
            .map(|(i, _)| i)
    }

    /// Bounding box of a net's copper.
    pub fn net_bounds(&self, net: u32) -> Option<[f32; 4]> {
        let list = self.by_net.get(&net)?;
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &i in list {
            let g = self.prims[i].geom.bbox();
            b = [
                b[0].min(g[0]),
                b[1].min(g[1]),
                b[2].max(g[2]),
                b[3].max(g[3]),
            ];
        }
        Some(b)
    }
}

fn inside_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let side =
        |u: [f32; 2], v: [f32; 2]| (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0]);
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Splits every copper pour into triangles once, so drawing is cheap.
/// Planes can have thousands of cut-outs, so this uses a sweep-line
/// tessellator with the even-odd rule rather than ear clipping.
fn triangulate(
    board: &Board,
    slot: &dyn Fn(&LayerRef) -> Slot,
) -> (Vec<Fill>, Vec<[f32; 2]>, Vec<u32>) {
    use lyon_tessellation::geometry_builder::{BuffersBuilder, VertexBuffers};
    use lyon_tessellation::math::point;
    use lyon_tessellation::path::Path;
    use lyon_tessellation::{FillOptions, FillRule, FillTessellator, FillVertex};

    let mut fills = Vec::new();
    let mut verts: Vec<[f32; 2]> = Vec::new();
    let mut tris: Vec<u32> = Vec::new();
    let mut tess = FillTessellator::new();
    let options = FillOptions::tolerance(0.05).with_fill_rule(FillRule::EvenOdd);
    for poly in &board.geometry.polygons {
        let mut path = Path::builder();
        for ring in std::iter::once(&poly.outline).chain(poly.holes.iter()) {
            if ring.len() < 3 {
                continue;
            }
            let mut pts = ring
                .iter()
                .map(|p| point(p.x.mil() as f32, p.y.mil() as f32));
            path.begin(pts.next().unwrap_or(point(0.0, 0.0)));
            for p in pts {
                path.line_to(p);
            }
            path.end(true);
        }
        let path = path.build();
        let mut out: VertexBuffers<lyon_tessellation::math::Point, u32> = VertexBuffers::new();
        if tess
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut out, |v: FillVertex| v.position()),
            )
            .is_err()
            || out.indices.is_empty()
        {
            continue;
        }
        let v0 = verts.len();
        let t0 = tris.len();
        let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &out.vertices {
            bbox = [
                bbox[0].min(p.x),
                bbox[1].min(p.y),
                bbox[2].max(p.x),
                bbox[3].max(p.y),
            ];
            verts.push([p.x, p.y]);
        }
        tris.extend_from_slice(&out.indices);
        fills.push(Fill {
            slot: slot(&poly.layer),
            net: poly.net,
            bbox,
            verts: v0..verts.len(),
            tris: t0..tris.len(),
        });
    }
    (fills, verts, tris)
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    /// Clicks the middle of every track on the boards in TRACE_PICK_BOARDS
    /// (separated by ';') and reports tracks whose own net is not picked.
    #[test]
    fn every_track_is_pickable() {
        let Ok(list) = std::env::var("TRACE_PICK_BOARDS") else {
            return;
        };
        for path in list.split(';').filter(|s| !s.is_empty()) {
            let scene = Scene::load(PathBuf::from(path)).expect(path);
            let (mut total, mut bad) = (0, Vec::new());
            for (i, p) in scene.prims.iter().enumerate() {
                let (Source::Track(_), Geom::Seg { x1, y1, x2, y2, .. }, Some(net)) =
                    (p.source, p.geom, p.net)
                else {
                    continue;
                };
                total += 1;
                let (x, y) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
                let tol = 3.0 / 0.24;
                let got = scene.net_at(x, y, tol, p.slot).0;
                if got != Some(net) {
                    bad.push((i, got, net));
                }
            }
            eprintln!("{path}: {} of {total} tracks not pickable", bad.len());
            for (i, got, net) in bad.iter().take(8) {
                let p = &scene.prims[*i];
                eprintln!(
                    "  {:?} {:?} net {} got {:?} shows={}",
                    p.slot,
                    p.geom,
                    scene.board.nets[*net as usize].name,
                    got.map(|g| scene.board.nets[g as usize].name.clone()),
                    scene.shows(p)
                );
            }
        }
    }
}
