//! Altium boards saved as ASCII ("PCB ASCII File"): one `|RECORD=Kind|KEY=VALUE...`
//! line per object, the same properties the binary format keeps in its text
//! streams, with primitives spelled out as text too.

use crate::pcb::{parse_outline, parse_pour, parse_rule, parse_stackup, LayerMap};
use crate::records::Props;
use crate::{Error, Result};
use std::io::Read;
use std::path::Path;
use trace_core::*;

/// True when the file starts like an ASCII board rather than an OLE file.
pub(crate) fn is_ascii(path: &Path) -> bool {
    let mut head = [0u8; 64];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    let text: Vec<u8> = head[..n]
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace() || *b == 0xEF || *b == 0xBB || *b == 0xBF)
        .collect();
    text.starts_with(b"|RECORD=") || text.starts_with(b"RECORD=")
}

/// Altium layer id from the layer names ASCII files use.
/// Layer ids from a `REMOVEDPADS` layer hash such as
/// `SerializeLayerHash.Version=2,ClassName=...,16777218=1,16777219=1,`:
/// keys are V7 layer ids (0x01000001 top, 0x0100FFFF bottom).
pub(crate) fn removed_pads(hash: &str) -> Vec<u8> {
    hash.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if v.trim() != "1" {
                return None;
            }
            match k.trim().parse::<u32>().ok()? {
                0x0100_FFFF => Some(32),
                k if (0x0100_0001..=0x0100_001F).contains(&k) => Some((k - 0x0100_0000) as u8),
                _ => None,
            }
        })
        .collect()
}

pub(crate) fn layer_id(name: &str) -> u8 {
    let n = name.trim().to_ascii_uppercase();
    let num = |prefix: &str| n.strip_prefix(prefix).and_then(|d| d.parse::<u8>().ok());
    match n.as_str() {
        "TOP" => 1,
        "BOTTOM" => 32,
        "TOPOVERLAY" => 33,
        "BOTTOMOVERLAY" => 34,
        "TOPPASTE" => 35,
        "BOTTOMPASTE" => 36,
        "TOPSOLDER" => 37,
        "BOTTOMSOLDER" => 38,
        "DRILLGUIDE" => 55,
        "KEEPOUT" => 56,
        "DRILLDRAWING" => 73,
        "MULTILAYER" => 74,
        _ => {
            if let Some(m) = num("MID").filter(|m| (1..=30).contains(m)) {
                1 + m
            } else if let Some(p) = num("PLANE").filter(|p| (1..=16).contains(p)) {
                38 + p
            } else if let Some(m) = num("MECHANICAL").filter(|m| (1..=16).contains(m)) {
                56 + m
            } else {
                0
            }
        }
    }
}

fn len(p: &Props, key: &str) -> Option<Length> {
    p.get(key).and_then(Length::parse)
}

fn pt(p: &Props, x: &str, y: &str) -> Option<Point> {
    Some(Point {
        x: len(p, x)?,
        y: len(p, y)?,
    })
}

fn index(p: &Props, key: &str) -> Option<u32> {
    p.int(key).filter(|&n| n >= 0).map(|n| n as u32)
}

/// The mask and paste expansion an area asks for: its own, the rule's, or
/// none (no opening).
fn expansions(p: &Props, rules: &(Length, Length)) -> (Option<Length>, Option<Length>) {
    let pick =
        |mode: &str, manual: &str, rule: Length| match p.str(mode).to_ascii_uppercase().as_str() {
            "MANUAL" => Some(len(p, manual).unwrap_or_default()),
            "RULE" => Some(rule),
            _ => None,
        };
    (
        pick(
            "SOLDERMASKEXPANSIONMODE",
            "SOLDERMASKEXPANSION_MANUAL",
            rules.0,
        ),
        pick(
            "PASTEMASKEXPANSIONMODE",
            "PASTEMASKEXPANSION_MANUAL",
            rules.1,
        ),
    )
}

