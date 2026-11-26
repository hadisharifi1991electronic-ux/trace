---
name: field-maps-emi-sim
description: 2026-11-17 thread "Show electromagnetic field density maps": scope Hadi chose (FDTD field maps + real EMI simulation)
metadata:
  type: project
---
Hadi 2026-11-17 15:47: "show the electromagnetic wave density, add to the simulation". Hadi then chose (via coordinator) to rebuild EMI as a real simulation in the same work: currents from real drivers, cable/connector common mode, plane cavity resonance, decoupling loops, 3 m spectrum vs limit, field map on board. Limit (FCC B vs CISPR 32) and real clock rates still unknown: per-signal-type defaults until Hadi answers.
Plan: 0.80 = FDTD field maps (|E|,|H|, energy, surface J on cut planes; SVG/GIF; app viewer). 0.81 = EMI simulation + board near-field overlay. See [[next-features-backlog]].

2026-11-17 20:10: 0.80.0 pushed to master (feb18e4) with BOTH (field maps: emfield.rs, fdtd FieldCapture, `trace via --fields`, app fieldview.rs; EMI sim: emi_sim.rs as emi::sim, `trace emi` runs it by default, --signal/--ibis/--cable/--rail/--map/--quick, EMI window Simulate). dist hash trace-app-0.80.0.exe c3a022c9…02f6. NOT installed yet: RC refuses without Hadi's words in the thread; asked Hadi for "go". After go: install + run on ADRV (emi, emi cispr-b, via 1 8 --fields 6) into results-0.80.0. Samples in /mnt/project-files/trace-release/reports/0.80.0/. Full cargo test > 30 min locally; CI runs it.
Hadi 2026-11-17 20:15: use CISPR 32 class B (now the default since 0.80.1, e331638). 0.80.0 was installed by the other thread ("Size vias…", which holds Hadi's go/RC); its ADRV/DAC runs found the LOGO2-as-SMA bug (fixed in 0.80.1). Real clock/RF levels still unknown; --signal class:lo=... etc.
0.80.2 (694c68b): each connector drives its own cable (lengths by kind, weight by net nearness). Thermal density maps went to a separate thread.
2026-11-18 00:17: 0.80.3 (I2C/SPI/reset lines not clocks, merged same-name lines) and 0.81.1 (cavity below first resonance = vertical dipole, k·εr·side/2; thermal thread took 0.81.0) installed. results-0.81.1: DAC passes +6.5 dB; ADRV fails -12.3 dB only from EXT_LO1 at assumed 0 dBm on outer layer (inner reroute +22.7 dB). Rail switching rate is assumed (fastest SE clock, else 100 MHz). Awaiting Hadi's real LO level.
