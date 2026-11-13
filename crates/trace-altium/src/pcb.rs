use crate::records::{f64_at, i32_at, split_binary, split_pcb, u16_at, Props};
use crate::{Container, Error, Result};
use std::collections::BTreeMap;
use std::path::Path;
use trace_core::*;

/// Altium stores coordinates in units of 1/10,000 mil.
fn internal(v: i32) -> f64 {
    v as f64 / 10_000.0
}

/// Maps Altium layer ids (1 top copper ... 32 bottom copper, 33+ other
/// layers) onto Trace layers, numbering copper from the top as stacked.
pub(crate) struct LayerMap {
    copper: [Option<u8>; 33],
}

impl LayerMap {
    pub(crate) fn new(stackup: &Stackup) -> Self {
        let mut copper = [None; 33];
        let ids: Vec<u8> = stackup
            .copper_layers()
            .filter_map(|l| l.altium_id)
            .collect();
        for (i, id) in ids.iter().enumerate() {
            if let Some(slot) = copper.get_mut(*id as usize) {
                *slot = Some(i as u8);
            }
        }
        LayerMap { copper }
    }

    /// Altium layer byte of copper layer `i` (counted from the top).
    pub(crate) fn id_of(&self, i: u8) -> Option<u8> {
        (1..=32u8).find(|&id| self.copper[id as usize] == Some(i))
    }

    pub(crate) fn get(&self, id: u8) -> LayerRef {
        match id {
            1..=32 => self.copper[id as usize]
                .map(LayerRef::Copper)
                .unwrap_or(LayerRef::Other(id)),
            33 => LayerRef::TopSilk,
            34 => LayerRef::BottomSilk,
            56 => LayerRef::Keepout,
            57..=72 => LayerRef::Mechanical(id - 56),
            74 => LayerRef::AllCopper,
            _ => LayerRef::Other(id),
        }
    }
}

fn net(b: &[u8]) -> Option<u32> {
    u16_at(b, 3).filter(|&n| n != u16::MAX).map(u32::from)
}

fn point(b: &[u8], at: usize) -> Option<Point> {
    Some(Point {
        x: Length::from_mil(internal(i32_at(b, at)?)),
        y: Length::from_mil(internal(i32_at(b, at + 4)?)),
    })
}

fn len_at(b: &[u8], at: usize) -> Option<Length> {
    Some(Length::from_mil(internal(i32_at(b, at)?)))
}

/// Layers whose text is drawn as strokes: copper, silkscreen, paste and
/// solder mask (what goes to manufacture).
pub(crate) fn text_layer(l: &LayerRef) -> bool {
    matches!(
        l,
        LayerRef::Copper(_) | LayerRef::TopSilk | LayerRef::BottomSilk | LayerRef::Other(35..=38)
    )
}

/// A text primitive as Altium keeps it.
pub(crate) struct AltText {
    pub layer: LayerRef,
    pub component: Option<u32>,
    pub at: Point,
    pub height: Length,
    pub width: Length,
    pub rotation_deg: f64,
    pub mirror: bool,
    pub text: String,
    pub truetype: bool,
    pub bold: bool,
    pub designator: bool,
    pub comment: bool,
    /// Bounding box (corners), when the file gives it.
    pub bbox: Option<(Point, Point)>,
}

/// Whether each part shows its designator and its comment.
pub(crate) struct Shown {
    pub name: Vec<bool>,
    pub comment: Vec<bool>,
}

/// The text drawn in the stroke font as tracks, or nothing when it is a
/// hidden designator or comment, or a special string that cannot be
/// filled in.
pub(crate) fn text_tracks(t: &AltText, board: &Board, shown: &Shown) -> Vec<Track> {
    if !text_layer(&t.layer) || t.height.0 <= 0 {
        return Vec::new();
    }
    let part = t.component.map(|c| c as usize);
    let on = |v: &Vec<bool>| part.and_then(|c| v.get(c)).copied().unwrap_or(true);
    if t.designator && !on(&shown.name) || t.comment && !on(&shown.comment) {
        return Vec::new();
    }
    let mut text = t.text.replace("\r\n", "\n");
    if let Some(c) = part.and_then(|c| board.components.get(c)) {
        if text.eq_ignore_ascii_case(".Designator") {
            text = c.designator.clone();
        } else if let Some(v) = text.strip_prefix('.').and_then(|k| c.param(&[k])) {
            // Special strings such as `.Comment` or `.Text1`: the part's
            // parameter of that name.
            text = v.to_string();
        }
    }
    if text.starts_with('.') || text.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let (height, width, at) = if t.truetype {
        // TrueType: the height is the font size; draw caps at about 70 %
        // of it, with a stroke to match the weight.
        let w = Length((t.height.0 as f64 * if t.bold { 0.13 } else { 0.09 }) as i64);
        let h = Length((t.height.0 as f64 * 0.7) as i64);
        let (s, c) = t.rotation_deg.to_radians().sin_cos();
        let up = t.height.0 as f64 * 0.1;
        let ox = if t.mirror { -1.0 } else { 1.0 } * w.0 as f64 / 2.0;
        (
            h,
            w,
            Point {
                x: Length(t.at.x.0 + (ox * c - up * s) as i64),
                y: Length(t.at.y.0 + (ox * s + up * c) as i64),
            },
        )
    } else {
        (t.height, t.width, t.at)
    };
    // The room the text takes in Altium, from its box (in the text's own
    // frame, before turning), so text lands the same length.
    let length = t.bbox.filter(|_| lines.len() == 1).and_then(|(a, b)| {
        let span = (b.x.0 - a.x.0).abs();
        let pad = if t.truetype { width.0 } else { 2 * width.0 };
        (span - pad > 0).then(|| Length(span - pad))
    });
    let (s, c) = t.rotation_deg.to_radians().sin_cos();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // Further lines go down the page.
        let down = i as f64 * height.0 as f64 * 1.6;
        let p = trace_core::font::Placing {
            at: Point {
                x: Length(at.x.0 + (down * s) as i64),
                y: Length(at.y.0 - (down * c) as i64),
            },
            height,
            width,
            rotation_deg: t.rotation_deg,
            mirror: t.mirror,
            length,
        };
        out.extend(trace_core::font::text_tracks(
            line,
            &p,
            t.layer.clone(),
            t.component,
        ));
    }
    out
}

