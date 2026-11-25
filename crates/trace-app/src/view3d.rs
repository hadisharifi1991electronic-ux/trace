//! 3D board view: the board as it is built. Laminate between copper layers
//! at their stack-up heights, copper with its real thickness, plated
//! barrels in every hole, solder mask, silkscreen and part bodies, drawn
//! with OpenGL and turned with the mouse. X-ray makes the laminate clear so
//! the inner layers show, and a section plane cuts the board open anywhere
//! with the cut faces drawn solid.

use crate::scene::{Geom, Scene, Slot, Source};
use eframe::egui::{self, Color32, RichText};
use eframe::glow::{self, HasContext};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use trace_core::{LayerKind, LayerRef, PadShape, Side};

const MM_PER_MIL: f32 = 0.0254;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vert {
    p: [f32; 3],
    /// Colour, and in alpha how shiny (0 matte, 255 polished metal).
    c: [u8; 4],
    /// Normal, and in w what the vertex belongs to (see `TAG_*`).
    n: [i8; 4],
}

/// Belongs to nothing that can be hidden: parts, silkscreen, barrels.
const TAG_MISC: i8 = 0;
/// Laminate and solder mask: nearly clear in X-ray, where copper is
/// half clear and only `TAG_MISC` stays solid.
const TAG_DIEL: i8 = 100;
/// Copper layer i is tagged 1 + i.
fn layer_tag(i: usize) -> i8 {
    1 + i.min(63) as i8
}

#[derive(Default)]
pub struct Mesh {
    verts: Vec<Vert>,
    idx: Vec<u32>,
    /// Half the board's diagonal, mm.
    pub radius: f32,
    pub parts: usize,
    /// Half the board's size in x and y, mm.
    pub half: [f32; 2],
    /// What the section plane cuts, when the mesh was built for one.
    section: Option<Arc<CapSrc>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MaskColor {
    Green,
    Blue,
    Black,
    Red,
    White,
}

impl MaskColor {
    pub const ALL: [MaskColor; 5] = [
        MaskColor::Green,
        MaskColor::Blue,
        MaskColor::Black,
        MaskColor::Red,
        MaskColor::White,
    ];
    pub fn name(self) -> &'static str {
        match self {
            MaskColor::Green => "Green",
            MaskColor::Blue => "Blue",
            MaskColor::Black => "Black",
            MaskColor::Red => "Red",
            MaskColor::White => "White",
        }
    }
    /// (mask over bare laminate, mask over copper, silkscreen)
    fn colors(self) -> ([u8; 3], [u8; 3], [u8; 3]) {
        match self {
            MaskColor::Green => ([22, 82, 44], [52, 128, 66], [238, 238, 232]),
            MaskColor::Blue => ([18, 52, 120], [44, 88, 165], [238, 238, 232]),
            MaskColor::Black => ([22, 22, 24], [44, 44, 48], [238, 238, 232]),
            MaskColor::Red => ([128, 22, 24], [168, 48, 46], [238, 238, 232]),
            MaskColor::White => ([224, 224, 218], [240, 240, 234], [30, 30, 30]),
        }
    }
}

const FINISH: [u8; 3] = [214, 172, 82];
const COPPER: [u8; 3] = [206, 122, 72];
const BARREL: [u8; 3] = [196, 116, 66];
const EDGE: [u8; 3] = [190, 170, 120];
const CORE: [u8; 3] = [214, 196, 140];
const PREPREG: [u8; 3] = [184, 160, 102];
const DRILL_WALL: [u8; 3] = [150, 136, 96];

/// Shine of polished copper and gold, and of everything else. Below
/// `METAL` the shine is a clear gloss over a coloured surface: solder mask
/// is lacquer-glossy, silkscreen ink and part bodies a little.
const METAL: u8 = 255;
const MASK_GLOSS: u8 = 150;
const INK_GLOSS: u8 = 40;
const BODY_GLOSS: u8 = 70;
const MATTE: u8 = 0;

/// Gold finish on bare pads, above their copper, mm.
const Z_FINISH: f32 = 0.004;
/// Silkscreen above the outer copper, mm.
const Z_SILK: f32 = 0.020;

/// Heights of the board's copper and laminate, mm, board centred on z = 0.
struct Stack {
    /// Each copper layer's (bottom, top), top layer first.
    cu: Vec<[f32; 2]>,
    /// Each dielectric's (bottom, top) and whether it is a core.
    diel: Vec<([f32; 2], bool)>,
    /// The laminate's faces under the outer copper (bottom, top).
    lam: [f32; 2],
    /// Half the board's thickness.
    half: f32,
}

fn stack(board: &trace_core::Board, n: usize) -> Stack {
    let rows: Vec<&trace_core::StackLayer> = board
        .stackup
        .layers
        .iter()
        .filter(|l| matches!(l.kind, LayerKind::Copper | LayerKind::Dielectric))
        .collect();
    let ncu = rows
        .iter()
        .filter(|l| matches!(l.kind, LayerKind::Copper))
        .count();
    let th = |l: &trace_core::StackLayer| {
        let t = l.thickness.mm() as f32;
        if matches!(l.kind, LayerKind::Copper) && t <= 0.0 {
            0.035
        } else {
            t.max(0.0)
        }
    };
    let total: f32 = rows.iter().map(|l| th(l)).sum();
    let mut cu = Vec::new();
    let mut diel = Vec::new();
    let half;
    if ncu == n && total > 0.2 {
        half = total / 2.0;
        let mut z = half;
        let mut cores = 0;
        for l in rows {
            let z0 = z - th(l);
            if matches!(l.kind, LayerKind::Copper) {
                cu.push([z0, z]);
            } else if z0 < z {
                let name = format!("{} {}", l.name, l.material.as_deref().unwrap_or(""))
                    .to_ascii_lowercase();
                let core = if name.contains("core") {
                    true
                } else if name.contains("prepreg") || name.contains("pp") {
                    false
                } else {
                    cores += 1;
                    cores % 2 == 1
                };
                diel.push(([z0, z], core));
            }
            z = z0;
        }
    } else {
        let t = board.stackup.total_thickness().mm() as f32;
        let total = if t > 0.2 { t } else { 1.6 };
        half = total / 2.0;
        let c = 0.035;
        let gap = if n > 1 {
            (total - n as f32 * c) / (n - 1) as f32
        } else {
            total - c
        };
        let mut z = half;
        for i in 0..n.max(1) {
            cu.push([z - c, z]);
            z -= c;
            if i + 1 < n.max(1) || n <= 1 {
                diel.push(([z - gap, z], i % 2 == 1 || n <= 2));
                z -= gap;
            }
        }
    }
    let lam_top = cu[0][0];
    let lam_bot = if cu.len() > 1 {
        cu[cu.len() - 1][1]
    } else {
        -half
    };
    Stack {
        cu,
        diel,
        lam: [lam_bot, lam_top],
        half,
    }
}

/// A triangle in the board plane, mm.
type Tri2 = [[f32; 2]; 3];

/// What a section plane can cut through: 2D triangles (mm, board centred)
/// with the heights they span and their colour.
#[derive(Default)]
struct CapSrc {
    /// Copper on each layer: (bottom, top), colour, triangles.
    layers: Vec<([f32; 2], [u8; 3], Vec<Tri2>)>,
    /// The laminate, its holes left out.
    board: Vec<[[f32; 2]; 3]>,
    /// Dielectric slices: (bottom, top), colour.
    slices: Vec<([f32; 2], [u8; 3])>,
    /// Plated barrels: centre, inner and outer radius, (bottom, top).
    barrels: Vec<([f32; 2], f32, f32, [f32; 2])>,
}

struct Builder {
    m: Mesh,
    cx: f32,
    cy: f32,
    /// Tag and shine given to new vertices.
    tag: i8,
    shine: u8,
    cap: Option<CapSrc>,
    /// The laminate's outer faces (bottom, top): the outer copper's faces
    /// lying on them are left out, as they would fight the laminate for
    /// the same depth when seen from between the layers.
    lam: [f32; 2],
}

fn shade(c: [u8; 3], k: f32) -> [u8; 3] {
    c.map(|v| (v as f32 * k).round().clamp(0.0, 255.0) as u8)
}

impl Builder {
    fn pt(&self, x: f32, y: f32) -> [f32; 2] {
        [(x - self.cx) * MM_PER_MIL, (y - self.cy) * MM_PER_MIL]
    }

