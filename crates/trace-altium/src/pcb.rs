use crate::records::{f64_at, i32_at, split_binary, split_pcb, u16_at, Props};
use crate::{Container, Error, Result};
use std::collections::BTreeMap;
use std::path::Path;
use trace_core::*;

/// Altium stores coordinates in units of 1/10,000 mil.
fn internal(v: i32) -> f64 {
    v as f64 / 10_000.0
}

/// Copper layers use ids 1 (top) to 32 (bottom).
fn is_copper(layer: u8) -> bool {
    (1..=32).contains(&layer)
}

/// Reads a `.PcbDoc`: stack-up, nets with routed lengths, classes, differential
/// pairs, components and design rules.
pub fn read_pcbdoc(path: impl AsRef<Path>) -> Result<Board> {
    let mut c = Container::open(path.as_ref())?;
    let board_data = c
        .stream("/Board6/Data")?
        .ok_or_else(|| Error::MissingStream("Board6/Data".into()))?;

    let mut board = Board::default();
    if let Some(props) = split_pcb(&board_data, 0).into_iter().next() {
        board.stackup = parse_stackup(&props);
    }

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

    // Routed length per net from copper tracks and arcs. Both records start
    // with: layer u8, two flag bytes, net index u16 and 8 bytes of links.
    // Tracks then hold their end points from offset 13; arcs hold centre,
    // radius and start/end angles (f64 degrees) from offset 13.
    let mut length_mil = vec![0.0f64; board.nets.len()];
    let mut add = |net: Option<u16>, mil: f64| {
        if let Some(slot) = net.and_then(|n| length_mil.get_mut(n as usize)) {
            *slot += mil;
        }
    };
    if let Some(d) = c.stream("/Tracks6/Data")? {
        let recs = split_binary(&d, 1);
        board.stats.tracks = recs.len();
        for b in recs {
            if !b.first().is_some_and(|&l| is_copper(l)) {
                continue;
            }
            if let (Some(x1), Some(y1), Some(x2), Some(y2)) =
                (i32_at(b, 13), i32_at(b, 17), i32_at(b, 21), i32_at(b, 25))
            {
                let dx = internal(x2) - internal(x1);
                let dy = internal(y2) - internal(y1);
                add(u16_at(b, 3), dx.hypot(dy));
            }
        }
    }
    if let Some(d) = c.stream("/Arcs6/Data")? {
        for b in split_binary(&d, 1) {
            if !b.first().is_some_and(|&l| is_copper(l)) {
                continue;
            }
            if let (Some(r), Some(a0), Some(a1)) = (i32_at(b, 21), f64_at(b, 25), f64_at(b, 33)) {
                if r <= 0 || !a0.is_finite() || !a1.is_finite() {
                    continue;
                }
                let mut sweep = (a1 - a0).rem_euclid(360.0);
                if sweep == 0.0 {
                    sweep = 360.0;
                }
                add(u16_at(b, 3), internal(r) * sweep.to_radians());
            }
        }
    }
    for (net, mil) in board.nets.iter_mut().zip(length_mil) {
        net.routed_length = Length::from_mil(mil);
    }
    if let Some(d) = c.stream("/Vias6/Data")? {
        let recs = split_binary(&d, 1);
        board.stats.vias = recs.len();
        for b in recs {
            if let Some(net) = u16_at(b, 3).and_then(|n| board.nets.get_mut(n as usize)) {
                net.vias += 1;
            }
        }
    }
    if let Some(d) = c.stream("/Pads6/Data")? {
        board.stats.pads = split_binary(&d, 6).len();
    }
    board.stats.polygons = text(&mut c, "/Polygons6/Data", 0)?.len();

    Ok(board)
}

fn parse_stackup(p: &Props) -> Stackup {
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

fn parse_rule(p: &Props) -> Rule {
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
