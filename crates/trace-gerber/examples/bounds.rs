//! `bounds <path>`: outline points and the extent of each kind of object.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = trace_gerber::read_path(&p).unwrap().board;
    let g = &b.geometry;
    for q in &g.outline {
        print!("({:.1},{:.1}) ", q.x.mm(), q.y.mm());
    }
    println!();
    let ext = |it: &mut dyn Iterator<Item = (f64, f64)>| {
        let (mut a, mut c, mut bb, mut d) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in it {
            a = a.min(x);
            c = c.min(y);
            bb = bb.max(x);
            d = d.max(y);
        }
        format!("({a:.1},{c:.1})-({bb:.1},{d:.1})")
    };
    let mut by: std::collections::BTreeMap<String, Vec<(f64, f64)>> = Default::default();
    for t in &g.tracks {
        by.entry(format!("{:?}", t.layer))
            .or_default()
            .push((t.start.x.mm(), t.start.y.mm()));
    }
    for p in &g.pads {
        by.entry(format!("pad {:?}", p.layer))
            .or_default()
            .push((p.at.x.mm(), p.at.y.mm()));
    }
    for (k, v) in by {
        println!("{k}: {} {}", v.len(), ext(&mut v.into_iter()));
    }
}
