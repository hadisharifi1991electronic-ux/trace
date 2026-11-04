# Trace

An open-source schematic and PCB design tool, in the spirit of KiCad, aimed at
Allegro-class high-speed design: one constraint manager that the router,
checker and simulator all read.

> Status: phase 1. Trace opens Altium boards in a desktop app and checks
> routing and differential pairs. Editing is not there yet.

## Desktop app

`trace-app` (or `trace-app.exe` on Windows) opens a window where you can:

- open any `.PcbDoc` from File → Open, the recent list, or by dropping it on the window
- zoom (scroll), pan (drag), flip to the bottom (B), fit (F)
- show or hide each copper layer; double-click a layer to show only that one
- click a track, pad or via to select its net and see its length, via
  barrels, routing state and pair skew; or search a net or part by name
- review every differential pair in the bottom panel, sorted by problem,
  click one to zoom to it, and export the list to CSV

## Command-line tool

```
trace inspect board.PcbDoc       stack-up, components, nets, classes, rules, primitive counts
trace inspect sheet.SchDoc       symbols with designators, values and pin counts; labels; child sheets
trace pairs board.PcbDoc [5mil]  length of each differential pair (tracks, arcs, via barrels), skew, and open nets
trace view board.PcbDoc          open the board viewer in your browser
trace rules board.PcbDoc         design rules grouped by kind
trace convert <file> out.json    export to Trace's open JSON model
```

Tested on an 18-layer board with 2,370 parts, 2,252 nets and 137 differential
pairs: it reads in about 0.3 s.

## Limits to know

- Lengths include tracks, arcs and via barrels between the layers used, but
  not package pin delays. Nets joined through copper pours are not checked
  for completeness yet.
- Schematic pins stored as binary records are counted against the previous
  symbol; the text form is exact.
- Copper pours and regions are counted but not imported yet.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/trace-core` | Format-neutral design model (lengths in nanometres) |
| `crates/trace-altium` | Altium `.PcbDoc` / `.SchDoc` reader (OLE compound files) |
| `crates/trace-cli` | The `trace` command |
| `crates/trace-app` | The desktop app (egui) |

## Build

Install Rust from https://rustup.rs, then:

```
cargo build --release
target\release\trace.exe inspect path\to\board.PcbDoc     (Windows)
./target/release/trace inspect path/to/board.PcbDoc       (Linux, macOS)
```

## Licence

GPL-3.0-or-later, the same licence family as KiCad.
