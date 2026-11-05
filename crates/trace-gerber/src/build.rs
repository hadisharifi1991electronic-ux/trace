//! Rebuilds a board from recognised fabrication layers: copper tracks, arcs,
//! pads and pours per layer, vias and through-hole pads from the drill
//! files, silkscreen, the outline, and nets.

use crate::detect::{CuPos, Role, Side};
use crate::excellon::Drill;
use crate::geom::{self, Pt};
use crate::gerber::{Aperture, Gerber, Item, Obj, PrimKind, Shape, Xform};
use crate::nets;
use std::collections::HashMap;
use trace_core::*;

/// One input file after parsing and recognition.
pub struct Layer {
    pub file: String,
    pub role: Role,
    pub reason: String,
    pub data: LayerData,
}

pub enum LayerData {
    Gerber(Gerber),
    Drill(Drill),
    Netlist(crate::ipc356::Netlist),
    None,
}

/// What happened to each input file.
#[derive(Clone, Debug)]
pub struct FileReport {
    pub file: String,
    pub role: String,
    pub reason: String,
    /// Copper layer index the file became, for copper files.
    pub copper_index: Option<u8>,
    pub objects: usize,
}

#[derive(Debug)]
pub struct Import {
    pub board: Board,
    /// Hole rings and exact pad shapes used for net tracing (for tools).
    #[doc(hidden)]
    pub trace_input: nets::Extra,
    pub files: Vec<FileReport>,
    pub warnings: Vec<String>,
    /// How nets were found: from X2 attributes, an ODB++ netlist, or traced
    /// through the copper.
    pub nets_from: &'static str,
}

fn mm(v: f64) -> Length {
    Length::from_mm(v)
}

fn pt(p: Pt) -> Point {
    Point {
        x: mm(p[0]),
        y: mm(p[1]),
    }
}

/// Where a copper object came from, so nets can be given back to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjRef {
    Track(usize),
    Arc(usize),
    Pad(usize),
    Poly(usize),
    Via(usize),
}

struct Builder {
    board: Board,
    warnings: Vec<String>,
    /// Net name from attributes for each copper object.
    named: HashMap<ObjRef, String>,
    /// Component and pin from X2 `.P` attributes, per pad.
    pins: HashMap<usize, (String, String)>,
    /// Mask openings per side: centre and radius.
    mask: HashMap<Side, Vec<(Pt, f64)>>,
    /// Flashed pads on each copper layer, for matching holes.
    pad_grid: HashMap<(u8, i64, i64), Vec<usize>>,
    /// Per-via pad radius on each layer it has a pad on.
    via_rings: Vec<Vec<(u8, f64)>>,
    /// Per-layer ring radius of through-hole pads (pad index, rings).
    pad_rings: Vec<(usize, Vec<(u8, f64)>)>,
    /// Flashes absorbed into a via or through-hole pad.
    dead: std::collections::HashSet<usize>,
    copper: u8,
    flash_function: HashMap<usize, String>,
    /// Half the largest pad size, to bound pad searches.
    max_pad: f64,
    /// Exact copper of pads whose shape a rectangle would misrepresent.
    exact: HashMap<usize, Vec<Vec<Pt>>>,
    /// Flashes absorbed into a through-hole pad, per (pad, layer).
    layer_pads: HashMap<(usize, u8), (Pad, Option<Vec<Vec<Pt>>>)>,
    /// Local outline per macro aperture and transform (None: rect is fine).
    macro_cache: HashMap<(u32, [u64; 4]), Option<Vec<Vec<Pt>>>>,
}

const PAD_CELL: f64 = 0.5;

fn cell(p: Pt) -> (i64, i64) {
    (
        (p[0] / PAD_CELL).floor() as i64,
        (p[1] / PAD_CELL).floor() as i64,
    )
}

pub fn build(layers: Vec<Layer>, named_source: &'static str) -> Import {
    build_with(layers, named_source, Vec::new())
}

