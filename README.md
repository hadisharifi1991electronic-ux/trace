# Trace

An open-source schematic and PCB design tool, in the spirit of KiCad, aimed at
Allegro-class high-speed design: one constraint manager that the router,
checker and simulator all read.

> Status: phase 1, board viewer. Trace opens Altium `.PcbDoc` and Cadence
> Allegro `.brd` boards in a desktop app, measures distances and checks
> routing and differential pairs. It also rebuilds a board from Gerber and
> drill files or an ODB++ job. Editing is paused.

## Desktop app

`trace-app` (or `trace-app.exe` on Windows) opens a window where you can:

- open any Altium `.PcbDoc` or Allegro `.brd` from File → Open, the recent
  list, or by dropping it on the window
- read the schematic (the Schematic button, or S): open an Altium `.PrjPcb`
  for all its sheets and its board, or a single `.SchDoc`. Opening a board
  also brings in its project's schematic. Click a wire or pin to select its
  net on the board too; on the board, "Show in schematic" jumps to the sheet
  with the selected net, and the sheet list marks every sheet that has it.
  Double-click a sheet symbol to open its sheet
- open fabrication data: a folder or `.zip` of Gerber and drill files, several
  files at once, or an ODB++ job (`.tgz`, `.zip` or folder). Each file's layer
  is recognised automatically and the board is rebuilt with copper, holes,
  vias, silkscreen, outline and nets; "Rebuilt from N files" shows what each
  file was taken as and why. File → Save as Trace board keeps the result
- save any open board as a KiCad board (File → Save as KiCad board), to keep
  working on it in KiCad 7 or later, or as an Altium board (File → Save as
  Altium board), which writes a `.PcbDoc`
- see tracks, pads, vias, copper pours, silkscreen and the board outline
- turn the board in 3D (the 3D button, or 3): solder mask, exposed pads,
  silkscreen, holes and the parts: Altium boards show the 3D (STEP) models
  stored in the file, loaded in the background; other parts get a simple
  body sized from their pads. Drag to turn, right-drag to pan, B to flip
  over, choice of mask colour
- see each via's hole, layer span (such as 1-4) and net, coloured by span,
  and net names on tracks and pads as you zoom in
- zoom (scroll), pan (drag), flip to the bottom (B), fit (F)
- show or hide each copper layer; double-click a layer to show only that one
- measure (M): click two points to get the distance and the x/y offsets in
  mil and mm; clicks snap to pad and via centres and track ends
- click a track, pad, via or pour to select its net and see its length, via
  barrels, routing state and pair skew; or search a net or part by name
- review every differential pair in the bottom panel, sorted by problem,
  click one to zoom to it, and export the list to CSV
- set length rules in the Constraint Manager (the Constraints button, or C),
  in the style of Allegro's: groups of nets (all nets, a net class, a pair
  class, a name pattern such as `DDR_DQ*`, or a list) with a minimum and
  maximum length, a match tolerance against the longest member, a maximum
  pair skew and a maximum via count. Altium boards start with their own
  Length and Matched Length rules. Every net is checked as you edit, failing
  nets are listed with how far off they are, and clicking one selects it on
  the board. Your groups are kept per board in Trace's settings; the design
  file is never changed
- route tracks (the Route button, or X): click a pad, via or track end to
  start, click to place 45° corners, V / Shift+V for a via to the next
  layer down / up, Backspace to take back a corner, and the route ends on
  copper of its own net. A route that breaks clearance to another net is
  drawn red and is not placed. Ctrl+Z / Ctrl+Y undo and redo; Delete
  removes the clicked track or via; Shift+Delete unroutes the selected net
- check impedance (the Impedance button, or I): the board's stack-up with
  editable thickness and Er per layer, the trace width for the single-ended
  target and the width and gap for the differential target on each copper
  layer, the delay in ps/inch, a width and gap calculator, and the Z0 or
  Zdiff of every routed net and pair against its target. Per-net rules set
  other targets by name pattern (USB pairs start at 90 Ω, PCIe at 85 Ω).
  Stack-up edits and rules are kept per board in Trace's settings

## Command-line tool

