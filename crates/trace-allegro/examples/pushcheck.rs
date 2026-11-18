//! Push-and-shove on a real board: for tracks spread over the board, a new
//! route is laid alongside each, too close, and pushes it (and whatever
//! that runs into) aside. Prints how many pushes worked, how long they
//! took, how far the chains went, and checks every moved track and via
//! against the rest of the board afterwards.
//!
//! cargo run --release -p trace-allegro --example pushcheck -- board.brd [count]
use trace_core::push::{push, Pusher};
use trace_core::route::{self, Obstacles};
use trace_core::LayerRef;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("board.brd");
    let count: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(200);
    let board = trace_allegro::read_brd(&path).expect("read");
    let clearance = route::board_clearance(&board);
    let obs = Obstacles::new(&board);
    let g = &board.geometry;
    let p = |q: trace_core::Point| [q.x.mil(), q.y.mil()];
    let cands: Vec<usize> = (0..g.tracks.len())
        .filter(|&i| {
            let t = &g.tracks[i];
            matches!(t.layer, LayerRef::Copper(_))
                && t.net.is_some()
                && (p(t.start)[0] - p(t.end)[0]).hypot(p(t.start)[1] - p(t.end)[1]) > 120.0
        })
        .collect();
    println!(
        "{path}: {} tracks, {} vias, clearance {clearance:.1} mil, {} candidates",
        g.tracks.len(),
        g.vias.len(),
        cands.len()
    );
    let (mut ok, mut blocked, mut bad, mut chained, mut via_moves, mut pads) = (0, 0, 0, 0, 0, 0);
    let mut times = Vec::new();
    let step = (cands.len() / count.max(1)).max(1);
    for &ti in cands.iter().step_by(step).take(count) {
        let t = &g.tracks[ti];
        let LayerRef::Copper(layer) = t.layer else {
            continue;
        };
        let (a, b) = (p(t.start), p(t.end));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
        let w = route::board_width(&board);
        // Alongside the middle third, half a clearance too close.
        let off = t.width.mil() / 2.0 + w / 2.0 + clearance / 2.0;
        let n = [-uy * off, ux * off];
        let s = [a[0] + ux * len / 3.0 + n[0], a[1] + uy * len / 3.0 + n[1]];
        let e = [
            a[0] + ux * len * 2.0 / 3.0 + n[0],
            a[1] + uy * len * 2.0 / 3.0 + n[1],
        ];
        // A net no track has: the route is new copper.
        let net = Some(u32::MAX - 1);
        let route = [Pusher::track(layer, s, e, w / 2.0, &[net])];
        let mut on_pad = false;
        obs.hits(
            Some(layer),
            s,
            e,
            w / 2.0,
            &[net],
            clearance,
            &mut |i, _, _| {
                if i == trace_core::route::Item::Fixed {
                    on_pad = true;
                }
            },
        );
        if on_pad {
            pads += 1;
            continue;
        }
        let t0 = std::time::Instant::now();
        let r = push(&board, &obs, clearance, &route, &[]);
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        if ms > 300.0 {
            eprintln!(
                "  track {ti}: {ms:.0} ms ({})",
                if r.is_some() { "pushed" } else { "no room" }
            );
        }
        times.push(ms);
        let Some(r) = r else {
            blocked += 1;
            continue;
        };
        ok += 1;
        let lines = r.remove.len();
        if r.add
            .iter()
            .map(|t| t.net)
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1
        {
            chained += 1;
        }
        via_moves += r.vias.len();
        // Check: the moved copper on the new board, against everything.
        let mut nb = board.clone();
        nb.geometry.tracks.push(trace_core::Track {
            component: None,
            layer: LayerRef::Copper(layer),
            net,
            start: trace_core::Point {
                x: trace_core::Length::from_mil(s[0]),
                y: trace_core::Length::from_mil(s[1]),
            },
            end: trace_core::Point {
                x: trace_core::Length::from_mil(e[0]),
                y: trace_core::Length::from_mil(e[1]),
            },
            width: trace_core::Length::from_mil(w),
            text: false,
        });
        let before = board.geometry.tracks.len() + 1;
        for ed in r.edits(&board, before) {
            ed.apply(&mut nb);
        }
        let nobs = Obstacles::new(&nb);
        let orig: std::collections::HashSet<_> = board
            .geometry
            .tracks
            .iter()
            .map(|t| {
                (
                    t.start.x.0,
                    t.start.y.0,
                    t.end.x.0,
                    t.end.y.0,
                    format!("{:?}", t.layer),
                )
            })
            .collect();
        let mut broke = false;
        for t in &nb.geometry.tracks {
            if orig.contains(&(
                t.start.x.0,
                t.start.y.0,
                t.end.x.0,
                t.end.y.0,
                format!("{:?}", t.layer),
            )) {
                continue;
            }
            // A piece of a board track left where it was (split where the
            // moved stretch starts): any tightness there was drawn so.
            let on = |q: [f64; 2], u: &trace_core::Track| {
                let (c, d) = (p(u.start), p(u.end));
                let (dx, dy) = (d[0] - c[0], d[1] - c[1]);
                let l = dx * dx + dy * dy;
                let k = if l > 0.0 {
                    (((q[0] - c[0]) * dx + (q[1] - c[1]) * dy) / l).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (c[0] + k * dx - q[0]).hypot(c[1] + k * dy - q[1]) < 1e-3
            };
            if board.geometry.tracks.iter().any(|u| {
                u.layer == t.layer && u.net == t.net && on(p(t.start), u) && on(p(t.end), u)
            }) {
                continue;
            }
            let LayerRef::Copper(l) = t.layer else {
                continue;
            };
            if let Some(h) =
                nobs.check_track(l, p(t.start), p(t.end), t.width.mil(), t.net, clearance)
            {
                let pair = board.diff_pairs.iter().any(|d| {
                    h.net
                        .and_then(|n| board.nets.get(n as usize))
                        .is_some_and(|n| n.name == d.positive || n.name == d.negative)
                });
                if !pair {
                    broke = true;
                    eprintln!(
                        "  track {ti}: moved copper {:.2} mil from net {:?}",
                        h.gap, h.net
                    );
                }
            }
        }
        for &(vi, _) in &r.vias {
            let v = &nb.geometry.vias[vi];
            if let Some(h) = nobs.check_via(p(v.at), v.diameter.mil(), v.net, clearance) {
                broke = true;
                eprintln!(
                    "  track {ti}: moved via {vi} {:.2} mil from net {:?}",
                    h.gap, h.net
                );
            }
        }
        if broke {
            bad += 1;
        }
        let _ = lines;
    }
    times.sort_by(f64::total_cmp);
    let avg = times.iter().sum::<f64>() / times.len().max(1) as f64;
    println!(
        "{pads} routes ran onto a pad themselves (skipped); pushes: {ok} worked, {blocked} found no room, {bad} left a clearance break; \
         {chained} moved more than one net, {via_moves} vias moved"
    );
    println!(
        "time per push: mean {avg:.1} ms, median {:.1} ms, worst {:.1} ms",
        times.get(times.len() / 2).copied().unwrap_or(0.0),
        times.last().copied().unwrap_or(0.0)
    );
}
