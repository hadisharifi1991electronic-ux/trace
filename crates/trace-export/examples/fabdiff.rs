//! Compares two sets of Gerbers layer by layer: draws each layer at 25 µm
//! and counts the area found in one set and not the other.
//!
//!     cargo run --release -p trace-export --example fabdiff -- <ours dir> <theirs dir> [png dir]
//!
//! Layers are paired by what each file was recognised as. The second set
//! is shifted so the plated holes of both line up (CAD tools may put the
//! origin elsewhere).

use std::collections::BTreeMap;
use std::path::Path;
use trace_gerber::detect::{detect, Content};
use trace_gerber::geom::{self, Pt};
use trace_gerber::gerber::{self, Gerber, Obj};

const RES: f64 = 0.025;

fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_file() {
            out.push((
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            ));
        }
    }
    out
}

struct Set {
    layers: BTreeMap<String, Gerber>,
    holes: Vec<(Pt, f64, Option<bool>)>,
}

fn load(dir: &Path) -> Set {
    let mut layers = BTreeMap::new();
    let mut holes = Vec::new();
    for (name, data) in files(dir) {
        if trace_gerber::excellon::sniff(&data) {
            if let Ok(d) = trace_gerber::excellon::parse(&data) {
                let (role, _) = detect(&name, Content::Drill(&d));
                let plated = match role {
                    trace_gerber::detect::Role::Drill { plated, .. } => plated,
                    _ => None,
                };
                for h in d.holes {
                    holes.push((h.at, h.diameter, h.plated.or(plated)));
                }
            }
            continue;
        }
        let Ok(g) = gerber::parse(&data) else {
            continue;
        };
        if g.items.is_empty() {
            continue;
        }
        let (role, _) = detect(&name, Content::Gerber(&g));
        // Pair copper by its place in the stack: an X2 "L3" is the third
        // layer, an Altium ".G2" the second inner one (also the third).
        let label = match &role {
            trace_gerber::detect::Role::Copper { pos, number, .. } => match pos {
                trace_gerber::detect::CuPos::Top => "Copper L01 top".into(),
                trace_gerber::detect::CuPos::Bottom => "Copper L99 bottom".into(),
                trace_gerber::detect::CuPos::Inner => {
                    let n = number.unwrap_or(0) + g.file_function.is_none() as u32;
                    format!("Copper L{n:02}")
                }
            },
            r => r.label(),
        };
        if matches!(
            role,
            trace_gerber::detect::Role::Unknown
                | trace_gerber::detect::Role::Ignore
                | trace_gerber::detect::Role::DrillDrawing
        ) {
            continue;
        }
        layers.insert(label, g);
    }
    Set { layers, holes }
}

struct Bitmap {
    w: usize,
    h: usize,
    x0: f64,
    y0: f64,
    px: Vec<u8>,
}

impl Bitmap {
    fn fill(&mut self, poly: &[Pt], dark: bool, d: Pt) {
        if poly.len() < 3 {
            return;
        }
        let ys = poly.iter().map(|p| p[1] + d[1]);
        let (lo, hi) = ys.fold((f64::MAX, f64::MIN), |a, y| (a.0.min(y), a.1.max(y)));
        let r0 = (((lo - self.y0) / RES).floor().max(0.0)) as usize;
        let r1 = (((hi - self.y0) / RES).ceil().max(0.0) as usize).min(self.h);
        let mut xs = Vec::new();
        for r in r0..r1 {
            let y = self.y0 + (r as f64 + 0.5) * RES;
            xs.clear();
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                let (ay, by) = (a[1] + d[1], b[1] + d[1]);
                if (ay > y) != (by > y) {
                    let t = (y - ay) / (by - ay);
                    xs.push(a[0] + d[0] + t * (b[0] - a[0]));
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pair in xs.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                let c0 = (((pair[0] - self.x0) / RES - 0.5).ceil().max(0.0)) as usize;
                let c1 = ((((pair[1] - self.x0) / RES - 0.5).floor() + 1.0).max(0.0) as usize)
                    .min(self.w);
                for c in c0..c1 {
                    self.px[r * self.w + c] = dark as u8;
                }
            }
        }
    }
}

fn draw(g: &Gerber, bm: &mut Bitmap, d: Pt) {
    for it in &g.items {
        match &it.obj {
            Obj::Flash { ap, at, xf } => {
                for (on, poly) in geom::flash_shapes(g.aperture(*ap), *at, xf) {
                    bm.fill(&poly, on == it.dark, d);
                }
            }
            Obj::Line { ap, a, b } => {
                bm.fill(&geom::line_shape(g.aperture(*ap), *a, *b), it.dark, d)
            }
            Obj::Arc { ap, a, b, c, cw } => {
                let pts = geom::arc_points(*c, *a, *b, *cw, a == b);
                let w = geom::aperture_width(g.aperture(*ap));
                for s in geom::stroke_path(&pts, w) {
                    bm.fill(&s, it.dark, d);
                }
                for p in [a, b] {
                    bm.fill(&geom::circle(*p, w), it.dark, d);
                }
            }
            Obj::Region(contours) => {
                for c in contours {
                    bm.fill(c, it.dark, d);
                }
            }
        }
    }
}

