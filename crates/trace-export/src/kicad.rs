//! KiCad board files (`.kicad_pcb`, the KiCad 7 format, which KiCad 7, 8
//! and 9 all open).
//!
//! Tracks, arcs and vias are written as they are. KiCad keeps pads inside
//! footprints, so pads of a known part go into that part's footprint and
//! every other pad gets a one-pad footprint of its own. Copper pours become
//! zones that carry their current fill, so they look and connect exactly as
//! imported until KiCad is asked to refill them.

use std::collections::BTreeMap;
use std::fmt::Write;
use trace_core::{Arc, Board, LayerRef, Length, Pad, PadShape, Point, Side};

/// The board as the text of a `.kicad_pcb` file.
pub fn to_kicad(board: &Board) -> String {
    Writer::new(board).run()
}

struct Writer<'a> {
    b: &'a Board,
    copper: u8,
    out: String,
    uuid: u64,
    /// Inner-layer land of a through-hole pad (pad index -> land index).
    lands: std::collections::HashMap<usize, usize>,
    /// Inner-layer pads KiCad cannot hold as pads; written as small zones.
    inner: Vec<usize>,
    /// Inner-layer lands of through-hole pads, written as small zones:
    /// layer, outline in board mm, net.
    land_zones: Vec<(u8, Vec<[f64; 2]>, Option<u32>)>,
}

/// A number in mm, without trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

fn len(l: Length) -> String {
    num(l.mm())
}

/// KiCad's y axis points down.
fn xy(x: f64, y: f64) -> String {
    format!("{} {}", num(x), num(-y))
}

fn at(p: &Point) -> String {
    xy(p.x.mm(), p.y.mm())
}

fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Angle in degrees normalised to [0, 360).
fn angle(a: f64) -> String {
    let a = a.rem_euclid(360.0);
    num(if (a - 360.0).abs() < 1e-9 { 0.0 } else { a })
}

impl<'a> Writer<'a> {
    fn new(b: &'a Board) -> Self {
        let mut copper = b.stackup.copper_layers().count() as u8;
        let used = b
            .geometry
            .tracks
            .iter()
            .map(|t| &t.layer)
            .chain(b.geometry.arcs.iter().map(|a| &a.layer))
            .chain(b.geometry.pads.iter().map(|p| &p.layer))
            .chain(b.geometry.polygons.iter().map(|p| &p.layer))
            .filter_map(|l| match l {
                LayerRef::Copper(n) => Some(n + 1),
                _ => None,
            })
            .chain(
                b.geometry
                    .vias
                    .iter()
                    .map(|v| v.from_layer.max(v.to_layer) + 1),
            )
            .max()
            .unwrap_or(2);
        copper = copper.max(used).clamp(2, 32);
        // KiCad needs an even number of copper layers.
        if copper % 2 == 1 {
            copper += 1;
        }
        Writer {
            b,
            copper,
            out: String::new(),
            uuid: 0,
            lands: Default::default(),
            inner: Vec::new(),
            land_zones: Vec::new(),
        }
    }

    fn uuid(&mut self) -> String {
        self.uuid += 1;
        format!("7ace0000-0000-4000-8000-{:012x}", self.uuid)
    }

    /// KiCad layer number and name of copper layer `n` (0 = top).
    fn cu(&self, n: u8) -> (u8, String) {
        let last = self.copper - 1;
        if n == 0 {
            (0, "F.Cu".into())
        } else if n >= last {
            (31, "B.Cu".into())
        } else {
            (n, format!("In{n}.Cu"))
        }
    }

    /// Name of the KiCad layer for a non-pad item, or None when KiCad has no
    /// place for it.
    fn layer(&self, l: &LayerRef) -> Option<String> {
        Some(match l {
            LayerRef::Copper(n) => self.cu(*n).1,
            LayerRef::TopSilk => "F.SilkS".into(),
            LayerRef::BottomSilk => "B.SilkS".into(),
            LayerRef::Keepout => "Cmts.User".into(),
            LayerRef::Mechanical(_) => "Dwgs.User".into(),
            LayerRef::Other(_) => "Eco1.User".into(),
            LayerRef::AllCopper => return None,
        })
    }

    fn net(&self, n: Option<u32>) -> u32 {
        n.map(|n| n + 1).unwrap_or(0)
    }

