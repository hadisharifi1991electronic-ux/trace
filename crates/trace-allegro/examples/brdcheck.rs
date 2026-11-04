//! Sanity checks of geometry conventions against stored bounding boxes.
use trace_allegro::blocks::{Data, Db};
fn main() {
    for p in std::env::args().skip(1) {
        let data = std::fs::read(&p).unwrap();
        let db = Db::parse(&data).unwrap();
        let (mut ok, mut bad, mut okb, mut badb) = (0, 0, 0, 0);
        let mut worst = String::new();
        for b in &db.blocks {
            let Data::FpInst {
                bottom,
                rotation,
                x,
                y,
                first_pad,
                ..
            } = b.data
            else {
                continue;
            };
            let r = (rotation as f64 / 1000.0).to_radians();
            let (s, c) = r.sin_cos();
            for pp in db.walk_with(first_pad, b.key, |b| match b.data {
                Data::PlacedPad { next_in_fp, .. } => next_in_fp,
                _ => 0,
            }) {
                let Data::PlacedPad { pad, bbox, .. } = pp.data else {
                    continue;
                };
                let Some(Data::Pad { x: px, y: py, .. }) = db.get(pad).map(|b| &b.data) else {
                    continue;
                };
                let lx = if bottom { -*px as f64 } else { *px as f64 };
                let ly = *py as f64;
                let (bx, by) = (x as f64 + lx * c - ly * s, y as f64 + lx * s + ly * c);
                let (cx, cy) = (
                    (bbox[0] + bbox[2]) as f64 / 2.0,
                    (bbox[1] + bbox[3]) as f64 / 2.0,
                );
                let size = ((bbox[2] - bbox[0]).abs().max((bbox[3] - bbox[1]).abs())) as f64;
                let d = (bx - cx).hypot(by - cy);
                let good = d <= size / 2.0 + 1.0;
                if bottom {
                    if good {
                        okb += 1
                    } else {
                        badb += 1
                    }
                } else if good {
                    ok += 1
                } else {
                    bad += 1;
                    if worst.is_empty() {
                        worst = format!("rot {} pad ({px},{py}) fp ({x},{y}) got ({bx:.0},{by:.0}) bbox {bbox:?}", rotation)
                    }
                }
            }
        }
        // Arc direction check.
        let (mut aok, mut abad) = (0, 0);
        for b in &db.blocks {
            let Data::Arc {
                cw,
                start,
                end,
                center,
                radius,
                bbox,
                ..
            } = b.data
            else {
                continue;
            };
            if start == end || !radius.is_finite() || radius <= 0.0 {
                continue;
            }
            let a0 = (start.1 as f64 - center.1).atan2(start.0 as f64 - center.0);
            let a1 = (end.1 as f64 - center.1).atan2(end.0 as f64 - center.0);
            let tau = std::f64::consts::TAU;
            let sweep = if cw {
                (a0 - a1).rem_euclid(tau)
            } else {
                (a1 - a0).rem_euclid(tau)
            };
            let mid = if cw {
                a0 - sweep / 2.0
            } else {
                a0 + sweep / 2.0
            };
            let (mx, my) = (center.0 + radius * mid.cos(), center.1 + radius * mid.sin());
            let (x0, y0, x1, y1) = (
                bbox[0].min(bbox[2]) as f64,
                bbox[1].min(bbox[3]) as f64,
                bbox[0].max(bbox[2]) as f64,
                bbox[1].max(bbox[3]) as f64,
            );
            let slack = 2.0 + radius * 0.01;
            if mx >= x0 - slack && mx <= x1 + slack && my >= y0 - slack && my <= y1 + slack {
                aok += 1
            } else {
                abad += 1
            }
        }
        println!("{}: pads top {ok} ok {bad} off, bottom {okb} ok {badb} off; arcs {aok} ok {abad} off\n   {worst}", p.rsplit('/').next().unwrap());
    }
}