    fn vert(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 3]) -> u32 {
        let q = |v: f32| (v * 127.0).round().clamp(-127.0, 127.0) as i8;
        self.m.verts.push(Vert {
            p,
            c: [c[0], c[1], c[2], self.shine],
            n: [q(n[0]), q(n[1]), q(n[2]), self.tag],
        });
        (self.m.verts.len() - 1) as u32
    }

    /// A convex polygon (mm, board-centred) at height z, as a fan.
    fn fan(&mut self, pts: &[[f32; 2]], z: f32, c: [u8; 3]) {
        if pts.len() < 3 {
            return;
        }
        let nz = if z >= 0.0 { 1.0 } else { -1.0 };
        let i0 = self.m.verts.len() as u32;
        for p in pts {
            self.vert([p[0], p[1], z], [0.0, 0.0, nz], c);
        }
        for i in 1..pts.len() as u32 - 1 {
            self.m.idx.extend_from_slice(&[i0, i0 + i, i0 + i + 1]);
        }
    }

    /// Triangles (mm, board-centred) at height z, facing up or down.
    fn face(&mut self, verts: &[[f32; 2]], tris: &[u32], z: f32, nz: f32, c: [u8; 3]) {
        let i0 = self.m.verts.len() as u32;
        for p in verts {
            self.vert([p[0], p[1], z], [0.0, 0.0, nz], c);
        }
        self.m.idx.extend(tris.iter().map(|t| i0 + t));
    }

    /// Upright quad from a to b between heights z0 and z1.
    fn wall(&mut self, a: [f32; 2], b: [f32; 2], z0: f32, z1: f32, c: [u8; 3]) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        if l < 1e-7 {
            return;
        }
        let n = [dy / l, -dx / l, 0.0];
        let i0 = self.vert([a[0], a[1], z0], n, c);
        self.vert([b[0], b[1], z0], n, c);
        self.vert([b[0], b[1], z1], n, c);
        self.vert([a[0], a[1], z1], n, c);
        self.m
            .idx
            .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
    }

    /// A solid of triangulated outline from z0 to z1: top, bottom and the
    /// walls along its boundary (edges used by one triangle only). Copper
    /// is remembered for the section plane when `layer` is given.
    #[allow(clippy::too_many_arguments)]
    fn slab(
        &mut self,
        verts: &[[f32; 2]],
        tris: &[u32],
        z0: f32,
        z1: f32,
        top: [u8; 3],
        side: [u8; 3],
        layer: Option<usize>,
    ) {
        if tris.len() < 3 {
            return;
        }
        if z1 != self.lam[0] {
            self.face(verts, tris, z1, 1.0, top);
        }
        if z0 != self.lam[1] {
            self.face(verts, tris, z0, -1.0, side);
        }
        let mut edges: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::new();
        for t in tris.chunks_exact(3) {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let k = (a.min(b), a.max(b));
                edges.entry(k).or_insert((a, b, 0)).2 += 1;
            }
        }
        for (_, (a, b, n)) in edges {
            if n == 1 {
                self.wall(verts[a as usize], verts[b as usize], z0, z1, side);
            }
        }
        if let (Some(l), Some(cap)) = (layer, self.cap.as_mut()) {
            if let Some(dst) = cap.layers.get_mut(l) {
                dst.2.extend(tris.chunks_exact(3).map(|t| {
                    [
                        verts[t[0] as usize],
                        verts[t[1] as usize],
                        verts[t[2] as usize],
                    ]
                }));
            }
        }
    }

    /// A convex ring of points as a solid.
    #[allow(clippy::too_many_arguments)]
    fn convex_slab(
        &mut self,
        pts: &[[f32; 2]],
        z0: f32,
        z1: f32,
        top: [u8; 3],
        side: [u8; 3],
        layer: Option<usize>,
    ) {
        if pts.len() < 3 {
            return;
        }
        let tris: Vec<u32> = (1..pts.len() as u32 - 1)
            .flat_map(|i| [0, i, i + 1])
            .collect();
        self.slab(pts, &tris, z0, z1, top, side, layer);
    }

    /// A tube of radius r about (x, y) mm between z0 and z1.
    fn tube(&mut self, x: f32, y: f32, r: f32, z0: f32, z1: f32, c: [u8; 3]) {
        self.tube_lit(x, y, r, [z0, z1], [c, c]);
    }

    /// A tube coloured c[0] at z[0] blending to c[1] at z[1].
    fn tube_lit(&mut self, x: f32, y: f32, r: f32, z: [f32; 2], c: [[u8; 3]; 2]) {
        let (z0, z1) = (z[0], z[1]);
        let n = segments(r).max(10);
        let ring = circle_pts(x, y, r, n);
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            // Normals pointing out from the axis, for smooth shading.
            let i0 = self.m.verts.len() as u32;
            for (p, z, c) in [(a, z0, c[0]), (b, z0, c[0]), (b, z1, c[1]), (a, z1, c[1])] {
                let nn = [(p[0] - x) / r, (p[1] - y) / r, 0.0];
                self.vert([p[0], p[1], z], nn, c);
            }
            self.m
                .idx
                .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
        }
    }

    fn geom_ring(&self, g: &Geom) -> Vec<[f32; 2]> {
        match *g {
            Geom::Circle { x, y, r } => {
                let [x, y] = self.pt(x, y);
                let r = r * MM_PER_MIL;
                circle_pts(x, y, r, segments(r))
            }
            Geom::Rect { x, y, sx, sy, rot } => {
                let (s, co) = rot.to_radians().sin_cos();
                [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .iter()
                    .map(|(u, v)| {
                        let (u, v) = (u * sx / 2.0, v * sy / 2.0);
                        self.pt(x + u * co - v * s, y + u * s + v * co)
                    })
                    .collect()
            }
            Geom::Seg { x1, y1, x2, y2, w } => {
                let [ax, ay] = self.pt(x1, y1);
                let [bx, by] = self.pt(x2, y2);
                let r = w * MM_PER_MIL / 2.0;
                if r <= 0.0 {
                    return Vec::new();
                }
                let th = (by - ay).atan2(bx - ax);
                let half = segments(r) / 2;
                let mut pts = Vec::with_capacity(2 * half + 2);
                let pi = std::f32::consts::PI;
                for i in 0..=half {
                    let a = th + pi / 2.0 + pi * i as f32 / half as f32;
                    pts.push([ax + r * a.cos(), ay + r * a.sin()]);
                }
                for i in 0..=half {
                    let a = th - pi / 2.0 + pi * i as f32 / half as f32;
                    pts.push([bx + r * a.cos(), by + r * a.sin()]);
                }
                pts
            }
        }
    }

    fn geom(&mut self, g: &Geom, z: f32, c: [u8; 3]) {
        let pts = self.geom_ring(g);
        self.fan(&pts, z, c);
    }

    /// A box standing on the board from height z0 to z1 (both signed).
    fn block(&mut self, corners: &[[f32; 2]; 4], z0: f32, z1: f32, c: [u8; 3]) {
        let side = shade(c, 0.8);
        self.fan(corners, z1, c);
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            self.wall(a, b, z0, z1, side);
        }
    }
}

fn circle_pts(x: f32, y: f32, r: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [x + r * a.cos(), y + r * a.sin()]
        })
        .collect()
}

fn segments(r_mm: f32) -> usize {
    ((r_mm * 60.0) as usize).clamp(8, 32) & !1
}

/// Triangulates rings (mm) with the even-odd rule.
fn tessellate(rings: &[Vec<[f32; 2]>]) -> (Vec<[f32; 2]>, Vec<u32>) {
    use lyon_tessellation::geometry_builder::{BuffersBuilder, VertexBuffers};
    use lyon_tessellation::math::point;
    use lyon_tessellation::path::Path;
    use lyon_tessellation::{FillOptions, FillRule, FillTessellator, FillVertex};
    let mut path = Path::builder();
    for ring in rings.iter().filter(|r| r.len() >= 3) {
        path.begin(point(ring[0][0], ring[0][1]));
        for p in &ring[1..] {
            path.line_to(point(p[0], p[1]));
        }
        path.end(true);
    }
    let path = path.build();
    let mut out: VertexBuffers<lyon_tessellation::math::Point, u32> = VertexBuffers::new();
    let ok = FillTessellator::new()
        .tessellate_path(
            &path,
            &FillOptions::tolerance(0.002).with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut out, |v: FillVertex| v.position()),
        )
        .is_ok();
    if !ok {
        return (Vec::new(), Vec::new());
    }
    (
        out.vertices.iter().map(|p| [p.x, p.y]).collect(),
        out.indices,
    )
}

/// Drill holes (mm, board centred) with the copper layers they pass,
/// found by a coarse grid.
#[derive(Default)]
struct Drills {
    holes: Vec<([f32; 2], f32, u8, u8)>,
    grid: HashMap<(i32, i32), Vec<usize>>,
}

impl Drills {
    const CELL: f32 = 1.0;

    fn add(&mut self, c: [f32; 2], r: f32, from: u8, to: u8) {
        let k = self.holes.len();
        self.holes.push((c, r, from, to));
        let cell = |v: f32| (v / Self::CELL).floor() as i32;
        for gx in cell(c[0] - r)..=cell(c[0] + r) {
            for gy in cell(c[1] - r)..=cell(c[1] + r) {
                self.grid.entry((gx, gy)).or_default().push(k);
            }
        }
    }

    /// The holes on `layer` that a box (x0, y0, x1, y1) touches.
    fn near(&self, b: [f32; 4], layer: u8, out: &mut Vec<usize>) {
        let cell = |v: f32| (v / Self::CELL).floor() as i32;
        for gx in cell(b[0])..=cell(b[2]) {
            for gy in cell(b[1])..=cell(b[3]) {
                for &k in self.grid.get(&(gx, gy)).into_iter().flatten() {
                    let (c, r, a, z) = self.holes[k];
                    if layer >= a
                        && layer <= z
                        && c[0] + r > b[0]
                        && c[0] - r < b[2]
                        && c[1] + r > b[1]
                        && c[1] - r < b[3]
                        && !out.contains(&k)
                    {
                        out.push(k);
                    }
                }
            }
        }
    }

    /// Copper triangles with the holes on `layer` taken out, or None when
    /// no hole touches them. Triangles clear of every hole are kept as
    /// they are; the rest are clipped together.
    fn cut(&self, pts: &[[f32; 2]], tris: &[u32], layer: u8) -> Option<(Vec<[f32; 2]>, Vec<u32>)> {
        use i_overlay::core::fill_rule::FillRule;
        use i_overlay::core::overlay_rule::OverlayRule;
        use i_overlay::float::single::SingleFloatOverlay;

        if self.holes.is_empty() {
            return None;
        }
        let mut hit: Vec<usize> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut local = Vec::new();
        let mut keep: Vec<u32> = Vec::new();
        let mut subject: Vec<Vec<[f64; 2]>> = Vec::new();
        for t in tris.chunks_exact(3) {
            let p = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
            let b = [
                p[0][0].min(p[1][0]).min(p[2][0]),
                p[0][1].min(p[1][1]).min(p[2][1]),
                p[0][0].max(p[1][0]).max(p[2][0]),
                p[0][1].max(p[1][1]).max(p[2][1]),
            ];
            local.clear();
            self.near(b, layer, &mut local);
            if local.is_empty() {
                keep.extend_from_slice(t);
            } else {
                subject.push(p.iter().map(|q| [q[0] as f64, q[1] as f64]).collect());
                hit.extend(local.iter().filter(|&&k| seen.insert(k)));
            }
        }
        if subject.is_empty() {
            return None;
        }
        let clip: Vec<Vec<[f64; 2]>> = hit
            .iter()
            .map(|&k| {
                let (c, r, ..) = self.holes[k];
                circle_pts(c[0], c[1], r, segments(r).max(10))
                    .into_iter()
                    .map(|q| [q[0] as f64, q[1] as f64])
                    .collect()
            })
            .collect();
        let shapes = subject.overlay(&clip, OverlayRule::Difference, FillRule::NonZero);
        let mut verts = pts.to_vec();
        for shape in shapes {
            let rings: Vec<Vec<[f32; 2]>> = shape
                .iter()
                .map(|c| c.iter().map(|q| [q[0] as f32, q[1] as f32]).collect())
                .collect();
            let (v, t) = tessellate(&rings);
            let base = verts.len() as u32;
            verts.extend(v);
            keep.extend(t.iter().map(|i| base + i));
        }
        Some((verts, keep))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Options {
    pub mask: MaskColor,
    pub parts: bool,
    pub silk: bool,
    /// Rigid-flex bends folded this far, percent of their angle.
    pub fold: u8,
    /// Clear laminate and mask, copper coloured by layer.
    pub xray: bool,
    /// Cut open by the section plane.
    pub section: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mask: MaskColor::Green,
            parts: true,
            silk: true,
            fold: 0,
            xray: false,
            section: false,
        }
    }
}

/// Builds the board's 3D mesh, in mm with the board centred on the origin.
/// Tessellated part models, by index into `Geometry::models`.
pub type Models = [Option<Arc<trace_step::Mesh>>];
/// Every part model of a board, meshed where the reader managed it.
pub type Meshes = Arc<Vec<Option<Arc<trace_step::Mesh>>>>;