/// Like [`build`], starting from a known list of parts (ODB++ gives them).
pub fn build_with(
    mut layers: Vec<Layer>,
    named_source: &'static str,
    parts: Vec<Component>,
) -> Import {
    let mut b = Builder {
        board: Board {
            geometry: Geometry {
                per_layer_copper: true,
                ..Default::default()
            },
            ..Default::default()
        },
        warnings: Vec::new(),
        named: HashMap::new(),
        pins: HashMap::new(),
        mask: HashMap::new(),
        pad_grid: HashMap::new(),
        via_rings: Vec::new(),
        pad_rings: Vec::new(),
        dead: Default::default(),
        copper: 0,
        flash_function: HashMap::new(),
        max_pad: 0.0,
        exact: HashMap::new(),
        layer_pads: HashMap::new(),
        macro_cache: HashMap::new(),
    };
    // Expand aperture blocks so later stages see plain objects.
    for l in &mut layers {
        if let LayerData::Gerber(g) = &mut l.data {
            expand_blocks(g);
        }
    }

    // ---- copper order ---------------------------------------------------
    let mut copper: Vec<usize> = (0..layers.len())
        .filter(|&i| layers[i].role.is_copper())
        .collect();
    let all_ordered = copper
        .iter()
        .all(|&i| matches!(layers[i].role, Role::Copper { order: Some(_), .. }));
    // Inner layers numbered 2..15 with no repeats (as Allegro or KiCad name
    // them) go by number; Protel numbers signal and plane layers separately
    // (G1, GP1), so there planes go after signals.
    let inner_numbers: Vec<Option<u32>> = copper
        .iter()
        .filter_map(|&i| match &layers[i].role {
            Role::Copper {
                pos: CuPos::Inner,
                number,
                ..
            } => Some(*number),
            _ => None,
        })
        .collect();
    let by_number = inner_numbers.iter().all(|n| n.is_some()) && {
        let mut v: Vec<u32> = inner_numbers.iter().flatten().copied().collect();
        v.sort();
        v.dedup();
        v.len() == inner_numbers.len()
    };
    let key = |i: usize| -> (u32, u32, u32, u32, usize) {
        match &layers[i].role {
            Role::Copper {
                pos,
                number,
                order,
                plane,
            } => {
                let rank = match pos {
                    CuPos::Top => 0,
                    CuPos::Inner => 1,
                    CuPos::Bottom => 2,
                };
                if all_ordered {
                    (order.unwrap_or(0), 0, 0, 0, i)
                } else if by_number {
                    (rank, number.unwrap_or(999), 0, 0, i)
                } else {
                    (rank, *plane as u32, number.unwrap_or(999), 0, i)
                }
            }
            _ => (9, 0, 0, 0, i),
        }
    };
    copper.sort_by_key(|&i| key(i));
    b.copper = copper.len() as u8;
    let mut files: Vec<FileReport> = layers
        .iter()
        .map(|l| FileReport {
            file: l.file.clone(),
            role: l.role.label(),
            reason: l.reason.clone(),
            copper_index: None,
            objects: match &l.data {
                LayerData::Gerber(g) => g.items.len(),
                LayerData::Drill(d) => d.holes.len(),
                LayerData::Netlist(n) => n.points.len(),
                LayerData::None => 0,
            },
        })
        .collect();
    let n = copper.len();
    let mut inner_no = 0;
    for (ci, &li) in copper.iter().enumerate() {
        files[li].copper_index = Some(ci as u8);
        let plane = matches!(layers[li].role, Role::Copper { plane: true, .. });
        let name = if ci == 0 && n > 1 {
            "Top".to_string()
        } else if ci + 1 == n && n > 1 {
            "Bottom".to_string()
        } else if n == 1 {
            "Copper".to_string()
        } else {
            inner_no += 1;
            if plane {
                format!("Inner {inner_no} (plane)")
            } else {
                format!("Inner {inner_no}")
            }
        };
        b.board.stackup.layers.push(StackLayer {
            name,
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            material: None,
        });
    }

    // ---- outline --------------------------------------------------------
    let copper_box = copper
        .iter()
        .filter_map(|&i| match &layers[i].data {
            LayerData::Gerber(g) => layer_bbox(g),
            _ => None,
        })
        .reduce(geom::bbox_union);
    let outline = find_outline(&layers, copper_box);
    b.board.geometry.outline = outline.iter().map(|&p| pt(p)).collect();

    // ---- mask openings (to tell vias from pins) -------------------------
    for l in &layers {
        if let (Role::Mask(side), LayerData::Gerber(g)) = (&l.role, &l.data) {
            let list = b.mask.entry(*side).or_default();
            for it in &g.items {
                if let Obj::Flash { ap, at, .. } = &it.obj {
                    let a = g.aperture(*ap);
                    list.push((*at, geom::aperture_width(a).max(max_extent(a)) / 2.0));
                }
            }
        }
    }

    // ---- copper layers --------------------------------------------------
    for (ci, &li) in copper.iter().enumerate() {
        if let LayerData::Gerber(g) = &layers[li].data {
            b.copper_layer(ci as u8, g, &outline);
        }
    }

    // ---- silkscreen -----------------------------------------------------
    for l in &layers {
        if let (Role::Silk(side), LayerData::Gerber(g)) = (&l.role, &l.data) {
            let layer = if *side == Side::Top {
                LayerRef::TopSilk
            } else {
                LayerRef::BottomSilk
            };
            b.silk(layer, g);
        }
    }

    // ---- holes ----------------------------------------------------------
    let mut last_fix: Option<(f64, Pt)> = None;
    for l in &layers {
        if let LayerData::Drill(d) = &l.data {
            let file_plated = match &l.role {
                Role::Drill { plated, .. } => *plated,
                _ => None,
            };
            let span = match &l.role {
                Role::Drill {
                    span: Some((a, z)), ..
                } => {
                    let a = (*a as u8).saturating_sub(1).min(b.copper.saturating_sub(1));
                    let z = (*z as u8).saturating_sub(1).min(b.copper.saturating_sub(1));
                    Some((a, z))
                }
                _ => None,
            };
            let fix = b.align_drill(d, span).or_else(|| {
                // A small file (such as slots) follows the fix found for the others.
                let (s, o) = last_fix?;
                (d.holes.len() < 8 && b.hit_rate(d, span, 1.0, [0.0, 0.0]) < 0.5).then_some((s, o))
            });
            match fix {
                Some((s, o)) => {
                    last_fix = Some((s, o));
                    let mut d2 = Drill {
                        holes: d.holes.clone(),
                        file_function: d.file_function.clone(),
                        plated: d.plated,
                        span: d.span,
                    };
                    let f = |p: Pt| [p[0] * s + o[0], p[1] * s + o[1]];
                    for h in &mut d2.holes {
                        h.at = f(h.at);
                        h.end = h.end.map(f);
                        h.diameter *= if (s - 25.4).abs() < 0.01 || (s - 1.0 / 25.4).abs() < 0.001 {
                            s
                        } else {
                            1.0
                        };
                    }
                    let what = if (s - 1.0).abs() < 1e-9 {
                        format!("moved by {:.3}, {:.3} mm", o[0], o[1])
                    } else {
                        format!("scaled by {s} and moved by {:.3}, {:.3} mm", o[0], o[1])
                    };
                    b.warnings.push(format!(
                        "{}: holes did not line up with the copper, so they were {what}",
                        l.file
                    ));
                    b.holes(&d2, file_plated, span);
                }
                None => b.holes(d, file_plated, span),
            }
        }
    }
    b.remove_dead_pads();

    // ---- nets -----------------------------------------------------------
    let extra = nets::Extra {
        via_rings: std::mem::take(&mut b.via_rings),
        pad_rings: b.pad_rings.drain(..).collect(),
        exact: std::mem::take(&mut b.exact),
        layer_pads: std::mem::take(&mut b.layer_pads),
    };
    let nets_from = if !b.named.is_empty() {
        b.apply_named_nets();
        named_source
    } else {
        nets::trace(&mut b.board, &extra);
        match layers.iter().find_map(|l| match &l.data {
            LayerData::Netlist(n) => Some((l.file.clone(), n)),
            _ => None,
        }) {
            Some((file, n)) => {
                b.apply_ipc(n, &file, &extra);
                "the IPC-356 netlist, with connections traced through the copper"
            }
            None => "traced through the copper",
        }
    };
    b.board.components = parts;
    b.components();

    let g = &b.board.geometry;
    b.board.stats = BoardStats {
        tracks: g.tracks.len() + g.arcs.len(),
        vias: g.vias.len(),
        pads: g.pads.len(),
        polygons: g.polygons.len(),
    };
    connectivity::analyse(&mut b.board);
    for (i, net) in b.board.nets.iter_mut().enumerate() {
        net.has_pour = b
            .board
            .geometry
            .polygons
            .iter()
            .any(|p| p.net == Some(i as u32));
    }
    Import {
        board: b.board,
        trace_input: extra,
        files,
        warnings: b.warnings,
        nets_from,
    }
}

fn max_extent(a: &Aperture) -> f64 {
    let pts: Vec<Pt> = geom::flash_shapes(a, [0.0, 0.0], &Xform::default())
        .into_iter()
        .filter(|s| s.0)
        .flat_map(|s| s.1)
        .collect();
    if pts.is_empty() {
        return 0.0;
    }
    let bb = geom::bbox(&pts);
    (bb[2] - bb[0]).max(bb[3] - bb[1])
}

fn expand_blocks(g: &mut Gerber) {
    if !g
        .apertures
        .iter()
        .any(|a| matches!(a.shape, Shape::Block(_)))
    {
        return;
    }
    let apertures = g.apertures.clone();
    fn go(items: &[Item], aps: &[Aperture], offset: Pt, depth: u32, out: &mut Vec<Item>) {
        for it in items {
            match &it.obj {
                Obj::Flash { ap, at, .. } if depth < 8 => {
                    if let Shape::Block(inner) = &aps[*ap as usize].shape {
                        go(inner, aps, geom::add(*at, offset), depth + 1, out);
                        continue;
                    }
                    out.push(shift(it, offset));
                }
                _ => out.push(shift(it, offset)),
            }
        }
    }
    let mut out = Vec::new();
    go(&g.items, &apertures, [0.0, 0.0], 0, &mut out);
    g.items = out;
}

fn shift(it: &Item, d: Pt) -> Item {
    if d == [0.0, 0.0] {
        return it.clone();
    }
    let t = |p: Pt| geom::add(p, d);
    let obj = match &it.obj {
        Obj::Flash { ap, at, xf } => Obj::Flash {
            ap: *ap,
            at: t(*at),
            xf: *xf,
        },
        Obj::Line { ap, a, b } => Obj::Line {
            ap: *ap,
            a: t(*a),
            b: t(*b),
        },
        Obj::Arc { ap, a, b, c, cw } => Obj::Arc {
            ap: *ap,
            a: t(*a),
            b: t(*b),
            c: t(*c),
            cw: *cw,
        },
        Obj::Region(cs) => Obj::Region(
            cs.iter()
                .map(|c| c.iter().map(|&p| t(p)).collect())
                .collect(),
        ),
    };
    Item {
        obj,
        dark: it.dark,
        attrs: it.attrs,
    }
}

