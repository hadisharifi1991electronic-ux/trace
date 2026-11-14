//! `trace`: command-line front end until the editor exists.

mod view;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use trace_core::{Board, LayerKind, Length, Sheet};

const USAGE: &str = "\
trace: read Altium, Allegro and Gerber designs with the Trace engine

Boards can be Altium .PcbDoc, Cadence Allegro .brd (16.0 or newer), Trace
.tracepcb, or Gerber and drill files: a folder, a .zip, or one file.
Any command takes --stackup <file> to put a stack-up file (see trace stackup)
on the board, for boards whose file has none.

USAGE:
    trace inspect <board|file.SchDoc>           Summary of a board or sheet
    trace pairs <board> [max-skew]              Differential pair lengths and skew (default max-skew 5mil)
    trace rules <board>                         Design rules, grouped by kind
    trace check <board>                         Check length, match, skew and via limits from the board's rules
    trace drc <board> [--all]                   Design rule check: clearance, shorts, width, unrouted nets,
                                                holes, annular rings, mask slivers, silk on pads, silk to silk,
                                                part clearance, board edge, return paths over plane voids
                                                and splits, crosstalk between parallel tracks
    trace impedance <board> [--field]           Stack-up, widths for 50/100 ohm, and the impedance of each pair;
                                                with --field, nets from the field solver, pour beside tracks
                                                counted as coplanar ground
    trace field <board>                         2D field solver on each layer: widths for 50 ohm and 100 ohm
                                                pairs, delay and loss, beside the formulas' answers
    trace field <board> <layer> <w> [s] [--ground G] [out.svg]
                                                Solve one cross-section (a track, or a pair with gap s; mil,
                                                or mm with an mm suffix; --ground: copper G beside it on its layer):
                                                impedance, modes, coupling, RLGC and loss; with out, draw the field
    trace panel <board> <cols>x<rows> <out.tracepcb> [--gap 2mm] [--rail 5mm] [--tabs N] [--vscore]
                                                Panel of copies with rails, fiducials, tooling holes, and
                                                tabs with mouse bites (or V-score lines); then fab or odb it
    trace match <board> <net,net,...> [out.tracepcb] [--target L] [--amp L]
                                                Match the nets' lengths (to the longest, or the target) with
                                                meanders where they fit; with out, save the board tuned
    trace sim <board> <U1.pin> <U2.pin> --ibis drv.ibs [--rx-ibis rx.ibs] [--model M] [--rx-model M]
              [--rate 1G] [--bits N] [--corner typ|min|max] [--term 50|50@0.9|100diff] [--single]
              [--df 0.02] [--rough 1um] [--lossless] [--no-xtalk] [--formulas] [out.svg]
                                                Simulate the route from a driver pin to a receiver pin with IBIS
                                                models (a pair when the net has a partner): waveforms and eye,
                                                with track loss and crosstalk from nets beside the route
    trace ir <board> <net> [--source U5] [--volts 1.0] [--load U1=5A ...] [--total 10A] [--plating 0.8mil] [out.svg]
                                                DC voltage drop (IR drop) over a supply net's pours, tracks and
                                                vias from the source part to the loads (no --load: the total,
                                                default 1 A, shared by pad count); with out, a drop map per layer
    trace via <board> <layer> <layer> [--drill D] [--pad P] [--antipad A] [--width W] [--width2 W]
              [--pair PITCH] [--oval] [--returns N@D] [--backdrill] [--fmax 40] [--cell C]
              [--optimise GHz [--free-pitch]] [out.s2p] [out.svg]
                                                3D full-wave (FDTD) solve of a via from one layer to another
                                                with the board's stack-up (default drill, pad and antipad from
                                                its commonest via): S11/S21, or Sdd for a pair, as Touchstone
                                                and a plot; the stub resonance when the barrel runs on.
                                                --optimise GHz sweeps the antipad (--oval for a pair) and the
                                                return vias for the best return loss to GHz, keeping the pair's
                                                pitch unless --free-pitch
    trace pairsize <board> <layer> [--zdiff 100] [--wmin W] [--wmax W] [--gap S] [--ghz 8] [--next D] [--xtalk 2]
                                                Every track width and gap on a layer that gives the target Zdiff
                                                (2D field solver), ranked by loss, with crosstalk and etch
    trace cascade <load.s1p> [stage.s2p | --line Z@len@Eeff[@dB/in]]... [--ref R] [--step GHz]
                                                Return loss into a chain of two-ports (vias from trace via, lines)
                                                ending in a load such as a chip's port model, input side first
    trace stitch <board> <net> <out.tracepcb> [--grid 100mil] [--stagger] [--via 16/8] [--layers 1-4]
              [--clearance C] [--edge 20mil] [--area x0,y0,x1,y1]
                                                Stitch a net's pours and planes with vias on a grid, each clear
                                                of other copper and landing on the net on two or more layers
    trace fence <board> <net[,net]> <out.tracepcb> [--ground GND] [--ghz 6 | --pitch P] [--gap G]
              [--rows 1] [--via 16/8] [--layers 1-4] [--on 1,3] [--clearance C]
                                                Line a net's tracks (RF lines, a pair) with ground vias on both
                                                sides, a twentieth of a wavelength apart at GHz (default 6)
    trace teardrops <board> <out.tracepcb> [--nets a,b] [--smd] [--no-vias] [--no-th] [--length 1.0]
              [--width 0.9] [--clearance C] [--remove]
                                                Teardrops where tracks meet vias and through-hole pads (--smd:
                                                surface pads too), kept clear of other nets; --remove takes them off
    trace gloss <board> <out.tracepcb> [--nets a,b] [--no-merge] [--keep-dangling] [--corners] [--clearance C]
                                                Tidy routing: zero-length and duplicate tracks, straight joins
                                                merged, dangling ends cut back, square corners chamfered (--corners)
    trace swap <board> <part> <pin> <pin> <out.tracepcb>
    trace erc <sheet.SchDoc|project.PrjPcb|sch.tracesch> [--all]
    trace annotate <sheet.SchDoc|project.PrjPcb|sch.tracesch> <out.tracesch> [--all] [--down] [--start N] [--per-sheet 100]
    trace fanout <board> <U1[,U2]> <out.tracepcb> [--via 18/8] [--layers 1-4] [--in-pad] [--inward]
              [--skip-rows N] [--width W] [--clearance C] [--no-net]
                                                Fan out BGA and other surface parts: a short track from each
                                                pad to a via between the pads (away from the centre), or a via
                                                in the pad; outer rows can be left for surface routing
    trace stackup <board> [out.txt]             Print the board's stack-up as a stack-up file (name, kind,
                                                thickness, Er, Df); edit it and give it back with --stackup
    trace validate [--quick] [out.txt]          Accuracy checks: the field, IR drop and 3D via solvers against
                                                exact answers, the 2D solver and finer grids (--quick: no 3D)
    trace stubs <board> [out.tracepcb]          Via stubs on nets with a stub rule (default: pair nets, 10 mil);
                                                with out, backdrill those too long and save the board
    trace view <board> [out.html]               Open the board in the viewer (in your browser)
    trace convert <file> <out.json>             Export to Trace's open JSON model
    trace convert <file> <out.kicad_pcb>        Write a KiCad board (KiCad 7 and later)
    trace convert <file> <out.PcbDoc>           Write an Altium Designer board
    trace convert <file> <out.xml>              Write an IPC-2581 (revision C) file
    trace convert <file> <out.dxf>              Write every layer of a board as a DXF (mm)
    trace convert <file.SchDoc> <out.svg>       Draw a schematic sheet as SVG
    trace convert <file.SchDoc> <out.SchDoc>    Write a schematic sheet as an Altium sheet
    trace convert <file.tracesch> <folder>      Write every sheet of a Trace schematic as Altium sheets
    trace fab <board> <out.zip|folder>          Gerber X2, drill, IPC-356, pick-and-place and BOM files
    trace odb <board> <out.tgz|folder>          ODB++ job: layers, drills, parts and nets in one package
    trace assembly <board> <out.pdf> [--a4]     Assembly drawing: top and bottom views with part bodies,
                                                designators and pin 1 marks, then the parts list (A3)
    trace step <board> <out.step> [--vias] [--no-parts] [--min-hole 0.3]
                                                STEP (AP214) model for mechanical CAD: the board solid with its
                                                cut-outs and holes (vias with --vias, mm), parts as boxes
    trace fabdrawing <board> <out.pdf> [--a4]   Fabrication drawing: outline with drill symbols and
                                                dimensions, drill table, notes, then the stack-up (A3)
    trace layers <gerber folder|zip|board>      Show which layer each Gerber or drill file was taken as
                                                (for a board, its stack-up)
    trace symbols <lib.SchLib|IntLib> [out.svg] List the symbols of an Altium schematic library;
                                                with out, draw them all as SVG
    trace footprints <lib|board> [out]          List the footprints of a .PcbLib, .IntLib, .tracelib or board;
                                                with out, save them as a Trace library (.tracelib)
                                                or an Altium one (.PcbLib)
