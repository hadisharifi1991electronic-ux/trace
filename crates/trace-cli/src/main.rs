//! `trace`: command-line front end until the editor exists.

use std::path::Path;
use std::process::ExitCode;
use trace_core::{Board, LayerKind, Length, Sheet};

const USAGE: &str = "\
trace: read Altium designs with the Trace engine

USAGE:
    trace inspect <file.PcbDoc|file.SchDoc>     Summary of a board or sheet
    trace pairs <file.PcbDoc> [max-skew]        Differential pair lengths and skew (default max-skew 5mil)
    trace rules <file.PcbDoc>                   Design rules, grouped by kind
    trace convert <file> <out.json>             Export to Trace's open JSON model
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["inspect", file] => inspect(file),
        ["pairs", file] => pairs(file, "5mil"),
        ["pairs", file, skew] => pairs(file, skew),
        ["rules", file] => rules(file),
        ["convert", file, out] => convert(file, out),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<(), Box<dyn std::error::Error>>;

enum Design {
    Board(Board),
    Sheet(Sheet),
}

fn load(file: &str) -> Result<Design, Box<dyn std::error::Error>> {
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("pcbdoc") => Ok(Design::Board(trace_altium::read_pcbdoc(file)?)),
        Some("schdoc") => Ok(Design::Sheet(trace_altium::read_schdoc(file)?)),
        _ => Err(format!("{file}: expected a .PcbDoc or .SchDoc file").into()),
    }
}

fn board(file: &str) -> Result<Board, Box<dyn std::error::Error>> {
    match load(file)? {
        Design::Board(b) => Ok(b),
        Design::Sheet(_) => Err("this command needs a .PcbDoc file".into()),
    }
}

fn inspect(file: &str) -> Res {
    match load(file)? {
        Design::Board(b) => print_board(&b),
        Design::Sheet(s) => print_sheet(&s),
    }
    Ok(())
}

fn print_board(b: &Board) {
    let copper = b.stackup.copper_layers().count();
    println!(
        "Board: {copper} copper layers, {:.2} mm thick",
        b.stackup.total_thickness().mm()
    );
    println!();
    println!(
        "  {:<16} {:<11} {:>10} {:>6}",
        "Layer", "Type", "Thickness", "Er"
    );
    for l in &b.stackup.layers {
        if matches!(
            l.kind,
            LayerKind::Silkscreen | LayerKind::Paste | LayerKind::Other
        ) {
            continue;
        }
        let er = l
            .dielectric_constant
            .map(|e| format!("{e:.2}"))
            .unwrap_or_default();
        println!(
            "  {:<16} {:<11} {:>7.1} um {:>6}",
            l.name,
            format!("{:?}", l.kind),
            l.thickness.0 as f64 / 1000.0,
            er
        );
    }
    let top = b
        .components
        .iter()
        .filter(|c| c.side == trace_core::Side::Top)
        .count();
    println!();
    println!(
        "Components: {} ({} top, {} bottom)",
        b.components.len(),
        top,
        b.components.len() - top
    );
    println!(
        "Nets: {} ({} routed)",
        b.nets.len(),
        b.nets.iter().filter(|n| n.routed_length.0 > 0).count()
    );
    println!(
        "Net classes: {}   Differential pairs: {} in {} classes",
        b.net_classes.len(),
        b.diff_pairs.len(),
        b.diff_pair_classes.len()
    );
    println!("Rules: {}", b.rules.len());
    println!(
        "Primitives: {} tracks, {} vias, {} pads, {} polygons",
        b.stats.tracks, b.stats.vias, b.stats.pads, b.stats.polygons
    );
}

fn print_sheet(s: &Sheet) {
    println!("Sheet: {}", s.name);
    println!(
        "Symbols: {}   Wires: {}   Junctions: {}",
        s.symbols.len(),
        s.wires,
        s.junctions
    );
    println!(
        "Net labels: {}   Power ports: {}   Ports: {}",
        s.net_labels.len(),
        s.power_ports.len(),
        s.ports.len()
    );
    if !s.sheet_refs.is_empty() {
        println!("Child sheets: {}", s.sheet_refs.join(", "));
    }
    let mut symbols: Vec<_> = s
        .symbols
        .iter()
        .filter(|x| !x.designator.is_empty())
        .collect();
    symbols.sort_by_key(|x| natural(&x.designator));
    if !symbols.is_empty() {
        println!();
        println!(
            "  {:<10} {:<28} {:<16} {:>5}",
            "Ref", "Part", "Value", "Pins"
        );
        for x in symbols.iter().take(40) {
            println!(
                "  {:<10} {:<28} {:<16} {:>5}",
                x.designator,
                clip(&x.library_ref, 28),
                clip(x.value.as_deref().unwrap_or(""), 16),
                x.pins
            );
        }
        if symbols.len() > 40 {
            println!("  ... and {} more", symbols.len() - 40);
        }
    }
}

fn pairs(file: &str, max_skew: &str) -> Res {
    let b = board(file)?;
    let limit = Length::parse(max_skew).ok_or("max-skew must look like 5mil or 0.1mm")?;
    let mut rows: Vec<_> = b
        .diff_pairs
        .iter()
        .filter_map(|p| {
            let (pos, neg) = (b.net(&p.positive)?, b.net(&p.negative)?);
            let skew = Length((pos.routed_length.0 - neg.routed_length.0).abs());
            Some((p, pos, neg, skew))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.3));
    println!(
        "{:<34} {:>10} {:>10} {:>9}",
        "Pair", "P (mil)", "N (mil)", "Skew"
    );
    let mut over = 0;
    let mut unrouted = 0;
    for (p, pos, neg, skew) in &rows {
        let flag = if pos.routed_length.0 == 0 || neg.routed_length.0 == 0 {
            unrouted += 1;
            "unrouted"
        } else if *skew > limit {
            over += 1;
            "OVER"
        } else {
            ""
        };
        println!(
            "{:<34} {:>10.1} {:>10.1} {:>9.1}  {flag}",
            clip(&p.name, 34),
            pos.routed_length.mil(),
            neg.routed_length.mil(),
            skew.mil()
        );
    }
    println!();
    println!(
        "{} pairs: {} over {:.1} mil skew, {} not fully routed",
        rows.len(),
        over,
        limit.mil(),
        unrouted
    );
    Ok(())
}

fn rules(file: &str) -> Res {
    let b = board(file)?;
    let mut by_kind: std::collections::BTreeMap<&str, Vec<&trace_core::Rule>> = Default::default();
    for r in &b.rules {
        by_kind.entry(&r.kind).or_default().push(r);
    }
    for (kind, rs) in by_kind {
        println!("{kind} ({})", rs.len());
        for r in rs {
            let state = if r.enabled { "" } else { "  [disabled]" };
            println!(
                "  {:<36} {}{state}",
                clip(&r.name, 36),
                r.scope.first().map(String::as_str).unwrap_or("")
            );
        }
    }
    Ok(())
}

fn convert(file: &str, out: &str) -> Res {
    let json = match load(file)? {
        Design::Board(b) => serde_json::to_string_pretty(&b)?,
        Design::Sheet(s) => serde_json::to_string_pretty(&s)?,
    };
    std::fs::write(out, json)?;
    println!("wrote {out}");
    Ok(())
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "~"
    }
}

/// Sort key so R2 comes before R10.
fn natural(s: &str) -> (String, u64) {
    let split = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    (s[..split].to_string(), s[split..].parse().unwrap_or(0))
}
