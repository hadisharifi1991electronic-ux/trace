//! `items <gerber> x y [r]`: objects of one Gerber file around a point, in file order.
use trace_gerber::gerber::{parse, Obj};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, y): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let r: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(0.5);
    let g = parse(&std::fs::read(&a[0]).unwrap()).unwrap();
    let near = |p: &[f64; 2]| (p[0] - x).hypot(p[1] - y) < r;
    for (i, it) in g.items.iter().enumerate() {
        let hit = match &it.obj {
            Obj::Flash { at, ap, .. } => {
                near(at).then(|| format!("flash D{ap} {:?}", g.aperture(*ap).shape))
            }
            Obj::Line { a, b, ap } => {
                (near(a) || near(b)).then(|| format!("line D{ap} {a:.3?}-{b:.3?}"))
            }
            Obj::Arc { a, b, ap, .. } => {
                (near(a) || near(b)).then(|| format!("arc D{ap} {a:.3?}-{b:.3?}"))
            }
            Obj::Region(cs) => cs.iter().flatten().any(near).then(|| {
                let pts: usize = cs.iter().map(|c| c.len()).sum();
                let bb =
                    cs.iter()
                        .flatten()
                        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| {
                            [
                                b[0].min(q[0]),
                                b[1].min(q[1]),
                                b[2].max(q[0]),
                                b[3].max(q[1]),
                            ]
                        });
                format!("region {} contours {pts} pts bbox {bb:.3?}", cs.len())
            }),
        };
        if let Some(h) = hit {
            println!("{i:6} {} {h}", if it.dark { "dark " } else { "clear" });
        }
    }
}
