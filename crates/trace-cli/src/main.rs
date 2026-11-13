//! `trace`: command-line front end until the editor exists.

mod view;

use std::path::Path;
use std::process::ExitCode;
use trace_core::{Board, LayerKind, Length, Sheet};

const USAGE: &str = "\
trace: read Altium, Allegro and Gerber designs with the Trace engine

Boards can be Altium .PcbDoc, Cadence Allegro .brd (16.0 or newer), Trace
.tracepcb, or Gerber and drill files: a folder, a .zip, or one file.

USAGE:
    trace inspect <board|file.SchDoc>           Summary of a board or sheet
    trace pairs <board> [max-skew]              Differential pair lengths and skew (default max-skew 5mil)
    trace rules <board>                         Design rules, grouped by kind
    trace check <board>                         Check length, match, skew and via limits from the board's rules
    trace drc <board> [--all]                   Design rule check: clearance, shorts, width, unrouted nets,
                                                holes, annular rings, mask slivers, silk on pads, silk to silk,
                                                part clearance, board edge, return paths over plane voids
                                                and splits, crosstalk between parallel tracks
    trace impedance <board>                     Stack-up, widths for 50/100 ohm, and the impedance of each pair
    trace panel <board> <cols>x<rows> <out.tracepcb> [--gap 2mm] [--rail 5mm] [--tabs N] [--vscore]
                                                Panel of copies with rails, fiducials, tooling holes, and
                                                tabs with mouse bites (or V-score lines); then fab or odb it
    trace match <board> <net,net,...> [out.tracepcb] [--target L] [--amp L]
                                                Match the nets' lengths (to the longest, or the target) with
                                                meanders where they fit; with out, save the board tuned
    trace sim <board> <U1.pin> <U2.pin> --ibis drv.ibs [--rx-ibis rx.ibs] [--model M] [--rx-model M]
              [--rate 1G] [--bits N] [--corner typ|min|max] [--term 50|50@0.9|100diff] [--single]
              [--df 0.02] [--rough 1um] [--lossless] [--no-xtalk] [out.svg]
                                                Simulate the route from a driver pin to a receiver pin with IBIS
                                                models (a pair when the net has a partner): waveforms and eye,
                                                with track loss and crosstalk from nets beside the route
    trace stubs <board> [out.tracepcb]          Via stubs on nets with a stub rule (default: pair nets, 10 mil);
                                                with out, backdrill those too long and save the board
    trace view <board> [out.html]               Open the board in the viewer (in your browser)
    trace convert <file> <out.json>             Export to Trace's open JSON model
    trace convert <file> <out.kicad_pcb>        Write a KiCad board (KiCad 7 and later)
    trace convert <file> <out.PcbDoc>           Write an Altium Designer board
    trace convert <file.SchDoc> <out.svg>       Draw a schematic sheet as SVG
    trace convert <file.SchDoc> <out.SchDoc>    Write a schematic sheet as an Altium sheet
    trace convert <file.tracesch> <folder>      Write every sheet of a Trace schematic as Altium sheets
    trace fab <board> <out.zip|folder>          Gerber X2, drill, IPC-356, pick-and-place and BOM files
    trace odb <board> <out.tgz|folder>          ODB++ job: layers, drills, parts and nets in one package
    trace layers <gerber folder|zip>            Show which layer each Gerber or drill file was taken as
    trace symbols <lib.SchLib|IntLib> [out.svg] List the symbols of an Altium schematic library;
                                                with out, draw them all as SVG
    trace footprints <lib|board> [out]          List the footprints of a .PcbLib, .IntLib, .tracelib or board;
                                                with out, save them as a Trace library (.tracelib)
                                                or an Altium one (.PcbLib)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version" | "-V" | "version"] => {
            println!("trace {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["inspect", file] => inspect(file),
        ["pairs", file] => pairs(file, "5mil"),
        ["pairs", file, skew] => pairs(file, skew),
        ["rules", file] => rules(file),
        ["check", file] => check(file),
        ["drc", file] => drc(file, 15),
        ["drc", file, "--all"] => drc(file, usize::MAX),
        ["impedance", file] => impedance(file),
        ["panel", file, grid, out, rest @ ..] => panel(file, grid, out, rest),
        ["match", file, nets, rest @ ..] => match_nets(file, nets, rest),
        ["sim", file, from, to, rest @ ..] => sim(file, from, to, rest),
        ["stubs", file] => stubs(file, None),
        ["stubs", file, out] => stubs(file, Some(out)),
        ["convert", file, out] => convert(file, out),
        ["layers", file] => layers(file),
        ["fab", file, out] => fab(file, out),
        ["odb", file, out] => odb(file, out),
        ["symbols", file] => symbols(file, None),
        ["symbols", file, out] => symbols(file, Some(out)),
        ["footprints", file] => footprints(file, None),
        ["footprints", file, out] => footprints(file, Some(out)),
        ["view", file] => view_board(file, None),
        ["view", file, out] => view_board(file, Some(out)),
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

/// Lists the symbols of a `.SchLib`; with `out`, draws them all on one
/// SVG sheet.
fn symbols(file: &str, out: Option<&str>) -> Res {
    let lib = if file.to_ascii_lowercase().ends_with(".intlib") {
        trace_altium::read_intlib(file)?.symbols
    } else {
        trace_altium::read_schlib(file)?
    };
    println!("{}: {} symbols", lib.name, lib.symbols.len());
    for s in &lib.symbols {
        let pins = s.drawing.pins.len();
        println!(
            "  {:<32} {:<5} {:>4} pins  {}  {}",
            s.title(),
            s.symbol.designator,
            pins,
            if s.symbol.footprint.is_empty() {
                "-"
            } else {
                &s.symbol.footprint
            },
            s.symbol.description
        );
    }
    if let Some(out) = out {
        // A grid of symbols, each in a cell as big as the biggest.
        let cell = lib
            .symbols
            .iter()
            .map(|s| s.drawing.size[0].max(s.drawing.size[1]))
            .fold(400.0f64, f64::max)
            + 400.0;
        let cols = (lib.symbols.len() as f64).sqrt().ceil().max(1.0) as usize;
        let rows = lib.symbols.len().div_ceil(cols).max(1);
        let mut sheet = trace_core::Sheet {
            name: lib.name.clone(),
            ..Default::default()
        };
        sheet.drawing.size = [cols as f64 * cell, rows as f64 * cell];
        sheet.drawing.paper = [255, 255, 255];
        for (k, s) in lib.symbols.iter().enumerate() {
            let q = &s.drawing.parts[0];
            let mid = [(q.min[0] + q.max[0]) / 2.0, (q.min[1] + q.max[1]) / 2.0];
            let (c, r) = ((k % cols) as f64, (rows - 1 - k / cols) as f64);
            let at = [(c + 0.5) * cell - mid[0], (r + 0.5) * cell - mid[1]];
            sheet.place_symbol(s, at, &s.symbol.designator);
            sheet.drawing.texts.push(trace_core::SchText {
                at: [(c + 0.5) * cell, r * cell + 100.0],
                text: s.title(),
                size: 80.0,
                color: [0, 0, 0],
                rot: 0,
                just: 1,
                bold: false,
                italic: false,
                wrap: None,
                part: None,
            });
        }
        std::fs::write(out, trace_export::sheet_svg(&sheet))?;
        println!("wrote {out}");
    }
    Ok(())
}

fn footprints(file: &str, out: Option<&str>) -> Res {
    use trace_core::footprint::{Footprint, Library};
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let lib = match ext.as_str() {
        "pcblib" => trace_altium::read_pcblib(file)?,
        "intlib" => trace_altium::read_intlib(file)?.footprints,
        "tracelib" => Library::load(Path::new(file))?,
        _ => {
            let Design::Board(board) = load(file)? else {
                return Err("not a board or footprint library".into());
            };
            let mut seen = std::collections::HashSet::new();
            let mut lib = Library::default();
            for (i, c) in board.components.iter().enumerate() {
                if seen.insert(c.footprint.to_ascii_uppercase()) {
                    lib.footprints.extend(Footprint::from_part(&board, i));
                }
            }
            lib
        }
    };
    for f in &lib.footprints {
        let b = f.bounds();
        println!(
            "{:40} {:4} pads  {:7.2} x {:7.2} mm  {}",
            f.name,
            f.pads.len(),
            (b[2] - b[0]) * 0.0254,
            (b[3] - b[1]) * 0.0254,
            f.description
        );
    }
    println!("{} footprints", lib.footprints.len());
    if let Some(out) = out {
        let mut lib = lib;
        lib.version = env!("CARGO_PKG_VERSION").to_string();
        if out.to_ascii_lowercase().ends_with(".pcblib") {
            let name = Path::new(out)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            std::fs::write(out, trace_export::to_pcblib(&lib, &name)?)?;
        } else {
            lib.save(Path::new(out))?;
        }
        println!("saved {out}");
    }
    Ok(())
}

#[allow(clippy::large_enum_variant)]
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
        Some("brd") => Ok(Design::Board(trace_allegro::read_brd(file)?)),
        Some("tracepcb") => Ok(Design::Board(trace_core::file::load(Path::new(file))?)),
        _ if is_fab(file) => Ok(Design::Board(trace_gerber::read_path(file)?.board)),
        _ => {
            Err(format!("{file}: expected a .PcbDoc, .brd, .SchDoc, Gerber folder or .zip").into())
        }
    }
}

