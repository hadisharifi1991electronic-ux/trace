//! `pcbdoc2kicad <in.PcbDoc> <out.kicad_pcb>`: reads a PcbDoc with Trace's
//! Altium reader, moves it back to its user origin and writes KiCad, for
//! round-trip checks of the Altium writer.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut b = trace_altium::read_pcbdoc(&a[0]).unwrap();
    let (ox, oy) = origin(&a[0]);
    let mv = |p: &mut trace_core::Point| {
        p.x.0 -= ox;
        p.y.0 -= oy;
    };
    let g = &mut b.geometry;
    g.outline.iter_mut().for_each(mv);
    g.tracks.iter_mut().for_each(|t| {
        mv(&mut t.start);
        mv(&mut t.end)
    });
    g.arcs.iter_mut().for_each(|t| mv(&mut t.center));
    g.vias.iter_mut().for_each(|t| mv(&mut t.at));
    g.pads.iter_mut().for_each(|t| mv(&mut t.at));
    g.polygons.iter_mut().for_each(|q| {
        q.outline.iter_mut().for_each(mv);
        q.holes.iter_mut().flatten().for_each(mv)
    });
    // Graphic outline copies on mechanical 1 are not copper; drop them.
    eprintln!(
        "{} tracks {} vias {} pads {} pours",
        g.tracks.len(),
        g.vias.len(),
        g.pads.len(),
        g.polygons.len()
    );
    std::fs::write(&a[1], trace_export::to_kicad(&b)).unwrap();
}

fn origin(path: &str) -> (i64, i64) {
    use std::io::Read;
    let mut f = cfb::open(path).unwrap();
    let mut d = Vec::new();
    f.open_stream("/Board6/Data")
        .unwrap()
        .read_to_end(&mut d)
        .unwrap();
    let s = String::from_utf8_lossy(&d);
    let get = |k: &str| -> i64 {
        let v = s
            .split('|')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
            .unwrap();
        trace_core::Length::parse(v).unwrap().0
    };
    (get("ORIGINX"), get("ORIGINY"))
}