    fn net_name(&self, n: Option<u32>) -> String {
        n.and_then(|n| self.b.nets.get(n as usize))
            .map(|n| quote(&n.name))
            .unwrap_or_else(|| "\"\"".into())
    }

    fn run(mut self) -> String {
        let b = self.b;
        let thickness = b.stackup.total_thickness().mm();
        let thickness = if thickness > 0.05 { thickness } else { 1.6 };
        let o = &mut self.out;
        o.push_str("(kicad_pcb (version 20221018) (generator trace)\n\n");
        let _ = writeln!(o, "  (general\n    (thickness {})\n  )\n", num(thickness));
        o.push_str("  (paper \"A4\")\n  (layers\n");
        let names: Vec<(String, bool)> = b
            .stackup
            .copper_layers()
            .map(|l| {
                (
                    l.name.clone(),
                    l.name.to_ascii_lowercase().contains("plane")
                        || l.name.to_ascii_lowercase().contains("gnd"),
                )
            })
            .collect();
        let copper = self.copper;
        for n in 0..copper {
            let (id, name) = self.cu(n);
            let user = names
                .get(n as usize)
                .map(|(u, _)| u.clone())
                .unwrap_or_default();
            let kind = if names.get(n as usize).is_some_and(|(_, plane)| *plane) {
                "power"
            } else {
                "signal"
            };
            let o = &mut self.out;
            if user.is_empty() || user == name {
                let _ = writeln!(o, "    ({id} \"{name}\" {kind})");
            } else {
                let _ = writeln!(o, "    ({id} \"{name}\" {kind} {})", quote(&user));
            }
        }
        let o = &mut self.out;
        for (id, name, user) in [
            (32, "B.Adhes", Some("B.Adhesive")),
            (33, "F.Adhes", Some("F.Adhesive")),
            (34, "B.Paste", None),
            (35, "F.Paste", None),
            (36, "B.SilkS", Some("B.Silkscreen")),
            (37, "F.SilkS", Some("F.Silkscreen")),
            (38, "B.Mask", None),
            (39, "F.Mask", None),
            (40, "Dwgs.User", Some("User.Drawings")),
            (41, "Cmts.User", Some("User.Comments")),
            (42, "Eco1.User", Some("User.Eco1")),
            (43, "Eco2.User", Some("User.Eco2")),
            (44, "Edge.Cuts", None),
            (45, "Margin", None),
            (46, "B.CrtYd", Some("B.Courtyard")),
            (47, "F.CrtYd", Some("F.Courtyard")),
            (48, "B.Fab", None),
            (49, "F.Fab", None),
        ] {
            match user {
                Some(u) => {
                    let _ = writeln!(o, "    ({id} \"{name}\" user \"{u}\")");
                }
                None => {
                    let _ = writeln!(o, "    ({id} \"{name}\" user)");
                }
            }
        }
        o.push_str("  )\n\n  (setup\n    (pad_to_mask_clearance 0)\n  )\n\n");
        o.push_str("  (net 0 \"\")\n");
        for (i, n) in b.nets.iter().enumerate() {
            let _ = writeln!(o, "  (net {} {})", i + 1, quote(&n.name));
        }
        o.push('\n');

        self.footprints();
        self.graphics();
        self.copper_items();
        self.zones();
        self.out.push(')');
        self.out.push('\n');
        self.out
    }

    // ---- footprints -----------------------------------------------------

