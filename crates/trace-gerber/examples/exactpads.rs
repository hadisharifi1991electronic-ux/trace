//! `exactpads <path> x y`: whether pads near a point carry an exact copper shape.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, y): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let imp = trace_gerber::read_path(&a[0]).unwrap();
    for (i, p) in imp.board.geometry.pads.iter().enumerate() {
        if (p.at.x.mm() - x).hypot(p.at.y.mm() - y) < 0.3 {
            let e = imp.trace_input.exact.get(&i);
            println!(
                "pad {i} {:?} {:?} exact {:?}",
                p.layer,
                p.shape,
                e.map(|r| r.iter().map(|q| q.len()).collect::<Vec<_>>())
            );
            if let Some(r) = e {
                for q in r.iter().take(1) {
                    println!("  {:.3?}", &q[..q.len().min(12)]);
                }
            }
        }
    }
    println!("{} exact pads in all", imp.trace_input.exact.len());
}
