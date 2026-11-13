# Changes

Each release is installed as `trace-app-<version>.exe`; the previous version
is moved to the Recycle Bin.

## 0.41.0 (2026-11-13)

- Power integrity: DC voltage drop (IR drop) over a supply net. The
  net's pours and pads on every layer are laid on a grid (about 250 000
  cells a layer, finer for small nets) with each layer's copper
  thickness; tracks and arcs keep their own width's resistance; vias join
  the layers through their plated barrels. The source part's pads are held
  at the supply voltage, each load draws its current from its pads, and
  the network is solved by conjugate gradients.
- It reports each load's lowest and mean voltage, the largest drop, the
  copper's power loss, the via carrying most current, the track with the
  densest current (A/mm²), loads the source does not reach, and a drop
  map per layer.
- Checked against hand results: a pour strip's drop within 3 % of
  I·ρL/(W·t), a thin track's within 2 %, and two layers joined by vias
  halve the drop.
- View > "Power integrity (IR drop)…": pick a supply net (nets with pours
  are listed first), the source (guessed: an output pin, the regulator's
  inductor, a small IC with several pins on the net, or a connector), the
  voltage (guessed from names like 3p3V or 1V8), and the loads' currents
  (the total shared by pad count; capacitors and test points left out).
  Drop maps per layer, and "Save maps (SVG)".
- `trace ir board net [--source U5] [--volts 1.0] [--load U1=5A ...]
  [--total 10A] [--plating 0.8mil] [out.svg]`.
- CI: an example in trace-export renamed so Windows builds no longer
  clash over two `brdcheck.exe` files.

## 0.40.0 (2026-11-13)

- 2D field solver for transmission lines. Each cross-section is meshed
  (graded, fine at the copper's edges) and Laplace's equation solved by
  finite volumes, with every dielectric's own Er and Df, the copper's
  thickness, the solder mask over outer layers, and pour beside a track
  on its own layer as coplanar ground (found by itself, each side apart).
  It gives Z0, Zdiff, odd and even modes, Er eff, delay, L and C, near-
  and far-end coupling, and loss (Wheeler's incremental inductance rule
  for the copper, field energy per slab for the dielectric).
- Checked against exact results: thin stripline within 1 % of Cohn's
  formula, thin microstrip within 2 % of Hammerstad-Jensen, edge-coupled
  stripline modes within 2 % of Cohn, coplanar waveguide with ground
  within 1-3 % of conformal mapping, wide-strip loss equal to Rs/w.
- Impedance window: "Field solver" (on by default) solves the stack-up's
  widths and every routed net in the background; the calculator takes a
  pour gap and shows the modes, delay, L, C, loss and a picture of the
  field.
- Signal simulation takes track impedance, delay, loss and crosstalk
  coupling from the solver ("Field solver" in the window, `--formulas`
  for the old closed forms).
- `trace field board` (each layer) and `trace field board layer w [s]
  [--ground G] [out.svg]` (one cross-section, with a picture);
  `trace impedance board --field`.

## 0.39.0 (2026-11-13)

- Assembly drawings as PDF: File > Export assembly drawing (PDF)… or
  `trace assembly board out.pdf [--a4]`. A3 landscape by default: the top
  side, the bottom side seen from below (mirrored) when it has parts, then
  the parts list grouped by value and footprint, with the side of each
  line. Each part is drawn as a body around its pads and silk outline,
  with its pads, its silk, its designator fitted inside and turned with
  it, and a red dot on pin 1 (diodes, and parts with 3 or more pins).
  Every sheet has a title block: board, contents, date and sheet number.
  On TI's DAC39RF12 EVM (596 parts) it writes five sheets in 1.5 s.

## 0.38.1 (2026-11-13)

- New logo: the T drawn as a differential pair, with a via pad at each
  end, on navy. It is the app window's icon and the exe's icon, from
  16 to 256 px; the source is `crates/trace-app/assets/icon.svg`.

## 0.38.0 (2026-11-13)

- Loss from the board's own materials: each dielectric's loss tangent
  (Df) is read from Altium stack-ups and ODB++ jobs, and kept in Altium,
  ODB++ and Trace files. The Impedance window shows Df for each
  dielectric and roughness for each copper layer, and edits both.
- Signal simulation uses the loss tangent next to each layer and each
  copper layer's roughness, with the window's values where the stack-up
  gives none ("From the stack-up", on by default); the results say which
  values were used. On TI's DAC39RF12 EVM, the 6SRX pair runs in Itera
  MT40 (Df 0.003) and loses 0.23 dB at 1 GHz against 0.31 dB with FR-4
  values.
- `trace sim --df` and `--rough` now set every layer; without them each
  layer's own values are used.

## 0.37.0 (2026-11-13)

- Track loss in signal simulation: each line loses signal to its copper
  (DC resistance, skin effect, copper roughness) and its dielectric (loss
  tangent), through a causal (minimum-phase) filter, so long and fast
  routes show the slower edges and smaller eyes they will have. The
  window and `trace sim` give the loss at half the bit rate; Df and
  roughness can be set, with presets from FR-4 to Megtron 6.
- Crosstalk in signal simulation: nets running beside the route on the
  same layer are found, driven with patterns of their own (pairs against
  each other), and their near- and far-end crosstalk is added from the
  tracks' even and odd modes (Garg and Bahl for microstrip, Cohn for
  stripline). The results list the aggressors, the peak noise at the
  receiver and the eye without crosstalk.
