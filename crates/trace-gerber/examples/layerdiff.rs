//! `layerdiff <a> <b>`: per copper layer, counts and pour area of two boards.
fn area(p: &[trace_core::Point]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (&p[i], &p[(i + 1) % n]);
            a.x.mm() * b.y.mm() - b.x.mm() * a.y.mm()
        })
        .sum::<f64>()
        .abs()
        / 2.0
}
fn stats(path: &str) -> Vec<(usize, usize, usize, f64, f64)> {
    let b = trace_gerber::read_path(path).unwrap().board;
    let g = &b.geometry;
    let mut out = vec![(0, 0, 0, 0.0, 0.0); 40];
    for t in &g.tracks {
        if let trace_core::LayerRef::Copper(l) = t.layer {
            out[l as usize].0 += 1;
            out[l as usize].4 +=
                t.width.mm() * (t.start.x.mm() - t.end.x.mm()).hypot(t.start.y.mm() - t.end.y.mm());
        }
    }
    for p in &g.pads {
        if let trace_core::LayerRef::Copper(l) = p.layer {
            out[l as usize].1 += 1;
        }
    }
    for p in &g.polygons {
        if let trace_core::LayerRef::Copper(l) = p.layer {
            out[l as usize].2 += 1;
            out[l as usize].3 += area(&p.outline) - p.holes.iter().map(|h| area(h)).sum::<f64>();
        }
    }
    out
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, y) = (stats(&a[0]), stats(&a[1]));
    for l in 0..40 {
        if x[l] != (0, 0, 0, 0.0, 0.0) || y[l] != (0, 0, 0, 0.0, 0.0) {
            println!("L{l:2} tracks {:6} {:6}  pads {:5} {:5}  pours {:4} {:4} area {:10.1} {:10.1}  trackarea {:8.1} {:8.1}", x[l].0, y[l].0, x[l].1, y[l].1, x[l].2, y[l].2, x[l].3, y[l].3, x[l].4, y[l].4);
        }
    }
}
