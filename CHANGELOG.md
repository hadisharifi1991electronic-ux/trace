# Changes

Each release is installed as `trace-app-<version>.exe`; the previous version
is moved to the Recycle Bin.

## 0.18.0 (2026-11-12)

- Manufacturing checks in the DRC, from the board's own Altium rules,
  listed with the rest and marked on the board:
  - hole size (smallest and largest drill);
  - hole to hole spacing (slots included; stacked and blind vias that
    share no layer are left alone);
  - annular ring of plated holes and vias, layer by layer;
  - solder mask slivers between openings, with each pad's own expansion
    and tenting, and mask openings drawn by hand;
  - silkscreen over mask openings, or over the pad itself where the rule
    measures to exposed copper;
  - copper near the board edge, or past it, with the rule's gaps per
    object (pours often have their own).
  Each kind has its own chip in the DRC panel, shown once it finds
  something. `trace drc board --all` lists every one.
- Rule scopes now understand parts: `InComponent`, `HasFootprint` (with
  `*` wildcards), `InComponentClass`, `InAnyComponent`, `IsFree`. Altium
  component classes are read.
- Rounded corners and other arcs of the board shape are read from Altium
  boards (they were cut straight before), so the board edge, the outline
  Gerber and pours by the corners are right.
- Copper regions, fills and mask openings drawn in a footprint move with
  their part.
- On the DAC39RF12 EVM: 17 board edge, 2 annular ring, 52 mask sliver and
  37 silk to mask findings: edge-launch connector copper drawn to the
  edge, two balun shield holes larger than their pads, 2–3 mil
  mask between fine-pitch pins, silk over pads.

## 0.17.0 (2026-11-06)

- Fab outputs: File → Export fab outputs (Gerber, drill, BOM), or
  `trace fab board out.zip`. One zip holds:
  - Gerber X2 for every copper layer, both solder masks, both pastes, both
    silkscreens, each mechanical layer in use and the board outline, named
    as Altium names them (GTL, G1…, GBL, GTS, GBS, GTP, GBP, GTO, GBO, GKO,
    GM1…). Copper carries net, part and pin attributes;
  - Excellon drill files: plated, non-plated, and one per blind or buried
    via span; slots are routed;
  - an IPC-D-356A netlist for bare-board test;
  - pick-and-place (part centre and origin, side, rotation) and a BOM
    grouped by value, part number, maker, description and footprint.
- Checked against TI's own Altium Gerbers for the DAC39RF12 EVM: all 16
  copper layers, both masks and both pastes within 0.5 % of area, the same
  drill holes, and all 375 nets when read back.
- Altium boards (binary and ASCII) now bring in what manufacture needs:
  each pad's own mask and paste expansion, tenting and "no paste",
  plated or not, slotted holes, via tenting, mask and paste openings of
  copper areas, fills and regions drawn on mask, paste and silk layers,
  and part parameters (value, part number, manufacturer, description).
- Silkscreen and copper text from Altium boards is drawn (stroke font,
  placed and sized as in Altium), including designators and special
  strings such as `.Designator` and `.Text1`, and moves with its part.

## 0.16.0 (2026-11-06)

- Length-match groups in the Constraint Manager, Allegro style:
  - match to the longest member, to a member you pick (a net, or a pair
    by name) or to a set value, within a tolerance either way;
  - match propagation delay in ps instead of length: each track at the
    speed of its layer from the stack-up (microstrip outside, stripline
    inside), via barrels at the board's mean dielectric; a Delay column
    shows every net's delay;
  - "pairs as one" matches each differential pair by the mean of its two
    nets;
  - an "Off target" column shows how far each member is, signed.
- The tuner (T) tunes to the group's target, working out the length a
  delay target needs on the layer of the track being tuned.
- Nets failing the Constraint Manager's rules are listed in the design
  rule check panel under Length; a click selects and shows the net.

## 0.15.0 (2026-11-06)

- Move parts as in Altium: press on a part and drag it. While dragging,
  Space turns it 90° about the cursor, L flips it to the other side
  (mirrored, onto the other outer layer), Escape puts it back.
  - Pads, silk, part copper and the 3D model move with it.
  - Tracks that end on its pads stretch to stay joined.
  - A ratsnest shows where its unrouted pads connect while it moves.
  - Pours are poured again around the new place, the design rule check
    runs again, and Ctrl+Z puts the part back in one step.
- Silk and other primitives of a part read from Altium files (binary and
  ASCII) know their part, and exported PcbDoc files keep it.

## 0.14.0 (2026-11-05)