fn item_bbox(g: &Gerber, it: &Item) -> [f64; 4] {
    match &it.obj {
        Obj::Flash { ap, at, .. } => {
            let r = max_extent(g.aperture(*ap)) / 2.0;
            [at[0] - r, at[1] - r, at[0] + r, at[1] + r]
        }
        Obj::Line { ap, a, b } => {
            let r = geom::aperture_width(g.aperture(*ap)) / 2.0;
            [
                a[0].min(b[0]) - r,
                a[1].min(b[1]) - r,
                a[0].max(b[0]) + r,
                a[1].max(b[1]) + r,
            ]
        }
        Obj::Arc { ap, c, a, .. } => {
            let r = geom::dist(*c, *a) + geom::aperture_width(g.aperture(*ap)) / 2.0;
            [c[0] - r, c[1] - r, c[0] + r, c[1] + r]
        }
        Obj::Region(cs) => cs
            .iter()
            .map(|c| geom::bbox(c))
            .reduce(geom::bbox_union)
            .unwrap_or([0.0; 4]),
    }
}

fn layer_bbox(g: &Gerber) -> Option<[f64; 4]> {
    g.items
        .iter()
        .map(|it| item_bbox(g, it))
        .reduce(geom::bbox_union)
}

/// Outlines of an object as filled rings (dark parts only).
fn item_rings(g: &Gerber, it: &Item) -> Vec<Vec<Pt>> {
    match &it.obj {
        Obj::Flash { ap, at, xf } => geom::flash_shapes(g.aperture(*ap), *at, xf)
            .into_iter()
            .filter(|s| s.0)
            .map(|s| s.1)
            .collect(),
        Obj::Line { ap, a, b } => vec![geom::line_shape(g.aperture(*ap), *a, *b)],
        Obj::Arc { ap, a, b, c, cw } => {
            let full = geom::dist(*a, *b) < 1e-9;
            let pts = geom::arc_points(*c, *a, *b, *cw, full);
            geom::stroke_path(&pts, geom::aperture_width(g.aperture(*ap)))
        }
        Obj::Region(cs) => cs.clone(),
    }
}

/// Shape, size and rotation of a pad flashed with an aperture.
fn pad_geometry(a: &Aperture, xf: &Xform) -> (PadShape, f64, f64, f64, Pt) {
    let s = xf.scale;
    let (shape, w, h, rot, off) = match &a.shape {
        Shape::Circle(d) => (PadShape::Round, *d, *d, 0.0, [0.0, 0.0]),
        Shape::Rect(w, h) => (PadShape::Rect, *w, *h, 0.0, [0.0, 0.0]),
        Shape::Obround(w, h) => (PadShape::Round, *w, *h, 0.0, [0.0, 0.0]),
        Shape::Polygon { d, n, rot } => {
            if *n == 8 {
                (PadShape::Octagon, *d, *d, *rot, [0.0, 0.0])
            } else {
                (PadShape::Round, *d, *d, 0.0, [0.0, 0.0])
            }
        }
        Shape::Macro(prims) => {
            let on: Vec<_> = prims.iter().filter(|p| p.on).collect();
            if on.len() == 1 {
                if let PrimKind::Circle { c, d } = on[0].kind {
                    return (PadShape::Round, d * s, d * s, 0.0, xf.apply(c));
                }
            }
            let pts: Vec<Pt> = geom::flash_shapes(a, [0.0, 0.0], &Xform::default())
                .into_iter()
                .filter(|x| x.0)
                .flat_map(|x| x.1)
                .collect();
            let (c, w, h, deg) = geom::min_rect(&pts);
            (PadShape::Rect, w, h, deg, c)
        }
        Shape::Block(_) => (PadShape::Round, 0.0, 0.0, 0.0, [0.0, 0.0]),
    };
    let mut rot = rot + xf.rot;
    if xf.mirror_x != xf.mirror_y {
        rot = -rot;
    }
    (shape, w * s, h * s, rot.rem_euclid(360.0), xf.apply(off))
}

/// The exact copper of a macro pad around its flash point, when the
/// bounding rectangle (area `rect_area`) would overstate it by over 15%.
fn exact_macro(a: &Aperture, xf: &Xform, rect_area: f64) -> Option<Vec<Vec<Pt>>> {
    let shapes = geom::flash_shapes(a, [0.0, 0.0], xf);
    let on: Vec<Vec<Pt>> = shapes.iter().filter(|s| s.0).map(|s| s.1.clone()).collect();
    let off: Vec<Vec<Pt>> = shapes
        .iter()
        .filter(|s| !s.0)
        .map(|s| s.1.clone())
        .collect();
    let merged = if off.is_empty() {
        geom::union(&on)
    } else {
        geom::difference(&on, &off)
    };
    let area: f64 = merged
        .iter()
        .map(|s| s.iter().map(|r| geom::area(r)).sum::<f64>().abs())
        .sum();
    if rect_area <= 0.0 || area >= rect_area * 0.85 {
        return None;
    }
    Some(merged.into_iter().flatten().collect())
}

fn core_arc(layer: LayerRef, c: Pt, a: Pt, b: Pt, cw: bool, width: f64) -> Option<Arc> {
    let r = geom::dist(c, a);
    if r < 1e-6 {
        return None;
    }
    let ang = |p: Pt| {
        (p[1] - c[1])
            .atan2(p[0] - c[0])
            .to_degrees()
            .rem_euclid(360.0)
    };
    let (mut s, mut e) = (ang(a), ang(b));
    if geom::dist(a, b) < 1e-9 {
        s = 0.0;
        e = 360.0;
    } else if cw {
        std::mem::swap(&mut s, &mut e);
    }
    Some(Arc {
        layer,
        net: None,
        center: pt(c),
        radius: mm(r),
        start_deg: s,
        end_deg: e,
        width: mm(width),
    })
}

impl Builder {
    fn named(&mut self, r: ObjRef, g: &Gerber, attrs: u32) {
        if let Some(n) = g.attrs.get(attrs as usize).and_then(|a| a.net.as_ref()) {
            if !n.is_empty() && n != "N/C" {
                self.named.insert(r, n.clone());
            }
        }
    }

