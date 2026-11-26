# Claude project threads (as of 2026-11-26)

One thread per feature. Per-feature details, numbers and decisions are in the
repo's `CHANGELOG.md` and in `../memory/` (MEMORY.md is the index).

| Thread | Started | Last activity | State |
| --- | --- | --- | --- |
| Back up project data to GitHub | 2026-11-26 | 2026-11-26 | this backup |
| Fix FEM power loss on DAC board | 2026-11-26 | 2026-11-26 | working: zero-ohm links as copper, wave/lumped port power fixes, power sums 1.000; final DAC sweep 1-6 GHz then 0.110.0 |
| Make FEM faster and more reliable | 2026-11-26 | 2026-11-26 | review (0.109.0 / 0.109.1 shipped) |
| Couple thermal and IR-drop simulation | 2026-11-25 | 2026-11-26 | done (0.107.x) |
| Add length and phase tuning | 2026-11-25 | 2026-11-26 | waiting on Hadi (0.106.0, 0.108.0 shipped) |
| Lower memory for board cut-outs | 2026-11-25 | 2026-11-25 | review (0.105.0) |
| Make the 2D board view realistic | 2026-11-25 | 2026-11-25 | waiting on Hadi (0.102.0) |
| Polish the schematic view | 2026-11-25 | 2026-11-25 | review (0.104.0) |
| Make the 3D board look realistic | 2026-11-25 | 2026-11-25 | waiting on Hadi |
| Add field arrows and streamlines | 2026-11-24 | 2026-11-25 | done (0.100.0, 0.102.1) |
| Open 3D models from the board | 2026-11-25 | 2026-11-25 | done (0.101.0) |
| Make the whole-board 3D view realistic | 2026-11-24 | 2026-11-24 | waiting on Hadi |
| Make vias and 3D views look real | 2026-11-24 | 2026-11-24 | waiting on Hadi (0.98.0+) |
| Solve board cut-outs with many nets | 2026-11-24 | 2026-11-24 | done (0.97.0) |
| Build an HFSS-style 3D EM solver | 2026-11-18 | 2026-11-24 | waiting on Hadi (0.90-0.96) |
| Add a whole-board RF path report | 2026-11-18 | 2026-11-18 | done |
| Add a schematic symbol editor | 2026-11-18 | 2026-11-18 | done (0.89.0) |
| Change the Trace logo | 2026-11-18 | 2026-11-18 | done ("1 · Pair" chosen) |
| Improve PCB and schematic editing | 2026-11-18 | 2026-11-18 | done (0.85.0) |
| Add an L/C parasitics library | 2026-11-18 | 2026-11-18 | done |
| Show thermal density maps | 2026-11-17 | 2026-11-18 | done (0.81.0) |
| Show results on the board, add cancel | 2026-11-04 | 2026-11-18 | done |
| Show electromagnetic field density maps | 2026-11-17 | 2026-11-18 | done (0.80.0) |
| Size vias on the accurate grid | 2026-11-17 | 2026-11-17 | done (0.78.0) |
| Add an Altium-style design rules editor | 2026-11-18 | 2026-11-18 | done (0.88.0) |
| Strengthen push routing on the board | 2026-11-18 | 2026-11-18 | done (0.86.0) |
| Add schematic Drag, buses and harnesses | 2026-11-18 | 2026-11-18 | done (0.87.0) |
| Add balun 3-port models | 2026-11-18 | 2026-11-18 | stopped by Hadi |
| Add openEMS as a second solver | 2026-11-17 | 2026-11-18 | done |
| Set up Trace GitHub repo in Chrome | 2026-11-13 | 2026-11-13 | done |
| Write Altium PcbDoc files | 2026-11-12 | 2026-11-13 | done |
| Make Trace look and work like Altium | 2026-11-15 | 2026-11-15 | done |

Work queue (Hadi, 2026-11-26): 1) FEM power-loss fix on DAC, 2) Gerber/ODB++/drill/pick-place output, 3) live DRC halos.
