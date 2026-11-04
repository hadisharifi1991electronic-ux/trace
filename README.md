# Trace

An open-source schematic and PCB design tool, in the spirit of KiCad, aimed at
Allegro-class high-speed design: one constraint manager that the router,
checker and simulator all read.

> Status: phase 1, day one. There is no editor yet. What exists is the core
> data model and an Altium importer, driven from a command-line tool.

## What works today

```
trace inspect board.PcbDoc       stack-up, components, nets, classes, rules, primitive counts
trace inspect sheet.SchDoc       symbols with designators, values and pin counts; labels; child sheets
trace pairs board.PcbDoc [5mil]  routed length of each differential pair net and the P/N skew
trace rules board.PcbDoc         design rules grouped by kind
trace convert <file> out.json    export to Trace's open JSON model
```

Tested on an 18-layer board with 2,370 parts, 2,252 nets and 137 differential
pairs: it reads in about 0.3 s.

## Limits to know

- Pair lengths count copper tracks and arcs only. Pad-to-pad length, via
  barrels and package delays are not included yet, and a partly routed net
  looks short rather than unrouted.
- Schematic pins stored as binary records are counted against the previous
  symbol; the text form is exact.
- No geometry (pads, polygons, outlines) is imported yet, only counted.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/trace-core` | Format-neutral design model (lengths in nanometres) |
| `crates/trace-altium` | Altium `.PcbDoc` / `.SchDoc` reader (OLE compound files) |
| `crates/trace-cli` | The `trace` command |

## Build

Install Rust from https://rustup.rs, then:

```
cargo build --release
target\release\trace.exe inspect path\to\board.PcbDoc     (Windows)
./target/release/trace inspect path/to/board.PcbDoc       (Linux, macOS)
```

## Licence

GPL-3.0-or-later, the same licence family as KiCad.
