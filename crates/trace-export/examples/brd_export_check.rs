//! `brdcheck <board.brd> <gerber dir>`: compares an Allegro board's nets
//! with the nets traced from Gerbers plotted from its KiCad export.
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let truth = trace_allegro::read_brd(&a[0]).unwrap();
    let test = trace_gerber::read_path(&a[1]).unwrap().board;
    let r = |v: i64| (v as f64 / 10000.0).round() as i64;
    let tk = |t: &trace_core::Track| {
        (
            format!("{:?}", t.layer),
            r(t.start.x.0),
            r(t.start.y.0),
            r(t.end.x.0),
            r(t.end.y.0),
        )
    };
    let tmap: HashMap<_, Option<u32>> = test
        .geometry
        .tracks
        .iter()
        .map(|t| (tk(t), t.net))
        .collect();
    let vk = |v: &trace_core::Via| {
        (
            r(v.at.x.0),
            r(v.at.y.0),
            v.from_layer.min(v.to_layer),
            v.from_layer.max(v.to_layer),
        )
    };
    let vmap: HashMap<_, Option<u32>> = test.geometry.vias.iter().map(|v| (vk(v), v.net)).collect();
    let mut pairs = Vec::new();
    for t in &truth.geometry.tracks {
        if let (Some(n), Some(m)) = (t.net, tmap.get(&tk(t))) {
            pairs.push((
                n,
                *m,
                format!(
                    "track {:?} ({:.3},{:.3})",
                    t.layer,
                    t.start.x.mm(),
                    t.start.y.mm()
                ),
            ));
        }
    }
    for v in &truth.geometry.vias {
        if let (Some(n), Some(m)) = (v.net, vmap.get(&vk(v))) {
            pairs.push((
                n,
                *m,
                format!("via ({:.3},{:.3})", v.at.x.mm(), v.at.y.mm()),
            ));
        }
    }
    let mut fwd: HashMap<u32, HashMap<Option<u32>, String>> = HashMap::new();
    let mut back: HashMap<u32, HashMap<u32, String>> = HashMap::new();
    for (n, m, s) in &pairs {
        fwd.entry(*n).or_default().entry(*m).or_insert(s.clone());
        if let Some(m) = m {
            back.entry(*m).or_default().entry(*n).or_insert(s.clone());
        }
    }
    let name = |n: u32| {
        truth
            .nets
            .get(n as usize)
            .map(|x| x.name.clone())
            .unwrap_or_default()
    };
    let split: Vec<_> = fwd.iter().filter(|(_, v)| v.len() > 1).collect();
    let merge: Vec<_> = back.iter().filter(|(_, v)| v.len() > 1).collect();
    println!(
        "{} nets, {} objects compared; {} nets split, {} traced nets merge several",
        fwd.len(),
        pairs.len(),
        split.len(),
        merge.len()
    );
    for (n, v) in split.iter().take(5) {
        println!(
            "  split {}: {:?}",
            name(**n),
            v.values().take(4).collect::<Vec<_>>()
        );
    }
    for (_, v) in merge.iter().take(5) {
        println!(
            "  merge: {:?}",
            v.iter()
                .take(6)
                .map(|(n, s)| (name(*n), s))
                .collect::<Vec<_>>()
        );
    }
}