";

/// A stack-up file given with --stackup, put on every board loaded.
static STACKUP: std::sync::OnceLock<trace_core::Stackup> = std::sync::OnceLock::new();

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--stackup") {
        let Some(path) = args.get(i + 1).cloned() else {
            eprintln!("error: --stackup needs a file");
            return ExitCode::FAILURE;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| trace_core::stackfile::parse(&t))
        {
            Ok(st) => {
                let _ = STACKUP.set(st);
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
        args.drain(i..i + 2);
    }
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
        ["impedance", file] => impedance(file, false),
        ["impedance", file, "--field"] => impedance(file, true),
        ["field", file] => field_layers(file),
        ["field", file, layer, w, rest @ ..] => field_one(file, layer, w, rest),
        ["panel", file, grid, out, rest @ ..] => panel(file, grid, out, rest),
        ["match", file, nets, rest @ ..] => match_nets(file, nets, rest),
        ["sim", file, from, to, rest @ ..] => sim(file, from, to, rest),
        ["ir", file, net, rest @ ..] => ir(file, net, rest),
        ["via", file, from, to, rest @ ..] => via(file, from, to, rest),
        ["pairsize", file, layer, rest @ ..] => pairsize(file, layer, rest),
        ["validate", rest @ ..] => validate(rest),
        ["stitch", file, net, out, rest @ ..] => stitch_cmd(file, net, out, rest),
        ["teardrops", file, out, rest @ ..] => teardrops_cmd(file, out, rest),
        ["gloss", file, out, rest @ ..] => gloss_cmd(file, out, rest),
        ["fanout", file, parts, out, rest @ ..] => fanout_cmd(file, parts, out, rest),
        ["erc", file, rest @ ..] => erc_cmd(file, rest),
        ["swap", file, part, a, b, out] => swap_cmd(file, part, a, b, out),
        ["annotate", file, out, rest @ ..] => annotate_cmd(file, out, rest),
        ["fence", file, nets, out, rest @ ..] => fence_cmd(file, nets, out, rest),
        ["stackup", file] => stackup(file, None),
        ["stackup", file, out] => stackup(file, Some(out)),
        ["cascade", load, rest @ ..] => cascade(load, rest),
        ["stubs", file] => stubs(file, None),
        ["stubs", file, out] => stubs(file, Some(out)),
        ["convert", file, out] => convert(file, out),
        ["layers", file] => layers(file),
        ["fab", file, out] => fab(file, out),
        ["odb", file, out] => odb(file, out),
        ["assembly", file, out] => assembly(file, out, trace_export::Paper::A3),
        ["assembly", file, out, "--a4"] => assembly(file, out, trace_export::Paper::A4),
        ["step", file, out, rest @ ..] => step_cmd(file, out, rest),
        ["fabdrawing", file, out] => fab_drawing(file, out, trace_export::Paper::A3),
        ["fabdrawing", file, out, "--a4"] => fab_drawing(file, out, trace_export::Paper::A4),
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
    let mut d = load_file(file)?;
    if let (Design::Board(b), Some(st)) = (&mut d, STACKUP.get()) {
        trace_core::stackfile::apply(b, st)?;
    }
    Ok(d)
}