    fn footprints(&mut self) {
        let b = self.b;
        let last = self.copper_count().saturating_sub(1);
        // Pads on inner layers: lands of through-hole pads, or loose copper.
        let key = |p: &Pad| (p.at.x.0 / 5000, p.at.y.0 / 5000);
        let mut th: std::collections::HashMap<(i64, i64), usize> = Default::default();
        for (i, p) in b.geometry.pads.iter().enumerate() {
            if p.layer == LayerRef::AllCopper && p.hole.0 > 0 {
                th.insert(key(p), i);
            }
        }
        let mut skip = std::collections::HashSet::new();
        for (i, p) in b.geometry.pads.iter().enumerate() {
            if let LayerRef::Copper(l) = p.layer {
                if l > 0 && l < last {
                    skip.insert(i);
                    match th.get(&key(p)) {
                        Some(&t) => {
                            self.lands.entry(t).or_insert(i);
                        }
                        None => self.inner.push(i),
                    }
                }
            }
        }
        let mut by_part: BTreeMap<u32, Vec<&Pad>> = BTreeMap::new();
        let mut loose: Vec<&Pad> = Vec::new();
        for (i, p) in b.geometry.pads.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            match p.component {
                Some(c) if (c as usize) < b.components.len() => {
                    by_part.entry(c).or_default().push(p)
                }
                _ => loose.push(p),
            }
        }
        for (i, c) in b.components.iter().enumerate() {
            let pads = by_part.remove(&(i as u32)).unwrap_or_default();
            let fp = if c.footprint.is_empty() {
                "Trace:Part".to_string()
            } else {
                format!("Trace:{}", c.footprint)
            };
            let origin = Point { x: c.x, y: c.y };
            self.footprint(
                &fp,
                &c.designator,
                c.library_ref.as_deref().unwrap_or(""),
                origin,
                c.rotation_deg,
                c.side,
                &pads,
            );
        }
        for (n, p) in loose.iter().enumerate() {
            let side = if matches!(p.layer, LayerRef::Copper(l) if l > 0 && l + 1 >= self.copper_count())
            {
                Side::Bottom
            } else {
                Side::Top
            };
            let name = if p.name == "NPTH" {
                format!("H{}", n + 1)
            } else {
                format!("P{}", n + 1)
            };
            self.footprint("Trace:Pad", &name, "", p.at, 0.0, side, &[p]);
        }
    }

    /// Copper layers in the source board (before KiCad's even-count rounding).
    fn copper_count(&self) -> u8 {
        let n = self.b.stackup.copper_layers().count() as u8;
        if n == 0 {
            self.copper
        } else {
            n
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn footprint(
        &mut self,
        lib: &str,
        reference: &str,
        value: &str,
        origin: Point,
        rot: f64,
        side: Side,
        pads: &[&Pad],
    ) {
        let id = self.uuid();
        let layer = if side == Side::Bottom { "B.Cu" } else { "F.Cu" };
        let (silk, fab) = if side == Side::Bottom {
            ("B.SilkS", "B.Fab")
        } else {
            ("F.SilkS", "F.Fab")
        };
        let mirror = if side == Side::Bottom {
            " (justify mirror)"
        } else {
            ""
        };
        let rot_s = if rot.rem_euclid(360.0).abs() < 1e-9 {
            String::new()
        } else {
            format!(" {}", angle(rot))
        };
        let o = &mut self.out;
        let _ = writeln!(
            o,
            "  (footprint {} (layer \"{layer}\")\n    (tstamp {id})\n    (at {}{rot_s})",
            quote(lib),
            at(&origin),
        );
        let _ = writeln!(
            o,
            "    (fp_text reference {} (at 0 0{rot_s}) (layer \"{silk}\") hide\n      (effects (font (size 1 1) (thickness 0.15)){mirror})\n    )",
            quote(reference)
        );
        let _ = writeln!(
            o,
            "    (fp_text value {} (at 0 0{rot_s}) (layer \"{fab}\") hide\n      (effects (font (size 1 1) (thickness 0.15)){mirror})\n    )",
            quote(value)
        );
        let (s, c) = rot.to_radians().sin_cos();
        for p in pads {
            // Offset from the footprint origin in KiCad's y-down axes, turned
            // back by the footprint's rotation.
            let dx = p.at.x.mm() - origin.x.mm();
            let dy = -(p.at.y.mm() - origin.y.mm());
            let lx = dx * c - dy * s;
            let ly = dx * s + dy * c;
            self.pad(p, lx, ly);
        }
        self.out.push_str("  )\n");
    }

    fn pad(&mut self, p: &Pad, lx: f64, ly: f64) {
        if !p.stack.is_empty() && p.hole.0 > 0 && p.layer == LayerRef::AllCopper {
            self.stacked_pad(p, lx, ly);
            return;
        }
        let index = (p as *const Pad as usize - self.b.geometry.pads.as_ptr() as usize)
            / std::mem::size_of::<Pad>();
        if let Some(&land) = self.lands.get(&index) {
            // KiCad 7 gives a pad one shape on every layer: use the inner
            // land for the through-hole pad and add the outer shape as pads
            // on the two outer layers.
            let land = &self.b.geometry.pads[land];
            let mut inner = land.clone();
            inner.layer = LayerRef::AllCopper;
            inner.hole = p.hole;
            inner.net = p.net;
            inner.name = p.name.clone();
            inner.at = p.at;
            self.pad_shape(&inner, lx, ly, "thru_hole", "\"*.Cu\" \"*.Mask\"", "");
            let same = land.shape == p.shape && land.size_x == p.size_x && land.size_y == p.size_y;
            if !same {
                self.pad_shape(p, lx, ly, "smd", "\"F.Cu\" \"F.Mask\"", "");
                self.pad_shape(p, lx, ly, "smd", "\"B.Cu\" \"B.Mask\"", "");
            }
            return;
        }
        let last = self.copper_count().saturating_sub(1);
        let hole = p.hole.mm();
        let npth = hole > 0.0
            && p.net.is_none()
            && (p.name == "NPTH" || p.size_x.0.min(p.size_y.0) <= p.hole.0);
        let (kind, layers) = match &p.layer {
            LayerRef::AllCopper if npth => ("np_thru_hole", "\"*.Cu\" \"*.Mask\"".to_string()),
            LayerRef::AllCopper if hole > 0.0 => ("thru_hole", "\"*.Cu\" \"*.Mask\"".to_string()),
            LayerRef::Copper(0) | LayerRef::AllCopper => {
                ("smd", "\"F.Cu\" \"F.Paste\" \"F.Mask\"".to_string())
            }
            LayerRef::Copper(l) if *l >= last => {
                ("smd", "\"B.Cu\" \"B.Paste\" \"B.Mask\"".to_string())
            }
            LayerRef::Copper(l) => ("smd", format!("\"{}\"", self.cu(*l).1)),
            _ => return,
        };
        let (mut w, mut h) = (p.size_x.mm(), p.size_y.mm());
        if npth {
            w = w.max(hole);
            h = h.max(hole);
        }
        if p.outline.len() >= 3 && !npth {
            self.custom_pad(p, kind, &layers, lx, ly);
            return;
        }
        let mut q = p.clone();
        q.size_x = Length::from_mm(w);
        q.size_y = Length::from_mm(h);
        if npth {
            q.name.clear();
            q.net = None;
        }
        // Through-hole pads with no land of their own on inner layers stay
        // off the inner layers they do not connect on, as in the source.
        let extra = if kind == "thru_hole" && !self.b.geometry.per_layer_copper {
            " (remove_unused_layers) (keep_end_layers)"
        } else {
            ""
        };
        self.pad_shape(&q, lx, ly, kind, &layers, extra);
    }

    /// A through-hole pad whose copper differs by layer. KiCad 7 gives a pad
    /// one shape on every layer, so the hole goes in as a plated hole with
    /// no ring, the outer lands as pads on the outer layers, and the inner
    /// lands as small zones of the pad's net. Layers without a land keep
    /// only the hole, as in the source.
    fn stacked_pad(&mut self, p: &Pad, lx: f64, ly: f64) {
        let last = self.copper_count().saturating_sub(1);
        let mut barrel = p.clone();
        barrel.shape = PadShape::Round;
        barrel.size_x = p.hole;
        barrel.size_y = p.hole;
        barrel.rotation_deg = 0.0;
        barrel.outline.clear();
        self.pad_shape(&barrel, lx, ly, "thru_hole", "\"*.Cu\"", "");
        for land in &p.stack {
            let mut q = p.clone();
            q.shape = land.shape;
            q.size_x = land.size_x;
            q.size_y = land.size_y;
            q.rotation_deg = land.rotation_deg;
            q.outline = land.outline.clone();
            let layers = if land.layer == 0 {
                "\"F.Cu\" \"F.Mask\""
            } else if land.layer >= last {
                "\"B.Cu\" \"B.Mask\""
            } else {
                let pts = if q.outline.len() >= 3 {
                    q.outline.iter().map(|v| [v.x.mm(), v.y.mm()]).collect()
                } else {
                    pad_polygon(&q)
                };
                self.land_zones.push((land.layer, pts, p.net));
                continue;
            };
            if q.outline.len() >= 3 {
                self.custom_pad(&q, "smd", layers, lx, ly);
            } else {
                self.pad_shape(&q, lx, ly, "smd", layers, "");
            }
        }
    }

    fn pad_shape(&mut self, p: &Pad, lx: f64, ly: f64, kind: &str, layers: &str, extra: &str) {
        let (w, h) = (p.size_x.mm(), p.size_y.mm());
        let hole = p.hole.mm();
        let shape = match p.shape {
            PadShape::Round if (w - h).abs() < 1e-6 => "circle",
            PadShape::Round => "oval",
            PadShape::Rect => "rect",
            PadShape::Octagon => "roundrect",
        };
        let rot = if p.rotation_deg.rem_euclid(360.0).abs() < 1e-9 {
            String::new()
        } else {
            format!(" {}", angle(p.rotation_deg))
        };
        let id = self.uuid();
        let net = if p.net.is_some() {
            format!(" (net {} {})", self.net(p.net), self.net_name(p.net))
        } else {
            String::new()
        };
        let o = &mut self.out;
        let _ = write!(
            o,
            "    (pad {} {kind} {shape} (at {}{rot}) (size {} {})",
            quote(&p.name),
            xy(lx, -ly),
            num(w),
            num(h)
        );
        if hole > 0.0 && kind != "smd" {
            let _ = write!(o, " (drill {})", num(hole));
        }
        let _ = write!(o, " (layers {layers}){extra}");
        if p.shape == PadShape::Octagon {
            o.push_str(" (roundrect_rratio 0) (chamfer_ratio 0.2929) (chamfer top_left top_right bottom_left bottom_right)");
        }
        let _ = writeln!(o, "{net} (tstamp {id}))");
    }

    /// A pad with an exact outline, as a KiCad custom pad: a tiny round
    /// anchor plus the outline as a filled polygon, at angle 0 so the
    /// polygon is just the offset from the pad in board axes.
    fn custom_pad(&mut self, p: &Pad, kind: &str, layers: &str, lx: f64, ly: f64) {
        let id = self.uuid();
        let net = if p.net.is_some() {
            format!(" (net {} {})", self.net(p.net), self.net_name(p.net))
        } else {
            String::new()
        };
        let drill = if p.hole.0 > 0 && kind != "smd" {
            format!(" (drill {})", len(p.hole))
        } else {
            String::new()
        };
        let a = if p.hole.0 > 0 {
            p.hole.mm() * 1.05
        } else {
            0.02
        };
        let o = &mut self.out;
        let _ = write!(
            o,
            "    (pad {} {kind} custom (at {}) (size {} {}){drill} (layers {layers})\n      (options (clearance outline) (anchor circle))\n      (primitives\n        (gr_poly\n          (pts",
            quote(&p.name),
            xy(lx, -ly),
            num(a),
            num(a)
        );
        for q in &p.outline {
            let _ = write!(
                o,
                " (xy {})",
                xy(q.x.mm() - p.at.x.mm(), q.y.mm() - p.at.y.mm())
            );
        }
        let _ = writeln!(
            o,
            ")\n          (width 0) (fill yes))\n      ){net} (tstamp {id}))"
        );
    }

    // ---- graphics -------------------------------------------------------

    fn graphics(&mut self) {
        let b = self.b;
        let outline = &b.geometry.outline;
        if outline.len() >= 3 {
            for i in 0..outline.len() {
                let (a, c) = (&outline[i], &outline[(i + 1) % outline.len()]);
                if a == c {
                    continue;
                }
                let id = self.uuid();
                let _ = writeln!(
                    self.out,
                    "  (gr_line (start {}) (end {}) (stroke (width 0.1) (type solid)) (layer \"Edge.Cuts\") (tstamp {id}))",
                    at(a),
                    at(c)
                );
            }
        }
        for t in &b.geometry.tracks {
            if matches!(t.layer, LayerRef::Copper(_)) {
                continue;
            }
            let Some(l) = self.layer(&t.layer) else {
                continue;
            };
            let id = self.uuid();
            let _ = writeln!(
                self.out,
                "  (gr_line (start {}) (end {}) (stroke (width {}) (type solid)) (layer \"{l}\") (tstamp {id}))",
                at(&t.start),
                at(&t.end),
                len(t.width)
            );
        }
        for a in &b.geometry.arcs {
            if matches!(a.layer, LayerRef::Copper(_)) {
                continue;
            }
            let Some(l) = self.layer(&a.layer) else {
                continue;
            };
            for (s, m, e) in arc_points(a) {
                let id = self.uuid();
                let _ = writeln!(
                    self.out,
                    "  (gr_arc (start {s}) (mid {m}) (end {e}) (stroke (width {}) (type solid)) (layer \"{l}\") (tstamp {id}))",
                    len(a.width)
                );
            }
        }
        for p in &b.geometry.polygons {
            if matches!(p.layer, LayerRef::Copper(_)) || p.outline.len() < 3 {
                continue;
            }
            let Some(l) = self.layer(&p.layer) else {
                continue;
            };
            let pts = fracture(&p.outline, &p.holes);
            let id = self.uuid();
            let o = &mut self.out;
            o.push_str("  (gr_poly\n    (pts\n");
            pts_block(o, &pts, "     ");
            let _ = writeln!(o, "    )\n    (stroke (width 0) (type solid)) (fill solid) (layer \"{l}\") (tstamp {id}))");
        }
    }

    // ---- copper ---------------------------------------------------------

    fn copper_items(&mut self) {
        let b = self.b;
        for t in &b.geometry.tracks {
            let LayerRef::Copper(n) = t.layer else {
                continue;
            };
            if t.start == t.end {
                // KiCad drops zero-length segments; keep the dot as a tiny one.
                let id = self.uuid();
                let e = xy(t.end.x.mm() + 0.0001, t.end.y.mm());
                let _ = writeln!(
                    self.out,
                    "  (segment (start {}) (end {e}) (width {}) (layer \"{}\") (net {}) (tstamp {id}))",
                    at(&t.start),
                    len(t.width),
                    self.cu(n).1,
                    self.net(t.net)
                );
                continue;
            }
            let id = self.uuid();
            let _ = writeln!(
                self.out,
                "  (segment (start {}) (end {}) (width {}) (layer \"{}\") (net {}) (tstamp {id}))",
                at(&t.start),
                at(&t.end),
                len(t.width),
                self.cu(n).1,
                self.net(t.net)
            );
        }
        for a in &b.geometry.arcs {
            let LayerRef::Copper(n) = a.layer else {
                continue;
            };
            for (s, m, e) in arc_points(a) {
                let id = self.uuid();
                let _ = writeln!(
                    self.out,
                    "  (arc (start {s}) (mid {m}) (end {e}) (width {}) (layer \"{}\") (net {}) (tstamp {id}))",
                    len(a.width),
                    self.cu(n).1,
                    self.net(a.net)
                );
            }
        }
        let last = self.copper_count().saturating_sub(1);
        for v in &b.geometry.vias {
            let (a, z) = (
                v.from_layer.min(v.to_layer),
                v.from_layer.max(v.to_layer).min(last),
            );
            let through = a == 0 && z >= last;
            let kind = if through { "" } else { " blind" };
            // KiCad 7 gives a via one size on every layer. With per-layer
            // rings the via takes its end-layer land; a larger ring on any
            // layer, and every inner land when some layer has none, becomes a
            // small round zone of the via's net, so the copper is there
            // whatever KiCad decides when it removes unused pads.
            let mut size = v.diameter.max(v.hole);
            let mut unused = !b.geometry.per_layer_copper;
            if !v.rings.is_empty() {
                let land = |d: Length| d.0 > v.hole.0 + 2000;
                let ring = |l: u8| v.rings.iter().find(|r| r.0 == l).map(|r| r.1);
                let ends: Vec<Length> = [a, z].iter().filter_map(|&l| ring(l)).collect();
                size = if ends.len() == 2 && ends.iter().all(|&d| land(d)) {
                    ends.iter().copied().min().unwrap_or(size)
                } else {
                    v.hole
                };
                unused = v.rings.iter().any(|r| !land(r.1)) || v.rings.len() < (z - a + 1) as usize;
                for &(l, d) in &v.rings {
                    let inner = l != a && l != z;
                    if land(d) && (d.0 > size.0 + 2000 || (unused && inner)) {
                        let r = d.mm() / 2.0;
                        let (cx, cy) = (v.at.x.mm(), v.at.y.mm());
                        let pts = (0..24)
                            .map(|i| {
                                let t = i as f64 * std::f64::consts::TAU / 24.0;
                                [cx + r * t.cos(), cy + r * t.sin()]
                            })
                            .collect();
                        self.land_zones.push((l, pts, v.net));
                    }
                }
            }
            let flags = if unused {
                " (remove_unused_layers) (keep_end_layers)"
            } else {
                ""
            };
            let id = self.uuid();
            let _ = writeln!(
                self.out,
                "  (via{kind} (at {}) (size {}) (drill {}) (layers \"{}\" \"{}\"){flags} (net {}) (tstamp {id}))",
                at(&v.at),
                len(size),
                len(v.hole),
                self.cu(a).1,
                self.cu(z).1,
                self.net(v.net)
            );
        }
    }

    fn zones(&mut self) {
        let b = self.b;
        let mut small = std::mem::take(&mut self.land_zones);
        for i in std::mem::take(&mut self.inner) {
            let p = &b.geometry.pads[i];
            let LayerRef::Copper(n) = p.layer else {
                continue;
            };
            let pts = if p.outline.len() >= 3 {
                p.outline.iter().map(|q| [q.x.mm(), q.y.mm()]).collect()
            } else {
                pad_polygon(p)
            };
            small.push((n, pts, p.net));
        }
        for (n, pts, pnet) in small {
            let layer = self.cu(n).1;
            let id = self.uuid();
            let net = self.net(pnet);
            let name = self.net_name(pnet);
            let o = &mut self.out;
            let _ = writeln!(
                o,
                "  (zone (net {net}) (net_name {name}) (layer \"{layer}\") (tstamp {id}) (hatch edge 0.5)\n    (priority 2)\n    (connect_pads yes (clearance 0))\n    (min_thickness 0.01) (filled_areas_thickness no)\n    (fill yes (thermal_gap 0.25) (thermal_bridge_width 0.25))\n    (polygon\n      (pts"
            );
            pts_block(o, &pts, "       ");
            let _ = writeln!(
                o,
                "      )\n    )\n    (filled_polygon\n      (layer \"{layer}\")\n      (pts"
            );
            pts_block(o, &pts, "       ");
            o.push_str("      )\n    )\n  )\n");
        }
        for p in &b.geometry.polygons {
            let LayerRef::Copper(n) = p.layer else {
                continue;
            };
            if p.outline.len() < 3 {
                continue;
            }
            let layer = self.cu(n).1;
            let id = self.uuid();
            let fill = fracture(&p.outline, &p.holes);
            let o = &mut self.out;
            let net = p.net.map(|n| n + 1).unwrap_or(0);
            let name = p
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| quote(&n.name))
                .unwrap_or_else(|| "\"\"".into());
            let _ = writeln!(
                o,
                "  (zone (net {net}) (net_name {name}) (layer \"{layer}\") (tstamp {id}) (hatch edge 0.5)\n    (priority 1)\n    (connect_pads yes (clearance 0))\n    (min_thickness 0.01) (filled_areas_thickness no)\n    (fill yes (thermal_gap 0.25) (thermal_bridge_width 0.25) (island_removal_mode 1))\n    (polygon\n      (pts"
            );
            let outer: Vec<[f64; 2]> = ccw(&p.outline);
            pts_block(o, &outer, "       ");
            let _ = writeln!(
                o,
                "      )\n    )\n    (filled_polygon\n      (layer \"{layer}\")\n      (pts"
            );
            pts_block(o, &fill, "       ");
            o.push_str("      )\n    )\n  )\n");
        }
    }
}

