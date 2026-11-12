//! Turns the raw `.brd` database into a Trace board: layers, nets, tracks,
//! arcs, vias, copper fills, parts with their pads, silkscreen and outline.

use crate::blocks::{class, Block, Data, Db, LayerInfo, LayerName, PadComp, Padstack};
use crate::Result;
use std::collections::HashMap;
use trace_core::*;

pub fn read_brd_bytes(data: &[u8]) -> Result<Board> {
    let db = Db::parse(data)?;
    Ok(Builder::new(&db).build())
}

struct Builder<'a> {
    db: &'a Db,
    /// Nanometres per file unit.
    scale: f64,
    copper: usize,
    nets: HashMap<u32, u32>,
    /// Nets with a real copper pour, which the routing check skips.
    poured: std::collections::HashSet<u32>,
    board: Board,
}

/// Silkscreen subclasses for the classes that carry visible legend.
fn silk_side(l: LayerInfo) -> Option<LayerRef> {
    match (l.class, l.sub) {
        (class::PACKAGE_GEOMETRY, 0xF7)
        | (class::BOARD_GEOMETRY, 0xF1)
        | (class::REF_DES, 0xFB) => Some(LayerRef::TopSilk),
        (class::PACKAGE_GEOMETRY, 0xF6)
        | (class::BOARD_GEOMETRY, 0xF0)
        | (class::REF_DES, 0xFA) => Some(LayerRef::BottomSilk),
        _ => None,
    }
}

fn deg(millideg: u32) -> f64 {
    millideg as f64 / 1000.0
}

impl<'a> Builder<'a> {
    fn new(db: &'a Db) -> Self {
        let base = match db.header.units {
            1 => 25_400.0,     // mils
            2 => 25_400_000.0, // inches
            3 => 1_000_000.0,  // millimetres
            4 => 10_000_000.0, // centimetres
            _ => 1_000.0,      // micrometres
        };
        Builder {
            db,
            scale: base / db.header.divisor as f64,
            copper: 0,
            nets: HashMap::new(),
            poured: Default::default(),
            board: Board::default(),
        }
    }

    fn len(&self, v: f64) -> Length {
        Length((v * self.scale).round() as i64)
    }

    fn pt(&self, x: f64, y: f64) -> Point {
        Point {
            x: self.len(x),
            y: self.len(y),
        }
    }

    fn net_of_assignment(&self, key: u32) -> Option<u32> {
        match self.db.get(key)?.data {
            Data::NetAssign { net, .. } => self.nets.get(&net).copied(),
            _ => None,
        }
    }

    fn copper_layer(&self, l: LayerInfo) -> Option<u8> {
        (l.class == class::ETCH && (l.sub as usize) < self.copper).then_some(l.sub)
    }

    fn build(mut self) -> Board {
        self.layers();
        self.net_list();
        self.routing();
        self.parts();
        self.outline();
        self.board_silk();
        let g = &self.board.geometry;
        self.board.stats = BoardStats {
            tracks: g.tracks.len(),
            vias: g.vias.len(),
            pads: g.pads.len(),
            polygons: g.polygons.len(),
        };
        connectivity::analyse(&mut self.board);
        for &n in &self.poured {
            if let Some(net) = self.board.nets.get_mut(n as usize) {
                net.has_pour = true;
            }
        }
        self.board
    }

    /// Copper layer names come from the ETCH class's layer list.
    fn layers(&mut self) {
        let key = self.db.header.layer_lists[class::ETCH as usize];
        let Some(Data::LayerList(list)) = self.db.get(key).map(|b| &b.data) else {
            return;
        };
        self.copper = list.len();
        for (i, (name, _)) in list.iter().enumerate() {
            let name = match name {
                LayerName::Text(s) => s.clone(),
                LayerName::Id(id) => self.db.string(*id).to_string(),
            };
            self.board.stackup.layers.push(StackLayer {
                name: if name.is_empty() {
                    format!("ETCH {}", i + 1)
                } else {
                    name
                },
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                material: None,
            });
        }
    }

    fn net_list(&mut self) {
        let ll = self.db.header.ll_nets;
        for b in self.db.walk(ll.head, ll.tail) {
            if let Data::Net { name, .. } = b.data {
                let n = self.board.nets.len();
                let name = self.db.string(name);
                self.nets.insert(b.key, n as u32);
                self.board.nets.push(Net {
                    name: if name.is_empty() {
                        format!("Net_{}", n + 1)
                    } else {
                        name.to_string()
                    },
                    ..Default::default()
                });
            }
        }
    }