fn load_file(file: &str) -> Result<Design, Box<dyn std::error::Error>> {
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
    if !is_fab(file) {
        // A board: its stack-up.
        return stackup(file, None);
    }
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

/// The field solver on every copper layer.
fn field_layers(file: &str) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("field needs a board".into());
    };
    let geo = b.layer_geometry();
    let t0 = std::time::Instant::now();
    println!(
        "{:<14} {:>15} {:>12} {:>15} {:>7} {:>7} {:>8}",
        "layer", "50R w field/ipc", "Z0 at w ipc", "100R w/s field", "ps/in", "dB/in", "dB/in"
    );
    println!(
        "{:<14} {:>15} {:>12} {:>15} {:>7} {:>7} {:>8}",
        "", "mil", "ohm", "mil", "", "1 GHz", "10 GHz"
    );
    for g in &geo {
        let Some(w) = g.width_for_field(50.0) else {
            println!("{:<14} -", g.name);
            continue;
        };
        let wd = w * 0.75;
        let s = g.gap_for_field(wd, 100.0);
        let l = trace_core::field::single(g, w).ok_or("solver failed")?;
        let td = g.tan_d.unwrap_or(0.02);
        println!(
            "{:<14} {:>7.2}/{:<7} {:>12.1} {:>15} {:>7.0} {:>7.3} {:>8.3}",
            g.name,
            w,
            g.width_for(50.0)
                .map(|x| format!("{x:.2}"))
                .unwrap_or_else(|| "-".into()),
            g.z0(w),
            s.map(|s| format!("{wd:.2}/{s:.2}"))
                .unwrap_or_else(|| "-".into()),
            l.ps_per_in(),
            l.db_per_in(1e9, td, g.rough_um),
            l.db_per_in(1e10, td, g.rough_um)
        );
    }
    println!(
        "\n(Df {} where the stack-up gives none; solved in {:.1} s)",
        0.02,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// One cross-section from the field solver.
fn field_one(file: &str, layer: &str, w: &str, rest: &[&str]) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("field needs a board".into());
    };
    let geo = b.layer_geometry();
    let g = layer
        .parse::<usize>()
        .ok()
        .and_then(|n| geo.get(n.checked_sub(1)?))
        .or_else(|| geo.iter().find(|g| g.name.eq_ignore_ascii_case(layer)))
        .ok_or_else(|| {
            format!(
                "no copper layer {layer}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let len = |v: &str| -> Result<f64, String> {
        let (n, k) = match v.strip_suffix("mm") {
            Some(n) => (n, 1.0 / 0.0254),
            None => (v.trim_end_matches("mil"), 1.0),
        };
        n.parse::<f64>()
            .map(|x| x * k)
            .map_err(|_| format!("not a length: {v}"))
    };
    let w = len(w)?;
    // `--ground G`: ground copper G beside the track on its own layer.
    let mut side = None;
    let mut args: Vec<&str> = Vec::new();
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        if a == "--ground" {
            side = Some(len(it.next().ok_or("--ground needs a gap")?)?);
        } else {
            args.push(a);
        }
    }
    let rest = args.as_slice();
    let (s, out) = match rest {
        [] => (None, None),
        [x] if x.ends_with(".svg") => (None, Some(*x)),
        [x] => (Some(len(x)?), None),
        [x, o] => (Some(len(x)?), Some(*o)),
        _ => return Err("too many arguments".into()),
    };
    let fmt = |v: Option<f64>| v.map_or("-".into(), |x| format!("{x:.2}"));
    println!(
        "{}: copper {:.2} mil, {} mil above, {} mil below, Er {:.2}{}",
        g.name,
        g.t,
        fmt(g.above),
        fmt(g.below),
        g.er,
        g.mask
            .map(|m| format!(", solder mask {:.2} mil Er {:.2}", m[0], m[1]))
            .unwrap_or_default()
    );
    for (side, sl) in [("above", &g.slabs_above), ("below", &g.slabs_below)] {
        if !sl.is_empty() {
            let v: Vec<String> = sl
                .iter()
                .map(|x| {
                    format!(
                        "{:.2} mil Er {:.2}{}",
                        x[0],
                        x[1],
                        if x[2] >= 0.0 {
                            format!(" Df {:.4}", x[2])
                        } else {
                            String::new()
                        }
                    )
                })
                .collect();
            println!("  {side}: {}", v.join(", "));
        }
    }
    let t0 = std::time::Instant::now();
    let td = g.tan_d.unwrap_or(0.02);
    let (pic, title) = match s {
        None => {
            let (l, pic) = trace_core::field::single_picture(g, w, side).ok_or("solver failed")?;
            if let Some(gap) = side {
                println!("ground beside the track {gap:.2} mil from each edge (coplanar)");
            }
            println!(
                "track {w:.2} mil: Z0 {:.2} ohm (microstrip/stripline formula {:.2}), Er eff {:.3}, {:.1} ps/in",
                l.z0,
                g.z0(w),
                l.eeff,
                l.ps_per_in()
            );
            println!(
                "  L {:.2} nH/in, C {:.3} pF/in, R dc {:.3} ohm/in, R 1 GHz {:.3} ohm/in (smooth)",
                l.l * 0.0254 * 1e9,
                l.c * 0.0254 * 1e12,
                l.r_dc * 0.0254,
                l.r_ghz * 0.0254
            );
            println!(
                "  loss {:.3} dB/in at 1 GHz, {:.3} at 10 GHz (Df {:.4}, roughness {})",
                l.db_per_in(1e9, td, g.rough_um),
                l.db_per_in(1e10, td, g.rough_um),
                if l.fill_given > 0.0 {
                    l.tan_eff / l.fill_given
                } else {
                    td
                },
                g.rough_um.map_or("none".into(), |r| format!("{r} um"))
            );
            (pic, format!("{} · {w:.2} mil · Z0 {:.1} Ω", g.name, l.z0))
        }
        Some(s) => {
            let (p, pic) = trace_core::field::pair_picture(g, w, s, side).ok_or("solver failed")?;
            println!(
                "pair {w:.2}/{s:.2} mil: Zdiff {:.2} ohm (formula {:.2}), Zcomm {:.2} ohm",
                p.zdiff,
                g.zdiff(w, s),
                p.zcomm
            );
            println!(
                "  Z odd {:.2}, Z even {:.2} ohm; Er eff odd {:.3}, even {:.3}; single Z0 {:.2} ohm",
                p.z_odd, p.z_even, p.e_odd, p.e_even, p.single.z0
            );
            println!(
                "  coupling: near end {:.2} %, far end {:.3} ps/in of rise time",
                100.0 * p.kb,
                // Far-end crosstalk = Kf · delay · dV/dt.
                p.kf * p.single.ps_per_in()
            );
            let alpha_c = p.r_odd_ghz / (2.0 * p.z_odd);
            let alpha_d = PI_ * 1e9 * p.e_odd.sqrt() / 2.998e8
                * (p.tan_odd + (1.0 - p.single.fill_given) * td * p.single.fill);
            println!(
                "  differential loss {:.3} dB/in at 1 GHz (smooth copper)",
                8.686 * (alpha_c + alpha_d) * 0.0254
            );
            (
                pic,
                format!(
                    "{} · {w:.2}/{s:.2} mil · Zdiff {:.1} Ω (odd mode)",
                    g.name, p.zdiff
                ),
            )
        }
    };
    println!("  solved in {:.2} s", t0.elapsed().as_secs_f64());
    if let Some(out) = out {
        std::fs::write(out, pic.svg(900.0, &title))?;
        println!("Wrote {out}");
    }
    Ok(())
}

const PI_: f64 = std::f64::consts::PI;

