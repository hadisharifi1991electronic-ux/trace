//! Tries schematic edits on a sheet and its board and prints what Update
//! PCB would change.
//!
//!     cargo run --release -p trace-altium --example ecotry -- <sheet.SchDoc> <board.PcbDoc>

use std::collections::HashMap;
use trace_core::eco::plan;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let sheet = trace_altium::read_schdoc(std::path::Path::new(&a[0])).unwrap();
    let board = trace_altium::read_pcbdoc(std::path::Path::new(&a[1])).unwrap();
    let d0 = sheet.drawing.clone();
    let t = std::time::Instant::now();
    let p = plan(&[&d0], &[&d0], &HashMap::new(), &board);
    println!(
        "unchanged: {} changes ({} parts, {} nets on sheet, {:.2} s)",
        p.changes.len(),
        d0.parts.len(),
        d0.nets.len(),
        t.elapsed().as_secs_f64()
    );
    let nl = trace_core::eco::netlist(&[&d0]);
    println!("project nets: {}", nl.len());

    // Rename the first net label, delete the first wire on a net with a
    // label, move and turn the first part, rename a part.
    let mut e = sheet.clone();
    if let Some(t) = e
        .drawing
        .tags
        .iter()
        .position(|t| t.kind == trace_core::TagKind::Label)
    {
        let old = e.drawing.tags[t].text.clone();
        e.drawing.rename_tag(t, "RENAMED_NET");
        println!("label {old} -> RENAMED_NET");
    }
    if let Some(w) = (0..e.drawing.wires.len()).find(|&w| {
        e.drawing
            .nets
            .iter()
            .any(|n| n.wires.contains(&(w as u32)) && n.pins.len() >= 3)
    }) {
        println!("deleting wire {w}: {:?}", e.drawing.wires[w]);
        e.drawing.delete_wire(w);
    }
    let name = e.drawing.parts[0].designator.clone();
    e.drawing.move_part(0, [500.0, 0.0]);
    e.drawing.turn_part(0);
    e.drawing.rename_part(0, "ZZ99");
    let renames: HashMap<String, String> = [("ZZ99".to_string(), name.to_ascii_uppercase())].into();
    let p = plan(&[&d0], &[&e.drawing], &renames, &board);
    for c in &p.changes {
        println!("  {}", c.describe());
    }
    let mut b = board.clone();
    if !p.edits.is_empty() {
        p.edits.iter().for_each(|e| e.apply(&mut b));
        trace_core::connectivity::analyse(&mut b);
        let p2 = plan(&[&e.drawing], &[&e.drawing], &HashMap::new(), &b);
        println!("after applying, plan again: {} changes", p2.changes.len());
    }

    // Save and read back.
    let f = trace_core::SchFile {
        version: "test".into(),
        project: None,
        sheets: vec![trace_core::SavedSheet {
            file: "Clocker.SchDoc".into(),
            sheet: e.clone(),
            synced: Some(d0.clone()),
            original: None,
        }],
        ..Default::default()
    };
    let path = std::env::temp_dir().join("ecotry.tracesch");
    f.save(&path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    let back = trace_core::SchFile::load(&path).unwrap();
    let s = &back.sheets[0].sheet.drawing;
    println!(
        "saved {} kB; read back {} parts, {} wires, {} images ({} bytes)",
        size / 1024,
        s.parts.len(),
        s.wires.len(),
        s.images.len(),
        s.images.iter().map(|i| i.data.len()).sum::<usize>()
    );
    assert_eq!(
        s.images.iter().map(|i| i.data.len()).sum::<usize>(),
        e.drawing.images.iter().map(|i| i.data.len()).sum::<usize>()
    );
}