pub fn build(scene: &Scene, opt: Options, models: &Models) -> Mesh {
    let b = scene.bounds;
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let section = opt.section && opt.fold == 0;
    let mut bld = Builder {
        m: Mesh::default(),
        cx,
        cy,
        tag: TAG_MISC,
        shine: MATTE,
        cap: section.then(CapSrc::default),
        lam: [f32::NAN; 2],
    };
    bld.m.radius = ((b[2] - b[0]).hypot(b[3] - b[1]) / 2.0 * MM_PER_MIL).max(1.0);
    bld.m.half = [
        (b[2] - b[0]) / 2.0 * MM_PER_MIL,
        (b[3] - b[1]) / 2.0 * MM_PER_MIL,
    ];
    let board = &scene.board;
    let n = scene.copper_count.max(1) as usize;
    let last = n - 1;
    let st = stack(board, n);
    let t = st.half;
    let (mask, mask_cu, silk) = opt.mask.colors();
    // Inner copper only shows through clear laminate or a cut.
    let layer_color = |i: usize| -> [u8; 3] {
        if !opt.xray {
            return COPPER;
        }
        scene
            .layers
            .iter()
            .find(|l| l.slot == Slot::Copper(i as u8))
            .map_or(COPPER, |l| l.color)
    };
    // Top and side colour of copper on a layer.
    let cu_color = |i: usize| -> ([u8; 3], [u8; 3]) {
        if !opt.xray && (i == 0 || i == last) {
            (mask_cu, shade(mask_cu, 0.82))
        } else {
            let c = layer_color(i);
            (c, shade(c, 0.72))
        }
    };
    // Inner layers are built always: hidden by the laminate from outside,
    // but there when the camera goes in between the layers.
    let wanted = |i: usize| i < n;
    if let Some(cap) = bld.cap.as_mut() {
        cap.layers = (0..n)
            .map(|i| (st.cu[i], layer_color(i), Vec::new()))
            .collect();
        cap.slices = st
            .diel
            .iter()
            .map(|(z, core)| (*z, if *core { CORE } else { PREPREG }))
            .collect();
        // Resin fills the copper layers between the copper.
        for i in 1..last {
            cap.slices.push((st.cu[i], PREPREG));
        }
    }
    let g = &board.geometry;
    let mil = |l: trace_core::Length| l.mil() as f32;

    // Every hole through the laminate: centre (mm), drill radius, plated.
    let mut holes: Vec<([f32; 2], f32, bool)> = Vec::new();
    for pad in &g.pads {
        let r = mil(pad.hole) / 2.0 * MM_PER_MIL;
        if r > 0.0 {
            holes.push((bld.pt(mil(pad.at.x), mil(pad.at.y)), r, !pad.fab.unplated));
        }
    }
    for v in &g.vias {
        let r = mil(v.hole) / 2.0 * MM_PER_MIL;
        if r > 0.0 {
            holes.push((bld.pt(mil(v.at.x), mil(v.at.y)), r, true));
        }
    }

    // The same holes by layer span, for drilling pours and tracks.
    let mut drills = Drills::default();
    for pad in &g.pads {
        let r = mil(pad.hole) / 2.0 * MM_PER_MIL;
        if r > 0.0 {
            drills.add(bld.pt(mil(pad.at.x), mil(pad.at.y)), r, 0, 255);
        }
    }
    for v in &g.vias {
        let r = mil(v.hole) / 2.0 * MM_PER_MIL;
        if r > 0.0 {
            let (a, z) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
            drills.add(bld.pt(mil(v.at.x), mil(v.at.y)), r, a, z);
        }
    }

    // Laminate: top and bottom faces with the holes drilled, and the edge.
    let mut ring: Vec<[f32; 2]> = if scene.outline.len() >= 3 {
        scene.outline.iter().map(|p| bld.pt(p[0], p[1])).collect()
    } else {
        vec![
            bld.pt(b[0], b[1]),
            bld.pt(b[2], b[1]),
            bld.pt(b[2], b[3]),
            bld.pt(b[0], b[3]),
        ]
    };
    if ring.len() > 3 && ring.first() == ring.last() {
        ring.pop();
    }
    let mut rings = vec![ring.clone()];
    for s in &scene.slots {
        rings.push(s.iter().map(|p| bld.pt(p[0], p[1])).collect());
    }
    for &(c, r, _) in &holes {
        rings.push(circle_pts(c[0], c[1], r, segments(r).max(10)));
    }
    let (mut v, mut tr) = tessellate(&rings);
    if tr.is_empty() {
        // Overlapping holes can upset the tessellator: a plain slab then.
        (v, tr) = tessellate(std::slice::from_ref(&ring));
    }
    let [lam_bot, lam_top] = st.lam;
    bld.lam = st.lam;
    bld.tag = TAG_DIEL;
    bld.shine = if opt.xray { MATTE } else { MASK_GLOSS };
    bld.face(&v, &tr, lam_top, 1.0, mask);
    bld.face(&v, &tr, lam_bot, -1.0, mask);
    bld.shine = MATTE;
    for i in 0..ring.len() {
        let (a, c) = (ring[i], ring[(i + 1) % ring.len()]);
        bld.wall(a, c, lam_bot, lam_top, EDGE);
    }
    for s in rings[1..1 + scene.slots.len()].iter().cloned() {
        for i in 0..s.len() {
            bld.wall(s[i], s[(i + 1) % s.len()], lam_bot, lam_top, EDGE);
        }
    }
    if let Some(cap) = bld.cap.as_mut() {
        cap.board = tr
            .chunks_exact(3)
            .map(|t| [v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]])
            .collect();
    }
    bld.tag = TAG_MISC;
    for &(c, r, plated) in &holes {
        if !plated {
            bld.tube(c[0], c[1], r, lam_bot, lam_top, DRILL_WALL);
        }
    }

    let slot_layer = |s: Slot| match s {
        Slot::Copper(i) if (i as usize) < n && wanted(i as usize) => Some(i as usize),
        _ => None,
    };

    // Pours and planes.
    for f in &scene.fills {
        let fb = f.bbox;
        if fb[2] < b[0] || fb[0] > b[2] || fb[3] < b[1] || fb[1] > b[3] {
            continue;
        }
        let pts: Vec<[f32; 2]> = scene.fill_verts[f.verts.clone()]
            .iter()
            .map(|p| bld.pt(p[0], p[1]))
            .collect();
        let tris = &scene.fill_tris[f.tris.clone()];
        match f.slot {
            Slot::TopSilk | Slot::BottomSilk if opt.silk => {
                let z = if f.slot == Slot::TopSilk {
                    st.cu[0][1] + Z_SILK
                } else {
                    -(st.cu[last][0].abs() + Z_SILK)
                };
                bld.shine = INK_GLOSS;
                bld.face(&pts, tris, z, z.signum(), silk);
                bld.shine = MATTE;
            }
            s => {
                if let Some(i) = slot_layer(s) {
                    let (top, side) = cu_color(i);
                    bld.tag = layer_tag(i);
                    bld.shine = if top == mask_cu { MASK_GLOSS } else { METAL };
                    // Relief spokes run to the pad's centre: drill them.
                    let cut = drills.cut(&pts, tris, i as u8);
                    let (pts, tris) = match &cut {
                        Some((v, t)) => (v, t.as_slice()),
                        None => (&pts, tris),
                    };
                    bld.slab(pts, tris, st.cu[i][0], st.cu[i][1], top, side, Some(i));
                    bld.tag = TAG_MISC;
                    bld.shine = MATTE;
                }
            }
        }
    }

    // Tracks and arcs, silkscreen on top of the mask.
    for p in &scene.prims {
        if !matches!(p.source, Source::Track(_) | Source::Arc(_)) {
            continue;
        }
        // Drawing notes outside the board would float in the air.
        let pb = p.geom.bbox();
        if pb[2] < b[0] || pb[0] > b[2] || pb[3] < b[1] || pb[1] > b[3] {
            continue;
        }
        match p.slot {
            Slot::TopSilk | Slot::BottomSilk if opt.silk => {
                let z = if p.slot == Slot::TopSilk {
                    st.cu[0][1] + Z_SILK
                } else {
                    st.cu[last][0] - Z_SILK
                };
                bld.shine = INK_GLOSS;
                bld.geom(&p.geom, z, silk);
                bld.shine = MATTE;
            }
            s => {
                if let Some(i) = slot_layer(s) {
                    let (top, side) = cu_color(i);
                    let pts = bld.geom_ring(&p.geom);
                    bld.tag = layer_tag(i);
                    bld.shine = if top == mask_cu { MASK_GLOSS } else { METAL };
                    // A track ending on a via or pad stops at its hole.
                    let fan: Vec<u32> = (1..pts.len().max(2) as u32 - 1)
                        .flat_map(|k| [0, k, k + 1])
                        .collect();
                    match drills.cut(&pts, &fan, i as u8) {
                        Some((v, t)) => {
                            bld.slab(&v, &t, st.cu[i][0], st.cu[i][1], top, side, Some(i))
                        }
                        None => bld.convex_slab(&pts, st.cu[i][0], st.cu[i][1], top, side, Some(i)),
                    }
                    bld.tag = TAG_MISC;
                    bld.shine = MATTE;
                }
            }
        }
    }

    // Pads: copper lands with a gold finish where they are bare, drilled
    // through where they have a hole.
    let bare = |i: usize| (i == 0 || i == last) && !opt.xray;
    for pad in &g.pads {
        let (x, y) = (mil(pad.at.x), mil(pad.at.y));
        let hole = mil(pad.hole) / 2.0 * MM_PER_MIL;
        let mut lands: Vec<LandRef> = Vec::new();
        match &pad.layer {
            LayerRef::AllCopper => {
                for layer in 0..n {
                    if !wanted(layer) {
                        continue;
                    }
                    if pad.stack.is_empty() || last == 0 {
                        lands.push((
                            layer,
                            pad.shape,
                            mil(pad.size_x),
                            mil(pad.size_y),
                            pad.rotation_deg as f32,
                            &pad.outline,
                        ));
                    } else if let Some(l) = pad.stack.iter().find(|l| l.layer as usize == layer) {
                        lands.push((
                            layer,
                            l.shape,
                            mil(l.size_x),
                            mil(l.size_y),
                            l.rotation_deg as f32,
                            &l.outline,
                        ));
                    }
                }
            }
            LayerRef::Copper(i) if (*i as usize) < n && wanted(*i as usize) => {
                lands.push((
                    *i as usize,
                    pad.shape,
                    mil(pad.size_x),
                    mil(pad.size_y),
                    pad.rotation_deg as f32,
                    &pad.outline,
                ));
            }
            _ => continue,
        }
        for (layer, shape, sx, sy, rot, outline) in lands {
            let land: Vec<[f32; 2]> = if outline.len() >= 3 {
                outline.iter().map(|p| bld.pt(mil(p.x), mil(p.y))).collect()
            } else {
                bld.geom_ring(&pad_geom(x, y, shape, sx, sy, rot))
            };
            let [z0, mut z1] = st.cu[layer];
            let (top, side) = if bare(layer) {
                z1 += if layer == 0 { Z_FINISH } else { 0.0 };
                (FINISH, shade(COPPER, 0.85))
            } else {
                let c = layer_color(layer);
                (c, shade(c, 0.72))
            };
            let z0 = if bare(layer) && layer == last && last > 0 {
                z0 - Z_FINISH
            } else {
                z0
            };
            bld.tag = layer_tag(layer);
            bld.shine = METAL;
            if hole > 0.0 {
                let c = bld.pt(x, y);
                let rings = [land, circle_pts(c[0], c[1], hole, segments(hole).max(10))];
                let (v, tr) = tessellate(&rings);
                bld.slab(&v, &tr, z0, z1, top, side, Some(layer));
            } else if outline.len() >= 3 {
                let (v, tr) = tessellate(&[land]);
                bld.slab(&v, &tr, z0, z1, top, side, Some(layer));
            } else {
                bld.convex_slab(&land, z0, z1, top, side, Some(layer));
            }
        }
        bld.tag = TAG_MISC;
        bld.shine = MATTE;
        if hole > 0.0 && !pad.fab.unplated {
            let c = bld.pt(x, y);
            barrel(&mut bld, c, hole, st.cu[last][0], st.cu[0][1]);
        }
    }

    // Vias: annular rings on the layers they join, joined by a barrel.
    for v in &g.vias {
        let (x, y) = (mil(v.at.x), mil(v.at.y));
        let c = bld.pt(x, y);
        let hole = mil(v.hole) / 2.0 * MM_PER_MIL;
        let (lo, hi) = (
            v.from_layer.min(v.to_layer) as usize,
            (v.from_layer.max(v.to_layer) as usize).min(last),
        );
        let ring_on = |layer: usize| {
            v.rings
                .iter()
                .find(|r| r.0 as usize == layer)
                .map_or(mil(v.diameter), |r| mil(r.1))
                / 2.0
                * MM_PER_MIL
        };
        for layer in lo..=hi {
            if !wanted(layer) || v.drilled(layer as u8).is_some() {
                continue;
            }
            let r = ring_on(layer);
            if r <= hole + 1e-4 {
                continue;
            }
            let (top, side) = cu_color(layer);
            let rings = [
                circle_pts(c[0], c[1], r, segments(r).max(12)),
                circle_pts(c[0], c[1], hole, segments(hole).max(10)),
            ];
            let (vv, tr) = tessellate(&rings);
            bld.tag = layer_tag(layer);
            bld.shine = if top == mask_cu { MASK_GLOSS } else { METAL };
            bld.slab(
                &vv,
                &tr,
                st.cu[layer][0],
                st.cu[layer][1],
                top,
                side,
                Some(layer),
            );
        }
        bld.tag = TAG_MISC;
        bld.shine = MATTE;
        if hole > 0.0 {
            // A backdrilled stub has no barrel left.
            let top = v.fab.back_top.map_or(lo, |b| (b.keep as usize).min(last));
            let bot = v
                .fab
                .back_bottom
                .map_or(hi, |b| (b.keep as usize).min(last));
            if top <= bot {
                barrel(&mut bld, c, hole, st.cu[bot][0], st.cu[top][1]);
            }
        }
    }

    bld.tag = TAG_MISC;
    bld.shine = BODY_GLOSS;
    if opt.parts {
        let placed = place_models(&mut bld, scene, t, models);
        bld.m.parts = placed.len() + bodies(&mut bld, scene, t, &placed);
    }
    bld.shine = MATTE;
    if opt.fold > 0 {
        if let Some(f) = trace_core::flex::fold(board, opt.fold as f64 / 100.0) {
            fold_mesh(&mut bld, &f);
        }
    }
    bld.m.section = bld.cap.take().map(Arc::new);
    bld.m
}

