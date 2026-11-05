//! `near <dir> x y [r]`: lists objects near a point.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, y): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let r: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(1.0);
    let b = trace_gerber::read_path(&a[0]).unwrap().board;
    let g = &b.geometry;
    let d = |p: &trace_core::Point| (p.x.mm() - x).hypot(p.y.mm() - y);
    let net = |n: Option<u32>| n.map(|n| b.nets[n as usize].name.clone()).unwrap_or("-".into());
    for p in &g.pads { if d(&p.at) < r { println!("pad {:?} {:?} at ({:.3},{:.3}) {:.3}x{:.3} rot {:.1} hole {:.3} net {}", p.layer, p.shape, p.at.x.mm(), p.at.y.mm(), p.size_x.mm(), p.size_y.mm(), p.rotation_deg, p.hole.mm(), net(p.net)); } }
    for v in &g.vias { if d(&v.at) < r { println!("via at ({:.3},{:.3}) d {:.3} hole {:.3} net {}", v.at.x.mm(), v.at.y.mm(), v.diameter.mm(), v.hole.mm(), net(v.net)); } }
    for t in &g.tracks { if (d(&t.start) < r || d(&t.end) < r) && matches!(t.layer, trace_core::LayerRef::Copper(_)) { println!("track {:?} ({:.3},{:.3})-({:.3},{:.3}) w {:.3} net {}", t.layer, t.start.x.mm(), t.start.y.mm(), t.end.x.mm(), t.end.y.mm(), t.width.mm(), net(t.net)); } }
}
