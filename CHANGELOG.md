# Changes

Each release is installed as `trace-app-<version>.exe`; the previous version
is moved to the Recycle Bin.

## 0.106.0 (2026-11-25)

Length and phase tuning, as Altium's three tuning tools:

- The tuning bar picks what to tune (Route menu, or Shift+T while
  tuning): **Net length** (one net, as before), **Pair length** (both legs
  of a differential pair meander together at their gap, to the pair's
  mean target, choosing the bumps that keep the skew) and **Pair phase**
  (small bumps on the shorter leg, on the side away from its partner,
  until the legs are the same length; height 3 track widths by default).
- Five bump styles (Tab while tuning): accordion, mitered (45° corners),
  rounded (arcs, semicircles when tall enough), trombone (one wide
  detour) and sawtooth. Each is sized to add just the length needed.
- Bumps are built where you point on the track, on as short a stretch as
  holds them (the rest stays straight), and drop to a lower height where
  the full one would come too close to other copper. A pair leg tuned on
  its own bumps away from its partner (Space still flips it).
- A gauge by the cursor shows the length (or skew) now and with the bumps,
  the target with its match tolerance, and the pair's skew against its
  limit from the Constraint Manager: green when it passes, amber when
  short, red over or too close to other copper. 1 / 2 change the spacing,
  3 / 4 the height.
- `trace match` takes `--style`, `--space` and `--skew` (phase tuning of
  each pair named). On the DAC39RF12 EVM the 13 pairs over 2 mil skew
  (up to 16.5 mil) come to 0.0 mil with phase bumps, with the same 296
  DRC results before and after.

## 0.105.0 (2026-11-25)

Board cut-outs that fit in a PC's memory:

- The starting mesh no longer runs every via's pad and antipad lines
  through the whole box (on a 240 mil CutiePi window that was 2.6 million
  tetrahedra before the first solve, so it ran out of memory). When the
  plain grid is too big, it starts from a coarse grid on the layers and
  port edges and refines it only about each barrel, pad and antipad ring,
  and along the driven pair's own traces, so the path from port through
  the via is whole from the first pass. A new note says when the start
  ran out of room before reaching all of the driven copper.
- `via:NET` drives only the via's own pair: every other trace leaving the
  window is a matched termination that absorbs but costs no solves (it
  was 15 driven ports on that CutiePi window, now 2). `--drive NET,...`
  picks the nets for an explicit window, `--all-ports` drives them all.
- Where the pair ends on pads inside the window (BGA balls, chip pins)
  instead of leaving it, the pads get a lumped port: P pad to N pad at
  100 ohm, or a single net to the ground pad beside it. That was why
  cut-outs on the DAC board came out with one port.
- Traces that cross the window with both ends outside it are no longer
  dropped.
- Solver memory: the matrix is assembled straight into its sparse
  pattern (no list of entries, about a fifth of the memory there), and
  GMRES keeps only its Krylov basis, in batches of right-hand sides
  (half the vectors of the flexible version).
- Cut-outs use first-order elements by default (`--order 2` for second
  order); their copper needs many small elements more than high-order
  ones. The 3D model window does the same for cut-outs and starts them at
  400k unknowns.
- Checked on the CutiePi HDMI_D2 pair via, +-120 mil window, 6 layers, 23
  nets: 3.1 GB peak at 354k unknowns (out of memory before); on 100 ohm
  Sdd21 -1.1 dB at 3 GHz and -1.5 dB at 6 GHz, Sdd11 -14.8 / -13.6 dB
  (the TOP pair reads 144 ohm, so most of the reflection is that
  mismatch).

## 0.104.1 (2026-11-25)

- 3D view: drill holes are cut out of pours and tracks too. Relief spokes
  (and track ends) that run to a pad's or via's centre used to be drawn
  straight across the hole, so thermal pads showed a green cross and vias
  in a pour looked filled. Every hole now shows open, through the copper
  on each layer it passes.

## 0.104.0 (2026-11-25)

- Schematic view polished to match the board view: the sheet lies on the
  desk with a soft shadow, a faint 100 mil grid (every inch a little
  stronger) fades in as you zoom, and part bodies and boxes cast a small
  shadow on the paper.
- Junction dots where wires meet in a T, as Altium draws them: Altium does
  not save these dots, so sheets like rx_rf showed none. A wire ending on
  another wire or on its corner, or three ends and pins at one point, get
  a dot in the wire's colour (lit with its net); crossings and plain
  corners do not.
- Smoother drawing: circles (junctions, pin bubbles) are drawn round and
  antialiased and never vanish when zoomed out, filled shapes get smooth
  edges, and thick wires have round ends and corners.
- Text too small to read shows as a faint stroke where it lies, so a
  zoomed-out sheet still shows where the labels are.
- `TRACE_OPEN=schat:x0,y0,x1,y1` opens the schematic on a box (mil, or
  fractions of the sheet), for screenshots.

## 0.103.0 (2026-11-25)

- 3D view looks like a real board: the solder mask is glossy, with a soft
  sheen that moves across the board as you turn it and stronger
  reflections at grazing angles. Gold pads and copper reflect a studio
  sky and catch highlights, silkscreen ink and part bodies have a slight
  gloss, and plated holes go dark half way down the barrel instead of
  showing a flat copper wall.
- Lighting per pixel, with a fill light, and a gradient backdrop.

## 0.102.2 (2026-11-25)

- 3D view: the inner layers are there when you zoom in between them. They
  used to be built only for X-ray and Section, so the board was hollow
  inside. Now every inner plane and track is built, with its antipads
  round the barrels.
- The stripes across the board when seen edge-on are gone: the outer
  copper no longer sits at the same depth as the laminate, and a copper
  layer's far face is no longer drawn through its near face.
- Seen from between the layers, the laminate shows as bare glass-epoxy
  instead of solder mask.

## 0.102.1 (2026-11-25)

- 3D view arrows: each arrow now shows the part of E or H that lies in the
  cut face, and a field mostly through the face is always drawn as a dot
  (out toward you) or a cross (into it), sized by its strength. Before,
  the choice went by the arrow's length on screen, so in a tilted view H
  circling a trace (through the face) showed as long arrows that seemed
  to run along the trace (seen on the DAC39RF12 EVM SYSREF pair via).

## 0.102.0 (2026-11-25)

- A more realistic 2D board view. The board is drawn as a dark laminate
  with a soft shadow under it, instead of copper floating on the
  background.
- Copper on the active layer, and every via, is drawn as solid metal: a
  dark edge round each track and pad, and a faint sheen down the middle.
  Its pours are solid copper under its tracks.
- Planes on the other layers are drawn as copper seen through the board,
  opaque and dark, under all the tracks. Overlapping pours no longer pile
  up into a pale pink and green wash, and inner routing still shows.
- Drill holes look like plated bores, lighter at the wall and dark in the
  middle.
- `TRACE_OPEN=at:x,y,scale` starts the board view centred on (x, y) mil.

## 0.101.0 (2026-11-25)

- 3D models straight from the board. Right-click a via on the PCB and
  choose "3D model of this via": a cut-out +-120 mil about the via and
  its pair partner's vias, through every copper layer with every net in
  it, opens in the 3D model window (turn it, cut it open, solve and paint
  its fields). The same is in View > "3D model of the selected via" for a
  via you clicked.
- View > "3D model of an area…" (or right-click > the same): drag a box
  on the board and it opens as a 3D cut-out, as `trace cutout` builds it
  (wave ports where traces leave, two-pad parts as lumped ports). A box
  drawn with Shift+drag can be opened too (right-click > "3D model of the
  box drawn"). Boxes up to 1500 mil a side, since the solver meshes
  everything in them.

## 0.100.0 (2026-11-25)

- 3D view: vector plots, like HFSS's. With |E| or |H| on the cut, arrows
  on a regular grid over the face show the field's direction, sized and
  coloured by its strength (the map underneath is dimmed so they read);
  a field mostly through the face shows as a dot (out toward you) or a
  cross (into it). With the surface current, evenly spaced current-flow
  lines are traced over the visible copper with heads along them showing
  which way the current runs. Both are taken at the phase shown (0 deg in
  magnitude mode) and move through the cycle when animated.
- App: 3D model window > "Arrows" (|E|, |H|) or "Flow lines" (surface
  current) next to the overlay buttons; saved PNGs and GIFs include them.
- Command line: `--vectors` with `--render` on any FEM command, e.g.
  `trace via board L1 L8 --check fem --render e.gif --field e --vectors`.

## 0.99.1 (2026-11-24)

- 3D FEM: when the half-memory iterative solver cannot converge (seen on a
  DAC39RF12 EVM cut-out with four lumped ports: residual 1e-1, or stalled
  at 3e-6 after 300 iterations), a pass, a sweep point or the field solve
  now falls back to the direct double-precision solve instead of
  stopping. A residual that stalls below 1e-4 is accepted (far below
  what moves S).
- Via/cut-out 3D model: outer-layer copper sits on top of the laminate
  (it was centred on the surface, half in the air).

## 0.99.0 (2026-11-24)

The whole-board 3D view, built as the board is:

- Every copper layer sits at its stack-up height with its real
  thickness: tracks, pours, planes and pads are solid copper with side
  walls, not flat paint. Outer copper shows under the solder mask, bare
  pads in gold.
- Holes are drilled through the laminate, and every plated hole has its
  barrel (with the plating thickness), joined to the annular rings on
  each layer it reaches. Backdrilled stubs have no barrel left.
- X-ray (button or X key): the laminate and mask go clear and the copper
  half clear, coloured by layer, so the inner layers show through. A
  Layers menu shows or hides each copper layer.
- Section: cut the board open across x or y anywhere with a slider, and
  keep either side. The cut face is drawn solid: core and prepreg
  bands, every copper layer, and the barrels of the vias it passes
  through.
- `TRACE_OPEN=3d:<spec>` opens the 3D view set up for screenshots
  (xray, section, y, flip, pos=, hide=, yaw=, pitch=, zoom=, at=).

## 0.98.0 (2026-11-24)

Vias and cut-outs that look real, with their fields on them:

- A new realistic 3D view of any 3D FEM model (a via, a board cut-out, a
  planar window), ray cast on the CPU: copper with its real thickness,
  plated barrels with their drilled holes, pads, traces, planes with
  their antipads, see-through laminate (amber, deeper for higher Er),
  wave ports, soft shadows, a metal look with reflections, box outline,
  axes and a scale bar. By default it is cut open through the signal via,
  so the barrel, the pads and the antipads show in section.
- The solver keeps the fields of one excitation on a 3D grid after the
  last pass. They paint |E| or |H| on the cut face, or the surface
  current |n x H| on every copper surface, on a dB scale, as the
  magnitude or at any phase; a GIF animates one cycle.
- App: Via window > "3D model…": drag to turn, right-drag to move, wheel
  to zoom (a quick picture while moving, a fine one when still); cut
  across x or y with a slider; laminate and shadows on or off; "Solve
  fields (3D FEM)" at any frequency and driven port, then |E|, |H| or
  surface current, Instant + phase slider, Animate; save a PNG (1800 x
  1200) or a GIF.
- Command line: `--render FILE.png|FILE.gif` on `trace via ... --check
  fem`, `trace cutout`, `trace planar` and `trace fem`, with `--field
  e|h|j|none`, `--field-ghz`, `--field-port`, `--view AZ,EL[,ZOOM]`,
  `--cut y=MIL|x=MIL|none`, `--phase`, `--range`, `--size`, `--frames`,
  `--geometry-only` (draw and stop before solving).
- GIFs of shaded scenes get their own palette (median cut), so copper
  and laminate no longer band.
- `trace cutout` and `trace planar`: `--fmin` and `--points` set the
  sweep (default 100 points up to --fmax, as before).
- RF report: a termination within a fifth of a wavelength of the chip
  (about 200 mil at 6 GHz; it was a tenth, about 100 mil) is taken as
  terminating the chip, so clock terminations at 140 to 180 mil no
  longer read as about 3.5 dB of loss.
- Checked: `trace via --fields 6 out.gif` writes the field animation (not
  a Touchstone) under the .gif name.

## 0.97.0 (2026-11-24)

Board cut-outs with many nets:

- `trace cutout <board> <x0,y0,x1,y1>` (or `trace planar <board> all ...`):
  a 3D FEM solve of a window of the board through every copper layer,
  with every net in it. Traces, arcs, pads and pours sit on their own
  layers; vias and through-hole pins are barrels with their real nets
  (signal vias are no longer left open), with per-layer via pads.
- Each trace that leaves the window over ground copper is a wave port,
  referenced to the ground above and below it at the edge. A pair that
  leaves side by side is one differential port; other close neighbours
  share the gap. Two-pad parts on the top and bottom are lumped ports, as
  in `planar`, so `--value` re-tunes them.
- `via:NET[,HALF]` in place of the window centres it on NET's via (and
  its pair's), +-120 mil by default.
- A barrel through another net's pour that has no clearance there (a
  negative plane, or lost voids) gets a ring, and a note says how many.
- Prints every port's Z0, return loss and its strongest transmission.
- `--order 1` (planar and cutout) for a lighter first-order solve of a
  big window; S is referenced to 100 ohm when every port is a pair.