/// Plating thickness of a barrel, mm.
const PLATING: f32 = 0.025;

/// A plated barrel lining a drill of radius r from z0 to z1.
fn barrel(bld: &mut Builder, c: [f32; 2], r: f32, z0: f32, z1: f32) {
    let ri = (r - PLATING).max(r * 0.5);
    bld.shine = METAL;
    if bld.cap.is_some() {
        bld.tube(c[0], c[1], ri, z0, z1, BARREL);
    } else {
        // Looking down a hole little light reaches the middle of the
        // barrel: bright at the rims, dark half way down.
        let d = (1.2 * ri).min((z1 - z0) / 2.0);
        let deep = shade(BARREL, 0.10);
        bld.tube_lit(c[0], c[1], ri, [z0, z0 + d], [BARREL, deep]);
        bld.tube_lit(c[0], c[1], ri, [z0 + d, z1 - d], [deep, deep]);
        bld.tube_lit(c[0], c[1], ri, [z1 - d, z1], [deep, BARREL]);
    }
    bld.tube(c[0], c[1], r, z0, z1, shade(BARREL, 0.8));
    bld.shine = MATTE;
    if let Some(cap) = bld.cap.as_mut() {
        cap.barrels.push((c, ri, r, [z0, z1]));
    }
}

/// The faces the section plane leaves: the laminate, the copper and the
/// barrels it cuts, drawn on the plane. `axis` 0 cuts across x at `pos`
/// mm, 1 across y; `keep` is the side kept (+1 or -1 along the axis).
fn caps(src: &CapSrc, axis: usize, pos: f32, keep: f32) -> Mesh {
    let mut m = Mesh::default();
    let o = 1 - axis;
    let spans = |tris: &[[[f32; 2]; 3]]| -> Vec<[f32; 2]> {
        let mut out: Vec<[f32; 2]> = Vec::new();
        for t in tris {
            let mut s = Vec::with_capacity(2);
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let (da, db) = (a[axis] - pos, b[axis] - pos);
                if (da < 0.0) != (db < 0.0) {
                    let k = da / (da - db);
                    s.push(a[o] + (b[o] - a[o]) * k);
                }
            }
            if s.len() == 2 {
                out.push([s[0].min(s[1]), s[0].max(s[1])]);
            }
        }
        merge(out)
    };
    // Facing the viewer on the cut-away side; copper a hair in front of
    // the laminate so the two do not fight.
    let nrm = {
        let mut n = [0.0f32; 3];
        n[axis] = -keep;
        n
    };
    let quad =
        |m: &mut Mesh, s: [f32; 2], z: [f32; 2], c: [u8; 3], tag: i8, ahead: f32, shine: u8| {
            let i0 = m.verts.len() as u32;
            for (a, h) in [(s[0], z[0]), (s[1], z[0]), (s[1], z[1]), (s[0], z[1])] {
                let mut p = [0.0f32; 3];
                p[axis] = pos - keep * ahead;
                p[o] = a;
                p[2] = h;
                let q = |v: f32| (v * 127.0).round() as i8;
                m.verts.push(Vert {
                    p,
                    c: [c[0], c[1], c[2], shine],
                    n: [q(nrm[0]), q(nrm[1]), q(nrm[2]), tag],
                });
            }
            m.idx
                .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
        };
    let board = spans(&src.board);
    for (z, c) in &src.slices {
        for s in &board {
            quad(&mut m, *s, *z, *c, TAG_MISC, 0.0, MATTE);
        }
    }
    for (i, (z, c, tris)) in src.layers.iter().enumerate() {
        for s in spans(tris) {
            quad(&mut m, s, *z, cut_copper(*c), layer_tag(i), 0.002, METAL);
        }
    }
    for &(c, ri, ro, z) in &src.barrels {
        let d = (c[axis] - pos).abs();
        if d >= ro {
            continue;
        }
        let so = (ro * ro - d * d).sqrt();
        let parts: Vec<[f32; 2]> = if d < ri {
            let si = (ri * ri - d * d).sqrt();
            vec![[c[o] - so, c[o] - si], [c[o] + si, c[o] + so]]
        } else {
            vec![[c[o] - so, c[o] + so]]
        };
        for s in parts {
            quad(&mut m, s, z, cut_copper(BARREL), TAG_MISC, 0.003, METAL);
        }
    }
    m
}

/// Freshly cut copper is brighter than its oxidised faces.
fn cut_copper(c: [u8; 3]) -> [u8; 3] {
    if c == COPPER || c == BARREL {
        [240, 158, 98]
    } else {
        c
    }
}

/// Joins overlapping spans.
fn merge(mut v: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    v.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(v.len());
    for s in v {
        match out.last_mut() {
            Some(l) if s[0] <= l[1] + 1e-6 => l[1] = l[1].max(s[1]),
            _ => out.push(s),
        }
    }
    out
}

/// Bends the mesh at the board's bends: cuts the triangles along lines
/// across each bend so it curves, then moves every vertex.
fn fold_mesh(bld: &mut Builder, f: &trace_core::flex::Fold) {
    let (cx, cy) = (bld.cx as f64, bld.cy as f64);
    let k = MM_PER_MIL as f64;
    let to_mil = |p: [f32; 3]| [p[0] as f64 / k + cx, p[1] as f64 / k + cy, p[2] as f64 / k];
    let m = &mut bld.m;
    for (o, n) in f.cuts(16) {
        let side = |v: &Vert| {
            let q = to_mil(v.p);
            (q[0] - o[0]) * n[0] + (q[1] - o[1]) * n[1]
        };
        let mut idx = Vec::with_capacity(m.idx.len());
        let old = std::mem::take(&mut m.idx);
        for t in old.chunks_exact(3) {
            let d = [
                side(&m.verts[t[0] as usize]),
                side(&m.verts[t[1] as usize]),
                side(&m.verts[t[2] as usize]),
            ];
            let eps = 1e-6;
            if d.iter().all(|&x| x >= -eps) || d.iter().all(|&x| x <= eps) {
                idx.extend_from_slice(t);
                continue;
            }
            let mut lo: Vec<u32> = Vec::with_capacity(4);
            let mut hi: Vec<u32> = Vec::with_capacity(4);
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let (da, db) = (d[e], d[(e + 1) % 3]);
                if da <= 0.0 {
                    lo.push(a);
                }
                if da >= 0.0 {
                    hi.push(a);
                }
                if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
                    let s = (da / (da - db)) as f32;
                    let (va, vb) = (m.verts[a as usize], m.verts[b as usize]);
                    let lerp = |x: f32, y: f32| x + (y - x) * s;
                    let v = Vert {
                        p: [
                            lerp(va.p[0], vb.p[0]),
                            lerp(va.p[1], vb.p[1]),
                            lerp(va.p[2], vb.p[2]),
                        ],
                        c: va.c,
                        n: va.n,
                    };
                    m.verts.push(v);
                    let i = (m.verts.len() - 1) as u32;
                    lo.push(i);
                    hi.push(i);
                }
            }
            for poly in [lo, hi] {
                for i in 1..poly.len().saturating_sub(1) {
                    idx.extend_from_slice(&[poly[0], poly[i], poly[i + 1]]);
                }
            }
        }
        m.idx = idx;
    }
    for v in &mut m.verts {
        let (q, turn) = f.map(to_mil(v.p));
        v.p = [
            ((q[0] - cx) * k) as f32,
            ((q[1] - cy) * k) as f32,
            (q[2] * k) as f32,
        ];
        let nn = turn.vec([
            v.n[0] as f64 / 127.0,
            v.n[1] as f64 / 127.0,
            v.n[2] as f64 / 127.0,
        ]);
        let q8 = |x: f64| (x * 127.0).round().clamp(-127.0, 127.0) as i8;
        v.n = [q8(nn[0]), q8(nn[1]), q8(nn[2]), v.n[3]];
    }
}

/// Copper layer, shape, size x and y, rotation and outline.
type LandRef<'a> = (usize, PadShape, f32, f32, f32, &'a Vec<trace_core::Point>);

