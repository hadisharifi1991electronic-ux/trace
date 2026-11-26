---
name: next-features-backlog
description: Trace feature backlog after 0.71.0 (2026-11-15), offered to Hadi; pick from here after the weekly reset
metadata:
  type: project
  modified: 2026-11-15T18:19:06.398Z
---

Offered to Hadi 18:18 (recommended first: 1):
1. BOM with part data (price/stock/lifecycle/datasheet, obsolete/single-source flags)
2. 3D fold view for rigid-flex (bend clearance vs parts/enclosure)
3. Thermal map (part power through copper/vias, thermal via hints)
4. Bus simulation (DDR/parallel bus eye with crosstalk + timing per bit)
5. Board-level EMI check (plane-split crossings, edge radiators)

Gaps found testing ADRV0926 (RF accuracy):
- Balun vendor .s3p / loss+imbalance model (T301 still ideal 1:2)
- L/C parasitics library (SRF/Q, Murata/Coilcraft 0201/0402); --tune picks 39-47 nH near SRF
- Ferrite beads by impedance curve, not one 100 MHz point
- Via optimiser/zcheck converged grid by default; zcheck coplanar ground for outer RF necks
- --tune per band with tolerance, E24/E96, write values back as variant/BOM
- Fitted/DNI from Allegro/Altium BOM per variant (no manual --fit)
- DONE 2026-11-18: Whole-board RF report (`trace rf`, rfreport.rs; 0.83.0, ADRV fixes 0.88.1/0.89.1/0.90.1). ADRV results-0.89.1: 27 paths, RX/ORX 0.4-0.7 dB @6 GHz, RL into SEDZ ~-4.5 dB (TBD match), LO has 2 fitted baluns T409/T410 + 2 terminations R449/R452. 0.90.1 installed (results-0.90.1, 513 s): FPGA_REF_CLK fixed. Open: rfreport `short` = c/f/10/1.9 is ~103 mil at 6 GHz (meant ~200), so CLK R606 176 mil, OUT7 R625 138 mil terminations still count as loss; U.FL J602-J612 stop at a resistor; driver-output paths judged like inputs. Resume after weekly reset.
- PDN per-rail current, on-package C, finer-grid check
- ReturnPath DRC triage (190 hits on ADRV): group by net/split, suggest stitch caps/vias
- Allegro fab-drawing stack-up fallback built in

Hadi 17:55: pause until weekly reset, then continue. See [[pcb-tool-project]].
- Accurate EM mode: adaptive mesh refinement to convergence (dS<0.02) + larger region with nearby traces (Hadi asked 2026-11-17 why EM is fast vs HFSS).
- 2026-11-17 queue (Hadi "go" 09:19): 0.72 BOM (installed), 0.73 rigid-flex 3D fold + BOM Allegro fixes, 0.74 EM accuracy (Hadi 09:57: auto-converge + Richardson error bars, conformal round shapes, passivity/causality, benchmarks; de-embedding + dispersive loss later), 0.75 ACCURATE 3D thermal with heatsink (Hadi 10:00: "not a simple one but an accurate one": stack-up conduction from real copper, vias, 2R package θJC/θJB, finned heatsink+TIM, convection correlations + radiation, mesh check, JEDEC 2s2p/analytic fin validation), then bus sim, EMI.
- BUG (2026-11-19): trace via --fields with a GIF output path wrote the Touchstone file under the .gif name; check the -o/GIF option handling after reset. Pair field maps: Trace\results-0.91.1\fields.
- Advanced 3D solver roadmap (offered Hadi 2026-11-23, decision card pending): 1 low-memory iterative (AMS-preconditioned) FEM + symmetry planes, 2 fast sweep (MOR), 3 conductor/dielectric loss, 4 board cut-outs multi-net, 5 3D view. Extra ideas: GPU FDTD, planar MoM solver (pairs w/ Smith chart tuning), hybrid 3D-via/2D-line channel.
- Hadi's PC hardware (2026-11-23): Ryzen AI 7 350 (8C/16T), 15 GB RAM, AMD Radeon 860M iGPU only (no CUDA) -> GPU FDTD low value; prefer multithreaded CPU. Benchmarks: ADRV pair via FDTD 41 s, openEMS 552 s.
