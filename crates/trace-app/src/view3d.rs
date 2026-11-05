//! 3D board view: the board as a solder-masked slab with outer copper,
//! silkscreen, holes and simple part bodies, drawn with OpenGL and turned
//! with the mouse.

use crate::scene::{Geom, Scene, Slot, Source};
use eframe::egui::{self, Color32, RichText};
use eframe::glow::{self, HasContext};
use std::sync::{Arc, Mutex};
use trace_core::{LayerRef, PadShape, Side};

const MM_PER_MIL: f32 = 0.0254;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vert {
    p: [f32; 3],
    c: [u8; 4],
    n: [i8; 4],
}

#[derive(Default)]
pub struct Mesh {
    verts: Vec<Vert>,
    idx: Vec<u32>,
    /// Half the board's diagonal, mm.
    pub radius: f32,
    pub parts: usize,
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
const HOLE: [u8; 3] = [12, 12, 12];
const EDGE: [u8; 3] = [190, 170, 120];

/// Height of each surface item above the laminate, mm. Exaggerated a little
/// so the layers do not fight in the depth buffer.
const Z_POUR: f32 = 0.012;
const Z_TRACK: f32 = 0.018;
const Z_PAD: f32 = 0.030;
const Z_SILK: f32 = 0.040;
const Z_HOLE: f32 = 0.050;

struct Builder {
    m: Mesh,
    cx: f32,
    cy: f32,
}

impl Builder {
    fn pt(&self, x: f32, y: f32) -> [f32; 2] {
        [(x - self.cx) * MM_PER_MIL, (y - self.cy) * MM_PER_MIL]
    }

    fn vert(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 3]) -> u32 {
        let q = |v: f32| (v * 127.0).round().clamp(-127.0, 127.0) as i8;
        self.m.verts.push(Vert {
            p,
            c: [c[0], c[1], c[2], 255],
            n: [q(n[0]), q(n[1]), q(n[2]), 0],
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

    /// Triangles from a tessellator (mm, board-centred) at height z.
    fn tris(&mut self, verts: &[[f32; 2]], tris: &[u32], z: f32, c: [u8; 3]) {
        let nz = if z >= 0.0 { 1.0 } else { -1.0 };
        let i0 = self.m.verts.len() as u32;
        for p in verts {
            self.vert([p[0], p[1], z], [0.0, 0.0, nz], c);
        }
        self.m.idx.extend(tris.iter().map(|t| i0 + t));
    }

    fn circle(&mut self, x: f32, y: f32, r: f32, z: f32, c: [u8; 3]) {
        let [x, y] = self.pt(x, y);
        let r = r * MM_PER_MIL;
        let n = segments(r);
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [x + r * a.cos(), y + r * a.sin()]
            })
            .collect();
        self.fan(&pts, z, c);
    }

    fn geom(&mut self, g: &Geom, z: f32, c: [u8; 3]) {
        match *g {
            Geom::Circle { x, y, r } => self.circle(x, y, r, z, c),
            Geom::Rect { x, y, sx, sy, rot } => {
                let (s, co) = rot.to_radians().sin_cos();
                let pts: Vec<[f32; 2]> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .iter()
                    .map(|(u, v)| {
                        let (u, v) = (u * sx / 2.0, v * sy / 2.0);
                        self.pt(x + u * co - v * s, y + u * s + v * co)
                    })
                    .collect();
                self.fan(&pts, z, c);
            }
            Geom::Seg { x1, y1, x2, y2, w } => {
                let [ax, ay] = self.pt(x1, y1);
                let [bx, by] = self.pt(x2, y2);
                let r = w * MM_PER_MIL / 2.0;
                if r <= 0.0 {
                    return;
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
                self.fan(&pts, z, c);
            }
        }
    }

    /// A box standing on the board from height z0 to z1 (both signed).
    fn block(&mut self, corners: &[[f32; 2]; 4], z0: f32, z1: f32, c: [u8; 3]) {
        let top = c;
        let side = [
            (c[0] as f32 * 0.8) as u8,
            (c[1] as f32 * 0.8) as u8,
            (c[2] as f32 * 0.8) as u8,
        ];
        self.fan(corners, z1, top);
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy).max(1e-9);
            let n = [dy / l, -dx / l, 0.0];
            let i0 = self.vert([a[0], a[1], z0], n, side);
            self.vert([b[0], b[1], z0], n, side);
            self.vert([b[0], b[1], z1], n, side);
            self.vert([a[0], a[1], z1], n, side);
            self.m
                .idx
                .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
        }
    }
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

#[derive(Clone, Copy, PartialEq)]
pub struct Options {
    pub mask: MaskColor,
    pub parts: bool,
    pub silk: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mask: MaskColor::Green,
            parts: true,
            silk: true,
        }
    }
}