fn pad_geom(x: f32, y: f32, shape: PadShape, sx: f32, sy: f32, rot: f32) -> Geom {
    if shape == PadShape::Round && (sx - sy).abs() < 1e-3 {
        Geom::Circle { x, y, r: sx / 2.0 }
    } else if shape == PadShape::Round {
        let (long, short) = (sx.max(sy), sx.min(sy));
        let ang = (rot + if sx >= sy { 0.0 } else { 90.0 }).to_radians();
        let h = (long - short) / 2.0;
        Geom::Seg {
            x1: x - h * ang.cos(),
            y1: y - h * ang.sin(),
            x2: x + h * ang.cos(),
            y2: y + h * ang.sin(),
            w: short,
        }
    } else {
        Geom::Rect { x, y, sx, sy, rot }
    }
}

/// Simple boxes for parts, sized from their pads. Returns how many.
/// Draws every part model that loaded. Returns the components drawn.
fn place_models(
    bld: &mut Builder,
    scene: &Scene,
    t: f32,
    models: &Models,
) -> std::collections::HashSet<u32> {
    let mut done = std::collections::HashSet::new();
    let (ox, oy) = (bld.cx * MM_PER_MIL, bld.cy * MM_PER_MIL);
    for body in &scene.board.geometry.bodies {
        let Some(Some(mesh)) = models.get(body.model as usize) else {
            continue;
        };
        if body.opacity <= 0.05
            || body
                .component
                .is_some_and(|c| !trace_core::variant::fitted(&scene.board, c as usize))
        {
            continue;
        }
        let [rx, ry, rz] = body.model_rotation_deg;
        let (rx, ry) = (rx.to_radians(), ry.to_radians());
        // Altium's z rotation already includes the part's own rotation. On
        // the bottom it turns the same way before the model is turned over.
        let rz = (rz - body.rotation_deg).to_radians();
        let rot = |a: f64, axis: usize| -> [[f64; 3]; 3] {
            let (s, c) = a.sin_cos();
            match axis {
                0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
                1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
                _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            }
        };
        let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
            let mut r = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
                }
            }
            r
        };
        let mut m = mul(rot(rz, 2), mul(rot(ry, 1), rot(rx, 0)));
        let dz = body.standoff.mm();
        let (x, y) = (body.at.x.mm() as f32 - ox, body.at.y.mm() as f32 - oy);
        let z0;
        if body.bottom {
            // Turned over about the board's x axis, hanging below it.
            m = mul([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]], m);
            z0 = -(t as f64) - dz;
        } else {
            z0 = t as f64 + dz;
        }
        let i0 = bld.m.verts.len() as u32;
        let q = |v: f64| (v * 127.0).round().clamp(-127.0, 127.0) as i8;
        for k in 0..mesh.positions.len() {
            let p = mesh.positions[k].map(f64::from);
            let n = mesh.normals[k].map(f64::from);
            let tp = |v: [f64; 3]| {
                [
                    m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
                    m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
                    m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
                ]
            };
            let (wp, wn) = (tp(p), tp(n));
            let c = mesh.colors[k];
            bld.m.verts.push(Vert {
                p: [wp[0] as f32 + x, wp[1] as f32 + y, (wp[2] + z0) as f32],
                c: [c[0], c[1], c[2], 40],
                n: [q(wn[0]), q(wn[1]), q(wn[2]), 0],
            });
        }
        bld.m.idx.extend(mesh.indices.iter().map(|i| i0 + i));
        if std::env::var_os("TRACE_DEBUG3D").is_some() {
            let vs = &bld.m.verts[i0 as usize..];
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for v in vs {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.p[k]);
                    hi[k] = hi[k].max(v.p[k]);
                }
            }
            let des = body
                .component
                .and_then(|c| scene.board.components.get(c as usize))
                .map_or("-", |c| c.designator.as_str());
            eprintln!(
                "{des} {} x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1} at {x:.1},{y:.1}",
                scene.board.geometry.models[body.model as usize].name,
                lo[0],
                hi[0],
                lo[1],
                hi[1],
                lo[2],
                hi[2]
            );
        }
        if let Some(c) = body.component {
            done.insert(c);
        }
    }
    done
}

/// Simple boxes for parts without a model, sized from their pads. Returns
/// how many.
fn bodies(
    bld: &mut Builder,
    scene: &Scene,
    t: f32,
    skip: &std::collections::HashSet<u32>,
) -> usize {
    let board = &scene.board;
    let g = &board.geometry;
    let mut pads_of: Vec<Vec<usize>> = vec![Vec::new(); board.components.len()];
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(c) = p.component.and_then(|c| pads_of.get_mut(c as usize)) {
            c.push(i);
        }
    }
    let mut count = 0;
    for (ci, comp) in board.components.iter().enumerate() {
        let pads = &pads_of[ci];
        if pads.is_empty() || skip.contains(&(ci as u32)) || !trace_core::variant::fitted(board, ci)
        {
            continue;
        }
        let des = comp.designator.to_ascii_uppercase();
        let prefix: String = des
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if matches!(
            prefix.as_str(),
            "FID" | "FD" | "TP" | "MH" | "H" | "MK" | "LOGO" | "J_TP" | "SJ" | "JP" | "NT"
        ) {
            continue;
        }
        let rot = comp.rotation_deg as f32;
        let (s, c) = (-rot.to_radians()).sin_cos();
        // Pad extents in the part's own frame (mm).
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        let mut centres = Vec::with_capacity(pads.len());
        let mut through = false;
        let mut bottom_only = true;
        for &pi in pads {
            let p = &g.pads[pi];
            let (x, y) = (p.at.x.mm() as f32, p.at.y.mm() as f32);
            let (u, v) = (x * c - y * s, x * s + y * c);
            let rel = (p.rotation_deg as f32 - rot).rem_euclid(180.0);
            let (mut hx, mut hy) = (p.size_x.mm() as f32 / 2.0, p.size_y.mm() as f32 / 2.0);
            if (rel - 90.0).abs() < 30.0 {
                std::mem::swap(&mut hx, &mut hy);
            } else if (30.0..=150.0).contains(&rel) {
                let m = hx.max(hy);
                hx = m;
                hy = m;
            }
            lo = [lo[0].min(u - hx), lo[1].min(v - hy)];
            hi = [hi[0].max(u + hx), hi[1].max(v + hy)];
            centres.push([u, v, hx.min(hy) * 2.0, hx.max(hy) * 2.0]);
            through |= p.hole.0 > 0;
            if p.layer != LayerRef::Copper(scene.copper_count.saturating_sub(1)) {
                bottom_only = false;
            }
        }
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        if !(w > 0.1 && h > 0.1) || w.max(h) > 120.0 {
            continue;
        }
        let bottom = comp.side == Side::Bottom || (bottom_only && !through);
        let box_lo;
        let box_hi;
        let height;
        let mut col = [28, 28, 30];
        if pads.len() <= 3 && !through {
            // Chip parts: the body spans the terminals, a little narrower.
            let (sx, sy) = if w >= h {
                (0.0, h * 0.06)
            } else {
                (w * 0.06, 0.0)
            };
            box_lo = [lo[0] + sx, lo[1] + sy];
            box_hi = [hi[0] - sx, hi[1] - sy];
            let short = w.min(h);
            height = (short * 0.55).clamp(0.25, 2.5);
            col = match prefix.as_str() {
                "C" => [196, 160, 110],
                "L" | "FB" => [70, 70, 74],
                "D" | "LED" => [40, 40, 44],
                _ => [24, 24, 26],
            };
            if pads.len() == 2 {
                // Silver end caps.
                let cap = 0.18 * w.max(h);
                let (a_hi, b_lo) = if w >= h {
                    ([box_lo[0] + cap, box_hi[1]], [box_hi[0] - cap, box_lo[1]])
                } else {
                    ([box_hi[0], box_lo[1] + cap], [box_lo[0], box_hi[1] - cap])
                };
                let metal = [200, 200, 205];
                let z0 = if bottom { -t } else { t };
                let z1 = if bottom { -(t + height) } else { t + height };
                let ca = corners([box_lo, a_hi], s, c);
                let cb = corners([b_lo, box_hi], s, c);
                bld.block(&to_board(bld, ca), z0, z1, metal);
                bld.block(&to_board(bld, cb), z0, z1, metal);
                let mid = if w >= h {
                    [[box_lo[0] + cap, box_lo[1]], [box_hi[0] - cap, box_hi[1]]]
                } else {
                    [[box_lo[0], box_lo[1] + cap], [box_hi[0], box_hi[1] - cap]]
                };
                bld.block(&to_board(bld, corners(mid, s, c)), z0, z1, col);
                count += 1;
                continue;
            }
        } else {
            // Leads around the edge (SOIC, QFP) sit outside the body; an
            // array of pads (BGA, QFN centre pad) sits under it.
            let inner = centres.iter().any(|q| {
                q[0] > lo[0] + w * 0.3
                    && q[0] < hi[0] - w * 0.3
                    && q[1] > lo[1] + h * 0.3
                    && q[1] < hi[1] - h * 0.3
            });
            let lead = centres.iter().map(|q| q[3]).fold(0.0f32, f32::max);
            if through {
                box_lo = [lo[0] - 0.3, lo[1] - 0.3];
                box_hi = [hi[0] + 0.3, hi[1] + 0.3];
            } else if inner || pads.len() > 64 {
                box_lo = [lo[0] - 0.2, lo[1] - 0.2];
                box_hi = [hi[0] + 0.2, hi[1] + 0.2];
            } else {
                let i = (lead * 0.7).min(w.min(h) * 0.25);
                let on_x = centres
                    .iter()
                    .any(|q| (q[0] - lo[0]).abs() < lead || (hi[0] - q[0]).abs() < lead);
                let on_y = centres
                    .iter()
                    .any(|q| (q[1] - lo[1]).abs() < lead || (hi[1] - q[1]).abs() < lead);
                let ix = if on_x { i } else { 0.0 };
                let iy = if on_y { i } else { 0.0 };
                box_lo = [lo[0] + ix, lo[1] + iy];
                box_hi = [hi[0] - ix, hi[1] - iy];
            }
            let area = (box_hi[0] - box_lo[0]) * (box_hi[1] - box_lo[1]);
            height = if through {
                (area.sqrt() * 0.35).clamp(2.0, 8.0)
            } else {
                (0.6 + area.sqrt() * 0.06).clamp(0.6, 2.6)
            };
            if matches!(prefix.as_str(), "J" | "P" | "CN" | "CON" | "X") {
                col = [205, 198, 180];
            }
        }
        let z0 = if bottom { -t } else { t };
        let z1 = if bottom { -(t + height) } else { t + height };
        let cs = corners([box_lo, box_hi], s, c);
        bld.block(&to_board(bld, cs), z0, z1, col);
        // Pin 1 dot on larger parts.
        if pads.len() > 3 && !through {
            if let Some(p1) = pads
                .iter()
                .map(|&i| &g.pads[i])
                .find(|p| p.name == "1" || p.name.eq_ignore_ascii_case("A1"))
            {
                let (x, y) = (p1.at.x.mm() as f32, p1.at.y.mm() as f32);
                let (u, v) = (x * c - y * s, x * s + y * c);
                let size = (box_hi[0] - box_lo[0]).min(box_hi[1] - box_lo[1]);
                let r = (size * 0.06).clamp(0.15, 0.6);
                let inset = |x: f32, lo: f32, hi: f32| {
                    if hi - lo > 4.0 * r {
                        x.clamp(lo + 2.0 * r, hi - 2.0 * r)
                    } else {
                        (lo + hi) / 2.0
                    }
                };
                let u = inset(u, box_lo[0], box_hi[0]);
                let v = inset(v, box_lo[1], box_hi[1]);
                let (bx, by) = (u * c + v * s, -u * s + v * c);
                let z = z1 + if bottom { -0.01 } else { 0.01 };
                let [px, py] = [bx - bld.cx * MM_PER_MIL, by - bld.cy * MM_PER_MIL];
                let n = 16;
                let pts: Vec<[f32; 2]> = (0..n)
                    .map(|k| {
                        let a = k as f32 / n as f32 * std::f32::consts::TAU;
                        [px + r * a.cos(), py + r * a.sin()]
                    })
                    .collect();
                bld.fan(&pts, z, [150, 150, 150]);
            }
        }
        count += 1;
    }
    count
}