fn is_fab(file: &str) -> bool {
    let p = Path::new(file);
    p.is_dir() || file.to_ascii_lowercase().ends_with(".zip") || trace_gerber::is_fab_file(file)
}

fn layers(file: &str) -> Res {
    let t = std::time::Instant::now();
    let imp = trace_gerber::read_path(file)?;
    println!("{:<52} {:<26} Because of", "File", "Taken as");
    for f in &imp.files {
        let role = match f.copper_index {
            Some(i) => format!("{} (L{})", f.role, i + 1),
            None => f.role.clone(),
        };
        println!("{:<52} {:<26} {}", f.file, role, f.reason);
    }
    let g = &imp.board.geometry;
    println!();
    println!(
        "Board: {} copper layers, {} tracks, {} arcs, {} pads, {} vias, {} pours, outline {} points",
        imp.board.stackup.copper_layers().count(),
        g.tracks.iter().filter(|t| matches!(t.layer, trace_core::LayerRef::Copper(_))).count(),
        g.arcs.iter().filter(|t| matches!(t.layer, trace_core::LayerRef::Copper(_))).count(),
        g.pads.len(),
        g.vias.len(),
        g.polygons.len(),
        g.outline.len()
    );
    println!("Nets: {} ({})", imp.board.nets.len(), imp.nets_from);
    for w in imp.warnings.iter().take(10) {
        println!("warning: {w}");
    }
    println!("Read in {:.2} s", t.elapsed().as_secs_f64());
    Ok(())
}

