# Changes

Each release is installed as `trace-app-<version>.exe`; the previous version
is moved to the Recycle Bin.

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