- Walkaround and push-and-shove in the route tool, as in Altium. Shift+R
  (or the list on the route bar) cycles Walkaround, Push and walkaround,
  and Stop at obstacle.
  - Walkaround finds a 45° path round pads, vias and tracks of other
    nets, hugging them at the clearance; when the cursor is on something
    it cannot get round, the route stops just short of it.
  - Push moves tracks of other nets out of the way, each re-routed between
    its own ends; the moved tracks show in blue while routing and go in
    with the route. Vias and pads are not pushed.
  - Both work for differential pairs.
- Clearance searches are about three times quicker on many-layer boards
  (obstacles are indexed by layer), which also speeds up routing checks.
- A clearance marker points at the spot where a long track is in the
  way, not at its middle.

## 0.13.0 (2026-11-05)

- Live design rule check (DRC button, or D). Clearance between copper of
  different nets, short circuits, track and arc width, and unrouted nets,
  from the board's own Altium rules: per object type (track, arc, via,
  SMD pad, through-hole pad, pour, region, hole), first rule by priority,
  net, class and pair scopes. Rooms and footprints in a scope count as
  "may apply", and the smaller gap wins. It runs in the background on
  open and after every edit and re-pour; the status bar shows the count,
  markers show each spot, and a click in the list zooms there and selects
  the net. Matches Altium on the test boards: no false reports on
  Fastino, LimeSDR-USB, video and pic_programmer, and only real overlaps
  (alternate parts on the same pads) on the DAC39RF12 EVM.
- `trace drc <board>` on the command line.
- Altium boards read more exactly:
  - unused inner pads that Altium removed, on vias and through-hole pads,
    are gone (also tightening re-pour on plane layers);
  - rounded-rectangle pads keep their rounded corners;
  - shape-based regions in ASCII files (custom pad shapes) are read, and
    take the net of the right pad (PADINDEX counts from 1);
  - poured copper without a net of its own takes its polygon's net.
- Connectivity follows copper regions and pours, so nets joined through a
  region are no longer reported as open.
- Plated slots from Gerbers only have copper on the layers that show it.

## 0.12.1 (2026-11-05)

- Opens fab packages that wrap the real data in another archive, such as
  TI's `ODB++.zip` holding the ODB++ `.tgz` (and a `.zip` of it). Before,
  Trace read the inner archive as a Gerber and crashed.
- IPC-356 netlists from Altium now land on the copper: the space Altium
  writes for a plus sign is read, and the shift between the netlist's
  board origin and the Gerbers' absolute origin is found and taken out.
  Nets and part pins come from the netlist again.
- The board edge from Gerbers is the smallest outline that holds every
  drill hole, so the drawing frame Altium plots on every film is no
  longer taken for the board.
- A file that makes a reader fail now gives a message instead of closing
  the app.

## 0.12.0 (2026-11-05)

- Copper pours are poured again after every edit that touches them:
  routing, deleting, length tuning and unrouting. A new track gets its
  clearance cut out of other nets' pours; a deleted one leaves no void.
  Gaps follow the board's clearance rules (per object: track, pad, via,
  pour), pads and vias join their own net's pour as the polygon connect
  rule says (direct or thermal relief spokes), pours keep the board
  outline clearance, and islands without their own net's copper are
  removed. It runs in the background; Ctrl+Z takes the pour back with
  the edit. Works for Altium boards (binary and ASCII), which keep each
  pour's outline; Gerber, ODB++ and Allegro pours stay as read.
- ASCII PcbDoc files now take their design rules from the `DXPRule`
  records, as Altium does.

## 0.11.0 (2026-11-05)

- Toolbars in the manner of Altium Designer, with icons:
  - a standard bar under the menu: open, save, undo, redo, fit, zoom in
    and out, bottom view, board / 3D / schematic, Constraint Manager and
    Impedance; the options of the current tool follow it;
  - an Active Bar floating over the top of the board: select, measure,
    route one track, route a differential pair, length tuning, place a
    via (while routing) and unroute the selected net;
  - layer tabs under the board: click a copper tab to make it the
    current layer, right-click any tab to show or hide it. LS (or
    Shift+S) is single layer mode, and + / - step through the copper
    layers.
- Clicking a track now marks that one segment in yellow, with handles at
  its ends, apart from the rest of its net in white. Delete removes only
  that segment; Shift+Delete unroutes the whole net.

## 0.10.1 (2026-11-05)

