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

- open any Altium `.PcbDoc` (binary or ASCII) or Allegro `.brd` from File → Open, the recent
  list, or by dropping it on the window
- read the schematic (the Schematic button, or S): open an Altium `.PrjPcb`
  for all its sheets and its board, or a single `.SchDoc`. Opening a board
  also brings in its project's schematic. Click a wire or pin to select its
  net on the board too; on the board, "Show in schematic" jumps to the sheet
  with the selected net, and the sheet list marks every sheet that has it.
  Double-click a sheet symbol to open its sheet
- edit the schematic: drag parts (their wires follow), Space turns, Delete
  removes; W draws wires (click the corners, end on a pin or wire), N
  places net labels; change a part's designator or value in the side
  panel; Ctrl+Z / Ctrl+Y undo and redo. Save (Ctrl+S) writes a Trace
  schematic (`.tracesch`) next to the project, which opening the
  `.PrjPcb` picks up again. Update PCB… lists what the edits change on the
  board (pins moving between nets, renamed nets and parts) and carries
  them over in one undoable step; pours are poured again
- place new parts from symbol libraries: Place symbol… (or P) opens an
  Altium `.SchLib` (the libraries an Altium project lists open by
  themselves, and opened ones are remembered). Find a symbol by name or
  description, see it drawn with its pins and footprint, and Place it: it
  follows the pointer until a click drops it (Space turns it, Esc takes it
  back). It gets the next free designator; a later unit of a multi-part
  part (U1B) joins the first placed part still missing it. Update PCB adds
  it to the board when its footprint is on the board or in the open
  footprint library
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
  maximum length, a match tolerance, a maximum pair skew and a maximum via
  count. A matched group matches its members to the longest one, to a
  member you pick, or to a set value; by length, or by propagation delay
  in ps as Allegro's relative propagation delay does (each layer at its
  own speed, so a track on an outer layer counts as faster than the same
  length inside); and each differential pair can count as one member.
  The tuner (T) tunes to the group's target, delay included, and failing
  nets also show in the design rule check list (Length). Altium boards start with their own
  Length and Matched Length rules. Every net is checked as you edit, failing
  nets are listed with how far off they are, and clicking one selects it on
  the board. Your groups are kept per board in Trace's settings; the design
  file is never changed
- route tracks (the Route button, or X): click a pad, via or track end to
  start, click to place corners (Shift+Space cycles 45°, 45° arc, 90°,
  90° arc and any angle; Space flips the bend), V / Shift+V for a via to the next
  layer down / up (the Via list on the route bar: Auto takes the
  narrowest via the board uses that joins the two layers, stacking
  microvias and buried vias end to end where needed; or pick a through,
  blind, buried or micro via, each with its own size, and + adds a span
  the board does not use yet), Backspace to take back a corner or a
  whole via stack, and the route ends on
  copper of its own net. Obstacles are handled as in Altium (Shift+R or
  the list on the route bar): Walkaround hugs what is in the way at 45°
  and stops short when there is no way round; Push and walkaround moves
  tracks of other nets aside first (shown in blue, placed with the
  route; Ctrl+Z puts them back); Stop at obstacle draws a route that breaks
  clearance red and does not place it. Starting on a net of a differential pair
  routes both nets together at the width and gap the Impedance window
  works out for the pair. Ctrl+Z / Ctrl+Y undo and redo; Delete
  removes the clicked track or via; Shift+Delete unroutes the selected net
- move parts as in Altium: press on a part and drag it; Space turns it 90°
  about the cursor, L flips it to the other side, Escape puts it back. Its
  pads, silk and 3D model go with it, tracks that end on its pads stretch
  to follow, and a ratsnest shows where its unrouted pads connect. Pours
  are poured again around it, and the design rule check runs again
- toolbars as in Altium: icons for files, undo, zoom and views under the
  menu, an Active Bar over the board for the tools, and layer tabs under
  it (click to make a layer current, right-click to hide; LS or Shift+S
  single layer mode; + / - next and previous layer)