    /// Tracks, vias and copper fills hang off each net's assignments.
    fn routing(&mut self) {
        let ll = self.db.header.ll_nets;
        let nets: Vec<&Block> = self.db.walk(ll.head, ll.tail);
        for net in nets {
            let Data::Net { assignment, .. } = net.data else {
                continue;
            };
            let id = self.nets.get(&net.key).copied();
            for a in self.db.walk(assignment, net.key) {
                let Data::NetAssign { conn, .. } = a.data else {
                    continue;
                };
                for item in self.db.walk(conn, a.key) {
                    match &item.data {
                        Data::Track { layer, first_seg } => {
                            if let Some(l) = self.copper_layer(*layer) {
                                self.track(*first_seg, item.key, LayerRef::Copper(l), id);
                            }
                        }
                        Data::Via { x, y, padstack, .. } => self.via(*x, *y, *padstack, id),
                        Data::Shape {
                            layer,
                            first_seg,
                            first_keepout,
                            flags,
                        } => {
                            if let Some(l) = self.copper_layer(*layer) {
                                // Bit 12 marks copper Allegro generated itself
                                // (teardrops, fillets) rather than a real pour.
                                let pour = flags & 0x1000 == 0;
                                self.fill(*first_seg, *first_keepout, LayerRef::Copper(l), id);
                                if pour {
                                    if let Some(n) = id {
                                        self.poured.insert(n);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Adds the segments and arcs of one track or graphic.
    fn track(&mut self, first: u32, owner: u32, layer: LayerRef, net: Option<u32>) {
        for b in self.db.walk(first, owner) {
            match &b.data {
                Data::Seg { width, start, end } => self.board.geometry.tracks.push(Track {
                    component: None,
                    layer: layer.clone(),
                    net,
                    start: self.pt(start.0 as f64, start.1 as f64),
                    end: self.pt(end.0 as f64, end.1 as f64),
                    width: self.len(*width as f64),
                }),
                Data::Arc { .. } => {
                    if let Some(a) = self.arc(b, layer.clone(), net) {
                        self.board.geometry.arcs.push(a);
                    }
                }
                _ => break,
            }
        }
    }

    fn arc(&self, b: &Block, layer: LayerRef, net: Option<u32>) -> Option<Arc> {
        let Data::Arc {
            cw,
            width,
            start,
            end,
            center,
            radius,
            ..
        } = b.data
        else {
            return None;
        };
        if !radius.is_finite() || !center.0.is_finite() || !center.1.is_finite() {
            return None;
        }
        let ang = |p: (i32, i32)| {
            (p.1 as f64 - center.1)
                .atan2(p.0 as f64 - center.0)
                .to_degrees()
        };
        let (a, e) = (ang(start), ang(end));
        // Trace arcs run counter-clockwise from start_deg to end_deg.
        let (start_deg, end_deg) = if start == end {
            (0.0, 360.0)
        } else if cw {
            (e, a)
        } else {
            (a, e)
        };
        Some(Arc {
            component: None,
            layer,
            net,
            center: self.pt(center.0, center.1),
            radius: self.len(radius),
            start_deg,
            end_deg,
            width: self.len(width as f64),
        })
    }

    /// A closed outline from a chain of segments and arcs, in file units.
    fn chain(&self, first: u32) -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = Vec::new();
        let mut key = first;
        let mut steps = 0;
        while let Some(b) = self.db.get(key) {
            steps += 1;
            if steps > 200_000 {
                break;
            }
            match b.data {
                Data::Seg { start, end, .. } => {
                    let s = (start.0 as f64, start.1 as f64);
                    if pts.last() != Some(&s) {
                        pts.push(s);
                    }
                    pts.push((end.0 as f64, end.1 as f64));
                }
                Data::Arc {
                    cw,
                    start,
                    end,
                    center,
                    radius,
                    ..
                } if radius.is_finite() => {
                    let a0 = (start.1 as f64 - center.1).atan2(start.0 as f64 - center.0);
                    let a1 = (end.1 as f64 - center.1).atan2(end.0 as f64 - center.0);
                    let tau = std::f64::consts::TAU;
                    let mut sweep = if cw { a0 - a1 } else { a1 - a0 };
                    sweep = sweep.rem_euclid(tau);
                    if start == end {
                        sweep = tau;
                    }
                    let n = ((sweep.to_degrees() / 10.0).ceil() as usize).max(2);
                    let s = (start.0 as f64, start.1 as f64);
                    if pts.last() != Some(&s) {
                        pts.push(s);
                    }
                    for i in 1..=n {
                        let t = sweep * i as f64 / n as f64;
                        let a = if cw { a0 - t } else { a0 + t };
                        pts.push((center.0 + radius * a.cos(), center.1 + radius * a.sin()));
                    }
                }
                _ => break,
            }
            key = b.next;
        }
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        pts
    }

    fn points(&self, raw: &[(f64, f64)]) -> Vec<Point> {
        raw.iter().map(|&(x, y)| self.pt(x, y)).collect()
    }

    fn fill(&mut self, first_seg: u32, first_keepout: u32, layer: LayerRef, net: Option<u32>) {
        let outline = self.chain(first_seg);
        if outline.len() < 3 {
            return;
        }
        let mut holes = Vec::new();
        let mut key = first_keepout;
        let mut seen = std::collections::HashSet::new();
        while let Some(b) = self.db.get(key) {
            let Data::Keepout { first_seg } = b.data else {
                break;
            };
            if !seen.insert(key) {
                break;
            }
            let h = self.chain(first_seg);
            if h.len() >= 3 {
                holes.push(self.points(&h));
            }
            key = b.next;
        }
        self.board.geometry.polygons.push(Polygon {
            layer,
            net,
            outline: self.points(&outline),
            holes,
            pour: None,
            component: None,
        });
    }

    /// Copper span and pad size of a padstack.
    fn via(&mut self, x: i32, y: i32, padstack: u32, net: Option<u32>) {
        let Some(Data::Padstack(ps)) = self.db.get(padstack).map(|b| &b.data) else {
            return;
        };
        let top = self.copper.saturating_sub(1);
        let count = ps.layer_count as usize;
        let (from, to) = if count > 1 && count < self.copper {
            let from = (ps.start_layer as usize).min(top);
            (from, (from + count - 1).min(top))
        } else {
            (0, top)
        };
        let diameter = (0..count)
            .find_map(|i| ps.pad(i))
            .map(|c| c.w.abs() as f64)
            .unwrap_or(ps.drill as f64 * 2.0);
        self.board.geometry.vias.push(Via {
            net,
            from_layer: from as u8,
            to_layer: to as u8,
            at: self.pt(x as f64, y as f64),
            diameter: self.len(diameter),
            hole: self.len(ps.drill as f64),
            rings: Vec::new(),
            fab: Default::default(),
        });
    }

    fn parts(&mut self) {
        let ll = self.db.header.ll_fp_defs;
        for def in self.db.walk(ll.head, ll.tail) {
            let Data::FpDef { name, first_inst } = def.data else {
                continue;
            };
            let footprint = self.db.string(name).to_string();
            for inst in self.db.walk(first_inst, def.key) {
                if let Data::FpInst { .. } = inst.data {
                    self.part(inst, &footprint);
                }
            }
        }
    }

    fn part(&mut self, inst: &Block, footprint: &str) {
        let Data::FpInst {
            bottom,
            rotation,
            x,
            y,
            inst_ref,
            graphics,
            first_pad,
        } = inst.data
        else {
            return;
        };
        let designator = match self.db.get(inst_ref).map(|b| &b.data) {
            Some(Data::CompInst { refdes }) => self.db.string(*refdes).to_string(),
            _ => String::new(),
        };
        let index = self.board.components.len() as u32;
        let rot = deg(rotation);
        self.board.components.push(Component {
            designator,
            footprint: footprint.to_string(),
            library_ref: None,
            x: self.len(x as f64),
            y: self.len(y as f64),
            rotation_deg: rot,
            side: if bottom { Side::Bottom } else { Side::Top },
            sheet: None,
            params: Default::default(),
        });

        let (s, c) = rot.to_radians().sin_cos();
        // Pad positions are in the part's own frame: mirror for the bottom,
        // then rotate and move to the placement.
        let place = |lx: f64, ly: f64| {
            let lx = if bottom { -lx } else { lx };
            (x as f64 + lx * c - ly * s, y as f64 + lx * s + ly * c)
        };
        let pads = self.db.walk_with(first_pad, inst.key, |b| match b.data {
            Data::PlacedPad { next_in_fp, .. } => next_in_fp,
            _ => 0,
        });
        for pp in pads {
            let Data::PlacedPad { net, pad, bbox, .. } = pp.data else {
                continue;
            };
            let Some(Data::Pad {
                name,
                x: px,
                y: py,
                padstack,
                rotation: prot,
            }) = self.db.get(pad).map(|b| &b.data)
            else {
                continue;
            };
            let Some(Data::Padstack(ps)) = self.db.get(*padstack).map(|b| &b.data) else {
                continue;
            };
            let net = self.net_of_assignment(net);
            let pad_rot = if bottom {
                rot - deg(*prot)
            } else {
                rot + deg(*prot)
            };
            let pad = self.pad(ps, place(*px as f64, *py as f64), pad_rot, bottom, bbox);
            if let Some(mut p) = pad {
                p.net = net;
                p.component = Some(index);
                p.name = self.db.string(*name).to_string();
                self.board.geometry.pads.push(p);
            }
        }

        // Part graphics are stored in board coordinates already.
        for g in self.db.walk(graphics, inst.key) {
            if let Data::Graphic { layer, first_seg } = g.data {
                if let Some(side) = silk_side(layer) {
                    self.track(first_seg, g.key, side, None);
                }
            }
        }
    }

    fn pad(
        &self,
        ps: &Padstack,
        at: (f64, f64),
        rotation_deg: f64,
        bottom: bool,
        bbox: [i32; 4],
    ) -> Option<Pad> {
        let count = ps.layer_count as usize;
        let through = count > 1 && ps.drill > 0;
        let comp: Option<&PadComp> = (0..count.max(1)).find_map(|i| ps.pad(i));
        let layer = if through || count == 0 {
            LayerRef::AllCopper
        } else {
            let start = ps.start_layer as usize;
            let top = self.copper.saturating_sub(1);
            let l = if bottom {
                top.saturating_sub(start)
            } else {
                start.min(top)
            };
            LayerRef::Copper(l as u8)
        };
        let (mut w, mut h, shape, off) = match comp {
            Some(c) => {
                let shape = match c.kind {
                    0x02 | 0x0B | 0x0C => PadShape::Round,
                    0x03 => PadShape::Octagon,
                    _ => PadShape::Rect,
                };
                let h = if c.kind == 0x05 { c.w } else { c.h };
                (
                    c.w.abs() as f64,
                    h.abs() as f64,
                    shape,
                    (c.off_x as f64, c.off_y as f64),
                )
            }
            None if through => (
                ps.drill as f64,
                ps.drill as f64,
                PadShape::Round,
                (0.0, 0.0),
            ),
            None => return None,
        };
        if w <= 0.0 || h <= 0.0 {
            // Custom shapes: fall back to the placed pad's extent.
            w = (bbox[2] - bbox[0]).abs() as f64;
            h = (bbox[3] - bbox[1]).abs() as f64;
        }
        let (s, c) = rotation_deg.to_radians().sin_cos();
        let ox = if bottom { -off.0 } else { off.0 };
        let center = (at.0 + ox * c - off.1 * s, at.1 + ox * s + off.1 * c);
        Some(Pad {
            layer,
            net: None,
            component: None,
            name: String::new(),
            at: self.pt(center.0, center.1),
            size_x: self.len(w),
            size_y: self.len(h),
            hole: if through {
                self.len(ps.drill as f64)
            } else {
                Length(0)
            },
            shape,
            rotation_deg,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        })
    }

    /// The board outline: the largest closed shape on an outline layer.
    fn outline(&mut self) {
        let h = &self.db.header;
        let is_outline = |l: LayerInfo| {
            matches!(
                (l.class, l.sub),
                (class::BOARD_GEOMETRY, 0xEA) | (class::BOARD_GEOMETRY, 0xFD)
            )
        };
        let mut best: Vec<(f64, f64)> = Vec::new();
        let area = |p: &[(f64, f64)]| {
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for &(x, y) in p {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            (x1 - x0) * (y1 - y0)
        };
        for ll in [h.ll_rects_shapes, h.ll_shapes, h.ll_graphics] {
            for b in self.db.walk(ll.head, ll.tail) {
                let pts = match b.data {
                    Data::Shape {
                        layer, first_seg, ..
                    }
                    | Data::Graphic { layer, first_seg } => {
                        if !is_outline(layer) {
                            continue;
                        }
                        self.chain(first_seg)
                    }
                    Data::Rect { layer, coords, .. } => {
                        if !is_outline(layer) {
                            continue;
                        }
                        let [x0, y0, x1, y1] = coords.map(|v| v as f64);
                        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                    }
                    _ => continue,
                };
                if pts.len() >= 3 && (best.is_empty() || area(&pts) > area(&best)) {
                    best = pts;
                }
            }
        }
        self.board.geometry.outline = self.points(&best);
    }

    /// Board-level silkscreen drawn outside parts.
    fn board_silk(&mut self) {
        let ll = self.db.header.ll_graphics;
        let items: Vec<(LayerRef, u32, u32)> = self
            .db
            .walk(ll.head, ll.tail)
            .into_iter()
            .filter_map(|b| match b.data {
                Data::Graphic { layer, first_seg } => {
                    silk_side(layer).map(|s| (s, first_seg, b.key))
                }
                _ => None,
            })
            .collect();
        for (side, first, key) in items {
            self.track(first, key, side, None);
        }
    }
}