fn board(file: &str) -> Result<Board, Box<dyn std::error::Error>> {
    match load(file)? {
        Design::Board(b) => Ok(b),
        Design::Sheet(_) => Err("this command needs a board (.PcbDoc or .brd)".into()),
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
    let multi: Vec<_> = b
        .nets
        .iter()
        .filter(|n| n.pads >= 2 && !n.has_pour)
        .collect();
    let open = multi.iter().filter(|n| !n.is_complete()).count();
    println!(
        "Nets: {} ({} join two or more pads: {} fully routed, {} still open; {} use copper pours, not checked yet)",
        b.nets.len(),
        multi.len(),
        multi.len() - open,
        open,
        b.nets.iter().filter(|n| n.has_pour).count()
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
            let skew = Length((pos.total_length().0 - neg.total_length().0).abs());
            let done = pos.is_complete()
                && neg.is_complete()
                && pos.routed_length.0 > 0
                && neg.routed_length.0 > 0;
            Some((p, pos, neg, skew, done))
        })
        .collect();
    // Finished pairs first, worst skew first: those are the ones to fix.
    rows.sort_by_key(|r| (!r.4, std::cmp::Reverse(r.3)));
    println!("Lengths include tracks, arcs and via barrels between the layers used.");
    println!();
    println!(
        "{:<30} {:>9} {:>9} {:>8} {:>7}  Status",
        "Pair", "P (mil)", "N (mil)", "Skew", "Vias"
    );
    let (mut over, mut open) = (0, 0);
    for (p, pos, neg, skew, done) in &rows {
        let status = if !done {
            open += 1;
            match (
                pos.routed_length.0 == 0 && neg.routed_length.0 == 0,
                pos.is_complete(),
                neg.is_complete(),
            ) {
                (true, _, _) => "not routed".to_string(),
                (_, false, false) => "P and N open".into(),
                (_, false, true) => "P open".into(),
                _ => "N open".into(),
            }
        } else if *skew > limit {
            over += 1;
            "OVER".into()
        } else {
            "ok".into()
        };
        println!(
            "{:<30} {:>9.1} {:>9.1} {:>8.1} {:>3}/{:<3}  {status}",
            clip(&p.name, 30),
            pos.total_length().mil(),
            neg.total_length().mil(),
            skew.mil(),
            pos.vias,
            neg.vias
        );
    }
    let done = rows.len() - open;
    println!();
    println!(
        "{} pairs: {} fully routed ({} within {:.1} mil, {} over), {} open or not routed",
        rows.len(),
        done,
        done - over,
        limit.mil(),
        over,
        open
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

fn impedance(file: &str) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("impedance needs a board".into());
    };
    let geo = b.layer_geometry();
    println!(
        "{:<14} {:>6} {:>8} {:>8} {:>5}  {:>9} {:>9} {:>9}",
        "layer", "Cu mil", "above", "below", "Er", "50R w", "100R w/s", "ps/in"
    );
    let f = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".into());
    for g in &geo {
        let w50 = g.width_for(50.0);
        let wd = g.width_for(50.0).map(|w| w * 0.75);
        let s100 = wd.and_then(|w| g.gap_for(w, 100.0));
        println!(
            "{:<14} {:>6.2} {:>8} {:>8} {:>5.2}  {:>9} {:>9} {:>9.0}",
            g.name,
            g.t,
            f(g.above),
            f(g.below),
            g.er,
            f(w50),
            match (wd, s100) {
                (Some(w), Some(s)) => format!("{w:.1}/{s:.1}"),
                _ => "-".into(),
            },
            g.delay_ps_per_in(w50.unwrap_or(5.0))
        );
    }
    let rows = b.net_impedance();
    let mut pairs: Vec<_> = rows.iter().filter(|r| r.zdiff.is_some()).collect();
    pairs.sort_by(|a, b| b.length.total_cmp(&a.length));
    if !pairs.is_empty() {
        println!("\npair nets (longest runs first)");
        println!(
            "{:<28} {:<10} {:>6} {:>6} {:>7} {:>7} {:>8}",
            "net", "layer", "w", "gap", "Z0", "Zdiff", "length"
        );
        for r in pairs.iter().take(40) {
            println!(
                "{:<28} {:<10} {:>6.2} {:>6.2} {:>7.1} {:>7.1} {:>8.0}",
                clip(&b.nets[r.net as usize].name, 28),
                geo.get(r.layer as usize)
                    .map(|g| g.name.as_str())
                    .unwrap_or("?"),
                r.width,
                r.gap.unwrap_or(0.0),
                r.z0,
                r.zdiff.unwrap_or(0.0),
                r.length
            );
        }
    }
    Ok(())
}