/// Fills: rectangles turned about their centre.
pub(crate) fn fill_outline(a: Point, b: Point, rotation_deg: f64) -> Vec<Point> {
    let (cx, cy) = ((a.x.0 + b.x.0) as f64 / 2.0, (a.y.0 + b.y.0) as f64 / 2.0);
    let (hw, hh) = (
        (b.x.0 - a.x.0).abs() as f64 / 2.0,
        (b.y.0 - a.y.0).abs() as f64 / 2.0,
    );
    let (s, c) = rotation_deg.to_radians().sin_cos();
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)]
        .iter()
        .map(|&(x, y)| Point {
            x: Length((cx + x * c - y * s).round() as i64),
            y: Length((cy + x * s + y * c).round() as i64),
        })
        .collect()
}

/// The board-wide expansion rule of `kind` (`SolderMaskExpansion` or
/// `PasteMaskExpansion`), or Altium's default.
pub(crate) fn rule_expansion(board: &Board, kind: &str) -> Length {
    board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case(kind))
        .filter(|r| {
            r.scope
                .first()
                .is_none_or(|s| s.eq_ignore_ascii_case("All"))
        })
        .min_by_key(|r| r.priority)
        .and_then(|r| r.params.get("EXPANSION"))
        .and_then(|v| Length::parse(v))
        .unwrap_or(if kind.starts_with("Solder") {
            Length::from_mil(4.0)
        } else {
            Length(0)
        })
}

/// The mask and paste openings a copper area on an outer layer makes when
/// it asks for them (Altium regions and fills with an expansion mode other
/// than None), as areas on the mask and paste layers.
pub(crate) fn openings(
    copper: &Polygon,
    ncopper: u8,
    mask: Option<Length>,
    paste: Option<Length>,
) -> Vec<Polygon> {
    let top = match copper.layer {
        LayerRef::Copper(0) => true,
        LayerRef::Copper(l) if l + 1 == ncopper => false,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for (e, id) in [
        (mask, if top { 37 } else { 38 }),
        (paste, if top { 35 } else { 36 }),
    ] {
        let Some(e) = e.filter(|e| !no_paste(*e)) else {
            continue;
        };
        for (outline, holes) in trace_core::geometry::offset_area(&copper.outline, &copper.holes, e)
        {
            out.push(Polygon {
                layer: LayerRef::Other(id),
                net: None,
                outline,
                holes,
                pour: None,
                component: copper.component,
            });
        }
    }
    out
}

/// Mode bytes Altium uses for mask and paste expansion.
const MODE_MANUAL: u8 = 2;

/// Paste expansions this negative mean "no paste" (Altium stores -1 m).
pub(crate) fn no_paste(e: Length) -> bool {
    e.0 <= -Length::from_mil(1000.0).0
}

// Track, arc, via and pad records all start with: layer u8, two flag bytes,
// net index u16 and 8 bytes of links, with geometry from offset 13.

pub(crate) fn parse_track(b: &[u8], layers: &LayerMap) -> Option<Track> {
    Some(Track {
        component: u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from),
        layer: layers.get(*b.first()?),
        net: net(b),
        start: point(b, 13)?,
        end: point(b, 21)?,
        width: len_at(b, 29)?,
        text: false,
    })
}

pub(crate) fn parse_arc(b: &[u8], layers: &LayerMap) -> Option<Arc> {
    let (start_deg, end_deg) = (f64_at(b, 25)?, f64_at(b, 33)?);
    if !start_deg.is_finite() || !end_deg.is_finite() {
        return None;
    }
    Some(Arc {
        component: u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from),
        layer: layers.get(*b.first()?),
        net: net(b),
        center: point(b, 13)?,
        radius: len_at(b, 21)?,
        start_deg,
        end_deg,
        width: len_at(b, 41)?,
    })
}

