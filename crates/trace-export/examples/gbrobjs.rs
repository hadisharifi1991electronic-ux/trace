//! Lists the largest objects in a Gerber file (for comparing outputs).
use trace_gerber::geom;
use trace_gerber::gerber::{self, Obj};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = gerber::parse(&std::fs::read(&a[0]).unwrap()).unwrap();
    let mut v: Vec<(f64, String)> = Vec::new();
    for it in &g.items {
        let (pts, what): (Vec<geom::Pt>, String) = match &it.obj {
            Obj::Flash { ap, at, xf } => (
                geom::flash_shapes(g.aperture(*ap), *at, xf)
                    .into_iter()
                    .flat_map(|s| s.1)
                    .collect(),
                format!(
                    "flash D{} {:?}",
                    g.aperture(*ap).dcode,
                    g.aperture(*ap).shape
                )
                .chars()
                .take(80)
                .collect(),
            ),
            Obj::Region(c) => (
                c.iter().flatten().copied().collect(),
                format!("region {} pts", c.iter().map(|c| c.len()).sum::<usize>()),
            ),
            _ => continue,
        };
        let b = geom::bbox(&pts);
        v.push((
            (b[2] - b[0]) * (b[3] - b[1]),
            format!(
                "{what} dark={} box {:.3},{:.3} .. {:.3},{:.3} ({:.0},{:.0} mil)",
                it.dark,
                b[0],
                b[1],
                b[2],
                b[3],
                b[0] / 0.0254,
                b[1] / 0.0254
            ),
        ));
    }
    v.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (a, s) in v
        .iter()
        .take(a.get(1).and_then(|n| n.parse().ok()).unwrap_or(15))
    {
        println!("{a:9.2} {s}");
    }
}
