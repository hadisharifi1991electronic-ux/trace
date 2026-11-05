//! `bodies <file.PcbDoc>`: embedded 3D models and where each sits.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = trace_altium::read_pcbdoc(&p).unwrap();
    let g = &b.geometry;
    println!("{} models, {} bodies", g.models.len(), g.bodies.len());
    for body in &g.bodies {
        let c = body.component.and_then(|c| b.components.get(c as usize));
        println!(
            "{:8} {:28} at {:.2},{:.2} rot {:.0} model {:?} dz {:.2} bottom {} | comp at {:.2},{:.2} rot {:.0} {:?}",
            c.map_or("-", |c| c.designator.as_str()),
            g.models[body.model as usize].name,
            body.at.x.mm(), body.at.y.mm(), body.rotation_deg, body.model_rotation_deg,
            body.standoff.mm(), body.bottom,
            c.map_or(0.0, |c| c.x.mm()), c.map_or(0.0, |c| c.y.mm()), c.map_or(0.0, |c| c.rotation_deg), c.map(|c| c.side)
        );
    }
}