/// Corners of an axis-aligned box in the part frame, back in board mm.
fn corners(b: [[f32; 2]; 2], s: f32, c: f32) -> [[f32; 2]; 4] {
    let back = |u: f32, v: f32| [u * c + v * s, -u * s + v * c];
    [
        back(b[0][0], b[0][1]),
        back(b[1][0], b[0][1]),
        back(b[1][0], b[1][1]),
        back(b[0][0], b[1][1]),
    ]
}

/// Board mm to the mesh's centred mm.
fn to_board(bld: &Builder, p: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let (ox, oy) = (bld.cx * MM_PER_MIL, bld.cy * MM_PER_MIL);
    p.map(|q| [q[0] - ox, q[1] - oy])
}

// ---- Camera ---------------------------------------------------------------

type Mat4 = [f32; 16];

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: [f32; 3],
}

const FOV: f32 = 0.6;

impl Camera {
    pub fn fit(&mut self, radius: f32) {
        self.yaw = if self.pitch < 0.0 {
            std::f32::consts::PI
        } else {
            0.0
        };
        self.pitch = if self.pitch < 0.0 { -0.95 } else { 0.95 };
        self.target = [0.0, 0.0, 0.0];
        self.dist = radius / (FOV / 2.0).tan() * 0.9;
    }

    fn eye_dir(&self) -> [f32; 3] {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        [cp * sy, -cp * cy, sp]
    }

    fn eye(&self) -> [f32; 3] {
        let d = self.eye_dir();
        [
            self.target[0] + d[0] * self.dist,
            self.target[1] + d[1] * self.dist,
            self.target[2] + d[2] * self.dist,
        ]
    }

    /// Camera right and up vectors.
    fn basis(&self) -> ([f32; 3], [f32; 3]) {
        let f = norm(sub(self.target, self.eye()));
        let s = norm(cross(f, [0.0, 0.0, 1.0]));
        (s, cross(s, f))
    }

    fn matrix(&self, aspect: f32, radius: f32) -> Mat4 {
        let eye = self.eye();
        let f = norm(sub(self.target, eye));
        let (s, u) = self.basis();
        let view = [
            s[0],
            u[0],
            -f[0],
            0.0,
            s[1],
            u[1],
            -f[1],
            0.0,
            s[2],
            u[2],
            -f[2],
            0.0,
            -dot(s, eye),
            -dot(u, eye),
            dot(f, eye),
            1.0,
        ];
        let near = (self.dist - radius * 1.5).max(self.dist * 0.05);
        let far = self.dist + radius * 3.0;
        let t = 1.0 / (FOV / 2.0).tan();
        let proj = [
            t / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            t,
            0.0,
            0.0,
            0.0,
            0.0,
            (far + near) / (near - far),
            -1.0,
            0.0,
            0.0,
            2.0 * far * near / (near - far),
            0.0,
        ];
        mul(&proj, &view)
    }
}

// ---- GPU ------------------------------------------------------------------

struct Buffers {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    count: i32,
}

struct Gpu {
    program: glow::Program,
    board: Buffers,
    caps: Buffers,
    u_mvp: Option<glow::UniformLocation>,
    u_eye: Option<glow::UniformLocation>,
    u_cam: Option<glow::UniformLocation>,
    u_mode: Option<glow::UniformLocation>,
    u_clip: Option<glow::UniformLocation>,
    u_hide: Option<glow::UniformLocation>,
}

#[derive(Default)]
struct Shared {
    gpu: Option<Gpu>,
    upload: Option<Mesh>,
    caps: Option<Mesh>,
    error: Option<String>,
}

/// Part models being tessellated in the background.
struct Loader {
    done: AtomicUsize,
    total: usize,
    result: Mutex<Option<Vec<Option<Arc<trace_step::Mesh>>>>>,
}

fn start_loader(models: &[trace_core::Model3d]) -> Arc<Loader> {
    let loader = Arc::new(Loader {
        done: AtomicUsize::new(0),
        total: models.len(),
        result: Mutex::new(None),
    });
    let steps: Arc<Vec<Arc<Vec<u8>>>> = Arc::new(models.iter().map(|m| m.step.clone()).collect());
    let slots: Arc<Mutex<Vec<Option<Arc<trace_step::Mesh>>>>> =
        Arc::new(Mutex::new(vec![None; steps.len()]));
    let next = Arc::new(AtomicUsize::new(0));
    let workers = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 6);
    let finished = Arc::new(AtomicUsize::new(0));
    for _ in 0..workers {
        let (steps, slots, next, l, finished) = (
            steps.clone(),
            slots.clone(),
            next.clone(),
            loader.clone(),
            finished.clone(),
        );
        std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(step) = steps.get(i).cloned() else {
                    break;
                };
                // A model the reader chokes on must not hold up the others.
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let r = std::panic::catch_unwind(|| trace_step::mesh(&step).ok())
                        .ok()
                        .flatten();
                    let _ = tx.send(r);
                });
                let mesh = rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .ok()
                    .flatten();
                if let Ok(mut s) = slots.lock() {
                    s[i] = mesh.map(Arc::new);
                }
                l.done.fetch_add(1, Ordering::Relaxed);
            }
            // The last worker out hands over the results.
            if finished.fetch_add(1, Ordering::AcqRel) + 1 == workers {
                let out = slots
                    .lock()
                    .map(|mut s| std::mem::take(&mut *s))
                    .unwrap_or_default();
                if let Ok(mut r) = l.result.lock() {
                    *r = Some(out);
                }
            }
        });
    }
    loader
}

/// Where the section plane cuts: across x (axis 0) or y (1) at `pos` mm
/// from the board's centre, keeping the low side unless flipped.
#[derive(Clone, Copy, PartialEq)]
pub struct Section {
    pub axis: usize,
    pub pos: f32,
    pub flip: bool,
}

impl Section {
    /// The side kept, +1 or -1 along the axis.
    fn keep(&self) -> f32 {
        if self.flip {
            1.0
        } else {
            -1.0
        }
    }
}

pub struct View3d {
    pub camera: Camera,
    pub options: Options,
    pub section: Section,
    /// Copper layers hidden, by index.
    pub hidden: [bool; 64],
    built: Option<(Options, bool, u64)>,
    loader: Option<Arc<Loader>>,
    models: Option<Arc<Vec<Option<Arc<trace_step::Mesh>>>>>,
    model_count: usize,
    radius: f32,
    half: [f32; 2],
    parts: usize,
    cut_src: Option<Arc<CapSrc>>,
    cut_built: Option<(Section, usize)>,
    /// Camera set from `apply_spec`, applied once the board is fitted:
    /// yaw, pitch (degrees), zoom and target (mm).
    pending_cam: Option<(f32, f32, f32, [f32; 3])>,
    shared: Arc<Mutex<Shared>>,
}

impl Default for View3d {
    fn default() -> Self {
        View3d {
            camera: Camera {
                yaw: 0.0,
                pitch: 0.95,
                dist: 100.0,
                target: [0.0; 3],
            },
            options: Options::default(),
            section: Section {
                axis: 0,
                pos: 0.0,
                flip: false,
            },
            hidden: [false; 64],
            built: None,
            loader: None,
            models: None,
            model_count: 0,
            radius: 50.0,
            half: [25.0, 25.0],
            parts: 0,
            cut_src: None,
            cut_built: None,
            pending_cam: None,
            shared: Arc::default(),
        }
    }
}

impl View3d {
    /// Forget the mesh so it is rebuilt for a new board.
    pub fn reset(&mut self) {
        self.built = None;
        self.loader = None;
        self.models = None;
        self.hidden = [false; 64];
        self.section.pos = 0.0;
    }

    /// Sets the view from a spec such as
    /// `xray,section,y,flip,pos=3.5,hide=2-13,yaw=30,pitch=40,zoom=2,at=1;2;0`
    /// (for screenshots and tests; `TRACE_OPEN=3d:<spec>`).
    pub fn apply_spec(&mut self, spec: &str) {
        let (mut yaw, mut pitch, mut zoom, mut at) = (0.0, 55.0, 1.0, [0.0f32; 3]);
        let mut cam = false;
        for w in spec.split(',').map(str::trim) {
            let (k, val) = w.split_once('=').unwrap_or((w, ""));
            let f = val.parse::<f32>().unwrap_or(0.0);
            match k {
                "xray" => self.options.xray = true,
                "section" => self.options.section = true,
                "y" => self.section.axis = 1,
                "flip" => self.section.flip = true,
                "pos" => self.section.pos = f,
                "noparts" => self.options.parts = false,
                "nosilk" => self.options.silk = false,
                "hide" => {
                    for r in val.split(';') {
                        let (a, b) = r.split_once('-').unwrap_or((r, r));
                        if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                            for i in a..=b.min(63) {
                                self.hidden[i] = true;
                            }
                        }
                    }
                }
                "yaw" => (yaw, cam) = (f, true),
                "pitch" => (pitch, cam) = (f, true),
                "zoom" => (zoom, cam) = (f.max(0.01), true),
                "at" => {
                    for (i, c) in val.split(';').take(3).enumerate() {
                        at[i] = c.parse().unwrap_or(0.0);
                    }
                    cam = true;
                }
                _ => {}
            }
        }
        if cam {
            self.pending_cam = Some((yaw, pitch, zoom, at));
        }
    }

    /// Build the mesh again from the same board (parts fitted changed).
    pub fn rebuild(&mut self) {
        self.built = None;
    }

    /// The board's part models meshed, read first if need be: how far the
    /// reading has got (done, of) while it runs.
    pub fn meshes(&mut self, models: &[trace_core::Model3d]) -> Result<Meshes, (usize, usize)> {
        if let Some(m) = &self.models {
            return Ok(m.clone());
        }
        let l = self
            .loader
            .get_or_insert_with(|| start_loader(models))
            .clone();
        if let Some(r) = l.result.lock().ok().and_then(|mut r| r.take()) {
            self.model_count = r.iter().filter(|m| m.is_some()).count();
            let a = Arc::new(r);
            self.models = Some(a.clone());
            self.loader = None;
            return Ok(a);
        }
        Err((l.done.load(Ordering::Relaxed), l.total))
    }

    pub fn destroy(&self, gl: &glow::Context) {
        if let Some(g) = self.shared.lock().ok().and_then(|mut s| s.gpu.take()) {
            unsafe {
                gl.delete_program(g.program);
                for b in [g.board, g.caps] {
                    gl.delete_vertex_array(b.vao);
                    gl.delete_buffer(b.vbo);
                    gl.delete_buffer(b.ebo);
                }
            }
        }
    }
}