/// Outline of a plain pad, counter-clockwise, in board mm (y up).
fn pad_polygon(p: &Pad) -> Vec<[f64; 2]> {
    let (w, h) = (p.size_x.mm(), p.size_y.mm());
    let (s, c) = p.rotation_deg.to_radians().sin_cos();
    let (cx, cy) = (p.at.x.mm(), p.at.y.mm());
    let place = |u: f64, v: f64| [cx + u * c - v * s, cy + u * s + v * c];
    match p.shape {
        PadShape::Rect | PadShape::Octagon => vec![
            place(-w / 2.0, -h / 2.0),
            place(w / 2.0, -h / 2.0),
            place(w / 2.0, h / 2.0),
            place(-w / 2.0, h / 2.0),
        ],
        PadShape::Round => {
            // A stadium along the long side (a circle when w == h).
            let (r, half, along_x) = if w >= h {
                (h / 2.0, (w - h) / 2.0, true)
            } else {
                (w / 2.0, (h - w) / 2.0, false)
            };
            let mut pts = Vec::new();
            for (k, sign) in [(0, 1.0), (1, -1.0)] {
                for i in 0..=12 {
                    let t = std::f64::consts::PI * (i as f64 / 12.0 + k as f64)
                        - std::f64::consts::FRAC_PI_2;
                    let (u, v) = (r * t.cos() + sign * half, r * t.sin());
                    pts.push(if along_x { place(u, v) } else { place(-v, u) });
                }
            }
            pts
        }
    }
}

