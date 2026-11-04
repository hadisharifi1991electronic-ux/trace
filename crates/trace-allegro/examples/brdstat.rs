//! Prints what the block parser found in each `.brd` given.
fn main() {
    for p in std::env::args().skip(1) {
        let data = std::fs::read(&p).unwrap();
        let t = std::time::Instant::now();
        match trace_allegro::blocks::Db::parse(&data) {
            Ok(db) => {
                let mut counts = std::collections::BTreeMap::new();
                for b in &db.blocks {
                    *counts.entry(b.kind).or_insert(0usize) += 1;
                }
                println!(
                    "{p}: Allegro {} ({}) units {} div {} strings {} blocks {} in {:?}",
                    db.header.ver.label(),
                    db.header.program,
                    db.header.units,
                    db.header.divisor,
                    db.strings.len(),
                    db.blocks.len(),
                    t.elapsed()
                );
                println!(
                    "   {:?}",
                    counts
                        .iter()
                        .map(|(k, v)| format!("{k:02x}:{v}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            Err(e) => println!("{p}: ERROR {e}"),
        }
    }
}