- The starting state before the first edge is now solved for the whole
  circuit (lossy lines, terminations and packages included), so lossy
  routes start without ringing too.
- `trace sim` takes `--df`, `--rough`, `--lossless` and `--no-xtalk`. On
  TI's DAC39RF12 EVM, LMK_SDI (U30.7 to U22.20) at 200 Mb/s picks up
  509 mV of crosstalk from LMK_SCLK, LMK_C\S\ and two more nets, and the
  6SRX pair at 2 Gb/s loses 0.31 dB, its eye going from 3.27 to 3.12 V.

## 0.36.0 (2026-11-13)

- IBIS signal simulation: View > Signal simulation (IBIS)… simulates a
  routed net or differential pair from a driver pin to a receiver pin with
  the buffers and packages of IBIS models (V-I tables, clamps, ramps,
  C_comp, [Pin] and [Diff Pin], typical/min/max corners). The route is
  built from the board's copper as lossless transmission lines (stack-up
  impedance and delay, via barrels, stubs as branches, pairs at odd-mode
  impedance). A PRBS7 pattern gives the waveforms and an eye diagram with
  its height and width; Save picture… writes an SVG.
- `trace sim board from to --ibis file.ibs [--rate 2G] [--term 100diff] [out.svg]`
  runs the same from the command line. On TI's DAC39RF12 EVM, the 6SRX
  pair from J10.Z29 to U1.F1 (212 ps of route) with a 3.3 V demo buffer
  at 2 Gb/s and 100 Ω across the pair gives a 3.27 V eye, 94% of the UI
  open.

## 0.35.0 (2026-11-13)

- Bus routing: with the router on, Ctrl+click several pads or track ends
  to gather a bus, then click to route it. Its nets run side by side in
  the order of their pads, at the track width and the Bus gap on the
  route bar, and join their pads with 45° jogs. V places a row of vias
  across the bus; landing on a pad of one of its nets ends every track on
  its own net's pad. Walkaround and push work for the bus as a band.
- Match bus (route bar) brings the nets of the bus placed last to the
  length of the longest, or to the tuning target, with meanders that keep
  their clearance; nets with no room are named.
- `trace match board net,net,... [out] [--target L] [--amp L]` matches
  any nets' lengths from the command line. On TI's DAC39RF12 EVM, the
  positive nets of eight JESD lanes (1573 to 1775 mil) are brought to
  1775 mil on six lanes, with the design rule check unchanged.

## 0.34.0 (2026-11-13)

- Panelization: File > Panelize… lays copies of the board out in a grid
  (columns × rows) with rails top and bottom, and opens the panel in place
  of the board, unsaved. Copies come apart by tabs with mouse bites across
  a milled gap, or by V-score lines on a free mechanical layer. The rails
  carry fiducials and tooling holes.
- The milled gaps between copies are slots in the board outline, and fab
  outputs, ODB++ and KiCad boards carry them; mouse bites are unplated
  holes in the NPTH drill file.
- The design rule check on a panel applies the board's rules to every copy
  (nets and designators renamed `_2`, `_3`… are mapped back) and treats the
  milled gaps as board edge.
- `trace panel board 2x3 out.tracepcb [--gap L] [--rail L] [--tabs N]
  [--vscore]` makes a panel from the command line. TI's DAC39RF12 EVM as a
  2 × 2 panel is 259.6 × 154.0 mm with 20 tabs and 196 mouse bite holes.

## 0.33.0 (2026-11-13)

