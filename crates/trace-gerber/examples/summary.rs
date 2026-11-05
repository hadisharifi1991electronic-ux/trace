//! `summary <path>`: parts, nets and nets that came out in several pieces.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = trace_gerber::read_path(&p).unwrap().board;
    let split: Vec<&String> = b
        .nets
        .iter()
        .map(|n| &n.name)
        .filter(|n| n.ends_with(')') && n.contains(" ("))
        .collect();
    println!(
        "parts {}, nets {}, split pieces {}",
        b.components.len(),
        b.nets.len(),
        split.len()
    );
    for s in split.iter().take(15) {
        println!("  {s}");
    }
    let named = b
        .geometry
        .pads
        .iter()
        .filter(|p| p.component.is_some())
        .count();
    println!("pads with a part {named} of {}", b.geometry.pads.len());
}