- Checked on the CutiePi HDMI_D2 pair (TOP to L03 through its vias, six
  layers, every net in a 90 mil window): 3 passes, 6 min, on 100 ohm
  S21 -0.14 dB at 3 GHz and -0.32 dB at 6 GHz, S11 -24 / -16.5 dB. The
  pair reads 108 ohm on L03 and 160 ohm on TOP. A single-ended
  line through a 120 mil window passes -0.13 dB at 6 GHz.

More line types in `trace fem`:

- `trace fem cpw` and `trace fem cpwg`: coplanar waveguide, without and
  with a plane under it (`g=` sets the gap to the coplanar grounds).
- `s=SPACING` on microstrip, stripline, CPW or CPWG makes it an
  edge-coupled pair, solved in the odd mode: Z0 is then the differential
  impedance.
- Checked: CPW (w 10, g 5 mil, thick Er 4) reads 73.8 ohm against 76.3
  from conformal mapping, Er,eff 2.48 against 2.5; a microstrip pair reads
  1.78x its single line, and CPWG reads below the bare microstrip.

FEM solver:

- The iterative solver no longer gives up when it stalls just short of
  its 1e-9 target (anything under 1e-6 is taken).
- A sweep point whose borrowed factor fails to precondition it (stall
  or NaN) is factored at its own frequency instead of failing the run.
- Pad and antipad edges stay in the first mesh: merged away, a via's
  clearance ring closed up and shorted the via to the planes.

FEM meshing:

- Only the copper a port launches keeps its exact outline in the first
  mesh (and where a trace meets its port face at an angle); the rest of a
  cut-out's copper is refined by the passes. A dense window's first mesh
  went from 281 M to 0.26 M tetrahedra.

## 0.96.0 (2026-11-24)

Planar EM solver for traces and matching networks:

- `trace planar <board> <layer> <x0,y0,x1,y1>`: a 3D FEM solve of a
  window of an outer layer (traces, pads, pours, ground vias down to the
  next layer's plane). Each trace that leaves the window is a wave port.
- Each two-pad part (a series or shunt L, C or R) becomes a lumped port,
  and its value goes in after the solve, from the board or from
  `--value C1=2.2pF`. A matching network can be re-tuned without solving
  again. Values like 1.5nH, 4p7, 0R, 49.9 and DNP are understood.
- Writes the full N-port Touchstone (wave ports and parts) and a Smith
  chart of S11 with the parts fitted.
- Checked: a 1 pF series capacitor in a 50 ohm microstrip matches the
  circuit result from 1 to 4 GHz; shorted and open gaps behave.
- Lumped ports in the FEM (resistive sheets, generalized S) and
  `fem3d::terminate` to load any port.

FEM ports:

- Each adaptive pass now prints every port's Z0, so you can see if the
  ports settle.
- Fixed the top/bottom port Z0 mismatch on symmetric stack-ups: a plane
  sat at its copper's top face even when the trace was below it, so a
  bottom microstrip saw the plane's thickness as extra gap. ADRV pair,
  converged: 126.3 / 125.8 ohm (was 126.3 / 130.8), S unchanged (S11
  within 2.7 dB of the FDTD).

## 0.95.0 (2026-11-24)

3D FEM: real copper and laminate loss:

- Copper loss on every conductor in the model (planes, barrels, pads,
  traces, both sides of a sheet, and a box wall that is a real plane):
  skin-effect surface resistance with the stack-up's roughness
  (Hammerstad), from the solved field's surface current, taken off the
  power that gets through. Checked on a microstrip: the fields' electric
  and magnetic energy agree and the strip and ground carry the same
  current.
- The laminate's Df goes into the FEM as a complex permittivity
  (exact at every frequency, unlike the FDTD's mid-band conductivity).
  Both are on with the via's metal loss (the default).
- ADRV9026 RF pair via, converged: FEM S21 -0.150 dB at 6 GHz and
  -0.334 dB at 12 GHz (lossless -0.095 / -0.234). The FDTD adds only the
  barrel's loss, analytically; the FEM now counts every copper surface.
- With losses on, the power-balance warning looks only for gain (loss is
  real).

## 0.94.0 (2026-11-24)

3D FEM: a fast, interpolating frequency sweep:

- The sweep solves only enough frequencies to pin the response down
  (the ends, then the middle of the widest gap, until two new points in a
  row land within 0.005 of what the solved ones predict) and fills in the
  rest with rational interpolation. `trace via ... --check fem` now gives
  100 frequencies (the Touchstone has them all; the table shows ten).
  ADRV pair via: 100 frequencies from 10 solves in 18 minutes (0.93.0: 10
  frequencies in 28 minutes), matching a solve at every point to 0.01 dB.
  TRACE_FEM_INTERP=0 solves every point.
- One single-precision factor serves the whole sweep (one more below 40%
  of the adaptive frequency), and all port excitations are solved
  together, so each sweep point is a few GMRES iterations, not a factor.
- Adaptive passes no longer stop early when a full refinement would pass
  `--fem-dof`: the worst half (or quarter) of the flagged tetrahedra is
  refined instead (on the ADRV board 0.93.0 stopped after 2 passes).

## 0.93.0 (2026-11-24)

3D FEM: a low-memory solver, so big models converge:

- Above 150,000 unknowns the FEM factors its matrix in single precision
  (half the memory) and polishes the answer to full double precision with
  flexible GMRES, typically in 3 to 8 iterations. Single precision alone
  loses the static (gradient) part of the field at low frequencies, so
  the polish solves that part exactly, and sweep points below 40% of the
  adaptive frequency use a factor taken at that 40%. Same S-parameters
  as the direct solver to 1e-6.
- The default `--fem-dof` is now 650,000 (about 6.6 GB).
- ADRV9026 RF pair via (TOP to BOTTOM): converged for the first time,
  3 passes, 636,684 unknowns, 6.6 GB, 28 minutes for 10 frequencies. The
  same run with the old solver needed about 12 GB. FEM vs FDTD: S11
  within 2.7 dB (-14.9 vs -17.6 dB at 12 GHz), S21 within 0.07 dB,
  passive.

## 0.92.0 (2026-11-24)

3D FEM mesh fix, and a Smith chart:

- `--fem-dof N` now caps the first mesh too: the starting grid merges the
  lines of small details (pad edges, copper thickness) until it fits a
  third of N, keeping the box, layers, barrels and traces. ADRV9026 RF
  pair via: first pass 25,200 tetrahedra (was 61,000 and 347,000
  unknowns), so several passes fit; near the limit each pass refines
  fewer tetrahedra, and a pass that would pass N is not taken. ADRV pair
  at the default 400,000: two passes in 2.2 GB and 5 minutes (was one
  pass in 9.8 GB), but still not converged (ports off by up to 10% in
  power): its accurate answer needs the low-memory solver that comes
  next. The FEM check now says so when the power balance is off.
- Ports with a coarse face are refined before the first pass.
- No more false shorts on coarse meshes: an edge or face is conductor
  only when all its points are on the sheet or inside the solid, so an
  edge from a via in its antipad to the plane beyond no longer joins them.
- The top layer's copper thickness is air beside a top trace, not
  laminate (the two ports of a TOP to BOTTOM pair now agree within 3%).
- Via wave ports sit closer to the via (they need only clear its fringing
  field).
- `trace via <stackup.txt> ...`: a via in a stack-up file alone, with no
  board.
- The FEM result is saved as `<name>_fem.s2p` beside the via's Touchstone.
- Field animations: a `.gif` output of `trace via --fields` is always the
  animation (it was written as Touchstone).
- `trace help` lists `--check fem` and `--fem-dof`.
- Smith chart: `trace smith <file.sNp>... [--param S11] [--markers GHz,..]
  [-o chart.svg]` draws S11 (or any Sij) with markers reading Z, VSWR and
  return loss; in the app, Analysis > Smith chart (Touchstone files and
  the via window's last result; hover reads, click drops a marker).

## 0.91.1 (2026-11-19)

3D FEM via model:

- Differential pairs: `trace via ... --pair P --check fem` solves the
  pair with an odd-mode wave port (Sdd11/Sdd21, Z0 differential), each
  via its own conductor; oval antipads (`--oval`) as one capsule hole.
- Thick copper: each trace is its real thickness of perfect conductor, so
  line impedances now match the stack-up (CutiePi 50 ohm line: 47.6-52.7
  ohm in FEM, was 55-58 as zero-thickness sheets).
- `--fem-dof N` caps the unknowns (memory: roughly 40 kB each).
- Port Z0 must settle within 0.5% between passes (8 mil stripline: 55.9
  against 56.5 ohm exact).
- Starting grid grades less (thin copper no longer fills the box with
  fine lines).

## 0.91.0 (2026-11-18)

3D FEM EM solver, second stage:

- Second-order elements (HFSS's default): 20 unknowns per tetrahedron
  (hierarchical edge, edge-gradient and face functions), on a coarser
  starting mesh. WR-90 waveguide now within 0.0007 of the exact S21.
- Modal wave ports: each port absorbs its mode with the exact modal
  condition (electric shape from the 2D solve with the dielectrics, the
  magnetic shape from the one without), not a local one, so microstrip and
  other mixed fills stay lossless: a lossless microstrip now gives S11
  -61 dB and |S21| 1.000 (it lost 3% of the power before).
- Port Z0 from a finer second-order 2D solve; adaptive passes also wait
  for every port's Z0 to settle within 1% (8 mil stripline: 55.4 against
  56.5 ohm exact for a zero-thickness strip).
- New solver for the 3D system: symmetric indefinite (Bunch-Kaufman LBLᵀ)
  with minimum-degree ordering, about 2.5x the unknowns of the general LU
  in the same memory (285,000 unknowns in under 15 GB).
- Board vias: `trace via <board> <from> <to> --check fem` solves the same
  via as the FDTD solver (planes, antipads, pads, barrel and stub, return
  vias, both traces, wave ports, reference planes at the via) and prints
  both side by side. CutiePi TOP to L03, 10 GHz: |S21| within 0.13 dB of
  FDTD over the band; S11 within about 2-6 dB (FEM copper is a zero-
  thickness perfect conductor, so its traces read 55-58 ohm, not 50).
- Next: the openEMS check on the ADRV RF via, thick copper, and a window.
## 0.90.1 (2026-11-18)

RF report, from the 0.89.1 run on the ADRV9026 EVB:

- A connector is not taken as the chip a path ends at (FPGA_REF_CLK
  ended at the FMC connector, not the clock chip); it ends a path only
  when nothing else is reached.
- A termination is taken as the load with the chip high impedance
  behind it when it sits within a tenth of a wavelength of the chip (a
  twentieth before, which turned away clock-chip terminations 140 to
  180 mil from the pin).

## 0.90.0 (2026-11-18)

3D finite-element EM solver, the HFSS way (first stage):

- Frequency-domain FEM on a tetrahedral mesh with edge (Whitney/Nedelec)
  elements, so the field is solved in full 3D, not on a stepped grid.
- Wave ports: each port solves its own 2D cross-section for the mode
  (quasi-TEM lines: Z0 and Er,eff from the field with and without the
  dielectrics; waveguides: TE10), launches it and absorbs it. Walls are
  perfect conductor or a radiation boundary; dielectric loss (tan d).
- Adaptive meshing: solve at the adapt frequency, estimate each
  tetrahedron's error (residual and field jumps across its faces), split
  the worst 30% (longest-edge bisection, the mesh stays conforming, port
  cross-sections are refined from their own 2D error), and repeat until no
  S-parameter moves by more than the target dS (0.02) between passes.
  Then the final mesh sweeps the band; S renormalised to 50 ohm (or kept
  at the port's own Z0), Touchstone out.
- CLI: `trace fem waveguide|microstrip|stripline key=value ... -o x.s2p`
  (w, h, len in mil; er, tand; f = adapt GHz; sweep=lo:hi:n; ds; passes).
- Checked: WR-90 waveguide at 10 GHz, S11 0.003 and S21 within 0.004 of
  the exact answer; 8 mil stripline in 20 mil Er 4: Z0 54.3 ohm against
  56.5 exact, Er,eff 4.000, power conserved; 10/5 mil microstrip Er 4:
  S11 below -30 dB to 20 GHz, phase within 1%.
- Next stage: second-order elements (HFSS's default, for tighter Z0),
  board vias from the stack-up checked against openEMS, and a window.
## 0.89.1 (2026-11-18)

RF report and path loss, from the 0.88.1 run on the ADRV9026 EVB:

- A path ends at the largest chip it reaches, then the nearest: DEVCLK
  ended at the clock chip's output on the same net (0.88.1 took the
  nearest) and now reaches the transceiver again; the VCXO tune line
  still ends at the clock chip, not the VCXO.
- A balun's balanced nets taken as a pair get their polarity from their
  names (+/-, _P/_N), else the lower pin number is the positive line.
- Other baluns on the path's nets (alternatives behind 0 Ω selectors, as
  T409/T410 on the LO inputs) are named in a note: the board does not
  say which is fitted. Alternative footprints stacked on the same pads
  are not counted.
- RF report:
  - A termination more than a twentieth of a wavelength from the chip
    is no longer taken as the load with the chip open behind it (an open
    line past it made a 100 Ω termination read -0.8 dB): the chip is
    taken as the line's impedance, with a note. Terminating resistors
    earlier on the path are named as a second termination to check.
  - With a chip model, the insertion loss is the power the chip takes
    (mismatch included), not |S21| into the reference.
  - Lines found not to be RF are listed apart below the table, and each
    path's detail gives its return loss into the load.

## 0.89.0 (2026-11-18)

Schematic symbol editor, as Altium's SCH Library editor:

- Symbol editor… (in the schematic panel, or Edit… beside a symbol in
  Place symbol) opens an Altium `.SchLib`, or starts a new library. The
  component list on the left has Find, New, Copy and Delete.
- Draw: a canvas with the component's body and pins on a 10, 50 or
  100 mil grid. Tools: Select (pick, drag, Space turns, Delete removes),
  Pin (click on the body edge, Space turns it, numbers count up), Line,
  Rectangle, Ellipse, Polygon and Text. The pin's connecting end is
  marked with an x; hidden pins show faintly. The panel beside it edits
  what is picked: a pin's number, name (a backslash after a letter
  draws a bar), electrical type, direction, length, place, part,
  description, shown name and number, hidden, negated (bubble) and clock
  (wedge); a shape's line, colour, fill and corners; texts.
- Parts: A, B, C… for multi-part components. "+" adds an empty part,
  Duplicate adds one drawn like the current one with the next pin
  numbers (supply pins keep theirs, as shared pins), Remove part. Pins and
  graphics can sit on every part.
- Pins: the pin table, for this part or all, with Find; number, name,
  type, direction, length and the show/hide, bubble and clock ticks edit
  in place; Add pin, Sort by number, remove.
- Component: name (library reference), description, designator (U?),
  comment, footprints (the first is used), parameters (Manufacturer,
  Manufacturer Part Number, Datasheet…), and a list of problems: pins
  with no number or a number used twice, pin ends off the grid, parts
  with no pins.
- Symbol wizard: paste a pin list (number, name, type, side; commas or
  tabs, as copied from a datasheet table) and get a box symbol with the
  pins on their sides, 100 mil apart, sized for the names; optionally the
  power pins on a part of their own.
- Save writes the `.SchLib` (the first save over a file keeps the old one
  as `.SchLib.bak`); components not edited are written back byte for
  byte. Saved libraries are read again for Place symbol.
- Place puts the part shown on the current sheet. Update sheets redraws
  every placed part of the component on the open sheets: body, pins and
  texts from the library, placed where and how the part stands (turned
  or mirrored, worked out from its pins), designator and value kept,
  wires following pins that moved. One undo step.
- CLI: `trace symwizard pins.csv NAME lib.SchLib [--split-power]` adds a
  symbol from a pin list; `trace update-symbols sheet.SchDoc lib.SchLib
  out.SchDoc` redraws a sheet's parts from a library.

Checked on the SDR card's library: all 129 components read; ADRV9026
(3 parts, 289 pins) written from the editor draws the same pins, shapes
and texts as Altium's own records. Updating rx_rf.SchDoc from
SDR_Card.SchLib redraws all 101 parts with no pin moved or lost and the
same ERC result.
## 0.88.1 (2026-11-18)

RF report and path loss, from the first whole-board run on the ADRV9026
EVB (29 paths):

- Nets joined by pour alone (no tracks), such as the TX baluns' balanced
  nets, are followed: their pads are taken as joined with no length.
  The TX paths stopped at the balun and took it as a 0 Ω link to its
  unused pin; they now reach the transceiver.
- A balun reached from its unbalanced side whose two other signal nets
  are no pair yet (unnamed nets, as on the LO baluns) makes them a pair,
  followed on through matching parts, so the path is differential after
  it. A balun that is the path's last part sets the end's mode too.
- A path ends at the nearest chip pin it reaches (it took the farthest,
  so the VCXO tune input ran back through 0 Ω links to the VCXO's own
  output).
- RF report:
  - Lines with a capacitor of 1 nF or more to ground (bias, supply,
    tune and control inputs) and power-width copper (under 25 Ω) are
    listed as "not RF" and not judged; DC jacks (barrel, PJ-) are not
    taken as RF connectors.
  - A 0 Ω resistor to ground on a path is taken as an assembly option
    not fitted, with a note (fitted, it would short the path).
  - Resistors at the chip end (after the last series part) of about the
    end's impedance, to ground or across the pair, are its termination:
    with no chip model the chip is taken as high impedance behind them
    and the return loss judged into them, and the insertion loss counts
    the power into the termination as delivered (a 100 Ω across a
    100 Ω pair cost 3 dB before).
  - Path names split a balun's "A > B" nets, so RX1 is RX1_IN.

## 0.88.0 (2026-11-18)

Design rules editor, as Altium's PCB Rules and Constraints Editor:

- Design > Rules (or Rules… in the Properties panel) lists every rule of
  the board in a tree by category (Electrical, Routing, SMT, Mask, Plane,
  Testpoint, Manufacturing, High Speed, Placement, Signal Integrity) and
  kind, in priority order. 45 kinds are known with their fields; kinds
  Trace does not know are kept under Other with all their parameters.
- Each rule: name, comment, enabled, priority (Increase / Decrease; the
  smallest number wins), one scope or two for rules between objects
  (clearance, hole to hole, silk), and its constraints: lengths in mil,
  choices, ticks, and a per-layer table for Width and Differential Pairs
  Routing (TOPLAYER_PREFWIDTH...). "All parameters" edits the raw keys.
- Scopes are Altium queries, checked as you type: a scope that does not
  read is shown in red, terms Trace keeps but does not judge (IsBGA,
  OnTopSilkscreen) in amber. Quick picks write InNet, InNetClass,
  InDifferentialPair(Class), OnLayer, InComponent or WithinRoom; Test
  counts the nets, tracks, vias and pads a scope takes; Select selects
  them on the board.
- + New rule (defaults of the kind, lowest priority), Duplicate, Delete,
  and Add default rules for boards that have none (Allegro, Gerber),
  sized from the board's narrowest track and most common via.
- "Which rules apply to net" shows, for the rules as edited, the rule of
  each kind that wins for a net, with its width per layer and pair sizes.
- Problems (names used twice, shared priorities, lengths that do not
  read, min above max, scopes that do not read) are listed before Apply.
  Apply makes the change one undoable step and DRC runs again on it.
- The interactive router starts each route at its net's rules: the
  preferred width of the Width rule that takes it (per layer where the
  rule says), the clearance of its Clearance rule, the via of its Routing
  Via Style rule, and for pairs the width and gap of the Differential
  Pairs Routing rule per layer. "Rule sizes" in the router bar turns it
  off.
- Saving a PcbDoc writes the rules: edits over the original's records, and
  new kinds the file had none of from Trace's own Altium records. Boards
  saved fresh (from Allegro, Gerbers) now carry their rules. Rule comments
  are kept.
- CLI: `trace rules <board>` by category with priorities and values;
  `--net <net>` the rule that applies to a net; `--check` problems and
  scope terms Trace does not judge; `--add <Kind> <scope> KEY=VALUE...
  -o out.PcbDoc` and `--defaults -o out.PcbDoc` change rules from scripts.

## 0.87.0 (2026-11-18)

Schematic Drag and Move, buses and signal harnesses, as in Altium:

- Drag or Move: dragging parts keeps their wires connected, rubber-banded
  and bent square (Altium's Drag). Tick "Move breaks wires" in the side
  panel for Altium's Move, where only the selection moves and the wires
  stay; Ctrl at the start of a drag does the other one.
- Wires drag by the piece taken hold of: a level or upright segment moves
  only across itself with its neighbours stretching, a corner moves
  freely, and an end on a pin keeps the pin with a new leg back to it.
  Wires teeing onto the segment follow, junctions come and go.
- Buses: B (or the Bus tool) draws a bus; Bus entry places 45 degree
  entries (Space turns them). Select a bus and Break out lays out, for
  D[0..7], an entry, a wire and a net label per member (100 mil apart,
  labels written along upright wires) and a label naming the bus.
  As in Altium a bus carries nothing itself: its members join by their
  own labels, and a port or sheet entry named D[0..7] joins D0 to D7 on
  the sheet below to D0 to D7 above (flat projects: ports of that name).
- Signal harnesses: the Harness tool places a harness connector of a
  type (SPI) with its entries (MOSI, MISO, SCK, CS) on the left or right;
  Signal harness draws the thick line from its open side to a port, a
  sheet entry or another connector; the Port tool places ports. Wires on
  the entries are the harness's members: connectors joined by a signal
  harness join entry by entry, a harness port joins the connector on the
  sheet entry above, and the nets are named SPI1.MOSI and so on. Select a
  connector to change its type and entries.
- Buses, bus entries, connectors and signal harnesses select, box-select,
  move, turn, mirror, copy, paste and delete with everything else; a
  moved connector drags its wires and signal harness with it.
- Altium sheets: buses (record 26) and bus entries (37) are read and
  written; harness connectors, entries, types and signal harnesses
  (215 to 218) are read from and written to the Additional stream, keeping
  the colours and ids of those already in the file; a new port on a
  signal harness is written with its harness type.
- The ERC no longer takes a wire ending on a bus entry or harness entry
  for a loose end. Sheets drawn as SVG (`trace convert`) show buses and
  harnesses.
- Tested by reading back Altium sheets with a bus breakout and a harness
  (every net the same after writing, with and without the original file)
  and with a two-sheet project joining D[0..1] and SPI1 through the
  hierarchy in hierarchical, strict and flat scopes.

## 0.86.0 (2026-11-18)

Push-and-shove routing that moves more than one track:

- Route modes as in Altium (Shift+R or the route bar): Walkaround,
  Push obstacles, Hug and push, Stop at obstacle. Push obstacles pushes
  along the route as drawn and stops at what cannot move (pads); Hug and
  push goes round pads, close to them, and pushes everything else, and
  walks round when the push cannot be done.
- Chains: a pushed track pushes the tracks behind it, and those push the
  next, however many deep (up to 64 lines of track and 32 vias).
- Vias move: a via in the way moves the least distance that clears the
  route, to a spot clear of pads, and the tracks ending on it follow, on
  every layer it joins. A via pushes tracks on all its layers. Pressing V
  in a push mode pushes copper out of the new via's way too, and the
  preview's push is no longer lost when changing layer.
- Pushed tracks hug: only the stretch near what pushes them moves, at
  0°/45°/90°, leaving and rejoining the old path as late and as early as
  it can; the rest of the track stays put.
- Differential pairs move together: a pushed pair net takes its partner
  with it and keeps the pair's spacing through the bends.
- Rules: everything moved keeps the board clearance and each net's own
  clearance rule, checked again once the push is worked out; copper
  already tighter than the rules on the board does not block a push.
- Tested on the CutiePi board (198 of 200 pushes, about 2 ms each) and
  the BeagleBone Black (108 pushes in its dense DDR area, worst 145 ms),
  with no clearance break left by any push (`pushcheck` example).

## 0.85.0 (2026-11-18)

Schematic editing at the level of everyday Altium work:

- Select many things at once: drag on the empty sheet for a selection box
  (left to right picks what is wholly inside, right to left what it
  touches; blue and dashed green as on the board), Shift or Ctrl+click
  adds or takes away, Ctrl+A selects the whole sheet, Esc clears. Parts,
  wires, net labels, power ports, ports, junctions and No ERC markers can
  all be picked. Panning moved to the right or middle mouse button (the
  left still pans with the Wire, Label and other tools).
- Drag a selection to move it as one: wires inside move whole, wires to
  parts left behind stretch and bend square to keep hold of their pins.
  Space turns the selection, X and Y mirror it (parts flip their pins,
  graphics and arcs; texts stay readable), Delete removes it, all as one
  undo step. The side panel shows what is selected with Turn, Mirror X,
  Mirror Y, Copy and Delete buttons.
- Copy, cut and paste (Ctrl+C, Ctrl+X, Ctrl+V), on the same sheet or
  another: the paste follows the pointer until a click drops it (Space,
  X and Y work while it floats, Esc takes it back). Parts get the next
  free designators across the sheets and the board, gates of one
  component keep sharing theirs, and the copy's wires join the copy's
  pins. Ctrl+D duplicates a selection the same way (a single part still
  lands beside itself).
- Junctions look after themselves: a dot appears where a wire end, pin or
  branch meets the middle of a wire and goes when fewer than three ways
  meet, after moves, turns, pastes and deletes.
- Power ports (Power port tool, or G): GND, AGND, +3V3, +5V and VCC
  presets or any net name, in any of Altium's 11 symbol styles (bar,
  arrow, circle, wave, power/signal ground, earth, GOST), Space turns it
  before placing. They name the net they touch, move, turn and delete
  with their symbol and name (deleting an Altium power port used to leave
  its symbol behind), and Save as Altium writes them as power port
  records.
- No ERC markers (No ERC tool): mark a pin or wire as left open on
  purpose so the ERC skips it; they move, copy and delete like the rest
  and are written to the Altium sheet (new markers used to be dropped).

## 0.84.0 (2026-11-18)

- Chip L/C parasitics library (`trace_core::lcparts`): the Path loss, the
  RF report (`trace rf`) and `--tune` now model chip capacitors and
  inductors as real parts instead of ideal L and C plus a fixed guess.
  - Capacitors: C in series with ESL (by case and series: 0201 0.18-0.20
    nH, 0402 0.25-0.30 nH, 0603 0.40-0.45 nH) and an ESR that grows with
    frequency (electrode skin loss plus the dielectric's loss tangent; C0G
    for pF parts, X7R/X5R above 1 nF unless the text says C0G/NP0).
  - Inductors: L in series with DC resistance plus skin loss, chosen to
    give the series' Q, with the winding capacitance across that sets the
    SRF (and grows a little with L, so 47 nH 0402 resonates near 1.8 GHz).
  - Families fitted to typical datasheet values: Murata GJM03/GJM15/
    GQM18/GQM22, GRM03/GRM15/GRM18, LQW15AN/LQW03AW/LQW18AN, LQG15HS,
    LQP15MN, LQP03TN/LQP03HQ; Coilcraft 0201HL/0201DS, 0402HP(CS)/0402DC,
    0603HP(CS). A part is matched by its part number (or value,
    description, footprint, library ref), else a generic family for its
    case (0201 thin-film, 0402/0603 wire-wound inductors; MLCC caps).
    A vendor `.s2p` given with `--model` still wins.
  - Each path's notes flag an inductor whose SRF falls inside or below the
    band (it is a capacitor above it, with what it looks like at the top)
    and a pF capacitor that resonates inside the band. The RF report lists
    every part on a path with its model (family, SRF, Q, ESL/ESR or DCR).
  - The tuner's candidates use the generic family of the site's case, so
    it no longer picks inductors that are past their SRF in the band.
- `trace lc <value> [--case 0402] [--family LQW15AN] [--band 0.1-6]`:
  SRF, Q, ESR/ESL and |Z|, R, X, Q and the value it looks like across the
  band for one part; `trace lc --list` lists the families.

## 0.83.0 (2026-11-18)

- Whole-board RF report (Tools > Signal and power integrity > RF report…,
  `trace rf`): every RF path on the board in one run. Each coaxial
  connector's signal pin (SMA, U.FL, MMCX, SMP and the like by name, or
  any connector whose one signal pin sits among ground pins, as a maker's
  part number often hides the SMA) starts a path, followed as the Path
  loss does it through series parts and baluns to the farthest chip pin.
  - Each kind of via is solved in 3D once for the whole board (the solves
    are shared between paths); `--quick-vias` for the coaxial estimate.
  - Paths are named from their nets (the chip end's named net first) and
    sorted by class: RX, ORX, TX, LO, clock, other RF. A path between two
    connectors is reported once.
  - Chip port models per path: `--load PATTERN=chip.s1p` (repeatable),
    the pattern matched against the end pin (U101.H17), its net, the
    path's name or its class (`--load "RX*=ADRV902x_RX2_RX3_SEDZ.s1p"`).
    Where one matches, the return loss is judged into the chip.
  - Judged in the band (`--band 0.5-6`): the worst return loss from the
    bottom against the goal (`--rl 10`), and the insertion loss at the
    top against a budget when one is given (`--il-budget 2`). Each path
    lists the sections that reflect most (impedance against 50 Ω, or
    100 Ω in a pair, on runs long enough to matter, vias and parts) and
    those that lose most, so a path to check says where to start.
  - Output: a table (IL at points in the band, worst RL, length, vias,
    parts, pass or CHECK) with each path's detail and notes, CSV, one
    chart of every path's IL and RL coloured by class, and with
    `--out DIR` each path's Touchstone and plot. `--only J3*` for some
    connectors, `--from PIN` for starts that are not on a connector.
    Exits 1 when a path needs a look.
  - The window runs it in the background with a cancel button, shows
    the table, and a path's detail with its reflections and losses (click
    to show them on the board); "Open in Path loss…" takes the path, its
    band and its chip model to the Path loss window for the section
    table, the tuner and the eye.

## 0.82.1 (2026-11-18)

- Thermal package guesses, from the ADRV9026 rerun (an LDO came out the
  hottest part on a leaded guess of 58 / 70 K/W):
  - Five equal SOT-23 leads were taken for an exposed pad (the "next
    largest pad" skipped equal ones); now they are leaded.
  - An exposed pad drawn as several pads (paste windows, split pads,
    thermal vias as pads) is found by the copper near the body's centre,
    and an SOT-223 / DPAK / D2PAK tab (3 or 4 pads, one large) counts as
    an exposed pad: a 3 mm LFCSP LDO gets θJB about 13 K/W, an SOT-223
    about 8.
  - Leaded θJB tops out at 50 K/W (SOT-23 and SOIC-8 datasheets give
    about 35 to 55), where it reached 120.

## 0.82.0 (2026-11-18)

- openEMS as a second, independent solver for vias and pairs. The
  transition Trace's 3D solver runs (stack-up, planes with round or oval
  antipads, pads, barrels and stub, return vias, traces, the resistors at
  their ends and the probes on them) is written as an openEMS model
  (CSXCAD XML, lengths in mil, the same graded grid and PML, on half
  Trace's smallest cell by default as openEMS draws round metal in steps).
  openEMS is run as an external program (found in C:\openEMS, through
  `TRACE_OPENEMS`, or on the PATH), with progress from its energy decay
  and a Cancel that stops it. Its voltage and current probes are read
  back and turned into S-parameters by the same power-wave extraction and
  barrel loss Trace uses, so the two results differ only by the solvers.
- `trace via ... --check openems` solves in Trace, then in openEMS, and
  prints both side by side with the difference (worst |S11| and |S21|
  gap, via impedance from each), and saves compare.svg (openEMS dashed
  over Trace), trace.s2p, openems.s2p and compare.txt in the run folder
  (`--openems-dir`, default `<board>_openems`). `--openems-exe`,
  `--openems-cell`; `--export-openems DIR` only writes the model and
  `--openems-read DIR` reads a run finished elsewhere.
- Via window: Check with openEMS (dashed curves on the plot, the gap
  above it, Cancel beside the spinner) and Export openEMS model.

## 0.81.2 (2026-11-18)

- Thermal: package resistances, when not given, are guessed by package
  kind as well as size, from typical JEDEC 2s2p datasheet values. A ball
  grid (many round pads of one size) gets a low θJC and θJB (14 mm: about
  1 and 4 K/W, where 0.81 guessed 8.9 and 18.4 and so ran large BGAs like
  the ADRV9026 far too hot); an exposed-pad QFN a high θJC through the
  mould and a low θJB (5 mm: 30 and 8); leaded parts (SOIC, QFP, SOT)
  high both (SOIC-8: about 55 and 65). Datasheet values with
  `--power U1=4.5W@jc0.6/jb3.9` still win.

## 0.81.1 (2026-11-18)

- EMI simulation, plane cavities: below a plane pair's first resonance
  the voltage is the same all round its edge, and opposite edges cancel.
  What is left radiates as the pair's vertical dipole, k·εr·side/2 of
  one edge alone, so small or low-frequency cavities no longer read as
  strong radiators. At resonance and above nothing changes.

## 0.81.0 (2026-11-17)

- Thermal density maps, as the field maps do for the EM solver. The 3D
  thermal solve now keeps every slab of the stack-up (not only the copper
  layers) and draws, on any slab seen from above or in side views along x
  and y through the hottest point (thickness drawn taller):
  - temperature (°C), smoothed between cell centres;
  - heat flux density |q| = |k∇T| (W/cm²), with the conductivity the
    solver used across each face, so it is the heat really crossing (it
    shows how the planes spread heat and where the vias carry it down);
  - temperature gradient |∇T| (K/mm);
  - power density (W/cm³): the heat each part puts into the board under
    its pads;
  - heat to air (W/cm²): convection plus radiation from each face cell.
  The densities are on a log scale (`--decades`, default 3), temperature
  linear, each on one scale for every plane so layers compare.
- `trace thermal ... maps.svg` writes every quantity on every copper
  layer, the dielectric mid-way between them and the side views
  (`--maps t,flux` picks quantities, `--planes all` every slab, `--at X,Y`
  moves the side views); `maps.gif` sweeps the cut down through the
  layers (`--sweep x|y`: a side view moving across the board), one GIF
  per quantity.
- Thermal window: pick the quantity and the plane (▲ ▼ step through the
  board), with a scale bar; the board overlay follows the chosen slab and
  quantity. Save writes the full SVG page and a GIF too.
## 0.80.3 (2026-11-17)

- EMI simulation: control lines are no longer taken as 50 MHz clocks.
  I²C (SCL, SDA) is taken at 400 kHz with slow edges, SPI and its kin
  (SCLK, SCK, MOSI, MISO, SDIO, CS, JTAG, MDIO) at 10 MHz with 3 ns
  edges, and resets, enables, requests and status lines (CLK_RESETB,
  CLK_SYSREF_REQUEST) at 1 MHz, even when their names hold "CLK".
- EMI simulation: nets that share a name are one line in the report,
  their fields added, instead of the same line twice.

## 0.80.2 (2026-11-17)

- EMI simulation, cables: every connector with a cable is now driven,
  not only the longest one. A net drives each cable fully when it runs
  beside that connector and half from across the board, and the cable
  that radiates most for it is the one reported. Cable lengths by kind:
  power jacks and terminals (or a small connector on a supply net)
  1.5 m, Ethernet 2 m, USB, video and RF test coax (SMA, 2.92 mm, BNC)
  1 m, U.FL and MMCX pigtails 0.1 m, a mated FMC board 0.15 m, headers
  0.3 m, optical cages none; `--cable J1=2m` overrides. The choke fix
  goes on the cable named.

## 0.80.1 (2026-11-17)

- The EMI check and simulation judge against CISPR 32 class B by
  default (10 m below 1 GHz, 3 m above), as Hadi's boards need; FCC
  limits stay one choice away (`--limit fcc-b`).
- EMI simulation fixes from the first runs on the ADRV9026 and DAC39RF12
  boards:
  - Only parts with pads on a signal net count as connectors with a
    cable, and connector words are matched whole: a hot-surface logo
    ("IEC_HotSurface_Small") was taken for an SMA and given a 1 m cable.
  - Sources can be given for a whole kind of net:
    `--signal class:lo=3.5GHz,-6dBm`, `class:rf`, `class:serdes`,
    `class:lvds`, `class:clock`, `class:cmos` (a net's own line still
    wins). The same lines work in the EMI window's Sources box.
  - RF defaults by role: an LO or a transmitter's output 0 dBm, a
    receiver's input (RX, ORX) −20 dBm.
  - The layout check's own radiation note no longer says cables are
    not modelled.

## 0.80.0 (2026-11-17)

- Field maps from the 3D solver. The via and pair solve can now keep
  the electromagnetic fields at a frequency and draw them: |E| (V/m),
  |H| (A/m), energy density ¼(ε|E|² + μ|H|²) (J/m³) and the surface
  current density on the copper (A/m), on cut planes through the
  structure: each trace layer, mid-way through every dielectric the
  barrel crosses (where the via launches waves between the planes), a
  side view along the traces and one across through the via. dB colour
  scale with a scale bar, scaled to 1 W into port 1 (checked: on a
  50 Ω stripline the field between trace and plane is within 5 % of
  V/h and the current round the trace within 15 % of V/Z). "At an
  instant" shows the field at one phase of the cycle; Animate steps it
  through the cycle so the wave moves. In the Via window tick "Field
  maps at … GHz" before Solve; the viewer has the plane, quantity,
  scale and phase, and saves an SVG page of every plane or an animated
  GIF of one. CLI: `trace via b 1 8 --fields 6 map.svg` (or map.gif,
  `--field-plane`, `--field-quantity`, `--field-db`).
- EMI simulation (the EMI window's Simulate, and `trace emi`), in place
  of the single-clock estimate:
  - Every signal net and pair is a source, not only the clocks: its
    IBIS driver (edge and swing from [Ramp]) when given, your line
    (`NET=12Gbps@30ps,0.8V`, `=125MHz`, `=2.4GHz,0dBm`), else a
    default for its kind found from its name: clocks at the frequency
    in the name (CLK_125M, REF_245M76) as lines; SerDes (JESD, GT,
    PCIe, SATA, USB3, HDMI, SFP) as PRBS at their usual rates; LVDS,
    USB 2 and MIPI pairs; RF and LO tones; single-ended data. Which
    ones are assumed is listed.
  - Traces: each outer segment and its return under it is a small
    loop; the loops are added as vectors with the travelling wave's
    phase along the net, toward 144 directions, worst kept (one loop
    gives Ott's formula exactly). A pair is its two lines, the negative
    one late by the pair's real length mismatch and 2 % weaker, so
    skew and imbalance radiate as common mode by themselves.
  - Cables: the return current's voltage across the plane (Hockanson's
    current-driven model) drives the longest cable on the connectors
    (coax, USB, Ethernet, HDMI 1 to 2 m, a mated FMC board 0.15 m,
    headers 0.3 m; `--cable J1=2m`) as a monopole, capped at resonance.
  - Power planes: each rail's plane impedance from the PDN model (the
    cavity's resonances, its capacitors and regulator), times the
    switching current that gets past the die's capacitance and the
    package, is the voltage at the plane's edge, radiating as a slot;
    and each decoupling capacitor's current in its mounting loop.
    `--rail VCCINT=2A@250MHz` for real currents.
  - Spectrum 30 MHz to 6 GHz against FCC class A/B or CISPR 32 class
    A/B with the top sources drawn, every contribution ranked by
    margin, and fixes scored by re-running the source with the change
    (inner layer, matched lengths, slower edge, common-mode choke, two
    more capacitors): the dB each one gains at the worst frequency.
  - Near field on the board at the worst frequency: |H| 1 mm above the
    top and below the bottom (Biot-Savart of each segment and its
    image) and the current density on every copper layer (the track's
    current and its return spread in the plane under it), drawn over
    the board like the other results; `--map near.svg` in the CLI.

## 0.79.0 (2026-11-17)

- Analysis results on the board. The EMI check, the thermal solve and
  the bus simulation now draw their results over the board as well as
  in their windows:
  - EMI: a marker at each finding (red high, orange medium, blue low,
    labelled with the kind and net) and the nets with a high finding
    drawn red.
  - Thermal: the temperature map of the layer picked in the window laid
    over the board, with each part's junction temperature beside it.
  - Bus: each bit's net green when it meets setup and hold, red when it
    fails, the strobe blue, and the margins at the receiver pin.
  While coloured nets are shown the rest of the board is dimmed. A
  "Results on the board" box in the canvas's top right corner lists
  them, with a tick to hide each and a cross to remove it; the
  analysis windows stay on top of it. Labels that would cover another
  are left out until you zoom in.
- Cancel. Every long job has a "✖ Cancel" button beside its spinner:
  via solve, via sizing, via optimiser, thermal, bus simulation, path
  loss (and its tuner and CTLE/FFE optimiser), PDN (and its decap
  optimiser), IR drop, impedance check, pair cross-sections, via
  stitching and fences, the autorouter and BOM pricing. The solver
  stops at its next check (within a second or two) and frees the CPU;
  the autorouter keeps the nets it has already routed.

## 0.78.0 (2026-11-17)

- Via sizing (`trace via --target`, the via window's Solve with a
  target) now meets the target on the accurate, converged grid, not
  the quick one. The quick grid only steers: each round solves the
  quick answer accurately (grids refined until S11/S21 stop moving,
  then extrapolated to a zero cell size), measures how far the quick
  grid was off there, and searches the quick grid again aimed off by
  that much, until the accurate impedance is within 2 %. The steps,
  each accurate grid study and the final error bars are listed.
- The via optimiser (`--optimise`, the window's Optimise) confirms its
  front runners (up to 3) and the starting setup with the accurate
  solve too, so the winner is picked on converged return loss.
- The accurate solve also extrapolates the via impedance to a zero
  cell size, not only the S-parameters.
- `--quick` (CLI) or unticking Accurate (window) keeps the old single
  finer grid; Accurate is now on by default in the via window and
  applies to Solve, sizing and Optimise. `--accurate dB` sets the
  tolerance sizing and the optimiser use.

## 0.77.1 (2026-11-17)

- Return path (DRC and the EMI check): a track's reference is now the
  nearest two planes on each side, not only the layers right next to
  it, so a partly poured mixed or signal layer beside the track no
  longer hides the solid plane beyond it. On the ADRV9026 board this
  removes the "plane gap" alarms on L11_DSIG, whose real references
  are L9_DGND and L13_DPWR.
- EMI check severity follows the net's speed: a split or gap is high
  for a fast edge (knee at 100 MHz or more, given or a clock by name),
  medium for a pair whose speed isn't given, low for a slow net given
  with --net; a short gap is one step less.

## 0.77.0 (2026-11-17)

- EMI check (`trace emi`, Tools > Signal and power integrity > EMI
  check) of the fast nets: clocks found by name (CLK, SCK, XTAL,
  OSC...), the nets given with their frequency, edge and swing, and
  every differential pair. It reports, worst first, with where and
  what to do:
  - tracks over a gap in their reference plane, or across a split
    between two plane nets;
  - vias that move a net to a layer with another reference plane with
    no return via (same plane net) or capacitor (different nets) near;
  - fast tracks closer to the board edge than five times their height
    over the plane;
  - stretches of board edge with no ground via longer than a
    twentieth of a wavelength at the fastest edge's knee frequency.
- Radiation estimate for each clock: the loop its outer-layer route
  makes with the plane under it (length times height) and its
  harmonic currents (a trapezoid of the swing over twice the line's
  impedance), into Ott's differential-mode formula over a ground
  plane, against FCC Part 15 or CISPR 32 class A or B at their
  distances. Margins per net, spectrum chart (SVG) and findings (CSV);
  exits 1 on a high finding. "Show" in the window selects the net and
  zooms to the spot.
- Cables (common-mode radiation) are not modelled; the estimate is
  the board's own.

## 0.76.0 (2026-11-17)

- Bus simulation (`trace bus`, Tools > Signal and power integrity >
  Bus simulation): a DDR byte lane, an address bus or any
  source-synchronous group simulated bit by bit. Each bit's routed
  path is cascaded from the board (traces, vias and parts, as in
  `trace path`) and its eye built with the nets beside it switching,
  the bus's other bits among them. The strobe or clock (a pair is
  found by its name or either line, DQS0 / DQSN0 included) is sent
  as a clock with the same aggressors.
- From each eye comes the window in which the bit is valid at the
  receiver, measured from the moment it leaves the driver, so the
  flight time, loss, reflections and crosstalk all show in it. The
  strobe's edge lands at its own flight time plus its launch offset
  (half a bit when centred, as DDR sends it) with its own jitter.
- Per bit: length, insertion loss, skew, eye height and width,
  crosstalk closure and the worst aggressor, setup and hold margins
  against the receiver's tSU/tH, and the receiver mask (VdIVW over
  TdIVW, DDR4 and later) at the strobe.
- For the whole bus: the window where every bit is valid, and the
  margin as routed, with the strobe centred on the lane (training),
  and with each bit deskewed (per-bit training). Timing chart (SVG),
  CSV and a text report; exits 1 when a bit fails.
- Eyes: a clock pattern for strobes; the threshold crossings and the
  eye's inner edge are kept; the "pulse has not settled" note no
  longer fires on slow edges.

## 0.75.0 (2026-11-17)

- Thermal analysis (`trace thermal`, Tools > Signal and power
  integrity > Thermal): a 3D steady-state finite-volume solve of the
  whole board. Every copper layer is sampled from the real tracks,
  pours and pads (in-plane conductivity bounded from both sides, so a
  hatched or broken plane is not treated as solid), every via barrel
  carries heat between layers, and the dielectric uses the stack-up
  material (FR-4, polyimide, Rogers, PTFE, ceramic, metal core).
- Parts use the JEDEC two-resistor model (junction to case top and
  junction to board, from the datasheet; sensible guesses from the
  package size and exposed pad when left empty). Junction heat enters
  the board through the part's own pads, shared by their copper.
- Heatsinks: a plate-fin sink on any part with its base spread solved
  in 2D, fin efficiency, thermal interface material and contact
  resistance, natural convection between fins (Bar-Cohen and Rohsenow),
  forced air (Teertstra, Yovanovich and Culham) and radiation.
- Board surfaces use the right correlation for the side and
  orientation (heated plate facing up or down, vertical board by
  Churchill and Chu, forced flow over a flat plate) with air
  properties at the film temperature, plus radiation; the solve
  iterates until these settle and checks that heat in equals heat out.
- `--check` (grid check in the window) solves again on a 1.5 times
  finer grid and gives every temperature an error bar.
- Results: junction, case, board and heatsink temperature per part,
  share of heat into the board, θJA, and per-layer temperature maps
  (SVG, CSV, text report).
- `trace validate` now checks the solver against an exact Bessel
  solution for a heated disc on a copper plate (0.2%), the exact fin
  efficiency, an energy balance, the grid check, and JEDEC 2s2p
  boards for SOIC-8 (7% from the datasheet θJA) and SON-8 (17%).

## 0.74.0 (2026-11-17)

- Accurate 3D via solves (`trace via ... --accurate [dB]`, Accurate in
  the via window): the via is solved on finer and finer grids (0.7
  times the cell each time) until |S11| and |S21| stop moving, up to 4
  grids and 9 M cells. When the last three grids converge steadily
  the answer is extrapolated to a zero cell size (Richardson); every
  frequency gets an error bar from Roache's grid convergence index
  (1.25 times the step to the limit, or 3 times when the grids are not
  yet converging steadily, keeping the finest grid). The table shows
  each grid's worst S11, S21 and via impedance, then the dB range of
  S11 and S21 per frequency.
- Conformal metal (Dey-Mittra) is now on in every via solve: round
  barrels, pads and antipads cut the grid cells where they really are
  instead of in steps, and the faces they cut use only their free
  edges and area (thin slivers borrow area so the time step stays
  stable, at about 20% more steps). On a through via the 2 mil grid
  went from 59.1 to 53.0 ohm and S11 at 5 GHz from -24.3 to -32.9 dB,
  where a 0.55 mil stepped grid (20 times the time) gives 53.7 ohm and
  -31.4 dB; the stepped grid's error on low return loss was 5 to 18 dB.
  `--staircase` gives the old grid.
- Every via solve ends with a physics check: passivity (|S11|^2 +
  |S21|^2 at most 1) and causality (impulse response energy well
  before t = 0, allowing for the band limit).
- `trace sparam check file.sNp...`: passivity (largest singular value
  of S at each frequency), reciprocity (Sij = Sji) and causality of
  any Touchstone file, from Trace, a vendor or another solver.
- `trace validate` checks that accurate mode's error bars hold a finer
  grid's answer and that its result is causal.
- BOM: logic parts whose value starts with NC (NC7WZ16, NC7WV125) are
  no longer read as not connected, and passive footprint names such as
  R0201 or C0603 at the end of an Allegro device type are dropped
  whatever the placed footprint (R701-R703 join their E-number line).

## 0.73.0 (2026-11-17)

- Rigid-flex in 3D: the 3D view gets a Fold slider (shown when the
  board has bends) that bends the flex at each bend by a share of its
  angle, up to the full angle. The board is split into pieces at the
  bend areas; the largest piece stays put and each piece beyond a bend
  turns with it, through chains of bends. A bend curves over its area
  (radius = width / angle) with the board, copper, silk and parts on it
  cut into strips along the fold line, so it bends smoothly. Without a
  set fold line, a long narrow bend area folds about its long side.
- BOM on Allegro and ADI-style boards (tested on the ADRV9026 EVB):
  parts valued TBD, DNI, DNP, NC and the like are not fitted;
  footprints stacked on the same pads as alternatives are counted once
  (a capacitor before an inductor before a resistor, else the first
  designator) and the others listed; the footprint tail of an Allegro
  device type is dropped so one part code on footprint variants is one
  line; a value that is itself a maker's part number (ADRV9025BBCZ,
  2450BL15B200E) is used as the part number, so it can be looked up.
  BOM lines now hold one part number over all its footprints, and a
  part number on two footprints is a warning, not an error.

## 0.72.0 (2026-11-17)

- Bill of materials with part data (Reports > Bill of materials with
  part data…, `trace bom board [--boards N] [--parts file.csv]
  [--mouser KEY] [bom.csv|report.txt]`). Parts are grouped by part
  number, maker, value and footprint for the active variant; parts
  left off by the variant or marked DNP/DNI/not fitted, fiducials,
  drawings and parts marked no BOM are listed apart, and parts without
  pads stay in when they have a part number (cables, screws, boxes).
  Allegro boards, which carry no maker's part number, use the device
  type as a library code. Each part number is joined with its
  lifecycle, stock, price breaks, currency, datasheet and replacement
  from parts files (any CSV with an MPN column and Manufacturer,
  Lifecycle, Stock, Price or Price@1/Price@100 ... columns, such as a
  distributor's export; "Save parts file" writes one to fill in), from
  earlier lookups (kept in Trace\parts-cache.json under AppData), or
  from the Mouser Search API with your own free key (looked up ten at a
  time through the system's curl, within Mouser's 30 a minute). The
  checks: no part number, obsolete, end of life, not recommended for
  new designs, less stock than N boards need (and under five times),
  one part number on two footprints, lines alike in value and
  footprint under other part numbers, library codes instead of makers'
  numbers, and ICs with only one maker's number. Cost per board and
  for N boards at the matching price break. The window filters to the
  lines with problems, selects a line's parts on the board, links the
  part page and datasheet, and saves the BOM, a report or the parts
  file. The Mouser lookup could not be tried against Mouser from the
  build machine; its answer format is tested.

## 0.71.0 (2026-11-15)

- Rigid-flex stack-ups (Design > Rigid-flex zones and bends…, `trace
  flex board [--radius [B1=]MIL] [--dynamic] [report.txt]`). A board
  can have zones, each an area with only some of the stack-up's layers
  (a flex zone keeps its polyimide and copper, a rigid zone has them
  all, a stiffened flex zone may carry parts), and bends inside flex
  zones with an inner radius, an angle, static or dynamic use and a
  fold line (set, or square to the tracks crossing the bend). ODB++
  jobs bring their zones (the step's `zones` file: each zone's outline
  and layer rows) and BEND_AREA and STIFFENER layers in. The check
  follows IPC-2223: vias in flex and within `--keepout` (30 mil) of a
  rigid-flex edge, parts on flex that is not stiffened and in bends,
  copper on a layer the zone does not have, tracks not crossing the
  fold line square to it (within `--angle`, 10 degrees), corners in a
  bend, tracks stacked on two layers in a bend, solid copper in a bend,
  bends outside flex, and the bend radius against IPC-2223's minimum
  (6, 12 or 24 times the flex's thickness by its copper layers, 100
  times for dynamic flex). The window edits zones (flex, stiffened,
  layers) and bends, adds them by box, applies them as one undoable
  step, draws them on the board with the fold lines, and lists the
  issues to click through. On the ODB++ rigid-flex sample the flex is
  7.7 mil with 2 copper layers (rigid 53.2 mil, 10), so its bend needs
  at least 93 mil.

## 0.70.0 (2026-11-15)

- Design compare (Project > Compare with another version…, `trace
  compare first second [report.txt|.csv]`): what changed between two
  versions of a board. Parts by designator: added, removed, moved
  (past `--tol`, 1 mil), rotated, flipped, footprint, value and part
  number. Nets by name: added, removed, renamed (the same pins under a
  new name), and pins joined or left. Routing net by net: length (past
  `--length-tol` 10 mil and 2 %), vias and layers. Net classes,
  differential pairs, rules (on/off, priority, scope, each parameter),
  the stack-up layer by layer (name, kind, thickness, Er, Df, material)
  and the outline. The window reads the other board in the background
  (pick it or paste its path), says which is the newer one, filters by
  area or text, shows each change on the open board, and saves the
  report as text or CSV. The CLI exits 1 when the boards differ.

## 0.69.0 (2026-11-15)

- Crosstalk in the eye (Eye diagram > crosstalk, `trace path --eye 5
  --xtalk`): the tracks running beside the path's traces on the same
  layer (within four dielectric heights) are found, grouped by net or
  pair (pairs from the board or by name, P/N), and coupled in with
  their backward (NEXT, Kb (1 - e^-j2wt)) and forward (FEXT, Kf jwt)
  coupling from the coupled-line modes. Each aggressor switches at the
  same rate with its own part of the pattern, at its worst phase and in
  its worse direction (driven from the receiver's end or alongside); the
  eye's traces, height, width and jitter include it, and each one's
  worst case closure and peak are listed (click to show it on the
  board). A pair victim takes its lines' difference, so noise both
  lines pick up alike cancels. The noise is taken where it couples,
  without the loss on its way to the receiver, so it is a bound.
- The path report keeps the nets its traces run on (and their pair's
  other line), for the crosstalk.

## 0.68.0 (2026-11-15)

- Equalisation in the eye (Path loss window > Eye diagram, `trace path
  --eye 16 --eq auto`): a 3-tap transmit FFE (pre and post taps, sharing
  the driver's swing with the main tap; `--ffe -0.05,-0.15`) and a
  receive CTLE (one zero, two poles, DC this many dB below the peak;
  `--ctle 6@8` for 6 dB at 8 GHz, default half the bit rate). "Pick
  best" (`--eq auto|ffe|ctle`) tries taps in 0.025 steps and CTLE
  peaking 1-18 dB at 0.5, 0.75 and 1 x half the bit rate, about 5800
  settings in a second or two, and keeps the one with the largest worst
  case eye area (the opening from the pulse's cursors summed across the
  bit), so it does not buy height with width. The eye then samples at
  the point whose opening holds best a quarter bit either side.
  Clocker IC1.6 to J15.1 at 20 Gbit/s: 88 -> 100 mV, width kept.
- The eye's pulse is sampled with turned phasors instead of a sine per
  point, several times faster on fine sweeps.

## 0.67.0 (2026-11-15)

- Eye diagram from the path (Path loss window > Eye diagram, `trace path
  --eye 10`): the path's S-parameters, driven by a matched source and
  ended in the chip's model when one is given (`--load`), give the
  response to one bit with a Gaussian edge (`--rise`, 20-80 %, default
  30 % of a bit). A PRBS pattern (`--prbs` 7, 9, 11 or 15) is built from
  it by superposition and folded into an eye two bits wide: height and
  width at the centre, jitter from the threshold crossings, and the
  worst case opening from the cursors (peak distortion). The window
  draws the eye and saves it as SVG with the pulse response and its
  cursors; "Sweep for it" sets the top of the sweep to 2.5 x the bit
  rate and a fine step, and the CLI does the same unless `--ghz` or
  `--points` is given. Notes say when the sweep stops too low for the
  edge, when the pulse does not settle within the sweep's period, and
  when the ports differ (a balun from 100 to 50 ohm scales the voltage).

## 0.66.0 (2026-11-15)

- Matching tuner (Path loss window > Tune the match, `trace path --tune
  1-6`): chooses E12 capacitors and inductors, with their case's ESR,
  ESL and self-resonance, for the empty part sites on the path (TBD,
  DNI, not fitted, or parts named to choose again with `--free`), for
  the best worst-case return loss over the band (into the chip's model
  with `--load`) plus `--il-weight` times the mean insertion loss. Each
  extra part costs a little, so a site stays empty unless a part helps.
  Series sites take a 0 Ω link or a part; shunt sites stay open or take
  a part, through the stub that leads to them. C sites take capacitors
  and L sites inductors unless `--any`. The window lists the parts
  chosen (click to show them on the board), the before and after worst
  return loss and insertion loss, and the tuned path's plot and table.
- Path loss, from the ADRV0926 RX1 retest:
  - Footprints stacked on the same pads (C302 18 pF, R302 0 Ω and
    L302 TBD on one site) are alternatives: one is taken, a capacitor
    first, then an inductor, then a resistor, or the ones `--fit` names
    (Fitted where footprints share pads, in the window), with a note.
    0.65.1 put them all in parallel, so the 0 Ω link hid the DC block.
  - A single-ended chip model (50 Ω, such as ADI's SEDZ port models) at
    a pair's end is taken as the single-ended equivalent: the
    differential impedance is twice its own (0.65.1 used it as is).
  - An empty shunt site keeps its pad and the stub to it (an open stub)
    in the path.

## 0.65.1 (2026-11-15)

- Path loss, from the ADRV0926 RX1 test (J301 to U101.H17):
  - Parts valued TBD, DNI, DNP, NC or N/A, or not fitted in the variant,
    are left out (a shunt site stays open) and listed in a note, instead
    of taking 100 nF or 10 nH. A capacitor, inductor or resistor whose
    value gives no farads, henries or ohms is treated the same way.
  - Parts on the same two nets combine side by side, so a fitted 18 pF
    DC block beside an unfitted TBD inductor footprint counts (before,
    the path took one of them at random). A series site with nothing
    fitted is a 0 Ω link, with a note.
  - An ideal open as a vendor model (Γ = 1) is an open, not a short.
  - `--fmin` (RL from, in the window): the worst return loss is taken
    from there up, so a DC block's corner below the band does not hide
    the in-band match.
  - With `--load`, the worst return loss with the chip at the end is
    taken inside the load model's valid range (its "Valid Frequency
    Range" comment, else its points), with a note when the sweep runs
    outside it, and is printed at 1, 3 and 6 GHz.

## 0.65.0 (2026-11-15)

- Whole-path loss (Tools > Signal and power integrity > Path loss…,
  `trace path`): pick a start pin (J503.1, a part with one signal pin, or
  a net) and an end pin (or the farthest chip pin), and Trace follows the
  routed copper between them, through series parts and baluns, and
  cascades every section into one insertion and return loss to your GHz:
  - Traces: each run (same layer, width and pair gap) from the 2D field
    solver, with the pour beside it, copper roughness and Df; coupled
    pair runs in odd mode, break-outs uncoupled.
  - Vias: each kind solved in 3D (pairs as pairs), or a coaxial estimate
    with the stub below (`--quick-vias`).
  - Parts: a vendor Touchstone file by designator, value or part number
    (`--model PATTERN=file`: .s2p series part, .s1p shunt part, .s3p
    balun with port 1 unbalanced, converted to mixed mode), or a model
    from the value and case (capacitor ESR/ESL, resistor, inductor,
    bead, ideal balun from its 1:2 or 50:100 ratio).
  - Shunt parts off the path (to ground or across the pair) with the
    stub that leads to them.
  - Pairs the board does not define are followed through matching parts
    on both lines (AC caps to a balun).
  - `--load chip.s1p`: return loss into the path ending in a chip's port
    model.
  - Output: a section table (impedance, own loss, loss from the start),
    the IL and RL plot, a Touchstone file (version 2 when one end is a
    pair, 100 Ω, and the other single-ended), CSV and SVG.
- Clocker IN_CLK1 (IC1 to the SMA J15 through C5, TR2 and C1, two pair
  vias in 3D): 3.98 dB at 6 GHz, 0.84 dB at 3 GHz.

## 0.64.2 (2026-11-15)

- Impedance check: vias whose barrel is shorter than a twentieth of a
  wavelength at the frequency of interest (microvias, short blind vias)
  are electrically short: listed apart, not failed, with a note that the
  pad capacitance rules them (an ADRV TOP to L2 microvia read 9.7 Ω and
  asked for a 140 mil antipad). The antipad hint says so when no antipad
  that fits beside the pad or pair could reach the target.

## 0.64.1 (2026-11-15)

- Impedance check:
  - Pairs by name: nets X_P/X_N, X+/X-, XP/XN, _DP/_DN, _T/_C that run
    side by side count as differential pairs where the board defines none
    (Allegro boards often define none; CutiePi: 10 found).
  - Electrically short runs (under a twentieth of a wavelength at the
    frequency of interest, 6 GHz by default; `--ghz`) that are off target
    are listed apart, in amber, and do not fail: neck-downs at connector
    pins and caps.
  - Says so when the stack-up has no dielectric thicknesses (nothing can
    be solved) instead of reporting nothing.
- `trace cascade --line Z@1in@…`: line lengths take in, mm or um as well
  as mil.

## 0.64.0 (2026-11-15)

- Impedance check (Tools > Signal and power integrity > Impedance check…,
  `trace zcheck`): every controlled trace run and every kind of via on
  the board against its target impedance, the worst first.
  - Targets: rules by net or pair name (* and ?), every differential pair
    (100 Ω, 90 Ω for USB, 85 Ω for PCIe and SATA) and every net on the
    signal pin of a coaxial connector (SMA, U.FL, MMCX..., 50 Ω).
  - Traces: per net, layer and width from the 2D field solver, pour
    beside the track counted (coplanar); coupled pair runs by Zdiff,
    uncoupled stretches by Z0; with the width or gap that would meet the
    target.
  - Vias: grouped by how they are built (layers joined, drill, pad,
    antipad from the plane holes, pair pitch, ground vias around); each
    kind solved once in 3D for its impedance, with the antipad that would
    meet the target and a button that opens it in the via window to size
    or optimise it.
  - Off by more than the tolerance (10 %) fails; click a line to show it
    on the board; report as CSV.
- Via loss: the 3D via solve now adds the barrel's copper loss (skin
  effect, 1 µm roughness; `--rough`) and its dielectric loss (the
  stack-up's Df) from layer to layer, which the grid's perfect copper
  left out, and says how much; `--lossless` for the old result. Through
  loss is shown to 3 decimals. `trace via … --feed 1in` adds that much of
  each trace's loss (2D field solver) for a channel figure.
- Materials for the via: the via window shows the stack-up's Er and Df
  between the two layers and takes values of one's own ("own": Er, Df for
  every dielectric layer, traces and via alike), with the copper loss
  switch and roughness; CLI `--er 3.6 --df 0.004`. The CLI prints the
  materials and which losses it counted.
- Altium internal planes are read: plane layers join the stack-up as
  copper and are poured from their net over the board, keeping the
  PlaneClearance rule (20 mil when none) round other nets. Before, a
  board with planes lost them, so impedance, return path and 3D via
  results took the next signal layer as the reference (Clocker's top 50 Ω
  width read 90 mil; now 11.6 mil, as designed). Saving over the
  original PcbDoc leaves the planes as they were.

## 0.63.1 (2026-11-15)

- Via optimiser: the quick grid can be 10 dB or more off when the return
  loss is very good, so the four best setups are now always solved again
  on the finer grid (before, only those within 3 dB of the best), and
  the CLI lists them with their fine-grid return loss.
- Sizing to a target impedance: the sized via's S-parameters come from a
  grid 0.7 times as fine (the quick grid overstated its return loss, for
  example -49.8 dB against -38.9 dB on the ADRV pair via).

## 0.63.0 (2026-11-15)

- Via design to manufacturing limits and a target impedance (Via
  S-parameters window, "Manufacturing limits and target"):
  - Limits from the fab's capabilities: drill min to max, annular ring,
    antipad min to max and copper gap. Every setup Solve and Optimise try
    keeps to them.
  - Target impedance (50 Ω single, 100 Ω for a pair by default). Solve
    sizes the via to it: a coaxial first guess, then the antipad by the
    secant method on its 3D solves, and the drill when the antipad meets
    a limit. It stops within 2 % (or 1 Ω), and says how close the limits
    allow when the target is out of reach.
  - Optimise now weighs insertion loss with return loss (worst and mean
    return loss plus mean insertion loss across the band), tries the
    drill too, and solves its best few again on a finer grid to pick the
    winner.
  - The via's own impedance (as a TDR would show it) is fitted from each
    3D solve and shown with the result.
- CLI: `trace via … --target 50 --fab drill=8-16,ring=4,antipad=20-70,gap=4`
  sizes to the target; add `--optimise <GHz>` to optimise within the
  limits after it.

## 0.62.0 (2026-11-15)

- PCB Filter panel: select by a query, as in Altium, such as
  `IsVia And InNet('GND') And HoleSize < 12mil` or
  `IsComponent And Designator Like 'TP*' And OnBottom`. Kinds (IsTrack,
  IsArc, IsVia, IsComponent), tests (InNet, InNetClass,
  InDifferentialPair, OnLayer, OnTop, OnBottom, InComponent,
  InComponentClass, HasNet), values (Net, Layer, Width, HoleSize,
  Diameter, Length, X, Y, Designator, Footprint, Value, Rotation, Side),
  `= <> < > <= >= Like`, And, Or, Not and brackets; lengths in mil, mm,
  um or in. Apply (Ctrl+Enter) selects the result and zooms to it;
  examples and recent queries are one click away.
- Find Similar Objects (Edit menu, Shift+F): tick what must match the
  selected part, track, arc or via (net, layer, width, hole, footprint,
  value, side...) and select everything like it.
- Selection memory: Ctrl+1..8 stores the selection, Alt+1..8 brings it
  back (also Edit > Selection memory).
- Core: `trace_core::query` (parse, run, attributes).

## 0.61.0 (2026-11-15)

- 3D via solver, stronger:
  - Results are kept passive: where the grid's small errors give more
    power out than in (|S11|² + |S21|² > 1), both are scaled back, with a
    note when it was more than half a percent.
  - Grid check (`trace via … --check`): solves again on a grid 0.7 times
    as fine and says how far |S11| and |S21| moved, and whether the
    result has converged.
- Via optimiser, smarter:
  - Repeated passes over all the settings while they still gain, and a
    parabola through the best value and its neighbours to try the value
    between them.
  - It also tries smaller pads (down to a 4 mil annular ring; `--keep-pad`
    or the "pad" box to keep them) and, when asked, backdrilling the stub
    (`--try-backdrill`, or the "backdrill" box).
  - It scores the worst return loss plus a tenth of the mean, and counts
    anything below −45 dB (under the solver's accuracy) as equal, so it
    stops chasing noise and keeps the simpler setup.
## 0.60.0 (2026-11-15)

Trace Studio: the workspace now works the way Altium Designer users know,
in Trace's own navy and teal.

- Menus in Altium's order: File, Edit, View, Project, Place, Design,
  Tools, Route, Reports, Window, Help. Every earlier command is in one of
  them (the analysis windows under Tools > Signal and power integrity,
  outputs under File > Fabrication outputs, Assembly outputs, Export).
- Docked panels on both sides with a tab strip along the bottom: Projects
  (the project tree: board, sheets, libraries), Navigator (the old side
  panel), PCB (nets, parts, classes and pairs with a filter; click to
  select and zoom), Layers, Properties and Messages. Each panel's title
  bar moves it to the other side, collapses the dock to a strip of
  upright tabs, or closes it; the Panels button (status bar, View and
  Window menus) brings any panel back. The layout is remembered.
- Properties panel: the board's summary; the selected part's X, Y,
  rotation and side, track width, via diameter and hole (typed in mil or
  mm, Enter applies, Ctrl+Z undoes); one width for many selected tracks;
  the current tool's options (routing, tuning, text, dimension, shapes).
- Document tabs over the canvas (Home Page, the board, the schematic, the
  footprint library) and a Home page with Open, recent documents and the
  keys.
- Altium keys: P opens the Place key menu (P T track, P I pair, P S text,
  P D dimension, P G pour, P C component, P F fanout), U the Route key
  menu (U A autoroute, U F fanout, U R tuning, U U unroute), Ctrl+W routes,
  Ctrl+M measures, Q switches mil and mm. Help > Keyboard shortcuts lists
  them all.
- Status bar: X and Y in the chosen units, the units button, the current
  layer; View > Theme switches dark and light.

## 0.58.0 (2026-11-15)

- PDN, stronger solver:
  - The chip's own capacitance inside its package (on-package and
    on-die), behind the package inductance, so the curves are what the
    die sees (`--die-c 20n --pkg-l 50p`, or "Chip capacitance" in the
    window). Without it the curves stay at the balls.
  - Ferrite beads from their rating: Murata BLM codes (BLM18PG221 = 220 Ω
    at 100 MHz), "600R@100MHz" or a plain ohm value; a bead is an
    inductance with its loss across it, so it is low at low frequency and
    resistive near its rating. Filter inductors take their µH value. (A
    bead was a fixed 1 µH, which made sub-rails look too high at 100 kHz.)
  - 120 points by default (60 placed sharp resonances poorly).
- PDN, smarter optimiser:
  - It can add capacitors at empty sites around each chip (`--add N`, or
    "new sites per chip"), and adds one only when it clearly helps.
  - It changes a capacitor's value only for a real gain, so it suggests
    fewer changes for the same result.
- PDN charts: `trace pdn … out.svg` and "Save chart…" in the window
  write the curves (as built, and as optimised) with the target.

## 0.57.2 (2026-11-14)

- PDN: a sub-rail whose bead sits off its plane (on a short track) is
  still fed by that bead, joined at the nearest point of the plane; it
  was taken as feeding the rail before it, which left the sub-rail with
  no supply (ADRV9026 board: VANA1_1P3, VRFVCO1_1P3).
- PDN: connectors are not taken as loads on a rail that feeds sub-rails;
  the sub-rail list says how many more there are past ten.

## 0.57.1 (2026-11-14)

- PDN: ferrite beads, filter inductors and 0 Ω links to other rails are
  no longer taken as loads. A rail that only feeds sub-rails is looked at
  from those feeds, with a note; its sub-rails are listed (in the window,
  as buttons that switch to the sub-rail and analyse it). A sub-rail with
  chips on it is fed by its bead, taken as 50 mΩ + 1 µH toward the rail
  before it. A regulator's own output inductor (to a switch node) is
  still the regulator.
- PDN: the ground is guessed from the capacitors only (beads and
  resistors to other rails no longer vote).

## 0.57.0 (2026-11-14)

- PDN impedance and decoupling (View > "PDN impedance and decaps…",
  `trace pdn`): the supply's impedance from 100 kHz to 1 GHz at each
  load, from the plane pair, every decoupling capacitor between the
  supply and ground (ESR and ESL by case size, mounting inductance from
  the via length) and the regulator. It is drawn against the target
  impedance (V × ripple / current step), with the frequencies over it.
  Capacitors can be ticked off or given another value to see what each
  does, and the optimiser picks values (or takes capacitors off) to bring
  the worst excess down. The board is not changed.
- Allegro: part values and device types are read (component value text
  and the device of each part), so BOMs and the PDN window have them.

## 0.56.1 (2026-11-14)

- Allegro: shape symbol pads (custom pad shapes) are read with their own
  outline, on every layer of a through-hole padstack, instead of a box
  from the pad's extent. On the Arduino UNO Q board this takes away 4
  false shorts at the crystal's pads; the ADRV9026 board's NF1846-1 pad 1
  is one of these.

## 0.56.0 (2026-11-14)

- Hierarchical schematics: sheet symbols are followed, and the sheets'
  nets join as the project's net identifier scope says (Altium's
  HierarchyMode, which OrCAD Capture's hierarchy matches): flat (ports
  join everywhere, net labels stay on their sheet), hierarchical (ports
  join the sheet entries of their sheet symbol), strict hierarchical
  (power ports stay on their sheet too), global (net labels and ports
  join everywhere) or automatic. ERC, Update PCB and the new `trace
  hierarchy` follow it; LimeSDR-USB (global) gives the board's 627 nets.
- Multi-channel sheets: a sheet placed by several sheet symbols, or by a
  Repeat(CH,1,4) one, gets one copy per channel, with its own nets and its
  parts named R1_CH1, R1_CH2 and so on.
- ERC: sheet symbols naming a sheet the project lacks (error), sheet
  entries with no port on their sheet, ports with no sheet entry above.
- Schematic view: the sheet list is the sheet tree, ⬆ goes up to the
  sheet that places the current one, double-clicking a sheet entry goes to
  its port below and a port to its sheet entry above, and "Nets join"
  changes the scope (saved in the Trace schematic).
- Two sheet entries of the same name on one sheet no longer join unless
  wired together.

## 0.55.0 (2026-11-14)

- Autorouter (Edit > Autoroute…, `trace autoroute`): routes the open
  connections of every net (or the selected one; poured nets are left to
  their pours) with tracks and through vias, around everything already on
  the board. Each net's pieces are joined along their shortest links,
  shortest first; each link is an A* search over the chosen layers (0°,
  45° and 90° steps, a via where one fits on every layer, at a set cost)
  pulled tight and checked segment by segment. Track width and clearance
  follow each net's Width and Clearance rules (a net class asking more
  room keeps it from everything routed near it); vias are the board's most
  used size. Plane layers are left out by default. Links that fail are
  tried again with a wider search and dearer vias. Runs in the background
  in the app; one undo step. On the Sinara Clocker with all 109 signal
  nets taken up it routes 180 of 188 links in 8 s with no clearance or
  width errors.
- `trace autoroute --unroute --nets ...` takes the nets' routing up first.

## 0.54.0 (2026-11-14)

- 3D clearance (Edit > 3D clearance…, `trace clear3d`): part bodies that
  run into each other or come closer than a set room, parts taller or
  lower than the board's Height rules (Altium MAXHEIGHT / MINHEIGHT, by
  side or part) or a limit per side, and bodies hanging over the board
  edge. Bodies are the parts' 3D models, compared by their outlines and
  then by height maps, so a part under a shield can's lid or a
  connector's overhang is not a collision; parts without a model get a
  box sized from their pads. Click a finding to select and show it.
- Pair-aware length tuning (`trace match` with a pair's name): both legs
  of a differential pair meander together, one meander offset to each
  leg, so the pair keeps its gap and its skew while its length grows to
  the target. Pairs in a match group are tuned first, and their mean
  length counts towards the group.
- 3D models: models in centimetres (and other metre prefixes) now come
  in at their true size; STEP assemblies whose writer lists the parent
  first now place their parts; a model with a broken plane, or a spline
  edge read past its own span, no longer makes reading use all memory;
  and bottom-side parts turned by a quarter turn now face the right way.
  On the LimeSDR-USB every model now matches OpenCascade's size.
- Net ties found on their own are now only parts with pads really laid
  one on another; 0.53 also took parts whose neighbouring pads touch.

## 0.53.0 (2026-11-14)

- Design reuse (Edit > Copy block layout…, `trace reuse`): a block laid
  out once (a channel, a supply) is copied onto another instance of it.
  Take the block's parts, pick the copy's counterpart of its anchor part,
  and Trace matches the rest through their nets (falling back to the
  designator offset, R12 -> R22, for parts joined only by ground or a
  supply), places them the same way about the anchor and copies the
  block's tracks, arcs and vias with the copy's nets. Shared nets copy
  only the stubs from the block's pads and their vias. The copy's own
  routing in its area is taken up first (unless kept). One undo step.
- Net ties (Edit > Net tie: mark selected parts, `trace nettie`): a
  part's own copper may join its nets (a star ground, a Kelvin joint)
  without the rule check calling it a short. Parts named NT…, footprints
  named NetTie, and parts with pads of different nets laid one on another
  (as Allegro draws them) are net ties without marking.

## 0.52.0 (2026-11-14)

- Test points (Edit > Test points…, `trace testpoints`): one probe point
  per net with two or more pads, from the bottom, the top or both sides,
  on a via, a through-hole pad, a test point part's pad or (when allowed)
  a surface pad, big enough to hit, clear of part bodies, the board edge
  and the other probes (1.27 mm by default). Nets with the fewest choices
  go first. Shows the coverage and each net left without one and why;
  click a net to find it. Marked points are kept in the board, written to
  a `-Testpoints.csv` with the fab outputs, and open the mask on probed
  vias. Undo takes them back.
- Assembly variants (Edit > Assembly variants…, `trace variants`, and
  `--variant <name>` on any command): parts not fitted and parts with
  changed parameters (an alternate value or part number) in each build. A
  PcbDoc brings the variants of the Altium project (.PrjPcb) beside it.
  For the chosen variant the BOM and pick and place leave out the parts
  not fitted and use the changed values; the assembly drawing crosses
  them out and leaves them off the parts list; IPC-2581 marks them not
  populated; STEP and the 3D view leave them off. Variants follow
  designators renamed by annotation or Update PCB.

## 0.51.0 (2026-11-14)

- STEP export (File > Export STEP (3D)…, `trace step`, or `trace convert`
  to .step): an AP214 model in millimetres with the board as a solid
  (outline, cut-outs, round and slotted holes; vias with `--vias`) and
  each part as a coloured box sized from its pads, named by designator in
  one assembly. Holes crossing the edge, a cut-out or another hole are
  left out and counted. All solids check valid in OpenCascade on the
  sample boards.
- Fabrication drawing (File > Export fabrication drawing (PDF)…,
  `trace fabdrawing`): the outline with each hole marked by its size's
  symbol, overall dimensions and a drill table, then the stack-up and
  fabrication notes (A3 or A4).
- IPC-2581 revision C export (File > Export IPC-2581…, `trace convert`
  to .xml): stack-up with dielectrics, every layer's features with nets,
  drills, parts, logical nets and the BOM.
- DXF export (File > Export DXF…, `trace convert` to .dxf): a DXF layer
  per board layer (copper, silk, mask, paste, mechanical, plated and
  unplated drills), in millimetres; tracks keep their width and arcs
  their curve.

## 0.50.0 (2026-11-14)

- Annotation (schematic panel > Annotate…, `trace annotate`): numbers
  parts without a number (R?, U?A), or every part again, across then
  down or down then across, from a start number, optionally a block per
  sheet (R101… on the second sheet). Gates of one part share its number
  even across sheets, and new gates fill a part of the same kind on the
  same sheet before another is started. Preview first; one undo step.
  Renamed parts reach the board through Update PCB.
- Electrical rule check (schematic panel > Check (ERC), `trace erc`):
  parts without a number or with one used twice, input pins left open
  (errors), other open pins, nets with only one pin or none, outputs
  driving each other or a supply pin, inputs with nothing driving them,
  nets with two names, net labels on nothing and loose wire ends. No-ERC
  markers and pins named NC are left out. The results list jumps to each
  issue and rings it on the sheet.
- Pin and gate swap (Edit > Pin and gate swap…, `trace swap`): two pins
  of a part trade nets, taking along the tracks and vias that lead only
  to each pad; routing that reaches other pads stays and is reported for
  routing again. Two gates of a part (U1A and U1B, from the open
  schematic) trade every pin, matched by where the pins sit on the
  symbol. The open schematic changes to match (pin numbers, and the
  gates' letters), so Update PCB does not swap them back.
- Pins keep their electrical type (input, output, power, ...), parts
  their gate letter, and sheets their No-ERC markers and buses, read from
  Altium and written back to Altium sheets.
- Schematic wires and pins less than 1 mil apart now count as joined
  (sheets drawn in millimetres land a fraction of a mil off): two pins on
  the LimeSDR-USB sheets were unconnected before.
- The schematic's undo takes back a change to several sheets in one step.

## 0.49.0 (2026-11-14)

- BGA fanout (Edit > Fanout…, `trace fanout`): for the selected parts, or
  a part picked from those with 16 or more surface pads, each ball pad
  gets a short neck track to a via in the middle of its grid cell, away
  from the part's centre (or towards it), or a via in the pad. The grid
  pitch is found from the pads, so turned parts and any pitch work. Outer
  rows can be left for routing on the surface, pads already routed are
  skipped, and a via is placed only where it and its neck keep the
  clearance to tracks, pads, vias and other nets' copper. Planes of other
  nets between the via's layers get a clearance hole cut (holes that meet
  are joined), pours are poured again. One undo step.
- Design rule check: two footprints placed on one spot as alternatives
  (an assembly option, such as two balun packages) are no longer reported
  as shorts between their pads. Parts count as alternatives when two or
  more of their same-net pads overlap.

## 0.48.0 (2026-11-14)

- Shapes: a Shape tool (polygon icon; Edit > "Draw and edit shapes")
  draws pours, copper regions, keepouts, pour cutouts and a new board
  outline on the current layer, corner by corner (45° optional) or as a
  rectangle; a pour or region takes the net typed in the bar, or the
  selected net. Edit mode picks a shape by its edge or from inside; drag a
  corner to move it, the middle of an edge to add one, Ctrl+click a corner
  to take it away, drag inside to move the whole shape, Delete removes
  it. Every change is one undo step and pours are poured again.
- Thermal relief styles per pour: the Shape window sets a pour's net,
  dead-copper removal, pour order and its connection to its own net's
  through-hole pads, surface pads and vias, each Direct, Relief (air gap,
  spoke width, 2 or 4 spokes, straight or at 45°) or None, instead of the
  board's polygon connect rules. "Pour again" refills it. Kept in Trace
  boards (`.tracepcb`); Altium and KiCad exports carry the poured copper.
- Pours stop at the board outline even when drawn past it.

## 0.47.0 (2026-11-14)

- Select many things at once: Shift+drag a box (left to right takes what
  is wholly inside it, right to left what it touches), Ctrl+click to add
  or take away, Ctrl+A for everything. A selection of parts, tracks, arcs
  and vias drags as one, nudges with the arrow keys (5 mil, 50 with
  Shift), turns 90° with Ctrl+R and deletes with Delete.
- Edit > Align: left, right, top, bottom, centres on a line, and spread
  evenly across or up, for the selected parts and vias.
- Copy, cut and paste (Ctrl+C, Ctrl+X, Ctrl+V), pasting at the cursor.
  Pasted parts are given the next free designators and no nets; tracks
  and vias pasted on their own keep their nets.
- Text: the A tool places stroke text on top or bottom silk (mirrored),
  the current copper layer or a mechanical layer, at a chosen height,
  stroke and angle.
- Dimensions: the dimension tool measures between two points (snapping to
  pads, vias and track ends) and draws extension lines, arrows and the
  length in mil or mm, on a mechanical layer by default. Mechanical layers
  now have their own tab, shown once something is drawn there.
- On Windows, Shift+Delete (which Windows treats as Cut) now unroutes the
  selected net as the menu says.

## 0.46.0 (2026-11-14)

- Teardrops: Edit > "Teardrops and gloss…" (or `trace teardrops <board>
  <out>`) widens each track where it meets a via or through-hole pad
  (surface pads if asked), on every net or the selected one. Each is a
  curved taper from 0.9 of the pad's width down to the track over one pad
  width; one that would come nearer other copper than the clearance is
  made shorter and narrower, or left out. Pours are poured again around
  them; fixed copper areas and other teardrops are kept clear of. Adding
  again replaces them; "Remove teardrops" (`--remove`) takes them off.
- Gloss, in the same window (or `trace gloss <board> <out>`): removes
  zero-length and duplicate tracks, merges tracks that carry on in a
  straight line with nothing else at the joint, cuts back dangling tracks
  (ends touching nothing of their net), and with "Chamfer square corners"
  (`--corners`) puts a 45° cut on 90° corners where it keeps the
  clearance. Joints with a pad, via or third track are left alone.
- The return-path check no longer takes teardrops for plane copper.

## 0.45.1 (2026-11-14)

- Allegro boards: through-hole pads and vias keep their copper layer by
  layer, as the padstack gives it. A square pin-1 pad with round inner
  pads was drawn square on every layer, so its corners poked out of the
  round clearance holes in the planes around it, and DRC reported shorts
  that are not on the board (112 of 136 on ADI's ADRV9026 EVB).
- Via fences: spots already served by the ground net's own vias are
  reported as such ("already covered") instead of as spacing, and the
  pitch uses each fenced track's effective Er (a microstrip's field is
  partly in air), so it is no denser than it needs to be.

## 0.45.0 (2026-11-14)

- Via stitching: Edit > "Via stitching and fences…" (or `trace stitch
  <board> <net> <out>`) fills a net's pours and planes with vias on a grid
  (staggered if asked), each clear of other copper on every layer it
  passes, off the board edge, keepouts and cutouts, and landing on the
  net's copper on at least two layers. Stitching again adds nothing.
- Via fences: the same window fences the selected net (and its pair
  partner), or `trace fence <board> <nets> <out>`: ground vias on both
  sides of its tracks and arcs, a twentieth of a wavelength apart at the
  frequency given (or a pitch of your own), at a gap from the track, in
  one or more rows.
- Other nets' planes that a new via passes get a clearance hole; pours
  are poured again. Ctrl+Z undoes the lot.

## 0.44.1 (2026-11-14)

- 3D via solver: with more than two return vias around a pair, the extra
  ones landed on the traces leaving the vias and shorted them. Return vias
  now go straight out from each via first, then to each side at the
  shallowest angle that clears the widest trace; a count that does not fit
  at the distance given is refused, as is any return via overlapping the
  antipad or another return via.
- Via optimiser: a pair keeps its pitch unless asked (`--free-pitch`, or
  "pitch too" in the window), since the part's pins usually set it. Every
  setup keeps 4 mil of copper clearance (pad to plane, pad to pad, return
  pad to antipad), or the starting setup's own where that is tighter, and
  setups that cannot be built are listed as such and
  never chosen.
- The pair note gives each line's own impedance (S-parameters were and
  stay referenced to 100 Ω differential).

## 0.44.0 (2026-11-14)

- Allegro boards bring their stack-up: the cross-section Allegro 17.x and
  newer keep (each layer's material, thickness, Er and loss tangent), so
  impedance, the field solver, the 3D via solver and the optimisers work
  on .brd files. Allegro 16.x files whose cross-section was never filled
  in still come with copper names only.
- Stack-up files: `trace stackup <board> [out.txt]` writes a board's
  stack-up as text (name, copper or dielectric, thickness, Er, Df), and
  any command takes `--stackup file` to use one instead of the board's;
  in the app, Impedance > "Load stack-up…" and "Save stack-up…".
- `trace layers` on a board prints its stack-up.

## 0.43.0 (2026-11-14)

- Via optimiser: View > "Via S-parameters (3D)…" > Optimise (or `trace
  via … --optimise GHz`) sweeps the antipad, one oval antipad around a
  pair or a round one each, the pair's pitch, and the return vias' count
  and distance, for the best worst-case return loss up to the frequency
  given, then solves the winner in full. Every setup tried is listed, and
  "Use this setup" loads the best one.
- Oval antipads for via pairs (`--oval`).
- Pair cross-sections: View > "Pair cross-sections…" (or `trace
  pairsize`) lists every track width from the board's minimum, with the
  gap that gives the target Zdiff on a layer (2D field solver), its
  differential loss (copper and dielectric), delay, crosstalk to the next
  pair, the space it takes and how far Zdiff moves with 0.5 mil of etch,
  and picks the lowest loss under a crosstalk limit.
- Touchstone files: `trace cascade` reads .s1p and .s2p files and gives
  the return loss into a chain of vias and lines ending in a load, such
  as a chip's port model.
- Accuracy checks: `trace validate` runs every solver against exact
  answers (Cohn's stripline and coupled stripline, ρL/wt), textbook
  formulas (Hammerstad and Jensen, conformal CPW), the 2D solver and
  finer grids, and prints each error.
- Fixed: the 3D via solver drew thin metal edges on grid lines, which act
  as if they reach a third of a cell further, so traces came out about
  9% low in impedance and vias too capacitive; metal is now drawn a third
  of a cell inside its true edges (trace impedance within 1% of the 2D
  solver), and cells default to a quarter of the drill (a stub's notch
  within about 2% of where finer grids converge, from about 7%).
- Stub notches are found between frequency points.

## 0.42.0 (2026-11-14)

- 3D full-wave solver for vias: the finite-difference time-domain
  (FDTD) method on a graded Yee grid. A via (or a differential pair of
  vias) carries a trace from one copper layer to another through the
  board's own stack-up: planes with round antipads, pads, the barrel
  (with its stub, or backdrilled), and return vias joining the planes,
  closed all round by convolutional PML so the planes act as an endless
  board. Each trace ends in a resistor; the first is driven by a pulse,
  voltage and current are sampled on both traces, each trace's own
  impedance and propagation are measured from two probes, and the
  reference planes are moved to the via's centre.
- It gives return and insertion loss (S11 and S21, or Sdd11 and Sdd21 for
  a pair) up to the frequency asked (40 GHz by default), the stub's
  resonance when the barrel runs on past the outgoing layer, and a
  Touchstone file (.s2p) for other tools.
- Checked: a plain trace through the same ports and probes comes out
  with |S11| below −45 dB and |S21| within 0.1 dB (stripline: −60 dB and
  0.01 dB to 40 GHz); a via is passive (|S11|² + |S21|² between 0.9 and
  1.05) and nearly transparent at 1 GHz; a 60 mil stub notches |S21|
  below its bare quarter-wave frequency, as the planes it passes load it,
  and backdrilling removes the notch. On the DAC39RF12 EVM, a top-to-L7
  via leaves a 51.6 mil stub that notches S21 to −20.8 dB at 18 GHz.
- View > "Via S-parameters (3D)…": layers, drill, pad and antipad (taken
  from the board's commonest via and its plane clearances), return vias,
  pair pitch, backdrill, top frequency; solves in the background with a
  plot, "Save Touchstone…" and "Save plot (SVG)…".
- `trace via board layer layer [--drill D] [--pad P] [--antipad A]
  [--width W] [--width2 W] [--pair PITCH] [--returns N@D] [--backdrill]
  [--fmax 40] [--cell C] [out.s2p] [out.svg]`.

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
