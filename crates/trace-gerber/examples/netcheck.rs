//! Compares nets traced from bare Gerbers with the nets named in X2 files of
//! the same board: `netcheck <x2 dir> <bare dir>`.
use std::collections::{HashMap, HashSet};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let imp = trace_gerber::read_path(&a[0]).unwrap();
    let truth = imp.board;
    if std::env::var_os("TRACE_NET_DEBUG").is_some() {
        let mut t = truth.clone();
        trace_gerber::nets::trace(&mut t, &imp.trace_input);
    }
    let t0 = std::time::Instant::now();
    let test = trace_gerber::read_path(&a[1]).unwrap().board;
    println!("traced in {:.2}s", t0.elapsed().as_secs_f64());
    let mut pairs: Vec<(Option<u32>, Option<u32>, String)> = Vec::new();
    let (g1, g2) = (&truth.geometry, &test.geometry);
    let tk = |t: &trace_core::Track| {
        (
            format!("{:?}", t.layer),
            t.start.x.0 / 1000,
            t.start.y.0 / 1000,
            t.end.x.0 / 1000,
            t.end.y.0 / 1000,
            t.width.0 / 1000,
        )
    };
    let tmap: HashMap<_, Option<u32>> = g2
        .tracks
        .iter()
        .filter(|t| matches!(t.layer, trace_core::LayerRef::Copper(_)))
        .map(|t| (tk(t), t.net))
        .collect();
    for (i, x) in g1.tracks.iter().enumerate() {
        if matches!(x.layer, trace_core::LayerRef::Copper(_)) {
            if let Some(n) = tmap.get(&tk(x)) {
                pairs.push((
                    x.net,
                    *n,
                    format!(
                        "track {i} {:?} ({:.3},{:.3})",
                        x.layer,
                        x.start.x.mm(),
                        x.start.y.mm()
                    ),
                ));
            }
        }
    }
    let vmap: HashMap<(i64, i64), Option<u32>> = g2
        .vias
        .iter()
        .map(|v| ((v.at.x.0 / 1000, v.at.y.0 / 1000), v.net))
        .collect();
    for (i, x) in g1.vias.iter().enumerate() {
        if let Some(n) = vmap.get(&(x.at.x.0 / 1000, x.at.y.0 / 1000)) {
            pairs.push((
                x.net,
                *n,
                format!("via {i} ({:.3},{:.3})", x.at.x.mm(), x.at.y.mm()),
            ));
        }
    }
    let key2: HashMap<(String, i64, i64), Option<u32>> = g2
        .pads
        .iter()
        .map(|p| {
            (
                (format!("{:?}", p.layer), p.at.x.0 / 1000, p.at.y.0 / 1000),
                p.net,
            )
        })
        .collect();
    for p in &g1.pads {
        if let Some(n) = key2.get(&(format!("{:?}", p.layer), p.at.x.0 / 1000, p.at.y.0 / 1000)) {
            pairs.push((
                p.net,
                *n,
                format!("pad {:?} ({:.3},{:.3})", p.layer, p.at.x.mm(), p.at.y.mm()),
            ));
        }
    }
    let name = |b: &trace_core::Board, n: Option<u32>| {
        n.map(|n| b.nets[n as usize].name.clone())
            .unwrap_or("-".into())
    };
    let mut t2s: HashMap<String, HashSet<String>> = HashMap::new();
    let mut s2t: HashMap<String, HashSet<String>> = HashMap::new();
    let mut example: HashMap<(String, String), String> = HashMap::new();
    for (x, y, what) in &pairs {
        let (tn, sn) = (name(&truth, *x), name(&test, *y));
        if tn == "-" {
            continue;
        }
        t2s.entry(tn.clone()).or_default().insert(sn.clone());
        s2t.entry(sn.clone()).or_default().insert(tn.clone());
        example.entry((tn, sn)).or_insert(what.clone());
    }
    let splits: Vec<_> = t2s.iter().filter(|(_, v)| v.len() > 1).collect();
    let merges: Vec<_> = s2t
        .iter()
        .filter(|(k, v)| *k != "-" && v.len() > 1)
        .collect();
    println!(
        "{} named nets, {} objects compared; {} nets split, {} traced nets merge several",
        t2s.len(),
        pairs.len(),
        splits.len(),
        merges.len()
    );
    for (k, v) in splits.iter().take(8) {
        println!(
            "  split {k}: {:?}",
            v.iter()
                .map(|s| (s, example.get(&((*k).clone(), s.clone()))))
                .collect::<Vec<_>>()
        );
    }
    for (k, v) in merges.iter().take(8) {
        println!(
            "  merge {k}: {:?}",
            v.iter()
                .take(6)
                .map(|t| (t, example.get(&(t.clone(), (*k).clone()))))
                .collect::<Vec<_>>()
        );
    }
}
