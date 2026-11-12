fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let board = trace_altium::read_pcbdoc(&a[0]).unwrap();
    for (i, c) in board
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.footprint == a[1])
    {
        let pads: Vec<String> = board
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(i as u32))
            .map(|p| {
                format!(
                    "{}:({:.1},{:.1}) r{:.0} {:?}",
                    p.name,
                    p.at.x.mil() - c.x.mil(),
                    p.at.y.mil() - c.y.mil(),
                    p.rotation_deg,
                    p.layer
                )
            })
            .collect();
        println!(
            "{} {:?} rot {} : {}",
            c.designator,
            c.side,
            c.rotation_deg,
            pads.join("  ")
        );
    }
}
