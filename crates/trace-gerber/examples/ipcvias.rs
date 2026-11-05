//! `ipcvias <path> <ipc>`: vias the import has that the IPC-356 netlist lacks, and the reverse.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let b = trace_gerber::read_path(&a[0]).unwrap().board;
    let n = trace_gerber::ipc356::parse(&std::fs::read(&a[1]).unwrap());
    let key = |x: f64, y: f64| ((x * 20.0).round() as i64, (y * 20.0).round() as i64);
    let ipc: std::collections::HashSet<(i64, i64)> = n
        .points
        .iter()
        .filter(|p| p.via)
        .map(|p| key(p.at[0], p.at[1]))
        .collect();
    let ours: std::collections::HashSet<(i64, i64)> = b
        .geometry
        .vias
        .iter()
        .map(|v| key(v.at.x.mm(), v.at.y.mm()))
        .collect();
    let near = |s: &std::collections::HashSet<(i64, i64)>, k: (i64, i64)| {
        (-1..=1).any(|dx| (-1..=1).any(|dy| s.contains(&(k.0 + dx, k.1 + dy))))
    };
    let extra: Vec<_> = b
        .geometry
        .vias
        .iter()
        .filter(|v| !near(&ipc, key(v.at.x.mm(), v.at.y.mm())))
        .collect();
    println!(
        "ours {} (distinct {}), ipc via points {} (distinct {})",
        b.geometry.vias.len(),
        ours.len(),
        n.points.iter().filter(|p| p.via).count(),
        ipc.len()
    );
    println!("ours not in ipc: {}", extra.len());
    for v in extra.iter().take(8) {
        println!(
            "  ({:.3},{:.3}) {}-{} d {:.3}",
            v.at.x.mm(),
            v.at.y.mm(),
            v.from_layer + 1,
            v.to_layer + 1,
            v.diameter.mm()
        );
    }
    let missing = ipc.iter().filter(|k| !near(&ours, **k)).count();
    println!("ipc not in ours: {missing}");
}
