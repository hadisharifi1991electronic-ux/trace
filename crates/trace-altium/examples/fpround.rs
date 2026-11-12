//! Gives every part of a board its own footprint again (taken from the
//! first part using it) and reports pads that moved.
//!
//!     cargo run --release -p trace-altium --example fpround -- <board.PcbDoc>
use trace_core::footprint::{parts_using, replace, Footprint};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let board = trace_altium::read_pcbdoc(&a[0]).unwrap();
    let mut names: Vec<String> = board
        .components
        .iter()
        .map(|c| c.footprint.clone())
        .collect();
    names.sort();
    names.dedup();
    let mut worst = 0.0f64;
    for name in names {
        let parts = parts_using(&board, &name);
        let Some(fp) = Footprint::from_part(&board, parts[0]) else {
            continue;
        };
        let Some(e) = replace(&board, &fp, &parts) else {
            continue;
        };
        let mut b = board.clone();
        e.apply(&mut b);
        for &i in &parts {
            let me = Some(i as u32);
            let old: Vec<_> = board
                .geometry
                .pads
                .iter()
                .filter(|p| p.component == me)
                .collect();
            let new: Vec<_> = b
                .geometry
                .pads
                .iter()
                .filter(|p| p.component == me)
                .collect();
            if old.len() != new.len() {
                println!(
                    "{name} {}: {} pads before, {} after",
                    board.components[i].designator,
                    old.len(),
                    new.len()
                );
                continue;
            }
            for p in &old {
                let dist = |q: &&&trace_core::Pad| {
                    (p.at.x.mil() - q.at.x.mil()).hypot(p.at.y.mil() - q.at.y.mil())
                };
                let Some(q) = new
                    .iter()
                    .filter(|q| q.name == p.name && q.layer == p.layer)
                    .min_by(|a, b| dist(a).total_cmp(&dist(b)))
                else {
                    println!(
                        "{name} {} pad {} {:?}: gone",
                        board.components[i].designator, p.name, p.layer
                    );
                    continue;
                };
                let d = (p.at.x.mil() - q.at.x.mil()).hypot(p.at.y.mil() - q.at.y.mil());
                let ds = (p.size_x.mil() - q.size_x.mil()).abs()
                    + (p.size_y.mil() - q.size_y.mil()).abs();
                let dr = ((p.rotation_deg - q.rotation_deg).rem_euclid(180.0))
                    .min((q.rotation_deg - p.rotation_deg).rem_euclid(180.0));
                if d > 0.01 || ds > 0.01 || dr > 0.01 || p.net != q.net {
                    worst = worst.max(d);
                    println!(
                        "{name} {} pad {}: moved {d:.3} mil, size {ds:.3}, turn {:.1} -> {:.1}, net {:?} -> {:?}",
                        board.components[i].designator, p.name, p.rotation_deg, q.rotation_deg, p.net, q.net
                    );
                }
            }
        }
    }
    println!("worst move {worst:.3} mil");
}