pub(crate) fn parse_via(b: &[u8], layers: &LayerMap) -> Option<Via> {
    let copper = |id: u8| match layers.get(id) {
        LayerRef::Copper(i) => Some(i),
        _ => None,
    };
    let (from, to) = (copper(*b.get(29)?)?, copper(*b.get(30)?)?);
    let hole = len_at(b, 25)?;
    // Full-stack vias carry a diameter for each layer: slot 0 is the top,
    // 1 to 30 the mid layers and 31 the bottom.
    let mut rings = Vec::new();
    if b.get(74) == Some(&2) && b.len() >= 75 + 32 * 4 {
        for l in from.min(to)..=from.max(to) {
            let Some(id) = layers.id_of(l) else { continue };
            if let Some(d) = len_at(b, 75 + 4 * (id as usize - 1)) {
                rings.push((l, d));
            }
        }
    }
    // "Remove unused pads": one flag per layer id from byte 209 (top first).
    let removed: Vec<u8> = match b.get(209..241) {
        Some(flags) => (1..=32u8)
            .filter(|id| flags[*id as usize - 1] == 1)
            .collect(),
        None => Vec::new(),
    };
    let diameter = len_at(b, 21)?;
    let rings = with_removed_pads(rings, &removed, layers, (from, to), diameter, hole);
    let flags = b.get(1).copied().unwrap_or(0);
    Some(Via {
        net: net(b),
        from_layer: from,
        to_layer: to,
        at: point(b, 13)?,
        diameter,
        hole,
        rings,
        fab: Fab {
            tent_top: flags & 0x20 != 0,
            tent_bottom: flags & 0x40 != 0,
            // Mask expansion of its own: manual flag at 66, value at 54.
            mask: b
                .get(66)
                .filter(|&&m| m & 0x02 != 0)
                .and_then(|_| len_at(b, 54)),
            ..Default::default()
        },
    })
}

/// Marks the layers whose via pad Altium removed (unused inner pads) with a
/// ring the size of the hole, which means no pad there.
pub(crate) fn with_removed_pads(
    rings: Vec<(u8, Length)>,
    removed: &[u8],
    layers: &LayerMap,
    (from, to): (u8, u8),
    diameter: Length,
    hole: Length,
) -> Vec<(u8, Length)> {
    let gone: Vec<u8> = removed
        .iter()
        .filter_map(|&id| match layers.get(id) {
            LayerRef::Copper(l) => Some(l),
            _ => None,
        })
        .collect();
    if gone.is_empty() {
        return rings;
    }
    (from.min(to)..=from.max(to))
        .map(|l| {
            let d = if gone.contains(&l) {
                hole
            } else {
                rings.iter().find(|r| r.0 == l).map_or(diameter, |r| r.1)
            };
            (l, d)
        })
        .collect()
}

/// Copper regions: an 18-byte header, a property block, then the outline
/// and each hole as a vertex count and (x, y) pairs of doubles.
/// Fills: layer, net at 3, part at 7, corners at 13 and 21, turn at 29.
pub(crate) fn parse_fill(b: &[u8], layers: &LayerMap) -> Option<Polygon> {
    let (Some(&id), Some(a), Some(e)) = (b.first(), point(b, 13), point(b, 21)) else {
        return None;
    };
    let layer = layers.get(id);
    if matches!(layer, LayerRef::AllCopper | LayerRef::Keepout) || b.get(1) == Some(&2) {
        return None;
    }
    let rot = f64_at(b, 29).filter(|r| r.is_finite()).unwrap_or(0.0);
    Some(Polygon {
        layer,
        net: net(b),
        outline: fill_outline(a, e, rot),
        holes: Vec::new(),
        pour: None,
        component: u16_at(b, 7).filter(|&i| i != u16::MAX).map(u32::from),
    })
}

pub(crate) fn parse_region(b: &[u8], layers: &LayerMap) -> Option<Polygon> {
    parse_any_region(b, layers)
        .filter(|(_, cutout)| !cutout)
        .map(|(p, _)| p)
}

/// A copper region, or a polygon cutout (`true`): an area pours leave empty.
pub(crate) fn parse_any_region(b: &[u8], layers: &LayerMap) -> Option<(Polygon, bool)> {
    let layer = layers.get(*b.first()?);
    let holes = u16_at(b, 14)? as usize;
    let plen = i32_at(b, 18)? as usize;
    let props = Props::parse(&String::from_utf8_lossy(b.get(22..22 + plen)?));
    let cutout = props.int("KIND") == Some(1);
    if props.bool("ISBOARDCUTOUT")
        || !(cutout || props.int("KIND").unwrap_or(0) == 0)
        || layer == LayerRef::Keepout
        || (layer == LayerRef::AllCopper && !cutout)
    {
        return None;
    }
    let mut i = 22 + plen;
    let mut ring = || -> Option<Vec<Point>> {
        let n = i32_at(b, i)? as usize;
        i += 4;
        let mut pts = Vec::with_capacity(n.min(100_000));
        for _ in 0..n {
            let (x, y) = (f64_at(b, i)?, f64_at(b, i + 8)?);
            i += 16;
            pts.push(Point {
                x: Length::from_mil(x / 10_000.0),
                y: Length::from_mil(y / 10_000.0),
            });
        }
        Some(pts)
    };
    let outline = ring()?;
    let holes = (0..holes).map(|_| ring()).collect::<Option<Vec<_>>>()?;
    let poly = Polygon {
        layer,
        net: if cutout { None } else { net(b) },
        outline,
        holes,
        pour: u16_at(b, 5)
            .filter(|&i| i != u16::MAX && !cutout)
            .map(u32::from),
        component: u16_at(b, 7).filter(|&i| i != u16::MAX).map(u32::from),
    };
    (poly.outline.len() >= 3).then_some((poly, cutout))
}

