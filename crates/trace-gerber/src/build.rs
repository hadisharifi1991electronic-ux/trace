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
    Point { x: mm(p[0]), y: mm(p[1]) }
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
    ((p[0] / PAD_CELL).floor() as i64, (p[1] / PAD_CELL).floor() as i64)
}

pub fn build(mut layers: Vec<Layer>, named_source: &'static str) -> Import {
    let mut b = Builder {
        board: Board::default(),
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
    let mut copper: Vec<usize> = (0..layers.len()).filter(|&i| layers[i].role.is_copper()).collect();
    let all_ordered = copper
        .iter()
        .all(|&i| matches!(layers[i].role, Role::Copper { order: Some(_), .. }));
    let key = |i: usize| -> (u32, u32, u32, u32, usize) {
        match &layers[i].role {
            Role::Copper { pos, number, order, plane } => {
                let rank = match pos {
                    CuPos::Top => 0,
                    CuPos::Inner => 1,
                    CuPos::Bottom => 2,
                };
                if all_ordered {
                    (order.unwrap_or(0), 0, 0, 0, i)
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
            let layer = if *side == Side::Top { LayerRef::TopSilk } else { LayerRef::BottomSilk };
            b.silk(layer, g);
        }
    }

    // ---- holes ----------------------------------------------------------
    for l in &layers {
        if let LayerData::Drill(d) = &l.data {
            let file_plated = match &l.role {
                Role::Drill { plated, .. } => *plated,
                _ => None,
            };
            let span = match &l.role {
                Role::Drill { span: Some((a, z)), .. } => {
                    let a = (*a as u8).saturating_sub(1).min(b.copper.saturating_sub(1));
                    let z = (*z as u8).saturating_sub(1).min(b.copper.saturating_sub(1));
                    Some((a, z))
                }
                _ => None,
            };
            b.holes(d, file_plated, span);
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
        "traced through the copper"
    };
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
        net.has_pour = b.board.geometry.polygons.iter().any(|p| p.net == Some(i as u32));
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
    if !g.apertures.iter().any(|a| matches!(a.shape, Shape::Block(_))) {
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
        Obj::Flash { ap, at, xf } => Obj::Flash { ap: *ap, at: t(*at), xf: *xf },
        Obj::Line { ap, a, b } => Obj::Line { ap: *ap, a: t(*a), b: t(*b) },
        Obj::Arc { ap, a, b, c, cw } => Obj::Arc { ap: *ap, a: t(*a), b: t(*b), c: t(*c), cw: *cw },
        Obj::Region(cs) => Obj::Region(cs.iter().map(|c| c.iter().map(|&p| t(p)).collect()).collect()),
    };
    Item { obj, dark: it.dark, attrs: it.attrs }
}

fn item_bbox(g: &Gerber, it: &Item) -> [f64; 4] {
    match &it.obj {
        Obj::Flash { ap, at, .. } => {
            let r = max_extent(g.aperture(*ap)) / 2.0;
            [at[0] - r, at[1] - r, at[0] + r, at[1] + r]
        }
        Obj::Line { ap, a, b } => {
            let r = geom::aperture_width(g.aperture(*ap)) / 2.0;
            [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[0].max(b[0]) + r, a[1].max(b[1]) + r]
        }
        Obj::Arc { ap, c, a, .. } => {
            let r = geom::dist(*c, *a) + geom::aperture_width(g.aperture(*ap)) / 2.0;
            [c[0] - r, c[1] - r, c[0] + r, c[1] + r]
        }
        Obj::Region(cs) => cs.iter().map(|c| geom::bbox(c)).reduce(geom::bbox_union).unwrap_or([0.0; 4]),
    }
}

fn layer_bbox(g: &Gerber) -> Option<[f64; 4]> {
    g.items.iter().map(|it| item_bbox(g, it)).reduce(geom::bbox_union)
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
    let off: Vec<Vec<Pt>> = shapes.iter().filter(|s| !s.0).map(|s| s.1.clone()).collect();
    let merged = if off.is_empty() { geom::union(&on) } else { geom::difference(&on, &off) };
    let area: f64 = merged.iter().map(|s| s.iter().map(|r| geom::area(r)).sum::<f64>().abs()).sum();
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
    let ang = |p: Pt| (p[1] - c[1]).atan2(p[0] - c[0]).to_degrees().rem_euclid(360.0);
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
                geom::rect([(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0], bb[2] - bb[0] + 2.0 * m, bb[3] - bb[1] + 2.0 * m)
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
                    });
                    self.pad_grid.entry((ci, cell(at).0, cell(at).1)).or_default().push(i);
                    self.max_pad = self.max_pad.max(w.hypot(h) / 2.0).min(5.0);
                    if let Some(f) = &a.function {
                        self.flash_function.insert(i, f.clone());
                    }
                    if let Shape::Macro(_) = a.shape {
                        let key = (*ap, [xf.rot.to_bits(), xf.scale.to_bits(), xf.mirror_x as u64, xf.mirror_y as u64]);
                        let local = self
                            .macro_cache
                            .entry(key)
                            .or_insert_with(|| exact_macro(a, xf, w * h))
                            .clone();
                        if let Some(rings) = local {
                            let flash = geom::add(at, [-off[0], -off[1]]);
                            self.exact.insert(i, rings.iter().map(|r| r.iter().map(|&p| geom::add(p, flash)).collect()).collect());
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
                    let bb = cs.iter().map(|c| geom::bbox(c)).reduce(geom::bbox_union).unwrap_or([0.0; 4]);
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
                    holes: s.into_iter().map(|h| h.into_iter().map(pt).collect()).collect(),
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
                    if let Some(arc) = core_arc(layer.clone(), *c, *a, *b, *cw, geom::aperture_width(g.aperture(*ap))) {
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
        self.mask.get(&side).is_some_and(|list| {
            list.iter().any(|(c, r)| geom::dist(*c, p) <= r.max(0.05))
        })
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
                        let local = geom::rotate([p[0] - q.at.x.mm(), p[1] - q.at.y.mm()], -q.rotation_deg);
                        if local[0].abs() <= q.size_x.mm() / 2.0 + tol && local[1].abs() <= q.size_y.mm() / 2.0 + tol {
                            out.push(i);
                        }
                    }
                }
            }
        }
        out
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
                });
                continue;
            }
            // Pads around the hole on each layer of its span.
            let tol = 0.01;
            let mut ring: Vec<(u8, f64)> = Vec::new();
            let mut found: Vec<usize> = Vec::new();
            let mut outer_size: f64 = 0.0;
            let mut outer_pad: Option<usize> = None;
            let mut via_fn = h.function.as_deref().map(|f| f.contains("Via")).unwrap_or(false);
            let comp_fn = h.function.as_deref().is_some_and(|f| f.contains("Component") || f.contains("PressFit"));
            let mut layer_flash: Vec<(u8, usize)> = Vec::new();
            for l in from.min(to)..=from.max(to) {
                let at = self.pads_at(l, h.at, tol);
                let mut r: f64 = h.diameter / 2.0;
                for &i in &at {
                    let p = &self.board.geometry.pads[i];
                    if self.flash_function.get(&i).is_some_and(|f| f.starts_with("ViaPad")) {
                        via_fn = true;
                    }
                    // Only the land centred on the hole belongs to it; a pad
                    // the hole sits in (via in pad) or beside stays separate.
                    if (p.at.x.mm() - h.at[0]).hypot(p.at.y.mm() - h.at[1]) > 0.02 {
                        continue;
                    }
                    r = r.max(p.size_x.mm().min(p.size_y.mm()) / 2.0);
                    if (l == from.min(to) || l == from.max(to)) && p.size_x.mm().max(p.size_y.mm()) > outer_size {
                        outer_size = p.size_x.mm().max(p.size_y.mm());
                        outer_pad = Some(i);
                    }
                    found.push(i);
                    layer_flash.push((l, i));
                }
                ring.push((l, r));
            }
            let tented = !self.mask_open(Side::Top, h.at) && !self.mask_open(Side::Bottom, h.at);
            let is_via = !comp_fn
                && (via_fn
                    || (h.function.is_none() && (tented || (h.diameter <= 0.4 && outer_size <= 0.9))));
            // The hole's own copper replaces the per-layer flashes.
            let named = found.iter().find_map(|i| self.named.get(&ObjRef::Pad(*i)).cloned());
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
                    diameter: mm(if outer_size > 0.0 { outer_size } else { h.diameter + 0.2 }),
                    hole: mm(h.diameter),
                });
                self.via_rings.push(ring);
                if let Some(n) = named {
                    self.named.insert(ObjRef::Via(vi), n);
                }
            } else {
                let (shape, sx, sy, rot) = outer.unwrap_or((PadShape::Round, h.diameter + 0.2, h.diameter + 0.2, 0.0));
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
                });
                self.pad_rings.push((pi, ring));
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

    /// Nets named by X2 attributes. Objects without a name stay unconnected.
    fn apply_named_nets(&mut self) {
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<(ObjRef, String)> = self.named.drain().collect();
        names.sort_by(|a, b| a.1.cmp(&b.1));
        for (r, name) in names {
            let next = ids.len() as u32;
            let id = *ids.entry(name.clone()).or_insert_with(|| {
                self.board.nets.push(Net { name, ..Default::default() });
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
        let mut by_ref: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
        for (&pad, (refdes, pin)) in &self.pins {
            by_ref.entry(refdes.clone()).or_default().push(pad);
            self.board.geometry.pads[pad].name = pin.clone();
        }
        for (refdes, pads) in by_ref {
            let ci = self.board.components.len() as u32;
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
            let n = pads.len() as i64;
            self.board.components.push(Component {
                designator: refdes,
                footprint: String::new(),
                library_ref: None,
                x: Length(sx / n),
                y: Length(sy / n),
                rotation_deg: 0.0,
                side: if bottom * 2 > pads.len() { trace_core::Side::Bottom } else { trace_core::Side::Top },
                sheet: None,
            });
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
                    out.extend(list.iter().copied().filter(|&i| geom::bbox_overlap(self.boxes[i], b)));
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
        let best = loops.into_iter().max_by(|a, b| geom::area(a).abs().total_cmp(&geom::area(b).abs()));
        if let Some(l) = best {
            let a = geom::area(&l).abs();
            if rank == 0 {
                return l;
            }
            // A mechanical frame must hold most of the copper.
            if let Some(cb) = copper {
                let c = [(cb[0] + cb[2]) / 2.0, (cb[1] + cb[3]) / 2.0];
                if geom::inside(&[l.clone()], c) && a >= copper_area * 0.7 && a <= copper_area * 4.0 {
                    return l;
                }
            }
        }
    }
    match copper {
        Some(b) if b[2] > b[0] => geom::rect([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0], b[2] - b[0], b[3] - b[1]),
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
                            if geom::dist(q[0], p) <= TOL || geom::dist(*q.last().unwrap(), p) <= TOL {
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
            let Some(j) = find(end, &used, &ends) else { break };
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
