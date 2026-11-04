//! `trace view`: writes a self-contained HTML board viewer and opens it.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use trace_core::{Board, LayerRef, PadShape, Side};

const TEMPLATE: &str = include_str!("viewer.html");

fn key(l: &LayerRef) -> String {
    match l {
        LayerRef::Copper(i) => format!("C{i}"),
        LayerRef::TopSilk => "TS".into(),
        LayerRef::BottomSilk => "BS".into(),
        LayerRef::AllCopper => "ALL".into(),
        LayerRef::Keepout => "KO".into(),
        LayerRef::Mechanical(n) => format!("M{n}"),
        LayerRef::Other(id) => format!("L{id}"),
    }
}

fn label(l: &LayerRef, copper: &[String]) -> String {
    match l {
        LayerRef::Copper(i) => copper
            .get(*i as usize)
            .cloned()
            .unwrap_or_else(|| format!("Copper {i}")),
        LayerRef::TopSilk => "Top silk".into(),
        LayerRef::BottomSilk => "Bottom silk".into(),
        LayerRef::AllCopper => "Through-hole".into(),
        LayerRef::Keepout => "Keepout".into(),
        LayerRef::Mechanical(n) => format!("Mechanical {n}"),
        LayerRef::Other(id) => format!("Layer {id}"),
    }
}

/// Rounds to 0.01 mil so the embedded data stays small.
fn r(v: f64) -> Value {
    json!((v * 100.0).round() / 100.0)
}

fn net(n: Option<u32>) -> Value {
    n.map(|n| json!(n)).unwrap_or(json!(-1))
}

pub fn render(board: &Board, name: &str) -> String {
    let g = &board.geometry;
    let copper: Vec<String> = board
        .stackup
        .copper_layers()
        .map(|l| l.name.clone())
        .collect();
    let mut layer_keys = Map::new();
    let mut note = |l: &LayerRef| {
        layer_keys
            .entry(key(l))
            .or_insert_with(|| json!(label(l, &copper)));
    };

    let mut tracks: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for t in &g.tracks {
        note(&t.layer);
        tracks.entry(key(&t.layer)).or_default().extend([
            r(t.start.x.mil()),
            r(t.start.y.mil()),
            r(t.end.x.mil()),
            r(t.end.y.mil()),
            r(t.width.mil()),
            net(t.net),
        ]);
    }
    let mut arcs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for a in &g.arcs {
        note(&a.layer);
        arcs.entry(key(&a.layer)).or_default().extend([
            r(a.center.x.mil()),
            r(a.center.y.mil()),
            r(a.radius.mil()),
            r(a.start_deg),
            r(a.end_deg),
            r(a.width.mil()),
            net(a.net),
        ]);
    }
    let mut pads = Vec::with_capacity(g.pads.len() * 9);
    for p in &g.pads {
        note(&p.layer);
        let shape = match p.shape {
            PadShape::Round => 0,
            PadShape::Rect => 1,
            PadShape::Octagon => 2,
        };
        pads.extend([
            r(p.at.x.mil()),
            r(p.at.y.mil()),
            r(p.size_x.mil()),
            r(p.size_y.mil()),
            r(p.rotation_deg),
            json!(shape),
            net(p.net),
            json!(key(&p.layer)),
            p.component.map(|c| json!(c)).unwrap_or(json!(-1)),
        ]);
    }
    let vias: Vec<Value> = g
        .vias
        .iter()
        .flat_map(|v| {
            [
                r(v.at.x.mil()),
                r(v.at.y.mil()),
                r(v.diameter.mil()),
                r(v.hole.mil()),
                net(v.net),
            ]
        })
        .collect();
    let comps: Vec<Value> = board
        .components
        .iter()
        .map(|c| {
            json!([
                c.designator,
                (c.x.mil() * 100.0).round() / 100.0,
                (c.y.mil() * 100.0).round() / 100.0,
                if c.side == Side::Top { 0 } else { 1 },
                c.footprint
            ])
        })
        .collect();
    let nets: Vec<Value> = board
        .nets
        .iter()
        .map(|n| json!({"name": n.name, "len": (n.routed_length.mil() * 10.0).round() / 10.0, "vias": n.vias}))
        .collect();
    let pairs: Vec<Value> = board
        .diff_pairs
        .iter()
        .map(|p| json!([p.name, p.positive, p.negative]))
        .collect();
    let outline: Vec<Value> = g
        .outline
        .iter()
        .map(|p| json!([r(p.x.mil()), r(p.y.mil())]))
        .collect();
    for (i, name) in copper.iter().enumerate() {
        layer_keys
            .entry(format!("C{i}"))
            .or_insert_with(|| json!(name));
    }

    let data = json!({
        "name": name,
        "copper": copper,
        "layerKeys": layer_keys,
        "tracks": tracks,
        "arcs": arcs,
        "pads": pads,
        "vias": vias,
        "comps": comps,
        "nets": nets,
        "pairs": pairs,
        "outline": outline,
    });
    // `</` inside data must not close the script element.
    let data = data.to_string().replace("</", "<\\/");
    TEMPLATE.replacen("/*DATA*/null", &data, 1)
}

/// Opens a file with the system's default program.
pub fn open(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    if result.is_err() {
        eprintln!("Open {} in your browser to view the board.", path.display());
    }
}
