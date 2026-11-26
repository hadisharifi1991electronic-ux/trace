# Next-gen PCB design tool: design doc

Nov 4, 2026 · @Hadi

## Summary

We build a schematic and PCB design tool (working name **Trace**) that is as open and affordable as KiCad but brings Allegro-class constraint-driven design to everyone. The bet: engineers outgrow free tools at the first DDR, PCIe or USB 3 board, and the jump to Cadence or Altium costs thousands per seat per year.

This doc proposes who it is for, what makes it different, how it is built, and a four-phase plan to a usable MVP in about 12 months. Decided with Hadi: open source like KiCad, built as a personal project, high-speed design first, desktop first.

## Pain points in today's tools

KiCad is excellent for simple and mid-complexity boards, but users hit the same walls once designs get fast, dense or shared. Allegro and Altium solve most of these, at a price and learning curve most individuals and startups cannot carry. The list below is from common community feedback and hands-on experience, not a formal survey.

| Area | KiCad today | Allegro / Altium | Gap we can fill |
| --- | --- | --- | --- |
| Constraints | Net classes and custom rules in a text DSL | Spreadsheet constraint manager, rule hierarchies, topology rules | Visual constraint manager that is free and scriptable |
| High-speed routing | Diff pairs and length tuning, no delay or phase matching by layer stack-up | Delay-based tuning, xnets through resistors, pin-pair rules | Time-domain tuning and xnet support |
| Signal and power integrity | None built in | Add-on tools (Sigrity, extra licences) | Built-in impedance, crosstalk and PDN checks |
| Collaboration | Files in Git, no live co-editing, hard visual diffs | Vault or cloud workspaces, paid tiers | Live co-editing, visual diff and review comments on the board |
| Libraries | Large but uneven, parts often hand-made | Managed libraries, supplier data | Verified parts with live stock, price and lifecycle |
| Autorouting | No built-in router (external Freerouting) | Interactive and auto routers | Modern interactive router plus a strong batch router |
| UX | Separate editors, many modal dialogs, steep shortcuts | Powerful but dense and dated | One unified, fast, keyboard-and-command-palette UI |
| Variants and outputs | Basic variants, manual fab packages | Mature variants, one-click releases | Variants, release workflow and one-click fab packages |

## Target users

The first users are hardware startups and small design teams doing 4 to 12 layer boards with at least one high-speed interface. They feel KiCad's limits most and cannot justify Allegro seats.

| Segment | Typical board | What they need most | Priority |
| --- | --- | --- | --- |
| Hardware startups (2 to 20 engineers) | 6 to 12 layers, DDR4, USB 3, PCIe | Constraints, collaboration, low cost | Primary |
| Consultancies and freelancers | Many client boards a year | Fast workflow, reusable blocks, clean outputs | Primary |
| Students and university labs | 2 to 4 layers, mixed signal | Free tier, learning aids, simulation | Secondary (growth channel) |
| Hobbyists and makers | 2 layers | Simple UI, good libraries, one-click ordering | Secondary |
| Enterprise teams | 16+ layers, rigid-flex | Compliance, PLM integration, audit trail | Later |

## Differentiating features

The core differentiator is **constraint-driven design that is free to start**: every rule lives in one place, and the router, checker and simulator all read it. Features are grouped into four pillars, with the MVP set marked.

**1. Constraint-driven high-speed design (MVP)**

- Constraint manager as a spreadsheet: net classes, diff pairs, xnets, pin-pair rules and region rules, with inheritance.
- Stack-up editor with a built-in field solver, so trace widths come from target impedance (for example 50 ohm single, 90 or 100 ohm differential).
- Length and delay tuning in picoseconds per layer, matched groups for DDR byte lanes, live tuning meters while routing.
- Real-time DRC that flags a violation as you draw, not after a batch run.

**2. Modern editing and collaboration**

- One unified project: schematic, layout and 3D in tabs of the same window, with cross-probing everywhere.
- Command palette, consistent shortcuts, and undo history that survives restarts.
- Live co-editing with per-region locking, visual diff between versions, and review comments pinned to board locations.
- Open, text-based file format that diffs cleanly in Git, plus full import of Altium Designer and KiCad projects: schematics (.SchDoc), boards (.PcbDoc) and libraries (.SchLib, .PcbLib, .IntLib).

**3. Smarter routing and placement**