unsafe fn buffers(gl: &glow::Context) -> Result<Buffers, String> {
    let vao = gl.create_vertex_array()?;
    let vbo = gl.create_buffer()?;
    let ebo = gl.create_buffer()?;
    gl.bind_vertex_array(Some(vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
    let stride = std::mem::size_of::<Vert>() as i32;
    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_pointer_f32(1, 4, glow::UNSIGNED_BYTE, true, stride, 12);
    gl.enable_vertex_attrib_array(2);
    gl.vertex_attrib_pointer_f32(2, 4, glow::BYTE, true, stride, 16);
    gl.bind_vertex_array(None);
    Ok(Buffers {
        vao,
        vbo,
        ebo,
        count: 0,
    })
}

unsafe fn init(gl: &glow::Context) -> Result<Gpu, String> {
    let ver = eframe::egui_glow::ShaderVersion::get(gl);
    let new = ver.is_new_shader_interface();
    let header = format!(
        "{}{}",
        ver.version_declaration(),
        if ver.is_embedded() {
            "precision highp float;\n"
        } else {
            ""
        }
    );
    let (vin, vout, fin) = if new {
        ("in", "out", "in")
    } else {
        ("attribute", "varying", "varying")
    };
    let vs = format!(
        "{header}
{vin} vec3 a_pos;
{vin} vec4 a_col;
{vin} vec4 a_nrm;
uniform mat4 u_mvp;
uniform float u_hide[64];
{vout} vec3 v_col;
{vout} vec3 v_nrm;
{vout} vec3 v_pos;
{vout} float v_shine;
{vout} float v_tag;
{vout} float v_hide;
void main() {{
    gl_Position = u_mvp * vec4(a_pos, 1.0);
    v_col = a_col.rgb;
    v_shine = a_col.a;
    v_nrm = a_nrm.xyz;
    v_pos = a_pos;
    float tag = floor(a_nrm.w * 127.0 + 0.5);
    v_tag = tag;
    v_hide = 0.0;
    if (tag >= 1.0 && tag <= 64.0) {{
        v_hide = u_hide[int(tag) - 1];
    }}
}}
"
    );
    let fs = format!(
        "{header}
{fin} vec3 v_col;
{fin} vec3 v_nrm;
{fin} vec3 v_pos;
{fin} float v_shine;
{fin} float v_tag;
{fin} float v_hide;
uniform vec3 u_eye;
uniform vec3 u_cam;
uniform vec4 u_mode;
uniform vec4 u_clip;
{out}
void main() {{
    if (v_hide > 0.5) discard;
    if (u_clip.w > 0.5 && dot(v_pos.xy, u_clip.xy) > u_clip.z) discard;
    bool diel = v_tag > 99.5;
    bool solid = v_tag < 0.5;
    if (u_mode.x > 0.5 && u_mode.x < 1.5 && !solid) discard;
    if (u_mode.x > 1.5 && solid) discard;
    vec3 n = normalize(v_nrm);
    // View from the camera to this point (per pixel, so a highlight
    // slides across a flat board as it turns).
    vec3 v = normalize(u_cam - v_pos);
    vec3 col = v_col;
    // The laminate seen from between the layers: bare glass-epoxy.
    bool back = abs(n.z) > 0.9 && dot(n, v) < 0.0;
    if (diel && back) col = vec3(0.66, 0.58, 0.38);
    // The far face of a copper layer is always behind its near face; left
    // out, it cannot show through as stripes when seen edge on.
    if (back && v_tag > 0.5 && v_tag < 64.5) discard;
    if (dot(n, v) < 0.0) n = -n;
    vec3 key = normalize(vec3(-0.35, -0.45, 0.82));
    vec3 fill = normalize(vec3(0.55, 0.35, 0.45));
    float ne = max(dot(n, v), 0.0);
    float l = 0.30 + 0.34 * ne + 0.30 * abs(dot(n, key)) + 0.10 * abs(dot(n, fill))
        + 0.06 * n.z;
    vec3 base = col * min(l, 1.15);
    // A soft studio sky: bright overhead, dim at the horizon and below.
    vec3 r = reflect(-v, n);
    float up = clamp(r.z * 0.5 + 0.5, 0.0, 1.0);
    vec3 sky = mix(vec3(0.10, 0.11, 0.13), vec3(1.0, 1.0, 0.98), up * up);
    // A soft-box light where the board mirrors it to the viewer, so a
    // sheen lies over the far half of the board and moves as it turns,
    // and the fixed key light.
    vec3 head = normalize(vec3(-u_eye.x, -u_eye.y, abs(u_eye.z) + 0.3));
    float hb = max(dot(r, head), 0.0);
    float kb = max(dot(r, key), 0.0);
    vec3 c;
    if (v_shine > 0.9) {{
        // Metal: tinted reflections of the sky and lights.
        vec3 env = col * (0.30 + 0.85 * sky);
        float sp = 0.55 * pow(hb, 40.0) + 0.5 * pow(kb, 90.0);
        c = mix(base, env, 0.5) + col * (0.8 * sp + 0.10 * pow(hb, 6.0)) + vec3(0.15 * sp);
    }} else {{
        // Coloured surface under a clear gloss: white highlights, more
        // reflection at grazing angles (Fresnel).
        float g = v_shine / 0.6;
        float fr = 0.05 + 0.95 * pow(1.0 - ne, 5.0);
        float sp = 0.035 * pow(hb, 6.0) + 0.10 * pow(hb, 60.0 * g + 10.0)
            + 0.20 * pow(kb, 80.0);
        c = base * (1.0 - 0.4 * fr * g) + g * (fr * 0.5 * sky + vec3(sp));
    }}
    float a = u_mode.x > 1.5 ? (diel ? u_mode.y : u_mode.z) : 1.0;
    {frag} = vec4(c, a);
}}
",
        out = if new { "out vec4 o_col;" } else { "" },
        frag = if new { "o_col" } else { "gl_FragColor" },
    );
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let sh = gl.create_shader(kind)?;
        gl.shader_source(sh, &src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            return Err(gl.get_shader_info_log(sh));
        }
        gl.attach_shader(program, sh);
        shaders.push(sh);
    }
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.bind_attrib_location(program, 1, "a_col");
    gl.bind_attrib_location(program, 2, "a_nrm");
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    for sh in shaders {
        gl.detach_shader(program, sh);
        gl.delete_shader(sh);
    }
    Ok(Gpu {
        u_mvp: gl.get_uniform_location(program, "u_mvp"),
        u_eye: gl.get_uniform_location(program, "u_eye"),
        u_cam: gl.get_uniform_location(program, "u_cam"),
        u_mode: gl.get_uniform_location(program, "u_mode"),
        u_clip: gl.get_uniform_location(program, "u_clip"),
        u_hide: gl
            .get_uniform_location(program, "u_hide[0]")
            .or_else(|| gl.get_uniform_location(program, "u_hide")),
        board: buffers(gl)?,
        caps: buffers(gl)?,
        program,
    })
}

unsafe fn upload(gl: &glow::Context, b: &mut Buffers, m: &Mesh) {
    let bytes = |p: *const u8, n: usize| std::slice::from_raw_parts(p, n);
    gl.bind_vertex_array(Some(b.vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(b.vbo));
    gl.buffer_data_u8_slice(
        glow::ARRAY_BUFFER,
        bytes(
            m.verts.as_ptr() as *const u8,
            std::mem::size_of_val(m.verts.as_slice()),
        ),
        glow::STATIC_DRAW,
    );
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(b.ebo));
    gl.buffer_data_u8_slice(
        glow::ELEMENT_ARRAY_BUFFER,
        bytes(
            m.idx.as_ptr() as *const u8,
            std::mem::size_of_val(m.idx.as_slice()),
        ),
        glow::STATIC_DRAW,
    );
    gl.bind_vertex_array(None);
    b.count = m.idx.len() as i32;
}

unsafe fn draw(gl: &glow::Context, b: &Buffers) {
    if b.count > 0 {
        gl.bind_vertex_array(Some(b.vao));
        gl.draw_elements(glow::TRIANGLES, b.count, glow::UNSIGNED_INT, 0);
        gl.bind_vertex_array(None);
    }
}

/// How clear the laminate and mask are in X-ray (alpha).
const XRAY_ALPHA: f32 = 0.10;
/// How solid copper is in X-ray, so the layers under it show too.
const XRAY_COPPER: f32 = 0.5;

/// Draws the 3D view in the central panel.
pub fn show(v: &mut View3d, scene: &Scene, ui: &mut egui::Ui) {
    let g = &scene.board.geometry;
    if v.options.parts && v.loader.is_none() && v.models.is_none() && !g.bodies.is_empty() {
        v.loader = Some(start_loader(&g.models));
    }
    if let Some(l) = &v.loader {
        let res = l.result.lock().ok().and_then(|mut r| r.take());
        if let Some(r) = res {
            v.model_count = r.iter().filter(|m| m.is_some()).count();
            v.models = Some(Arc::new(r));
            v.loader = None;
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(300));
        }
    }
    let with_models = v.models.is_some();
    let bends = &scene.board.bends;
    let fold_sig = if v.options.fold == 0 || bends.is_empty() {
        0
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for b in bends {
            b.angle_deg.to_bits().hash(&mut h);
            b.axis_deg.map(f64::to_bits).hash(&mut h);
            for p in &b.area {
                p.x.mil().to_bits().hash(&mut h);
                p.y.mil().to_bits().hash(&mut h);
            }
        }
        h.finish() | 1
    };
    if v.built != Some((v.options, with_models, fold_sig)) {
        let first = v.built.is_none();
        let none: Vec<Option<Arc<trace_step::Mesh>>> = Vec::new();
        let models = v.models.clone();
        let t0 = std::time::Instant::now();
        let mut mesh = build(
            scene,
            v.options,
            models.as_deref().map_or(&none[..], |m| &m[..]),
        );
        if std::env::var_os("TRACE_DEBUG3D").is_some() {
            eprintln!(
                "3D mesh: {} vertices, {} triangles in {:.0} ms",
                mesh.verts.len(),
                mesh.idx.len() / 3,
                t0.elapsed().as_secs_f64() * 1e3
            );
        }
        v.radius = mesh.radius;
        v.half = mesh.half;
        v.parts = mesh.parts;
        v.cut_src = mesh.section.take();
        v.cut_built = None;
        if first {
            v.camera.fit(mesh.radius);
            if let Some((yaw, pitch, zoom, at)) = v.pending_cam.take() {
                v.camera.yaw = yaw.to_radians();
                v.camera.pitch = pitch.to_radians().clamp(-1.5605, 1.5605);
                v.camera.dist /= zoom;
                v.camera.target = at;
            }
        }
        if let Ok(mut s) = v.shared.lock() {
            s.upload = Some(mesh);
        }
        v.built = Some((v.options, with_models, fold_sig));
    }
    let cutting = v.options.section && v.options.fold == 0;
    if let (true, Some(src)) = (cutting, v.cut_src.clone()) {
        let key = (v.section, Arc::as_ptr(&src) as usize);
        if v.cut_built != Some(key) {
            let s = v.section;
            let m = caps(&src, s.axis, s.pos, s.keep());
            if let Ok(mut sh) = v.shared.lock() {
                sh.caps = Some(m);
            }
            v.cut_built = Some(key);
        }
    }

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let cam = &mut v.camera;
    let (mods, scroll, keys) = ui.input(|i| {
        (
            i.modifiers,
            i.smooth_scroll_delta.y,
            (
                i.key_pressed(egui::Key::T),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::X),
            ),
        )
    });
    let typing = ui.ctx().wants_keyboard_input();
    let pan = resp.dragged_by(egui::PointerButton::Secondary)
        || resp.dragged_by(egui::PointerButton::Middle)
        || (resp.dragged_by(egui::PointerButton::Primary) && mods.shift);
    let d = resp.drag_delta();
    if pan {
        let (s, u) = cam.basis();
        let k = 2.0 * cam.dist * (FOV / 2.0).tan() / rect.height().max(1.0);
        for i in 0..3 {
            cam.target[i] += (-s[i] * d.x + u[i] * d.y) * k;
        }
    } else if resp.dragged_by(egui::PointerButton::Primary) {
        cam.yaw -= d.x * 0.008;
        cam.pitch = (cam.pitch + d.y * 0.008).clamp(-1.5605, 1.5605);
    }
    if resp.hovered() && scroll != 0.0 {
        cam.dist = (cam.dist * (-scroll * 0.0015).exp()).clamp(v.radius * 0.003, v.radius * 30.0);
    }
    if resp.double_clicked() || (!typing && keys.2) {
        cam.fit(v.radius);
    }
    if !typing && keys.0 {
        cam.yaw = 0.0;
        cam.pitch = 1.5605;
    }
    if !typing && keys.1 {
        // Turn the board over left to right, as when flipping it by hand.
        cam.pitch = -cam.pitch;
        cam.yaw += std::f32::consts::PI;
        if cam.pitch.abs() < 0.2 {
            cam.pitch = -0.95;
        }
    }
    if !typing && keys.3 {
        v.options.xray = !v.options.xray;
    }

    let painter = ui.painter_at(rect);
    // Backdrop: a studio gradient, lighter at the top.
    let mut bg = egui::Mesh::default();
    let (hi, lo) = (Color32::from_rgb(62, 68, 80), Color32::from_rgb(22, 24, 30));
    bg.colored_vertex(rect.left_top(), hi);
    bg.colored_vertex(rect.right_top(), hi);
    bg.colored_vertex(rect.right_bottom(), lo);
    bg.colored_vertex(rect.left_bottom(), lo);
    bg.add_triangle(0, 1, 2);
    bg.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(bg));
    let mvp = cam.matrix(rect.width() / rect.height().max(1.0), v.radius);
    let eye = cam.eye_dir();
    let eye_at = cam.eye();
    let shared = v.shared.clone();
    let xray = v.options.xray;
    let clip = if cutting {
        let s = v.section;
        let k = -s.keep();
        let mut c = [0.0f32; 4];
        c[s.axis] = k;
        c[2] = k * s.pos;
        c[3] = 1.0;
        c
    } else {
        [0.0; 4]
    };
    let hide: Vec<f32> = v
        .hidden
        .iter()
        .map(|&h| if h { 1.0 } else { 0.0 })
        .collect();
    let cb = eframe::egui_glow::CallbackFn::new(move |_info, painter| {
        let gl = painter.gl();
        let Ok(mut s) = shared.lock() else { return };
        unsafe {
            if s.gpu.is_none() && s.error.is_none() {
                match init(gl) {
                    Ok(g) => s.gpu = Some(g),
                    Err(e) => s.error = Some(e),
                }
            }
            let Shared {
                gpu,
                upload: up,
                caps: cup,
                ..
            } = &mut *s;
            let Some(gpu) = gpu.as_mut() else { return };
            if let Some(m) = up.take() {
                upload(gl, &mut gpu.board, &m);
            }
            if let Some(m) = cup.take() {
                upload(gl, &mut gpu.caps, &m);
            }
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.use_program(Some(gpu.program));
            gl.uniform_matrix_4_f32_slice(gpu.u_mvp.as_ref(), false, &mvp);
            gl.uniform_3_f32(gpu.u_eye.as_ref(), eye[0], eye[1], eye[2]);
            gl.uniform_3_f32(gpu.u_cam.as_ref(), eye_at[0], eye_at[1], eye_at[2]);
            gl.uniform_1_f32_slice(gpu.u_hide.as_ref(), &hide);
            let mode =
                |m: f32| gl.uniform_4_f32(gpu.u_mode.as_ref(), m, XRAY_ALPHA, XRAY_COPPER, 0.0);
            let set_clip = |on: bool| {
                let c = if on { clip } else { [0.0; 4] };
                gl.uniform_4_f32(gpu.u_clip.as_ref(), c[0], c[1], c[2], c[3]);
            };
            mode(if xray { 1.0 } else { 0.0 });
            set_clip(true);
            draw(gl, &gpu.board);
            if clip[3] > 0.0 {
                set_clip(false);
                draw(gl, &gpu.caps);
                set_clip(true);
            }
            if xray {
                // Copper and laminate last, see-through, over the solid parts.
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.depth_mask(false);
                mode(2.0);
                draw(gl, &gpu.board);
                gl.depth_mask(true);
                gl.disable(glow::BLEND);
            }
            gl.disable(glow::DEPTH_TEST);
        }
    });
    painter.add(egui::PaintCallback {
        rect,
        callback: Arc::new(cb),
    });

    if let Some(e) = v.shared.lock().ok().and_then(|s| s.error.clone()) {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("3D view could not start OpenGL shaders:\n{e}"),
            egui::FontId::proportional(14.0),
            Color32::LIGHT_RED,
        );
    }

    // Options in the corner.
    let g_bends = &scene.board.bends;
    let mut opt = v.options;
    let mut sec = v.section;
    let half = v.half;
    let hidden = &mut v.hidden;
    let n = scene.copper_count as usize;
    let names: Vec<String> = (0..n.min(64))
        .map(|i| {
            scene
                .layers
                .iter()
                .find(|l| l.slot == Slot::Copper(i as u8))
                .map_or_else(|| format!("Copper {}", i + 1), |l| l.name.clone())
        })
        .collect();
    egui::Area::new(egui::Id::new("view3d_opts"))
        .fixed_pos(rect.left_top() + egui::vec2(10.0, 10.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mask");
                    egui::ComboBox::from_id_salt("mask")
                        .selected_text(opt.mask.name())
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for m in MaskColor::ALL {
                                ui.selectable_value(&mut opt.mask, m, m.name());
                            }
                        });
                    ui.checkbox(&mut opt.parts, "Parts");
                    ui.checkbox(&mut opt.silk, "Silkscreen");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut opt.xray, "X-ray")
                        .on_hover_text("Clear laminate and mask, so the inner layers show (X)");
                    let can_cut = opt.fold == 0;
                    ui.add_enabled(can_cut, egui::Checkbox::new(&mut opt.section, "Section"))
                        .on_hover_text("Cut the board open with a plane and look at the cut face");
                    ui.menu_button("Layers", |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("All").clicked() {
                                hidden.iter_mut().for_each(|h| *h = false);
                            }
                            if ui.small_button("None").clicked() {
                                hidden[..names.len()].iter_mut().for_each(|h| *h = true);
                            }
                        });
                        for (i, name) in names.iter().enumerate() {
                            let mut on = !hidden[i];
                            if ui.checkbox(&mut on, name).changed() {
                                hidden[i] = !on;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Copper layers to draw");
                });
                if opt.section && opt.fold == 0 {
                    ui.horizontal(|ui| {
                        ui.label("Cut");
                        ui.selectable_value(&mut sec.axis, 0, "across X");
                        ui.selectable_value(&mut sec.axis, 1, "across Y");
                        ui.checkbox(&mut sec.flip, "Other side");
                    });
                    let h = half[sec.axis].max(0.1);
                    sec.pos = sec.pos.clamp(-h, h);
                    ui.add(
                        egui::Slider::new(&mut sec.pos, -h..=h)
                            .suffix(" mm")
                            .step_by(0.01)
                            .text("from centre"),
                    );
                }
                if !g_bends.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Fold");
                        ui.add(egui::Slider::new(&mut opt.fold, 0..=100).suffix(" %"))
                            .on_hover_text("Rigid-flex: bend the flex at its bends, by this share of each bend's angle");
                    });
                }
                ui.label(
                    RichText::new(
                        "Drag to turn, right-drag to pan, scroll to zoom.\nT top, B flip over, X X-ray, F or double-click to reset.",
                    )
                    .small()
                    .weak(),
                );
                if let Some(l) = &v.loader {
                    ui.label(
                        RichText::new(format!(
                            "Loading 3D part models: {} of {}",
                            l.done.load(Ordering::Relaxed),
                            l.total
                        ))
                        .small(),
                    );
                } else if v.models.is_some() {
                    ui.label(
                        RichText::new(format!(
                            "{} of {} part models loaded",
                            v.model_count,
                            g.models.len()
                        ))
                        .small()
                        .weak(),
                    );
                }
                if opt.parts && v.parts == 0 {
                    ui.label(
                        RichText::new("This board has no part placement data.")
                            .small()
                            .weak(),
                    );
                }
            });
        });
    v.options = opt;
    v.section = sec;
    if resp.dragged() || scroll != 0.0 {
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_cuts_laminate_copper_and_barrel() {
        let sq = |x0: f32, x1: f32| -> Vec<Tri2> {
            vec![
                [[x0, -5.0], [x1, -5.0], [x1, 5.0]],
                [[x0, -5.0], [x1, 5.0], [x0, 5.0]],
            ]
        };
        let src = CapSrc {
            layers: vec![([0.765, 0.8], COPPER, sq(-2.0, 2.0))],
            board: sq(-10.0, 10.0),
            slices: vec![([-0.765, 0.765], CORE)],
            barrels: vec![([0.0, 3.0], 0.1, 0.125, [-0.8, 0.8])],
        };
        // Across y at 0: the laminate and the copper are each one span;
        // the barrel at y = 3 is missed.
        let m = caps(&src, 1, 0.0, -1.0);
        assert_eq!(m.idx.len(), 2 * 6);
        // Across x at 0: the barrel's two walls are cut too.
        let m = caps(&src, 0, 0.0, -1.0);
        assert_eq!(m.idx.len(), 4 * 6);
        let xs: Vec<f32> = m.verts.iter().map(|v| v.p[0]).collect();
        assert!(xs.iter().all(|x| x.abs() < 0.01));
        assert_eq!(
            merge(vec![[0.0, 1.0], [0.5, 2.0], [3.0, 4.0]]),
            vec![[0.0, 2.0], [3.0, 4.0]]
        );
    }
}
