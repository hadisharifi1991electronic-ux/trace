---
name: pcb-tool-project
description: Hadi's EDA tool "Trace" (KiCad-like, Allegro-class): decisions, where code/releases live, what each version added
metadata:
  type: project
  modified: 2026-11-13T20:40:00.000Z
---

Hadi wants EDA software "similar to KiCad with better options, like Allegro", started 2026-11-04. Open source (GPL-3.0-or-later), personal, high-speed first, desktop first. Design doc: https://claude.ai/artifact/HcAHhJCop3Py7qGFneKfZF

**Why:** these choices set scope and stack for all later work.
**How to apply:** check the doc and this history before new build work.

Hadi's Altium projects: C:\Users\hshar\Desktop\SDR\Altium. Main board SDR_Card_Final (ADRV9026 EVB + ZCU102, ~85 SchDocs, 62 MB PcbDoc). Hadi is touchy about files being read: only read what they point you to; never modify design files.

Code: Rust workspace "Trace" (egui app trace-app + CLI trace), on GitHub since 2026-11-13: github.com/hadisharifi1991electronic-ux/trace (public, master; history rewritten then, old hashes below are stale; release workflow builds the exe when Cargo.toml version changes). Commit as Hadi with hadisharifi1991electronic-ux@users.noreply.github.com, never the university email. Full history bundle /mnt/project-files/trace-release/trace.bundle (git clone it); /mnt/project-files/trace/ is a STALE early copy. Windows build cross-compiled (x86_64-pc-windows-gnu), staged in /mnt/project-files/trace-release/, installed at ...\Altium\Trace as trace-app-X.Y.Z.exe + trace.exe; shortcut Desktop\Trace.lnk repointed each release via Remote Control, old exe recycled. Samples: /mnt/project-files/trace-release/samples.

History (11-04/05): viewers, imports/exports, 3D, CM 0.5, sch 0.6, impedance 0.7, route 0.8, pairs 0.9, tuning 0.10, toolbars 0.11.
- 0.10.1 reads ASCII PcbDoc. Test board: C:\Users\hshar\Downloads\sbac313\Fab Data\DAC39RF12 EVM RevC_ASCII.PcbDoc (TI, 16 layers).
- 0.12 re-pour; 0.12.1 nested fab zips (TI: ...\sbac313\Fab Data\{Gerbers,ODB++}.zip); 0.13 live DRC (DAC 46 real clearances, stacked alt parts); 0.14 walk/push; 0.15 drag; 0.16 match groups; 0.17 fab zip; 0.18 fab DRC.
- 0.19.0 (e8ac387, 2026-11-12): schematic editing (sch_edit.rs) + Update PCB (eco.rs, Edit::Netlist), .tracesch save next to PrjPcb. Test project: Sinara Clocker (public).
- 0.20-0.31: footprint editor/PcbLib, blind/buried vias, silk/part DRC, SchLib/IntLib, PcbLib + SchDoc + PcbDoc write, ODB++ export, Altium pours poured on load, rooms + per-object rules.
- 0.32.0 (8d29df7): return path + crosstalk DRC (drc_si.rs). 0.33.0 (29bec3c): via stubs + backdrilling (backdrill.rs, Altium BackDrilling rule, BACKDRILL Excellon/ODB++).
- 0.34.0 (eb6b527, 2026-11-13): panelization (core panel.rs panelize(); Board.panel PanelOf maps copy suffix _2.. back to base names for DRC; Geometry.slots = milled gaps, in fab/ODB++/KiCad outlines and DRC board edge; app File > Panelize, CLI `trace panel b 2x2 out`). PcbDoc save drops slots.
- 0.35.0 (7c3efc8): bus routing (core bus.rs: order/side/lane, match_lengths; Obstacles nets now &[Option<u32>]; app Ctrl+click gathers, Router.lanes, Match bus button; CLI `trace match`).
- 0.36.0 (f1090d4): IBIS simulation (core ibis.rs parser; sim.rs Bergeron lossless lines from copper, DC init, PRBS7 eye; app simwin.rs View > Signal simulation; CLI `trace sim b U1.A U2.B --ibis f`). DAC 6SRX pair 2G: eye 3.27 V/94%.
- 0.37.0 (7bfaa45): sim loss (sim/loss.rs: Atten dc/skin/diel, min-phase FIR + 1-pole tail) + crosstalk (sim/xtalk.rs: Garg-Bahl/Cohn modes, weak-coupling NEXT/FEXT injected as waves; aggressors = 8 strongest parallel nets); full-circuit DC init. DAC LMK_SDI 200M: 509 mV xtalk.
- 0.38.0 (cccaca6): StackLayer.loss_tangent (Altium V9_STACK_LAYERn_DIELLOSSTANGENT, ODB++ .loss_tangent) + roughness_um (not in Altium; Impedance window edits); LayerGeometry tan_d/rough_um; Loss.stackup_df/stackup_rough. DAC 6SRX in Itera MT40 Df 0.003.