/// Mask, paste, tenting, plating and slot details a pad or via sets itself.
fn fab_of(p: &Props, hole: Length, rotation_deg: f64) -> Fab {
    let yes = |k: &str| p.bool(k);
    let manual = |k: &str| p.str(k).eq_ignore_ascii_case("MANUAL");
    let mut f = Fab {
        tent_top: yes("TENTINGTOP") || yes("TENTING"),
        tent_bottom: yes("TENTINGBOTTOM") || yes("TENTING"),
        unplated: hole.0 > 0
            && p.get("PLATED")
                .is_some_and(|v| v.eq_ignore_ascii_case("FALSE")),
        ..Default::default()
    };
    // Pads keep their own expansion in CSE and CPE (with CSEV and CPEV 2
    // for "manual"); the *_MANUAL fields are only right for areas.
    let own = |mode: &str, valid: &str, value: &str, old: &str| {
        if p.int(valid) == Some(2) {
            Some(len(p, value).unwrap_or_default())
        } else if manual(mode) {
            Some(len(p, old).unwrap_or_default())
        } else {
            None
        }
    };
    if let Some(e) = own(
        "SOLDERMASKEXPANSIONMODE",
        "CSEV",
        "CSE",
        "SOLDERMASKEXPANSION_MANUAL",
    ) {
        // Altium covers a pad by shrinking its opening by a metre.
        if crate::pcb::no_paste(e) {
            f.tent_top = true;
            f.tent_bottom = true;
        } else {
            f.mask = Some(e);
        }
    }
    if let Some(e) = own(
        "PASTEMASKEXPANSIONMODE",
        "CPEV",
        "CPE",
        "PASTEMASKEXPANSION_MANUAL",
    ) {
        f.paste = Some(e);
        f.no_paste = crate::pcb::no_paste(e);
    }
    if p.int("HOLETYPE") == Some(2) {
        if let Some(l) = len(p, "HOLEWIDTH").filter(|l| l.0 > hole.0) {
            f.slot = l;
            f.slot_deg = rotation_deg + angle(p, "HOLEROTATION");
        }
    }
    f
}

fn angle(p: &Props, key: &str) -> f64 {
    p.float(key).filter(|a| a.is_finite()).unwrap_or(0.0)
}

fn shape(p: &Props) -> PadShape {
    match p.str("SHAPE").to_ascii_uppercase().as_str() {
        "RECTANGLE" | "ROUNDEDRECTANGLE" => PadShape::Rect,
        "OCTAGONAL" => PadShape::Octagon,
        _ => PadShape::Round,
    }
}

/// A contour in internal units: `{x}{i}` / `{y}{i}` for `i` below `count`.
fn contour(p: &Props, x: &str, y: &str, count: usize) -> Vec<Point> {
    (0..count)
        .map_while(|i| {
            let fx = p.float(&format!("{x}{i}"))?;
            let fy = p.float(&format!("{y}{i}"))?;
            Some(Point {
                x: Length::from_mil(fx / 10_000.0),
                y: Length::from_mil(fy / 10_000.0),
            })
        })
        .collect()
}