- Via stubs: Trace measures each via's stub from the stack-up and the
  layers its net joins it on, and the design rule check reports stubs
  longer than the board's back drilling rule allows (Altium's BackDrilling
  rule is read: max stub, hole oversize, top and bottom), or 10 mil on
  pair nets when the board has no rule.
- Backdrilling: Edit > Backdrill long via stubs backdrills every such via
  from the side its stub is on, down to just short of the layer it joins;
  Edit > Remove backdrills takes them off; both undo. Backdrilled vias get
  a green ring on the board, and pours keep clear of the drill. Altium
  boards with a back drilling rule are backdrilled when they open.
- The design rule check also reports backdrills that would cut their
  via's own connection, and copper of other nets too close to a backdrill
  on the layers it drills through.
- Fab outputs: an Excellon backdrill file per side and kept layer, naming
  the layer that must not be cut; ODB++ gets a backdrill drill layer for
  each.
- `trace stubs board [out]` lists stubs that are too long and, with out,
  saves the board backdrilled.
- TI's DAC39RF12 EVM backdrills GND vias only, with a 10 mil limit, and no
  GND via there is over it. Held to 10 mil on its pair nets instead, 16
  clock and SYSREF vias would need backdrilling (stubs of 29 and 52 mil),
  and the board passes the check once Trace has drilled them.

## 0.32.0 (2026-11-13)

- Return path check in the design rule check: signal tracks that run
  over a void in the planes next to them (more than 50 mil, or five
  track widths), or across a split where the plane under them changes
  from one net to another. Planes are copper layers that pours and fills
  cover at least 30% of; clearance round a track's own vias and pads is
  not counted.
- Crosstalk check: tracks of two nets running side by side too close for
  too long, from the board's Altium Parallel Segment rules, or by default
  the 3W rule for nets of differential pairs (at most 250 mil within two
  track widths of another net). Reports estimate the coupling from the
  spacing and the height over the plane.
- Both show in the Design Rule Check window with their own colours and
  in `trace drc`. On TI's DAC39RF12 EVM they find one track (SYNC_DAC on
  L10) over a void in both GND planes next to it, and no crosstalk.
- CI is pinned to the Rust toolchain the checks are written against, and
  each version's Windows build is published by GitHub Actions.

## 0.31.0 (2026-11-12)

- Altium rooms are read (room definitions, enabled or not) and drawn on
  the board as dashed outlines with their names.
- Rule scopes understand `WithinRoom('…')` and `TouchesRoom('…')`: an
  object is within a room when it lies wholly inside it, touches it when
  they overlap. A room the board does not define holds nothing, as in
  Altium (TI's DAC39RF12 EVM has a clearance rule for rooms that are not
  on the board, which is now simply passed over).
- The design rule check judges a clearance rule on the two objects
  themselves when their nets and kinds cannot decide it: parts
  (`InComponent`, `HasFootprint`, `InComponentClass`), layers
  (`OnLayer`), rooms. Before, such rules were only a maybe. Answers are
  kept per object, so checks stay as fast (DAC board: 1.5 s, same 296
  results).
- Polygon connect rules for rooms apply to the pads and vias in them.

## 0.30.0 (2026-11-12)

- Boards save back into Altium: File → Save as Altium board on a board
  opened from a binary `.PcbDoc` starts from that file and keeps every
  record of it, so rules, classes, polygon pour settings, dimensions, 3D
  bodies, unions, library links and unique ids all survive. Saving a board
  you did not change writes every stream byte for byte as it was.
- What you edited is written into it: parts moved, turned or flipped take
  their pads, silk, texts, regions and 3D models along (flipped parts go to
  the other side, their text mirrored); removed tracks, arcs, vias and
  areas are dropped; new routing and poured copper are added, new vias and
  pads copied from a via or pad of the same style on the board; net changes
  from the schematic (Update PCB) are patched in; new parts get a part
  record with a designator and a comment; renamed parts get their new
  designator.
- Altium's tables that point at records by position (pad unique ids,
  primitive GUIDs, pad and via library links, part parameters,
  shape-based regions) are renumbered to match, and the ratsnest is left
  for Altium to rebuild.
- Every save is read back and compared with the board saved; the status
  line says how many records were kept, changed, added and removed, and
  any difference. A file already there is kept as `.bak` first.
- `trace convert board.PcbDoc out.PcbDoc` saves the same way and reports
  the check.
