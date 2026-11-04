# Trace

An open-source schematic and PCB design tool, in the spirit of KiCad, aimed at
Allegro-class high-speed design: one constraint manager that the router,
checker and simulator all read.

> Status: phase 1, board viewer. Trace opens Altium `.PcbDoc` and Cadence
> Allegro `.brd` boards in a desktop app, measures distances and checks
> routing and differential pairs. Editing is paused.

## Desktop app

`trace-app` (or `trace-app.exe` on Windows) opens a window where you can:

- open any Altium `.PcbDoc` or Allegro `.brd` from File → Open, the recent
  list, or by dropping it on the window
- see tracks, pads, vias, copper pours, silkscreen and the board outline
- zoom (scroll), pan (drag), flip to the bottom (B), fit (F)
- show or hide each copper layer; double-click a layer to show only that one
- measure (M): click two points to get the distance and the x/y offsets in
  mil and mm; clicks snap to pad and via centres and track ends
- click a track, pad, via or pour to select its net and see its length, via
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

Every command also takes an Allegro `.brd` (versions 16.0 to 23.x).

Tested on an 18-layer Altium board with 2,370 parts, 2,252 nets and 137 differential
pairs: it reads in about 0.3 s. A 22-layer Allegro board with 2,079 parts and
1.07 million records reads in about 0.7 s.

## Limits to know

- Lengths include tracks, arcs and via barrels between the layers used, but
  not package pin delays. Nets joined through copper pours are not checked
  for completeness yet.
- Schematic pins stored as binary records are counted against the previous
  symbol; the text form is exact.
- Altium copper pours and regions are counted but not drawn yet; Allegro
  pours are drawn.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/trace-core` | Format-neutral design model (lengths in nanometres) |
| `crates/trace-altium` | Altium `.PcbDoc` / `.SchDoc` reader (OLE compound files) |
| `crates/trace-allegro` | Allegro `.brd` reader, ported from KiCad's Allegro importer |
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

GPL-3.0-or-later, the same licence family as KiCad. The Allegro reader is
based on the GPL Allegro importer in KiCad's pcbnew; credit to the KiCad
developers for working out the format.
