//! `viadiff <a> <b>`: vias (position, span, size) present in one board and not the other.
use std::collections::HashMap;
type V = (i64, i64, u8, u8, i64, i64);
fn load(path: &str) -> Vec<V> {
    let b = trace_gerber::read_path(path).unwrap().board;
    b.geometry
        .vias
        .iter()
        .map(|v| {
            (
                v.at.x.0,
                v.at.y.0,
                v.from_layer,
                v.to_layer,
                v.diameter.0,
                v.hole.0,
            )
        })
        .collect()
}
fn missing(a: &[V], b: &[V], tag: &str) -> usize {
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, v) in b.iter().enumerate() {
        grid.entry((v.0 / 50000, v.1 / 50000)).or_default().push(i);
    }
    let mut used = vec![false; b.len()];
    let mut n = 0;
    for v in a {
        let (gx, gy) = (v.0 / 50000, v.1 / 50000);
        let mut hit = None;
        'f: for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in grid
                    .get(&(gx + dx, gy + dy))
                    .map(|v| v.as_slice())
                    .unwrap_or(&[])
                {
                    let w = b[i];
                    if !used[i]
                        && (w.0 - v.0).abs() < 5000
                        && (w.1 - v.1).abs() < 5000
                        && w.2 == v.2
                        && w.3 == v.3
                        && (w.4 - v.4).abs() < 5000
                        && (w.5 - v.5).abs() < 5000
                    {
                        hit = Some(i);
                        break 'f;
                    }
                }
            }
        }
        match hit {
            Some(i) => used[i] = true,
            None => {
                n += 1;
                if n < 25 {
                    println!(
                        "only in {tag}: ({:.3},{:.3}) L{}-{} d {:.3} hole {:.3}",
                        v.0 as f64 / 1e6,
                        v.1 as f64 / 1e6,
                        v.2,
                        v.3,
                        v.4 as f64 / 1e6,
                        v.5 as f64 / 1e6
                    );
                }
            }
        }
    }
    n
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (x, y) = (load(&a[0]), load(&a[1]));
    let n = missing(&x, &y, "a") + missing(&y, &x, "b");
    println!("{n} differences");
}
