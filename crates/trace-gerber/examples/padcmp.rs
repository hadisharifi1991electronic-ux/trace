//! `padcmp <a> <b>`: pads whose shape or size differ between two imports of the same board.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let x = trace_gerber::read_path(&a[0]).unwrap().board;
    let y = trace_gerber::read_path(&a[1]).unwrap().board;
    let key = |p: &trace_core::Pad| {
        (
            format!("{:?}", p.layer),
            (p.at.x.mm() * 100.0).round() as i64,
            (p.at.y.mm() * 100.0).round() as i64,
        )
    };
    let ym: std::collections::HashMap<_, &trace_core::Pad> =
        y.geometry.pads.iter().map(|p| (key(p), p)).collect();
    let mut shown = 0;
    let (mut missing, mut diff) = (0, 0);
    for p in &x.geometry.pads {
        match ym.get(&key(p)) {
            None => {
                missing += 1;
                if shown < 12 {
                    shown += 1;
                    println!(
                        "missing {:?} ({:.3},{:.3}) {:.3}x{:.3} {:?} rot {}",
                        p.layer,
                        p.at.x.mm(),
                        p.at.y.mm(),
                        p.size_x.mm(),
                        p.size_y.mm(),
                        p.shape,
                        p.rotation_deg
                    );
                }
            }
            Some(q) => {
                let (w1, h1) = (p.size_x.mm(), p.size_y.mm());
                let (w2, h2) = (q.size_x.mm(), q.size_y.mm());
                let same = |r1: f64, r2: f64| {
                    let d = (r1 - r2).rem_euclid(180.0);
                    !(0.5..=179.5).contains(&d)
                };
                let ok = ((w1 - w2).abs() < 0.01
                    && (h1 - h2).abs() < 0.01
                    && (same(p.rotation_deg, q.rotation_deg) || (w1 - h1).abs() < 0.01))
                    || ((w1 - h2).abs() < 0.01
                        && (h1 - w2).abs() < 0.01
                        && same(p.rotation_deg + 90.0, q.rotation_deg));
                if !ok {
                    diff += 1;
                    if shown < 24 {
                        shown += 1;
                        println!("differs {:?} ({:.3},{:.3}) {:.3}x{:.3} {:?} rot {} -> {:.3}x{:.3} {:?} rot {}", p.layer, p.at.x.mm(), p.at.y.mm(), w1, h1, p.shape, p.rotation_deg, w2, h2, q.shape, q.rotation_deg);
                    }
                }
            }
        }
    }
    println!(
        "{} pads, {missing} missing, {diff} differ",
        x.geometry.pads.len()
    );
}