- Interactive push-and-shove router that respects every constraint, plus fan-out and bus routing.
- Batch autorouter for non-critical nets, run as a cloud job.
- AI assist (later phase): placement suggestions from the schematic, design review that explains violations in plain language.

**4. Parts, simulation and manufacturing**

- Verified part library with symbols, footprints and 3D models, linked to supplier stock, price and lifecycle.
- Built-in SPICE (ngspice) from the schematic, then signal-integrity (IBIS) and power-delivery checks.
- Design variants, a release workflow, and one-click fab packages (Gerber X2, ODB++, IPC-2581, pick and place, BOM).
- Python and TypeScript scripting API plus a plugin store.

## Architecture and tech stack

One Rust engine holds the design model, constraints, router and checks, and compiles both to a native desktop app and to WebAssembly for the browser. That gives desktop speed, a zero-install web version, and the same results on every platform.

&#91;embedded content: architecture · 4 layers\]

The board data model is a CRDT, so offline edits and live co-editing merge without conflicts. Heavy work (batch routing, simulation) runs as cloud jobs, with a local fallback for offline use. Altium files are binary compound documents, so the Rust core gets a dedicated parser, using KiCad's open-source Altium importer and published format notes as behaviour references.

| Layer | Choice | Why |
| --- | --- | --- |
| Core engine | Rust | Speed and memory safety; compiles to native and WebAssembly |
| Rendering | WebGPU (wgpu on desktop) | Smooth zoom and pan on boards with 50,000+ objects |
| UI | TypeScript and React | Large talent pool; same UI on desktop and web |
| Desktop shell | Tauri | Small installers, native file access |
| Collaboration | CRDT (Automerge or Yrs) | Offline-first merging, live cursors |
| Geometry | Clipper2, OpenCASCADE | Proven polygon and STEP 3D libraries |
| Simulation | ngspice, IBIS models | Open, industry-standard engines |
| File format | Open, text-based (JSON or S-expression) | Clean Git diffs, easy scripting |
| Import and export | KiCad, Altium, Eagle in; Gerber X2, ODB++, IPC-2581 out | Low switching cost, any fab |

## Phased MVP plan

The MVP is a desktop tool that can take a DDR4-class board from schematic to fab files with every high-speed rule enforced, by month 9. Each phase ends at a gate tested on a real board, not a demo: Hadi's own Altium projects are the first test set, so the phase 1 gate is opening them without losing anything.

&#91;embedded content: roadmap · 4 phases, 4 gates\]

The month labels assume a small full-time team. As a solo, part-time project, expect each phase to take roughly two to three times longer, so treat the months as order, not deadlines. Desktop comes first; the web build and cloud services move after v1.0.

**First test board: Hadi's SDR card.** It merges the ADRV9026 RF transceiver evaluation board with the ZCU102 Zynq UltraScale+ board: about 85 schematic sheets and a 62 MB PcbDoc, with JESD204 links, DDR4, QSFP and FMC interfaces. That is exactly the high-speed case phase 3 targets. The project folder also holds hand-made snapshots ("before\_unroute", "before\_polyremove", "autorouted"), which is the case for built-in version history and visual diff.

## Risks and open questions

The biggest risk is scope: EDA tools take years to mature, so the MVP must win on one sharp workflow before it goes broad.

| Risk | Why it matters | Mitigation |
| --- | --- | --- |
| Scope creep | Schematic, layout, routing and simulation are each a product | Ship phase by phase; reuse ngspice, OpenCASCADE and Clipper2 |
| Router quality | Engineers judge the tool by the router in the first hour | Invest early in push-and-shove; benchmark on real open-hardware boards |
| Switching cost | Users have years of KiCad projects | KiCad and Altium import from day one (schematic, PCB, libraries), tested on real projects |
| Library trust | One bad footprint ruins a board run | Verified badge, automated IPC-7351 checks, community review |
| Web performance | Boards with 10,000+ parts and 50,000+ tracks | Rust core in WebAssembly, GPU rendering, desktop app as fallback |
| Solo maintainer load | One person cannot build every feature | Reuse open libraries, publish early, invite contributors from phase 2 |

Decisions so far:

- [x] MVP leads with Allegro-grade high-speed design
- [x] Open source, like KiCad (GPL-3.0 suggested, the same licence as KiCad)
- [x] Personal project
- [x] Desktop first; a web viewer can come later
- [x] Pick the licence and a project name, then create the GitHub repository