/// Texts: layer, part at 7, position at 13, height at 21, stroke font at
/// 25, turn at 27, mirror at 35, stroke width at 36; in newer files the
/// comment and designator flags at 40 and 41, the font kind at 43 (1 is
/// TrueType), bold at 44 and the wide-string index at 115. The second
/// block holds the text itself.
pub(crate) fn parse_text(
    r: &[&[u8]],
    layers: &LayerMap,
    wide: &std::collections::HashMap<u32, String>,
) -> Option<AltText> {
    let b = *r.first()?;
    let layer = layers.get(*b.first()?);
    if !text_layer(&layer) {
        return None;
    }
    let long = b.len() >= 123;
    let mut text = r
        .get(1)
        .and_then(|s| {
            let n = *s.first()? as usize;
            Some(String::from_utf8_lossy(s.get(1..1 + n)?).into_owned())
        })
        .unwrap_or_default();
    if long {
        if let Some(w) = u32_at(b, 115).and_then(|i| wide.get(&i)) {
            text = w.clone();
        }
    }
    Some(AltText {
        layer,
        component: u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from),
        at: point(b, 13)?,
        height: len_at(b, 21)?,
        width: len_at(b, 36)?,
        rotation_deg: f64_at(b, 27).filter(|r| r.is_finite()).unwrap_or(0.0),
        mirror: b.get(35).is_some_and(|&m| m != 0),
        text,
        truetype: long && b.get(43) == Some(&1),
        bold: long && b.get(44).is_some_and(|&v| v != 0),
        comment: long && b.get(40).is_some_and(|&v| v != 0),
        designator: long && b.get(41).is_some_and(|&v| v != 0),
        bbox: None,
    })
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Pads have six blocks: name, two unused, a layer-stack string, the main
/// geometry and extended shape data.
pub(crate) fn parse_pad(blocks: &[&[u8]], layers: &LayerMap) -> Option<Pad> {
    let name = blocks
        .first()
        .and_then(|n| n.get(1..))
        .map(|s| s.iter().map(|&c| c as char).collect())
        .unwrap_or_default();
    let b = *blocks.get(4)?;
    let shape = match b.get(49)? {
        2 => PadShape::Rect,
        3 => PadShape::Octagon,
        _ => PadShape::Round,
    };
    let layer = layers.get(*b.first()?);
    let hole = len_at(b, 45)?;
    let shape_of = |v: u8| match v {
        2 => PadShape::Rect,
        3 => PadShape::Octagon,
        _ => PadShape::Round,
    };
    // Full-stack pads: top, mid layer 1 and bottom in the main block, mid
    // layers 2 to 30 in the sixth block. A land no bigger than the hole is
    // no land at all.
    let mut stack = Vec::new();
    if layer == LayerRef::AllCopper && b.get(62) == Some(&2) {
        let rot = f64_at(b, 52).filter(|r| r.is_finite()).unwrap_or(0.0);
        let extra = blocks.get(5).copied().unwrap_or(&[]);
        for i in 0..32u8 {
            let Some(id) = layers.id_of(i) else { continue };
            let (sx, sy, sh) = match id {
                1 => (len_at(b, 21), len_at(b, 25), b.get(49).copied()),
                2 => (len_at(b, 29), len_at(b, 33), b.get(50).copied()),
                32 => (len_at(b, 37), len_at(b, 41), b.get(51).copied()),
                m if extra.len() >= 596 => {
                    let k = m as usize - 3;
                    (
                        len_at(extra, 4 * k),
                        len_at(extra, 116 + 4 * k),
                        extra.get(232 + k).copied(),
                    )
                }
                _ => (len_at(b, 29), len_at(b, 33), b.get(50).copied()),
            };
            let (Some(sx), Some(sy)) = (sx, sy) else {
                continue;
            };
            if sx.0.min(sy.0) <= hole.0 + 100 {
                continue;
            }
            stack.push(Land {
                layer: i,
                shape: shape_of(sh.unwrap_or(1)),
                size_x: sx,
                size_y: sy,
                rotation_deg: rot,
                outline: Vec::new(),
            });
        }
    }
    let at = point(b, 13)?;
    let (size_x, size_y) = (len_at(b, 21)?, len_at(b, 25)?);
    let rotation_deg = f64_at(b, 52).filter(|r| r.is_finite()).unwrap_or(0.0);
    // Rounded rectangles: the extended block's alternate shape 9, with the
    // corner radius as a percentage of half the short side.
    let mut outline = Vec::new();
    let extra = blocks.get(5).copied().unwrap_or(&[]);
    let id = match *b.first()? {
        id @ 1..=32 => id as usize,
        _ => 1,
    };
    if stack.is_empty()
        && shape != PadShape::Octagon
        && extra.len() >= 596
        && extra[532 + id - 1] == 9
    {
        let pct = extra[564 + id - 1].min(100) as f64;
        if pct > 0.0 {
            let r = Length((size_x.0.min(size_y.0) as f64 / 2.0 * pct / 100.0) as i64);
            outline = trace_core::geometry::round_rect(at, size_x, size_y, rotation_deg, r);
        }
    }
    // Mask and paste of its own (modes at 101 and 102, values at 86 and
    // 90), tenting flags, plating at 60, and the hole shape in the
    // extended block: 2 is a slot, its length at 263, its turn at 267.
    let flags = b.get(1).copied().unwrap_or(0);
    let mut fab = Fab {
        tent_top: flags & 0x20 != 0,
        tent_bottom: flags & 0x40 != 0,
        unplated: hole.0 > 0 && b.get(60) == Some(&0),
        ..Default::default()
    };
    if b.get(102) == Some(&MODE_MANUAL) {
        fab.mask = len_at(b, 90);
    }
    if b.get(101) == Some(&MODE_MANUAL) {
        fab.paste = len_at(b, 86);
        fab.no_paste = fab.paste.is_some_and(no_paste);
    }
    if extra.len() >= 275 && extra[262] == 2 {
        if let Some(l) = len_at(extra, 263).filter(|l| l.0 > hole.0) {
            fab.slot = l;
            fab.slot_deg =
                rotation_deg + f64_at(extra, 267).filter(|r| r.is_finite()).unwrap_or(0.0);
        }
    }
    Some(Pad {
        layer,
        net: net(b),
        component: u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from),
        name,
        at,
        size_x,
        size_y,
        hole: len_at(b, 45)?,
        shape,
        rotation_deg,
        outline,
        stack,
        fab,
    })
}

