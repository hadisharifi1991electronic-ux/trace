//! `pourlist <path> <layer>`: pours on one copper layer with net, box and hole count.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let l: u8 = a[1].parse().unwrap();
    let b = trace_gerber::read_path(&a[0]).unwrap().board;
    for q in &b.geometry.polygons {
        if q.layer != trace_core::LayerRef::Copper(l) {
            continue;
        }
        let xs: Vec<f64> = q.outline.iter().map(|p| p.x.mm()).collect();
        let ys: Vec<f64> = q.outline.iter().map(|p| p.y.mm()).collect();
        let f = |v: &[f64], m: bool| {
            v.iter().cloned().fold(
                if m { f64::MIN } else { f64::MAX },
                if m { f64::max } else { f64::min },
            )
        };
        let c = [
            (f(&xs, true) + f(&xs, false)) / 2.0,
            (f(&ys, true) + f(&ys, false)) / 2.0,
        ];
        if a.len() > 3 {
            let (x, y): (f64, f64) = (a[2].parse().unwrap(), a[3].parse().unwrap());
            if (c[0] - x).hypot(c[1] - y) > 3.0 {
                continue;
            }
        } else if f(&xs, true) - f(&xs, false) < 3.0 && f(&ys, true) - f(&ys, false) < 3.0 {
            continue;
        }
        println!(
            "x {:8.3}..{:8.3} y {:8.3}..{:8.3} pts {:5} holes {:4} net {}",
            f(&xs, false),
            f(&xs, true),
            f(&ys, false),
            f(&ys, true),
            q.outline.len(),
            q.holes.len(),
            q.net
                .map(|n| b.nets[n as usize].name.clone())
                .unwrap_or("-".into())
        );
    }
}