- Checked on two private boards (one of 67,000 records, one with 2,000
  3D bodies) and three public boards: unchanged saves are identical stream by
  stream; with parts moved, turned and flipped, routing removed and added,
  pads moved to other nets, a new net, a new part and pours poured again,
  each board reads back exactly as edited.
- Boards from an ASCII `.PcbDoc` are written fresh with their own
  coordinates (no longer moved), now with their silkscreen, mask, paste and
  mechanical areas and via mask expansions. The DAC39RF12 EVM reads back
  with every via, part, net and area; left out are 149 drill-drawing
  lines and 6 pads of negative size.
- Not yet: boards from Allegro or Gerber are still written fresh; new pours and new polygon cutouts are written as copper
  only (pours) or not at all (cutouts); a changed board
  shape or new stack-up layers are not written.
## 0.29.0 (2026-11-12)

- Altium polygon pours saved without their copper are poured when the
  board opens, each layer on its own thread, and their nets are then
  checked for unrouted pads like any other net.
- Pouring follows Altium more closely:
  - polygon cutouts (keep-out areas for pours) are read and obeyed, and
    move with their part;
  - vias whose pad Altium removed on a layer keep the pour back from the
    hole by the hole clearance, not from a pad that is not there;
  - holes of pads (mounting holes) are kept clear by the hole clearance
    as well as the land;
  - polygon connect rules are judged for each pad and via, so rules such
    as `IsPad and HoleSize <= 39` (direct connect for thermal vias), or
    rules for parts, footprints and layers, apply where they should;
  - diagonal thermal spokes on square pads reach the pour;
  - islands under the pour's area threshold are removed.
- On TI's DAC39RF12 EVM with its 167 poured regions taken out, Trace pours
  all 46 pours in about 5 s (4 cores); the copper is within 1% of TI's
  Gerbers on every layer (it was about 4.4%), and the design rule check
  gives the same 296 results as on TI's own copper.
- Rule scopes can now ask `HoleSize` (`=`, `<`, `<=`, `>`, `>=`, `<>`)
  and `OnLayer('…')`.

## 0.28.0 (2026-11-12)

- Schematics save as Altium sheets: Save as Altium… in the schematic
  editor writes every sheet as a `.SchDoc` into a folder you pick, with a
  copy of the `.PrjPcb` listing them (other documents point back to where
  they are). A file already there is kept as `.bak` first.
- What did not change is kept as Altium wrote it, in its order: parts with
  all their units and display modes, hidden parameters, footprint and
  simulation models, unique ids, directives (differential pairs, no ERC),
  notes, templates, blankets and pictures. Moved parts keep all of that
  too; a changed value or footprint is written into it.
- What changed is written from the sheet: parts moved, turned, copied or
  placed from a library, wires, junctions, net labels, power ports, ports,
  sheet symbols and their entries, text and graphics.
- `trace convert sheet.SchDoc out.SchDoc`, and `trace convert
  sheets.tracesch folder` for every sheet of a saved Trace schematic.
- Checked on seven public sheets (Clocker, and six from the Archimajor
  board, 20,000 records): each reads back with the same drawing, parts and
  nets, every record kept; and with parts moved, turned, copied, deleted
  and placed, wires, labels and values changed, the edits read back.
- Ports and power ports now link to the text that names them.
- Parameters of a part's models are no longer drawn on the sheet.

## 0.27.0 (2026-11-12)

- Altium integrated libraries (`.IntLib`) are read: the symbols and
  footprints compiled into them.
- Place symbol… opens `.IntLib` files as well as `.SchLib`, and the
  `.IntLib`s an Altium project lists open by themselves. Update PCB also
  takes a new part's footprint from an open `.IntLib`.
- Opening an `.IntLib` (File → Open or the footprint editor's Open
  library…) shows its footprints in the footprint editor and adds its
  symbols to Place symbol. The `.IntLib` itself is never overwritten; save
  the footprints as `.tracelib` or `.PcbLib`.
- `trace symbols` and `trace footprints` take `.IntLib` files.
- Checked on five public integrated libraries (3,906 symbols and 196
  footprints of resistors, capacitors and connectors): every one reads.

## 0.26.0 (2026-11-12)

- ODB++ export: File → Export ODB++…, or `trace odb <board> out.tgz`
  (or a folder). One job with every copper, mask, paste, silkscreen and
  mechanical layer, the drill layers (plated, non-plated, and one per
  blind or buried span, slots as routed lines), the board profile, top
  and bottom component layers with packages and pins, and the EDA data
  that ties every pad, via, track and copper area to its net and pin.
  The layers are drawn by the same code as the Gerber files, so the two
  always agree. Standard symbols are used where they fit (round, rect,
  oval); other pad shapes become user symbols.
