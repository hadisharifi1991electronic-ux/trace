---
name: openems-solver
description: 0.82.0 openEMS second solver for vias/pairs: how it works, CLI flags, Hadi's install, validation status
metadata:
  type: project
---
Hadi 2026-11-17 asked for free equivalents of Altium's bundled parts; picked openEMS first. Hadi's install: C:\openEMS (openEMS v0.37.0, CSXCAD 0.7.0, Python 3.13 bindings work).
0.82.0 (pushed to master 2026-11-18 ~00:45 UTC, 0a23f0f): core openems.rs writes trace_via.xml (CSXCAD XML, mil, same grid/PML as fdtd::model, default cell = half Trace's d0; planes = keyhole polygons, pads polygons, barrels cylinders, R lumped elements + soft E source, V/I ProbeBoxes at Trace's probe spots), runs openEMS.exe (cwd = run dir, progress from log energy dB, cancel kills it), reads probes, same fdtd::extract power-wave extraction. CLI `trace via ... --check openems [--openems-dir/--openems-exe/--openems-cell]`, `--export-openems DIR`, `--openems-read DIR`. Via window: Check with openEMS (dashed overlay) + Export.
Validated 2026-11-18 01:00 on Hadi's PC (openEMS v0.37, worked first time): ADRV RF pair via, 2.36 M cells, 0.98 mil, 552 s; |S11| within 0.51 dB, |S21| within 0.016 dB, Zvia 88.8 (Trace) vs 89.3 (openEMS); Sdd11 6 GHz -24.8 vs -25.3 (quick grid; older accurate run said -27.2). Results Trace\results-0.82.0. Both use Trace's geometry. Reference from results thread (2D line, Trace\results-openems\line): openEMS Zdiff 120.4 vs Trace 126.7 on the ADRV pair line. See [[field-maps-emi-sim]].