/// Builds the board's 3D mesh, in mm with the board centred on the origin.
pub fn build(scene: &Scene, opt: Options) -> Mesh {
    let b = scene.bounds;
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let mut bld = Builder {
        m: Mesh::default(),
        cx,
        cy,
    };
    bld.m.radius = ((b[2] - b[0]).hypot(b[3] - b[1]) / 2.0 * MM_PER_MIL).max(1.0);
    let board = &scene.board;
    let th = board.stackup.total_thickness().mm() as f32;
    let t = if th > 0.2 { th / 2.0 } else { 0.8 };
    let n = scene.copper_count.max(1);
    let last = n - 1;
    let (mask, mask_cu, silk) = opt.mask.colors();

    // Laminate: top and bottom faces plus the edge.
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
    let (v, tr) = tessellate(std::slice::from_ref(&ring));
    bld.tris(&v, &tr, t, mask);
    bld.tris(&v, &tr, -t, mask);
    let area: f32 = (0..ring.len())
        .map(|i| {
            let (a, c) = (ring[i], ring[(i + 1) % ring.len()]);
            a[0] * c[1] - c[0] * a[1]
        })
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    for i in 0..ring.len() {
        let (a, c) = (ring[i], ring[(i + 1) % ring.len()]);
        let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
        let l = dx.hypot(dy);
        if l < 1e-6 {
            continue;
        }
        let nrm = [sign * dy / l, -sign * dx / l, 0.0];
        let i0 = bld.vert([a[0], a[1], -t], nrm, EDGE);
        bld.vert([c[0], c[1], -t], nrm, EDGE);
        bld.vert([c[0], c[1], t], nrm, EDGE);
        bld.vert([a[0], a[1], t], nrm, EDGE);
        bld.m
            .idx
            .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
    }

    // Which outer face a copper slot sits on: +1 top, -1 bottom.
    let face = |s: Slot| match s {
        Slot::Copper(0) => Some(1.0),
        Slot::Copper(i) if i == last && last > 0 => Some(-1.0),
        _ => None,
    };

    // Pours under the mask.
    for f in &scene.fills {
        let side = match f.slot {
            Slot::TopSilk if opt.silk => Some((1.0, Z_SILK, silk)),
            Slot::BottomSilk if opt.silk => Some((-1.0, Z_SILK, silk)),
            s => face(s).map(|d| (d, Z_POUR, mask_cu)),
        };
        let Some((d, dz, col)) = side else { continue };
        let fb = f.bbox;
        if fb[2] < b[0] || fb[0] > b[2] || fb[3] < b[1] || fb[1] > b[3] {
            continue;
        }
        let pts: Vec<[f32; 2]> = scene.fill_verts[f.verts.clone()]
            .iter()
            .map(|p| bld.pt(p[0], p[1]))
            .collect();
        bld.tris(&pts, &scene.fill_tris[f.tris.clone()], d * (t + dz), col);
    }

    // Tracks and arcs under the mask, silkscreen on top of it.
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
            Slot::TopSilk if opt.silk => bld.geom(&p.geom, t + Z_SILK, silk),
            Slot::BottomSilk if opt.silk => bld.geom(&p.geom, -(t + Z_SILK), silk),
            s => {
                if let Some(d) = face(s) {
                    bld.geom(&p.geom, d * (t + Z_TRACK), mask_cu);
                }
            }
        }
    }

    // Pads: bare copper with a gold finish, and their holes.
    let g = &board.geometry;
    let mil = |l: trace_core::Length| l.mil() as f32;
    for pad in &g.pads {
        let (x, y) = (mil(pad.at.x), mil(pad.at.y));
        let mut lands: Vec<LandRef> = Vec::new();
        let sides: &[(u8, f32)] = &[(0, 1.0), (last, -1.0)];
        match &pad.layer {
            LayerRef::AllCopper => {
                for &(layer, d) in sides {
                    if pad.stack.is_empty() || last == 0 {
                        lands.push((
                            d,
                            pad.shape,
                            mil(pad.size_x),
                            mil(pad.size_y),
                            pad.rotation_deg as f32,
                            &pad.outline,
                        ));
                    } else if let Some(l) = pad.stack.iter().find(|l| l.layer == layer) {
                        lands.push((
                            d,
                            l.shape,
                            mil(l.size_x),
                            mil(l.size_y),
                            l.rotation_deg as f32,
                            &l.outline,
                        ));
                    }
                }
            }
            LayerRef::Copper(i) => {
                let d = if *i == 0 {
                    1.0
                } else if *i == last {
                    -1.0
                } else {
                    continue;
                };
                lands.push((
                    d,
                    pad.shape,
                    mil(pad.size_x),
                    mil(pad.size_y),
                    pad.rotation_deg as f32,
                    &pad.outline,
                ));
            }
            _ => continue,
        }
        for (d, shape, sx, sy, rot, outline) in lands {
            let z = d * (t + Z_PAD);
            if outline.len() >= 3 {
                let ring: Vec<[f32; 2]> =
                    outline.iter().map(|p| bld.pt(mil(p.x), mil(p.y))).collect();
                let (v, tr) = tessellate(&[ring]);
                bld.tris(&v, &tr, z, FINISH);
            } else {
                bld.geom(&pad_geom(x, y, shape, sx, sy, rot), z, FINISH);
            }
        }
        let hole = mil(pad.hole);
        if hole > 0.0 {
            bld.circle(x, y, hole / 2.0, t + Z_HOLE, HOLE);
            bld.circle(x, y, hole / 2.0, -(t + Z_HOLE), HOLE);
        }
    }

    // Vias: tented copper rings with an open hole on each face they reach.
    for v in &g.vias {
        let (x, y) = (mil(v.at.x), mil(v.at.y));
        let ring_on = |layer: u8| {
            v.rings
                .iter()
                .find(|r| r.0 == layer)
                .map_or(mil(v.diameter), |r| mil(r.1))
        };
        let hole = mil(v.hole) / 2.0;
        for (layer, d, reaches) in [
            (0u8, 1.0, v.from_layer.min(v.to_layer) == 0),
            (last, -1.0, v.from_layer.max(v.to_layer) >= last),
        ] {
            if !reaches {
                continue;
            }
            bld.circle(x, y, ring_on(layer) / 2.0, d * (t + Z_TRACK), mask_cu);
            if hole > 0.0 {
                bld.circle(x, y, hole, d * (t + Z_HOLE), HOLE);
            }
        }
    }

    if opt.parts {
        bld.m.parts = bodies(&mut bld, scene, t);
    }
    bld.m
}