- tune lengths (the Tune button, or T): click a straight track to add
  meanders until the net meets its target from the Constraint Manager, its
  pair partner or the Target box; Space puts them on the other side
- check impedance (the Impedance button, or I): the board's stack-up with
  editable thickness and Er per layer, the trace width for the single-ended
  target and the width and gap for the differential target on each copper
  layer, the delay in ps/inch, a width and gap calculator, and the Z0 or
  Zdiff of every routed net and pair against its target. Per-net rules set
  other targets by name pattern (USB pairs start at 90 Ω, PCIe at 85 Ω).
  Stack-up edits and rules are kept per board in Trace's settings
- export fab outputs (File → Export fab outputs): one `.zip` with Gerber X2
  for every copper layer, solder mask, paste, silkscreen, mechanical layer
  and the board outline (Altium names: GTL, G1…, GBL, GTS, GTO, GKO…),
  Excellon drill files (plated, non-plated, and one per blind or buried
  span, slots routed), an IPC-D-356A test netlist, a pick-and-place file
  and a bill of materials. Copper carries X2 net, part and pin attributes.
  Mask and paste follow the board's expansion rules and each pad's own
  settings (manual expansion, tenting, no paste); silkscreen text from
  Altium boards is drawn in a stroke font
- check design rules (the DRC button, or D): clearance, short circuits,
  track width and unrouted nets, from the board's own Altium rules (per
  object type, by priority, net, class, part, footprint and part class
  scopes); and the manufacturing rules: hole size, hole to hole, annular
  ring, solder mask slivers, silkscreen over mask openings or pads,
  silkscreen of one part on another's, parts too close to each other
  (component clearance, with through-hole and SMD part scopes), and
  copper near or over the board edge. Live is on by default:
  the board is checked again in the background after every edit, the
  count shows in the status bar, and markers on the board show where.
  Click a violation to zoom to it. Boards without rules (Gerber, ODB++,
  Allegro) are checked for overlapping copper of different nets only