- Opens Altium boards saved as ASCII (`.PcbDoc` written with "PCB ASCII
  File"): stack-up, nets, classes, pairs, parts, rules, tracks, arcs,
  vias, pads, pours and copper fills. Before, these failed with "Invalid
  CFB file".

## 0.10.0 (2026-11-05)

- Length tuning (T): point at a straight track of a net and click to add
  meanders until the net reaches its target length. The target comes from
  the Constraint Manager (match group, minimum length, or the pair's
  skew), else from the other net of a differential pair, or from the
  Target box in the toolbar. The toolbar shows the net's length now, the
  length after tuning and where the target came from. Height and Spacing
  set the meander size (Spacing auto is 4 × track width); Space puts the
  meanders on the other side of the track. The meanders are checked for
  clearance before they are placed, and Ctrl+Z takes them back.

## 0.9.0 (2026-11-05)

- Differential pair routing: starting a route on a net of a pair routes
  both nets together, the partner from its pad next to the one clicked,
  along one centre line. Width and gap on each layer come from the
  Impedance window (the pair's target, e.g. 90 Ω for USB). Corners,
  round corners and Space / Shift+Space work as for one track; V places
  a via for each net, spaced for the via pads, and the route lands on the
  pair's pads at the far end. Both tracks are checked for clearance. The
  Pairs box in the routing toolbar turns this off.

## 0.8.1 (2026-11-05)

- Corner styles while routing, as in Altium: Shift+Space cycles 45°,
  45° with an arc, 90°, 90° with an arc and any angle; Space flips which
  way the corner bends; , and . make round corners smaller or larger. The
  style is also in the routing toolbar. Round corners are placed as arcs.

## 0.8.0 (2026-11-05)

- Routing (X): click a pad, via or track end to start a track on the
  active layer, click to fix 45° corners (/ swaps which run comes first),
  V or Shift+V drops a through via and carries on one layer down or up,
  Backspace takes back a corner or via, and the route finishes by itself
  on a pad, via or track of the same net (or with Enter or a double-click).
  Width, clearance and via size start from the board's rules and most used
  via, and can be changed in the toolbar.
- Clearance is checked as you move: a route closer to another net than the
  clearance, or shorting to it, is drawn red, the copper it hits is
  circled, and it is not placed.
- Edit menu with Undo and Redo (Ctrl+Z, Ctrl+Y), Delete for the clicked
  track or via, and Unroute selected net (Shift+Delete).
- Edited boards show "(edited)" in the title and ask before closing. Save
  them as a Trace, KiCad or Altium board; the original file is never
  changed.

## 0.7.1 (2026-11-05)

- Clicking a track now selects that track's net. Before, copper on a hidden
  or lower layer (a wide plane track or a pad under the click) could win,
  so some tracks could not be selected, most often when zoomed in. Clicks
  now follow what is drawn: the active layer's copper, then its pours, then
  the other layers. Checked by clicking every track on eight boards (about
  57,000 tracks).
- The selected net is drawn in white with a dark edge over a darkened
  board, so it shows on many-layer boards where stacked pours make the
  view pale.

## 0.7.0 (2026-11-05)

- Impedance window (I): the board's stack-up with editable thickness and Er,
  the width for the single-ended target and width/gap for the differential
  target on each copper layer, delay in ps/inch, and a width and gap
  calculator (microstrip, stripline, dual stripline).
- Every routed net and pair is checked against its target, with per-net
  rules by name pattern (USB pairs default to 90 Ω, PCIe to 85 Ω). Click a
  net to select it on the board. The status bar counts pair nets off target.
- `trace impedance board` prints the same stack-up table and pair check.

## 0.6.0 (2026-11-05)

- Schematic viewer (S): opens an Altium `.PrjPcb` (all its sheets and its
  board) or a single `.SchDoc`, in binary or ASCII form. Parts, pins, wires,
  buses, net labels, power ports, ports, sheet symbols, notes, directives,
  pictures, the sheet border and the title block are drawn as Altium draws
  them.
- Opening a `.PcbDoc` also loads the schematic of its project, when a
  `.PrjPcb` next to it (or one folder up) lists it.
- Cross-probing: click a wire or pin to select its net on the board; select
  a net on the board and "Show in schematic" jumps to it. Sheets that carry
  the selected net are marked in the sheet list. Clicking a part selects it
  on the board.
- `trace convert sheet.SchDoc out.svg` draws a sheet as SVG.

## 0.5.0 (2026-11-05)

- Constraint Manager (C): length, match, pair-skew and via limits on groups
  of nets; Altium Length and Matched Length rules are read from the board.
- `trace check` lists nets that break the board's length rules.
- Version number in the window title, Help menu, `trace --version` and the
  exe's Properties.

## 0.4.0

- 3D view (3) with the STEP part models stored in Altium boards.
- Save as Altium board (`.PcbDoc`).

## 0.3.0

- Save as KiCad board.
- Allegro fabrication sets and IPC-356 netlists; via span labels.

## 0.2.0

- Gerber, Excellon and ODB++ import with automatic layer detection.
- Allegro `.brd` boards, copper pours, measure tool.

## 0.1.0

- Altium `.PcbDoc` viewer, differential pair lengths and skew.
