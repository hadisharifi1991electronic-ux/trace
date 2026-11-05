//! `touch <path> <layer> x y`: for the largest pour on a layer, whether a point
//! lies in its copper and how far the nearest edge is.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let l: u8 = a[1].parse().unwrap();
    let (x, y): (f64, f64) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let b = trace_gerber::read_path(&a[0]).unwrap().board;
    let p = b
        .geometry
        .polygons
        .iter()
        .filter(|p| p.layer == trace_core::LayerRef::Copper(l))
        .max_by_key(|p| p.holes.len())
        .unwrap();
    let ring = |r: &Vec<trace_core::Point>| -> Vec<[f64; 2]> {
        r.iter().map(|q| [q.x.mm(), q.y.mm()]).collect()
    };
    let inside = |r: &[[f64; 2]]| {
        let mut c = false;
        let n = r.len();
        for i in 0..n {
            let (p1, p2) = (r[i], r[(i + 1) % n]);
            if (p1[1] > y) != (p2[1] > y)
                && x < p1[0] + (y - p1[1]) / (p2[1] - p1[1]) * (p2[0] - p1[0])
            {
                c = !c;
            }
        }
        c
    };
    let seg = |p: [f64; 2], q: [f64; 2]| {
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let l = dx * dx + dy * dy;
        let t = if l > 0.0 {
            (((x - p[0]) * dx + (y - p[1]) * dy) / l).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (x - p[0] - t * dx).hypot(y - p[1] - t * dy)
    };
    let o = ring(&p.outline);
    println!("in outline {}", inside(&o));
    let mut best = (f64::MAX, 0usize);
    for (hi, h) in p.holes.iter().enumerate() {
        let h = ring(h);
        if inside(&h) {
            println!("inside hole {hi} ({} pts)", h.len());
        }
        for i in 0..h.len() {
            let d = seg(h[i], h[(i + 1) % h.len()]);
            if d < best.0 {
                best = (d, hi);
            }
        }
    }
    println!("nearest hole edge {:.4} mm (hole {})", best.0, best.1);
    let h = ring(&p.holes[best.1]);
    let bb = h
        .iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| {
            [
                b[0].min(q[0]),
                b[1].min(q[1]),
                b[2].max(q[0]),
                b[3].max(q[1]),
            ]
        });
    println!("that hole bbox {:.3?}", bb);
}
