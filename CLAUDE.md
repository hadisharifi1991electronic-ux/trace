# Trace: notes for Claude Code

Trace is Hadi's open-source (GPL-3.0-or-later) schematic and PCB design tool:
a Rust workspace (egui app `trace-app`, CLI `trace`), high-speed and RF
first, desktop first. Planning notes from earlier work are in PR #1
(`docs/project-archive/`, branch `claude/project-thread-yhddf3`).

## How Hadi works

- Act without asking for reversible steps (build, test, commit, push,
  install). Hadi often writes in Persian (Farsi): answer in the language of
  the message.
- After a release is installed, start the next queued item without waiting.
- Keep features lean. Priorities: 1) PCB and schematic tools, 2) accurate
  EM and thermal simulation (HFSS-like). No antenna far-field plots for now.
- Tuning bends are always mitered or arcs, with user-set angle and size.
- Hadi's design files: only read what Hadi points to, never modify them.
  `C:\Users\hshar\Desktop\SDR\Altium` (SDR_Card_Final) is read-only and
  needs Hadi's OK. Test new versions on
  `C:\Users\hshar\Downloads\sbac313\Fab Data\DAC39RF12 EVM RevC_ASCII.PcbDoc`
  (TI, 16 layers, read-only), not the ADRV9026 board.
- Hadi's PC: Ryzen AI 7 350 (8C/16T), about 15 GB RAM (often only ~3 GB
  free), Radeon 860M iGPU (no CUDA). openEMS v0.37.0 at `C:\openEMS`.

## Building and releasing

- Commit as `Hadi <hadisharifi1991electronic-ux@users.noreply.github.com>`,
  never a university email. End messages with the Claude co-author line.
- CI (`.github/workflows/ci.yml`, Rust 1.97.0 pinned): `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` on Linux and
  Windows. A full run takes 50-60 min; the full trace-core test run is
  long, so filter tests locally (`cargo test -p trace-export fab`).
- Don't run `cargo test` and a FEM run at the same time (out of memory).
- Release: bump `version` in the root `Cargo.toml` (and the workspace
  crates in `Cargo.lock`), add a `## X.Y.Z (date)` entry at the top of
  `CHANGELOG.md`, push to `master`. `release.yml` then builds the Windows
  exes onto the `dist` branch (`trace-app-<version>.exe`, `trace.exe`,
  `SHA256SUMS`).
- Install on Hadi's PC: copy `trace-app-<version>.exe` and `trace.exe` from
  `dist` into the Trace folder under `...\Altium\Trace`, check the hashes,
  point `Desktop\Trace.lnk` (TargetPath and IconLocation) at the new exe,
  move the old `trace-app-*.exe` to the Recycle Bin.
- Screenshots on Linux: Xvfb plus `TRACE_OPEN` (`at:x,y,scale`,
  `schat:...`, `tune:...`), `LIBGL_ALWAYS_SOFTWARE=1`.

## State (2026-11-28)

- `master` is **0.111.0** (Fabrication Outputs window, read-back check),
  released and installed on Hadi's PC (2026-11-28).
- Branch `claude/x2as-hfss-0.112` (not released): `trace_core::hfss` (any
  FEM model as an HFSS IronPython script; `trace fem ... hfss=out.py`,
  `trace hfss`, `trace cutout --hfss`), `trace_core::crossover` (the
  X2AS-class model, `trace fem crossover key=value`, `ref=X2AS.S2P`), and
  a CAD look for `--render` (`--style cad`, the CLI default: white, floor
  grid, green laminate, gold copper, mm scale; `--style dark` the old one).
  Results in `...\Altium\Trace\results-0.112-x2as`.
- Crossover so far (250k unknowns, 2 passes, not converged): the baseline
  has about 2 nH too much series inductance at the RF castellations (RL
  4-10 dB). `side_gnd=1.3` (ground-signal-ground launch) gives RL 15-20 dB
  to 4 GHz but a resonance near 5 GHz (IL up to 8 dB): probably a mode
  between the part's two ground planes; stitch them (vias along the RF
  path) next. Hadi's PC has about 3.8 GB free after a restart, which caps
  solves near 250k unknowns.

## Next: an X2AS-style RF/DC crossover, simulated in Trace, with HFSS export

Hadi (2026-11-28): design a part similar to the TTM/Anaren Xinger **X2AS**
SMT crossover, simulate it in Trace, and add an export that HFSS can
analyse.

- X2AS: an RF line and a DC line cross in one small SMT package, without a
  multilayer board. DC-6 GHz, 50 ohm. Return loss at least 20 dB (DC-2.5 GHz),
  17 dB (2.5-3.5 GHz) and 15 dB (3.5-6 GHz). Insertion loss at most 0.05,
  0.10 and 0.15 dB over the same bands. Hadi has the datasheet
  (`X2AS.pdf`, with the mechanical outline), the vendor's DXF land pattern
  (`X2AS.dxf`, layers PCB_Top, Component_Pads, Component_Outline,
  PCB_Solder_Mask, PCB_Plated_Holes, PCB_Land_Pattern, PCB_Solder_Stencil;
  inch units) and the measured RF path S-parameters (`X2AS.S2P`,
  100-6000 MHz in 5 MHz steps, RI, 50 ohm). Ask Hadi to put them in a
  folder you can read; they are third-party files, so don't commit them to
  this public repo.
- Plan: model the crossover's own stack (RF path as a stripline or CPW
  through the body with vias to the pads, DC path crossing on another
  layer) with its footprint on a 50 ohm microstrip test board. Solve it
  with the existing 3D FEM (`trace-core` `fem3d`, `trace cutout`,
  `trace fem`), compare with `X2AS.S2P` (return loss, insertion loss, phase),
  and tune the dimensions to meet the spec.
- HFSS export: write the 3D model (stack-up materials, copper, vias, ports)
  in a form HFSS can import and solve: a STEP or SAT geometry with a
  material and port map, or better an HFSS-scripting (IronPython) `.py`
  that builds the model, assigns materials, boundaries, wave or lumped
  ports and a sweep. Offer it from the 3D model window and
  `trace hfss <board> <window|via:NET> <out.py>`.