fn pts_block(o: &mut String, pts: &[[f64; 2]], indent: &str) {
    let mut line = String::new();
    for p in pts {
        let _ = write!(line, " (xy {})", xy(p[0], p[1]));
        if line.len() > 100 {
            let _ = writeln!(o, "{indent}{line}");
            line.clear();
        }
    }
    if !line.is_empty() {
        let _ = writeln!(o, "{indent}{line}");
    }
}

/// Start, middle and end of an arc in KiCad text, split in two when it is a
/// full circle (KiCad cannot store one as an arc).
fn arc_points(a: &Arc) -> Vec<(String, String, String)> {
    let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
    if sweep < 1e-6 {
        sweep = 360.0;
    }
    let r = a.radius.mm();
    let (cx, cy) = (a.center.x.mm(), a.center.y.mm());
    let p = |deg: f64| {
        let t = deg.to_radians();
        xy(cx + r * t.cos(), cy + r * t.sin())
    };
    let s = a.start_deg;
    if sweep > 359.999 {
        vec![
            (p(s), p(s + 90.0), p(s + 180.0)),
            (p(s + 180.0), p(s + 270.0), p(s + 360.0)),
        ]
    } else {
        vec![(p(s), p(s + sweep / 2.0), p(s + sweep))]
    }
}