- edit footprints (the Footprints button): open an Altium `.PcbLib` or a
  Trace library (`.tracelib`), or take the footprints from the open board,
  each listed with the number of parts using it. Select, drag and nudge
  pads, lines and text; P places a pad numbered after the last, L draws
  silk, courtyard or assembly lines; change a pad's number, place, size,
  turn, shape (round, rectangle, rounded rectangle with corner %,
  octagon), side, hole and plating, or add copies in a row (the step is
  taken from the nearest pad of the same size). Undo and redo, mm or mil,
  grids from 0.01 mm. "Update N parts on the board" gives every part using
  the footprint the new pads and lines in one undoable edit: nets follow
  pad numbers, parts on the bottom are mirrored, pours are poured again.
  Save keeps the library as `.tracelib`; "Save as PcbLib…" writes an
  Altium footprint library (pads with their rounded corners, slots, mask
  and paste, lines, arcs, text and regions). "Place on board" puts a new part
  with the footprint on the board under the cursor, with the next free
  designator (R, C, U... as the board's parts with that footprint use):
  click to drop it, Space turns it, L flips it, Escape takes it away

## Command-line tool

```
trace inspect board.PcbDoc       stack-up, components, nets, classes, rules, primitive counts
trace inspect sheet.SchDoc       symbols with designators, values and pin counts; labels; child sheets
trace pairs board.PcbDoc [5mil]  length of each differential pair (tracks, arcs, via barrels), skew, and open nets
trace view board.PcbDoc          open the board viewer in your browser
trace rules board.PcbDoc         design rules grouped by kind
trace check board.PcbDoc         check the board's length and matched-length rules; lists failing nets
trace drc board.PcbDoc [--all]   design rule check: clearance, shorts, width, unrouted nets, and the
                                 manufacturing rules (holes, rings, mask slivers, silk on pads, silk to silk,
                                 part clearance, board edge)
trace impedance board.PcbDoc     stack-up widths for 50 Ω and 100 Ω pairs, and each pair's Zdiff
trace convert <file> out.json    export to Trace's open JSON model
trace convert <file> out.kicad_pcb   write a KiCad board
trace convert <file> out.PcbDoc      write an Altium Designer board
trace convert sheet.SchDoc out.svg   draw a schematic sheet as SVG
trace fab board.PcbDoc out.zip   Gerber X2, drill, IPC-356, pick-and-place and BOM (or a folder)
trace symbols lib.SchLib [out.svg]     list a schematic library's symbols; draw them all as SVG
trace footprints lib.PcbLib [out]   list a library's (or a board's) footprints; save as .tracelib or .PcbLib
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

Fab outputs were checked against the Gerbers TI made in Altium for the
16-layer DAC39RF12 EVM: drawn at 25 µm, every copper layer, both mask and
both paste layers differ by under 0.5 % of their area, and the drill files
hold the same holes. Reading the outputs back gives all 375 nets.

Tested on an 18-layer Altium board with 2,370 parts, 2,252 nets and 137 differential
pairs: it reads in about 0.3 s. A 22-layer Allegro board with 2,079 parts and
1.07 million records reads in about 0.7 s.

## Limits to know

- Lengths include tracks, arcs and via barrels between the layers used, but
  not package pin delays. Nets joined through copper pours are not checked
  for completeness yet.
- Routing checks clearance to tracks, pads, vias and drill holes of other
  nets. Pours on Altium boards are poured again after edits, close to
  Altium's own result (within a few percent of copper area on the test
  boards; rules scoped to parts, rooms or layers are not applied). Pours
  from Gerber, ODB++ and
  Allegro files are not poured again.
  Moving a part stretches only the last segment of each track on its
  pads, so moving far leaves long diagonal segments to tidy up; tracks on
  surface pads stay behind when the part is flipped. Silk follows parts
  on Altium boards (other formats do not say which silk is a part's).
  Push moves tracks only (not vias or pads), one
  track deep: a track that would have to push another is not moved.
- The design rule check takes a rule whose scope it cannot judge (rooms,
  layers, hole sizes) as possibly applying, and uses the smaller gap of it
  and the rule that surely applies, so it errs towards fewer reports.
  Clearance rules do not yet tell parts apart (the manufacturing rules
  do). Component clearance takes each part as the rectangle around its
  pads (turned with the part), not its 3D body, so a part whose pins
  spread wide reaches further than its body does; a part wholly inside
  another's rectangle (under a shield can) is not reported, and heights
  are not checked. Free silkscreen is not checked against itself. Height
  and net antennae rules are not checked yet.
- Fab outputs draw text in one stroke font, so silkscreen text is placed
  and sized as in Altium but its letters are shaped differently (TrueType
  text too). Text on mechanical layers and Altium special strings other
  than part parameters are left out. The BOM groups parts by value, part
  number, maker, description and footprint; it does not know variants or
  parts not fitted.
- Length tuning places one row of square meanders on one straight track
  at a time; pick another track if one is too short for the length needed.
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
- Update PCB joins nets of the same name across sheets (flat), and moves
  pads between nets only: tracks keep their old nets, so the DRC shows
  what to reroute. A part new on the sheets is put beside the board with
  its nets when its footprint is in the open footprint library or on
  another part of the board; otherwise it is listed for placing by hand.
  Parts deleted from the sheets stay on the board. New parts come from
  copies or from `.SchLib` symbol libraries (not `.IntLib` yet). Edits
  are not written back to `.SchDoc` files.
- Footprint libraries are read from `.PcbLib` and saved as `.tracelib`
  or `.PcbLib`; reading `.IntLib` is not done yet, and 3D bodies and text
  on mechanical layers are left out, so a `.PcbLib` saved by Trace has no
  3D models. Updating parts keeps their designator and value text where
  it was.
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
| `crates/trace-export` | Writes boards in other tools' formats (KiCad, Altium) and fab outputs (Gerber X2, Excellon, IPC-356, pick-and-place, BOM) |
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
