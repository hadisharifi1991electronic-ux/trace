//! Lists the footprints of an Altium `.PcbLib`.
//!
//!     cargo run --release -p trace-altium --example libdump -- <lib.PcbLib>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let lib = trace_altium::read_pcblib(&a[0]).unwrap();
    for f in &lib.footprints {
        let b = f.bounds();
        println!(
            "{:30} pads {:3} tracks {:3} arcs {:3} regions {:3} texts {:?}  {:.0}x{:.0} mil  h {:?}",
            f.name,
            f.pads.len(),
            f.tracks.len(),
            f.arcs.len(),
            f.regions.len(),
            f.texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            b[2] - b[0],
            b[3] - b[1],
            f.height.map(|h| h.mil())
        );
        if a.len() > 1 {
            layers(f);
            arcs(f);
        }
    }
    println!("{} footprints", lib.footprints.len());
}
#[allow(dead_code)]
fn layers(f: &trace_core::footprint::Footprint) {
    use std::collections::BTreeMap;
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for t in &f.tracks {
        *m.entry(format!("track {:?}", t.layer)).or_default() += 1;
    }
    for t in &f.arcs {
        *m.entry(format!("arc {:?}", t.layer)).or_default() += 1;
    }
    for t in &f.regions {
        *m.entry(format!("region {:?}", t.layer)).or_default() += 1;
    }
    for t in &f.pads {
        *m.entry(format!(
            "pad {:?} {:?} hole {}",
            t.layer,
            t.shape,
            t.hole.mil()
        ))
        .or_default() += 1;
    }
    println!("  {m:?}");
}
#[allow(dead_code)]
fn arcs(f: &trace_core::footprint::Footprint) {
    for a in &f.arcs {
        println!(
            "  arc {:?} c ({:.2},{:.2}) r {:.2} w {:.2} {:.1}..{:.1}",
            a.layer,
            a.center.x.mil(),
            a.center.y.mil(),
            a.radius.mil(),
            a.width.mil(),
            a.start_deg,
            a.end_deg
        );
    }
}
