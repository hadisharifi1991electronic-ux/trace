# Changes

Each release is installed as `trace-app-<version>.exe` next to the earlier ones.

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
