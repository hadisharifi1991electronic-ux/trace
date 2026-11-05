fn main() {
    for a in std::env::args().skip(1) {
        let b = trace_altium::read_pcbdoc(&a).unwrap();
        let g = &b.geometry;
        println!("{a}: {} pours, {} fills", g.pours.len(), g.polygons.len());
        for (i, p) in g.pours.iter().enumerate().take(12) {
            let fills = g
                .polygons
                .iter()
                .filter(|f| f.pour == Some(i as u32))
                .count();
            let name = p
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| n.name.as_str())
                .unwrap_or("-");
            println!(
                "  #{i} {:?} {name} pts={} order={} dead={} fills={fills}",
                p.layer,
                p.outline.len(),
                p.order,
                p.remove_dead
            );
        }
        let orphan = g.polygons.iter().filter(|f| f.pour.is_none()).count();
        println!("  fills without pour: {orphan}");
    }
}