/// The board outline is stored as numbered vertices `VX0`, `VY0`, ...;
/// arcs between vertices are flattened to their end points for now.
/// The board shape, its rounded corners and other arcs included.
pub(crate) fn parse_outline(p: &Props) -> Vec<Point> {
    vertices(p)
}

/// An outline given as `VX0`/`VY0`... vertices, `KIND{i}=1` marking an arc
/// (centre `CX`/`CY`, radius `R`, angles `SA`/`EA`), arcs flattened in 5°
/// steps. Polygons and shape-based regions use it.
pub(crate) fn vertices(p: &Props) -> Vec<Point> {
    let mut outline: Vec<Point> = Vec::new();
    {
        for i in 0.. {
            let (Some(x), Some(y)) = (
                p.get(&format!("VX{i}")).and_then(Length::parse),
                p.get(&format!("VY{i}")).and_then(Length::parse),
            ) else {
                break;
            };
            let r = p
                .get(&format!("R{i}"))
                .and_then(Length::parse)
                .unwrap_or_default();
            if p.int(&format!("KIND{i}")) == Some(1) && r.0 > 0 {
                let c = |k: &str| {
                    p.get(&format!("{k}{i}"))
                        .and_then(Length::parse)
                        .unwrap_or_default()
                };
                let (cx, cy, r) = (c("CX").0 as f64, c("CY").0 as f64, r.0 as f64);
                let (mut a0, mut a1) = (
                    p.float(&format!("SA{i}")).unwrap_or(0.0),
                    p.float(&format!("EA{i}")).unwrap_or(0.0),
                );
                if a1 <= a0 {
                    a1 += 360.0;
                }
                let at = |a: f64| Point {
                    x: Length((cx + r * a.to_radians().cos()).round() as i64),
                    y: Length((cy + r * a.to_radians().sin()).round() as i64),
                };
                // Walk the arc from whichever end the outline is at.
                let from = outline.last().copied().unwrap_or(Point { x, y });
                let d = |q: Point| ((q.x.0 - from.x.0) as f64).hypot((q.y.0 - from.y.0) as f64);
                if d(at(a1)) < d(at(a0)) {
                    std::mem::swap(&mut a0, &mut a1);
                }
                let n = (((a1 - a0).abs() / 5.0).ceil() as usize).max(2);
                for k in 0..=n {
                    outline.push(at(a0 + (a1 - a0) * k as f64 / n as f64));
                }
            } else {
                outline.push(Point { x, y });
            }
        }
        if outline.len() > 1 && outline.first() == outline.last() {
            outline.pop();
        }
    }
    outline
}