- The stack-up goes with it: copper layers carry their weight, and
  dielectric layers their thickness, Er and material.
- Reading ODB++ now also takes the stack-up from the job (dielectric
  layers and layer attributes), so impedance works on ODB++ boards.
- Checked on the DAC39RF12 EVM: the exported job reads back with the
  same 16 layers and stack-up, 596 parts, 375 nets, 1862 vias and holes,
  and per-layer pad counts that match the ODB++ Altium wrote for it.

## 0.25.0 (2026-11-12)

- Footprint libraries can be saved as Altium `.PcbLib`: "Save as PcbLib…"
  in the footprint editor, or `trace footprints <lib|board> out.PcbLib`.
  Pads (with rounded corners, slotted holes, their own mask and paste
  expansion, tenting and plating), lines, arcs, text, and copper, paste
  and mask regions are written; names longer than Altium's 31-character
  storage limit are kept in full. 3D bodies are not written.
- Checked by reading every footprint back: all 90 footprints of ten
  public libraries come back the same as they were read from Altium's
  own files.
- Altium boards (Save as Altium board) now also keep rounded-rectangle
  pads, slotted holes, pad mask and paste expansion, tenting, unplated
  holes, and paste and solder mask regions.

## 0.24.0 (2026-11-12)

- Altium schematic libraries (`.SchLib`): symbols are read with their
  graphics, pins (also the binary pin records libraries use), designator,
  value, description and current footprint. Multi-part components give
  one symbol per part.
- Place symbol… in the schematic editor (or P): a window that lists the
  open libraries' symbols with a search box and a drawn preview. Place
  takes the symbol onto the sheet under the pointer; click drops it,
  Space turns it, Esc cancels. New parts get the next free designator,
  and units B, C… of a multi-part part join the first placed part that
  lacks them. Update PCB then adds the part when its footprint is known.
- The project's own libraries open with the project; libraries opened by
  hand are remembered for next time.
- `trace symbols lib.SchLib [out.svg]` lists a library and can draw every
  symbol on one SVG sheet.
- Tried on five public libraries (70 symbols: ICs, connectors, diodes,
  transistors, capacitors); all draw as in Altium.

## 0.23.0 (2026-11-12)

- Two more manufacturing checks in the DRC, from the board's own Altium
  rules:
  - Silk to silk: one part's silkscreen on or too near another part's,
    or the board's free silkscreen (SilkToSilkClearance). A part's own
    lines are not checked against each other, and each pair of parts is
    reported once, at its worst spot.
  - Part clearance: parts closer than their ComponentClearance rule
    allows. Each part is the rectangle around its pads, turned with it;
    parts on opposite sides count only when the rule asks for all
    layers; a part wholly inside another (under a shield can) is left
    alone.
- Rule scopes understand IsThruComponent, IsSMTComponent, Name Like and
  ObjectKind for parts, so per-footprint and through-hole clearance rules
  apply as in Altium.
- Clicking a part clearance fault zooms to both parts.
- Tried on Clocker (12 silk to silk, 1 part clearance) and the DAC39RF12
  EVM (23 silk to silk, 119 part clearance, most between 0402 parts
  under the SmallPassives rule).

## 0.22.0 (2026-11-12)

- Blind, buried and micro vias when routing. The route bar's Via list
  holds every via span the board uses (read from its vias, with the size
  most of them have) and the through via:
  - Auto (the default) places the narrowest via that joins the two
    layers, or a stack of microvias and buried vias meeting end to end
    (top to L3 on an HDI board: L1-L2, L2-L3 and the buried L3-L8, at one
    spot) before falling back to the through via;
  - or pick one via by hand; if it does not reach the next layer the
    route bar says so;
  - each via kind keeps its own pad and drill size, and + adds a span the
    board does not use yet (microvias start at 10/4 mil).
  Clearance is checked only on the layers each via spans. Backspace takes
  back a whole stack. Placed vias keep their span in the DRC, pours, 3D
  view, drill files and the KiCad and Altium writers.
- KiCad export writes vias joining an outer layer to the next as
  microvias.
- Tried on the ODB++ rigid-flex sample (10 layers, microvias L1-L2,
  L2-L3, L8-L9, L9-L10 and buried L3-L8): its via kinds are listed with
  their sizes.

