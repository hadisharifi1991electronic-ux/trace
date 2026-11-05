//! `fracturecheck <board>`: checks that joining pour holes keeps each pour's area.
fn area(p: &[[f64; 2]]) -> f64 {
    (0..p.len())
        .map(|i| {
            let (u, v) = (p[i], p[(i + 1) % p.len()]);
            u[0] * v[1] - v[0] * u[1]
        })
        .sum::<f64>()
        / 2.0
}
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = trace_gerber::read_path(&p).unwrap().board;
    for (i, poly) in b.geometry.polygons.iter().enumerate() {
        let ring = |r: &Vec<trace_core::Point>| -> Vec<[f64; 2]> {
            r.iter().map(|q| [q.x.mm(), q.y.mm()]).collect()
        };
        let want = area(&ring(&poly.outline)).abs()
            - poly.holes.iter().map(|h| area(&ring(h)).abs()).sum::<f64>();
        let f = trace_export::fracture(&poly.outline, &poly.holes);
        let got = area(&f);
        if (got - want).abs() > 1e-3 * want.abs().max(1.0) {
            println!(
                "pour {i}: {} holes, area {want:.3} but fractured {got:.3}",
                poly.holes.len()
            );
        }
    }
}