fn area(p: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (u, v) = (p[i], p[(i + 1) % p.len()]);
        a += u[0] * v[1] - v[0] * u[1];
    }
    a / 2.0
}

fn ccw(pts: &[Point]) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = pts.iter().map(|p| [p.x.mm(), p.y.mm()]).collect();
    if v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    if area(&v) < 0.0 {
        v.reverse();
    }
    v
}

/// One outline with its holes joined in through zero-width cuts, the form
/// KiCad keeps zone fills in.
#[doc(hidden)]
pub fn fracture(outline: &[Point], holes: &[Vec<Point>]) -> Vec<[f64; 2]> {
    trace_core::fracture(outline, holes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    fn p(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
        }
    }

    #[test]
    fn fracture_joins_hole() {
        let o = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let h = vec![p(4.0, 4.0), p(6.0, 4.0), p(6.0, 6.0), p(4.0, 6.0)];
        let f = super::fracture(&o, &[h]);
        assert_eq!(f.len(), 4 + 5 + 2);
        assert!((area(&f) - 96.0).abs() < 1e-9);
    }

    #[test]
    fn writes_minimal_board() {
        let mut b = Board::default();
        b.nets.push(Net {
            name: "GND".into(),
            ..Default::default()
        });
        b.geometry.tracks.push(Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: p(0.0, 0.0),
            end: p(1.0, 1.0),
            width: Length::from_mm(0.2),
            text: false,
        });
        b.geometry.pads.push(Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: None,
            name: "1".into(),
            at: p(1.0, 1.0),
            size_x: Length::from_mm(1.0),
            size_y: Length::from_mm(0.5),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 90.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        let s = to_kicad(&b);
        assert!(s.contains("(net 1 \"GND\")"));
        assert!(s.contains("(segment (start 0 0) (end 1 -1) (width 0.2) (layer \"F.Cu\") (net 1)"));
        assert!(s.contains("(pad \"1\" smd rect (at 0 0 90) (size 1 0.5)"));
        assert_eq!(s.matches('(').count(), s.matches(')').count());
    }
}