## 0.21.0 (2026-11-12)

- Place new parts from a footprint library: "Place on board" in the
  footprint editor puts a part with that footprint on the board with the
  next free designator (letters taken from the board's parts that use the
  footprint, or guessed from its name). It follows the cursor until a
  click drops it; Space turns it, L flips it, Escape takes it away. One
  Ctrl+Z takes back placing and moving together.
- Copy a part on the schematic (Copy part in the side panel, or Ctrl+D):
  the copy gets the next free designator, keeps the footprint and value,
  and lands in a clear spot beside the original, away from wires, labels,
  parts and notes, so it joins no net until you wire it.
- Update PCB adds parts new on the sheets: the footprint comes from the
  open footprint library, or from a part on the board that uses it. New
  parts go in a column right of the board with their pads already on the
  sheet's nets (a new net is made for a new label), ready to drag into
  place. Parts whose footprint is not found are listed as before.
- Each schematic part's footprint is read from the SchDoc (its current
  PCB model) and kept in saved `.tracesch` files.
- A new net label starts empty instead of with the last label's text.
- Tried on the Sinara Clocker project: copying R43 as R44, labelling its
  pins P3V3 and a new net, then Update PCB added R44 beside the board with
  both pads on the right nets; planning again found nothing to do.

## 0.20.0 (2026-11-12)

- Footprint editor (the Footprints button on the toolbar):
  - opens Altium `.PcbLib` footprint libraries (pads with rounded corners
    and per-layer stacks, lines, arcs, fills, regions and text), Trace
    libraries (`.tracelib`), or the footprints of the open board, each
    with the number of parts that use it;
  - select and drag pads, lines and text on a grid; arrows nudge, Delete
    removes; P places a pad numbered after the last one, L draws lines on
    silk, courtyard, assembly or copper;
  - the side panel edits a pad's number, place, size, turn, shape
    (rounded rectangle with corner %), side, hole and plating, and adds
    copies in a row, stepping like the nearest pad of the same size;
  - new, copy and remove footprints; name and description; undo and redo;
    mm or mil; Save and Save as keep a `.tracelib` library.
- "Update N parts on the board": every part using the footprint gets the
  edited pads and lines in one undoable edit. Pads keep their nets by
  pad number, designator and value text stay where they were, and pours
  are poured again.
- Parts on the bottom side now mirror top to bottom (as Altium and KiCad
  do), so a footprint taken from a bottom part and put back lands exactly;
  flipping a part with L turns it 180° as well to keep its left and right.
- Altium pads with rounded corners whose main shape is round are now read
  as rounded rectangles.
- `trace footprints <lib or board> [out.tracelib]` lists footprints and
  saves them as a Trace library.
- Tried on public Altium libraries (jefflongo/altium: ICs, connectors,
  passives, transistors) and the Sinara Clocker board: widening the pads of
  one capacitor footprint updated its 65 parts and the DRC count stayed
  the same.

## 0.19.0 (2026-11-12)

- Schematic editing, in the schematic view:
  - drag parts and their wires follow with a bend; Space turns a part,
    Delete removes a part, wire or label;
  - W draws wires: click the corners (a right-angle corner is added),
    and the wire ends on a pin or another wire, with a junction where it
    lands mid-wire. Clicks catch pin ends, wire ends and wires near the
    pointer, so off-grid pins connect;
  - N places a net label; select one to rename or remove it;
  - change a part's designator or value in the side panel (every unit of
    a multi-part component follows);
  - Ctrl+Z / Ctrl+Y undo and redo, up to 200 steps.
- Update PCB…: compares the sheets with the board and lists every change,
  pins moving between nets, nets renamed by a label, parts renamed, parts
  only on one side, then applies them as one undoable board edit and
  pours again. A board read fresh from its file is compared with the
  sheets as first read, so edits saved earlier still carry over after
  reopening.
- Save (Ctrl+S) keeps the edited sheets, pictures included, in a Trace
  schematic (`.tracesch`) next to the project; opening the `.PrjPcb` or
  the `.tracesch` brings them back. File → Open lists `.tracesch` files.
- Single-letter keys (S, C, F, B, M, 3) no longer fire when Ctrl, Shift
  or Alt is held, so Ctrl+S saves instead of switching views.
- Tried on the Sinara Clocker project (Altium, CERN-OHL): wiring a pin to
  another net, a new label and a renamed capacitor gave 7 changes, and
  re-planning after applying gave none.

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