fn drc(file: &str, show: usize) -> Res {
    let b = board(file)?;
    let t = std::time::Instant::now();
    let found = trace_core::drc::check(&b, 5000);
    let took = t.elapsed();
    let mut by_kind: std::collections::BTreeMap<String, Vec<&trace_core::drc::Violation>> =
        Default::default();
    for v in &found {
        by_kind.entry(format!("{:?}", v.kind)).or_default().push(v);
    }
    for (kind, list) in &by_kind {
        println!("{kind}: {}", list.len());
        for v in list.iter().take(show) {
            let layer = v
                .layer
                .and_then(|l| {
                    b.stackup
                        .layers
                        .iter()
                        .filter(|x| x.kind == trace_core::LayerKind::Copper)
                        .nth(l as usize)
                })
                .map(|l| l.name.clone())
                .unwrap_or_default();
            println!(
                "  {:>9.1} {:>9.1}  {:<10} {}",
                v.at[0],
                v.at[1],
                layer,
                trace_core::drc::describe(&b, v)
            );
        }
        if list.len() > show {
            println!("  ... {} more (--all lists them)", list.len() - show);
        }
    }
    println!("{} violations in {:.2} s", found.len(), took.as_secs_f64());
    Ok(())
}

fn panel(file: &str, grid: &str, out: &str, opts: &[&str]) -> Res {
    use trace_core::panel::{panelize, PanelSpec, Separation};
    let b = board(file)?;
    let (c, r) = grid
        .to_ascii_lowercase()
        .split_once('x')
        .and_then(|(c, r)| Some((c.trim().parse::<u32>().ok()?, r.trim().parse::<u32>().ok()?)))
        .filter(|&(c, r)| c >= 1 && r >= 1 && c * r <= 400)
        .ok_or("the grid is columns x rows, such as 3x2")?;
    let mut spec = PanelSpec {
        cols: c,
        rows: r,
        ..Default::default()
    };
    let len = |v: Option<&&str>| -> Result<f64, String> {
        v.and_then(|v| Length::parse(v))
            .map(|l| l.mil())
            .ok_or_else(|| "a length such as 2mm or 80mil".to_string())
    };
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            "--gap" => {
                spec.gap = len(opts.get(i + 1))?;
                i += 1;
            }
            "--rail" => {
                spec.rail = len(opts.get(i + 1))?;
                i += 1;
            }
            "--tabs" => {
                spec.tabs = opts
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--tabs takes a number")?;
                i += 1;
            }
            "--vscore" => spec.separation = Separation::VScore,
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let (p, made) = panelize(&b, &spec)?;
    trace_core::file::save(&p, Path::new(out))?;
    println!(
        "{c} x {r} panel, {:.1} x {:.1} mm: {} tabs, {} mouse bite holes{}; saved {out}",
        made.size[0] * 0.0254,
        made.size[1] * 0.0254,
        made.tabs,
        made.bites,
        made.vscore_layer
            .map(|m| format!(", V-score lines on mechanical layer {m}"))
            .unwrap_or_default()
    );
    Ok(())
}

