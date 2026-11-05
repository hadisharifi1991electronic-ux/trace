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

// Track, arc, via and pad records all start with: layer u8, two flag bytes,
// net index u16 and 8 bytes of links, with geometry from offset 13.

fn parse_track(b: &[u8], layers: &LayerMap) -> Option<Track> {
    Some(Track {
        layer: layers.get(*b.first()?),
        net: net(b),
        start: point(b, 13)?,
        end: point(b, 21)?,
        width: len_at(b, 29)?,
    })
}

fn parse_arc(b: &[u8], layers: &LayerMap) -> Option<Arc> {
    let (start_deg, end_deg) = (f64_at(b, 25)?, f64_at(b, 33)?);
    if !start_deg.is_finite() || !end_deg.is_finite() {
        return None;
    }
    Some(Arc {
        layer: layers.get(*b.first()?),
        net: net(b),
        center: point(b, 13)?,
        radius: len_at(b, 21)?,
        start_deg,
        end_deg,
        width: len_at(b, 41)?,
    })
}

fn parse_via(b: &[u8], layers: &LayerMap) -> Option<Via> {
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
    Some(Via {
        net: net(b),
        from_layer: from,
        to_layer: to,
        at: point(b, 13)?,
        diameter: len_at(b, 21)?,
        hole,
        rings,
    })
}

/// Copper regions: an 18-byte header, a property block, then the outline
/// and each hole as a vertex count and (x, y) pairs of doubles.
fn parse_region(b: &[u8], layers: &LayerMap) -> Option<Polygon> {
    let layer = layers.get(*b.first()?);
    if !matches!(layer, LayerRef::Copper(_)) {
        return None;
    }
    let holes = u16_at(b, 14)? as usize;
    let plen = i32_at(b, 18)? as usize;
    let props = Props::parse(&String::from_utf8_lossy(b.get(22..22 + plen)?));
    if props.int("KIND").unwrap_or(0) != 0 || props.bool("ISBOARDCUTOUT") {
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
    (outline.len() >= 3).then_some(Polygon {
        layer,
        net: net(b),
        outline,
        holes,
    })
}

/// Pads have six blocks: name, two unused, a layer-stack string, the main
/// geometry and extended shape data.
fn parse_pad(blocks: &[&[u8]], layers: &LayerMap) -> Option<Pad> {
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
    Some(Pad {
        layer,
        net: net(b),
        component: u16_at(b, 7).filter(|&c| c != u16::MAX).map(u32::from),
        name,
        at: point(b, 13)?,
        size_x: len_at(b, 21)?,
        size_y: len_at(b, 25)?,
        hole: len_at(b, 45)?,
        shape,
        rotation_deg: f64_at(b, 52).filter(|r| r.is_finite()).unwrap_or(0.0),
        outline: Vec::new(),
        stack,
    })
}

/// The board outline is stored as numbered vertices `VX0`, `VY0`, ...;
/// arcs between vertices are flattened to their end points for now.
pub(crate) fn parse_outline(p: &Props) -> Vec<Point> {
    (0..)
        .map_while(|i| {
            let x = p.get(&format!("VX{i}")).and_then(Length::parse)?;
            let y = p.get(&format!("VY{i}")).and_then(Length::parse)?;
            Some(Point { x, y })
        })
        .collect()
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
        g.polygons = split_binary(&d, 1)
            .iter()
            .filter_map(|r| parse_region(r[0], &layers))
            .collect();
    }
    read_models(&mut c, g)?;
    g.outline = parse_outline(&board_props);
    board.stats.tracks = g.tracks.len();
    board.stats.vias = g.vias.len();
    board.stats.pads = g.pads.len();

    trace_core::connectivity::analyse(&mut board);
    let polygons = text(&mut c, "/Polygons6/Data", 0)?;
    board.stats.polygons = polygons.len();
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