    fn copper_layer(&mut self, ci: u8, g: &Gerber, outline: &[Pt]) {
        let layer = LayerRef::Copper(ci);
        let negative = g.image_negative ^ g.file_polarity_negative;
        // Dark regions wait for later clear objects to cut them.
        let mut regions: Vec<(usize, Vec<Vec<Pt>>, [f64; 4], u32)> = Vec::new();
        let mut clears: Vec<(usize, Vec<Pt>, [f64; 4])> = Vec::new();
        if negative {
            let base = if outline.len() >= 3 {
                outline.to_vec()
            } else {
                let bb = layer_bbox(g).unwrap_or([0.0; 4]);
                let m = 1.0;
                geom::rect(
                    [(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0],
                    bb[2] - bb[0] + 2.0 * m,
                    bb[3] - bb[1] + 2.0 * m,
                )
            };
            let bb = geom::bbox(&base);
            regions.push((0, vec![base], bb, 0));
        }
        for (seq, it) in g.items.iter().enumerate() {
            let seq = seq + 1;
            let dark = it.dark ^ negative;
            if !dark {
                for ring in item_rings(g, it) {
                    if ring.len() >= 3 {
                        let bb = geom::bbox(&ring);
                        clears.push((seq, ring, bb));
                    }
                }
                continue;
            }
            match &it.obj {
                Obj::Flash { ap, at, xf } => {
                    let a = g.aperture(*ap);
                    let (shape, w, h, rot, off) = pad_geometry(a, xf);
                    if w <= 0.0 {
                        continue;
                    }
                    let i = self.board.geometry.pads.len();
                    let at = geom::add(*at, off);
                    self.board.geometry.pads.push(Pad {
                        layer: layer.clone(),
                        net: None,
                        component: None,
                        name: String::new(),
                        at: pt(at),
                        size_x: mm(w),
                        size_y: mm(h),
                        hole: Length(0),
                        shape,
                        rotation_deg: rot,
                        outline: Vec::new(),
                        stack: Vec::new(),
                    });
                    self.pad_grid
                        .entry((ci, cell(at).0, cell(at).1))
                        .or_default()
                        .push(i);
                    self.max_pad = self.max_pad.max(w.hypot(h) / 2.0).min(5.0);
                    if let Some(f) = &a.function {
                        self.flash_function.insert(i, f.clone());
                    }
                    if let Shape::Macro(_) = a.shape {
                        let key = (
                            *ap,
                            [
                                xf.rot.to_bits(),
                                xf.scale.to_bits(),
                                xf.mirror_x as u64,
                                xf.mirror_y as u64,
                            ],
                        );
                        let local = self
                            .macro_cache
                            .entry(key)
                            .or_insert_with(|| exact_macro(a, xf, w * h))
                            .clone();
                        if let Some(rings) = local {
                            let flash = geom::add(at, [-off[0], -off[1]]);
                            let rings: Vec<Vec<Pt>> = rings
                                .iter()
                                .map(|r| r.iter().map(|&p| geom::add(p, flash)).collect())
                                .collect();
                            if let Some(o) = single_outline(&rings) {
                                self.board.geometry.pads[i].outline = o;
                            }
                            self.exact.insert(i, rings);
                        }
                    }
                    self.named(ObjRef::Pad(i), g, it.attrs);
                    if let Some(attrs) = g.attrs.get(it.attrs as usize) {
                        if let (Some(c), Some(p)) = (&attrs.component, &attrs.pin) {
                            self.pins.insert(i, (c.clone(), p.clone()));
                        }
                    }
                }
                Obj::Line { ap, a, b } => {
                    let w = geom::aperture_width(g.aperture(*ap));
                    let i = self.board.geometry.tracks.len();
                    self.board.geometry.tracks.push(Track {
                        layer: layer.clone(),
                        net: None,
                        start: pt(*a),
                        end: pt(*b),
                        width: mm(w),
                    });
                    self.named(ObjRef::Track(i), g, it.attrs);
                }
                Obj::Arc { ap, a, b, c, cw } => {
                    let w = geom::aperture_width(g.aperture(*ap));
                    if let Some(arc) = core_arc(layer.clone(), *c, *a, *b, *cw, w) {
                        let i = self.board.geometry.arcs.len();
                        self.board.geometry.arcs.push(arc);
                        self.named(ObjRef::Arc(i), g, it.attrs);
                    }
                }
                Obj::Region(cs) => {
                    let bb = cs
                        .iter()
                        .map(|c| geom::bbox(c))
                        .reduce(geom::bbox_union)
                        .unwrap_or([0.0; 4]);
                    regions.push((seq, cs.clone(), bb, it.attrs));
                }
            }
        }
        // Cut each region by the clear objects that come after it.
        let grid = Grid::new(clears.iter().map(|c| c.2).collect());
        for (seq, rings, bb, attrs) in regions {
            let cuts: Vec<Vec<Pt>> = grid
                .query(bb)
                .into_iter()
                .filter(|&k| clears[k].0 > seq)
                .map(|k| clears[k].1.clone())
                .collect();
            let shapes = if cuts.is_empty() && rings.len() == 1 {
                vec![rings]
            } else if cuts.is_empty() {
                geom::union(&rings)
            } else {
                geom::difference(&rings, &cuts)
            };
            for mut s in shapes {
                if s.is_empty() || s[0].len() < 3 {
                    continue;
                }
                let outline = s.remove(0);
                let i = self.board.geometry.polygons.len();
                self.board.geometry.polygons.push(Polygon {
                    layer: layer.clone(),
                    net: None,
                    outline: outline.into_iter().map(pt).collect(),
                    holes: s
                        .into_iter()
                        .map(|h| h.into_iter().map(pt).collect())
                        .collect(),
                });
                self.named(ObjRef::Poly(i), g, attrs);
            }
        }
    }

    fn silk(&mut self, layer: LayerRef, g: &Gerber) {
        for it in &g.items {
            if !it.dark {
                continue;
            }
            match &it.obj {
                Obj::Line { ap, a, b } => self.board.geometry.tracks.push(Track {
                    layer: layer.clone(),
                    net: None,
                    start: pt(*a),
                    end: pt(*b),
                    width: mm(geom::aperture_width(g.aperture(*ap))),
                }),
                Obj::Arc { ap, a, b, c, cw } => {
                    if let Some(arc) = core_arc(
                        layer.clone(),
                        *c,
                        *a,
                        *b,
                        *cw,
                        geom::aperture_width(g.aperture(*ap)),
                    ) {
                        self.board.geometry.arcs.push(arc);
                    }
                }
                Obj::Flash { ap, at, .. } => {
                    let w = geom::aperture_width(g.aperture(*ap));
                    self.board.geometry.tracks.push(Track {
                        layer: layer.clone(),
                        net: None,
                        start: pt(*at),
                        end: pt(*at),
                        width: mm(w),
                    });
                }
                Obj::Region(cs) => {
                    for c in cs {
                        // Silk fills are drawn as their outline.
                        for k in 0..c.len() {
                            self.board.geometry.tracks.push(Track {
                                layer: layer.clone(),
                                net: None,
                                start: pt(c[k]),
                                end: pt(c[(k + 1) % c.len()]),
                                width: mm(0.1),
                            });
                        }
                    }
                }
            }
        }
    }

    fn mask_open(&self, side: Side, p: Pt) -> bool {
        self.mask
            .get(&side)
            .is_some_and(|list| list.iter().any(|(c, r)| geom::dist(*c, p) <= r.max(0.05)))
    }

    /// Pads flashed on a copper layer whose copper covers `p` (within `tol`).
    fn pads_at(&self, layer: u8, p: Pt, tol: f64) -> Vec<usize> {
        let (cx, cy) = cell(p);
        let reach = (self.max_pad / PAD_CELL).ceil() as i64 + 1;
        let mut out = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                if let Some(list) = self.pad_grid.get(&(layer, cx + dx, cy + dy)) {
                    for &i in list {
                        let q = &self.board.geometry.pads[i];
                        let local =
                            geom::rotate([p[0] - q.at.x.mm(), p[1] - q.at.y.mm()], -q.rotation_deg);
                        if local[0].abs() <= q.size_x.mm() / 2.0 + tol
                            && local[1].abs() <= q.size_y.mm() / 2.0 + tol
                        {
                            out.push(i);
                        }
                    }
                }
            }
        }
        out
    }

    /// Share of round holes (up to 400) that land inside an outer-layer pad
    /// once moved by `p * s + o`.
    fn hit_rate(&self, d: &Drill, span: Option<(u8, u8)>, s: f64, o: Pt) -> f64 {
        let (first, last) = span.unwrap_or((0, self.copper.saturating_sub(1)));
        let holes: Vec<&crate::excellon::Hole> = d
            .holes
            .iter()
            .filter(|h| h.end.is_none() && h.plated != Some(false))
            .collect();
        if holes.is_empty() {
            return 1.0;
        }
        let step = (holes.len() / 400).max(1);
        let (mut n, mut hit) = (0, 0);
        for h in holes.iter().step_by(step) {
            let p = [h.at[0] * s + o[0], h.at[1] * s + o[1]];
            n += 1;
            if [first, last]
                .iter()
                .any(|&l| !self.pads_at(l, p, 0.01).is_empty())
            {
                hit += 1;
            }
        }
        hit as f64 / n as f64
    }

    /// When a drill file was written with another origin or number format
    /// than the Gerbers, its holes miss the pads. Finds the scale and shift
    /// that puts most holes back on pads.
    fn align_drill(&self, d: &Drill, span: Option<(u8, u8)>) -> Option<(f64, Pt)> {
        let holes: Vec<Pt> = d
            .holes
            .iter()
            .filter(|h| h.end.is_none() && h.plated != Some(false))
            .map(|h| h.at)
            .collect();
        if holes.len() < 8 {
            return None;
        }
        let base = self.hit_rate(d, span, 1.0, [0.0, 0.0]);
        if base >= 0.6 {
            return None;
        }
        let (first, last) = span.unwrap_or((0, self.copper.saturating_sub(1)));
        let pads: Vec<Pt> = self
            .board
            .geometry
            .pads
            .iter()
            .filter(|p| matches!(p.layer, LayerRef::Copper(l) if l == first || l == last))
            .map(|p| [p.at.x.mm(), p.at.y.mm()])
            .collect();
        if pads.is_empty() {
            return None;
        }
        let sample: Vec<Pt> = holes
            .iter()
            .step_by((holes.len() / 150).max(1))
            .copied()
            .collect();
        let pad_step = (pads.len() / 6000).max(1);
        let mut best: Option<(f64, f64, Pt)> = None;
        for s in [
            1.0,
            10.0,
            0.1,
            100.0,
            0.01,
            25.4,
            1.0 / 25.4,
            2.54,
            1.0 / 2.54,
            254.0,
            1.0 / 254.0,
        ] {
            let mut votes: HashMap<(i64, i64), u32> = HashMap::new();
            for h in &sample {
                for q in pads.iter().step_by(pad_step) {
                    let o = [q[0] - h[0] * s, q[1] - h[1] * s];
                    *votes
                        .entry(((o[0] * 100.0).round() as i64, (o[1] * 100.0).round() as i64))
                        .or_insert(0) += 1;
                }
            }
            let mut top: Vec<((i64, i64), u32)> = votes.into_iter().collect();
            top.sort_by_key(|b| std::cmp::Reverse(b.1));
            for (k, _) in top.into_iter().take(3) {
                let o = [k.0 as f64 / 100.0, k.1 as f64 / 100.0];
                // The moved holes must spread over the board, not bunch up
                // inside one big pad.
                let xs: Vec<f64> = holes.iter().map(|h| h[0] * s + o[0]).collect();
                let spread = xs.iter().cloned().fold(f64::MIN, f64::max)
                    - xs.iter().cloned().fold(f64::MAX, f64::min);
                let pad_spread = pads.iter().map(|p| p[0]).fold(f64::MIN, f64::max)
                    - pads.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
                if spread < pad_spread * 0.2 || spread > pad_spread * 2.0 {
                    continue;
                }
                let r = self.hit_rate(d, span, s, o);
                if best.is_none_or(|b| r > b.0) {
                    best = Some((r, s, o));
                }
            }
        }
        let (r, s, o) = best?;
        if std::env::var_os("TRACE_ALIGN_DEBUG").is_some() {
            eprintln!("align: base {base:.2}, best {r:.2} at scale {s} offset {o:?}");
        }
        (r >= 0.75 && r > base + 0.3).then_some((s, o))
    }

    fn holes(&mut self, d: &Drill, file_plated: Option<bool>, span: Option<(u8, u8)>) {
        let last = self.copper.saturating_sub(1);
        let (from, to) = span.unwrap_or((0, last));
        for h in &d.holes {
            let plated = h.plated.or(file_plated).unwrap_or_else(|| {
                // Unknown: plated when there is copper around the hole.
                (0..self.copper).any(|l| !self.pads_at(l, h.at, 0.01).is_empty())
            });
            if let Some(end) = h.end {
                // Slot: an oblong pad through the board.
                let len = geom::dist(h.at, end);
                let c = [(h.at[0] + end[0]) / 2.0, (h.at[1] + end[1]) / 2.0];
                let deg = (end[1] - h.at[1]).atan2(end[0] - h.at[0]).to_degrees();
                let mut size = h.diameter;
                if plated {
                    for l in [0, last] {
                        for i in self.pads_at(l, c, 0.05) {
                            let p = &self.board.geometry.pads[i];
                            size = size.max(p.size_x.mm().min(p.size_y.mm()));
                            self.dead.insert(i);
                        }
                    }
                }
                self.board.geometry.pads.push(Pad {
                    layer: LayerRef::AllCopper,
                    net: None,
                    component: None,
                    name: if plated { String::new() } else { "NPTH".into() },
                    at: pt(c),
                    size_x: mm(len + size),
                    size_y: mm(size),
                    hole: mm(h.diameter),
                    shape: PadShape::Round,
                    rotation_deg: deg,
                    outline: Vec::new(),
                    stack: Vec::new(),
                });
                continue;
            }
            if !plated {
                self.board.geometry.pads.push(Pad {
                    layer: LayerRef::AllCopper,
                    net: None,
                    component: None,
                    name: "NPTH".into(),
                    at: pt(h.at),
                    size_x: mm(h.diameter),
                    size_y: mm(h.diameter),
                    hole: mm(h.diameter),
                    shape: PadShape::Round,
                    rotation_deg: 0.0,
                    outline: Vec::new(),
                    stack: Vec::new(),
                });
                continue;
            }
            // Pads around the hole on each layer of its span.
            let tol = 0.01;
            let mut ring: Vec<(u8, f64)> = Vec::new();
            let mut found: Vec<usize> = Vec::new();
            let mut outer_size: f64 = 0.0;
            let mut outer_pad: Option<usize> = None;
            let mut via_fn = h
                .function
                .as_deref()
                .map(|f| f.contains("Via"))
                .unwrap_or(false);
            let comp_fn = h
                .function
                .as_deref()
                .is_some_and(|f| f.contains("Component") || f.contains("PressFit"));
            let mut layer_flash: Vec<(u8, usize)> = Vec::new();
            let partial = from.min(to) > 0 || from.max(to) < last;
            for l in from.min(to)..=from.max(to) {
                let at = self.pads_at(l, h.at, tol);
                let mut r: f64 = h.diameter / 2.0;
                for &i in &at {
                    let p = &self.board.geometry.pads[i];
                    if self
                        .flash_function
                        .get(&i)
                        .is_some_and(|f| f.starts_with("ViaPad"))
                    {
                        via_fn = true;
                    }
                    // Only the land centred on the hole belongs to it; a pad
                    // the hole sits in (via in pad) or beside stays separate.
                    if (p.at.x.mm() - h.at[0]).hypot(p.at.y.mm() - h.at[1]) > 0.02 {
                        continue;
                    }
                    // A blind or buried via centred in a part's pad (via in
                    // pad) leaves the pad as it is: the pad is only on its
                    // own layer, while the via is drilled through a few.
                    if partial && p.size_x.mm().max(p.size_y.mm()) > h.diameter + 0.9 {
                        continue;
                    }
                    r = r.max(p.size_x.mm().min(p.size_y.mm()) / 2.0);
                    if (l == from.min(to) || l == from.max(to))
                        && p.size_x.mm().max(p.size_y.mm()) > outer_size
                    {
                        outer_size = p.size_x.mm().max(p.size_y.mm());
                        outer_pad = Some(i);
                    }
                    found.push(i);
                    layer_flash.push((l, i));
                }
                ring.push((l, r));
            }
            let tented = !self.mask_open(Side::Top, h.at) && !self.mask_open(Side::Bottom, h.at);
            let is_via = partial
                || !comp_fn
                    && (via_fn
                        || (h.function.is_none()
                            && (tented || (h.diameter <= 0.4 && outer_size <= 0.9))));
            // The hole's own copper replaces the per-layer flashes.
            let named = found
                .iter()
                .find_map(|i| self.named.get(&ObjRef::Pad(*i)).cloned());
            let pin = found.iter().find_map(|i| self.pins.get(i).cloned());
            let outer = outer_pad.map(|i| {
                let p = &self.board.geometry.pads[i];
                (p.shape, p.size_x.mm(), p.size_y.mm(), p.rotation_deg)
            });
            self.dead.extend(found.iter().copied());
            if is_via {
                let vi = self.board.geometry.vias.len();
                self.board.geometry.vias.push(Via {
                    net: None,
                    from_layer: from.min(to),
                    to_layer: from.max(to),
                    at: pt(h.at),
                    diameter: mm(if outer_size > 0.0 {
                        outer_size
                    } else {
                        h.diameter + 0.2
                    }),
                    hole: mm(h.diameter),
                    rings: Vec::new(),
                });
                // Keep each layer's ring when they differ, for example
                // smaller inner pads or none where unused pads were removed.
                let d = self.board.geometry.vias[vi].diameter;
                if ring
                    .iter()
                    .any(|&(_, r)| (mm(2.0 * r).0 - d.0).abs() > 2000)
                {
                    self.board.geometry.vias[vi].rings =
                        ring.iter().map(|&(l, r)| (l, mm(2.0 * r))).collect();
                }
                self.via_rings.push(ring);
                if let Some(n) = named {
                    self.named.insert(ObjRef::Via(vi), n);
                }
            } else {
                let (shape, sx, sy, rot) =
                    outer.unwrap_or((PadShape::Round, h.diameter + 0.2, h.diameter + 0.2, 0.0));
                let pi = self.board.geometry.pads.len();
                self.board.geometry.pads.push(Pad {
                    layer: LayerRef::AllCopper,
                    net: None,
                    component: None,
                    name: String::new(),
                    at: pt(h.at),
                    size_x: mm(sx.max(h.diameter)),
                    size_y: mm(sy.max(h.diameter)),
                    hole: mm(h.diameter),
                    shape,
                    rotation_deg: rot,
                    outline: Vec::new(),
                    stack: Vec::new(),
                });
                self.pad_rings.push((pi, ring));
                // Keep the copper of each layer when it is not the same
                // everywhere: inner layers often have a smaller land, or
                // none at all where the pad passes a plane of another net.
                let lands: Vec<Land> = (0..self.copper)
                    .filter_map(|l| {
                        let &(_, i) = layer_flash.iter().find(|f| f.0 == l)?;
                        let p = &self.board.geometry.pads[i];
                        Some(Land {
                            layer: l,
                            shape: p.shape,
                            size_x: p.size_x,
                            size_y: p.size_y,
                            rotation_deg: p.rotation_deg,
                            outline: p.outline.clone(),
                        })
                    })
                    .collect();
                let q = &self.board.geometry.pads[pi];
                let same = lands.len() == self.copper as usize
                    && lands.iter().all(|d| {
                        d.shape == q.shape
                            && d.outline.is_empty()
                            && (d.size_x.0 - q.size_x.0).abs() < 2000
                            && (d.size_y.0 - q.size_y.0).abs() < 2000
                            && ((d.rotation_deg - q.rotation_deg).rem_euclid(180.0) < 0.5
                                || (d.rotation_deg - q.rotation_deg).rem_euclid(180.0) > 179.5
                                || d.size_x == d.size_y)
                    });
                if !same && !lands.is_empty() {
                    self.board.geometry.pads[pi].stack = lands;
                }
                for (l, i) in layer_flash {
                    let flash = self.board.geometry.pads[i].clone();
                    let exact = self.exact.get(&i).cloned();
                    self.layer_pads.insert((pi, l), (flash, exact));
                }
                if let Some(n) = named {
                    self.named.insert(ObjRef::Pad(pi), n);
                }
                if let Some(p) = pin {
                    self.pins.insert(pi, p);
                }
            }
        }
    }

    /// Drops flashes that became part of a via or through-hole pad and
    /// renumbers everything that refers to pads.
    fn remove_dead_pads(&mut self) {
        if self.dead.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.board.geometry.pads);
        let mut map = vec![usize::MAX; old.len()];
        for (i, p) in old.into_iter().enumerate() {
            if !self.dead.contains(&i) {
                map[i] = self.board.geometry.pads.len();
                self.board.geometry.pads.push(p);
            }
        }
        let remap = |i: usize| (map[i] != usize::MAX).then(|| map[i]);
        self.named = std::mem::take(&mut self.named)
            .into_iter()
            .filter_map(|(k, v)| match k {
                ObjRef::Pad(i) => remap(i).map(|j| (ObjRef::Pad(j), v)),
                k => Some((k, v)),
            })
            .collect();
        self.pins = std::mem::take(&mut self.pins)
            .into_iter()
            .filter_map(|(i, v)| remap(i).map(|j| (j, v)))
            .collect();
        self.pad_rings = std::mem::take(&mut self.pad_rings)
            .into_iter()
            .filter_map(|(i, v)| remap(i).map(|j| (j, v)))
            .collect();
        self.layer_pads = std::mem::take(&mut self.layer_pads)
            .into_iter()
            .filter_map(|((i, l), v)| remap(i).map(|j| ((j, l), v)))
            .collect();
        self.exact = std::mem::take(&mut self.exact)
            .into_iter()
            .filter_map(|(i, v)| remap(i).map(|j| (j, v)))
            .collect();
        self.flash_function.clear();
        self.pad_grid.clear();
        self.dead.clear();
    }

    /// Names traced nets after the IPC-356 netlist and takes part pins from
    /// it. A traced net that holds test points of several netlist nets is a
    /// short in the rebuilt copper, reported as a warning.
    fn apply_ipc(&mut self, nl: &crate::ipc356::Netlist, file: &str, extra: &nets::Extra) {
        use crate::ipc356::TestPoint;
        let g = &self.board.geometry;
        let last = self.copper.saturating_sub(1);
        let key = |p: Pt| ((p[0] / 1.0).floor() as i64, (p[1] / 1.0).floor() as i64);
        let mut pad_cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in g.pads.iter().enumerate() {
            pad_cells
                .entry(key([p.at.x.mm(), p.at.y.mm()]))
                .or_default()
                .push(i);
        }
        let mut via_cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, v) in g.vias.iter().enumerate() {
            via_cells
                .entry(key([v.at.x.mm(), v.at.y.mm()]))
                .or_default()
                .push(i);
        }
        let near = |cells: &HashMap<(i64, i64), Vec<usize>>, p: Pt| -> Vec<usize> {
            let (cx, cy) = key(p);
            let mut v = Vec::new();
            for dx in -6..=6 {
                for dy in -6..=6 {
                    if let Some(l) = cells.get(&(cx + dx, cy + dy)) {
                        v.extend_from_slice(l);
                    }
                }
            }
            v
        };
        enum Hit {
            Pad(usize),
            Via(usize),
        }
        let find = |t: &TestPoint| -> Option<Hit> {
            if t.via || t.refdes.is_empty() {
                // Stacked vias of different nets can share a spot (1-3 over
                // 14-16): take the one whose span holds the access layer.
                let layer = match t.access {
                    0 => None,
                    n => Some((n as u8).saturating_sub(1).min(last)),
                };
                let mut found: Option<(u8, usize)> = None;
                for v in near(&via_cells, t.at) {
                    let w = &g.vias[v];
                    if (w.at.x.mm() - t.at[0]).hypot(w.at.y.mm() - t.at[1]) >= 0.05 {
                        continue;
                    }
                    let (a, z) = (w.from_layer.min(w.to_layer), w.from_layer.max(w.to_layer));
                    let rank = match layer {
                        None => 0,
                        Some(l) if l == a || l == z => 0,
                        Some(l) if l > a && l < z => 1,
                        _ => 2,
                    };
                    if found.is_none_or(|f| rank < f.0) {
                        found = Some((rank, v));
                    }
                }
                if let Some((r, v)) = found {
                    if r < 2 {
                        return Some(Hit::Via(v));
                    }
                }
            }
            let want = |l: &LayerRef| match (t.access, l) {
                (0, _) => true,
                (1, LayerRef::Copper(0)) | (1, LayerRef::AllCopper) => true,
                (n, LayerRef::Copper(c)) if n > 1 => {
                    (*c as u32 + 1 == n) || (n as u8 > last && *c == last)
                }
                (_, LayerRef::AllCopper) => true,
                _ => false,
            };
            let mut best: Option<(f64, usize)> = None;
            for i in near(&pad_cells, t.at) {
                let q = &g.pads[i];
                if !want(&q.layer) {
                    continue;
                }
                let local = geom::rotate(
                    [t.at[0] - q.at.x.mm(), t.at[1] - q.at.y.mm()],
                    -q.rotation_deg,
                );
                if local[0].abs() <= q.size_x.mm() / 2.0 + 0.02
                    && local[1].abs() <= q.size_y.mm() / 2.0 + 0.02
                {
                    let d = local[0].hypot(local[1]);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, i));
                    }
                }
            }
            best.map(|(_, i)| Hit::Pad(i))
        };
        let skip =
            |n: &str| n.is_empty() || n.eq_ignore_ascii_case("N/C") || n.eq_ignore_ascii_case("NC");
        let take_pins = self.pins.is_empty();
        let mut votes: HashMap<u32, HashMap<String, (u32, Pt)>> = HashMap::new();
        let mut loose: Vec<(Hit, String)> = Vec::new();
        let mut matched = 0;
        for t in &nl.points {
            let Some(hit) = find(t) else { continue };
            matched += 1;
            if let (Hit::Pad(i), true) = (&hit, take_pins && !t.via && !t.refdes.is_empty()) {
                self.pins.insert(*i, (t.refdes.clone(), t.pin.clone()));
            }
            if skip(&t.net) {
                continue;
            }
            let net = match hit {
                Hit::Pad(i) => g.pads[i].net,
                Hit::Via(v) => g.vias[v].net,
            };
            match net {
                Some(n) => {
                    let e = votes
                        .entry(n)
                        .or_default()
                        .entry(t.net.clone())
                        .or_insert((0, t.at));
                    e.0 += 1;
                }
                None => loose.push((hit, t.net.clone())),
            }
        }
        if std::env::var_os("TRACE_NET_DEBUG").is_some() {
            // Re-trace with only the netlist names on pads and vias, so each
            // place two netlist nets meet is printed.
            let mut b2 = self.board.clone();
            b2.nets.clear();
            let mut ids: HashMap<String, u32> = HashMap::new();
            let g2 = &mut b2.geometry;
            g2.tracks.iter_mut().for_each(|t| t.net = None);
            g2.arcs.iter_mut().for_each(|t| t.net = None);
            g2.polygons.iter_mut().for_each(|t| t.net = None);
            g2.pads.iter_mut().for_each(|t| t.net = None);
            g2.vias.iter_mut().for_each(|t| t.net = None);
            for t in &nl.points {
                if skip(&t.net) {
                    continue;
                }
                let Some(hit) = find(t) else { continue };
                let next = ids.len() as u32;
                let id = *ids.entry(t.net.clone()).or_insert(next);
                match hit {
                    Hit::Pad(i) => g2.pads[i].net = Some(id),
                    Hit::Via(v) => g2.vias[v].net = Some(id),
                }
            }
            let mut names: Vec<(String, u32)> = ids.into_iter().collect();
            names.sort_by_key(|x| x.1);
            b2.nets = names
                .into_iter()
                .map(|(name, _)| Net {
                    name,
                    ..Default::default()
                })
                .collect();
            nets::trace(&mut b2, extra);
        }
        // Name each traced net after its most common netlist net.
        let mut used: HashMap<String, u32> = HashMap::new();
        let mut shorts: Vec<String> = Vec::new();
        let mut order: Vec<u32> = votes.keys().copied().collect();
        order.sort_by_key(|n| std::cmp::Reverse(votes[n].values().map(|v| v.0).sum::<u32>()));
        for n in order {
            let mut names: Vec<(&String, &(u32, Pt))> = votes[&n].iter().collect();
            names.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
            if names.len() > 1 {
                let p = names[1].1 .1;
                shorts.push(format!(
                    "{} and {} are joined in the copper (near {:.2}, {:.2} mm){}",
                    names[0].0,
                    names[1].0,
                    p[0],
                    p[1],
                    if names.len() > 2 {
                        format!(" with {} other nets", names.len() - 2)
                    } else {
                        String::new()
                    }
                ));
            }
            let base = names[0].0.clone();
            let k = used.entry(base.clone()).or_insert(0);
            *k += 1;
            self.board.nets[n as usize].name = if *k == 1 {
                base
            } else {
                format!("{base} ({k})")
            };
        }
        // Single-pad nets the tracer left unnamed.
        let mut ids: HashMap<String, u32> = self
            .board
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i as u32))
            .collect();
        for (hit, name) in loose {
            let id = *ids.entry(name.clone()).or_insert_with(|| {
                self.board.nets.push(Net {
                    name,
                    ..Default::default()
                });
                self.board.nets.len() as u32 - 1
            });
            let g = &mut self.board.geometry;
            match hit {
                Hit::Pad(i) => g.pads[i].net = Some(id),
                Hit::Via(v) => g.vias[v].net = Some(id),
            }
        }
        if matched * 2 < nl.points.len() {
            self.warnings.push(format!(
                "{file}: only {matched} of {} test points landed on a pad or via",
                nl.points.len()
            ));
        }
        let total = shorts.len();
        for s in shorts.into_iter().take(12) {
            self.warnings.push(s);
        }
        if total > 12 {
            self.warnings.push(format!(
                "{} more places where netlist nets are joined in the copper",
                total - 12
            ));
        }
    }

    /// Nets named by X2 attributes. Objects without a name stay unconnected.
    fn apply_named_nets(&mut self) {
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<(ObjRef, String)> = self.named.drain().collect();
        names.sort_by(|a, b| a.1.cmp(&b.1));
        for (r, name) in names {
            let next = ids.len() as u32;
            let id = *ids.entry(name.clone()).or_insert_with(|| {
                self.board.nets.push(Net {
                    name,
                    ..Default::default()
                });
                next
            });
            let g = &mut self.board.geometry;
            match r {
                ObjRef::Track(i) => g.tracks[i].net = Some(id),
                ObjRef::Arc(i) => g.arcs[i].net = Some(id),
                ObjRef::Pad(i) => g.pads[i].net = Some(id),
                ObjRef::Poly(i) => g.polygons[i].net = Some(id),
                ObjRef::Via(i) => g.vias[i].net = Some(id),
            }
        }
    }

    /// Parts from X2 `.P` (component, pin) attributes, placed at the middle
    /// of their pads.
    fn components(&mut self) {
        if self.pins.is_empty() {
            return;
        }
        let mut index: HashMap<String, u32> = self
            .board
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| (c.designator.clone(), i as u32))
            .collect();
        let known = self.board.components.len();
        let mut by_ref: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
        for (&pad, (refdes, pin)) in &self.pins {
            by_ref.entry(refdes.clone()).or_default().push(pad);
            self.board.geometry.pads[pad].name = pin.clone();
        }
        for (refdes, pads) in by_ref {
            let ci = match index.get(&refdes) {
                Some(&i) => i,
                None => {
                    let i = self.board.components.len() as u32;
                    index.insert(refdes.clone(), i);
                    self.board.components.push(Component {
                        designator: refdes,
                        footprint: String::new(),
                        library_ref: None,
                        x: Length(0),
                        y: Length(0),
                        rotation_deg: 0.0,
                        side: trace_core::Side::Top,
                        sheet: None,
                    });
                    i
                }
            };
            let (mut sx, mut sy) = (0i64, 0i64);
            let mut bottom = 0;
            for &p in &pads {
                let pad = &mut self.board.geometry.pads[p];
                pad.component = Some(ci);
                sx += pad.at.x.0;
                sy += pad.at.y.0;
                if pad.layer == LayerRef::Copper(self.copper.saturating_sub(1)) && self.copper > 1 {
                    bottom += 1;
                }
            }
            if (ci as usize) >= known {
                // Placed at the middle of its pads, on the side most pads are.
                let n = pads.len() as i64;
                let c = &mut self.board.components[ci as usize];
                c.x = Length(sx / n);
                c.y = Length(sy / n);
                if bottom * 2 > pads.len() {
                    c.side = trace_core::Side::Bottom;
                }
            }
        }
    }
}