fn stubs(file: &str, out: Option<&str>) -> Res {
    use trace_core::backdrill;
    let mut b = board(file)?;
    let list = backdrill::stubs(&b);
    let ruled: Vec<_> = list.iter().filter(|s| s.limit.is_some()).collect();
    let name = |n: u32| {
        b.nets
            .get(n as usize)
            .map(|n| n.name.as_str())
            .unwrap_or("?")
    };
    let layer = |l: u8| {
        b.stackup
            .copper_layers()
            .nth(l as usize)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("L{}", l + 1))
    };
    println!(
        "{} vias joined by their net, {} on nets with a stub rule",
        list.len(),
        ruled.len()
    );
    let mut long: Vec<_> = ruled
        .iter()
        .filter(|s| {
            let max = s.limit.as_ref().map(|l| l.0).unwrap_or(f64::MAX);
            s.top_left.max(s.bottom_left) > max
        })
        .collect();
    long.sort_by(|a, b| (b.top_left.max(b.bottom_left)).total_cmp(&a.top_left.max(a.bottom_left)));
    println!("{} with a stub longer than allowed:", long.len());
    println!(
        "  {:<28} {:>9} {:>9}  {:<22} {:>7} {:>7}  rule",
        "net", "x", "y", "joins", "above", "below"
    );
    for s in &long {
        let v = &b.geometry.vias[s.via];
        let joins: Vec<String> = s.joins.iter().map(|&l| layer(l)).collect();
        let (max, rule) = s.limit.clone().unwrap_or_default();
        println!(
            "  {:<28} {:>9.1} {:>9.1}  {:<22} {:>7.1} {:>7.1}  {rule} ({max:.1} mil)",
            name(s.net),
            v.at.x.mil(),
            v.at.y.mil(),
            joins.join(", "),
            s.top_left,
            s.bottom_left
        );
    }
    if let Some(out) = out {
        let edits = backdrill::plan(&b);
        for e in &edits {
            e.apply(&mut b);
        }
        let n = b
            .geometry
            .vias
            .iter()
            .filter(|v| v.fab.back_top.is_some() || v.fab.back_bottom.is_some())
            .count();
        // Pours keep clear of the backdrills where they drill through.
        if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
            e.apply(&mut b);
        }
        trace_core::file::save(&b, Path::new(out))?;
        println!(
            "Backdrilled {} vias ({n} in all); saved {out}",
            edits.len() / 2
        );
    }
    Ok(())
}

fn match_nets(file: &str, nets: &str, rest: &[&str]) -> Res {
    let mut b = board(file)?;
    let mut out = None;
    let (mut target, mut amp) = (None, 40.0);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let len = |v: Option<&&str>| -> Result<f64, Box<dyn std::error::Error>> {
            let v = v.ok_or(format!("{a} needs a length"))?;
            Ok(Length::parse(v).ok_or(format!("not a length: {v}"))?.mil())
        };
        match a {
            "--target" => target = Some(len(it.next())?),
            "--amp" => amp = len(it.next())?,
            _ if out.is_none() && !a.starts_with("--") => out = Some(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let mut ids = Vec::new();
    for name in nets.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let i = b
            .nets
            .iter()
            .position(|n| n.name == name)
            .ok_or(format!("no net {name}"))?;
        ids.push(i as u32);
    }
    let clearance = trace_core::route::board_clearance(&b);
    let m = trace_core::bus::match_lengths(&b, &ids, target, amp, None, clearance);
    println!("Target {:.1} mil", m.target);
    for t in &m.nets {
        let short = m.target - t.after;
        println!(
            "  {:<28} {:>9.1} -> {:>9.1}{}",
            b.nets[t.net as usize].name,
            t.before,
            t.after,
            if short > 1.0 {
                format!("  ({short:.1} mil short: no room for meanders)")
            } else {
                String::new()
            }
        );
    }
    if let Some(out) = out {
        let edits = m.edits;
        for e in &edits {
            e.apply(&mut b);
        }
        if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
            e.apply(&mut b);
        }
        trace_core::connectivity::analyse(&mut b);
        trace_core::file::save(&b, Path::new(out))?;
        println!("Saved {out}");
    }
    Ok(())
}

