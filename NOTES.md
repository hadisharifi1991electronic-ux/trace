
Fabrication Outputs window (File > Fabrication outputs > Gerber, drill and
BOM…), in place of the one-click zip:

- Gerber X2, or X1 (RS-274X, no attributes) for older CAM. The menu said
  X1 before, but the files were always X2.
- Drill files in mm or inches, and plated and non-plated through holes in
  separate files or in one (X2 `MixedPlating`, each tool still marked
  plated or not). Inch files keep 5 decimals.
- Mask expansion, paste expansion and via tenting, starting from the
  board's rules.
- A list of every file with its kind and size; untick what the fab house
  does not want.
- Also in the zip: the ODB++ job, IPC-2581, the fabrication drawing and
  the assembly drawing.
- Check: the Gerber and drill files are read back the way a fab house's
  CAM reads them, then compared with the board: every file recognised,
  copper layers in the right order, holes by size, every net and part with
  copper named in the files, and the board outline. Differences are listed
  with the sizes or names that are missing.
- Drill tools are split by plating as well as size, and a tool is marked
  ViaDrill only when all its holes are vias (it was the first hole's kind).
- Gerber import takes an empty X1 layer (a format statement, nothing
  drawn, no attributes) as Gerber: it was dropped as "not Gerber", so the
  check of an X1 set with an empty silkscreen or paste layer failed.

`trace fab <board> <out> [--x1] [--drill-inch] [--drill-merge] [--check]`
does the same from the command line; `--check` prints the comparison and
exits with an error when anything differs.