```
trace inspect board.PcbDoc       stack-up, components, nets, classes, rules, primitive counts
trace inspect sheet.SchDoc       symbols with designators, values and pin counts; labels; child sheets
trace pairs board.PcbDoc [5mil]  length of each differential pair (tracks, arcs, via barrels), skew, and open nets
trace view board.PcbDoc          open the board viewer in your browser
trace rules board.PcbDoc         design rules grouped by kind
trace check board.PcbDoc         check the board's length and matched-length rules; lists failing nets
trace impedance board.PcbDoc     stack-up widths for 50 Ω and 100 Ω pairs, and each pair's Zdiff
trace convert <file> out.json    export to Trace's open JSON model
trace convert <file> out.kicad_pcb   write a KiCad board
trace convert <file> out.PcbDoc      write an Altium Designer board
trace convert sheet.SchDoc out.svg   draw a schematic sheet as SVG
```

Every command also takes an Allegro `.brd` (versions 16.0 to 23.x), a Gerber
folder or `.zip`, or an ODB++ job.

```
trace layers gerbers.zip         which layer each file was taken as, and why
```

Gerber import reads RS-274X with X2 attributes, aperture macros, step and
repeat and block apertures, plus Excellon drill files with slots. Layers are
recognised from X2 file functions, Altium layer comments, the extension
conventions of Altium/Protel, Eagle and OrCAD, KiCad file names, and finally
the content itself. Without X2 net attributes, nets are traced through the
copper: on six KiCad demo boards (about 19,000 objects) the traced nets match
KiCad's own netlist exactly. When an IPC-356 netlist comes with the
Gerbers, as Allegro and most other tools write one, it names the nets and
parts. Allegro artwork and drill files are recognised from Allegro's own
layer comments and tool tables. ODB++ jobs take their nets and parts from
the job's netlist and component files.

KiCad export writes copper, pads, vias (including blind and buried), pours,
silkscreen and the outline, with nets and parts. Pads and vias whose copper
changes from layer to layer keep that copper. To check it, Gerbers plotted
by KiCad from the exported board were traced again: the nets match the
source on 15 boards from Gerber, ODB++ and Allegro files, including a
16-layer board with 25,700 blind and buried vias.

Altium export writes a `.PcbDoc` with the layer stack, outline, nets, parts,
tracks, arcs, vias and pads (full-stack where the copper differs by layer),
and pours as copper regions. Each exported board was read back with Trace's
own Altium reader and matched the source on 12 boards; opening the files in
Altium Designer itself has not been checked yet. Designators, classes and
rules are not written.

Tested on an 18-layer Altium board with 2,370 parts, 2,252 nets and 137 differential
pairs: it reads in about 0.3 s. A 22-layer Allegro board with 2,079 parts and
1.07 million records reads in about 0.7 s.

## Limits to know

- Lengths include tracks, arcs and via barrels between the layers used, but
  not package pin delays. Nets joined through copper pours are not checked
  for completeness yet.
- Routing checks clearance to tracks, pads, vias and drill holes of other
  nets, but not to copper pours, and pours are not re-poured after edits.
  Vias are through vias. There is no push-and-shove or walk-around yet.
- Impedance uses closed-form models (Hammerstad–Jensen microstrip, Wheeler
  stripline, IPC-2141 coupling). Every copper layer is taken as a reference
  plane and solder mask is ignored, so expect a few percent against a field
  solver such as Polar Si9000.
- Schematic pins stored as binary records are given to the part before
  them; the text form is exact. The schematic viewer uses one font for all
  text, so Altium's bold, italic and font faces are not shown. Signal
  harnesses are drawn as lines without their connectors' labels.
- Schematic nets are joined within a sheet (wires, labels, power ports,
  ports); across sheets they are matched to the board through part pins.
- Altium solid regions are drawn; Altium polygon pours are counted but not
  drawn yet. Allegro pours are drawn.
- Traced Gerber nets get names like `Net-042`, since bare Gerber files do not
  carry net names. Parts are only known when the files carry X2 component
  attributes or come from ODB++.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/trace-core` | Format-neutral design model (lengths in nanometres) |
| `crates/trace-altium` | Altium `.PcbDoc` / `.SchDoc` reader (OLE compound files) |
| `crates/trace-allegro` | Allegro `.brd` reader, ported from KiCad's Allegro importer |
| `crates/trace-gerber` | Gerber, Excellon, IPC-356 and ODB++ import with layer detection and net tracing |
| `crates/trace-export` | Writes boards in other tools' formats (KiCad, Altium) |
| `crates/trace-step` | STEP part models to coloured triangle meshes (geometry via the `truck` crates) |
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