/// A polygon pour as drawn (`Polygons6` / ASCII `Polygon`): its outline,
/// with arc vertices flattened, net, layer and fill order. Pours Trace
/// cannot fill again (hatched, shelved, not on copper) get no outline.
pub(crate) fn parse_pour(p: &Props, layers: &LayerMap) -> Pour {
    let layer = layers.get(crate::pcb_ascii::layer_id(&p.str("LAYER")));
    let digits: String = p
        .str("NET")
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let net = digits.parse::<u32>().ok();
    let solid = p
        .get("HATCHSTYLE")
        .is_none_or(|h| h.eq_ignore_ascii_case("Solid"));
    let poured = p
        .get("POLYGONTYPE")
        .is_none_or(|t| t.eq_ignore_ascii_case("Polygon"));
    let outline = if solid && poured && !p.bool("SHELVED") && matches!(layer, LayerRef::Copper(_)) {
        vertices(p)
    } else {
        Vec::new()
    };
    Pour {
        layer,
        net,
        outline,
        remove_dead: p
            .get("REMOVEDEAD")
            .is_none_or(|v| v.eq_ignore_ascii_case("TRUE")),
        order: p.int("POURINDEX").unwrap_or(0) as i32,
        // AREATHRESHOLD is in square 1/10000 mil.
        min_island: if p.bool("REMOVEISLANDSBYAREA") {
            p.get("AREATHRESHOLD")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map_or(0.0, |a| a / 1e8)
        } else {
            0.0
        },
    }
}