fn sim(file: &str, from: &str, to: &str, rest: &[&str]) -> Res {
    use trace_core::ibis::{self, Corner};
    use trace_core::sim::{self, End, Pins, Setup, Term};
    let b = board(file)?;
    let mut opt: std::collections::HashMap<&str, &str> = Default::default();
    let mut out = None;
    let mut single = false;
    let (mut lossless, mut quiet) = (false, false);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--single" => single = true,
            "--lossless" => lossless = true,
            "--no-xtalk" => quiet = true,
            _ if a.starts_with("--") => {
                let v = it.next().ok_or(format!("{a} needs a value"))?;
                opt.insert(a, v);
            }
            _ => out = Some(a),
        }
    }
    let part = |s: &str| -> Result<(String, String), String> {
        let (d, p) = s.split_once('.').ok_or(format!(
            "{s}: give the pin as designator.pin, such as U1.A5"
        ))?;
        Ok((d.to_string(), p.to_string()))
    };
    let ((dd, dp), (rd, rp)) = (part(from)?, part(to)?);
    let pad = |d: &str, p: &str| sim::find_pad(&b, d, p).ok_or(format!("no pad {p} on {d}"));
    let (dpad, rpad) = (pad(&dd, &dp)?, pad(&rd, &rp)?);
    let ibs = opt.get("--ibis").ok_or("--ibis file.ibs is needed")?;
    let fd = ibis::load(Path::new(ibs))?;
    let fr = match opt.get("--rx-ibis") {
        Some(p) => ibis::load(Path::new(p))?,
        None => fd.clone(),
    };
    let (dm, dpkg) = sim::pin_model(&fd, &dp, opt.get("--model").copied(), true)?;
    let (rm, rpkg) = sim::pin_model(&fr, &rp, opt.get("--rx-model").copied(), false)?;
    let corner = match opt.get("--corner").copied().unwrap_or("typ") {
        "min" | "slow" => Corner::Min,
        "max" | "fast" => Corner::Max,
        _ => Corner::Typ,
    };
    let rate = match opt.get("--rate") {
        Some(r) => ibis::num(r).ok_or(format!("not a rate: {r}"))?,
        None => 1e9,
    };
    let term = match opt.get("--term").copied() {
        None | Some("none") => Term::None,
        Some(t) if t.ends_with("diff") => Term::Across(
            ibis::num(t.trim_end_matches("diff")).ok_or(format!("not a resistance: {t}"))?,
        ),
        Some(t) => {
            let (r, v) = t.split_once('@').unwrap_or((t, "0"));
            Term::ToRef {
                r: ibis::num(r).ok_or(format!("not a resistance: {r}"))?,
                v: ibis::num(v).ok_or(format!("not a voltage: {v}"))?,
            }
        }
    };
    let (dn, rn) = if single {
        (None, None)
    } else {
        (sim::partner_pad(&b, dpad), sim::partner_pad(&b, rpad))
    };
    let pins = Pins {
        driver: dpad,
        receiver: rpad,
        driver_n: dn,
        receiver_n: rn.filter(|_| dn.is_some()),
    };
    let setup = Setup {
        corner,
        rate,
        bits: opt
            .get("--bits")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(160),
        term,
        loss: if lossless {
            None
        } else {
            let mut l = sim::Loss::default();
            if let Some(d) = opt.get("--df") {
                l.tan_d = d.parse().map_err(|_| format!("not a loss tangent: {d}"))?;
            }
            if let Some(r) = opt.get("--rough") {
                let um = ibis::num(r.trim_end_matches("um").trim_end_matches('u'))
                    .ok_or(format!("not a roughness: {r}"))?;
                l.rough_um = um;
            }
            Some(l)
        },
        crosstalk: !quiet,
    };
    let t0 = std::time::Instant::now();
    let run = sim::simulate(
        &b,
        pins,
        End {
            model: dm,
            pkg: dpkg,
        },
        End {
            model: rm,
            pkg: rpkg,
        },
        &setup,
    )?;
    let net = |p: usize| {
        b.geometry.pads[p]
            .net
            .map_or("?".to_string(), |n| b.nets[n as usize].name.clone())
    };
    let what = match pins.driver_n {
        Some(n) => format!("pair {} / {}", net(dpad), net(n)),
        None => net(dpad),
    };
    println!(
        "{what}: {} ({}) to {} ({}), {} at {:.3} Gb/s",
        from,
        dm.name,
        to,
        rm.name,
        corner.label(),
        rate / 1e9
    );
    println!(
        "  route delay {:.1} ps, driver switching {:.0}/{:.0} ps, step {:.2} ps ({:.2} s)",
        run.delay * 1e12,
        run.switch.0 * 1e12,
        run.switch.1 * 1e12,
        run.dt * 1e12,
        t0.elapsed().as_secs_f64()
    );
    let e = &run.eye;
    println!(
        "  eye height {:.3} V (ones from {:.3} V, zeros to {:.3} V), width {:.1} ps = {:.0} % of {:.1} ps",
        e.height,
        e.high,
        e.low,
        e.width * 1e12,
        e.width / e.ui * 100.0,
        e.ui * 1e12
    );
    match &setup.loss {
        Some(l) => println!(
            "  loss {:.2} dB at {:.2} GHz (Nyquist), Df {}, roughness {} um",
            run.loss_db,
            rate / 2e9,
            l.tan_d,
            l.rough_um
        ),
        None => println!("  lossless lines"),
    }
    if let Some(q) = &run.quiet {
        println!(
            "  crosstalk: peak {:.1} mV at the receiver; without it the eye is {:.3} V, {:.0} % UI",
            run.noise * 1e3,
            q.height,
            q.width / q.ui * 100.0
        );
    }
    for a in &run.aggressors {
        println!(
            "    aggressor {} from {}{}: {:.0} mil beside, Kb {:.3}",
            a.net,
            a.from,
            if a.inverted { " (inverted)" } else { "" },
            a.coupled,
            a.kb
        );
    }
    for n in &run.notes {
        println!("  {n}");
    }
    if let Some(out) = out {
        std::fs::write(out, sim::svg(&run, &format!("{what}: {from} to {to}")))?;
        println!("Saved {out}");
    }
    Ok(())
}

