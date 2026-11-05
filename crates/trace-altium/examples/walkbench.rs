//! Times walkaround searches between random pads of a board.
use trace_core::route::Obstacles;
use trace_core::walk::{walk_toward, Ask};
use trace_core::*;

fn main() {
    let path = std::env::args().nth(1).expect("board");
    let board = trace_altium::read_pcbdoc(&path).unwrap();
    let t = std::time::Instant::now();
    let obs = Obstacles::new(&board);
    println!("obstacles {:?}", t.elapsed());
    let pads: Vec<&Pad> = board
        .geometry
        .pads
        .iter()
        .filter(|p| p.net.is_some())
        .collect();
    let mut seed = 12345u64;
    let mut rnd = |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % n
    };
    let (mut ok, mut part, mut none) = (0, 0, 0);
    let mut worst = 0.0f64;
    let t = std::time::Instant::now();
    let n = 200;
    for _ in 0..n {
        let a = pads[rnd(pads.len())];
        let b = pads[rnd(pads.len())];
        let from = [a.at.x.mil(), a.at.y.mil()];
        let mut to = [b.at.x.mil(), b.at.y.mil()];
        // Not too far: an interactive move.
        let d = (to[0] - from[0]).hypot(to[1] - from[1]);
        if d > 400.0 {
            let k = 400.0 / d;
            to = [
                from[0] + (to[0] - from[0]) * k,
                from[1] + (to[1] - from[1]) * k,
            ];
        }
        let ask = Ask {
            obstacles: &obs,
            layer: 0,
            r: 3.0,
            nets: [a.net, None],
            clearance: 5.0,
            extra: &[],
            skip: &[],
        };
        let s = std::time::Instant::now();
        match walk_toward(&ask, from, to) {
            Some((_, true)) => ok += 1,
            Some((_, false)) => part += 1,
            None => none += 1,
        }
        worst = worst.max(s.elapsed().as_secs_f64());
    }
    println!(
        "{n} searches: {ok} reached, {part} partial, {none} none; mean {:.1} ms, worst {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0 / n as f64,
        worst * 1000.0
    );
}