fn impedance(file: &str, field: bool) -> Res {
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
    let t0 = std::time::Instant::now();
    let rows = b.net_impedance_model(&geo, field);
    if field {
        println!("\nfield solver: {:.1} s", t0.elapsed().as_secs_f64());
        let mut cpw: Vec<_> = rows
            .iter()
            .filter(|r| r.side != [None, None] && r.zdiff.is_none())
            .collect();
        cpw.sort_by(|a, b| b.length.total_cmp(&a.length));
        if !cpw.is_empty() {
            println!("\ntracks with pour beside them (coplanar), longest first");
            println!(
                "{:<28} {:<10} {:>6} {:>13} {:>7} {:>7} {:>8}",
                "net", "layer", "w", "pour gaps", "Z0", "no pour", "length"
            );
            for r in cpw.iter().take(25) {
                let g = &geo[r.layer as usize];
                println!(
                    "{:<28} {:<10} {:>6.2} {:>13} {:>7.1} {:>7.1} {:>8.0}",
                    clip(&b.nets[r.net as usize].name, 28),
                    g.name,
                    r.width,
                    r.side
                        .map(|s| s.map_or("-".into(), |x| format!("{x:.2}")))
                        .join(" / "),
                    r.z0,
                    g.z0_field(r.width),
                    r.length
                );
            }
        }
    }
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
    let (mut lossless, mut quiet, mut formulas) = (false, false, false);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--single" => single = true,
            "--lossless" => lossless = true,
            "--no-xtalk" => quiet = true,
            "--formulas" => formulas = true,
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
                l.stackup_df = false;
            }
            if let Some(r) = opt.get("--rough") {
                let um = ibis::num(r.trim_end_matches("um").trim_end_matches('u'))
                    .ok_or(format!("not a roughness: {r}"))?;
                l.rough_um = um;
                l.stackup_rough = false;
            }
            Some(l)
        },
        crosstalk: !quiet,
        field: !formulas,
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
        Some(_) => println!(
            "  loss {:.2} dB at {:.2} GHz (Nyquist); {}",
            run.loss_db,
            rate / 2e9,
            run.materials
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

/// Parses a current such as `5A`, `500mA` or `2.5`.
fn amps(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase();
    if let Some(v) = t.strip_suffix("ma") {
        return v.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    t.strip_suffix('a').unwrap_or(&t).trim().parse().ok()
}

fn ir(file: &str, net: &str, rest: &[&str]) -> Res {
    use trace_core::pi;
    let b = board(file)?;
    let id = b
        .nets
        .iter()
        .position(|n| n.name == net || n.name.eq_ignore_ascii_case(net))
        .ok_or(format!("no net {net}"))? as u32;
    let part = |d: &str| -> Result<usize, Box<dyn std::error::Error>> {
        Ok(b.components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?)
    };
    let (mut source, mut volts, mut total, mut plating) = (None, 1.0, 1.0, 0.8);
    let mut loads: Vec<(usize, f64)> = Vec::new();
    let mut out = None;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--source" => source = Some(part(val()?)?),
            "--volts" => {
                let v = val()?;
                volts = v
                    .trim_end_matches(['V', 'v'])
                    .parse()
                    .map_err(|_| format!("not a voltage: {v}"))?
            }
            "--total" => {
                let v = val()?;
                total = amps(v).ok_or(format!("not a current: {v}"))?
            }
            "--plating" => {
                let v = val()?;
                plating = Length::parse(v).ok_or(format!("not a length: {v}"))?.mil()
            }
            "--load" => {
                let v = val()?;
                let (d, i) = v
                    .split_once('=')
                    .ok_or(format!("--load wants PART=AMPS, not {v}"))?;
                loads.push((part(d)?, amps(i).ok_or(format!("not a current: {i}"))?));
            }
            _ if out.is_none() && !a.starts_with("--") => out = Some(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let source = match source {
        Some(s) => s,
        None => {
            pi::guess_source(&b, id).ok_or("no likely source on the net: name one with --source")?
        }
    };
    let guessed = loads.is_empty();
    if guessed {
        // Share the total among the other parts by their pad count.
        let parts = pi::default_loads(&b, id, source);
        let pads: usize = parts.iter().map(|p| p.1).sum();
        if pads == 0 {
            return Err("no other parts on the net to load it".into());
        }
        loads = parts
            .iter()
            .map(|p| (p.0, total * p.1 as f64 / pads as f64))
            .collect();
    }
    let mut setup = pi::Setup::new(id, source, volts);
    setup.loads = loads;
    setup.plating_mil = plating;
    let t = std::time::Instant::now();
    let r = pi::solve(&b, &setup)?;
    let des = |i: usize| b.components[i].designator.as_str();
    println!(
        "{}: {:.3} V from {}, {:.3} A into {} loads{}",
        b.nets[id as usize].name,
        volts,
        des(source),
        r.loads.iter().map(|l| l.amps).sum::<f64>(),
        r.loads.len(),
        if guessed {
            " (shared by pad count)"
        } else {
            ""
        }
    );
    println!(
        "Grid {:.2} mil, {} x {} cells, {} vias carrying current; solved in {:.1} s",
        r.cell,
        r.cols,
        r.rows,
        r.vias,
        t.elapsed().as_secs_f64()
    );
    println!(
        "  {:<12} {:>8} {:>10} {:>10} {:>9}",
        "Load", "A", "V min", "V mean", "drop mV"
    );
    let mut rows: Vec<_> = r.loads.iter().collect();
    rows.sort_by(|a, b| a.v_min.total_cmp(&b.v_min));
    let hidden = rows.len().saturating_sub(20);
    for l in rows.into_iter().take(20) {
        if l.connected {
            println!(
                "  {:<12} {:>8.3} {:>10.4} {:>10.4} {:>9.2}",
                des(l.part),
                l.amps,
                l.v_min,
                l.v_mean,
                (volts - l.v_min) * 1000.0
            );
        } else {
            println!(
                "  {:<12} {:>8.3}   not joined to the source on this net",
                des(l.part),
                l.amps
            );
        }
    }
    if hidden > 0 {
        println!("  ... and {hidden} more with less drop");
    }
    println!(
        "Largest drop {:.2} mV ({:.2}%), copper loss {:.1} mW",
        r.max_drop * 1000.0,
        r.max_drop / volts * 100.0,
        r.loss_w * 1000.0
    );
    if let Some((p, a)) = r.worst_via {
        println!("Busiest via at ({:.0}, {:.0}) mil: {:.3} A", p[0], p[1], a);
    }
    if let Some((p, a, j)) = r.worst_track {
        println!(
            "Densest track at ({:.0}, {:.0}) mil: {:.3} A, {:.1} A/mm2",
            p[0], p[1], a, j
        );
    }
    for n in &r.notes {
        println!("Note: {n}");
    }
    if let Some(out) = out {
        // Marks: the source and each load at the middle of its pads.
        let mid = |part: usize| -> Option<[f64; 2]> {
            let ps: Vec<_> = b
                .geometry
                .pads
                .iter()
                .filter(|p| p.component == Some(part as u32) && p.net == Some(id))
                .map(|p| [p.at.x.mil(), p.at.y.mil()])
                .collect();
            (!ps.is_empty()).then(|| {
                let n = ps.len() as f64;
                [
                    ps.iter().map(|p| p[0]).sum::<f64>() / n,
                    ps.iter().map(|p| p[1]).sum::<f64>() / n,
                ]
            })
        };
        let mut marks = Vec::new();
        if let Some(p) = mid(source) {
            marks.push((p, format!("{} (source)", des(source))));
        }
        let mut heavy: Vec<_> = r.loads.iter().collect();
        heavy.sort_by(|a, b| b.amps.total_cmp(&a.amps));
        for l in heavy.into_iter().take(12) {
            if let Some(p) = mid(l.part) {
                marks.push((p, des(l.part).to_string()));
            }
        }
        let path = Path::new(out);
        let stem = path.with_extension("");
        for m in &r.maps {
            let f = if r.maps.len() == 1 {
                path.to_path_buf()
            } else {
                let safe: String = m
                    .name
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect();
                PathBuf::from(format!("{}-{safe}.svg", stem.display()))
            };
            let title = format!("{} on {}", b.nets[id as usize].name, m.name);
            std::fs::write(&f, r.svg(m, 1200.0, &title, &marks))?;
            println!("Saved {}", f.display());
        }
    }
    Ok(())
}

fn pairsize(file: &str, layer: &str, rest: &[&str]) -> Res {
    use trace_core::pairopt::{self, PairGoal};
    let b = board(file)?;
    let geo = b.layer_geometry();
    let g = layer
        .parse::<usize>()
        .ok()
        .and_then(|n| geo.get(n.checked_sub(1)?))
        .or_else(|| geo.iter().find(|g| g.name.eq_ignore_ascii_case(layer)))
        .ok_or_else(|| {
            format!(
                "no copper layer {layer}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let mut goal = PairGoal::new(100.0);
    if let Some(w) = trace_core::route::board_min_width(&b) {
        goal.w_min = w.max(1.0);
    }
    goal.s_min = trace_core::route::board_clearance(&b).clamp(2.0, 20.0);
    let mut next_max = 2.0;
    let mut it = rest.iter();
    let num = |x: Option<&&str>, what: &str| -> Result<f64, String> {
        let x = x.ok_or(format!("{what} needs a value"))?;
        x.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%')
            .parse()
            .map_err(|_| format!("not a number: {x}"))
    };
    while let Some(&a) = it.next() {
        match a {
            "--zdiff" => goal.zdiff = num(it.next(), a)?,
            "--wmin" => goal.w_min = num(it.next(), a)?,
            "--wmax" => goal.w_max = num(it.next(), a)?,
            "--gap" => goal.s_min = num(it.next(), a)?,
            "--ghz" => goal.f = num(it.next(), a)? * 1e9,
            "--next" => goal.neighbour = Some(num(it.next(), a)?),
            "--xtalk" => next_max = num(it.next(), a)?,
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    println!(
        "{}: {:.0} Ω pairs, tracks {:.2} to {:.2} mil, gap at least {:.2} mil, loss at {:.1} GHz",
        g.name,
        goal.zdiff,
        goal.w_min,
        goal.w_max,
        goal.s_min,
        goal.f / 1e9
    );
    let opts = pairopt::pair_options(g, &goal);
    if opts.is_empty() {
        return Err("no width in that range reaches the target with the gap allowed".into());
    }
    let best = pairopt::best(&opts, next_max, None);
    println!(
        "{:>7} {:>7} {:>7} {:>9} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} {:>6}",
        "width",
        "gap",
        "Zdiff",
        "dB/in",
        "copper",
        "diel",
        "ps/in",
        "NEXT%",
        "FEXT%",
        "span",
        "etch"
    );
    for (i, o) in opts.iter().enumerate() {
        println!(
            "{:>7.2} {:>7.2} {:>7.1} {:>9.3} {:>8.3} {:>8.3} {:>8.1} {:>7.2} {:>7.2} {:>7.1} {:>6.1}{}",
            o.w,
            o.s,
            o.zdiff,
            o.loss,
            o.loss_cu,
            o.loss_diel,
            o.delay,
            o.next,
            o.fext,
            o.span,
            o.etch,
            if Some(i) == best { "  <- best" } else { "" }
        );
    }
    if best.is_none() {
        println!("None keeps crosstalk under {next_max} %.");
    }
    Ok(())
}

fn cascade(load: &str, rest: &[&str]) -> Res {
    use trace_core::fdtd::Cx;
    use trace_core::touchstone::{self, Network};
    enum Stage {
        File(String, Network),
        Line(f64, f64, f64, f64),
    }
    let ld = touchstone::read(std::path::Path::new(load))?;
    if ld.ports != 1 {
        return Err("the load must be a one-port (.s1p)".into());
    }
    let mut stages = Vec::new();
    let mut z_print = None;
    let mut step = None;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--line" => {
                let x = it.next().ok_or("--line needs Z@length[@Eeff[@dB/in]]")?;
                let v: Vec<f64> = x
                    .split('@')
                    .map(|p| p.trim_end_matches("mil").parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("not Z@length@Eeff@dB/in: {x}"))?;
                if v.len() < 2 {
                    return Err(format!("--line needs Z@length: {x}").into());
                }
                stages.push(Stage::Line(
                    v[0],
                    v[1],
                    v.get(2).copied().unwrap_or(4.0),
                    v.get(3).copied().unwrap_or(0.0),
                ));
            }
            "--ref" => {
                z_print = Some(
                    it.next()
                        .and_then(|r| r.parse::<f64>().ok())
                        .ok_or("--ref needs ohms")?,
                )
            }
            "--step" => {
                step = Some(
                    it.next()
                        .and_then(|r| r.parse::<f64>().ok())
                        .ok_or("--step needs GHz")?,
                )
            }
            _ if !a.starts_with("--") => {
                let n = touchstone::read(std::path::Path::new(a))?;
                if n.ports != 2 {
                    return Err(format!("{a}: stages must be two-ports (.s2p)").into());
                }
                stages.push(Stage::File(a.to_string(), n));
            }
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let zr = z_print.unwrap_or_else(|| {
        stages
            .iter()
            .find_map(|s| match s {
                Stage::File(_, n) => Some(n.z_ref),
                _ => None,
            })
            .unwrap_or(ld.z_ref)
    });
    // The band all files share.
    let (mut lo, mut hi) = (ld.f[0], *ld.f.last().unwrap());
    for s in &stages {
        if let Stage::File(_, n) = s {
            lo = lo.max(n.f[0]);
            hi = hi.min(*n.f.last().unwrap());
        }
    }
    if hi <= lo {
        return Err("the files share no frequencies".into());
    }
    println!("Load {load} ({} Ω reference in the file)", ld.z_ref);
    for (i, s) in stages.iter().enumerate() {
        match s {
            Stage::File(p, n) => println!("  stage {}: {p} ({} Ω)", i + 1, n.z_ref),
            Stage::Line(z, l, e, db) => println!(
                "  stage {}: line {z} Ω, {l} mil, Eeff {e}, {db} dB/in",
                i + 1
            ),
        }
    }
    println!("Return loss in a {zr} Ω reference, from the first stage's input:");
    println!(
        "{:>9} {:>14} {:>10} {:>18} {:>10}",
        "GHz", "load Z (Ω)", "load dB", "input Z (Ω)", "input dB"
    );
    let dstep = step.unwrap_or(((hi - lo) / 20.0).max(0.01));
    let mut f = lo.max(dstep.min(hi));
    while f <= hi + 1e-9 {
        let zl = ld.impedance(f).ok_or("load outside its band")?;
        let mut z = zl;
        for s in stages.iter().rev() {
            z = match s {
                Stage::File(_, n) => {
                    let m = n.at(f).ok_or("stage outside its band")?;
                    touchstone::input_impedance(&m, n.z_ref, z)
                }
                Stage::Line(zc, l, e, db) => {
                    let m = touchstone::line(*zc, *e, *l, *db, f, zr);
                    touchstone::input_impedance(&m, zr, z)
                }
            };
        }
        let fmt = |z: Cx| format!("{:.1}{:+.1}j", z.re, z.im);
        println!(
            "{f:9.3} {:>14} {:10.2} {:>18} {:10.2}",
            fmt(zl),
            touchstone::z_to_gamma(zl, zr).db(),
            fmt(z),
            touchstone::z_to_gamma(z, zr).db()
        );
        f += dstep;
    }
    Ok(())
}

fn stackup(file: &str, out: Option<&str>) -> Res {
    let b = board(file)?;
    let text = trace_core::stackfile::write(&b.stackup);
    match out {
        Some(o) => {
            std::fs::write(o, &text)?;
            println!("Wrote {o}");
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn validate(rest: &[&str]) -> Res {
    use trace_core::validate;
    let quick = rest.contains(&"--quick");
    let mut c = validate::field_checks();
    c.extend(validate::ir_checks());
    if !quick {
        let t = std::time::Instant::now();
        let last = std::cell::Cell::new(-1i32);
        c.extend(validate::via_checks(&|p| {
            let pc = (p * 100.0) as i32 / 5 * 5;
            if pc > last.get() {
                last.set(pc);
                eprint!("\r  3D checks… {pc}%");
            }
        }));
        eprintln!("\r  3D checks: {:.0} s      ", t.elapsed().as_secs_f64());
    }
    let text = validate::report(&c);
    print!("{text}");
    if let Some(out) = rest.iter().find(|a| !a.starts_with("--")) {
        std::fs::write(out, &text)?;
        println!("Wrote {out}");
    }
    if c.iter().all(|c| c.pass()) {
        Ok(())
    } else {
        Err("some checks failed".into())
    }
}

fn via(file: &str, from: &str, to: &str, rest: &[&str]) -> Res {
    use trace_core::fdtd::{self, ViaSetup};
    let b = board(file)?;
    let geo = b.layer_geometry();
    let layer = |l: &str| -> Result<u8, String> {
        l.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&n| n < geo.len())
            .or_else(|| geo.iter().position(|g| g.name.eq_ignore_ascii_case(l)))
            .map(|n| n as u8)
            .ok_or_else(|| {
                format!(
                    "no copper layer {l}; layers: {}",
                    geo.iter()
                        .map(|g| g.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    };
    let mut v = ViaSetup::new(layer(from)?, layer(to)?);
    if let Some((d, p, a)) = fdtd::board_via(&b) {
        v.drill = d;
        v.pad = p;
        v.antipad = a;
    }
    let mut outs: Vec<&str> = Vec::new();
    let mut optimise: Option<f64> = None;
    let mut free_pitch = false;
    let mut it = rest.iter();
    let len = |x: Option<&&str>, what: &str| -> Result<f64, Box<dyn std::error::Error>> {
        let x = x.ok_or(format!("{what} needs a length"))?;
        Ok(Length::parse(x).ok_or(format!("not a length: {x}"))?.mil())
    };
    while let Some(&a) = it.next() {
        match a {
            "--drill" => v.drill = len(it.next(), a)?,
            "--pad" => v.pad = len(it.next(), a)?,
            "--antipad" => v.antipad = len(it.next(), a)?,
            "--width" => v.w_from = Some(len(it.next(), a)?),
            "--width2" => v.w_to = Some(len(it.next(), a)?),
            "--pair" => v.pair = Some(len(it.next(), a)?),
            "--cell" => v.cell = len(it.next(), a)?,
            "--backdrill" => v.backdrill = true,
            "--oval" => v.antipad_oval = true,
            "--free-pitch" => free_pitch = true,
            "--optimise" | "--optimize" => {
                let x = it.next().ok_or("--optimise needs GHz")?;
                optimise = Some(
                    x.trim_end_matches(['G', 'g', 'H', 'h', 'z'])
                        .parse::<f64>()
                        .map_err(|_| format!("not a frequency: {x}"))?,
                );
            }
            "--returns" => {
                let x = it.next().ok_or("--returns needs N or N@distance")?;
                let (n, d) = x.split_once('@').unwrap_or((x, ""));
                v.returns = n.parse().map_err(|_| format!("not a count: {n}"))?;
                if !d.is_empty() {
                    v.return_dist = Length::parse(d).ok_or(format!("not a length: {d}"))?.mil();
                }
            }
            "--fmax" => {
                let x = it.next().ok_or("--fmax needs GHz")?;
                v.fmax_ghz = x
                    .trim_end_matches(['G', 'g', 'H', 'h', 'z'])
                    .parse()
                    .map_err(|_| format!("not a frequency: {x}"))?;
            }
            _ if !a.starts_with("--") => outs.push(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let names = |l: u8| geo[l as usize].name.clone();
    println!(
        "Via {} to {}{}: drill {:.1} mil, pad {:.1}, antipad {:.1}, {} return via(s) at {:.1} mil{}",
        names(v.from),
        names(v.to),
        v.pair.map_or(String::new(), |p| format!(", pair at {p:.1} mil pitch")),
        v.drill,
        v.pad,
        v.antipad,
        v.returns,
        v.return_dist,
        if v.backdrill { ", backdrilled" } else { "" }
    );
    if let Some(fg) = optimise {
        use trace_core::viaopt;
        let mut goal = viaopt::ViaGoal::around(&v, fg);
        if free_pitch {
            if let Some(p) = v.pair {
                let pad = v.pad.max(v.drill);
                goal.pitch = ((pad + goal.clearance).min(p), (2.5 * pad).max(p));
            }
        }
        let t = std::time::Instant::now();
        let o = viaopt::optimise(&b.stackup, &v, &goal, &|p, what| {
            eprint!("\r  optimising… {:3.0}% {what:<40}", p * 100.0);
        })?;
        eprintln!(
            "\r  optimised in {:.0} s{:50}",
            t.elapsed().as_secs_f64(),
            ""
        );
        println!("Setups tried (worst return loss to {fg} GHz, through loss at {fg} GHz):");
        for tr in &o.trials {
            match &tr.error {
                None => println!(
                    "  {:>7.1} dB {:>7.2} dB  {}",
                    tr.worst_rl,
                    tr.il,
                    viaopt::describe(&tr.setup)
                ),
                Some(e) => println!(
                    "  {:>21}  {}: {e}",
                    "not buildable",
                    viaopt::describe(&tr.setup)
                ),
            }
        }
        let (s0, _) = viaopt::judge(&o.start, fg);
        let (s1, il) = viaopt::judge(&o.best, fg);
        println!(
            "Best: {}\n  worst return loss {s1:.1} dB (was {s0:.1} dB), {il:.2} dB through at {fg} GHz",
            viaopt::describe(&o.best_setup)
        );
        let title = format!("Via {} to {} (optimised)", names(v.from), names(v.to));
        for out in outs {
            let text = if out.to_ascii_lowercase().ends_with(".svg") {
                o.best.svg(&title)
            } else {
                o.best.touchstone(&title)
            };
            std::fs::write(out, text)?;
            println!("Wrote {out}");
        }
        return Ok(());
    }
    let t = std::time::Instant::now();
    let last = std::cell::Cell::new(-1i32);
    let r = fdtd::solve_with(&b.stackup, &v, &|p| {
        let pc = (p * 100.0) as i32 / 10 * 10;
        if pc > last.get() {
            last.set(pc);
            eprint!("\r  solving… {pc}%");
        }
    })?;
    eprintln!(
        "\r  solved: {} cells, {} steps, {:.1} s",
        r.cells,
        r.steps,
        t.elapsed().as_secs_f64()
    );
    println!(
        "Traces: {:.2} mil ({:.1} ohm) in, {:.2} mil ({:.1} ohm) out; stub {:.1} mil{}",
        r.w[0],
        r.z0[0],
        r.w[1],
        r.z0[1],
        r.stub,
        if r.stub > 0.5 {
            format!(
                " (bare quarter-wave {:.1} GHz)",
                fdtd::stub_resonance_ghz(r.stub, 3.8)
            )
        } else {
            String::new()
        }
    );
    let kind = if r.z_ref > 60.0 { "Sdd" } else { "S" };
    println!(
        "  {:>7}  {:>9}  {:>9}  {:>9}",
        "GHz",
        format!("{kind}11 dB"),
        format!("{kind}21 dB"),
        "deg"
    );
    let n = r.f.len();
    for k in (0..n).filter(|k| (k + 1) % (n / 16).max(1) == 0) {
        println!(
            "  {:>7.2}  {:>9.2}  {:>9.3}  {:>9.1}",
            r.f[k],
            r.s11[k].db(),
            r.s21[k].db(),
            r.s21[k].deg()
        );
    }
    if let Some((f, db)) = r.notch() {
        println!("Stub resonance: {kind}21 dips to {db:.1} dB at {f:.2} GHz");
    }
    let worst = (0..n)
        .max_by(|&a, &b| r.s11[a].abs().total_cmp(&r.s11[b].abs()))
        .unwrap_or(0);
    println!(
        "Worst return loss {:.1} dB at {:.2} GHz",
        r.s11[worst].db(),
        r.f[worst]
    );
    for note in &r.notes {
        println!("Note: {note}");
    }
    let title = format!(
        "{} via, {} to {}",
        Path::new(file)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned()),
        names(v.from),
        names(v.to)
    );
    for o in outs {
        let low = o.to_ascii_lowercase();
        if low.ends_with(".svg") {
            std::fs::write(o, r.svg(&title))?;
        } else {
            std::fs::write(o, r.touchstone(&title))?;
        }
        println!("Saved {o}");
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

/// The assembly drawing as a PDF.
fn assembly(file: &str, out: &str, paper: trace_export::Paper) -> Res {
    let b = board(file)?;
    let title = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let pdf = trace_export::assembly_pdf(&b, &title, paper);
    std::fs::write(out, &pdf)?;
    let side = |s| b.components.iter().filter(|c| c.side == s).count();
    println!(
        "Wrote {out}: {} parts on top, {} on the bottom, {} KB",
        side(trace_core::Side::Top),
        side(trace_core::Side::Bottom),
        pdf.len().div_ceil(1024)
    );
    Ok(())
}

/// The board and its parts as a STEP model.
fn step_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    let mut opt = trace_export::StepOptions::default();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--vias" => opt.vias = true,
            "--no-parts" => opt.parts = false,
            "--min-hole" => {
                i += 1;
                let v = rest.get(i).ok_or("--min-hole needs a size in mm")?;
                opt.min_hole = v
                    .trim_end_matches("mm")
                    .parse()
                    .map_err(|_| format!("bad --min-hole {v}"))?;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let b = board(file)?;
    let name = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let (text, r) = trace_export::to_step(&b, &name, &opt)?;
    std::fs::write(out, &text)?;
    println!(
        "Wrote {out}: board {:.2} mm thick, {} holes, {} cut-outs, {} parts, {} KB",
        r.thickness,
        r.holes,
        r.cutouts,
        r.parts,
        text.len().div_ceil(1024)
    );
    if r.skipped > 0 {
        println!(
            "  {} holes left out: they cross the board edge, a cut-out or another hole",
            r.skipped
        );
    }
    Ok(())
}

/// The fabrication drawing as a PDF.
fn fab_drawing(file: &str, out: &str, paper: trace_export::Paper) -> Res {
    let b = board(file)?;
    let title = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let pdf = trace_export::fab_drawing_pdf(&b, &title, paper);
    std::fs::write(out, &pdf)?;
    let holes = trace_export::fabdraw::drill_table(&b);
    println!(
        "Wrote {out}: {} hole sizes, {} holes, {} KB",
        holes.len(),
        holes.iter().map(|d| d.count).sum::<usize>(),
        pdf.len().div_ceil(1024)
    );
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
    let lower = out.to_ascii_lowercase();
    if lower.ends_with(".step") || lower.ends_with(".stp") {
        return step_cmd(file, out, &[]);
    }
    if out.to_ascii_lowercase().ends_with(".xml") || out.to_ascii_lowercase().ends_with(".cvg") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as IPC-2581".into());
        };
        let name = Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "board".into());
        let (text, n) = trace_export::to_ipc2581(&b, &name);
        std::fs::write(out, text)?;
        println!("wrote {out}: IPC-2581 revision C, {n} features");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".dxf") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .dxf".into());
        };
        let (text, n) = trace_export::to_dxf(&b);
        std::fs::write(out, text)?;
        println!("wrote {out}: {n} objects");
        return Ok(());
    }
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

/// Options shared by `trace stitch` and `trace fence`.
struct ViaOpts {
    via: trace_core::stitch::ViaSpec,
    rules: trace_core::stitch::Rules,
}

fn layer_of(b: &Board, l: &str) -> Result<u8, String> {
    let geo = b.layer_geometry();
    l.parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&n| n < geo.len())
        .or_else(|| geo.iter().position(|g| g.name.eq_ignore_ascii_case(l)))
        .map(|n| n as u8)
        .ok_or_else(|| {
            format!(
                "no copper layer {l}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn mil_of(v: &str) -> Result<f64, String> {
    Length::parse(v)
        .map(|l| l.mil())
        .ok_or(format!("not a length: {v}"))
}

/// Reads the via and rule options it knows; returns the rest.
fn via_opts<'a>(b: &Board, rest: &[&'a str]) -> Result<(ViaOpts, Vec<&'a str>), String> {
    use trace_core::stitch::{Rules, ViaSpec};
    let mut o = ViaOpts {
        via: ViaSpec::of(b),
        rules: Rules::of(b),
    };
    let mut left = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--via" => {
                let v = val()?;
                let (p, d) = v
                    .split_once('/')
                    .ok_or("--via is pad/drill, such as 16/8 or 0.4mm/0.2mm")?;
                o.via.pad = mil_of(p)?;
                o.via.drill = mil_of(d)?;
                i += 1;
            }
            "--layers" => {
                let v = val()?;
                let (f, t) = v
                    .split_once('-')
                    .ok_or("--layers is from-to, such as 1-4 or TOP-BOTTOM")?;
                let (f, t) = (layer_of(b, f)?, layer_of(b, t)?);
                o.via.from = f.min(t);
                o.via.to = f.max(t);
                i += 1;
            }
            "--clearance" => {
                o.rules.clearance = mil_of(val()?)?;
                i += 1;
            }
            "--edge" => {
                o.rules.edge = mil_of(val()?)?;
                i += 1;
            }
            _ => left.push(a),
        }
        i += 1;
    }
    Ok((o, left))
}

fn net_of(b: &Board, name: &str) -> Result<u32, String> {
    trace_core::stitch::net_named(b, name).ok_or(format!("no net {name}"))
}

/// Applies the vias, pours touched pours again and saves.
fn save_with(
    mut b: Board,
    placed: &trace_core::stitch::Placed,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let edits = placed.edits(&b);
    for e in &edits {
        e.apply(&mut b);
    }
    if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
        e.apply(&mut b);
    }
    trace_core::connectivity::analyse(&mut b);
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        std::fs::write(out, trace_export::to_kicad(&b))?;
    } else {
        trace_core::file::save(&b, Path::new(out))?;
    }
    Ok(())
}

fn stitch_cmd(file: &str, net: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::stitch::{stitch, StitchGoal};
    let b = board(file)?;
    let (o, rest) = via_opts(&b, rest)?;
    let mut goal = StitchGoal {
        net: net_of(&b, net)?,
        via: o.via,
        rules: o.rules,
        grid: 100.0,
        stagger: false,
        area: None,
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--grid" => {
                goal.grid = mil_of(val()?)?;
                i += 1;
            }
            "--stagger" => goal.stagger = true,
            "--area" => {
                let v: Vec<f64> = val()?.split(',').map(mil_of).collect::<Result<_, _>>()?;
                if v.len() != 4 {
                    return Err("--area is x0,y0,x1,y1".into());
                }
                goal.area = Some([v[0], v[1], v[2], v[3]]);
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let p = stitch(&b, &goal)?;
    println!(
        "{}: {:.1}/{:.1} mil vias, layers {}-{}, every {:.0} mil{}",
        b.nets[goal.net as usize].name,
        goal.via.pad,
        goal.via.drill,
        goal.via.from + 1,
        goal.via.to + 1,
        goal.grid,
        if goal.stagger { ", staggered" } else { "" }
    );
    println!("{}", p.summary());
    save_with(b, &p, out)?;
    println!("saved {out}");
    Ok(())
}

fn fence_cmd(file: &str, nets: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::stitch::{fence, fence_pitch, ground_net, FenceGoal};
    let b = board(file)?;
    let (o, rest) = via_opts(&b, rest)?;
    let nets: Vec<u32> = nets
        .split(',')
        .map(|n| net_of(&b, n.trim()))
        .collect::<Result<_, _>>()?;
    let mut ground = None;
    let (mut ghz, mut pitch, mut gap, mut rows) = (6.0, None, None, 1);
    let mut on: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--ground" => ground = Some(net_of(&b, val()?)?),
            "--ghz" => {
                ghz = val()?
                    .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse()
                    .map_err(|_| "--ghz takes a number")?
            }
            "--pitch" => pitch = Some(mil_of(val()?)?),
            "--gap" => gap = Some(mil_of(val()?)?),
            "--rows" => rows = val()?.parse().map_err(|_| "--rows takes a number")?,
            "--on" => {
                on = val()?
                    .split(',')
                    .map(|l| layer_of(&b, l.trim()))
                    .collect::<Result<_, _>>()?
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 2;
    }
    let ground = ground
        .or_else(|| ground_net(&b))
        .ok_or("no ground net found; give --ground")?;
    // The highest effective Er the fenced tracks see sets the wavelength.
    let er = trace_core::stitch::fence_eeff(&b, &nets, &on);
    let mut rules = o.rules;
    // A fence via need only reach the ground plane.
    rules.min_layers = 1;
    let goal = FenceGoal {
        nets: nets.clone(),
        ground,
        pitch: pitch.unwrap_or_else(|| fence_pitch(ghz, er)),
        gap: gap.unwrap_or(rules.clearance),
        rows,
        layers: on,
        via: o.via,
        rules,
    };
    let p = fence(&b, &goal)?;
    println!(
        "fence of {} vias ({:.1}/{:.1} mil) beside {}: every {:.1} mil{}, {:.1} mil from the track edge",
        b.nets[ground as usize].name,
        goal.via.pad,
        goal.via.drill,
        nets.iter()
            .map(|&n| b.nets[n as usize].name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        goal.pitch,
        if pitch.is_none() {
            format!(" (a twentieth of a wavelength at {ghz} GHz, effective Er {er:.2})")
        } else {
            String::new()
        },
        goal.gap.max(goal.rules.clearance)
    );
    println!("{}", p.summary());
    save_with(b, &p, out)?;
    println!("saved {out}");
    Ok(())
}

fn nets_list(b: &Board, v: &str) -> Result<Vec<u32>, String> {
    v.split(',').map(|n| net_of(b, n.trim())).collect()
}

/// Applies edits, pours touched pours again and saves.
fn save_edits(
    mut b: Board,
    edits: Vec<trace_core::edit::Edit>,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for e in &edits {
        e.apply(&mut b);
    }
    if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
        e.apply(&mut b);
    }
    trace_core::connectivity::analyse(&mut b);
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        std::fs::write(out, trace_export::to_kicad(&b))?;
    } else {
        trace_core::file::save(&b, Path::new(out))?;
    }
    Ok(())
}

fn teardrops_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::teardrop::{add, remove, TeardropGoal};
    let b = board(file)?;
    let mut goal = TeardropGoal::of(&b);
    let mut take_off = false;
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        let num = |v: &str| v.parse::<f64>().map_err(|_| format!("not a number: {v}"));
        match a {
            "--nets" => {
                goal.nets = nets_list(&b, val()?)?;
                i += 1;
            }
            "--smd" => goal.smd_pads = true,
            "--no-vias" => goal.vias = false,
            "--no-th" => goal.th_pads = false,
            "--length" => {
                goal.length = num(val()?)?;
                i += 1;
            }
            "--width" => {
                goal.width = num(val()?)?.clamp(0.1, 1.0);
                i += 1;
            }
            "--clearance" => {
                goal.clearance = mil_of(val()?)?;
                i += 1;
            }
            "--remove" => take_off = true,
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    if take_off {
        let n = b.geometry.polygons.iter().filter(|p| p.teardrop).count();
        let edits: Vec<_> = remove(&b).into_iter().collect();
        println!("{n} teardrops removed");
        save_edits(b, edits, out)?;
    } else {
        let t = add(&b, &goal);
        println!("{}", t.summary());
        let edits = t.edits(&b);
        save_edits(b, edits, out)?;
    }
    println!("saved {out}");
    Ok(())
}

/// Sheets with their file names, and the Trace schematic they came from.
type Sheets = (
    Vec<(String, trace_core::Sheet)>,
    Option<trace_core::SchFile>,
);

/// The sheets of a `.SchDoc`, a `.PrjPcb` or a Trace schematic, with
/// their file names and, for a Trace schematic, the file itself.
fn sheets_of(file: &str) -> Result<Sheets, Box<dyn std::error::Error>> {
    let p = Path::new(file);
    let name = |q: &Path| {
        q.file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match p
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("tracesch") => {
            let f = trace_core::SchFile::load(p)?;
            let v = f
                .sheets
                .iter()
                .map(|s| (s.file.clone(), s.sheet.clone()))
                .collect();
            Ok((v, Some(f)))
        }
        Some("prjpcb") => {
            let dir = p.parent().unwrap_or(Path::new("."));
            let text = String::from_utf8_lossy(&std::fs::read(p)?).into_owned();
            let mut v = Vec::new();
            for l in text.lines() {
                let Some(doc) = l.trim().strip_prefix("DocumentPath=") else {
                    continue;
                };
                let q = dir.join(doc.replace('\\', std::path::MAIN_SEPARATOR_STR));
                if q.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("schdoc"))
                    && q.is_file()
                {
                    v.push((name(&q), trace_altium::read_schdoc(&q)?));
                }
            }
            if v.is_empty() {
                return Err(format!("{file} lists no schematic sheets that exist").into());
            }
            Ok((v, None))
        }
        Some("schdoc") => Ok((vec![(name(p), trace_altium::read_schdoc(p)?)], None)),
        _ => Err(format!("{file}: expected a .SchDoc, .PrjPcb or .tracesch").into()),
    }
}

fn swap_cmd(file: &str, part: &str, a: &str, b: &str, out: &str) -> Res {
    let b0 = board(file)?;
    let i = b0
        .components
        .iter()
        .position(|c| c.designator.eq_ignore_ascii_case(part))
        .ok_or(format!("no part {part}"))?;
    let s = trace_core::swap::swap_pads(&b0, &[((i, a.to_string()), (i, b.to_string()))])?;
    println!("{}", s.summary(&b0));
    save_edits(b0, s.edits, out)?;
    println!("saved {out}");
    Ok(())
}

fn erc_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::erc::{check, summary, Level};
    let all = rest.contains(&"--all");
    if let Some(o) = rest.iter().find(|a| **a != "--all") {
        return Err(format!("unknown option {o}").into());
    }
    let t = std::time::Instant::now();
    let (sheets, _) = sheets_of(file)?;
    let refs: Vec<&trace_core::Sheet> = sheets.iter().map(|s| &s.1).collect();
    let v = check(&refs);
    let shown = if all { v.len() } else { v.len().min(60) };
    for i in &v[..shown] {
        println!(
            "{:7}  {:24}  {:>8.0} {:>8.0}  {}",
            if i.level == Level::Error {
                "error"
            } else {
                "warning"
            },
            sheets[i.sheet].0,
            i.at[0],
            i.at[1],
            i.text
        );
    }
    if shown < v.len() {
        println!("  ... {} more (--all lists them)", v.len() - shown);
    }
    println!(
        "{} sheets: {} in {:.2} s",
        sheets.len(),
        summary(&v),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

fn annotate_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::annotate::{annotate, apply, AnnotateGoal, Order};
    let mut goal = AnnotateGoal::default();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--all" => goal.all = true,
            "--down" => goal.order = Order::DownThenAcross,
            "--start" => {
                goal.start = val()?.parse().map_err(|_| "--start takes a whole number")?;
                i += 1;
            }
            "--per-sheet" => {
                goal.sheet_step = Some(
                    val()?
                        .parse()
                        .map_err(|_| "--per-sheet takes a whole number")?,
                );
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    if !out.to_ascii_lowercase().ends_with(".tracesch") {
        return Err("the annotated sheets are saved as a Trace schematic (.tracesch)".into());
    }
    let (mut sheets, saved) = sheets_of(file)?;
    let refs: Vec<&trace_core::Sheet> = sheets.iter().map(|s| &s.1).collect();
    let r = annotate(&refs, &goal);
    for x in r.iter().take(40) {
        println!("{:24}  {} -> {}", sheets[x.sheet].0, x.from, x.to);
    }
    if r.len() > 40 {
        println!("  ... {} more", r.len() - 40);
    }
    let before: Vec<trace_core::schematic::Drawing> = sheets
        .iter()
        .map(|s| trace_core::schematic::Drawing {
            images: Vec::new(),
            ..s.1.drawing.clone()
        })
        .collect();
    {
        let mut m: Vec<&mut trace_core::Sheet> = sheets.iter_mut().map(|s| &mut s.1).collect();
        apply(&mut m, &r);
    }
    let mut f = saved.unwrap_or_else(|| trace_core::SchFile {
        project: Path::new(file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("prjpcb"))
            .then(|| {
                Path::new(file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .flatten(),
        sheets: sheets
            .iter()
            .zip(&before)
            .map(|((name, sh), d)| trace_core::schematic::SavedSheet {
                file: name.clone(),
                sheet: sh.clone(),
                synced: Some(d.clone()),
                original: Some(d.clone()),
            })
            .collect(),
        ..Default::default()
    });
    f.version = env!("CARGO_PKG_VERSION").to_string();
    for (s, (_, sh)) in f.sheets.iter_mut().zip(&sheets) {
        s.sheet = sh.clone();
    }
    // Numbered parts renamed: now -> as on the board.
    let renamed = trace_core::annotate::component_renames(&r);
    for map in [&mut f.renames, &mut f.original_renames] {
        let was: Vec<(String, String)> = renamed
            .iter()
            .map(|(from, to)| {
                let up = from.to_ascii_uppercase();
                (to.to_ascii_uppercase(), map.get(&up).cloned().unwrap_or(up))
            })
            .collect();
        for from in renamed.keys() {
            map.remove(&from.to_ascii_uppercase());
        }
        map.extend(was);
    }
    f.save(Path::new(out))?;
    println!("{} parts renamed; saved {out}", r.len());
    Ok(())
}

fn fanout_cmd(file: &str, parts: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::fanout::{fanout, Direction, FanoutGoal, Style};
    let b = board(file)?;
    let mut idx = Vec::new();
    for d in parts.split(',').map(str::trim).filter(|d| !d.is_empty()) {
        let i = b
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?;
        idx.push(i);
    }
    let mut goal = FanoutGoal::of(&b, idx);
    let (o, left) = via_opts(&b, rest)?;
    goal.via = o.via;
    goal.clearance = o.rules.clearance;
    let mut i = 0;
    while i < left.len() {
        let a = left[i];
        let val = || left.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--in-pad" => goal.style = Style::InPad,
            "--inward" => goal.direction = Direction::In,
            "--no-net" => goal.no_net = true,
            "--skip-rows" => {
                goal.skip_rows = val()?
                    .parse()
                    .map_err(|_| "--skip-rows takes a whole number")?;
                i += 1;
            }
            "--width" => {
                goal.width = Some(mil_of(val()?)?);
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let f = fanout(&b, &goal);
    println!("{}", f.summary());
    let edits = f.edits(&b);
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}

fn gloss_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::gloss::{gloss, GlossGoal};
    let b = board(file)?;
    let mut goal = GlossGoal::of(&b);
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--nets" => {
                goal.nets = nets_list(&b, val()?)?;
                i += 1;
            }
            "--no-merge" => goal.merge = false,
            "--keep-dangling" => goal.dangling = false,
            "--corners" => goal.corners = true,
            "--clearance" => {
                goal.clearance = mil_of(val()?)?;
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let g = gloss(&b, &goal);
    println!("{}", g.summary());
    let edits = g.edits(&b);
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}
