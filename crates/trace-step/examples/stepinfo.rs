//! `stepinfo <file.step>...`: triangles, colours and size of each model.
fn main() {
    for p in std::env::args().skip(1) {
        let t = std::time::Instant::now();
        let data = std::fs::read(&p).unwrap();
        match trace_step::mesh(&data) {
            Ok(m) => {
                let (lo, hi) = m.bounds().unwrap();
                let mut cols: Vec<[u8; 3]> = m.colors.clone();
                cols.sort();
                cols.dedup();
                println!(
                    "{p}: {} tris, {} colours, size {:.2} x {:.2} x {:.2} mm, z {:.2}..{:.2}, {:?}",
                    m.indices.len() / 3,
                    cols.len(),
                    hi[0] - lo[0],
                    hi[1] - lo[1],
                    hi[2] - lo[2],
                    lo[2],
                    hi[2],
                    t.elapsed()
                );
            }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
