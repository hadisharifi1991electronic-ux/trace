//! Pours every pour of a board again and compares with the file's copper.
use trace_core::*;

fn area(f: &Polygon) -> f64 {
    let a = |r: &[Point]| {
        let n = r.len();
        ((0..n)
            .map(|i| {
                let (p, q) = (r[i], r[(i + 1) % n]);
                p.x.mil() * q.y.mil() - q.x.mil() * p.y.mil()
            })
            .sum::<f64>()
            / 2.0)
            .abs()
    };
    a(&f.outline) - f.holes.iter().map(|h| a(h)).sum::<f64>()
}

fn main() {
    for path in std::env::args().skip(1) {
        let b = trace_altium::read_pcbdoc(&path).unwrap();
        println!("{path}");
        let t0 = std::time::Instant::now();
        let (mut tot_a, mut tot_b) = (0.0, 0.0);
        for (i, p) in b.geometry.pours.iter().enumerate() {
            if p.outline.len() < 3 {
                continue;
            }
            let old: f64 = b
                .geometry
                .polygons
                .iter()
                .filter(|f| f.pour == Some(i as u32))
                .map(area)
                .sum();
            let t = std::time::Instant::now();
            let new_f = trace_core::pour::pour(&b, i);
            let new: f64 = new_f.iter().map(area).sum();
            tot_a += old;
            tot_b += new;
            let name = p
                .net
                .and_then(|n| b.nets.get(n as usize))
                .map(|n| n.name.as_str())
                .unwrap_or("-");
            println!(
                "  #{i:<3} {:?} {name:<14} file {:>10.0} sq mil, poured {:>10.0} ({:+.1}%), {} pieces, {:.0} ms",
                p.layer,
                old,
                new,
                if old > 0.0 { (new - old) / old * 100.0 } else { 0.0 },
                new_f.len(),
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
        println!(
            "  total file {tot_a:.0} poured {tot_b:.0} in {:.1} s",
            t0.elapsed().as_secs_f64()
        );
    }
}
