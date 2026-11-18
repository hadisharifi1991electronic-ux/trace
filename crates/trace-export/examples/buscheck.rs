//! Draws a sheet with a bus breakout, a harness connector, a signal
//! harness and ports, saves it as an Altium .SchDoc, reads it back and
//! checks the nets, then writes the sheet as SVG.
//!
//! cargo run -p trace-export --example buscheck -- out_dir

use trace_core::sch_edit::Xf;
use trace_core::sch_select::SchSel;
use trace_core::{Harness, Sheet};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Sheet {
        name: "bus".into(),
        ..Default::default()
    };
    let d = &mut s.drawing;
    d.size = [6000.0, 4000.0];
    d.paper = [255, 252, 240];
    // A bus with eight members broken out, and a port carrying it away.
    let b = d.add_bus(vec![[500.0, 3000.0], [2000.0, 3000.0]]).unwrap();
    let n = d.bus_breakout(b, "D[0..7]", [600.0, 3000.0], 100.0, 400.0, true);
    let g = trace_altium::port_drawing([2000.0, 3000.0], "D[0..7]", 500.0);
    d.add_drawn_tag(&g);
    // A harness connector, its entries wired, a signal harness to a port.
    let h = d.add_harness(Harness::new(
        [3500.0, 3000.0],
        "SPI",
        &["MOSI", "MISO", "SCK", "CS"],
        true,
    ));
    let c = d.harnesses[h].clone();
    for e in &c.entries {
        let at = c.entry_at(e);
        d.add_wire(vec![[at[0] - 500.0, at[1]], at]);
    }
    let start = c.bundle_at();
    d.add_signal_harness(vec![
        start,
        [start[0] + 600.0, start[1]],
        [start[0] + 600.0, 2000.0],
    ]);
    let g = trace_altium::port_drawing([start[0] + 600.0, 2000.0], "SPI1", 500.0);
    d.add_drawn_tag(&g);
    // Moved, the wires follow.
    let sel = SchSel {
        harnesses: vec![h],
        ..Default::default()
    };
    d.xf_sel(&sel, Xf::Shift([200.0, 0.0]));
    let names = |s: &Sheet| {
        let mut v: Vec<String> = s
            .drawing
            .nets
            .iter()
            .filter_map(|n| n.name.clone())
            .collect();
        v.sort();
        v
    };
    println!("broke out {n} members; nets: {:?}", names(&s));
    let path = dir.join("bus-harness.SchDoc");
    std::fs::write(&path, trace_altium::write_schdoc(&s, None).unwrap()).unwrap();
    let back = trace_altium::read_schdoc(&path).unwrap();
    assert_eq!(names(&back), names(&s), "the nets read back the same");
    assert_eq!(back.drawing.harnesses, s.drawing.harnesses);
    assert_eq!(back.drawing.bus_entries, s.drawing.bus_entries);
    println!(
        "read back: {} buses, {} entries, {} harnesses, {} signal harnesses",
        back.drawing.buses.len(),
        back.drawing.bus_entries.len(),
        back.drawing.harnesses.len(),
        back.drawing.signal_harnesses.len()
    );
    std::fs::write(
        dir.join("bus-harness.svg"),
        trace_export::svg::sheet_svg(&back),
    )
    .unwrap();
}