/// Reads a `.PcbDoc`: stack-up, nets with routed lengths, classes, differential
/// pairs, components and design rules.
/// Also reads boards saved as ASCII (`File > Save As`, "PCB ASCII File").
pub fn read_pcbdoc(path: impl AsRef<Path>) -> Result<Board> {
    if crate::pcb_ascii::is_ascii(path.as_ref()) {
        return crate::pcb_ascii::read(path.as_ref());
    }
    let mut c = Container::open(path.as_ref())?;
    let board_data = c
        .stream("/Board6/Data")?
        .ok_or_else(|| Error::MissingStream("Board6/Data".into()))?;

    let mut board = Board::default();
    let board_props = split_pcb(&board_data, 0)
        .into_iter()
        .next()
        .unwrap_or_default();
    board.stackup = parse_stackup(&board_props);

    let text = |c: &mut Container<_>, name: &str, prefix| -> Result<Vec<Props>> {
        Ok(c.stream(name)?
            .map(|d| split_pcb(&d, prefix))
            .unwrap_or_default())
    };

    board.nets = text(&mut c, "/Nets6/Data", 0)?
        .iter()
        .map(|p| Net {
            name: p.str("NAME"),
            ..Default::default()
        })
        .collect();

    for p in text(&mut c, "/Classes6/Data", 0)? {
        let members: Vec<String> = (0..)
            .map_while(|i| p.get(&format!("M{i}")).map(str::to_string))
            .collect();
        let class = NetClass {
            name: p.str("NAME"),
            members,
        };
        match p.int("KIND") {
            Some(0) => board.net_classes.push(class),
            Some(1) => board.component_classes.push(class),
            Some(6) => board.diff_pair_classes.push(class),
            _ => {}
        }
    }

    board.diff_pairs = text(&mut c, "/DifferentialPairs6/Data", 0)?
        .iter()
        .map(|p| DiffPair {
            name: p.str("NAME"),
            positive: p.str("POSITIVENETNAME"),
            negative: p.str("NEGATIVENETNAME"),
        })
        .collect();

    board.components = text(&mut c, "/Components6/Data", 0)?
        .iter()
        .map(|p| Component {
            designator: p.str("SOURCEDESIGNATOR"),
            footprint: p.str("PATTERN"),
            library_ref: p.get("SOURCELIBREFERENCE").map(str::to_string),
            x: p.get("X").and_then(Length::parse).unwrap_or_default(),
            y: p.get("Y").and_then(Length::parse).unwrap_or_default(),
            rotation_deg: p.float("ROTATION").unwrap_or(0.0),
            side: if p.str("LAYER").eq_ignore_ascii_case("BOTTOM") {
                Side::Bottom
            } else {
                Side::Top
            },
            sheet: p.get("SOURCEHIERARCHICALPATH").map(str::to_string),
            params: Default::default(),
        })
        .collect();

    board.rules = text(&mut c, "/Rules6/Data", 2)?
        .iter()
        .map(parse_rule)
        .collect();

    let layers = LayerMap::new(&board.stackup);
    let g = &mut board.geometry;
    if let Some(d) = c.stream("/Tracks6/Data")? {
        g.tracks = split_binary(&d, 1)
            .iter()
            .filter_map(|r| parse_track(r[0], &layers))
            .collect();
    }
    if let Some(d) = c.stream("/Arcs6/Data")? {
        g.arcs = split_binary(&d, 1)
            .iter()
            .filter_map(|r| parse_arc(r[0], &layers))
            .collect();
    }
    if let Some(d) = c.stream("/Vias6/Data")? {
        g.vias = split_binary(&d, 1)
            .iter()
            .filter_map(|r| parse_via(r[0], &layers))
            .collect();
    }
    if let Some(d) = c.stream("/Pads6/Data")? {
        g.pads = split_binary(&d, 6)
            .iter()
            .filter_map(|r| parse_pad(r, &layers))
            .collect();
    }
    if let Some(d) = c.stream("/Regions6/Data")? {
        for (p, cutout) in split_binary(&d, 1)
            .iter()
            .filter_map(|r| parse_any_region(r[0], &layers))
        {
            if cutout {
                if matches!(p.layer, LayerRef::Copper(_) | LayerRef::AllCopper) {
                    g.cutouts.push(p);
                }
            } else if matches!(p.layer, LayerRef::Copper(_)) {
                g.polygons.push(p);
            } else {
                g.areas.push(p);
            }
        }
    }
    if let Some(d) = c.stream("/Fills6/Data")? {
        for r in split_binary(&d, 1) {
            let Some(poly) = parse_fill(r[0], &layers) else {
                continue;
            };
            if matches!(poly.layer, LayerRef::Copper(_)) {
                g.polygons.push(poly);
            } else {
                g.areas.push(poly);
            }
        }
    }
    read_models(&mut c, g)?;
    g.outline = parse_outline(&board_props);
    board.stats.tracks = g.tracks.len();
    board.stats.vias = g.vias.len();
    board.stats.pads = g.pads.len();

    // Text, in the stroke font, on the layers that go to manufacture.
    let comps = text(&mut c, "/Components6/Data", 0)?;
    let shown = Shown {
        name: comps
            .iter()
            .map(|p| {
                p.get("NAMEON")
                    .is_none_or(|v| v.eq_ignore_ascii_case("TRUE"))
            })
            .collect(),
        comment: comps.iter().map(|p| p.bool("COMMENTON")).collect(),
    };
    // Wide strings: index u32, byte length u32, UTF-16 text with a
    // closing zero.
    let mut wide: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    if let Some(d) = c.stream("/WideStrings6/Data")? {
        let mut i = 0;
        while i + 8 <= d.len() {
            let (Some(k), Some(n)) = (u32_at(&d, i), u32_at(&d, i + 4)) else {
                break;
            };
            let Some(bytes) = d.get(i + 8..i + 8 + n as usize) else {
                break;
            };
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            wide.insert(k, String::from_utf16_lossy(&units));
            i += 8 + n as usize;
        }
    }
    // Part parameters: a record naming the part (`ID=Component#N`, no
    // variant) and then its parameters, one record each.
    if let Some(d) = c.stream("/PrimitiveParameters/Data")? {
        let mut owner: Option<usize> = None;
        for p in split_pcb(&d, 0) {
            if let Some(id) = p.get("ID") {
                owner = id
                    .strip_prefix("Component#")
                    .and_then(|n| n.parse().ok())
                    .filter(|_| p.str("VARIANTGUID").is_empty() && p.get("APPURTENANCE").is_none());
                continue;
            }
            let Some(c) = owner.and_then(|i| board.components.get_mut(i)) else {
                continue;
            };
            let name = p.str("NAME");
            if !name.is_empty() {
                c.params.insert(name, p.str("VALUE"));
            }
        }
    }
    for (c, p) in board.components.iter_mut().zip(&comps) {
        if let Some(d) = p.get("SOURCEDESCRIPTION").filter(|d| !d.is_empty()) {
            c.params
                .entry("Description".into())
                .or_insert_with(|| d.to_string());
        }
    }
    if let Some(d) = c.stream("/Texts6/Data")? {
        for r in split_binary(&d, 2) {
            if let Some(t) = parse_text(&r, &layers, &wide) {
                let tracks = text_tracks(&t, &board, &shown);
                board.geometry.tracks.extend(tracks);
            }
        }
    }

    let polygons = text(&mut c, "/Polygons6/Data", 0)?;
    board.stats.polygons = polygons.len();
    board.geometry.pours = polygons.iter().map(|p| parse_pour(p, &layers)).collect();
    for p in &polygons {
        let digits: String = p
            .str("NET")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Some(n) = digits
            .parse::<usize>()
            .ok()
            .and_then(|n| board.nets.get_mut(n))
        {
            n.has_pour = true;
        }
    }
    // Poured copper can carry no net of its own: it takes its polygon's.
    let g = &mut board.geometry;
    for poly in &mut g.polygons {
        if poly.net.is_none() {
            poly.net = poly
                .pour
                .and_then(|i| g.pours.get(i as usize))
                .and_then(|p| p.net);
        }
    }
    // Vias the board's back drilling rule asks to backdrill, before
    // pouring so pours keep clear of them.
    trace_core::backdrill::apply_rules(&mut board);
    // Pours saved without their copper are poured here.
    trace_core::pour::pour_missing(&mut board);
    trace_core::pour::mark_unpoured(&mut board);
    trace_core::connectivity::analyse(&mut board);

    Ok(board)
}

