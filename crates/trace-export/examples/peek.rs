fn main() {
    let a: Vec<String> = std::env::args().collect();
    let x = trace_altium::read_pcbdoc(&a[1]).unwrap();
    let y = trace_altium::read_pcbdoc(&a[2]).unwrap();
    let k = |p: &trace_core::Polygon| (p.outline.first().map(|q| (q.x.0 / 1000, q.y.0 / 1000)), format!("{:?}", p.layer));
    let ya: Vec<_> = y.geometry.polygons.iter().chain(&y.geometry.areas).collect();
    let mut n = 0;
    for p in x.geometry.polygons.iter().chain(&x.geometry.areas) {
        if let Some(q) = ya.iter().find(|q| k(q) == k(p)) {
            if p.component != q.component || p.pour != q.pour || p.outline.len() != q.outline.len() || p.holes.len() != q.holes.len() {
                if n < 4 { println!("{:?} {:?} {} {} | {:?} {:?} {} {}", p.component, p.pour, p.outline.len(), p.holes.len(), q.component, q.pour, q.outline.len(), q.holes.len()); }
                n += 1;
            }
        }
    }
    println!("{n}");
    for p in &x.geometry.pads { if !y.geometry.pads.iter().any(|q| q.name == p.name && q.at == p.at && q.size_x == p.size_x && q.rotation_deg == p.rotation_deg && q.shape == p.shape && q.stack.len() == p.stack.len()) { println!("{:?}", p); let q = y.geometry.pads.iter().find(|q| q.at == p.at); println!("  -> {:?}", q); break; } }
}