fn check(file: &str) -> Res {
    let b = board(file)?;
    let set = b.imported_constraints();
    if set.groups.is_empty() {
        println!("No length, matched-length or skew rules found.");
        return Ok(());
    }
    let mil = |l: trace_core::Length| format!("{:.1}", l.mil());
    let opt = |l: Option<trace_core::Length>| l.map_or("-".to_string(), mil);
    for g in &set.groups {
        let r = b.check(g);
        println!(
            "{}  [{}]  {} nets, {} failing{}",
            g.name,
            g.members.describe(),
            r.nets.len(),
            r.failing,
            if g.enabled { "" } else { "  (disabled)" }
        );
        let l = &g.limits;
        println!(
            "  limits (mil): min {} max {} match {} skew {} vias {}",
            opt(l.min_length),
            opt(l.max_length),
            opt(l.match_tolerance),
            opt(l.max_skew),
            l.max_vias.map_or("-".into(), |v| v.to_string())
        );
        for n in r.nets.iter().filter(|n| !n.problems.is_empty()).take(20) {
            let what: Vec<&str> = n.problems.iter().map(|p| p.label()).collect();
            println!(
                "    {:<28} {:>9} mil  skew {:>7}  delta {:>7}  {}",
                clip(&n.name, 28),
                mil(n.length),
                opt(n.skew),
                opt(n.match_delta),
                what.join(", ")
            );
        }
    }
    Ok(())
}

fn view_board(file: &str, out: Option<&str>) -> Res {
    let b = board(file)?;
    let name = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = match out {
        Some(o) => std::path::PathBuf::from(o),
        None => Path::new(file).with_extension("trace.html"),
    };
    std::fs::write(&out, view::render(&b, &name))?;
    println!("wrote {}", out.display());
    view::open(&out);
    Ok(())
}