/// Embedded 3D models (`Models/N`, zlib-compressed STEP) and where each
/// part's model sits (`ComponentBodies6`).
fn read_models<R: std::io::Read + std::io::Seek>(
    c: &mut Container<R>,
    g: &mut trace_core::Geometry,
) -> Result<()> {
    use std::io::Read;
    let index = c
        .stream("/Models/Data")?
        .map(|d| split_pcb(&d, 0))
        .unwrap_or_default();
    let mut by_id = std::collections::HashMap::new();
    for (i, p) in index.iter().enumerate() {
        let Some(raw) = c.stream(&format!("/Models/{i}"))? else {
            continue;
        };
        let mut step = Vec::new();
        if flate2::read::ZlibDecoder::new(&raw[..])
            .read_to_end(&mut step)
            .is_err()
        {
            step = raw;
        }
        by_id.insert(p.str("ID"), g.models.len() as u32);
        g.models.push(trace_core::Model3d {
            name: p.str("NAME"),
            step: std::sync::Arc::new(step),
        });
    }
    let Some(d) = c.stream("/ComponentBodies6/Data")? else {
        return Ok(());
    };
    for r in split_binary(&d, 1) {
        let b = r[0];
        let Some(len) = i32_at(b, 18) else { continue };
        let Some(text) = b.get(22..22 + len.max(0) as usize) else {
            continue;
        };
        let p = Props::parse(&String::from_utf8_lossy(text));
        if !p.bool("MODEL.EMBED") {
            continue;
        }
        let Some(&model) = by_id.get(&p.str("MODELID")) else {
            continue;
        };
        let component = u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from);
        let len = |k: &str| p.get(k).and_then(Length::parse).unwrap_or_default();
        g.bodies.push(trace_core::Body3d {
            component,
            model,
            at: Point {
                x: len("MODEL.2D.X"),
                y: len("MODEL.2D.Y"),
            },
            rotation_deg: p.float("MODEL.2D.ROTATION").unwrap_or(0.0),
            model_rotation_deg: [
                p.float("MODEL.3D.ROTX").unwrap_or(0.0),
                p.float("MODEL.3D.ROTY").unwrap_or(0.0),
                p.float("MODEL.3D.ROTZ").unwrap_or(0.0),
            ],
            standoff: len("MODEL.3D.DZ"),
            bottom: p.int("BODYPROJECTION") == Some(1),
            opacity: p.float("BODYOPACITY3D").unwrap_or(1.0) as f32,
        });
    }
    Ok(())
}

pub(crate) fn parse_stackup(p: &Props) -> Stackup {
    let mut layers = Vec::new();
    for i in 0.. {
        let key = |f: &str| format!("V9_STACK_LAYER{i}_{f}");
        let Some(name) = p.get(&key("NAME")) else {
            break;
        };
        let id = p.int(&key("LAYERID")).unwrap_or(0);
        let material = p.get(&key("DIELMATERIAL")).map(str::to_string);
        let lname = name.to_ascii_lowercase();
        let kind = match id >> 16 {
            0x0100 => LayerKind::Copper,
            0x0104 => LayerKind::Dielectric,
            _ if lname.contains("mask") => LayerKind::SolderMask,
            _ if lname.contains("silk") || lname.contains("overlay") => LayerKind::Silkscreen,
            _ if lname.contains("paste") => LayerKind::Paste,
            _ => LayerKind::Other,
        };
        let thickness = match kind {
            LayerKind::Copper => p.get(&key("COPTHICK")),
            _ => p.get(&key("DIELHEIGHT")),
        }
        .and_then(Length::parse)
        .unwrap_or_default();
        layers.push(StackLayer {
            // Bottom copper is stored as 0xFFFF; primitives call it layer 32.
            altium_id: (kind == LayerKind::Copper).then_some(if id & 0xFFFF == 0xFFFF {
                32
            } else {
                (id & 0xFF) as u8
            }),
            name: name.to_string(),
            kind,
            thickness,
            dielectric_constant: p.float(&key("DIELCONST")),
            material,
        });
    }
    Stackup { layers }
}

/// Keys every primitive record carries; they say nothing about the rule.
const BOILERPLATE: &[&str] = &[
    "SELECTION",
    "LAYER",
    "LOCKED",
    "POLYGONOUTLINE",
    "USERROUTED",
    "KEEPOUT",
    "UNIONINDEX",
    "RULEKIND",
    "NAME",
    "ENABLED",
    "PRIORITY",
    "UNIQUEID",
    "COMMENT",
    "SCOPE1EXPRESSION",
    "SCOPE2EXPRESSION",
    "DEFINEDBYLOGICALDOCUMENT",
];

pub(crate) fn parse_rule(p: &Props) -> Rule {
    let params: BTreeMap<String, String> = p
        .keys()
        .filter(|(k, _)| !BOILERPLATE.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let scope = ["SCOPE1EXPRESSION", "SCOPE2EXPRESSION"]
        .iter()
        .filter_map(|k| p.get(k))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Rule {
        kind: p.str("RULEKIND"),
        name: p.str("NAME"),
        enabled: p.bool("ENABLED"),
        priority: p.int("PRIORITY").unwrap_or(0),
        scope,
        params,
    }
}