fn bbox(g: &Gerber) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for it in &g.items {
        let pts: Vec<Pt> = match &it.obj {
            Obj::Flash { at, .. } => vec![*at],
            Obj::Line { a, b, .. } | Obj::Arc { a, b, .. } => vec![*a, *b],
            Obj::Region(c) => c.iter().flatten().copied().collect(),
        };
        for p in pts {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        }
    }
    b
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (a, b) = (load(Path::new(&args[0])), load(Path::new(&args[1])));
    let png = args.get(2);
    // Only the board itself counts: title blocks and notes outside it are
    // left out.
    let area = a
        .layers
        .get("Board outline")
        .map(bbox)
        .map(|b| [b[0] - 0.5, b[1] - 0.5, b[2] + 0.5, b[3] + 0.5]);
    // Line up the plated holes.
    let centre = |s: &Set| {
        let h: Vec<Pt> = s
            .holes
            .iter()
            .filter(|h| h.2 != Some(false))
            .map(|h| h.0)
            .collect();
        let bb = geom::bbox(&h);
        [(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0]
    };
    let (ca, cb) = (centre(&a), centre(&b));
    let d = [ca[0] - cb[0], ca[1] - cb[1]];
    println!("shift of the second set: {:.4}, {:.4} mm", d[0], d[1]);
    println!(
        "{:<26} {:>12} {:>12} {:>12} {:>12} {:>8}",
        "layer", "ours mm²", "theirs mm²", "only ours", "only theirs", "diff %"
    );
    for (label, ga) in &a.layers {
        let Some(gb) = b.layers.get(label) else {
            println!("{label:<26} only in the first set");
            continue;
        };
        let (ba, bb) = (bbox(ga), bbox(gb));
        let bb = [bb[0] + d[0], bb[1] + d[1], bb[2] + d[0], bb[3] + d[1]];
        let all = [
            ba[0].min(bb[0]) - 3.0,
            ba[1].min(bb[1]) - 3.0,
            ba[2].max(bb[2]) + 3.0,
            ba[3].max(bb[3]) + 3.0,
        ];
        let r = area.unwrap_or(all);
        let (x0, y0) = (r[0], r[1]);
        let w = ((r[2] - x0) / RES) as usize;
        let h = ((r[3] - y0) / RES) as usize;
        let new = || Bitmap {
            w,
            h,
            x0,
            y0,
            px: vec![0; w * h],
        };
        let (mut ma, mut mb) = (new(), new());
        draw(ga, &mut ma, [0.0, 0.0]);
        draw(gb, &mut mb, d);
        let px = RES * RES;
        let (mut na, mut nb, mut oa, mut ob) = (0usize, 0usize, 0usize, 0usize);
        for (p, q) in ma.px.iter().zip(&mb.px) {
            na += *p as usize;
            nb += *q as usize;
            oa += (*p == 1 && *q == 0) as usize;
            ob += (*p == 0 && *q == 1) as usize;
        }
        let pct = 100.0 * (oa + ob) as f64 / na.max(nb).max(1) as f64;
        println!(
            "{label:<26} {:>12.1} {:>12.1} {:>12.2} {:>12.2} {:>8.2}",
            na as f64 * px,
            nb as f64 * px,
            oa as f64 * px,
            ob as f64 * px,
            pct
        );
        if let Some(dir) = png {
            // Red: only ours. Blue: only theirs. Grey: both.
            let name = label.replace([' ', ','], "_");
            let mut out = format!("P6\n{} {}\n255\n", w / 4, h / 4).into_bytes();
            for r in (0..h / 4).rev() {
                for c in 0..w / 4 {
                    let (mut sa, mut sb) = (0, 0);
                    for dr in 0..4 {
                        for dc in 0..4 {
                            let i = (r * 4 + dr) * w + c * 4 + dc;
                            sa += ma.px[i];
                            sb += mb.px[i];
                        }
                    }
                    let px = match (sa > 0, sb > 0) {
                        (true, true) => [150, 150, 150],
                        (true, false) => [255, 0, 0],
                        (false, true) => [0, 90, 255],
                        _ => [255, 255, 255],
                    };
                    out.extend_from_slice(&px);
                }
            }
            std::fs::write(format!("{dir}/{name}.ppm"), out).unwrap();
        }
    }
    for label in b.layers.keys().filter(|l| !a.layers.contains_key(*l)) {
        println!("{label:<26} only in the second set");
    }
    let count = |s: &Set, plated: bool| {
        let mut m: BTreeMap<i64, usize> = BTreeMap::new();
        for h in s.holes.iter().filter(|h| (h.2 != Some(false)) == plated) {
            *m.entry((h.1 * 1000.0).round() as i64).or_default() += 1;
        }
        m
    };
    for plated in [true, false] {
        println!(
            "{} holes (µm: count)\n  ours:   {:?}\n  theirs: {:?}",
            if plated { "plated" } else { "non-plated" },
            count(&a, plated),
            count(&b, plated)
        );
    }
}