/// Reads an ASCII `.PcbDoc`.
pub(crate) fn read(path: &Path) -> Result<Board> {
    let bytes = std::fs::read(path).map_err(|source| Error::Open {
        path: path.display().to_string(),
        source,
    })?;
    // Windows-1252 text; bytes map to chars directly for names that matter.
    let text: String = bytes.iter().map(|&b| b as char).collect();
    // A line not starting a record continues the one before (multi-line
    // text). The board's own settings come as many `Board` lines.
    let mut lines: Vec<(String, String)> = Vec::new();
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        let body = line.trim_start_matches('|');
        match body.strip_prefix("RECORD=") {
            Some(rest) => {
                let kind = rest.split('|').next().unwrap_or_default().to_string();
                match lines.last_mut() {
                    Some((k, text)) if kind == "Board" && k == "Board" => {
                        text.push('|');
                        text.push_str(body);
                    }
                    _ => lines.push((kind, body.to_string())),
                }
            }
            None => {
                if let Some((_, text)) = lines.last_mut() {
                    text.push(' ');
                    text.push_str(line);
                }
            }
        }
    }
    let records: Vec<(String, Props)> = lines
        .into_iter()
        .map(|(kind, text)| (kind, Props::parse(&text)))
        .collect();
    fn records_of<'a>(
        records: &'a [(String, Props)],
        kind: &'a str,
    ) -> impl Iterator<Item = &'a Props> + 'a {
        records
            .iter()
            .filter(move |(k, _)| k == kind)
            .map(|(_, p)| p)
    }
    let of = |kind: &'static str| records_of(&records, kind);

    let mut board = Board::default();
    let board_props = of("Board").next().cloned().unwrap_or_default();
    board.stackup = parse_stackup(&board_props);
    board.nets = of("Net")
        .map(|p| Net {
            name: p.str("NAME"),
            ..Default::default()
        })
        .collect();
    for p in of("Class") {
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
    board.diff_pairs = of("DifferentialPair")
        .map(|p| DiffPair {
            name: p.str("NAME"),
            positive: p.str("POSITIVENETNAME"),
            negative: p.str("NEGATIVENETNAME"),
        })
        .collect();
    board.components = of("Component")
        .map(|p| Component {
            designator: p.str("SOURCEDESIGNATOR"),
            footprint: p.str("PATTERN"),
            library_ref: p.get("SOURCELIBREFERENCE").map(str::to_string),
            x: len(p, "X").unwrap_or_default(),
            y: len(p, "Y").unwrap_or_default(),
            rotation_deg: angle(p, "ROTATION"),
            side: if p.str("LAYER").eq_ignore_ascii_case("BOTTOM") {
                Side::Bottom
            } else {
                Side::Top
            },
            sheet: p.get("SOURCEHIERARCHICALPATH").map(str::to_string),
            params: Default::default(),
        })
        .collect();
    // Part parameters (value, part number, maker...) and description.
    for p in of("ParamItem") {
        let Some(i) = p
            .str("PRIMITIVEINDEX")
            .strip_prefix("Component#")
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        if let Some(c) = board.components.get_mut(i) {
            let name = p.str("NAME");
            if !name.is_empty() {
                c.params.insert(name, p.str("VALUE"));
            }
        }
    }
    for (c, p) in board.components.iter_mut().zip(of("Component")) {
        if let Some(d) = p.get("SOURCEDESCRIPTION").filter(|d| !d.is_empty()) {
            c.params
                .entry("Description".into())
                .or_insert_with(|| d.to_string());
        }
    }
    // `DXPRule` records hold the rules as Altium uses them; `Rule` records
    // are an older form, kept for files without the newer ones.
    board.rules = of("DXPRule").map(parse_rule).collect();
    if board.rules.is_empty() {
        board.rules = of("Rule").map(parse_rule).collect();
    }

    let layers = LayerMap::new(&board.stackup);
    let layer = |p: &Props| layers.get(layer_id(&p.str("LAYER")));
    let copper = |name: &str| match layers.get(layer_id(name)) {
        LayerRef::Copper(i) => Some(i),
        _ => None,
    };
    let ncopper = board.stackup.copper_layers().count() as u8;
    let rules = (
        crate::pcb::rule_expansion(&board, "SolderMaskExpansion"),
        crate::pcb::rule_expansion(&board, "PasteMaskExpansion"),
    );
    let g = &mut board.geometry;

    g.tracks = of("Track")
        .filter_map(|p| {
            Some(Track {
                component: index(p, "COMPONENT"),
                layer: layer(p),
                net: index(p, "NET"),
                start: pt(p, "X1", "Y1")?,
                end: pt(p, "X2", "Y2")?,
                width: len(p, "WIDTH")?,
            })
        })
        .collect();
    g.arcs = of("Arc")
        .filter_map(|p| {
            Some(Arc {
                component: index(p, "COMPONENT"),
                layer: layer(p),
                net: index(p, "NET"),
                center: pt(p, "LOCATION.X", "LOCATION.Y")?,
                radius: len(p, "RADIUS")?,
                start_deg: p.float("STARTANGLE").filter(|a| a.is_finite())?,
                end_deg: p.float("ENDANGLE").filter(|a| a.is_finite())?,
                width: len(p, "WIDTH")?,
            })
        })
        .collect();
    g.vias = of("Via")
        .filter_map(|p| {
            let (from_layer, to_layer) =
                (copper(&p.str("STARTLAYER"))?, copper(&p.str("ENDLAYER"))?);
            let diameter = len(p, "DIAMETER")?;
            let hole = len(p, "HOLESIZE").unwrap_or_default();
            let rings = crate::pcb::with_removed_pads(
                Vec::new(),
                &removed_pads(&p.str("REMOVEDPADS")),
                &layers,
                (from_layer, to_layer),
                diameter,
                hole,
            );
            Some(Via {
                net: index(p, "NET"),
                from_layer,
                to_layer,
                at: pt(p, "X", "Y")?,
                diameter,
                hole,
                rings,
                fab: Fab {
                    unplated: false,
                    slot: Length(0),
                    ..fab_of(p, hole, 0.0)
                },
            })
        })
        .collect();
    g.pads = of("Pad")
        .filter_map(|p| {
            let at = pt(p, "X", "Y")?;
            let hole = len(p, "HOLESIZE").unwrap_or_default();
            let layer = layer(p);
            let rotation_deg = angle(p, "ROTATION");
            let shape = shape(p);
            // Top-middle-bottom pads give the land of each copper layer.
            let mut stack = Vec::new();
            if layer == LayerRef::AllCopper && p.int("PADMODE") == Some(1) {
                for i in 0..ncopper {
                    let side = if i == 0 {
                        "TOP"
                    } else if i + 1 == ncopper {
                        "BOT"
                    } else {
                        "MID"
                    };
                    let (Some(sx), Some(sy)) = (
                        len(p, &format!("{side}XSIZE")),
                        len(p, &format!("{side}YSIZE")),
                    ) else {
                        continue;
                    };
                    if sx.0.min(sy.0) <= hole.0 + 100 {
                        continue;
                    }
                    stack.push(Land {
                        layer: i,
                        shape,
                        size_x: sx,
                        size_y: sy,
                        rotation_deg,
                        outline: Vec::new(),
                    });
                }
            }
            let (size_x, size_y) = match (len(p, "XSIZE"), len(p, "YSIZE")) {
                (Some(x), Some(y)) => (x, y),
                _ => (len(p, "TOPXSIZE")?, len(p, "TOPYSIZE")?),
            };
            // Unused inner lands Altium removed: a stack without them.
            let gone: Vec<u8> = removed_pads(&p.str("REMOVEDPADS"))
                .into_iter()
                .filter_map(|id| match layers.get(id) {
                    LayerRef::Copper(l) => Some(l),
                    _ => None,
                })
                .collect();
            if layer == LayerRef::AllCopper && !gone.is_empty() {
                if stack.is_empty() {
                    stack = (0..ncopper)
                        .map(|i| Land {
                            layer: i,
                            shape,
                            size_x,
                            size_y,
                            rotation_deg,
                            outline: Vec::new(),
                        })
                        .collect();
                }
                stack.retain(|l| !gone.contains(&l.layer));
            }
            // Rounded corners: radius as a percentage of half the short side.
            let mut outline = Vec::new();
            if stack.is_empty() && p.str("SHAPE").eq_ignore_ascii_case("ROUNDEDRECTANGLE") {
                let key = if layer == LayerRef::Copper(ncopper.saturating_sub(1)) && ncopper > 1 {
                    "BOTTOMLAYERCRPCT"
                } else {
                    "TOPLAYERCRPCT"
                };
                let pct = p.float(key).unwrap_or(0.0).clamp(0.0, 100.0);
                if pct > 0.0 {
                    let r = Length((size_x.0.min(size_y.0) as f64 / 2.0 * pct / 100.0) as i64);
                    outline = trace_core::geometry::round_rect(at, size_x, size_y, rotation_deg, r);
                }
            }
            Some(Pad {
                layer,
                net: index(p, "NET"),
                component: index(p, "COMPONENT"),
                name: p.str("NAME"),
                at,
                size_x,
                size_y,
                hole,
                shape,
                rotation_deg,
                outline,
                stack,
                fab: fab_of(p, hole, rotation_deg),
            })
        })
        .collect();

    // Pour fills carry their polygon's net rather than their own; custom
    // pad shapes carry their pad's.
    let pour_nets: Vec<Option<u32>> = of("Polygon").map(|p| index(p, "NET")).collect();
    g.pours = of("Polygon").map(|p| parse_pour(p, &layers)).collect();
    let pad_nets: Vec<Option<u32>> = of("Pad").map(|p| index(p, "NET")).collect();

    for p in of("Region") {
        let layer = layer(p);
        if matches!(layer, LayerRef::AllCopper | LayerRef::Keepout)
            || p.int("KIND").unwrap_or(0) != 0
            || p.bool("ISBOARDCUTOUT")
        {
            continue;
        }
        let n = p.int("MAINCONTOURVERTEXCOUNT").unwrap_or(0).max(0) as usize;
        // Shape-based regions give their outline as VX/VY in mil, with arcs.
        let outline = if p.bool("ISSHAPEBASED") && p.get("MVX0").is_none() {
            crate::pcb::vertices(p)
        } else {
            contour(p, "MVX", "MVY", n)
        };
        if outline.len() < 3 {
            continue;
        }
        let holes = (0..p.int("HOLECOUNT").unwrap_or(0).max(0))
            .map(|h| {
                let n = p.int(&format!("HOLE{h}VERTEXCOUNT")).unwrap_or(0).max(0) as usize;
                contour(p, &format!("H{h}VX"), &format!("H{h}VY"), n)
            })
            .filter(|h| h.len() >= 3)
            .collect();
        let net = index(p, "NET")
            .or_else(|| {
                pour_nets
                    .get(index(p, "POLYGON")? as usize)
                    .copied()
                    .flatten()
            })
            .or_else(|| {
                // PADINDEX counts pads from 1.
                pad_nets
                    .get((index(p, "PADINDEX")? as usize).checked_sub(1)?)
                    .copied()
                    .flatten()
            });
        let poly = Polygon {
            layer: layer.clone(),
            net,
            outline,
            holes,
            pour: index(p, "POLYGON"),
            component: index(p, "COMPONENT"),
        };
        if matches!(layer, LayerRef::Copper(_)) {
            let (mask, paste) = expansions(p, &rules);
            g.areas
                .extend(crate::pcb::openings(&poly, ncopper, mask, paste));
            g.polygons.push(poly);
        } else {
            g.areas.push(poly);
        }
    }
    // Fills: rectangles, turned about their centre.
    for p in of("Fill") {
        let layer = layer(p);
        let (Some(a), Some(b)) = (pt(p, "X1", "Y1"), pt(p, "X2", "Y2")) else {
            continue;
        };
        if matches!(layer, LayerRef::AllCopper | LayerRef::Keepout) || p.bool("KEEPOUT") {
            continue;
        }
        let poly = Polygon {
            layer: layer.clone(),
            net: index(p, "NET"),
            outline: crate::pcb::fill_outline(a, b, angle(p, "ROTATION")),
            holes: Vec::new(),
            pour: None,
            component: index(p, "COMPONENT"),
        };
        if matches!(layer, LayerRef::Copper(_)) {
            let (mask, paste) = expansions(p, &rules);
            g.areas
                .extend(crate::pcb::openings(&poly, ncopper, mask, paste));
            g.polygons.push(poly);
        } else {
            g.areas.push(poly);
        }
    }
    // Text, in the stroke font, on the layers that go to manufacture.
    let comps: Vec<&Props> = of("Component").collect();
    let shown = crate::pcb::Shown {
        name: comps
            .iter()
            .map(|p| {
                p.get("NAMEON")
                    .is_none_or(|v| v.eq_ignore_ascii_case("TRUE"))
            })
            .collect(),
        comment: comps.iter().map(|p| p.bool("COMMENTON")).collect(),
    };
    let mut text_tracks = Vec::new();
    for p in of("Text") {
        let text = p
            .get("WIDESTRING")
            .map(|w| {
                w.split(',')
                    .filter_map(|n| n.trim().parse::<u32>().ok())
                    .filter_map(char::from_u32)
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| p.str("TEXT"));
        let Some(at) = pt(p, "X", "Y") else { continue };
        let t = crate::pcb::AltText {
            layer: layer(p),
            component: index(p, "COMPONENT"),
            at,
            height: len(p, "HEIGHT").unwrap_or_default(),
            width: len(p, "WIDTH").unwrap_or_default(),
            rotation_deg: angle(p, "ROTATION"),
            mirror: p.bool("MIRROR"),
            text,
            truetype: p.bool("USETTFONTS"),
            bold: p.bool("BOLD"),
            designator: p.bool("DESIGNATOR"),
            comment: p.bool("COMMENT"),
            bbox: pt(p, "X1", "Y1").zip(pt(p, "X2", "Y2")),
        };
        text_tracks.push(t);
    }
    g.outline = parse_outline(&board_props);
    for t in &text_tracks {
        let tracks = crate::pcb::text_tracks(t, &board, &shown);
        board.geometry.tracks.extend(tracks);
    }
    let g = &mut board.geometry;
    board.stats.tracks = g.tracks.len();
    board.stats.vias = g.vias.len();
    board.stats.pads = g.pads.len();

    trace_core::connectivity::analyse(&mut board);
    board.stats.polygons = pour_nets.len();
    for n in pour_nets.into_iter().flatten() {
        if let Some(net) = board.nets.get_mut(n as usize) {
            net.has_pour = true;
        }
    }
    Ok(board)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_pad_hash() {
        let h = "SerializeLayerHash.Version=2,ClassName=class CustomLayerHash * __ptr64,16777226=1,16777218=1,16842751=1,16777219=0,";
        assert_eq!(removed_pads(h), vec![10, 2, 32]);
    }

    #[test]
    fn layer_names() {
        assert_eq!(layer_id("TOP"), 1);
        assert_eq!(layer_id("MID4"), 5);
        assert_eq!(layer_id("BOTTOM"), 32);
        assert_eq!(layer_id("MECHANICAL13"), 69);
        assert_eq!(layer_id("MULTILAYER"), 74);
        assert_eq!(layer_id("PLANE2"), 40);
    }

    #[test]
    fn reads_a_small_ascii_board() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.PcbDoc");
        let text = [
            "|RECORD=Board|V9_STACK_LAYER0_NAME=Top Layer|V9_STACK_LAYER0_LAYERID=16777217|V9_STACK_LAYER0_COPTHICK=1.4mil|V9_STACK_LAYER1_NAME=Core|V9_STACK_LAYER1_LAYERID=17039361|V9_STACK_LAYER1_DIELHEIGHT=10mil|V9_STACK_LAYER2_NAME=Bottom Layer|V9_STACK_LAYER2_LAYERID=16842751|V9_STACK_LAYER2_COPTHICK=1.4mil|VX0=0mil|VY0=0mil|VX1=1000mil|VY1=0mil|VX2=1000mil|VY2=1000mil",
            "|RECORD=Net|ID=0|NAME=A",
            "|RECORD=Net|ID=1|NAME=GND",
            "|RECORD=Component|ID=0|LAYER=TOP|X=100mil|Y=100mil|PATTERN=R0603|SOURCEDESIGNATOR=R1|ROTATION= 9.00000000000000E+0001",
            "|RECORD=Pad|NET=0|COMPONENT=0|LAYER=TOP|NAME=1|X=100mil|Y=100mil|XSIZE=20mil|YSIZE=30mil|SHAPE=RECTANGLE|HOLESIZE=0mil|ROTATION= 0.00000000000000E+0000|PADMODE=0",
            "|RECORD=Track|NET=0|LAYER=TOP|X1=100mil|Y1=100mil|X2=500mil|Y2=100mil|WIDTH=8mil",
            "|RECORD=Arc|NET=0|LAYER=TOP|LOCATION.X=500mil|LOCATION.Y=200mil|RADIUS=100mil|STARTANGLE= 2.70000000000000E+0002|ENDANGLE= 3.60000000000000E+0002|WIDTH=8mil",
            "|RECORD=Via|NET=0|LAYER=MULTILAYER|X=600mil|Y=200mil|DIAMETER=18mil|HOLESIZE=8mil|STARTLAYER=TOP|ENDLAYER=BOTTOM",
            "|RECORD=Polygon|ID=0|NET=1|LAYER=BOTTOM",
            "RECORD=Region|POLYGON=0|LAYER=BOTTOM|KIND=0|MAINCONTOURVERTEXCOUNT=3|MVX0=0|MVY0=0|MVX1=10000000|MVY1=0|MVX2=0|MVY2=10000000|HOLECOUNT=0",
        ]
        .join("\r");
        std::fs::write(&path, text).unwrap();
        let b = crate::read_pcbdoc(&path).unwrap();
        assert_eq!(b.stackup.copper_layers().count(), 2);
        assert_eq!(b.nets.len(), 2);
        assert_eq!(b.components[0].designator, "R1");
        let g = &b.geometry;
        assert_eq!(g.tracks.len(), 1);
        assert_eq!(g.tracks[0].layer, LayerRef::Copper(0));
        assert_eq!(g.arcs.len(), 1);
        assert_eq!(g.vias[0].to_layer, 1);
        assert_eq!(g.pads[0].shape, PadShape::Rect);
        assert_eq!(g.polygons[0].net, Some(1));
        assert_eq!(g.polygons[0].layer, LayerRef::Copper(1));
        assert_eq!(g.outline.len(), 3);
        assert!(b.nets[1].has_pour);
    }
}