fn fab(file: &str, out: &str) -> Res {
    let b = board(file)?;
    let opt = trace_export::FabOptions::from_board(&b);
    let path = Path::new(out);
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let files = trace_export::fab_files(&b, &stem, &opt);
    println!(
        "mask expansion {:.3} mm, paste expansion {:.3} mm, vias {}",
        opt.mask_expansion.mm(),
        opt.paste_expansion.mm(),
        match (opt.tent_top, opt.tent_bottom) {
            (true, true) => "tented",
            (false, false) => "open both sides",
            (true, false) => "tented on top only",
            (false, true) => "tented on the bottom only",
        }
    );
    if out.to_ascii_lowercase().ends_with(".zip") {
        std::fs::write(path, trace_export::fab_zip(&files)?)?;
    } else {
        std::fs::create_dir_all(path)?;
        for f in &files {
            std::fs::write(path.join(&f.name), &f.data)?;
        }
    }
    for f in &files {
        println!("  {:<40} {:>9} bytes", f.name, f.data.len());
    }
    println!("wrote {} files to {out}", files.len());
    Ok(())
}

fn odb(file: &str, out: &str) -> Res {
    let b = board(file)?;
    let opt = trace_export::FabOptions::from_board(&b);
    let path = Path::new(out);
    let job = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let job = job
        .strip_suffix(".tgz")
        .or_else(|| job.strip_suffix(".TGZ"))
        .unwrap_or(&job)
        .to_string();
    let files = trace_export::odb_files(&b, &job, &opt);
    if out.to_ascii_lowercase().ends_with(".tgz") {
        std::fs::write(path, trace_export::odb_tgz(&files)?)?;
    } else {
        // The job's own folder is the one named.
        for f in &files {
            let rel = f.name.split_once('/').map(|r| r.1).unwrap_or(&f.name);
            let to = path.join(rel);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(to, &f.data)?;
        }
    }
    let layers = files
        .iter()
        .filter(|f| f.name.ends_with("/features"))
        .count();
    let bytes: usize = files.iter().map(|f| f.data.len()).sum();
    println!(
        "wrote ODB++ job {job} to {out}: {} files, {layers} feature layers, {} parts, {} nets, {bytes} bytes",
        files.len(),
        b.components.len(),
        b.nets.len()
    );
    Ok(())
}

fn convert(file: &str, out: &str) -> Res {
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .kicad_pcb".into());
        };
        std::fs::write(out, trace_export::to_kicad(&b))?;
        println!("wrote {out}");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".svg") {
        let Design::Sheet(sh) = load(file)? else {
            return Err("only schematic sheets can be drawn as .svg".into());
        };
        std::fs::write(out, trace_export::sheet_svg(&sh))?;
        println!("wrote {out}");
        return Ok(());
    }
    if file.to_ascii_lowercase().ends_with(".tracesch") {
        // A Trace schematic: every sheet, as Altium sheets in folder `out`.
        let f = trace_core::SchFile::load(Path::new(file))?;
        let from = Path::new(file).parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(out)?;
        for s in &f.sheets {
            let src = from.join(&s.file);
            let stem = Path::new(&s.file)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let original = src.is_file() && s.file.to_ascii_lowercase().ends_with(".schdoc");
            let bytes = trace_altium::write_schdoc(&s.sheet, original.then_some(src.as_path()))?;
            let to = Path::new(out).join(format!("{stem}.SchDoc"));
            std::fs::write(&to, bytes)?;
            println!("wrote {}", to.display());
        }
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".schdoc") {
        let Design::Sheet(sh) = load(file)? else {
            return Err("only schematic sheets can be written as .SchDoc".into());
        };
        let original = file.to_ascii_lowercase().ends_with(".schdoc");
        let bytes = trace_altium::write_schdoc(&sh, original.then(|| std::path::Path::new(file)))?;
        std::fs::write(out, bytes)?;
        println!("wrote {out}");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".pcbdoc") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .PcbDoc".into());
        };
        let name = std::path::Path::new(out)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if file.to_ascii_lowercase().ends_with(".pcbdoc") {
            // Saved over the original's records: nothing it holds is lost.
            let (bytes, report) =
                trace_export::to_altium_over(std::path::Path::new(file), &b, &name)?;
            std::fs::write(out, bytes)?;
            println!("wrote {out} ({})", report.summary());
        } else {
            std::fs::write(out, trace_export::to_altium(&b, &name)?)?;
            println!("wrote {out}");
        }
        let back = trace_altium::read_pcbdoc(out)?;
        for d in trace_export::altium_rt::compare(&b, &back) {
            println!("read back differs: {d}");
        }
        return Ok(());
    }
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