/// Uniform grid over bounding boxes, for overlap queries.
pub struct Grid {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
    boxes: Vec<[f64; 4]>,
}

impl Grid {
    pub fn new(boxes: Vec<[f64; 4]>) -> Self {
        let cell = 2.0;
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, b) in boxes.iter().enumerate() {
            for gx in (b[0] / cell).floor() as i64..=(b[2] / cell).floor() as i64 {
                for gy in (b[1] / cell).floor() as i64..=(b[3] / cell).floor() as i64 {
                    map.entry((gx, gy)).or_default().push(i);
                }
            }
        }
        Grid { cell, map, boxes }
    }

    pub fn query(&self, b: [f64; 4]) -> Vec<usize> {
        let mut out = Vec::new();
        for gx in (b[0] / self.cell).floor() as i64..=(b[2] / self.cell).floor() as i64 {
            for gy in (b[1] / self.cell).floor() as i64..=(b[3] / self.cell).floor() as i64 {
                if let Some(list) = self.map.get(&(gx, gy)) {
                    out.extend(
                        list.iter()
                            .copied()
                            .filter(|&i| geom::bbox_overlap(self.boxes[i], b)),
                    );
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The board outline: the largest closed loop on an outline layer, or on a
/// mechanical layer when it frames the copper.
fn find_outline(layers: &[Layer], copper: Option<[f64; 4]>) -> Vec<Pt> {
    let rank = |r: &Role| match r {
        Role::Outline => Some(0),
        Role::Keepout => Some(1),
        Role::Mechanical(_) => Some(2),
        _ => None,
    };
    let mut cands: Vec<(u32, &Gerber)> = layers
        .iter()
        .filter_map(|l| match (&l.data, rank(&l.role)) {
            (LayerData::Gerber(g), Some(r)) => Some((r, g)),
            _ => None,
        })
        .collect();
    cands.sort_by_key(|c| c.0);
    let copper_area = copper.map(|b| (b[2] - b[0]) * (b[3] - b[1])).unwrap_or(0.0);
    for (rank, g) in cands {
        let loops = closed_loops(g);
        // Outline films often carry the drawing sheet's frame too: skip loops
        // far larger than the copper.
        let cap = if rank == 0 { 6.0 } else { 4.0 };
        let best = loops
            .into_iter()
            .filter(|l| copper_area <= 0.0 || geom::area(l).abs() <= copper_area * cap)
            .max_by(|a, b| geom::area(a).abs().total_cmp(&geom::area(b).abs()));
        if let Some(l) = best {
            let a = geom::area(&l).abs();
            if rank == 0 {
                return l;
            }
            // A mechanical frame must hold most of the copper.
            if let Some(cb) = copper {
                let c = [(cb[0] + cb[2]) / 2.0, (cb[1] + cb[3]) / 2.0];
                if geom::inside(std::slice::from_ref(&l), c)
                    && a >= copper_area * 0.7
                    && a <= copper_area * 4.0
                {
                    return l;
                }
            }
        }
    }
    match copper {
        Some(b) if b[2] > b[0] => geom::rect(
            [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0],
            b[2] - b[0],
            b[3] - b[1],
        ),
        _ => Vec::new(),
    }
}

/// Closed loops formed by the draws and arcs of a layer (centre lines).
fn closed_loops(g: &Gerber) -> Vec<Vec<Pt>> {
    let mut paths: Vec<Vec<Pt>> = Vec::new();
    let mut loops: Vec<Vec<Pt>> = Vec::new();
    for it in &g.items {
        match &it.obj {
            Obj::Line { a, b, .. } => {
                if geom::dist(*a, *b) > 1e-6 {
                    paths.push(vec![*a, *b]);
                }
            }
            Obj::Arc { a, b, c, cw, .. } => {
                let full = geom::dist(*a, *b) < 1e-9;
                let pts = geom::arc_points(*c, *a, *b, *cw, full);
                if full {
                    loops.push(pts);
                } else {
                    paths.push(pts);
                }
            }
            Obj::Region(cs) => loops.extend(cs.iter().cloned()),
            Obj::Flash { .. } => {}
        }
    }
    if paths.len() > 200_000 {
        return loops;
    }
    const TOL: f64 = 0.01;
    let key = |p: Pt| ((p[0] / TOL).round() as i64, (p[1] / TOL).round() as i64);
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        ends.entry(key(p[0])).or_default().push(i);
        ends.entry(key(*p.last().unwrap())).or_default().push(i);
    }
    let mut used = vec![false; paths.len()];
    let find = |p: Pt, used: &[bool], ends: &HashMap<(i64, i64), Vec<usize>>| -> Option<usize> {
        let (kx, ky) = key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(list) = ends.get(&(kx + dx, ky + dy)) {
                    for &j in list {
                        if !used[j] {
                            let q = &paths[j];
                            if geom::dist(q[0], p) <= TOL
                                || geom::dist(*q.last().unwrap(), p) <= TOL
                            {
                                return Some(j);
                            }
                        }
                    }
                }
            }
        }
        None
    };
    for start in 0..paths.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = paths[start].clone();
        loop {
            let end = *chain.last().unwrap();
            if chain.len() > 2 && geom::dist(end, chain[0]) <= TOL {
                chain.pop();
                loops.push(chain);
                break;
            }
            let Some(j) = find(end, &used, &ends) else {
                break;
            };
            used[j] = true;
            let mut next = paths[j].clone();
            if geom::dist(next[0], end) > TOL {
                next.reverse();
            }
            chain.extend(next.into_iter().skip(1));
        }
    }
    loops
}

/// The exact copper of a pad as one outline: the ring itself, or for a
/// pad with holes (a donut or a keypad ring) the outer ring with its holes
/// joined in through zero-width cuts. None when the copper is in several
/// separate pieces.
fn single_outline(rings: &[Vec<Pt>]) -> Option<Vec<Point>> {
    let to_pts = |r: &[Pt]| r.iter().map(|&p| pt(p)).collect::<Vec<Point>>();
    match rings {
        [] => None,
        [r] => Some(to_pts(r)),
        _ => {
            // A ring inside another is a hole.
            let inside =
                |r: &[Pt], o: &[Pt]| r.first().is_some_and(|&p| geom::inside(&[o.to_vec()], p));
            let outers: Vec<usize> = (0..rings.len())
                .filter(|&i| !(0..rings.len()).any(|j| j != i && inside(&rings[i], &rings[j])))
                .collect();
            let [o] = outers[..] else { return None };
            let holes: Vec<Vec<Point>> = (0..rings.len())
                .filter(|&i| i != o)
                .map(|i| to_pts(&rings[i]))
                .collect();
            let f = trace_core::fracture(&to_pts(&rings[o]), &holes);
            Some(f.iter().map(|&p| pt(p)).collect())
        }
    }
}
