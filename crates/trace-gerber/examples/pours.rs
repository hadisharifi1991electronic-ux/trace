//! `pours <path> <layer>`: copper pours on one layer with their hole counts.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let l: u8 = a[1].parse().unwrap();
    let imp = trace_gerber::read_path(&a[0]).unwrap();
    let g = &imp.board.geometry;
    let mut n = 0;
    for p in &g.polygons {
        if p.layer == trace_core::LayerRef::Copper(l) {
            n += 1;
            if p.outline.len() > 6 || !p.holes.is_empty() {
                println!(
                    "pour pts {} holes {} net {:?}",
                    p.outline.len(),
                    p.holes.len(),
                    p.net.map(|n| imp.board.nets[n as usize].name.clone())
                );
            }
        }
    }
    println!("{n} pours");
    let gb = trace_gerber::gerber::parse(&std::fs::read(&a[2]).unwrap()).unwrap();
    let dark = gb.items.iter().filter(|i| i.dark).count();
    println!(
        "items {} dark {} neg {} {}",
        gb.items.len(),
        dark,
        gb.image_negative,
        gb.file_polarity_negative
    );
}