/// Face (+1 top, -1 bottom), shape, size x and y, rotation and outline.
type LandRef<'a> = (f32, PadShape, f32, f32, f32, &'a Vec<trace_core::Point>);

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
fn bodies(bld: &mut Builder, scene: &Scene, t: f32) -> usize {
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
        if pads.is_empty() {
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
        let near = (self.dist - radius * 1.5).max(self.dist * 0.02);
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

struct Gpu {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    count: i32,
    u_mvp: Option<glow::UniformLocation>,
    u_eye: Option<glow::UniformLocation>,
}

#[derive(Default)]
struct Shared {
    gpu: Option<Gpu>,
    upload: Option<Mesh>,
    error: Option<String>,
}

pub struct View3d {
    pub camera: Camera,
    pub options: Options,
    built: Option<Options>,
    radius: f32,
    parts: usize,
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
            built: None,
            radius: 50.0,
            parts: 0,
            shared: Arc::default(),
        }
    }
}

impl View3d {
    /// Forget the mesh so it is rebuilt for a new board.
    pub fn reset(&mut self) {
        self.built = None;
    }

    pub fn destroy(&self, gl: &glow::Context) {
        if let Some(g) = self.shared.lock().ok().and_then(|mut s| s.gpu.take()) {
            unsafe {
                gl.delete_program(g.program);
                gl.delete_vertex_array(g.vao);
                gl.delete_buffer(g.vbo);
                gl.delete_buffer(g.ebo);
            }
        }
    }
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
{vout} vec3 v_col;
{vout} vec3 v_nrm;
void main() {{
    gl_Position = u_mvp * vec4(a_pos, 1.0);
    v_col = a_col.rgb;
    v_nrm = a_nrm.xyz;
}}
"
    );
    let fs = format!(
        "{header}
{fin} vec3 v_col;
{fin} vec3 v_nrm;
uniform vec3 u_eye;
{out}
void main() {{
    vec3 n = normalize(v_nrm);
    if (dot(n, u_eye) < 0.0) n = -n;
    vec3 key = normalize(vec3(-0.35, -0.45, 0.82));
    float l = 0.38 + 0.42 * max(dot(n, u_eye), 0.0) + 0.30 * abs(dot(n, key));
    vec3 c = v_col * min(l, 1.15);
    {frag} = vec4(c, 1.0);
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
    Ok(Gpu {
        u_mvp: gl.get_uniform_location(program, "u_mvp"),
        u_eye: gl.get_uniform_location(program, "u_eye"),
        program,
        vao,
        vbo,
        ebo,
        count: 0,
    })
}

unsafe fn upload(gl: &glow::Context, gpu: &mut Gpu, m: &Mesh) {
    let bytes = |p: *const u8, n: usize| std::slice::from_raw_parts(p, n);
    gl.bind_vertex_array(Some(gpu.vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(gpu.vbo));
    gl.buffer_data_u8_slice(
        glow::ARRAY_BUFFER,
        bytes(
            m.verts.as_ptr() as *const u8,
            std::mem::size_of_val(m.verts.as_slice()),
        ),
        glow::STATIC_DRAW,
    );
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(gpu.ebo));
    gl.buffer_data_u8_slice(
        glow::ELEMENT_ARRAY_BUFFER,
        bytes(
            m.idx.as_ptr() as *const u8,
            std::mem::size_of_val(m.idx.as_slice()),
        ),
        glow::STATIC_DRAW,
    );
    gl.bind_vertex_array(None);
    gpu.count = m.idx.len() as i32;
}

/// Draws the 3D view in the central panel.
pub fn show(v: &mut View3d, scene: &Scene, ui: &mut egui::Ui) {
    if v.built != Some(v.options) {
        let first = v.built.is_none();
        let mesh = build(scene, v.options);
        v.radius = mesh.radius;
        v.parts = mesh.parts;
        if first {
            v.camera.fit(mesh.radius);
        }
        if let Ok(mut s) = v.shared.lock() {
            s.upload = Some(mesh);
        }
        v.built = Some(v.options);
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
        cam.dist = (cam.dist * (-scroll * 0.0015).exp()).clamp(v.radius * 0.02, v.radius * 30.0);
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

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(38, 42, 50));
    let mvp = cam.matrix(rect.width() / rect.height().max(1.0), v.radius);
    let eye = cam.eye_dir();
    let shared = v.shared.clone();
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
                gpu, upload: up, ..
            } = &mut *s;
            let Some(gpu) = gpu.as_mut() else { return };
            if let Some(m) = up.take() {
                upload(gl, gpu, &m);
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
            gl.bind_vertex_array(Some(gpu.vao));
            gl.draw_elements(glow::TRIANGLES, gpu.count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
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
    let mut opt = v.options;
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
                ui.label(
                    RichText::new(
                        "Drag to turn, right-drag to pan, scroll to zoom.\nT top, B flip over, F or double-click to reset.",
                    )
                    .small()
                    .weak(),
                );
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
    if resp.dragged() || scroll != 0.0 {
        ui.ctx().request_repaint();
    }
}
