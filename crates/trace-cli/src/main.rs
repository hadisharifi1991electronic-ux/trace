//! `trace`: command-line front end until the editor exists.

mod view;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use trace_core::{Board, LayerKind, Length, Sheet};

const USAGE: &str = "\
trace: read Altium, Allegro and Gerber designs with the Trace engine

Boards can be Altium .PcbDoc, Cadence Allegro .brd (16.0 or newer), Trace
.tracepcb, or Gerber and drill files: a folder, a .zip, or one file.
Any command takes --stackup <file> to put a stack-up file (see trace stackup)
on the board, for boards whose file has none.

USAGE:
    trace inspect <board|file.SchDoc>           Summary of a board or sheet
    trace pairs <board> [max-skew]              Differential pair lengths and skew (default max-skew 5mil)
    trace fem <waveguide|microstrip|stripline> [key=value ...] [-o out.s2p]
                                                3D FEM (HFSS-style): adaptive tetrahedral mesh to dS, wave ports,
                                                S-parameter sweep. Keys: w h len (mil), er tand, a b (mm, waveguide;
                                                len in mm too), f (adapt GHz), sweep=lo:hi:n, ds, passes, zref (0 = port Z0)
    trace rules <board>                         Design rules by category and kind, in priority order
    trace rules <board> --net <net>             The rule of each kind that applies to a net
    trace rules <board> --check                 Rule problems and scope terms Trace does not judge
    trace rules <board> --params                Every rule with all its parameters
    trace rules <board> --add <Kind> <scope> [KEY=VALUE]... -o <out.PcbDoc>
                                                Adds a rule (ahead of the rules for all nets)
    trace rules <board> --defaults -o <out.PcbDoc>  Default rules for a board that has none
    trace check <board>                         Check length, match, skew and via limits from the board's rules
    trace drc <board> [--all]                   Design rule check: clearance, shorts, width, unrouted nets,
                                                holes, annular rings, mask slivers, silk on pads, silk to silk,
                                                part clearance, board edge, return paths over plane voids
                                                and splits, crosstalk between parallel tracks
    trace impedance <board> [--field]           Stack-up, widths for 50/100 ohm, and the impedance of each pair;
                                                with --field, nets from the field solver, pour beside tracks
                                                counted as coplanar ground
    trace field <board>                         2D field solver on each layer: widths for 50 ohm and 100 ohm
                                                pairs, delay and loss, beside the formulas' answers
    trace field <board> <layer> <w> [s] [--ground G] [out.svg]
                                                Solve one cross-section (a track, or a pair with gap s; mil,
                                                or mm with an mm suffix; --ground: copper G beside it on its layer):
                                                impedance, modes, coupling, RLGC and loss; with out, draw the field
    trace panel <board> <cols>x<rows> <out.tracepcb> [--gap 2mm] [--rail 5mm] [--tabs N] [--vscore]
                                                Panel of copies with rails, fiducials, tooling holes, and
                                                tabs with mouse bites (or V-score lines); then fab or odb it
    trace match <board> <net|pair,...> [out.tracepcb] [--target L] [--amp L]
                                                Match the nets' lengths (to the longest, or the target) with
                                                meanders where they fit; with out, save the board tuned. A
                                                differential pair (named, or both nets given) is tuned as a pair:
                                                both legs meander together and its mean length is matched
    trace sim <board> <U1.pin> <U2.pin> --ibis drv.ibs [--rx-ibis rx.ibs] [--model M] [--rx-model M]
              [--rate 1G] [--bits N] [--corner typ|min|max] [--term 50|50@0.9|100diff] [--single]
              [--df 0.02] [--rough 1um] [--lossless] [--no-xtalk] [--formulas] [out.svg]
                                                Simulate the route from a driver pin to a receiver pin with IBIS
                                                models (a pair when the net has a partner): waveforms and eye,
                                                with track loss and crosstalk from nets beside the route
    trace pdn <board> <net> [--ground GND] [--volts 1.0] [--ripple 3] [--amps 2] [--target 10m] [--band 100M]
              [--fmin 100k] [--fmax 1G] [--points 120] [--vrm U5] [--loads U1,U2] [--cell 50]
              [--die-c 20n] [--die-r 5m] [--pkg-l 50p] [--add 4]
              [--optimise] [--no-assume] [--csv out.csv] [out.svg]
                                                PDN impedance of a supply's plane pair against frequency at
                                                each load, with its capacitors and regulator; target check;
                                                --optimise picks capacitor values and drops spare ones
                                                --die-c: the chip's capacitance inside its package (seen through
                                                --pkg-l), so the curves are what the die sees; --add N: N empty
                                                sites round each load the optimiser may fill; out.svg: the chart
    trace ir <board> <net> [--source U5] [--volts 1.0] [--load U1=5A ...] [--total 10A] [--plating 0.8mil] [out.svg]
                                                DC voltage drop (IR drop) over a supply net's pours, tracks and
                                                vias from the source part to the loads (no --load: the total,
                                                default 1 A, shared by pad count); with out, a drop map per layer
    trace path <board> <from> [<to>] [--ghz 6] [--points 200] [--quick-vias] [--model PATTERN=file.sNp]
               [--load chip.s1p] [--df 0.02] [--fmin GHz] [--fit PATTERN] [--tune [1-6] [--il-weight 1]
               [--any] [--free PATTERN]] [--eye GBPS [--amp 0.8] [--rise PS] [--prbs 7] [--eye-svg eye.svg]
               [--ffe PRE,POST] [--ctle DB[@GHZ]] [--eq auto|ffe|ctle] [--xtalk]]
               [out.s2p] [out.svg] [out.csv]
                                                whole-path loss: the routed path from a pin (J503.1, or a part
                                                with one signal pin, or a net) to another (default: the farthest
                                                chip pin), through series parts and baluns, with shunt parts off
                                                it; traces from the field solver, vias in 3D (--quick-vias: a
                                                coaxial estimate), parts from --model files (by designator,
                                                value or part number; .s2p, .s1p shunt, .s3p balun with port 1
                                                unbalanced) or from their value and case. Cascaded insertion and
                                                return loss to --ghz (worst RL from --fmin); --load ends it in
                                                a chip's port model. Parts valued TBD/DNI/DNP (or not fitted in
                                                the variant) are left out; parts on the same two nets combine,
                                                and of footprints stacked on the same pads one is taken (--fit).
                                                --tune: chooses E12 capacitors and inductors for the empty (TBD,
                                                DNI, or --free) sites for the best worst-case return loss (into
                                                --load) over the band, plus --il-weight x mean insertion loss;
                                                --any lets C and L sites take either.
                                                --eye: PRBS eye at the far end (into --load) from the path's
                                                pulse response: height, width, jitter, worst case from the
                                                cursors; --amp volts p-p into a matched load, --rise 20-80 %
                                                ps (default 0.3 bit); sweeps to 2.5 x the bit rate by default.
                                                --ffe: transmit pre and post taps; --ctle: receiver peaking, dB
                                                at GHz (default half the bit rate); --eq picks them for the
                                                widest worst case eye. --xtalk: nets beside the path's traces
                                                switch too (near or far end, whichever is worse), in the eye and
                                                as each one's worst case closure.
    trace rf <board> [--band 0.5-6] [--points 200] [--quick-vias] [--model PATTERN=file.sNp]
             [--load PATTERN=chip.s1p]... [--rl 10] [--il-budget DB] [--only J3*] [--from PIN]...
             [--fit PATTERN] [--df 0.02] [--out DIR] [report.txt] [report.csv] [report.svg]
                                                whole-board RF report: every coaxial connector's signal pin (SMA,
                                                U.FL, MMCX..., or a connector with one signal pin among ground
                                                pins) followed through series parts and baluns to the farthest
                                                chip pin, as trace path does, each kind of via solved in 3D once.
                                                Paths named and sorted by their nets (RX, ORX, TX, LO, clock);
                                                IL at points in the band, worst RL from its bottom (into a chip
                                                model where --load PATTERN matches the end pin, its net, the
                                                path or its class), the sections that reflect and lose most.
                                                --rl: return loss goal; --il-budget: IL limit at the top.
                                                --out DIR: report, CSV, chart, and each path's .s2p and .svg.
                                                Exits 1 when a path needs a look
    trace lc <value> [--case 0402] [--family LQW15AN] [--band 0.1-6] [--points 13] | trace lc --list
                                                chip L/C parasitics library (used by trace path, trace rf and
                                                --tune): SRF, Q, ESR/ESL and the impedance over the band of a
                                                0201/0402/0603/0805 capacitor (pF/nF/uF value) or inductor (nH),
                                                by family (Murata GJM/GRM/GQM, LQW/LQG/LQP, Coilcraft 0201HL/
                                                0402HP/0402DC/0603HP; a part number picks it) or generic by case.
                                                Typical datasheet values; a vendor .s2p (--model) wins
    trace bom <board> [--boards N] [--parts parts.csv]... [--mouser KEY] [--refresh] [--no-cache]
              [--template parts.csv] [bom.csv|report.txt]
                                                BOM with part data: lines by part number, value and footprint
                                                (variant and DNP parts left off), joined with each part
                                                number's lifecycle, stock, price breaks and datasheet from
                                                parts files (CSV: MPN, Manufacturer, Lifecycle, Stock,
                                                Price@1, Price@100 ...), the cache of earlier lookups, or the
                                                Mouser Search API (--mouser KEY or MOUSER_API_KEY; free key
                                                from mouser.com/api-hub). Flags obsolete, end of life and NRND
                                                parts, too little stock for N boards, no part number, a part
                                                number on two footprints, lines that could share a part, one
                                                source; costs per board and for N. --template writes a parts
                                                file to fill in. Exits 1 on errors.
    trace thermal <board> --power U1=2.5W[@jc0.5/jb8]... [--heatsink U1=40x40x20[,fins=10,fin=1,base=3,
                  k=200,tim=0.1@3,contact=0.05]] [--ambient 25] [--air M/S] [--vertical] [--cell MM]
                  [--k-diel 0.81/0.29] [--check] [--maps t,flux,gradient,power,air] [--planes all]
                  [--at X,Y] [--decades 3] [--sweep depth|x|y] [map.svg] [maps.gif] [temps.csv] [report.txt]
                                                3D steady-state temperatures: board conduction from the real copper
                                                of every layer and the via barrels, parts as JEDEC two-resistor
                                                packages (θJC, θJB; guessed from package kind and size when not given), plate-fin
                                                heatsinks with TIM and contact resistance, natural or forced
                                                convection from correlations and radiation; Tj per part, maps per
                                                layer, heat balance; --check solves again on a 1.5x finer grid and
                                                gives error bars. map.svg: density maps of temperature, heat flux
                                                |k grad T|, gradient, power density put in and heat to air on every
                                                copper layer, mid-dielectric and side views through the hottest
                                                point (--at X,Y mm; --planes all for every slab; --maps picks);
                                                maps.gif sweeps the cut down the layers (--sweep x|y: side views)
    trace bus <board> NETS... --from U1 [--to U5] --rate MT/S [--strobe DQS0] [--swing 0.8] [--rise PS]
              [--setup PS] [--hold PS] [--mask 136mV,0.2UI] [--offset 0.5 | --edge-aligned] [--no-xtalk]
              [--vias-3d] [--prbs 7] [timing.svg] [bits.csv] [report.txt]
                                                parallel bus (DDR byte lane, address bus, any source-synchronous
                                                group; NETS are names or patterns such as DDR_DQ*): every bit's
                                                routed path cascaded from the board, its eye with the nets beside
                                                it switching, its valid window against the strobe's edge (flight
                                                time, launch offset, jitter), setup/hold margins per bit, the
                                                receiver mask at the strobe (DDR4-2400: 136mV,0.2UI), and the
                                                bus's margin as routed, trained and per-bit deskewed. Exits 1 on
                                                a failure
    trace emi <board> [--net CLK=25MHz@2ns,3.3V]... [--no-auto] [--clock 50MHz@1ns,3.3V] [--signal ..] [--ibis ..]
              [--cable J1=1m] [--rail VCC=1A@100MHz] [--map near.svg] [--quick]
              [--limit cispr-b|cispr-a|fcc-b|fcc-a] [--margin 6] [spectrum.svg] [findings.csv] [report.txt]
                                                board-level EMI check of the fast nets (clocks found by name,
                                                the nets given, every pair): tracks over plane gaps or splits,
                                                layer changes with no return via or capacitor near, fast tracks
                                                near the board edge, ground stitching along the edge over
                                                lambda/20, and each clock's radiation (differential-mode loop,
                                                Ott) against the FCC or CISPR 32 limit; then the EMI simulation:
                                                every net and pair as a source (IBIS --ibis U1=f.ibs, given
                                                --signal NET=12Gbps@30ps,0.8V | 125MHz | 2.4GHz,0dBm, or a whole
                                                kind: class:rf|lo|serdes|lvds|clock|cmos=..., else a
                                                default for its kind), radiated by its outer traces (loops with
                                                the wave's phase, worst direction), by the cables on the
                                                connectors (common mode, --cable J1=2m), by the power planes'
                                                cavity edges (PDN model with the caps; --rail VCC=1A@100MHz) and
                                                the decoupling loops: spectrum against the limit with each
                                                source's share (spectrum.svg), the fixes and what each gains, and
                                                --map near.svg: |H| over the board and current density per layer
                                                at the worst frequency. --quick: the layout check only;
                                                --no-cmos, --no-planes. Exits 1 on a high finding or a failure
    trace flex <board> [--radius [B1=]MIL] [--dynamic] [--keepout 30] [--angle 10] [report.txt]
                                                rigid-flex: zones with their layers and thickness, bends with
                                                their least radius (IPC-2223: 6/12/24x the flex, 100x dynamic),
                                                and the flex checks: vias and parts in flex or near its edge,
                                                tracks off square across a bend or turning in it, copper on
                                                layers a zone lacks, stacked tracks or pours in a bend
    trace compare <first> <second> [--tol 1] [--length-tol 10] [--no-routing] [report.txt|report.csv]
                                                design compare: parts added, removed, moved, rotated, flipped or
                                                changed (footprint, value, part number), nets added, removed,
                                                renamed (same pins) or with pins joined or left, routing per
                                                net (length, vias, layers), net classes, pairs, rules and the
                                                stack-up; exits 1 when they differ
    trace zcheck <board> [--tol 10] [--rule PATTERN=Z] [--diff PATTERN=Z] [--no-pairs] [--no-rf]
                 [--no-vias] [--via-kinds 6] [--min-length 30] [--ghz 6] [--no-name-pairs] [report.csv]
                                                board-wide impedance check: every controlled trace run (field
                                                solver, per net, layer and width) and every kind of via (3D)
                                                against its target. Targets: --rule/--diff (net or pair name,
                                                * and ?), every pair (100, USB 90, PCIe/SATA 85) and nets on
                                                coax connector pins (50). Off by more than --tol % fails.
    trace via <board> <layer> <layer> [--drill D] [--pad P] [--antipad A] [--width W] [--width2 W]
              [--pair PITCH] [--oval] [--returns N@D] [--backdrill] [--fmax 40] [--cell C]
              [--target 50] [--fab drill=8-20,ring=4,antipad=20-80,gap=4]
              [--er 3.6] [--df 0.004] [--lossless] [--rough 1.0] [--feed 1in]
              [--optimise GHz [--free-pitch] [--try-backdrill] [--keep-pad]] [--check] [out.s2p] [out.svg]
                                                3D full-wave (FDTD) solve of a via from one layer to another
                                                with the board's stack-up (default drill, pad and antipad from
                                                its commonest via): S11/S21, or Sdd for a pair, as Touchstone
                                                and a plot; the stub resonance when the barrel runs on.
                                                --optimise GHz sweeps the antipad (--oval for a pair) and the
                                                return vias for the best return loss to GHz, keeping the pair's
                                                pitch unless --free-pitch; it also tries smaller pads (unless
                                                --keep-pad) and, with --try-backdrill, removing the stub.
                                                --check solves again on a finer grid and says if it converged;
                                                --accurate [dB] refines the grid until |S21| moves less than dB
                                                (0.05), extrapolates to a zero cell size and gives error bars;
                                                every solve ends with a passivity and causality check;
                                                round metal is conformal (--staircase for the old stepped grid);
                                                --target Z sizes the antipad (then the drill) for that via
                                                impedance within the --fab limits; with --fab, --optimise also
                                                tries drills in range and counts the through loss.
                                                --target and --optimise judge by the accurate (converged) solve
                                                by default, the quick grid only steering; --quick for the old
                                                single finer grid.
                                                --fields GHz[,GHz] map.svg keeps the fields at those frequencies
                                                and draws |E|, |H|, energy density and surface current density
                                                on cut planes (each trace layer, mid-way between the planes the
                                                barrel crosses, side views along and across), dB colour scale,
                                                per 1 W in; map.gif animates one plane through a cycle
                                                (--field-plane N|name, --field-quantity E|H|energy|current,
                                                --field-db 40 for the scale's depth)
                                                --check openems [--openems-dir DIR] [--openems-exe PATH]
                                                [--openems-cell C] runs the same transition in openEMS (an
                                                independent FDTD, C:\\openEMS by default) and compares the two:
                                                table, compare.svg, both .s2p in DIR; --export-openems DIR only
                                                writes the model; --openems-read DIR reads a finished run
    trace pairsize <board> <layer> [--zdiff 100] [--wmin W] [--wmax W] [--gap S] [--ghz 8] [--next D] [--xtalk 2]
                                                Every track width and gap on a layer that gives the target Zdiff
                                                (2D field solver), ranked by loss, with crosstalk and etch
    trace cascade <load.s1p> [stage.s2p | --line Z@len@Eeff[@dB/in]]... [--ref R] [--step GHz]
                                                Return loss into a chain of two-ports (vias from trace via, lines)
                                                ending in a load such as a chip's port model, input side first
    trace sparam check <file.sNp>...           Passivity, reciprocity and causality of any Touchstone file (from
                                                Trace, a vendor or another solver); exits 1 if one fails
    trace stitch <board> <net> <out.tracepcb> [--grid 100mil] [--stagger] [--via 16/8] [--layers 1-4]
              [--clearance C] [--edge 20mil] [--area x0,y0,x1,y1]
                                                Stitch a net's pours and planes with vias on a grid, each clear
                                                of other copper and landing on the net on two or more layers
    trace fence <board> <net[,net]> <out.tracepcb> [--ground GND] [--ghz 6 | --pitch P] [--gap G]
              [--rows 1] [--via 16/8] [--layers 1-4] [--on 1,3] [--clearance C]
                                                Line a net's tracks (RF lines, a pair) with ground vias on both
                                                sides, a twentieth of a wavelength apart at GHz (default 6)
    trace autoroute <board> <out.tracepcb> [--nets a,b] [--unroute] [--layers 1,4] [--width 6mil]
                    [--clearance 6mil] [--via 20/10] [--via-cost 60] [--poured]
                                                Route the open connections (of the nets named, or all but the
                                                poured ones unless --poured): tracks and through vias around
                                                what is on the board, shortest first. --unroute takes up the
                                                named nets' routing first. Layers count from 1 (top); planes
                                                are left out by default
    trace teardrops <board> <out.tracepcb> [--nets a,b] [--smd] [--no-vias] [--no-th] [--length 1.0]
              [--width 0.9] [--clearance C] [--remove]
                                                Teardrops where tracks meet vias and through-hole pads (--smd:
                                                surface pads too), kept clear of other nets; --remove takes them off
    trace gloss <board> <out.tracepcb> [--nets a,b] [--no-merge] [--keep-dangling] [--corners] [--clearance C]
                                                Tidy routing: zero-length and duplicate tracks, straight joins
                                                merged, dangling ends cut back, square corners chamfered (--corners)
    trace swap <board> <part> <pin> <pin> <out.tracepcb>
    trace erc <sheet.SchDoc|project.PrjPcb|sch.tracesch> [--all] [--scope automatic|flat|hierarchical|strict|global]
    trace hierarchy <project.PrjPcb|sch.tracesch> [--scope S]
                                                Sheet tree, and how the sheets' nets join (the project's
                                                net identifier scope unless --scope says)
    trace annotate <sheet.SchDoc|project.PrjPcb|sch.tracesch> <out.tracesch> [--all] [--down] [--start N] [--per-sheet 100]
    trace fanout <board> <U1[,U2]> <out.tracepcb> [--via 18/8] [--layers 1-4] [--in-pad] [--inward]
              [--skip-rows N] [--width W] [--clearance C] [--no-net]
                                                Fan out BGA and other surface parts: a short track from each
                                                pad to a via between the pads (away from the centre), or a via
                                                in the pad; outer rows can be left for surface routing
    trace testpoints <board> [out.tracepcb] [--top|--both] [--smd] [--no-vias] [--no-th] [--size 0.5mm]
              [--spacing 1.27mm] [--clearance 0.5mm] [--edge 2mm] [--tented] [--fresh] [--report out.csv]
                                                Test points for bed-of-nails or flying-probe test: one probe point
                                                per net (bottom side by default) on a via or pad big enough, clear
                                                of parts, the edge and other probes; coverage and the nets missed
    trace reuse <board> <U1,R1,C1,...> <U2> <out.tracepcb> [--margin 1mm] [--keep-routing]
                                                Copy a laid-out block onto another instance of it: the parts
                                                listed (anchor first) are matched with the copy's through their
                                                nets, starting from the copy's anchor U2, placed alike, and the
                                                block's tracks, arcs and vias copied with the copy's nets (the
                                                copy's own routing there is taken up first, unless --keep-routing)
    trace clear3d <board> [--clearance 0.25mm] [--max-top 10mm] [--max-bottom 3mm] [--no-models]
                  [--no-estimate] [--no-edge]
                                                3D clearance: part bodies (their 3D models, or boxes estimated
                                                from the pads) that run into each other or come closer than
                                                --clearance, parts taller or lower than the board's Height rules
                                                or the limits given, and bodies over the board edge
    trace nettie <board> <part[,part]> <out.tracepcb> [--remove]
                                                Mark parts as net ties: their own copper may join their nets
                                                (parts named NT…, or with a NetTie footprint, already are)
    trace variants <board>                      Assembly variants (read from the Altium project beside a PcbDoc):
                                                parts not fitted and changed in each. Any command takes
                                                --variant <name> to make its BOM, pick and place, assembly
                                                drawing or STEP for that variant
    trace stackup <board> [out.txt]             Print the board's stack-up as a stack-up file (name, kind,
                                                thickness, Er, Df); edit it and give it back with --stackup
    trace validate [--quick] [out.txt]          Accuracy checks: the field, IR drop and 3D via solvers against
                                                exact answers, the 2D solver and finer grids (--quick: no 3D)
    trace stubs <board> [out.tracepcb]          Via stubs on nets with a stub rule (default: pair nets, 10 mil);
                                                with out, backdrill those too long and save the board
    trace view <board> [out.html]               Open the board in the viewer (in your browser)
    trace convert <file> <out.json>             Export to Trace's open JSON model
    trace convert <file> <out.kicad_pcb>        Write a KiCad board (KiCad 7 and later)
    trace convert <file> <out.PcbDoc>           Write an Altium Designer board
    trace convert <file> <out.xml>              Write an IPC-2581 (revision C) file
    trace convert <file> <out.dxf>              Write every layer of a board as a DXF (mm)
    trace convert <file.SchDoc> <out.svg>       Draw a schematic sheet as SVG
    trace convert <file.SchDoc> <out.SchDoc>    Write a schematic sheet as an Altium sheet
    trace convert <file.tracesch> <folder>      Write every sheet of a Trace schematic as Altium sheets
    trace fab <board> <out.zip|folder>          Gerber X2, drill, IPC-356, pick-and-place and BOM files
    trace odb <board> <out.tgz|folder>          ODB++ job: layers, drills, parts and nets in one package
    trace assembly <board> <out.pdf> [--a4]     Assembly drawing: top and bottom views with part bodies,
                                                designators and pin 1 marks, then the parts list (A3)
    trace step <board> <out.step> [--vias] [--no-parts] [--min-hole 0.3]
                                                STEP (AP214) model for mechanical CAD: the board solid with its
                                                cut-outs and holes (vias with --vias, mm), parts as boxes
    trace fabdrawing <board> <out.pdf> [--a4]   Fabrication drawing: outline with drill symbols and
                                                dimensions, drill table, notes, then the stack-up (A3)
    trace layers <gerber folder|zip|board>      Show which layer each Gerber or drill file was taken as
                                                (for a board, its stack-up)
    trace symbols <lib.SchLib|IntLib> [out.svg] List the symbols of an Altium schematic library;
                                                with out, draw them all as SVG
    trace symwizard <pins.csv> <name> <lib.SchLib> [--split-power]
                                                Lay a box symbol out from a pin list (number, name,
                                                type, side) and add it to the library (made if new)
    trace update-symbols <sheet.SchDoc> <lib.SchLib> <out.SchDoc>
                                                Redraw the sheet's placed parts from the library's
                                                components, wires following moved pins
    trace footprints <lib|board> [out]          List the footprints of a .PcbLib, .IntLib, .tracelib or board;
                                                with out, save them as a Trace library (.tracelib)
                                                or an Altium one (.PcbLib)
";

/// A stack-up file given with --stackup, put on every board loaded.
static STACKUP: std::sync::OnceLock<trace_core::Stackup> = std::sync::OnceLock::new();
/// The assembly variant outputs are made for (`--variant`).
static VARIANT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--stackup") {
        let Some(path) = args.get(i + 1).cloned() else {
            eprintln!("error: --stackup needs a file");
            return ExitCode::FAILURE;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| trace_core::stackfile::parse(&t))
        {
            Ok(st) => {
                let _ = STACKUP.set(st);
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
        args.drain(i..i + 2);
    }
    if let Some(i) = args.iter().position(|a| a == "--variant") {
        let Some(name) = args.get(i + 1).cloned() else {
            eprintln!("error: --variant needs a name");
            return ExitCode::FAILURE;
        };
        let _ = VARIANT.set(name);
        args.drain(i..i + 2);
    }
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version" | "-V" | "version"] => {
            println!("trace {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["inspect", file] => inspect(file),
        ["pairs", file] => pairs(file, "5mil"),
        ["pairs", file, skew] => pairs(file, skew),
        ["fem", kind, rest @ ..] => fem(kind, rest),
        ["rules", file] => rules(file),
        ["rules", file, "--params"] => rules_params(file),
        ["rules", file, "--check"] => rules_check(file),
        ["rules", file, "--net", net] => rules_net(file, net),
        ["rules", file, "--defaults", "-o", out] => rules_edit(file, out, |b| {
            if !b.rules.is_empty() {
                return Err("the board has rules already".into());
            }
            b.rules = trace_core::rules::defaults(b);
            Ok(format!("added {} default rules", b.rules.len()))
        }),
        ["rules", file, "--add", kind, scope, rest @ ..] => {
            let (kv, out) = match rest {
                [kv @ .., "-o", out] => (kv, *out),
                _ => {
                    eprint!("{USAGE}");
                    return ExitCode::from(2);
                }
            };
            rules_edit(file, out, |b| {
                let mut r = trace_core::rules::new_rule(&b.rules, kind);
                r.scope[0] = scope.to_string();
                for item in kv {
                    let (k, v) = item.split_once('=').ok_or("parameters are KEY=VALUE")?;
                    trace_core::rules::set_param(&mut r, k, v);
                }
                if let Err(e) = trace_core::rules::check_scope(scope) {
                    return Err(format!("scope does not read: {e}").into());
                }
                // A rule for some nets goes ahead of the rules for all.
                if !scope.trim().eq_ignore_ascii_case("all") {
                    for x in b
                        .rules
                        .iter_mut()
                        .filter(|x| x.kind.eq_ignore_ascii_case(&r.kind))
                    {
                        x.priority += 1;
                    }
                    r.priority = 1;
                }
                let said = format!("added {} ({}) at priority {}", r.name, r.kind, r.priority);
                b.rules.push(r);
                Ok(said)
            })
        }
        ["check", file] => check(file),
        ["drc", file] => drc(file, 15),
        ["drc", file, "--all"] => drc(file, usize::MAX),
        ["impedance", file] => impedance(file, false),
        ["impedance", file, "--field"] => impedance(file, true),
        ["field", file] => field_layers(file),
        ["field", file, layer, w, rest @ ..] => field_one(file, layer, w, rest),
        ["panel", file, grid, out, rest @ ..] => panel(file, grid, out, rest),
        ["match", file, nets, rest @ ..] => match_nets(file, nets, rest),
        ["sim", file, from, to, rest @ ..] => sim(file, from, to, rest),
        ["ir", file, net, rest @ ..] => ir(file, net, rest),
        ["pdn", file, net, rest @ ..] => pdn_cmd(file, net, rest),
        ["via", file, from, to, rest @ ..] => via(file, from, to, rest),
        ["zcheck", file, rest @ ..] => zcheck_cmd(file, rest),
        ["path" | "pathloss", file, rest @ ..] => path_cmd(file, rest),
        ["rf" | "rfreport", file, rest @ ..] => rf_cmd(file, rest),
        ["lc", value, rest @ ..] => lc_cmd(value, rest),
        ["compare" | "diff", a, b, rest @ ..] => compare_cmd(a, b, rest),
        ["flex", file, rest @ ..] => flex_cmd(file, rest),
        ["thermal", file, rest @ ..] => thermal_cmd(file, rest),
        ["bus", file, rest @ ..] => bus_cmd(file, rest),
        ["emi", file, rest @ ..] => emi_cmd(file, rest),
        ["bom", file, rest @ ..] => bom_cmd(file, rest),
        ["pairsize", file, layer, rest @ ..] => pairsize(file, layer, rest),
        ["validate", rest @ ..] => validate(rest),
        ["stitch", file, net, out, rest @ ..] => stitch_cmd(file, net, out, rest),
        ["autoroute", file, out, rest @ ..] => autoroute_cmd(file, out, rest),
        ["teardrops", file, out, rest @ ..] => teardrops_cmd(file, out, rest),
        ["gloss", file, out, rest @ ..] => gloss_cmd(file, out, rest),
        ["fanout", file, parts, out, rest @ ..] => fanout_cmd(file, parts, out, rest),
        ["reuse", file, block, anchor, out, rest @ ..] => reuse_cmd(file, block, anchor, out, rest),
        ["clear3d", file, rest @ ..] => clear3d_cmd(file, rest),
        ["nettie", file, parts, out] => nettie_cmd(file, parts, out, true),
        ["nettie", file, parts, out, "--remove"] => nettie_cmd(file, parts, out, false),
        ["variants", file] => board(file).map(|b| print!("{}", trace_core::variant::summary(&b))),
        ["testpoints", file, rest @ ..] => testpoints_cmd(file, rest),
        ["erc", file, rest @ ..] => erc_cmd(file, rest),
        ["hierarchy", file, rest @ ..] => hierarchy_cmd(file, rest),
        ["swap", file, part, a, b, out] => swap_cmd(file, part, a, b, out),
        ["annotate", file, out, rest @ ..] => annotate_cmd(file, out, rest),
        ["fence", file, nets, out, rest @ ..] => fence_cmd(file, nets, out, rest),
        ["stackup", file] => stackup(file, None),
        ["stackup", file, out] => stackup(file, Some(out)),
        ["cascade", load, rest @ ..] => cascade(load, rest),
        ["sparam", "check", files @ ..] if !files.is_empty() => sparam_check(files),
        ["stubs", file] => stubs(file, None),
        ["stubs", file, out] => stubs(file, Some(out)),
        ["convert", file, out] => convert(file, out),
        ["layers", file] => layers(file),
        ["fab", file, out] => fab(file, out),
        ["odb", file, out] => odb(file, out),
        ["assembly", file, out] => assembly(file, out, trace_export::Paper::A3),
        ["assembly", file, out, "--a4"] => assembly(file, out, trace_export::Paper::A4),
        ["step", file, out, rest @ ..] => step_cmd(file, out, rest),
        ["fabdrawing", file, out] => fab_drawing(file, out, trace_export::Paper::A3),
        ["fabdrawing", file, out, "--a4"] => fab_drawing(file, out, trace_export::Paper::A4),
        ["symbols", file] => symbols(file, None),
        ["symbols", file, out] => symbols(file, Some(out)),
        ["symwizard", pins, name, lib] => symwizard(pins, name, lib, false),
        ["symwizard", pins, name, lib, "--split-power"] => symwizard(pins, name, lib, true),
        ["update-symbols", sheet, lib, out] => update_symbols(sheet, lib, out),
        ["footprints", file] => footprints(file, None),
        ["footprints", file, out] => footprints(file, Some(out)),
        ["view", file] => view_board(file, None),
        ["view", file, out] => view_board(file, Some(out)),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<(), Box<dyn std::error::Error>>;

/// Adds a box symbol laid out from a pin list to a `.SchLib`.
fn symwizard(pins: &str, name: &str, lib: &str, split: bool) -> Res {
    let text = std::fs::read_to_string(pins)?;
    let list = trace_core::symbol::parse_pin_list(&text);
    if list.is_empty() {
        return Err("no pins in the list".into());
    }
    let mut def = trace_core::symbol::SymbolDef::from_pin_list(name, &list);
    if split {
        def.split_power_part();
    }
    let mut file = if std::path::Path::new(lib).exists() {
        trace_altium::read_schlib_file(lib)?
    } else {
        let stem = std::path::Path::new(lib)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        trace_altium::SchLibFile::new(&stem)
    };
    for p in def.problems() {
        println!("  check: {p}");
    }
    file.symbols.retain(|d| !d.name.eq_ignore_ascii_case(name));
    file.symbols.push(def);
    std::fs::write(lib, file.write()?)?;
    let d = file.symbols.last().unwrap();
    println!(
        "{lib}: {name} with {} pins on {} part(s); {} components in the library",
        d.pins.len(),
        d.parts,
        file.symbols.len()
    );
    Ok(())
}

/// Redraws a sheet's placed parts from a library.
fn update_symbols(sheet: &str, lib: &str, out: &str) -> Res {
    let mut s = trace_altium::read_schdoc(sheet)?;
    let file = trace_altium::read_schlib_file(lib)?;
    let mut total = 0;
    for i in 0..file.symbols.len() {
        let parts = file.lib_symbols(i);
        let rep = s.update_from_library(&parts);
        if rep.updated.is_empty() {
            continue;
        }
        total += rep.updated.len();
        println!(
            "  {:<24} {} part(s): {}; wires moved on {} pin(s){}",
            file.symbols[i].name,
            rep.updated.len(),
            rep.updated.join(" "),
            rep.wires_moved,
            if rep.lost_pins.is_empty() {
                String::new()
            } else {
                format!("; pins gone: {}", rep.lost_pins.join(" "))
            }
        );
    }
    std::fs::write(
        out,
        trace_altium::write_schdoc(&s, Some(std::path::Path::new(sheet)))?,
    )?;
    println!("{out}: {total} part(s) redrawn");
    Ok(())
}

/// Lists the symbols of a `.SchLib`; with `out`, draws them all on one
/// SVG sheet.
fn symbols(file: &str, out: Option<&str>) -> Res {
    let lib = if file.to_ascii_lowercase().ends_with(".intlib") {
        trace_altium::read_intlib(file)?.symbols
    } else {
        trace_altium::read_schlib(file)?
    };
    println!("{}: {} symbols", lib.name, lib.symbols.len());
    for s in &lib.symbols {
        let pins = s.drawing.pins.len();
        println!(
            "  {:<32} {:<5} {:>4} pins  {}  {}",
            s.title(),
            s.symbol.designator,
            pins,
            if s.symbol.footprint.is_empty() {
                "-"
            } else {
                &s.symbol.footprint
            },
            s.symbol.description
        );
    }
    if let Some(out) = out {
        // A grid of symbols, each in a cell as big as the biggest.
        let cell = lib
            .symbols
            .iter()
            .map(|s| s.drawing.size[0].max(s.drawing.size[1]))
            .fold(400.0f64, f64::max)
            + 400.0;
        let cols = (lib.symbols.len() as f64).sqrt().ceil().max(1.0) as usize;
        let rows = lib.symbols.len().div_ceil(cols).max(1);
        let mut sheet = trace_core::Sheet {
            name: lib.name.clone(),
            ..Default::default()
        };
        sheet.drawing.size = [cols as f64 * cell, rows as f64 * cell];
        sheet.drawing.paper = [255, 255, 255];
        for (k, s) in lib.symbols.iter().enumerate() {
            let q = &s.drawing.parts[0];
            let mid = [(q.min[0] + q.max[0]) / 2.0, (q.min[1] + q.max[1]) / 2.0];
            let (c, r) = ((k % cols) as f64, (rows - 1 - k / cols) as f64);
            let at = [(c + 0.5) * cell - mid[0], (r + 0.5) * cell - mid[1]];
            sheet.place_symbol(s, at, &s.symbol.designator);
            sheet.drawing.texts.push(trace_core::SchText {
                at: [(c + 0.5) * cell, r * cell + 100.0],
                text: s.title(),
                size: 80.0,
                color: [0, 0, 0],
                rot: 0,
                just: 1,
                bold: false,
                italic: false,
                wrap: None,
                part: None,
            });
        }
        std::fs::write(out, trace_export::sheet_svg(&sheet))?;
        println!("wrote {out}");
    }
    Ok(())
}

fn footprints(file: &str, out: Option<&str>) -> Res {
    use trace_core::footprint::{Footprint, Library};
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let lib = match ext.as_str() {
        "pcblib" => trace_altium::read_pcblib(file)?,
        "intlib" => trace_altium::read_intlib(file)?.footprints,
        "tracelib" => Library::load(Path::new(file))?,
        _ => {
            let Design::Board(board) = load(file)? else {
                return Err("not a board or footprint library".into());
            };
            let mut seen = std::collections::HashSet::new();
            let mut lib = Library::default();
            for (i, c) in board.components.iter().enumerate() {
                if seen.insert(c.footprint.to_ascii_uppercase()) {
                    lib.footprints.extend(Footprint::from_part(&board, i));
                }
            }
            lib
        }
    };
    for f in &lib.footprints {
        let b = f.bounds();
        println!(
            "{:40} {:4} pads  {:7.2} x {:7.2} mm  {}",
            f.name,
            f.pads.len(),
            (b[2] - b[0]) * 0.0254,
            (b[3] - b[1]) * 0.0254,
            f.description
        );
    }
    println!("{} footprints", lib.footprints.len());
    if let Some(out) = out {
        let mut lib = lib;
        lib.version = env!("CARGO_PKG_VERSION").to_string();
        if out.to_ascii_lowercase().ends_with(".pcblib") {
            let name = Path::new(out)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            std::fs::write(out, trace_export::to_pcblib(&lib, &name)?)?;
        } else {
            lib.save(Path::new(out))?;
        }
        println!("saved {out}");
    }
    Ok(())
}

#[allow(clippy::large_enum_variant)]
enum Design {
    Board(Board),
    Sheet(Sheet),
}

fn load(file: &str) -> Result<Design, Box<dyn std::error::Error>> {
    let mut d = load_file(file)?;
    if let (Design::Board(b), Some(st)) = (&mut d, STACKUP.get()) {
        trace_core::stackfile::apply(b, st)?;
    }
    if let (Design::Board(b), Some(v)) = (&mut d, VARIANT.get()) {
        trace_core::variant::choose(b, Some(v))?;
    }
    Ok(d)
}

fn load_file(file: &str) -> Result<Design, Box<dyn std::error::Error>> {
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("pcbdoc") => Ok(Design::Board(trace_altium::read_pcbdoc(file)?)),
        Some("schdoc") => Ok(Design::Sheet(trace_altium::read_schdoc(file)?)),
        Some("brd") => Ok(Design::Board(trace_allegro::read_brd(file)?)),
        Some("tracepcb") => Ok(Design::Board(trace_core::file::load(Path::new(file))?)),
        _ if is_fab(file) => Ok(Design::Board(trace_gerber::read_path(file)?.board)),
        _ => {
            Err(format!("{file}: expected a .PcbDoc, .brd, .SchDoc, Gerber folder or .zip").into())
        }
    }
}

fn is_fab(file: &str) -> bool {
    let p = Path::new(file);
    p.is_dir() || file.to_ascii_lowercase().ends_with(".zip") || trace_gerber::is_fab_file(file)
}

fn layers(file: &str) -> Res {
    if !is_fab(file) {
        // A board: its stack-up.
        return stackup(file, None);
    }
    let t = std::time::Instant::now();
    let imp = trace_gerber::read_path(file)?;
    println!("{:<52} {:<26} Because of", "File", "Taken as");
    for f in &imp.files {
        let role = match f.copper_index {
            Some(i) => format!("{} (L{})", f.role, i + 1),
            None => f.role.clone(),
        };
        println!("{:<52} {:<26} {}", f.file, role, f.reason);
    }
    let g = &imp.board.geometry;
    println!();
    println!(
        "Board: {} copper layers, {} tracks, {} arcs, {} pads, {} vias, {} pours, outline {} points",
        imp.board.stackup.copper_layers().count(),
        g.tracks.iter().filter(|t| matches!(t.layer, trace_core::LayerRef::Copper(_))).count(),
        g.arcs.iter().filter(|t| matches!(t.layer, trace_core::LayerRef::Copper(_))).count(),
        g.pads.len(),
        g.vias.len(),
        g.polygons.len(),
        g.outline.len()
    );
    println!("Nets: {} ({})", imp.board.nets.len(), imp.nets_from);
    for w in imp.warnings.iter().take(10) {
        println!("warning: {w}");
    }
    println!("Read in {:.2} s", t.elapsed().as_secs_f64());
    Ok(())
}

fn board(file: &str) -> Result<Board, Box<dyn std::error::Error>> {
    match load(file)? {
        Design::Board(b) => Ok(b),
        Design::Sheet(_) => Err("this command needs a board (.PcbDoc or .brd)".into()),
    }
}

fn inspect(file: &str) -> Res {
    match load(file)? {
        Design::Board(b) => print_board(&b),
        Design::Sheet(s) => print_sheet(&s),
    }
    Ok(())
}

fn print_board(b: &Board) {
    let copper = b.stackup.copper_layers().count();
    println!(
        "Board: {copper} copper layers, {:.2} mm thick",
        b.stackup.total_thickness().mm()
    );
    println!();
    println!(
        "  {:<16} {:<11} {:>10} {:>6}",
        "Layer", "Type", "Thickness", "Er"
    );
    for l in &b.stackup.layers {
        if matches!(
            l.kind,
            LayerKind::Silkscreen | LayerKind::Paste | LayerKind::Other
        ) {
            continue;
        }
        let er = l
            .dielectric_constant
            .map(|e| format!("{e:.2}"))
            .unwrap_or_default();
        println!(
            "  {:<16} {:<11} {:>7.1} um {:>6}",
            l.name,
            format!("{:?}", l.kind),
            l.thickness.0 as f64 / 1000.0,
            er
        );
    }
    let top = b
        .components
        .iter()
        .filter(|c| c.side == trace_core::Side::Top)
        .count();
    println!();
    println!(
        "Components: {} ({} top, {} bottom)",
        b.components.len(),
        top,
        b.components.len() - top
    );
    let multi: Vec<_> = b
        .nets
        .iter()
        .filter(|n| n.pads >= 2 && !n.has_pour)
        .collect();
    let open = multi.iter().filter(|n| !n.is_complete()).count();
    println!(
        "Nets: {} ({} join two or more pads: {} fully routed, {} still open; {} use copper pours, not checked yet)",
        b.nets.len(),
        multi.len(),
        multi.len() - open,
        open,
        b.nets.iter().filter(|n| n.has_pour).count()
    );
    println!(
        "Net classes: {}   Differential pairs: {} in {} classes",
        b.net_classes.len(),
        b.diff_pairs.len(),
        b.diff_pair_classes.len()
    );
    println!("Rules: {}", b.rules.len());
    println!(
        "Primitives: {} tracks, {} vias, {} pads, {} polygons",
        b.stats.tracks, b.stats.vias, b.stats.pads, b.stats.polygons
    );
}

fn print_sheet(s: &Sheet) {
    println!("Sheet: {}", s.name);
    println!(
        "Symbols: {}   Wires: {}   Junctions: {}",
        s.symbols.len(),
        s.wires,
        s.junctions
    );
    println!(
        "Net labels: {}   Power ports: {}   Ports: {}",
        s.net_labels.len(),
        s.power_ports.len(),
        s.ports.len()
    );
    if !s.sheet_refs.is_empty() {
        println!("Child sheets: {}", s.sheet_refs.join(", "));
    }
    let mut symbols: Vec<_> = s
        .symbols
        .iter()
        .filter(|x| !x.designator.is_empty())
        .collect();
    symbols.sort_by_key(|x| natural(&x.designator));
    if !symbols.is_empty() {
        println!();
        println!(
            "  {:<10} {:<28} {:<16} {:>5}",
            "Ref", "Part", "Value", "Pins"
        );
        for x in symbols.iter().take(40) {
            println!(
                "  {:<10} {:<28} {:<16} {:>5}",
                x.designator,
                clip(&x.library_ref, 28),
                clip(x.value.as_deref().unwrap_or(""), 16),
                x.pins
            );
        }
        if symbols.len() > 40 {
            println!("  ... and {} more", symbols.len() - 40);
        }
    }
}

fn pairs(file: &str, max_skew: &str) -> Res {
    let b = board(file)?;
    let limit = Length::parse(max_skew).ok_or("max-skew must look like 5mil or 0.1mm")?;
    let mut rows: Vec<_> = b
        .diff_pairs
        .iter()
        .filter_map(|p| {
            let (pos, neg) = (b.net(&p.positive)?, b.net(&p.negative)?);
            let skew = Length((pos.total_length().0 - neg.total_length().0).abs());
            let done = pos.is_complete()
                && neg.is_complete()
                && pos.routed_length.0 > 0
                && neg.routed_length.0 > 0;
            Some((p, pos, neg, skew, done))
        })
        .collect();
    // Finished pairs first, worst skew first: those are the ones to fix.
    rows.sort_by_key(|r| (!r.4, std::cmp::Reverse(r.3)));
    println!("Lengths include tracks, arcs and via barrels between the layers used.");
    println!();
    println!(
        "{:<30} {:>9} {:>9} {:>8} {:>7}  Status",
        "Pair", "P (mil)", "N (mil)", "Skew", "Vias"
    );
    let (mut over, mut open) = (0, 0);
    for (p, pos, neg, skew, done) in &rows {
        let status = if !done {
            open += 1;
            match (
                pos.routed_length.0 == 0 && neg.routed_length.0 == 0,
                pos.is_complete(),
                neg.is_complete(),
            ) {
                (true, _, _) => "not routed".to_string(),
                (_, false, false) => "P and N open".into(),
                (_, false, true) => "P open".into(),
                _ => "N open".into(),
            }
        } else if *skew > limit {
            over += 1;
            "OVER".into()
        } else {
            "ok".into()
        };
        println!(
            "{:<30} {:>9.1} {:>9.1} {:>8.1} {:>3}/{:<3}  {status}",
            clip(&p.name, 30),
            pos.total_length().mil(),
            neg.total_length().mil(),
            skew.mil(),
            pos.vias,
            neg.vias
        );
    }
    let done = rows.len() - open;
    println!();
    println!(
        "{} pairs: {} fully routed ({} within {:.1} mil, {} over), {} open or not routed",
        rows.len(),
        done,
        done - over,
        limit.mil(),
        over,
        open
    );
    Ok(())
}

fn fem(kind: &str, rest: &[&str]) -> Res {
    fem_run(kind, rest).map_err(|e| e.into())
}

fn fem_run(kind: &str, rest: &[&str]) -> Result<(), String> {
    use trace_core::fem3d;
    let mut kv: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut out = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if *a == "-o" {
            out = it.next().copied();
        } else if let Some((k, v)) = a.split_once('=') {
            kv.insert(k, v);
        } else {
            return Err(format!("expected key=value, got {a}"));
        }
    }
    let num = |k: &str, d: f64| -> Result<f64, String> {
        kv.get(k)
            .map(|v| v.parse::<f64>().map_err(|_| format!("{k}: not a number")))
            .unwrap_or(Ok(d))
    };
    let model = match kind {
        "waveguide" => {
            let slab = match kv.get("slab") {
                Some(v) => Some((
                    num("er", 2.2)?,
                    v.parse::<f64>().map_err(|_| "slab: not a number")?,
                )),
                None => None,
            };
            fem3d::waveguide(num("a", 22.86)?, num("b", 10.16)?, num("len", 20.0)?, slab)
        }
        "microstrip" | "stripline" => {
            let k = if kind == "microstrip" {
                fem3d::LineKind::Microstrip
            } else {
                fem3d::LineKind::Stripline
            };
            fem3d::line(
                k,
                num("w", 10.0)?,
                num("h", 5.0)?,
                num("er", 4.0)?,
                num("tand", 0.0)?,
                num("len", 300.0)?,
            )
        }
        _ => return Err(format!("unknown model {kind}")),
    };
    let f = num("f", 10.0)?;
    let sweep = match kv.get("sweep") {
        Some(s) => {
            let p: Vec<f64> = s
                .split(':')
                .map(|x| x.parse::<f64>().map_err(|_| "sweep: lo:hi:n"))
                .collect::<Result<_, _>>()?;
            if p.len() != 3 || p[2] < 1.0 {
                return Err("sweep: lo:hi:n".into());
            }
            let n = p[2] as usize;
            (0..n)
                .map(|i| {
                    if n == 1 {
                        p[0]
                    } else {
                        p[0] + (p[1] - p[0]) * i as f64 / (n - 1) as f64
                    }
                })
                .collect()
        }
        None => Vec::new(),
    };
    let zref = num("zref", 50.0)?;
    let setup = fem3d::Setup {
        f_adapt: f,
        sweep,
        delta_s: num("ds", 0.02)?,
        max_passes: num("passes", 10.0)? as usize,
        max_unknowns: num("maxdof", 400_000.0)? as usize,
        z_ref: if zref > 0.0 { Some(zref) } else { None },
        ..fem3d::Setup::default()
    };
    let r = fem3d::solve(&model, &setup, |l| {
        eprintln!("{l}");
        true
    })?;
    println!(
        "{} after {} passes: {} tetrahedra, {} unknowns",
        if r.converged {
            "converged"
        } else {
            "NOT converged"
        },
        r.passes.len(),
        r.tets,
        r.unknowns
    );
    for (i, p) in r.ports.iter().enumerate() {
        let inf = r.info[0][i];
        println!(
            "port {p}: Z0 {:.2} ohm, Er,eff {:.3}, beta {:.2} rad/m",
            inf.z0, inf.eeff, inf.beta
        );
    }
    let n = r.n();
    for (k, f) in r.f.iter().enumerate() {
        let row: Vec<String> = (0..n * n)
            .map(|i| {
                let s = r.s[k][i];
                format!(
                    "S{}{} {:.2} dB {:.1} deg",
                    i / n + 1,
                    i % n + 1,
                    20.0 * s.norm().max(1e-12).log10(),
                    s.arg().to_degrees()
                )
            })
            .collect();
        println!("{f} GHz  {}", row.join("  "));
    }
    if let Some(o) = out {
        std::fs::write(o, r.touchstone()).map_err(|e| format!("{o}: {e}"))?;
        println!("wrote {o}");
    }
    Ok(())
}

fn rules(file: &str) -> Res {
    use trace_core::rules;
    let b = board(file)?;
    for cat in rules::Category::ALL {
        let mut kinds: Vec<&str> = Vec::new();
        for r in &b.rules {
            if rules::category(&r.kind) == cat
                && !kinds.iter().any(|k| k.eq_ignore_ascii_case(&r.kind))
            {
                kinds.push(&r.kind);
            }
        }
        if kinds.is_empty() {
            continue;
        }
        println!("{}", cat.label());
        for kind in kinds {
            let idx = rules::of_kind(&b.rules, kind);
            let label = rules::info(kind).map_or(kind, |k| k.label);
            println!("  {label} ({})", idx.len());
            for i in idx {
                let r = &b.rules[i];
                let state = if r.enabled { "" } else { "  [disabled]" };
                let scope = r
                    .scope
                    .iter()
                    .take(if rules::info(kind).is_some_and(|k| k.binary) {
                        2
                    } else {
                        1
                    })
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("  |  ");
                println!(
                    "    {:>2} {:<30} {:<44} {}{state}",
                    r.priority,
                    clip(&r.name, 30),
                    clip(&scope, 44),
                    rules::summary(r)
                );
            }
        }
    }
    Ok(())
}

/// Scopes that do not read, terms Trace does not judge, and rule problems.
fn rules_check(file: &str) -> Res {
    use trace_core::rules;
    let b = board(file)?;
    let p = rules::problems(&b.rules);
    for (i, what) in &p {
        println!("{} ({}): {what}", b.rules[*i].name, b.rules[*i].kind);
    }
    let mut unknown: Vec<String> = Vec::new();
    for r in &b.rules {
        for s in &r.scope {
            if let Ok(u) = rules::check_scope(s) {
                for w in u {
                    if !unknown.contains(&w) {
                        unknown.push(w);
                    }
                }
            }
        }
    }
    if !unknown.is_empty() {
        println!(
            "Scope terms kept for Altium but not judged by Trace: {}",
            unknown.join(", ")
        );
    }
    println!("{} rules, {} problems", b.rules.len(), p.len());
    Ok(())
}

/// Changes a board's rules and writes it as a PcbDoc (over the original's
/// records when it is one).
fn rules_edit(
    file: &str,
    out: &str,
    f: impl FnOnce(&mut Board) -> Result<String, Box<dyn std::error::Error>>,
) -> Res {
    let mut b = board(file)?;
    let said = f(&mut b)?;
    println!("{said}");
    let name = std::path::Path::new(out)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if file.to_ascii_lowercase().ends_with(".pcbdoc") {
        let (bytes, report) = trace_export::to_altium_over(std::path::Path::new(file), &b, &name)?;
        std::fs::write(out, bytes)?;
        println!("wrote {out} ({})", report.summary());
        for n in &report.notes {
            println!("  note: {n}");
        }
    } else {
        std::fs::write(out, trace_export::to_altium(&b, &name)?)?;
        println!("wrote {out}");
    }
    let back = trace_altium::read_pcbdoc(out)?;
    println!("read back: {} rules", back.rules.len());
    Ok(())
}

/// The rule of each kind that applies to one net.
fn rules_net(file: &str, net: &str) -> Res {
    use trace_core::rules;
    let b = board(file)?;
    let n = b
        .nets
        .iter()
        .position(|x| x.name.eq_ignore_ascii_case(net))
        .ok_or_else(|| format!("no net {net}"))?;
    let r = rules::Resolver::new(&b);
    println!(
        "Rules that apply to {} (highest priority that takes it):",
        b.nets[n].name
    );
    for (kind, name, what) in r.report(n as u32) {
        let label = rules::info(&kind).map_or(kind.as_str(), |k| k.label);
        println!("  {label:<30} {name:<30} {what}");
    }
    let copper: Vec<String> = b.stackup.copper_layers().map(|l| l.name.clone()).collect();
    for (l, name) in copper.iter().enumerate() {
        let w = r.width(n as u32, l as u8).map(|w| format!("{:.3}", w.0[1]));
        let p = r
            .pair(n as u32, l as u8)
            .map(|((w, g), _)| format!("pair {w:.3}/{g:.3}"));
        if w.is_some() || p.is_some() {
            println!(
                "    {name:<16} width {}  {}",
                w.unwrap_or_else(|| "-".into()),
                p.unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn rules_params(file: &str) -> Res {
    let b = board(file)?;
    for r in &b.rules {
        println!(
            "{} | {} | en={} pri={} | {:?}",
            r.kind, r.name, r.enabled, r.priority, r.scope
        );
        for (k, v) in &r.params {
            println!("    {k}={v}");
        }
    }
    Ok(())
}

/// The field solver on every copper layer.
fn field_layers(file: &str) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("field needs a board".into());
    };
    let geo = b.layer_geometry();
    let t0 = std::time::Instant::now();
    println!(
        "{:<14} {:>15} {:>12} {:>15} {:>7} {:>7} {:>8}",
        "layer", "50R w field/ipc", "Z0 at w ipc", "100R w/s field", "ps/in", "dB/in", "dB/in"
    );
    println!(
        "{:<14} {:>15} {:>12} {:>15} {:>7} {:>7} {:>8}",
        "", "mil", "ohm", "mil", "", "1 GHz", "10 GHz"
    );
    for g in &geo {
        let Some(w) = g.width_for_field(50.0) else {
            println!("{:<14} -", g.name);
            continue;
        };
        let wd = w * 0.75;
        let s = g.gap_for_field(wd, 100.0);
        let l = trace_core::field::single(g, w).ok_or("solver failed")?;
        let td = g.tan_d.unwrap_or(0.02);
        println!(
            "{:<14} {:>7.2}/{:<7} {:>12.1} {:>15} {:>7.0} {:>7.3} {:>8.3}",
            g.name,
            w,
            g.width_for(50.0)
                .map(|x| format!("{x:.2}"))
                .unwrap_or_else(|| "-".into()),
            g.z0(w),
            s.map(|s| format!("{wd:.2}/{s:.2}"))
                .unwrap_or_else(|| "-".into()),
            l.ps_per_in(),
            l.db_per_in(1e9, td, g.rough_um),
            l.db_per_in(1e10, td, g.rough_um)
        );
    }
    println!(
        "\n(Df {} where the stack-up gives none; solved in {:.1} s)",
        0.02,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// One cross-section from the field solver.
fn field_one(file: &str, layer: &str, w: &str, rest: &[&str]) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("field needs a board".into());
    };
    let geo = b.layer_geometry();
    let g = layer
        .parse::<usize>()
        .ok()
        .and_then(|n| geo.get(n.checked_sub(1)?))
        .or_else(|| geo.iter().find(|g| g.name.eq_ignore_ascii_case(layer)))
        .ok_or_else(|| {
            format!(
                "no copper layer {layer}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let len = |v: &str| -> Result<f64, String> {
        let (n, k) = match v.strip_suffix("mm") {
            Some(n) => (n, 1.0 / 0.0254),
            None => (v.trim_end_matches("mil"), 1.0),
        };
        n.parse::<f64>()
            .map(|x| x * k)
            .map_err(|_| format!("not a length: {v}"))
    };
    let w = len(w)?;
    // `--ground G`: ground copper G beside the track on its own layer.
    let mut side = None;
    let mut args: Vec<&str> = Vec::new();
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        if a == "--ground" {
            side = Some(len(it.next().ok_or("--ground needs a gap")?)?);
        } else {
            args.push(a);
        }
    }
    let rest = args.as_slice();
    let (s, out) = match rest {
        [] => (None, None),
        [x] if x.ends_with(".svg") => (None, Some(*x)),
        [x] => (Some(len(x)?), None),
        [x, o] => (Some(len(x)?), Some(*o)),
        _ => return Err("too many arguments".into()),
    };
    let fmt = |v: Option<f64>| v.map_or("-".into(), |x| format!("{x:.2}"));
    println!(
        "{}: copper {:.2} mil, {} mil above, {} mil below, Er {:.2}{}",
        g.name,
        g.t,
        fmt(g.above),
        fmt(g.below),
        g.er,
        g.mask
            .map(|m| format!(", solder mask {:.2} mil Er {:.2}", m[0], m[1]))
            .unwrap_or_default()
    );
    for (side, sl) in [("above", &g.slabs_above), ("below", &g.slabs_below)] {
        if !sl.is_empty() {
            let v: Vec<String> = sl
                .iter()
                .map(|x| {
                    format!(
                        "{:.2} mil Er {:.2}{}",
                        x[0],
                        x[1],
                        if x[2] >= 0.0 {
                            format!(" Df {:.4}", x[2])
                        } else {
                            String::new()
                        }
                    )
                })
                .collect();
            println!("  {side}: {}", v.join(", "));
        }
    }
    let t0 = std::time::Instant::now();
    let td = g.tan_d.unwrap_or(0.02);
    let (pic, title) = match s {
        None => {
            let (l, pic) = trace_core::field::single_picture(g, w, side).ok_or("solver failed")?;
            if let Some(gap) = side {
                println!("ground beside the track {gap:.2} mil from each edge (coplanar)");
            }
            println!(
                "track {w:.2} mil: Z0 {:.2} ohm (microstrip/stripline formula {:.2}), Er eff {:.3}, {:.1} ps/in",
                l.z0,
                g.z0(w),
                l.eeff,
                l.ps_per_in()
            );
            println!(
                "  L {:.2} nH/in, C {:.3} pF/in, R dc {:.3} ohm/in, R 1 GHz {:.3} ohm/in (smooth)",
                l.l * 0.0254 * 1e9,
                l.c * 0.0254 * 1e12,
                l.r_dc * 0.0254,
                l.r_ghz * 0.0254
            );
            println!(
                "  loss {:.3} dB/in at 1 GHz, {:.3} at 10 GHz (Df {:.4}, roughness {})",
                l.db_per_in(1e9, td, g.rough_um),
                l.db_per_in(1e10, td, g.rough_um),
                if l.fill_given > 0.0 {
                    l.tan_eff / l.fill_given
                } else {
                    td
                },
                g.rough_um.map_or("none".into(), |r| format!("{r} um"))
            );
            (pic, format!("{} · {w:.2} mil · Z0 {:.1} Ω", g.name, l.z0))
        }
        Some(s) => {
            let (p, pic) = trace_core::field::pair_picture(g, w, s, side).ok_or("solver failed")?;
            println!(
                "pair {w:.2}/{s:.2} mil: Zdiff {:.2} ohm (formula {:.2}), Zcomm {:.2} ohm",
                p.zdiff,
                g.zdiff(w, s),
                p.zcomm
            );
            println!(
                "  Z odd {:.2}, Z even {:.2} ohm; Er eff odd {:.3}, even {:.3}; single Z0 {:.2} ohm",
                p.z_odd, p.z_even, p.e_odd, p.e_even, p.single.z0
            );
            println!(
                "  coupling: near end {:.2} %, far end {:.3} ps/in of rise time",
                100.0 * p.kb,
                // Far-end crosstalk = Kf · delay · dV/dt.
                p.kf * p.single.ps_per_in()
            );
            let alpha_c = p.r_odd_ghz / (2.0 * p.z_odd);
            let alpha_d = PI_ * 1e9 * p.e_odd.sqrt() / 2.998e8
                * (p.tan_odd + (1.0 - p.single.fill_given) * td * p.single.fill);
            println!(
                "  differential loss {:.3} dB/in at 1 GHz (smooth copper)",
                8.686 * (alpha_c + alpha_d) * 0.0254
            );
            (
                pic,
                format!(
                    "{} · {w:.2}/{s:.2} mil · Zdiff {:.1} Ω (odd mode)",
                    g.name, p.zdiff
                ),
            )
        }
    };
    println!("  solved in {:.2} s", t0.elapsed().as_secs_f64());
    if let Some(out) = out {
        std::fs::write(out, pic.svg(900.0, &title))?;
        println!("Wrote {out}");
    }
    Ok(())
}

const PI_: f64 = std::f64::consts::PI;

fn impedance(file: &str, field: bool) -> Res {
    let Design::Board(b) = load(file)? else {
        return Err("impedance needs a board".into());
    };
    let geo = b.layer_geometry();
    println!(
        "{:<14} {:>6} {:>8} {:>8} {:>5}  {:>9} {:>9} {:>9}",
        "layer", "Cu mil", "above", "below", "Er", "50R w", "100R w/s", "ps/in"
    );
    let f = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".into());
    for g in &geo {
        let w50 = g.width_for(50.0);
        let wd = g.width_for(50.0).map(|w| w * 0.75);
        let s100 = wd.and_then(|w| g.gap_for(w, 100.0));
        println!(
            "{:<14} {:>6.2} {:>8} {:>8} {:>5.2}  {:>9} {:>9} {:>9.0}",
            g.name,
            g.t,
            f(g.above),
            f(g.below),
            g.er,
            f(w50),
            match (wd, s100) {
                (Some(w), Some(s)) => format!("{w:.1}/{s:.1}"),
                _ => "-".into(),
            },
            g.delay_ps_per_in(w50.unwrap_or(5.0))
        );
    }
    let t0 = std::time::Instant::now();
    let rows = b.net_impedance_model(&geo, field);
    if field {
        println!("\nfield solver: {:.1} s", t0.elapsed().as_secs_f64());
        let mut cpw: Vec<_> = rows
            .iter()
            .filter(|r| r.side != [None, None] && r.zdiff.is_none())
            .collect();
        cpw.sort_by(|a, b| b.length.total_cmp(&a.length));
        if !cpw.is_empty() {
            println!("\ntracks with pour beside them (coplanar), longest first");
            println!(
                "{:<28} {:<10} {:>6} {:>13} {:>7} {:>7} {:>8}",
                "net", "layer", "w", "pour gaps", "Z0", "no pour", "length"
            );
            for r in cpw.iter().take(25) {
                let g = &geo[r.layer as usize];
                println!(
                    "{:<28} {:<10} {:>6.2} {:>13} {:>7.1} {:>7.1} {:>8.0}",
                    clip(&b.nets[r.net as usize].name, 28),
                    g.name,
                    r.width,
                    r.side
                        .map(|s| s.map_or("-".into(), |x| format!("{x:.2}")))
                        .join(" / "),
                    r.z0,
                    g.z0_field(r.width),
                    r.length
                );
            }
        }
    }
    let mut pairs: Vec<_> = rows.iter().filter(|r| r.zdiff.is_some()).collect();
    pairs.sort_by(|a, b| b.length.total_cmp(&a.length));
    if !pairs.is_empty() {
        println!("\npair nets (longest runs first)");
        println!(
            "{:<28} {:<10} {:>6} {:>6} {:>7} {:>7} {:>8}",
            "net", "layer", "w", "gap", "Z0", "Zdiff", "length"
        );
        for r in pairs.iter().take(40) {
            println!(
                "{:<28} {:<10} {:>6.2} {:>6.2} {:>7.1} {:>7.1} {:>8.0}",
                clip(&b.nets[r.net as usize].name, 28),
                geo.get(r.layer as usize)
                    .map(|g| g.name.as_str())
                    .unwrap_or("?"),
                r.width,
                r.gap.unwrap_or(0.0),
                r.z0,
                r.zdiff.unwrap_or(0.0),
                r.length
            );
        }
    }
    Ok(())
}

fn drc(file: &str, show: usize) -> Res {
    let b = board(file)?;
    let t = std::time::Instant::now();
    let found = trace_core::drc::check(&b, 5000);
    let took = t.elapsed();
    let mut by_kind: std::collections::BTreeMap<String, Vec<&trace_core::drc::Violation>> =
        Default::default();
    for v in &found {
        by_kind.entry(format!("{:?}", v.kind)).or_default().push(v);
    }
    for (kind, list) in &by_kind {
        println!("{kind}: {}", list.len());
        for v in list.iter().take(show) {
            let layer = v
                .layer
                .and_then(|l| {
                    b.stackup
                        .layers
                        .iter()
                        .filter(|x| x.kind == trace_core::LayerKind::Copper)
                        .nth(l as usize)
                })
                .map(|l| l.name.clone())
                .unwrap_or_default();
            println!(
                "  {:>9.1} {:>9.1}  {:<10} {}",
                v.at[0],
                v.at[1],
                layer,
                trace_core::drc::describe(&b, v)
            );
        }
        if list.len() > show {
            println!("  ... {} more (--all lists them)", list.len() - show);
        }
    }
    println!("{} violations in {:.2} s", found.len(), took.as_secs_f64());
    Ok(())
}

fn panel(file: &str, grid: &str, out: &str, opts: &[&str]) -> Res {
    use trace_core::panel::{panelize, PanelSpec, Separation};
    let b = board(file)?;
    let (c, r) = grid
        .to_ascii_lowercase()
        .split_once('x')
        .and_then(|(c, r)| Some((c.trim().parse::<u32>().ok()?, r.trim().parse::<u32>().ok()?)))
        .filter(|&(c, r)| c >= 1 && r >= 1 && c * r <= 400)
        .ok_or("the grid is columns x rows, such as 3x2")?;
    let mut spec = PanelSpec {
        cols: c,
        rows: r,
        ..Default::default()
    };
    let len = |v: Option<&&str>| -> Result<f64, String> {
        v.and_then(|v| Length::parse(v))
            .map(|l| l.mil())
            .ok_or_else(|| "a length such as 2mm or 80mil".to_string())
    };
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            "--gap" => {
                spec.gap = len(opts.get(i + 1))?;
                i += 1;
            }
            "--rail" => {
                spec.rail = len(opts.get(i + 1))?;
                i += 1;
            }
            "--tabs" => {
                spec.tabs = opts
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--tabs takes a number")?;
                i += 1;
            }
            "--vscore" => spec.separation = Separation::VScore,
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let (p, made) = panelize(&b, &spec)?;
    trace_core::file::save(&p, Path::new(out))?;
    println!(
        "{c} x {r} panel, {:.1} x {:.1} mm: {} tabs, {} mouse bite holes{}; saved {out}",
        made.size[0] * 0.0254,
        made.size[1] * 0.0254,
        made.tabs,
        made.bites,
        made.vscore_layer
            .map(|m| format!(", V-score lines on mechanical layer {m}"))
            .unwrap_or_default()
    );
    Ok(())
}

fn stubs(file: &str, out: Option<&str>) -> Res {
    use trace_core::backdrill;
    let mut b = board(file)?;
    let list = backdrill::stubs(&b);
    let ruled: Vec<_> = list.iter().filter(|s| s.limit.is_some()).collect();
    let name = |n: u32| {
        b.nets
            .get(n as usize)
            .map(|n| n.name.as_str())
            .unwrap_or("?")
    };
    let layer = |l: u8| {
        b.stackup
            .copper_layers()
            .nth(l as usize)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("L{}", l + 1))
    };
    println!(
        "{} vias joined by their net, {} on nets with a stub rule",
        list.len(),
        ruled.len()
    );
    let mut long: Vec<_> = ruled
        .iter()
        .filter(|s| {
            let max = s.limit.as_ref().map(|l| l.0).unwrap_or(f64::MAX);
            s.top_left.max(s.bottom_left) > max
        })
        .collect();
    long.sort_by(|a, b| (b.top_left.max(b.bottom_left)).total_cmp(&a.top_left.max(a.bottom_left)));
    println!("{} with a stub longer than allowed:", long.len());
    println!(
        "  {:<28} {:>9} {:>9}  {:<22} {:>7} {:>7}  rule",
        "net", "x", "y", "joins", "above", "below"
    );
    for s in &long {
        let v = &b.geometry.vias[s.via];
        let joins: Vec<String> = s.joins.iter().map(|&l| layer(l)).collect();
        let (max, rule) = s.limit.clone().unwrap_or_default();
        println!(
            "  {:<28} {:>9.1} {:>9.1}  {:<22} {:>7.1} {:>7.1}  {rule} ({max:.1} mil)",
            name(s.net),
            v.at.x.mil(),
            v.at.y.mil(),
            joins.join(", "),
            s.top_left,
            s.bottom_left
        );
    }
    if let Some(out) = out {
        let edits = backdrill::plan(&b);
        for e in &edits {
            e.apply(&mut b);
        }
        let n = b
            .geometry
            .vias
            .iter()
            .filter(|v| v.fab.back_top.is_some() || v.fab.back_bottom.is_some())
            .count();
        // Pours keep clear of the backdrills where they drill through.
        if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
            e.apply(&mut b);
        }
        trace_core::file::save(&b, Path::new(out))?;
        println!(
            "Backdrilled {} vias ({n} in all); saved {out}",
            edits.len() / 2
        );
    }
    Ok(())
}

fn match_nets(file: &str, nets: &str, rest: &[&str]) -> Res {
    let mut b = board(file)?;
    let mut out = None;
    let (mut target, mut amp) = (None, 40.0);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let len = |v: Option<&&str>| -> Result<f64, Box<dyn std::error::Error>> {
            let v = v.ok_or(format!("{a} needs a length"))?;
            Ok(Length::parse(v).ok_or(format!("not a length: {v}"))?.mil())
        };
        match a {
            "--target" => target = Some(len(it.next())?),
            "--amp" => amp = len(it.next())?,
            _ if out.is_none() && !a.starts_with("--") => out = Some(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let mut ids = Vec::new();
    let id = |name: &str| b.nets.iter().position(|n| n.name == name).map(|i| i as u32);
    for name in nets.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        // A differential pair's name stands for both its nets.
        if let Some(d) = b.diff_pairs.iter().find(|d| d.name == name) {
            for leg in [&d.positive, &d.negative] {
                ids.push(id(leg).ok_or(format!("no net {leg}"))?);
            }
            continue;
        }
        ids.push(id(name).ok_or(format!("no net {name}"))?);
    }
    let clearance = trace_core::route::board_clearance(&b);
    let m = trace_core::bus::match_lengths(&b, &ids, target, amp, None, clearance);
    for &(p, n) in &m.pairs {
        println!(
            "  pair {} / {}: tuned together, mean {:.1} mil, skew {:.1} mil",
            b.nets[p as usize].name,
            b.nets[n as usize].name,
            m.length_of(p),
            m.nets.iter().find(|t| t.net == p).map_or(0.0, |t| t.after)
                - m.nets.iter().find(|t| t.net == n).map_or(0.0, |t| t.after)
        );
    }
    println!("Target {:.1} mil", m.target);
    for t in &m.nets {
        let short = m.target - m.length_of(t.net);
        println!(
            "  {:<28} {:>9.1} -> {:>9.1}{}",
            b.nets[t.net as usize].name,
            t.before,
            t.after,
            if short > 1.0 {
                format!("  ({short:.1} mil short: no room for meanders)")
            } else {
                String::new()
            }
        );
    }
    if let Some(out) = out {
        let edits = m.edits;
        for e in &edits {
            e.apply(&mut b);
        }
        if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
            e.apply(&mut b);
        }
        trace_core::connectivity::analyse(&mut b);
        trace_core::file::save(&b, Path::new(out))?;
        println!("Saved {out}");
    }
    Ok(())
}

fn sim(file: &str, from: &str, to: &str, rest: &[&str]) -> Res {
    use trace_core::ibis::{self, Corner};
    use trace_core::sim::{self, End, Pins, Setup, Term};
    let b = board(file)?;
    let mut opt: std::collections::HashMap<&str, &str> = Default::default();
    let mut out = None;
    let mut single = false;
    let (mut lossless, mut quiet, mut formulas) = (false, false, false);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--single" => single = true,
            "--lossless" => lossless = true,
            "--no-xtalk" => quiet = true,
            "--formulas" => formulas = true,
            _ if a.starts_with("--") => {
                let v = it.next().ok_or(format!("{a} needs a value"))?;
                opt.insert(a, v);
            }
            _ => out = Some(a),
        }
    }
    let part = |s: &str| -> Result<(String, String), String> {
        let (d, p) = s.split_once('.').ok_or(format!(
            "{s}: give the pin as designator.pin, such as U1.A5"
        ))?;
        Ok((d.to_string(), p.to_string()))
    };
    let ((dd, dp), (rd, rp)) = (part(from)?, part(to)?);
    let pad = |d: &str, p: &str| sim::find_pad(&b, d, p).ok_or(format!("no pad {p} on {d}"));
    let (dpad, rpad) = (pad(&dd, &dp)?, pad(&rd, &rp)?);
    let ibs = opt.get("--ibis").ok_or("--ibis file.ibs is needed")?;
    let fd = ibis::load(Path::new(ibs))?;
    let fr = match opt.get("--rx-ibis") {
        Some(p) => ibis::load(Path::new(p))?,
        None => fd.clone(),
    };
    let (dm, dpkg) = sim::pin_model(&fd, &dp, opt.get("--model").copied(), true)?;
    let (rm, rpkg) = sim::pin_model(&fr, &rp, opt.get("--rx-model").copied(), false)?;
    let corner = match opt.get("--corner").copied().unwrap_or("typ") {
        "min" | "slow" => Corner::Min,
        "max" | "fast" => Corner::Max,
        _ => Corner::Typ,
    };
    let rate = match opt.get("--rate") {
        Some(r) => ibis::num(r).ok_or(format!("not a rate: {r}"))?,
        None => 1e9,
    };
    let term = match opt.get("--term").copied() {
        None | Some("none") => Term::None,
        Some(t) if t.ends_with("diff") => Term::Across(
            ibis::num(t.trim_end_matches("diff")).ok_or(format!("not a resistance: {t}"))?,
        ),
        Some(t) => {
            let (r, v) = t.split_once('@').unwrap_or((t, "0"));
            Term::ToRef {
                r: ibis::num(r).ok_or(format!("not a resistance: {r}"))?,
                v: ibis::num(v).ok_or(format!("not a voltage: {v}"))?,
            }
        }
    };
    let (dn, rn) = if single {
        (None, None)
    } else {
        (sim::partner_pad(&b, dpad), sim::partner_pad(&b, rpad))
    };
    let pins = Pins {
        driver: dpad,
        receiver: rpad,
        driver_n: dn,
        receiver_n: rn.filter(|_| dn.is_some()),
    };
    let setup = Setup {
        corner,
        rate,
        bits: opt
            .get("--bits")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(160),
        term,
        loss: if lossless {
            None
        } else {
            let mut l = sim::Loss::default();
            if let Some(d) = opt.get("--df") {
                l.tan_d = d.parse().map_err(|_| format!("not a loss tangent: {d}"))?;
                l.stackup_df = false;
            }
            if let Some(r) = opt.get("--rough") {
                let um = ibis::num(r.trim_end_matches("um").trim_end_matches('u'))
                    .ok_or(format!("not a roughness: {r}"))?;
                l.rough_um = um;
                l.stackup_rough = false;
            }
            Some(l)
        },
        crosstalk: !quiet,
        field: !formulas,
    };
    let t0 = std::time::Instant::now();
    let run = sim::simulate(
        &b,
        pins,
        End {
            model: dm,
            pkg: dpkg,
        },
        End {
            model: rm,
            pkg: rpkg,
        },
        &setup,
    )?;
    let net = |p: usize| {
        b.geometry.pads[p]
            .net
            .map_or("?".to_string(), |n| b.nets[n as usize].name.clone())
    };
    let what = match pins.driver_n {
        Some(n) => format!("pair {} / {}", net(dpad), net(n)),
        None => net(dpad),
    };
    println!(
        "{what}: {} ({}) to {} ({}), {} at {:.3} Gb/s",
        from,
        dm.name,
        to,
        rm.name,
        corner.label(),
        rate / 1e9
    );
    println!(
        "  route delay {:.1} ps, driver switching {:.0}/{:.0} ps, step {:.2} ps ({:.2} s)",
        run.delay * 1e12,
        run.switch.0 * 1e12,
        run.switch.1 * 1e12,
        run.dt * 1e12,
        t0.elapsed().as_secs_f64()
    );
    let e = &run.eye;
    println!(
        "  eye height {:.3} V (ones from {:.3} V, zeros to {:.3} V), width {:.1} ps = {:.0} % of {:.1} ps",
        e.height,
        e.high,
        e.low,
        e.width * 1e12,
        e.width / e.ui * 100.0,
        e.ui * 1e12
    );
    match &setup.loss {
        Some(_) => println!(
            "  loss {:.2} dB at {:.2} GHz (Nyquist); {}",
            run.loss_db,
            rate / 2e9,
            run.materials
        ),
        None => println!("  lossless lines"),
    }
    if let Some(q) = &run.quiet {
        println!(
            "  crosstalk: peak {:.1} mV at the receiver; without it the eye is {:.3} V, {:.0} % UI",
            run.noise * 1e3,
            q.height,
            q.width / q.ui * 100.0
        );
    }
    for a in &run.aggressors {
        println!(
            "    aggressor {} from {}{}: {:.0} mil beside, Kb {:.3}",
            a.net,
            a.from,
            if a.inverted { " (inverted)" } else { "" },
            a.coupled,
            a.kb
        );
    }
    for n in &run.notes {
        println!("  {n}");
    }
    if let Some(out) = out {
        std::fs::write(out, sim::svg(&run, &format!("{what}: {from} to {to}")))?;
        println!("Saved {out}");
    }
    Ok(())
}

/// Parses a current such as `5A`, `500mA` or `2.5`.
fn amps(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase();
    if let Some(v) = t.strip_suffix("ma") {
        return v.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    t.strip_suffix('a').unwrap_or(&t).trim().parse().ok()
}

/// 100k, 1G, 10m, 20n, 4.7u: a number with an SI prefix.
fn si(v: &str) -> Option<f64> {
    let t = v
        .trim()
        .trim_end_matches(['H', 'F', 'z', 'Ω', 'A', 'V'])
        .trim_end_matches('H');
    let (num, k) = match t.chars().last()? {
        'G' => (&t[..t.len() - 1], 1e9),
        'M' => (&t[..t.len() - 1], 1e6),
        'k' | 'K' => (&t[..t.len() - 1], 1e3),
        'm' => (&t[..t.len() - 1], 1e-3),
        'u' => (&t[..t.len() - 1], 1e-6),
        'n' => (&t[..t.len() - 1], 1e-9),
        'p' => (&t[..t.len() - 1], 1e-12),
        _ => (t, 1.0),
    };
    num.trim().parse::<f64>().ok().map(|x| x * k)
}

fn pdn_cmd(file: &str, net: &str, rest: &[&str]) -> Res {
    use trace_core::pdn::{self, farads_text, hz_text, ohm_text};
    let b = board(file)?;
    let find_net = |n: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(b.nets
            .iter()
            .position(|x| x.name == n || x.name.eq_ignore_ascii_case(n))
            .ok_or(format!("no net {n}"))? as u32)
    };
    let part = |d: &str| -> Result<usize, Box<dyn std::error::Error>> {
        Ok(b.components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?)
    };
    let mut s = pdn::Setup::new(find_net(net)?);
    let (mut opt, mut csv, mut svg) = (false, None, None);
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        let mut num = |what: &str| -> Result<f64, Box<dyn std::error::Error>> {
            let v = val()?;
            Ok(si(v).ok_or(format!("{a}: not {what}: {v}"))?)
        };
        match a {
            "--ground" => s.ground = Some(find_net(val()?)?),
            "--volts" => s.volts = Some(num("a voltage")?),
            "--ripple" => s.ripple_pct = num("a percentage")?,
            "--amps" => s.amps = num("a current")?,
            "--target" => s.target = Some(num("an impedance")?),
            "--band" => s.target_fmax = num("a frequency")?,
            "--fmin" => s.fmin = num("a frequency")?,
            "--fmax" => s.fmax = num("a frequency")?,
            "--points" => s.points = num("a count")? as usize,
            "--cell" => s.cell_mil = num("a length in mil")?,
            "--vrm-r" => s.vrm_r = num("a resistance")?,
            "--vrm-l" => s.vrm_l = num("an inductance")?,
            "--via" => s.via_drill_mil = num("a drill in mil")?,
            "--vrm" => s.vrm = Some(part(val()?)?),
            "--loads" => {
                s.loads = val()?
                    .split(',')
                    .map(|d| part(d.trim()))
                    .collect::<Result<_, _>>()?
            }
            "--optimise" | "--optimize" => opt = true,
            "--no-assume" => s.assume = false,
            "--add" => s.spares = num("a count")? as usize,
            "--die-c" => {
                let c = num("a capacitance")?;
                s.die = Some(pdn::OnChip {
                    farads: c,
                    ..s.die.unwrap_or(pdn::OnChip::new(c))
                })
            }
            "--die-r" => {
                let r = num("a resistance")?;
                let d = s.die.get_or_insert(pdn::OnChip::new(0.0));
                d.esr = r;
            }
            "--pkg-l" => {
                let l = num("an inductance")?;
                let d = s.die.get_or_insert(pdn::OnChip::new(0.0));
                d.pkg_l = l;
            }
            "--csv" => csv = Some(val()?.to_string()),
            a if a.to_ascii_lowercase().ends_with(".svg") => svg = Some(a.to_string()),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    if s.die.is_some_and(|d| d.farads <= 0.0) {
        return Err("--die-r and --pkg-l need --die-c".into());
    }
    if !(s.fmin > 0.0 && s.fmax > s.fmin) {
        return Err("the sweep needs 0 < fmin < fmax".into());
    }
    let t = std::time::Instant::now();
    let m = pdn::model(&b, &s)?;
    let des = |i: usize| b.components[i].designator.as_str();
    let c = &m.cavity;
    println!(
        "{}: plane pair {} over {} ({}), {:.2} mil, εr {:.2}, tanδ {:.3}; {:.2} in², {:.2} nF; {} cells of {:.0} mil; solved in {:.1} s",
        b.nets[s.net as usize].name,
        c.power_layer,
        c.ground_layer,
        c.ground_net,
        c.d_mil,
        c.er,
        c.tan_d,
        c.area_in2,
        c.farads * 1e9,
        c.nodes,
        c.cell_mil,
        t.elapsed().as_secs_f64()
    );
    let mut kinds: std::collections::BTreeMap<(i64, &str), usize> = Default::default();
    for d in m.decaps.iter().filter(|d| !d.spare) {
        *kinds
            .entry(((d.model.farads * 1e15).round() as i64, d.package.label()))
            .or_default() += 1;
    }
    let list: Vec<String> = kinds
        .iter()
        .rev()
        .map(|((f, p), n)| format!("{n} × {} {p}", farads_text(*f as f64 / 1e15)))
        .collect();
    println!(
        "Capacitors: {} ({}); regulator {}",
        m.decaps.iter().filter(|d| !d.spare).count(),
        if list.is_empty() {
            "none".into()
        } else {
            list.join(", ")
        },
        m.vrm.map_or("none".to_string(), |v| if m.vrm_bead {
            format!("fed by {} (bead)", des(v))
        } else {
            des(v).to_string()
        })
    );
    for n in &m.notes {
        println!("  note: {n}");
    }
    match m.target {
        Some(t) => println!("Target: {} up to {}", ohm_text(t), hz_text(m.band)),
        None => println!("Target: none (give --volts and --amps, or --target)"),
    }
    let curves = m.evaluate(&m.as_built());
    // Decade points and the worst.
    let mut marks: Vec<usize> = Vec::new();
    let mut f = 10f64.powf(s.fmin.log10().ceil());
    while f <= s.fmax * 1.0001 {
        if let Some(i) = m.freqs.iter().position(|&x| x >= f * 0.999) {
            marks.push(i);
        }
        f *= 10.0;
    }
    print!("{:12}", "|Z| at");
    for &i in &marks {
        print!("{:>12}", hz_text(m.freqs[i]));
    }
    println!();
    for (l, c) in curves.iter().enumerate() {
        print!("{:12}", des(m.loads[l]));
        for &i in &marks {
            print!("{:>12}", ohm_text(c[i]));
        }
        let (fi, z) = c
            .iter()
            .enumerate()
            .fold((0, 0.0), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
        println!("   peak {} at {}", ohm_text(z), hz_text(m.freqs[fi]));
    }
    if let Some((r, f, l)) = m.worst(&curves) {
        let bands: Vec<String> = m
            .over(&curves)
            .iter()
            .map(|(a, b)| trace_core::pdn::band_text(*a, *b))
            .collect();
        if r <= 1.0 {
            println!(
                "Under the target everywhere (worst {r:.2}× at {} at {}).",
                hz_text(f),
                des(m.loads[l])
            );
        } else {
            println!(
                "Over the target: worst {r:.2}× at {} at {}; over at {}",
                hz_text(f),
                des(m.loads[l]),
                bands.join(", ")
            );
        }
    }
    if let Some(path) = &csv {
        let mut out = String::from("freq_hz");
        for &l in &m.loads {
            out.push_str(&format!(",{}", des(l)));
        }
        out.push_str(",target\n");
        for (fi, f) in m.freqs.iter().enumerate() {
            out.push_str(&format!("{f}"));
            for c in &curves {
                out.push_str(&format!(",{}", c[fi]));
            }
            out.push_str(&format!(",{}\n", m.target.unwrap_or(f64::NAN)));
        }
        std::fs::write(path, out)?;
        println!("wrote {path}");
    }
    let title = format!("{} impedance", b.nets[s.net as usize].name);
    if !opt {
        if let Some(path) = &svg {
            std::fs::write(path, pdn::svg(&m, &curves, None, &title))?;
            println!("wrote {path}");
        }
    }
    if opt {
        let t = std::time::Instant::now();
        let plan = pdn::optimise(&m, &m.as_built());
        if let Some(path) = &svg {
            let tried = m.evaluate(&plan.caps);
            std::fs::write(path, pdn::svg(&m, &curves, Some(&tried), &title))?;
            println!("wrote {path}");
        }
        println!(
            "Optimised in {:.1} s: {}",
            t.elapsed().as_secs_f64(),
            plan.summary()
        );
        for ch in &plan.changes {
            let d = &m.decaps[ch.decap];
            let text =
                |c: Option<pdn::CapModel>| c.map_or("off".to_string(), |c| farads_text(c.farads));
            println!(
                "  {:8} {:5}  {} -> {}",
                d.designator,
                d.package.label(),
                text(ch.from),
                text(ch.to)
            );
        }
    }
    Ok(())
}

fn ir(file: &str, net: &str, rest: &[&str]) -> Res {
    use trace_core::pi;
    let b = board(file)?;
    let id = b
        .nets
        .iter()
        .position(|n| n.name == net || n.name.eq_ignore_ascii_case(net))
        .ok_or(format!("no net {net}"))? as u32;
    let part = |d: &str| -> Result<usize, Box<dyn std::error::Error>> {
        Ok(b.components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?)
    };
    let (mut source, mut volts, mut total, mut plating) = (None, 1.0, 1.0, 0.8);
    let mut loads: Vec<(usize, f64)> = Vec::new();
    let mut out = None;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--source" => source = Some(part(val()?)?),
            "--volts" => {
                let v = val()?;
                volts = v
                    .trim_end_matches(['V', 'v'])
                    .parse()
                    .map_err(|_| format!("not a voltage: {v}"))?
            }
            "--total" => {
                let v = val()?;
                total = amps(v).ok_or(format!("not a current: {v}"))?
            }
            "--plating" => {
                let v = val()?;
                plating = Length::parse(v).ok_or(format!("not a length: {v}"))?.mil()
            }
            "--load" => {
                let v = val()?;
                let (d, i) = v
                    .split_once('=')
                    .ok_or(format!("--load wants PART=AMPS, not {v}"))?;
                loads.push((part(d)?, amps(i).ok_or(format!("not a current: {i}"))?));
            }
            _ if out.is_none() && !a.starts_with("--") => out = Some(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let source = match source {
        Some(s) => s,
        None => {
            pi::guess_source(&b, id).ok_or("no likely source on the net: name one with --source")?
        }
    };
    let guessed = loads.is_empty();
    if guessed {
        // Share the total among the other parts by their pad count.
        let parts = pi::default_loads(&b, id, source);
        let pads: usize = parts.iter().map(|p| p.1).sum();
        if pads == 0 {
            return Err("no other parts on the net to load it".into());
        }
        loads = parts
            .iter()
            .map(|p| (p.0, total * p.1 as f64 / pads as f64))
            .collect();
    }
    let mut setup = pi::Setup::new(id, source, volts);
    setup.loads = loads;
    setup.plating_mil = plating;
    let t = std::time::Instant::now();
    let r = pi::solve(&b, &setup)?;
    let des = |i: usize| b.components[i].designator.as_str();
    println!(
        "{}: {:.3} V from {}, {:.3} A into {} loads{}",
        b.nets[id as usize].name,
        volts,
        des(source),
        r.loads.iter().map(|l| l.amps).sum::<f64>(),
        r.loads.len(),
        if guessed {
            " (shared by pad count)"
        } else {
            ""
        }
    );
    println!(
        "Grid {:.2} mil, {} x {} cells, {} vias carrying current; solved in {:.1} s",
        r.cell,
        r.cols,
        r.rows,
        r.vias,
        t.elapsed().as_secs_f64()
    );
    println!(
        "  {:<12} {:>8} {:>10} {:>10} {:>9}",
        "Load", "A", "V min", "V mean", "drop mV"
    );
    let mut rows: Vec<_> = r.loads.iter().collect();
    rows.sort_by(|a, b| a.v_min.total_cmp(&b.v_min));
    let hidden = rows.len().saturating_sub(20);
    for l in rows.into_iter().take(20) {
        if l.connected {
            println!(
                "  {:<12} {:>8.3} {:>10.4} {:>10.4} {:>9.2}",
                des(l.part),
                l.amps,
                l.v_min,
                l.v_mean,
                (volts - l.v_min) * 1000.0
            );
        } else {
            println!(
                "  {:<12} {:>8.3}   not joined to the source on this net",
                des(l.part),
                l.amps
            );
        }
    }
    if hidden > 0 {
        println!("  ... and {hidden} more with less drop");
    }
    println!(
        "Largest drop {:.2} mV ({:.2}%), copper loss {:.1} mW",
        r.max_drop * 1000.0,
        r.max_drop / volts * 100.0,
        r.loss_w * 1000.0
    );
    if let Some((p, a)) = r.worst_via {
        println!("Busiest via at ({:.0}, {:.0}) mil: {:.3} A", p[0], p[1], a);
    }
    if let Some((p, a, j)) = r.worst_track {
        println!(
            "Densest track at ({:.0}, {:.0}) mil: {:.3} A, {:.1} A/mm2",
            p[0], p[1], a, j
        );
    }
    for n in &r.notes {
        println!("Note: {n}");
    }
    if let Some(out) = out {
        // Marks: the source and each load at the middle of its pads.
        let mid = |part: usize| -> Option<[f64; 2]> {
            let ps: Vec<_> = b
                .geometry
                .pads
                .iter()
                .filter(|p| p.component == Some(part as u32) && p.net == Some(id))
                .map(|p| [p.at.x.mil(), p.at.y.mil()])
                .collect();
            (!ps.is_empty()).then(|| {
                let n = ps.len() as f64;
                [
                    ps.iter().map(|p| p[0]).sum::<f64>() / n,
                    ps.iter().map(|p| p[1]).sum::<f64>() / n,
                ]
            })
        };
        let mut marks = Vec::new();
        if let Some(p) = mid(source) {
            marks.push((p, format!("{} (source)", des(source))));
        }
        let mut heavy: Vec<_> = r.loads.iter().collect();
        heavy.sort_by(|a, b| b.amps.total_cmp(&a.amps));
        for l in heavy.into_iter().take(12) {
            if let Some(p) = mid(l.part) {
                marks.push((p, des(l.part).to_string()));
            }
        }
        let path = Path::new(out);
        let stem = path.with_extension("");
        for m in &r.maps {
            let f = if r.maps.len() == 1 {
                path.to_path_buf()
            } else {
                let safe: String = m
                    .name
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect();
                PathBuf::from(format!("{}-{safe}.svg", stem.display()))
            };
            let title = format!("{} on {}", b.nets[id as usize].name, m.name);
            std::fs::write(&f, r.svg(m, 1200.0, &title, &marks))?;
            println!("Saved {}", f.display());
        }
    }
    Ok(())
}

fn pairsize(file: &str, layer: &str, rest: &[&str]) -> Res {
    use trace_core::pairopt::{self, PairGoal};
    let b = board(file)?;
    let geo = b.layer_geometry();
    let g = layer
        .parse::<usize>()
        .ok()
        .and_then(|n| geo.get(n.checked_sub(1)?))
        .or_else(|| geo.iter().find(|g| g.name.eq_ignore_ascii_case(layer)))
        .ok_or_else(|| {
            format!(
                "no copper layer {layer}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let mut goal = PairGoal::new(100.0);
    if let Some(w) = trace_core::route::board_min_width(&b) {
        goal.w_min = w.max(1.0);
    }
    goal.s_min = trace_core::route::board_clearance(&b).clamp(2.0, 20.0);
    let mut next_max = 2.0;
    let mut it = rest.iter();
    let num = |x: Option<&&str>, what: &str| -> Result<f64, String> {
        let x = x.ok_or(format!("{what} needs a value"))?;
        x.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%')
            .parse()
            .map_err(|_| format!("not a number: {x}"))
    };
    while let Some(&a) = it.next() {
        match a {
            "--zdiff" => goal.zdiff = num(it.next(), a)?,
            "--wmin" => goal.w_min = num(it.next(), a)?,
            "--wmax" => goal.w_max = num(it.next(), a)?,
            "--gap" => goal.s_min = num(it.next(), a)?,
            "--ghz" => goal.f = num(it.next(), a)? * 1e9,
            "--next" => goal.neighbour = Some(num(it.next(), a)?),
            "--xtalk" => next_max = num(it.next(), a)?,
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    println!(
        "{}: {:.0} Ω pairs, tracks {:.2} to {:.2} mil, gap at least {:.2} mil, loss at {:.1} GHz",
        g.name,
        goal.zdiff,
        goal.w_min,
        goal.w_max,
        goal.s_min,
        goal.f / 1e9
    );
    let opts = pairopt::pair_options(g, &goal);
    if opts.is_empty() {
        return Err("no width in that range reaches the target with the gap allowed".into());
    }
    let best = pairopt::best(&opts, next_max, None);
    println!(
        "{:>7} {:>7} {:>7} {:>9} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} {:>6}",
        "width",
        "gap",
        "Zdiff",
        "dB/in",
        "copper",
        "diel",
        "ps/in",
        "NEXT%",
        "FEXT%",
        "span",
        "etch"
    );
    for (i, o) in opts.iter().enumerate() {
        println!(
            "{:>7.2} {:>7.2} {:>7.1} {:>9.3} {:>8.3} {:>8.3} {:>8.1} {:>7.2} {:>7.2} {:>7.1} {:>6.1}{}",
            o.w,
            o.s,
            o.zdiff,
            o.loss,
            o.loss_cu,
            o.loss_diel,
            o.delay,
            o.next,
            o.fext,
            o.span,
            o.etch,
            if Some(i) == best { "  <- best" } else { "" }
        );
    }
    if best.is_none() {
        println!("None keeps crosstalk under {next_max} %.");
    }
    Ok(())
}

fn sparam_check(files: &[&str]) -> Res {
    let mut bad = false;
    for f in files {
        let n = trace_core::touchstone::read(Path::new(f))?;
        let c = n.checks();
        println!(
            "{f}: {} ports, {} points, {:.3} to {:.3} GHz",
            n.ports,
            n.f.len(),
            n.f.first().copied().unwrap_or(0.0),
            n.f.last().copied().unwrap_or(0.0)
        );
        for line in c.report().lines() {
            println!("  {line}");
        }
        bad |= !(c.passive() && c.causal());
    }
    if bad {
        std::process::exit(1);
    }
    Ok(())
}

fn cascade(load: &str, rest: &[&str]) -> Res {
    use trace_core::fdtd::Cx;
    use trace_core::touchstone::{self, Network};
    enum Stage {
        File(String, Network),
        Line(f64, f64, f64, f64),
    }
    let ld = touchstone::read(std::path::Path::new(load))?;
    if ld.ports != 1 {
        return Err("the load must be a one-port (.s1p)".into());
    }
    let mut stages = Vec::new();
    let mut z_print = None;
    let mut step = None;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--line" => {
                let x = it.next().ok_or("--line needs Z@length[@Eeff[@dB/in]]")?;
                // The length takes mil (the default), mm, um or in.
                let v: Vec<f64> = x
                    .split('@')
                    .enumerate()
                    .map(|(k, p)| {
                        if k == 1 {
                            trace_core::Length::parse(p).map(|l| l.mil())
                        } else {
                            p.parse::<f64>().ok()
                        }
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(|| format!("not Z@length@Eeff@dB/in: {x}"))?;
                if v.len() < 2 {
                    return Err(format!("--line needs Z@length: {x}").into());
                }
                stages.push(Stage::Line(
                    v[0],
                    v[1],
                    v.get(2).copied().unwrap_or(4.0),
                    v.get(3).copied().unwrap_or(0.0),
                ));
            }
            "--ref" => {
                z_print = Some(
                    it.next()
                        .and_then(|r| r.parse::<f64>().ok())
                        .ok_or("--ref needs ohms")?,
                )
            }
            "--step" => {
                step = Some(
                    it.next()
                        .and_then(|r| r.parse::<f64>().ok())
                        .ok_or("--step needs GHz")?,
                )
            }
            _ if !a.starts_with("--") => {
                let n = touchstone::read(std::path::Path::new(a))?;
                if n.ports != 2 {
                    return Err(format!("{a}: stages must be two-ports (.s2p)").into());
                }
                stages.push(Stage::File(a.to_string(), n));
            }
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let zr = z_print.unwrap_or_else(|| {
        stages
            .iter()
            .find_map(|s| match s {
                Stage::File(_, n) => Some(n.z_ref),
                _ => None,
            })
            .unwrap_or(ld.z_ref)
    });
    // The band all files share.
    let (mut lo, mut hi) = (ld.f[0], *ld.f.last().unwrap());
    for s in &stages {
        if let Stage::File(_, n) = s {
            lo = lo.max(n.f[0]);
            hi = hi.min(*n.f.last().unwrap());
        }
    }
    if hi <= lo {
        return Err("the files share no frequencies".into());
    }
    println!("Load {load} ({} Ω reference in the file)", ld.z_ref);
    for (i, s) in stages.iter().enumerate() {
        match s {
            Stage::File(p, n) => println!("  stage {}: {p} ({} Ω)", i + 1, n.z_ref),
            Stage::Line(z, l, e, db) => println!(
                "  stage {}: line {z} Ω, {l} mil, Eeff {e}, {db} dB/in",
                i + 1
            ),
        }
    }
    println!("Return loss in a {zr} Ω reference, from the first stage's input:");
    println!(
        "{:>9} {:>14} {:>10} {:>18} {:>10}",
        "GHz", "load Z (Ω)", "load dB", "input Z (Ω)", "input dB"
    );
    let dstep = step.unwrap_or(((hi - lo) / 20.0).max(0.01));
    let mut f = lo.max(dstep.min(hi));
    while f <= hi + 1e-9 {
        let zl = ld.impedance(f).ok_or("load outside its band")?;
        let mut z = zl;
        for s in stages.iter().rev() {
            z = match s {
                Stage::File(_, n) => {
                    let m = n.at(f).ok_or("stage outside its band")?;
                    touchstone::input_impedance(&m, n.z_ref, z)
                }
                Stage::Line(zc, l, e, db) => {
                    let m = touchstone::line(*zc, *e, *l, *db, f, zr);
                    touchstone::input_impedance(&m, zr, z)
                }
            };
        }
        let fmt = |z: Cx| format!("{:.1}{:+.1}j", z.re, z.im);
        println!(
            "{f:9.3} {:>14} {:10.2} {:>18} {:10.2}",
            fmt(zl),
            touchstone::z_to_gamma(zl, zr).db(),
            fmt(z),
            touchstone::z_to_gamma(z, zr).db()
        );
        f += dstep;
    }
    Ok(())
}

fn stackup(file: &str, out: Option<&str>) -> Res {
    let b = board(file)?;
    let text = trace_core::stackfile::write(&b.stackup);
    match out {
        Some(o) => {
            std::fs::write(o, &text)?;
            println!("Wrote {o}");
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn validate(rest: &[&str]) -> Res {
    use trace_core::validate;
    let quick = rest.contains(&"--quick");
    let mut c = validate::field_checks();
    c.extend(validate::ir_checks());
    let t = std::time::Instant::now();
    c.extend(validate::thermal_checks());
    eprintln!("  thermal checks: {:.0} s", t.elapsed().as_secs_f64());
    if !quick {
        let t = std::time::Instant::now();
        let last = std::cell::Cell::new(-1i32);
        c.extend(validate::via_checks(&|p| {
            let pc = (p * 100.0) as i32 / 5 * 5;
            if pc > last.get() {
                last.set(pc);
                eprint!("\r  3D checks… {pc}%");
            }
        }));
        eprintln!("\r  3D checks: {:.0} s      ", t.elapsed().as_secs_f64());
    }
    let text = validate::report(&c);
    print!("{text}");
    if let Some(out) = rest.iter().find(|a| !a.starts_with("--")) {
        std::fs::write(out, &text)?;
        println!("Wrote {out}");
    }
    if c.iter().all(|c| c.pass()) {
        Ok(())
    } else {
        Err("some checks failed".into())
    }
}

fn path_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::pathloss::{self, Options, TuneGoal};
    use trace_core::touchstone;
    let b = board(file)?;
    let mut opt = Options::default();
    let mut goal: Option<TuneGoal> = None;
    let mut eye: Option<trace_core::eye::EyeSetup> = None;
    let mut eye_svg: Option<&str> = None;
    let mut eq_auto: Option<(bool, bool)> = None;
    let mut xtalk = false;
    let (mut ghz_set, mut points_set) = (false, false);
    let mut pins: Vec<&str> = Vec::new();
    let mut outs: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let next = |i: usize| -> Result<&str, String> {
            rest.get(i + 1)
                .copied()
                .ok_or_else(|| format!("{} needs a value", rest[i]))
        };
        match rest[i] {
            "--ghz" => {
                opt.ghz = next(i)?.trim_end_matches("GHz").parse()?;
                ghz_set = true;
                i += 1;
            }
            "--points" => {
                opt.points = next(i)?.parse()?;
                points_set = true;
                i += 1;
            }
            "--eye" => {
                let mut e = eye.take().unwrap_or_default();
                e.gbps = next(i)?
                    .to_ascii_lowercase()
                    .trim_end_matches("gbps")
                    .trim_end_matches("gb/s")
                    .trim_end_matches('g')
                    .parse()?;
                eye = Some(e);
                i += 1;
            }
            "--amp" | "--rise" | "--prbs" | "--bits" => {
                let mut e = eye.take().unwrap_or_default();
                let v = next(i)?;
                match rest[i] {
                    "--amp" => e.amplitude = v.trim_end_matches(['V', 'v']).parse()?,
                    "--rise" => e.rise_ps = v.trim_end_matches("ps").parse()?,
                    "--prbs" => e.prbs = v.trim_start_matches("PRBS").parse()?,
                    _ => e.bits = v.parse()?,
                }
                eye = Some(e);
                i += 1;
            }
            "--ffe" => {
                let mut e = eye.take().unwrap_or_default();
                let v = next(i)?;
                let (a, b) = v
                    .split_once(',')
                    .ok_or_else(|| format!("{v}: give PRE,POST (such as -0.05,-0.15)"))?;
                e.eq.pre = -a.trim().parse::<f64>()?.abs();
                e.eq.post = -b.trim().parse::<f64>()?.abs();
                eye = Some(e);
                i += 1;
            }
            "--ctle" => {
                let mut e = eye.take().unwrap_or_default();
                let v = next(i)?.to_ascii_lowercase();
                let (db, at) = v.split_once('@').unwrap_or((v.as_str(), ""));
                e.eq.ctle_db = db.trim_end_matches("db").parse()?;
                if !at.is_empty() {
                    e.eq.ctle_ghz = at.trim_end_matches("ghz").parse()?;
                }
                eye = Some(e);
                i += 1;
            }
            "--eq" => {
                eq_auto = Some(match next(i)? {
                    "ffe" => (true, false),
                    "ctle" => (false, true),
                    _ => (true, true),
                });
                if eye.is_none() {
                    eye = Some(Default::default());
                }
                i += 1;
            }
            "--xtalk" => {
                xtalk = true;
                if eye.is_none() {
                    eye = Some(Default::default());
                }
            }
            "--eye-svg" => {
                eye_svg = Some(next(i)?);
                i += 1;
            }
            "--quick-vias" | "--no-vias" => opt.vias_3d = false,
            "--df" => {
                opt.tan_d = next(i)?.parse()?;
                i += 1;
            }
            "--fmin" => {
                opt.fmin = next(i)?.trim_end_matches("GHz").parse()?;
                i += 1;
            }
            "--tune" => {
                let mut g = goal.take().unwrap_or_default();
                // An optional band, "1-6" (GHz).
                if let Some((a, z)) = rest
                    .get(i + 1)
                    .and_then(|v| v.trim_end_matches("GHz").split_once('-'))
                {
                    if let (Ok(a), Ok(z)) = (a.parse::<f64>(), z.parse::<f64>()) {
                        g.band = (a, z);
                        i += 1;
                    }
                }
                goal = Some(g);
            }
            "--il-weight" => {
                let mut g = goal.take().unwrap_or_default();
                g.il_weight = next(i)?.parse()?;
                goal = Some(g);
                i += 1;
            }
            "--any" => opt.any_part = true,
            "--free" => {
                opt.free.push(next(i)?.to_string());
                i += 1;
            }
            "--fit" => {
                opt.fit.push(next(i)?.to_string());
                i += 1;
            }
            "--model" => {
                let a = next(i)?;
                let (pat, f) = a
                    .split_once('=')
                    .ok_or_else(|| format!("{a}: give PATTERN=file.sNp"))?;
                let n = touchstone::read(std::path::Path::new(f))?;
                println!(
                    "Model {pat}: {f} ({} port(s), {:.3}-{:.3} GHz, {} points)",
                    n.ports,
                    n.f.first().unwrap_or(&0.0),
                    n.f.last().unwrap_or(&0.0),
                    n.f.len()
                );
                opt.models.push((pat.to_string(), n));
                i += 1;
            }
            "--load" => {
                let f = next(i)?;
                opt.load = Some(touchstone::read(std::path::Path::new(f))?);
                i += 1;
            }
            a if a.starts_with("--") => return Err(format!("unknown option {a}").into()),
            a if a.contains('.')
                && [".s2p", ".svg", ".csv", ".txt"]
                    .iter()
                    .any(|e| a.to_ascii_lowercase().ends_with(e)) =>
            {
                outs.push(a)
            }
            a => pins.push(a),
        }
        i += 1;
    }
    let Some(from) = pins.first() else {
        return Err("give the start pin (J503.1, a part, or a net)".into());
    };
    opt.from = from.to_string();
    opt.to = pins.get(1).map(|s| s.to_string());
    if let Some(e) = &eye {
        // The edge's content reaches about 2.5 × the bit rate; a fine
        // step keeps the pulse from wrapping.
        if !ghz_set {
            opt.ghz = (2.5 * e.gbps).ceil().max(opt.ghz);
        }
        if !points_set {
            opt.points = ((opt.ghz * 40.0).ceil() as usize).max(opt.points);
        }
    }
    let t = std::time::Instant::now();
    let last = std::cell::Cell::new(String::new());
    let prog = |p: f64, what: &str| {
        if what != last.take().as_str() {
            eprintln!("  {:3.0} % {what}", p * 100.0);
        }
        last.set(what.to_string());
    };
    let rep = match &goal {
        None => pathloss::run(&b, &b.stackup, &opt, &prog)?,
        Some(g) => {
            let tuned = pathloss::tune(&b, &b.stackup, &opt, g, &prog)?;
            println!("Before: {}", tuned.before.summary());
            println!(
                "Tuned for {}-{} GHz{} ({} tries):",
                tuned.band.0,
                tuned.band.1,
                if opt.load.is_some() {
                    ", into the load"
                } else {
                    ""
                },
                tuned.evaluations
            );
            for c in &tuned.choices {
                println!(
                    "  {:<16} {:<10} {}",
                    c.site,
                    c.part,
                    if c.series {
                        "in series"
                    } else if c.across {
                        "across the pair"
                    } else {
                        "to ground"
                    }
                );
            }
            println!(
                "Worst return loss in the band: {:.1} -> {:.1} dB; mean insertion loss {:.3} -> {:.3} dB",
                tuned.worst.0, tuned.worst.1, tuned.il.0, tuned.il.1
            );
            tuned.after
        }
    };
    println!("{} ({:.0} s)", rep.summary(), t.elapsed().as_secs_f64());
    let top = rep.f.last().copied().unwrap_or(0.0);
    println!(
        "Ports: {} ({} ohm) to {} ({} ohm)",
        rep.start, rep.z_ref[0], rep.end, rep.z_ref[1]
    );
    println!(
        "  {:<44} {:>6} {:>8} {:>9} {:>9}  model",
        "section",
        "Z",
        "mil",
        format!("own@{top}"),
        "total dB"
    );
    for s in &rep.sections {
        println!(
            "  {:<44} {:>6} {:>8.0} {:>9.3} {:>9.3}  {}{}",
            format!("{} [{}]", s.what, s.net),
            s.z.map_or("".to_string(), |z| format!("{z:.1}")),
            s.length,
            s.loss_db,
            s.total_db,
            if s.diff { "(diff) " } else { "" },
            s.model
        );
    }
    let mut marks: Vec<f64> = [
        0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 30.0, 40.0,
    ]
    .into_iter()
    .filter(|f| *f < top - 1e-9)
    .collect();
    marks.push(top);
    let il: Vec<String> = marks
        .iter()
        .filter_map(|&f| rep.il_at(f).map(|d| format!("{f} GHz {d:.3}")))
        .collect();
    println!("Insertion loss, dB: {}", il.join(", "));
    if let Some((rl, f)) = rep.worst_rl() {
        println!("Return loss: worst {rl:.1} dB at {f:.2} GHz");
    }
    if !rep.s11_loaded.is_empty() {
        let w = rep.worst_loaded();
        if let Some((d, f)) = w {
            println!("Into the path with the load at the end: worst {d:.1} dB at {f:.2} GHz");
            let at: Vec<String> = [1.0, 3.0, 6.0]
                .into_iter()
                .filter_map(|g| {
                    let k = rep.f.iter().position(|x| *x >= g - 1e-9)?;
                    Some(format!("{g} GHz {:.1}", rep.s11_loaded[k].db()))
                })
                .collect();
            println!("  with the load, dB: {}", at.join(", "));
        }
    }
    for n in &rep.notes {
        println!("Note: {n}");
    }
    if let Some(e) = &mut eye {
        if xtalk {
            e.aggressors = trace_core::eye::aggressors(&b, &rep, e.amplitude);
            println!(
                "Crosstalk: {} net(s) run beside the path",
                e.aggressors.len()
            );
        }
        if let Some((ffe, ctle)) = eq_auto {
            let plain = trace_core::eye::EyeSetup {
                eq: Default::default(),
                ..e.clone()
            };
            let r0 = trace_core::eye::eye(&rep, &plain)?;
            print!("Without equalisation: {}", r0.summary());
            let c = trace_core::eye::optimise(&rep, e, ffe, ctle)?;
            println!(
                "Equalisation picked ({} tries): {}; worst case height {:.1} -> {:.1} mV",
                c.tries,
                c.eq.describe(e.gbps),
                c.before * 1e3,
                c.after * 1e3
            );
            e.eq = c.eq;
        }
        let r = trace_core::eye::eye(&rep, e)?;
        print!("{}", r.summary());
        if let Some(o) = eye_svg {
            std::fs::write(
                o,
                r.svg(&format!(
                    "{} to {} at {} Gbit/s",
                    rep.start, rep.end, e.gbps
                )),
            )?;
            println!("Wrote {o}");
        }
    }
    for o in outs {
        let lo = o.to_ascii_lowercase();
        let text = if lo.ends_with(".svg") {
            rep.svg(&format!("{} to {}", rep.start, rep.end))
        } else if lo.ends_with(".csv") {
            rep.csv()
        } else {
            rep.touchstone()
        };
        std::fs::write(o, text)?;
        println!("Wrote {o}");
    }
    Ok(())
}

fn lc_cmd(value: &str, rest: &[&str]) -> Res {
    use trace_core::lcparts::{self, ChipModel, Kind};
    use trace_core::pdn::{self, Package};
    if value == "--list" {
        for f in lcparts::FAMILIES {
            let what = match f.kind {
                Kind::Cap => format!("ESL {:.2} nH", f.esl * 1e9),
                Kind::Ind => {
                    let m = ChipModel {
                        family: f,
                        value: 10e-9,
                        class2: false,
                    };
                    format!("10 nH: SRF {:.2} GHz, Q {:.0} at 1 GHz", m.srf(), m.q(1.0))
                }
            };
            println!(
                "{:<40} {} {:<11} {:<18} {}",
                f.name,
                f.case.label(),
                f.tech,
                if f.keys.is_empty() {
                    "(by case)".to_string()
                } else {
                    f.keys.join(",")
                },
                what
            );
        }
        return Ok(());
    }
    let mut case = Package::P0402;
    let mut family: Option<String> = None;
    let (mut lo, mut hi, mut points) = (0.1f64, 6.0f64, 13usize);
    let mut i = 0;
    while i < rest.len() {
        let v = rest.get(i + 1).copied();
        let need = || v.ok_or_else(|| format!("{} needs a value", rest[i]));
        match rest[i] {
            "--case" => {
                case = match need()? {
                    "0201" => Package::P0201,
                    "0402" => Package::P0402,
                    "0603" => Package::P0603,
                    "0805" => Package::P0805,
                    "1206" => Package::P1206,
                    c => {
                        return Err(
                            format!("--case {c}: give 0201, 0402, 0603, 0805 or 1206").into()
                        )
                    }
                };
                i += 1;
            }
            "--family" | "--part" => {
                family = Some(need()?.to_string());
                i += 1;
            }
            "--band" => {
                let b = need()?.trim_end_matches("GHz");
                let (a, z) = b
                    .split_once('-')
                    .ok_or_else(|| format!("{b}: give LOW-HIGH in GHz, such as 0.1-6"))?;
                lo = a.parse()?;
                hi = z.parse()?;
                i += 1;
            }
            "--points" => {
                points = need()?.parse::<usize>()?.max(2);
                i += 1;
            }
            o => return Err(format!("trace lc: unknown option {o}").into()),
        }
        i += 1;
    }
    if !(lo > 0.0 && hi > lo) {
        return Err("--band: give LOW-HIGH with LOW above 0".into());
    }
    let text = family.clone().unwrap_or_default();
    let m = if let Some(h) = trace_core::pathloss::parse_henries(value)
        .filter(|_| value.to_ascii_lowercase().contains('h'))
    {
        let mut m = ChipModel::inductor(h, case, &text);
        if let Some(f) = family
            .as_deref()
            .and_then(|f| lcparts::family_named(Kind::Ind, f))
        {
            m.family = f;
        }
        m
    } else if let Some(c) = pdn::parse_farads(value) {
        let mut m = ChipModel::capacitor(c, case, &format!("{text} {value}"));
        if let Some(f) = family
            .as_deref()
            .and_then(|f| lcparts::family_named(Kind::Cap, f))
        {
            m.family = f;
        }
        m
    } else {
        return Err(format!("{value}: give a value such as 10nH, 4n7H, 1.2pF or 100nF").into());
    };
    if let Some(f) = &family {
        let kind = m.kind();
        if lcparts::family_named(kind, f).is_none() {
            eprintln!("note: no family matches {f}; using {}", m.family.name);
        }
    }
    // Log-spaced points over the band.
    let r = (hi / lo).powf(1.0 / (points - 1) as f64);
    let freqs: Vec<f64> = (0..points).map(|k| lo * r.powi(k as i32)).collect();
    print!("{}", m.table(&freqs));
    Ok(())
}

fn rf_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::rfreport::{self, Setup};
    use trace_core::touchstone;
    let b = board(file)?;
    let mut setup = Setup::default();
    let mut outs: Vec<&str> = Vec::new();
    let mut dir: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        let next = |i: usize| -> Result<&str, String> {
            rest.get(i + 1)
                .copied()
                .ok_or_else(|| format!("{} needs a value", rest[i]))
        };
        match rest[i] {
            "--band" => {
                let v = next(i)?.trim_end_matches("GHz");
                let (a, z) = v
                    .split_once('-')
                    .ok_or_else(|| format!("{v}: give LOW-HIGH in GHz, such as 0.5-6"))?;
                setup.band = (a.parse()?, z.parse()?);
                if setup.band.1 <= setup.band.0 {
                    return Err("--band: the top must be above the bottom".into());
                }
                i += 1;
            }
            "--ghz" => {
                setup.band.1 = next(i)?.trim_end_matches("GHz").parse()?;
                i += 1;
            }
            "--fmin" => {
                setup.band.0 = next(i)?.trim_end_matches("GHz").parse()?;
                i += 1;
            }
            "--points" => {
                setup.points = next(i)?.parse()?;
                i += 1;
            }
            "--quick-vias" | "--no-vias" => setup.vias_3d = false,
            "--df" => {
                setup.tan_d = next(i)?.parse()?;
                i += 1;
            }
            "--rl" => {
                setup.rl_goal = next(i)?.trim_end_matches("dB").parse::<f64>()?.abs();
                i += 1;
            }
            "--il-budget" => {
                setup.il_budget = Some(next(i)?.trim_end_matches("dB").parse::<f64>()?.abs());
                i += 1;
            }
            "--only" => {
                setup
                    .only
                    .extend(next(i)?.split(',').map(|s| s.trim().to_string()));
                i += 1;
            }
            "--from" => {
                setup.extra.push(next(i)?.to_string());
                i += 1;
            }
            "--fit" => {
                setup.fit.push(next(i)?.to_string());
                i += 1;
            }
            "--model" | "--load" => {
                let a = next(i)?;
                let (pat, f) = a
                    .split_once('=')
                    .ok_or_else(|| format!("{a}: give PATTERN=file.sNp"))?;
                let n = touchstone::read(std::path::Path::new(f))?;
                let name = std::path::Path::new(f)
                    .file_name()
                    .map_or(f.to_string(), |x| x.to_string_lossy().to_string());
                println!(
                    "{} {pat}: {name} ({} port(s), {:.3}-{:.3} GHz)",
                    if rest[i] == "--load" { "Load" } else { "Model" },
                    n.ports,
                    n.f.first().unwrap_or(&0.0),
                    n.f.last().unwrap_or(&0.0)
                );
                if rest[i] == "--load" {
                    setup.loads.push((pat.to_string(), name, n));
                } else {
                    setup.models.push((pat.to_string(), n));
                }
                i += 1;
            }
            "--out" => {
                dir = Some(next(i)?);
                i += 1;
            }
            a if a.starts_with("--") => return Err(format!("unknown option {a}").into()),
            a => outs.push(a),
        }
        i += 1;
    }
    let t = std::time::Instant::now();
    let last = std::cell::Cell::new(String::new());
    let prog = |p: f64, what: &str| {
        if what != last.take().as_str() {
            eprintln!("  {:3.0} % {what}", p * 100.0);
        }
        last.set(what.to_string());
    };
    let rep = rfreport::run(&b, &b.stackup, &setup, &prog)?;
    let text = rep.text();
    print!("{text}");
    println!("({:.0} s)", t.elapsed().as_secs_f64());
    let title = format!(
        "RF paths of {}",
        std::path::Path::new(file)
            .file_name()
            .map_or(file.to_string(), |n| n.to_string_lossy().to_string())
    );
    let write = |o: &str, body: String| -> Res {
        std::fs::write(o, body)?;
        println!("Wrote {o}");
        Ok(())
    };
    for o in &outs {
        let lo = o.to_ascii_lowercase();
        if lo.ends_with(".svg") {
            write(o, rep.svg(&title))?;
        } else if lo.ends_with(".csv") {
            write(o, rep.csv())?;
        } else {
            write(o, text.clone())?;
        }
    }
    if let Some(d) = dir {
        let d = std::path::Path::new(d);
        std::fs::create_dir_all(d)?;
        let at = |n: &str| d.join(n).to_string_lossy().to_string();
        write(&at("rf-report.txt"), text.clone())?;
        write(&at("rf-report.csv"), rep.csv())?;
        write(&at("rf-report.svg"), rep.svg(&title))?;
        for p in &rep.paths {
            if let Ok(r) = &p.report {
                let stem = p.file_stem();
                std::fs::write(d.join(format!("{stem}.s2p")), r.touchstone())?;
                std::fs::write(
                    d.join(format!("{stem}.svg")),
                    r.svg(&format!(
                        "{} {}: {} to {}",
                        p.class.name(),
                        p.name,
                        r.start,
                        r.end
                    )),
                )?;
            }
        }
        println!("Wrote each path's .s2p and .svg to {}", d.display());
    }
    if rep.failing() > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn thermal_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::thermal::{self, Air, Heatsink, Setup};
    use trace_core::thermfield;
    let b = board(file)?;
    let mut setup = Setup::new();
    let mut sinks: Vec<(String, Heatsink)> = Vec::new();
    let mut outs: Vec<&str> = Vec::new();
    let mut check = false;
    let mut speed = 0.0;
    let mut vertical = false;
    let mut maps_q: Vec<thermfield::TQuantity> = thermfield::TQuantity::ALL.to_vec();
    let mut all_planes = false;
    let mut at: Option<[f64; 2]> = None;
    let mut decades = 3.0;
    let mut sweep = thermfield::Sweep::Depth;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--power" => {
                for p in val()?.split(',') {
                    setup.sources.push(thermal::parse_power(&b, p)?);
                }
            }
            "--heatsink" => {
                let v = val()?;
                let (part, spec) = v.split_once('=').ok_or("--heatsink: PART=LxWxH[,...]")?;
                sinks.push((part.trim().to_string(), Heatsink::parse(spec)?));
            }
            "--ambient" => {
                setup.ambient_c = val()?
                    .trim_end_matches(['C', 'c'])
                    .parse()
                    .map_err(|_| "--ambient: not °C")?
            }
            "--air" => {
                speed = val()?
                    .trim_end_matches("m/s")
                    .parse()
                    .map_err(|_| "--air: not m/s")?
            }
            "--vertical" => vertical = true,
            "--cell" => {
                setup.cell_mm = val()?
                    .trim_end_matches("mm")
                    .parse()
                    .map_err(|_| "--cell: not mm")?
            }
            "--k-diel" => {
                let v = val()?;
                let (i, z) = v.split_once('/').unwrap_or((v, v));
                setup.k_diel = Some((
                    i.parse().map_err(|_| "--k-diel: not W/mK")?,
                    z.parse().map_err(|_| "--k-diel: not W/mK")?,
                ));
            }
            "--emissivity" => {
                setup.emissivity = val()?.parse().map_err(|_| "--emissivity: not a number")?
            }
            "--check" => check = true,
            "--maps" => {
                maps_q = val()?
                    .split(',')
                    .map(|q| {
                        thermfield::TQuantity::parse(q)
                            .ok_or(format!("--maps: {q}? (t, flux, gradient, power, air)"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            "--planes" => all_planes = val()?.eq_ignore_ascii_case("all"),
            "--at" => {
                let v = val()?;
                let (x, y) = v.split_once(',').ok_or("--at: X,Y in mm")?;
                at = Some([
                    x.trim()
                        .trim_end_matches("mm")
                        .parse()
                        .map_err(|_| "--at: not mm")?,
                    y.trim()
                        .trim_end_matches("mm")
                        .parse()
                        .map_err(|_| "--at: not mm")?,
                ]);
            }
            "--decades" => decades = val()?.parse().map_err(|_| "--decades: not a number")?,
            "--sweep" => {
                let v = val()?;
                sweep = thermfield::Sweep::parse(v).ok_or(format!("--sweep: {v}? (depth, x, y)"))?
            }
            _ if !a.starts_with("--") => outs.push(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    setup.air = if speed > 0.0 {
        Air::Forced { speed }
    } else {
        Air::Natural { vertical }
    };
    for (part, h) in sinks {
        let s = setup
            .sources
            .iter_mut()
            .find(|s| b.components[s.part].designator.eq_ignore_ascii_case(&part))
            .ok_or(format!("--heatsink {part}: give its --power first"))?;
        s.heatsink = Some(h);
    }
    for s in &setup.sources {
        println!(
            "{}: {:.2} W, θJC {:.2} K/W, θJB {:.2} K/W{}",
            b.components[s.part].designator,
            s.watts,
            s.theta_jc,
            s.theta_jb,
            s.heatsink
                .as_ref()
                .map_or(String::new(), |h| format!("; heatsink {}", h.describe()))
        );
    }
    println!(
        "Air: {}, ambient {:.1} °C",
        match setup.air {
            Air::Forced { speed } => format!("{speed:.1} m/s along x"),
            Air::Natural { vertical: true } => "still, board on its edge".into(),
            Air::Natural { vertical: false } => "still, board lying flat".into(),
        },
        setup.ambient_c
    );
    let t = std::time::Instant::now();
    let r = if check {
        let mc = thermal::check_mesh(&b, &setup)?;
        println!(
            "Grid check (cells {:.2} and {:.2} mm):",
            mc.coarse.cell, mc.fine.cell
        );
        for (d, tj, e) in &mc.error {
            println!("  {d}: Tj {tj:.1} °C ± {e:.2} K");
        }
        mc.fine
    } else {
        thermal::solve(&b, &setup)?
    };
    eprintln!("  solved in {:.1} s", t.elapsed().as_secs_f64());
    print!("{}", r.report());
    let title = format!(
        "{}: temperatures",
        Path::new(file)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned())
    );
    let cuts = r.field.planes(all_planes, at);
    for o in outs {
        let low = o.to_ascii_lowercase();
        if low.ends_with(".gif") {
            // One GIF per quantity asked; the first keeps the given name.
            for (k, &q) in maps_q.iter().enumerate() {
                let name = if k == 0 {
                    o.to_string()
                } else {
                    format!("{}-{}.gif", &o[..o.len() - 4], q.short())
                };
                std::fs::write(&name, r.field.gif(q, sweep, decades, 40))?;
                println!("Wrote {name} ({} swept {:?})", q.label(), sweep);
            }
            continue;
        }
        let text = if low.ends_with(".svg") {
            r.field.svg(&title, &maps_q, &cuts, decades)
        } else if low.ends_with(".csv") {
            r.csv()
        } else {
            r.report()
        };
        std::fs::write(o, text)?;
        println!("Wrote {o}");
    }
    Ok(())
}

fn bus_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::bussim::{self, BusSetup};
    let b = board(file)?;
    let mut setup = BusSetup::default();
    let mut outs: Vec<&str> = Vec::new();
    let mut rate_set = false;
    let num = |a: &str, v: &str, unit: &str| -> Result<f64, String> {
        v.trim()
            .to_ascii_lowercase()
            .trim_end_matches(unit)
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{a}: {v} is not a number"))
    };
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--from" => setup.from = val()?.to_string(),
            "--to" => setup.to = Some(val()?.to_string()),
            "--strobe" | "--clock" => setup.strobe = Some(val()?.to_string()),
            "--rate" => {
                setup.mts = num(a, val()?, "mt/s")?;
                rate_set = true;
            }
            "--swing" => setup.swing = num(a, val()?, "v")?,
            "--rise" => setup.rise_ps = num(a, val()?, "ps")?,
            "--setup" => setup.setup_ps = num(a, val()?, "ps")?,
            "--hold" => setup.hold_ps = num(a, val()?, "ps")?,
            "--offset" => setup.strobe_offset_ui = num(a, val()?, "ui")?,
            "--edge-aligned" => setup.strobe_offset_ui = 0.0,
            "--prbs" => setup.prbs = num(a, val()?, "")? as u32,
            "--no-xtalk" => setup.xtalk = false,
            "--vias-3d" => setup.vias_3d = true,
            "--mask" => {
                let v = val()?;
                let (h, w) = v.split_once(',').ok_or("--mask: HEIGHTmV,WIDTHUI")?;
                let h = num(a, h, "mv")? / 1e3;
                let w = num(a, w, "ui")?;
                if !(0.0..1.0).contains(&w) || h <= 0.0 {
                    return Err("--mask: height above 0 mV, width below 1 UI".into());
                }
                setup.mask = Some((h, w));
            }
            _ if a.starts_with("--") => return Err(format!("bus: unknown option {a}").into()),
            _ => {
                let low = a.to_ascii_lowercase();
                if low.ends_with(".svg") || low.ends_with(".csv") || low.ends_with(".txt") {
                    outs.push(a);
                } else {
                    setup.nets.push(a.to_string());
                }
            }
        }
    }
    if !rate_set {
        return Err("bus: give --rate in MT/s (DDR3-1600: 1600)".into());
    }
    if setup.nets.is_empty() {
        return Err("bus: name the bus's nets (patterns such as \"DDR_D*\" work)".into());
    }
    let t0 = std::time::Instant::now();
    let prog = |f: f64, what: &str| eprint!("\r  {:>3.0}% {:<40}", f * 100.0, what);
    let r = bussim::run(&b, &setup, &prog)?;
    eprintln!();
    print!("{}", r.report());
    println!("({:.1} s)", t0.elapsed().as_secs_f64());
    let title = format!(
        "{} bus timing",
        Path::new(file)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned())
    );
    for o in outs {
        let low = o.to_ascii_lowercase();
        let text = if low.ends_with(".svg") {
            r.svg(&title)
        } else if low.ends_with(".csv") {
            r.csv()
        } else {
            r.report()
        };
        std::fs::write(o, text)?;
        println!("Wrote {o}");
    }
    if !r.passes() {
        std::process::exit(1);
    }
    Ok(())
}

fn emi_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::emi::sim::{RailSpec, SignalSpec, SimSetup};
    use trace_core::emi::{self, EmiSetup, FastNet, Limit, Severity};
    let b = board(file)?;
    let mut setup = EmiSetup::default();
    let mut sim = SimSetup::default();
    let mut run_sim = true;
    let mut map_out: Option<String> = None;
    let mut outs: Vec<&str> = Vec::new();
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match a {
            "--net" => {
                let v = val()?;
                setup.nets.push(FastNet::parse(v)?);
                sim.signals.push(SignalSpec::parse(v)?);
            }
            "--signal" => sim.signals.push(SignalSpec::parse(val()?)?),
            "--rail" => sim.rails.push(RailSpec::parse(val()?)?),
            "--cable" => {
                let v = val()?;
                let (d, l) = v.split_once('=').ok_or("--cable J1=2m")?;
                let l = l.trim().trim_end_matches('m');
                sim.cables.push((
                    d.trim().to_string(),
                    l.parse().map_err(|_| format!("--cable: not metres: {l}"))?,
                ));
            }
            "--ibis" => {
                let v = val()?;
                let (d, f) = v.split_once('=').ok_or("--ibis U1=file.ibs")?;
                sim.ibis.push((
                    d.trim().to_string(),
                    trace_core::ibis::load(Path::new(f.trim()))?,
                ));
            }
            "--no-cmos" => sim.cmos = false,
            "--no-planes" => sim.planes = false,
            "--quick" => run_sim = false,
            "--map" => map_out = Some(val()?.to_string()),
            "--no-auto" => setup.auto = false,
            "--clock" => {
                let f = FastNet::parse(&format!("*={}", val()?))?;
                setup.auto_freq = f.freq;
                setup.auto_rise = f.rise;
                setup.auto_volts = f.volts;
            }
            "--limit" => {
                let v = val()?;
                setup.limit = Limit::parse(v)
                    .ok_or_else(|| format!("--limit: {v} (fcc-b, fcc-a, cispr-b, cispr-a)"))?;
                sim.limit = setup.limit;
            }
            "--margin" => {
                setup.margin_db = val()?
                    .trim_end_matches("dB")
                    .parse()
                    .map_err(|_| "--margin: not dB")?;
                sim.margin_db = setup.margin_db;
            }
            _ if a.starts_with("--") => return Err(format!("emi: unknown option {a}").into()),
            _ => outs.push(a),
        }
    }
    let t0 = std::time::Instant::now();
    let r = emi::check(&b, &setup);
    print!("{}", r.report());
    println!("({:.1} s)", t0.elapsed().as_secs_f64());
    let title = format!(
        "{} EMI",
        Path::new(file)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned())
    );
    let sres = if run_sim {
        let t1 = std::time::Instant::now();
        let last = std::cell::Cell::new(-1i32);
        let progress = |f: f64| {
            let pc = (f * 100.0) as i32;
            if pc != last.get() {
                last.set(pc);
                eprint!("\r  simulating… {pc}%");
            }
        };
        let s = emi::sim::simulate(&b, &sim, &progress)?;
        eprintln!();
        println!();
        print!("{}", s.report());
        println!("({:.1} s)", t1.elapsed().as_secs_f64());
        Some(s)
    } else {
        None
    };
    for o in outs {
        let low = o.to_ascii_lowercase();
        let text = if low.ends_with(".svg") {
            match &sres {
                Some(s) => s.svg(&title),
                None => r.svg(&title),
            }
        } else if low.ends_with(".csv") {
            r.csv()
        } else {
            let mut t = r.report();
            if let Some(s) = &sres {
                t.push('\n');
                t.push_str(&s.report());
            }
            t
        };
        std::fs::write(o, text)?;
        println!("Wrote {o}");
    }
    if let (Some(m), Some(s)) = (&map_out, &sres) {
        match s.map_svg(&b, &title) {
            Some(text) => {
                std::fs::write(m, text)?;
                println!("Wrote {m}");
            }
            None => println!("No near-field map (board too large or empty)"),
        }
    }
    if r.count(Severity::High) > 0 || sres.as_ref().is_some_and(|s| s.worst.1 < 0.0) {
        std::process::exit(1);
    }
    Ok(())
}

fn flex_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::flex;
    let mut b = board(file)?;
    let mut lim = flex::Limits::default();
    let mut out: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        let next = |i: usize| -> Result<&str, String> {
            rest.get(i + 1)
                .copied()
                .ok_or_else(|| format!("{} needs a value", rest[i]))
        };
        match rest[i] {
            "--keepout" => {
                lim.edge_keepout = next(i)?.parse()?;
                i += 1;
            }
            "--angle" => {
                lim.cross_angle = next(i)?.parse()?;
                i += 1;
            }
            // Give every bend (or one: B1=radius) a radius, mil.
            "--radius" => {
                let v = next(i)?;
                let (name, r) = v.split_once('=').map_or((None, v), |(n, r)| (Some(n), r));
                let r: f64 = r.trim_end_matches("mil").parse()?;
                for bd in b.bends.iter_mut() {
                    if name.is_none_or(|n| n.eq_ignore_ascii_case(&bd.name)) {
                        bd.radius = trace_core::Length::from_mil(r);
                    }
                }
                i += 1;
            }
            "--dynamic" => b.bends.iter_mut().for_each(|bd| bd.dynamic = true),
            x if x.starts_with("--") => return Err(format!("unknown option {x}").into()),
            x => out = Some(x),
        }
        i += 1;
    }
    let issues = flex::check(&b, &lim);
    let text = flex::report(&b, &issues);
    print!("{text}");
    if let Some(o) = out {
        std::fs::write(o, &text)?;
        println!("Wrote {o}");
    }
    Ok(())
}

fn compare_cmd(a: &str, b: &str, rest: &[&str]) -> Res {
    use trace_core::compare::{compare, Options};
    let mut opt = Options::default();
    let mut out: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--tol" => {
                opt.move_tol = rest.get(i + 1).ok_or("--tol needs mil")?.parse()?;
                i += 1;
            }
            "--length-tol" => {
                opt.length_tol = rest.get(i + 1).ok_or("--length-tol needs mil")?.parse()?;
                i += 1;
            }
            "--no-routing" => opt.routing = false,
            x if x.starts_with("--") => return Err(format!("unknown option {x}").into()),
            x => out = Some(x),
        }
        i += 1;
    }
    let (ba, bb) = (board(a)?, board(b)?);
    let c = compare(&ba, &bb, &opt);
    let name = |p: &str| {
        std::path::Path::new(p)
            .file_name()
            .map_or(p.to_string(), |n| n.to_string_lossy().to_string())
    };
    let text = c.report(&name(a), &name(b));
    print!("{text}");
    if let Some(o) = out {
        let body = if o.to_ascii_lowercase().ends_with(".csv") {
            c.csv()
        } else {
            text
        };
        std::fs::write(o, body)?;
        println!("Wrote {o}");
    }
    if c.changes.is_empty() {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

fn bom_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::bom;
    let mut boards = 1u64;
    let mut files: Vec<&str> = Vec::new();
    let mut key = std::env::var("MOUSER_API_KEY").ok();
    let (mut refresh, mut cache, mut template, mut out) = (false, true, None, None);
    let mut i = 0;
    while i < rest.len() {
        let next = |n: &str| rest.get(i + 1).copied().ok_or(format!("{n} needs a value"));
        match rest[i] {
            "--boards" => {
                boards = next("--boards")?.parse()?;
                i += 1;
            }
            "--parts" => {
                files.push(next("--parts")?);
                i += 1;
            }
            "--mouser" => {
                key = Some(next("--mouser")?.to_string());
                i += 1;
            }
            "--template" => {
                template = Some(next("--template")?);
                i += 1;
            }
            "--refresh" => refresh = true,
            "--no-cache" => cache = false,
            x if x.starts_with("--") => return Err(format!("unknown option {x}").into()),
            x => out = Some(x),
        }
        i += 1;
    }
    let b = board(file)?;
    let mut b0 = bom::build(&b);
    let mut db = if cache {
        bom::load_cache()
    } else {
        bom::Db::default()
    };
    for f in &files {
        let text = std::fs::read_to_string(f)?;
        let name = std::path::Path::new(f)
            .file_name()
            .map_or(f.to_string(), |n| n.to_string_lossy().to_string());
        let n = db.add(bom::parts_csv(&text, &name)?);
        eprintln!("{n} part numbers from {f}");
    }
    if let Some(k) = key.filter(|k| !k.trim().is_empty()) {
        let want = bom::to_look_up(&b0, &db, refresh);
        if !want.is_empty() {
            eprintln!("Looking up {} part numbers at Mouser...", want.len());
            let (got, notes) = bom::mouser_fetch(&k, &want, &bom::today(), |d, n| {
                eprintln!("  {d}/{n}");
            })?;
            for n in &notes {
                eprintln!("  {n}");
            }
            let mut fetched = bom::Db::default();
            fetched.add(got);
            if cache {
                let mut c = bom::load_cache();
                c.add(fetched.parts.values().cloned().collect());
                bom::save_cache(&c)?;
            }
            db.add(fetched.parts.into_values().collect());
        }
    }
    bom::check(&mut b0, &db, boards);
    let text = b0.report();
    print!("{text}");
    if let Some(t) = template {
        std::fs::write(t, b0.parts_template())?;
        println!("Wrote {t}");
    }
    if let Some(o) = out {
        let body = if o.to_ascii_lowercase().ends_with(".csv") {
            b0.csv()
        } else {
            text
        };
        std::fs::write(o, body)?;
        println!("Wrote {o}");
    }
    if b0
        .lines
        .iter()
        .any(|l| l.worst() == Some(bom::Level::Error))
    {
        std::process::exit(1)
    }
    Ok(())
}

fn zcheck_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::zcheck::{self, Goal, Rule};
    let b = board(file)?;
    let mut goal = Goal::default();
    let mut out: Option<&str> = None;
    let mut i = 0;
    let rule = |a: &str, diff: bool| -> Result<Rule, String> {
        let (p, z) = a
            .rsplit_once('=')
            .ok_or_else(|| format!("{a}: give PATTERN=OHMS"))?;
        let ohms: f64 = z
            .trim_end_matches(|c: char| c.is_alphabetic())
            .parse()
            .map_err(|_| format!("{a}: bad ohms"))?;
        Ok(Rule {
            pattern: p.to_string(),
            diff,
            ohms,
        })
    };
    while i < rest.len() {
        let next = |i: usize| -> Result<&str, String> {
            rest.get(i + 1)
                .copied()
                .ok_or_else(|| format!("{} needs a value", rest[i]))
        };
        match rest[i] {
            "--tol" => {
                goal.tolerance = next(i)?.trim_end_matches('%').parse::<f64>()? / 100.0;
                i += 1;
            }
            "--rule" => {
                goal.rules.push(rule(next(i)?, false)?);
                i += 1;
            }
            "--diff" => {
                goal.rules.push(rule(next(i)?, true)?);
                i += 1;
            }
            "--no-pairs" => goal.pairs = false,
            "--no-rf" => goal.rf = false,
            "--no-vias" => goal.vias = false,
            "--no-name-pairs" => goal.pairs_by_name = false,
            "--ghz" => {
                goal.ghz = next(i)?.trim_end_matches("GHz").parse()?;
                i += 1;
            }
            "--via-kinds" => {
                goal.via_kinds = next(i)?.parse()?;
                i += 1;
            }
            "--min-length" => {
                goal.min_length = next(i)?.trim_end_matches("mil").parse()?;
                i += 1;
            }
            a if !a.starts_with("--") && out.is_none() => out = Some(a),
            a => return Err(format!("unknown option {a}").into()),
        }
        i += 1;
    }
    let geo = b.layer_geometry();
    let (named, _) = zcheck::with_name_pairs(&b, &goal);
    let nets = zcheck::controlled(named.as_ref().unwrap_or(&b), &goal);
    println!("Controlled nets: {}", nets.len());
    for c in nets.iter().take(40) {
        println!(
            "  {} {:.0} Ω {} ({})",
            c.name,
            c.ohms,
            if c.diff { "differential" } else { "single" },
            c.why
        );
    }
    if nets.len() > 40 {
        println!("  and {} more", nets.len() - 40);
    }
    let t = std::time::Instant::now();
    let last = std::cell::Cell::new(String::new());
    let rep = zcheck::check(&b, &b.stackup, &goal, &|p, what| {
        if what != last.take().as_str() {
            eprintln!("  {:3.0} % {what}", p * 100.0);
        }
        last.set(what.to_string());
    });
    println!("{} ({:.0} s)", rep.summary(), t.elapsed().as_secs_f64());
    let fails: Vec<_> = rep.failing().collect();
    if !fails.is_empty() {
        println!("Off target:");
        for f in &fails {
            println!("  {}", f.describe(&geo));
        }
    }
    let short: Vec<_> = rep
        .findings
        .iter()
        .filter(|f| f.short && f.off().abs() > rep.tolerance)
        .collect();
    if !short.is_empty() {
        println!(
            "Off but electrically short (under a twentieth of a wavelength at {} GHz):",
            goal.ghz
        );
        for f in &short {
            println!("  {}", f.describe(&geo));
        }
    }
    let ok: Vec<_> = rep
        .findings
        .iter()
        .filter(|f| f.off().abs() <= rep.tolerance)
        .collect();
    if !ok.is_empty() {
        println!("On target: {}", ok.len());
        for f in ok.iter().take(20) {
            println!("  {}", f.describe(&geo));
        }
    }
    if rep.vias_skipped.0 > 0 {
        println!(
            "Not solved: {} more via kind(s), {} via(s) (--via-kinds to raise)",
            rep.vias_skipped.0, rep.vias_skipped.1
        );
    }
    for n in &rep.notes {
        println!("Note: {n}");
    }
    if let Some(o) = out {
        std::fs::write(o, rep.csv(&geo))?;
        println!("Wrote {o}");
    }
    Ok(())
}

fn via(file: &str, from: &str, to: &str, rest: &[&str]) -> Res {
    use trace_core::fdtd::{self, ViaSetup};
    let b = board(file)?;
    let geo = b.layer_geometry();
    let layer = |l: &str| -> Result<u8, String> {
        l.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&n| n < geo.len())
            .or_else(|| geo.iter().position(|g| g.name.eq_ignore_ascii_case(l)))
            .map(|n| n as u8)
            .ok_or_else(|| {
                format!(
                    "no copper layer {l}; layers: {}",
                    geo.iter()
                        .map(|g| g.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    };
    let mut v = ViaSetup::new(layer(from)?, layer(to)?);
    if let Some((d, p, a)) = fdtd::board_via(&b) {
        v.drill = d;
        v.pad = p;
        v.antipad = a;
    }
    let mut outs: Vec<&str> = Vec::new();
    let mut optimise: Option<f64> = None;
    let mut free_pitch = false;
    let (mut try_backdrill, mut keep_pad, mut check) = (false, false, false);
    let mut accurate: Option<fdtd::Tolerance> = None;
    let mut quick = false;
    let mut fab: Option<trace_core::viaopt::FabLimits> = None;
    let mut target: Option<f64> = None;
    let mut feed: Option<f64> = None;
    // Field maps: output, dB depth, plane, quantity (for an animation).
    let mut field_out: Option<String> = None;
    let mut field_db = 40.0;
    let mut field_plane: Option<String> = None;
    // openEMS: compare, export only, its folder, program, cell, or
    // read a finished run.
    let mut oems_check = false;
    let mut fem_check = false;
    let mut fem_dof: Option<usize> = None;
    let mut oems_export: Option<String> = None;
    let mut oems_dir: Option<String> = None;
    let mut oems_read: Option<String> = None;
    let mut oems_exe: Option<String> = None;
    let mut oems_cell = 0.0;
    let mut field_q = trace_core::emfield::Quantity::E;
    let mut it = rest.iter();
    let len = |x: Option<&&str>, what: &str| -> Result<f64, Box<dyn std::error::Error>> {
        let x = x.ok_or(format!("{what} needs a length"))?;
        Ok(Length::parse(x).ok_or(format!("not a length: {x}"))?.mil())
    };
    while let Some(&a) = it.next() {
        match a {
            "--drill" => v.drill = len(it.next(), a)?,
            "--pad" => v.pad = len(it.next(), a)?,
            "--antipad" => v.antipad = len(it.next(), a)?,
            "--width" => v.w_from = Some(len(it.next(), a)?),
            "--width2" => v.w_to = Some(len(it.next(), a)?),
            "--pair" => v.pair = Some(len(it.next(), a)?),
            "--cell" => v.cell = len(it.next(), a)?,
            "--backdrill" => v.backdrill = true,
            "--oval" => v.antipad_oval = true,
            "--free-pitch" => free_pitch = true,
            "--try-backdrill" => try_backdrill = true,
            "--keep-pad" => keep_pad = true,
            "--check" if it.clone().next() == Some(&"fem") => {
                it.next();
                fem_check = true;
            }
            "--fem-dof" => {
                let x = it.next().ok_or("--fem-dof needs a count")?;
                fem_dof = Some(
                    x.parse::<usize>()
                        .map_err(|_| format!("not a count: {x}"))?,
                );
            }
            "--check" if it.clone().next() == Some(&"openems") => {
                it.next();
                oems_check = true;
            }
            "--export-openems" => {
                oems_export = Some(
                    it.next()
                        .ok_or("--export-openems needs a folder")?
                        .to_string(),
                )
            }
            "--openems-dir" => {
                oems_dir = Some(it.next().ok_or("--openems-dir needs a folder")?.to_string())
            }
            "--openems-read" => {
                oems_read = Some(
                    it.next()
                        .ok_or("--openems-read needs a folder")?
                        .to_string(),
                );
                oems_check = true;
            }
            "--openems-exe" => {
                oems_exe = Some(it.next().ok_or("--openems-exe needs a path")?.to_string())
            }
            "--openems-cell" => oems_cell = len(it.next(), a)?,
            "--check" => check = true,
            "--quick" => quick = true,
            "--accurate" => {
                let mut t = fdtd::Tolerance::default();
                // An optional tolerance: --accurate 0.02 (|S21| dB).
                if let Some(x) = it.clone().next().and_then(|x| x.parse::<f64>().ok()) {
                    it.next();
                    t.s21_db = x;
                    t.s11_db = (x * 20.0).max(0.25);
                }
                accurate = Some(t);
            }
            "--lossless" => v.metal_loss = false,
            "--conformal" => v.conformal = true,
            "--staircase" => v.conformal = false,
            "--rough" => {
                let x = it.next().ok_or("--rough needs um")?;
                v.rough_um = x
                    .trim_end_matches("um")
                    .parse::<f64>()
                    .map_err(|_| format!("not a roughness: {x}"))?;
            }
            "--feed" => feed = Some(len(it.next(), a)?),
            "--er" | "--df" => {
                let x = it.next().ok_or(format!("{a} needs a value"))?;
                let v_: f64 = x.parse().map_err(|_| format!("not a number: {x}"))?;
                if a == "--er" {
                    v.er = Some(v_);
                } else {
                    v.df = Some(v_);
                }
            }
            "--target" => {
                let x = it.next().ok_or("--target needs ohms")?;
                target = Some(
                    x.trim_end_matches(['o', 'h', 'm', 'Ω', 'O', 'H', 'M'])
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("not an impedance: {x}"))?,
                );
            }
            "--fab" => {
                // drill=8-20,ring=4,antipad=20-80,gap=4 (mil)
                let x = it.next().ok_or("--fab needs limits")?;
                let mut f = fab.unwrap_or_default();
                for part in x.split(',') {
                    let (k, val) = part
                        .split_once('=')
                        .ok_or(format!("--fab: {part}: not key=value"))?;
                    let n = |t: &str| -> Result<f64, String> {
                        Length::parse(t.trim())
                            .map(|l| l.mil())
                            .or_else(|| t.trim().parse().ok())
                            .ok_or(format!("--fab: not a length: {t}"))
                    };
                    let range = |t: &str| -> Result<(f64, f64), String> {
                        match t.split_once('-') {
                            Some((a, b)) => Ok((n(a)?, n(b)?)),
                            None => n(t).map(|v| (v, v)),
                        }
                    };
                    match k.trim() {
                        "drill" => f.drill = range(val)?,
                        "ring" => f.ring = n(val)?,
                        "antipad" => f.antipad = range(val)?,
                        "gap" | "clearance" => f.clearance = n(val)?,
                        _ => return Err(format!("--fab: unknown {k}").into()),
                    }
                }
                fab = Some(f);
            }
            "--optimise" | "--optimize" => {
                let x = it.next().ok_or("--optimise needs GHz")?;
                optimise = Some(
                    x.trim_end_matches(['G', 'g', 'H', 'h', 'z'])
                        .parse::<f64>()
                        .map_err(|_| format!("not a frequency: {x}"))?,
                );
            }
            "--returns" => {
                let x = it.next().ok_or("--returns needs N or N@distance")?;
                let (n, d) = x.split_once('@').unwrap_or((x, ""));
                v.returns = n.parse().map_err(|_| format!("not a count: {n}"))?;
                if !d.is_empty() {
                    v.return_dist = Length::parse(d).ok_or(format!("not a length: {d}"))?.mil();
                }
            }
            "--fields" => {
                // --fields 6[,10]GHz out.svg|out.gif
                let x = it.next().ok_or("--fields needs GHz")?;
                for f in x.trim_end_matches(['G', 'g', 'H', 'h', 'z']).split(',') {
                    v.fields.push(
                        f.trim()
                            .parse()
                            .map_err(|_| format!("not a frequency: {f}"))?,
                    );
                }
                field_out = Some(
                    it.next()
                        .ok_or("--fields needs an output (.svg or .gif)")?
                        .to_string(),
                );
            }
            "--field-db" => {
                let x = it.next().ok_or("--field-db needs dB")?;
                field_db = x
                    .trim_end_matches("dB")
                    .parse()
                    .map_err(|_| format!("not a number: {x}"))?;
            }
            "--field-plane" => {
                field_plane = Some(
                    it.next()
                        .ok_or("--field-plane needs a name or number")?
                        .to_string(),
                )
            }
            "--field-quantity" => {
                let x = it
                    .next()
                    .ok_or("--field-quantity needs E, H, energy or current")?;
                field_q = trace_core::emfield::Quantity::parse(x)
                    .ok_or(format!("not a quantity: {x} (E, H, energy, current)"))?;
            }
            "--fmax" => {
                let x = it.next().ok_or("--fmax needs GHz")?;
                v.fmax_ghz = x
                    .trim_end_matches(['G', 'g', 'H', 'h', 'z'])
                    .parse()
                    .map_err(|_| format!("not a frequency: {x}"))?;
            }
            _ if !a.starts_with("--") => outs.push(a),
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    // Materials given stand for the stack-up's everywhere (traces too).
    let (b, geo) = if v.er.is_some() || v.df.is_some() {
        let mut b = b;
        b.stackup = fdtd::materials(&b.stackup, v.er, v.df);
        let geo = b.layer_geometry();
        (b, geo)
    } else {
        (b, geo)
    };
    let names = |l: u8| geo[l as usize].name.clone();
    if let Some((er, df)) = fdtd::materials_between(&b.stackup, v.from, v.to) {
        println!(
            "Dielectric between the layers: Er {er:.2}, Df {df:.4}{}; copper {}",
            if v.er.is_some() || v.df.is_some() {
                " (as given)"
            } else {
                " (stack-up)"
            },
            if v.metal_loss {
                format!("loss on, {:.1} um rough", v.rough_um)
            } else {
                "perfect (--lossless)".into()
            }
        );
    }
    println!(
        "Via {} to {}{}: drill {:.1} mil, pad {:.1}, antipad {:.1}, {} return via(s) at {:.1} mil{}",
        names(v.from),
        names(v.to),
        v.pair.map_or(String::new(), |p| format!(", pair at {p:.1} mil pitch")),
        v.drill,
        v.pad,
        v.antipad,
        v.returns,
        v.return_dist,
        if v.backdrill { ", backdrilled" } else { "" }
    );
    if let Some(zt) = target {
        use trace_core::viaopt;
        let goal = match &fab {
            Some(f) => viaopt::ViaGoal::within(&v, v.fmax_ghz, f),
            None => viaopt::ViaGoal::around(&v, v.fmax_ghz),
        };
        let mut goal = goal;
        goal.accurate = if quick {
            None
        } else {
            Some(accurate.unwrap_or_default())
        };
        let t = std::time::Instant::now();
        let sz = viaopt::size_to(&b.stackup, &v, zt, &goal, &|p, what| {
            eprint!("\r  sizing… {:3.0}% {what:<44}", p * 100.0);
        })?;
        eprintln!("\r  sized in {:.0} s{:60}", t.elapsed().as_secs_f64(), "");
        if !sz.accurate.is_empty() {
            println!("Quick-grid solves (they steer the search):");
        }
        for (s, z) in &sz.steps {
            println!(
                "  {:>8}  {}",
                z.map_or("?".into(), |z| format!("{z:.1} Ω")),
                viaopt::describe(s)
            );
        }
        for (k, (s, z, verdict)) in sz.accurate.iter().enumerate() {
            println!(
                "Accurate solve {}: {} at {}\n{verdict}",
                k + 1,
                z.map_or("?".into(), |z| format!("{z:.1} Ω")),
                viaopt::describe(s)
            );
        }
        println!(
            "{}: {} gives {:.1} Ω (target {zt:.0} Ω)",
            if sz.reached { "Sized" } else { "Closest" },
            viaopt::describe(&sz.setup),
            sz.result.z_via.unwrap_or(f64::NAN)
        );
        if let Some(n) = &sz.note {
            println!("  {n}");
        }
        if optimise.is_none() {
            let title = format!("Via {} to {} ({zt:.0} Ω)", names(v.from), names(v.to));
            for out in &outs {
                let text = if out.to_ascii_lowercase().ends_with(".svg") {
                    sz.result.svg(&title)
                } else {
                    sz.result.touchstone(&title)
                };
                std::fs::write(out, text)?;
                println!("Wrote {out}");
            }
            return Ok(());
        }
        v = sz.setup.clone();
    }
    if let Some(fg) = optimise {
        use trace_core::viaopt;
        let mut goal = match &fab {
            Some(f) => viaopt::ViaGoal::within(&v, fg, f),
            None => viaopt::ViaGoal::around(&v, fg),
        };
        goal.try_backdrill = try_backdrill;
        goal.accurate = if quick {
            None
        } else {
            Some(accurate.unwrap_or_default())
        };
        if keep_pad {
            goal.pad = (v.pad, v.pad);
        }
        if free_pitch {
            if let Some(p) = v.pair {
                let pad = v.pad.max(v.drill);
                goal.pitch = ((pad + goal.clearance).min(p), (2.5 * pad).max(p));
            }
        }
        let t = std::time::Instant::now();
        let o = viaopt::optimise(&b.stackup, &v, &goal, &|p, what| {
            eprint!("\r  optimising… {:3.0}% {what:<40}", p * 100.0);
        })?;
        eprintln!(
            "\r  optimised in {:.0} s{:50}",
            t.elapsed().as_secs_f64(),
            ""
        );
        println!("Setups tried (worst return loss to {fg} GHz, through loss at {fg} GHz):");
        for tr in &o.trials {
            match &tr.error {
                None => println!(
                    "  {:>7.1} dB {:>7.3} dB  {}",
                    tr.worst_rl,
                    tr.il,
                    viaopt::describe(&tr.setup)
                ),
                Some(e) => println!(
                    "  {:>21}  {}: {e}",
                    "not buildable",
                    viaopt::describe(&tr.setup)
                ),
            }
        }
        println!(
            "Front runners solved again {} (worst return loss to {fg} GHz):",
            if goal.accurate.is_some() {
                "accurately, on grids refined to convergence"
            } else {
                "on a finer grid"
            }
        );
        for (k, (setup, rl)) in o.confirmed.iter().enumerate() {
            println!("  {rl:>7.1} dB  {}", viaopt::describe(setup));
            if let Some(v) = o.verdicts.get(k) {
                println!("{v}");
            }
        }
        let (s0, _) = viaopt::judge(&o.start, fg);
        let (s1, il) = viaopt::judge(&o.best, fg);
        println!(
            "Best: {}\n  worst return loss {s1:.1} dB (was {s0:.1} dB), {il:.3} dB through at {fg} GHz",
            viaopt::describe(&o.best_setup)
        );
        let title = format!("Via {} to {} (optimised)", names(v.from), names(v.to));
        for out in outs {
            let text = if out.to_ascii_lowercase().ends_with(".svg") {
                o.best.svg(&title)
            } else {
                o.best.touchstone(&title)
            };
            std::fs::write(out, text)?;
            println!("Wrote {out}");
        }
        return Ok(());
    }
    if let Some(dir) = &oems_export {
        use trace_core::openems;
        let s = openems::setup_for(&b.stackup, &v, oems_cell)?;
        let ex = openems::export(&b.stackup, &s, Path::new(dir))?;
        println!(
            "Wrote {} for openEMS ({} cells, smallest {:.2} mil): run openEMS {} in that folder, then --openems-read {dir}",
            ex.xml.display(),
            ex.cells,
            ex.cell,
            openems::MODEL
        );
        return Ok(());
    }
    let t = std::time::Instant::now();
    let last = std::cell::Cell::new(-1i32);
    let progress = |p: f64| {
        let pc = (p * 100.0) as i32 / 10 * 10;
        if pc > last.get() {
            last.set(pc);
            eprint!("\r  solving… {pc}%");
        }
    };
    let r = if let Some(tol) = &accurate {
        let c = fdtd::converge(&b.stackup, &v, tol, &progress)?;
        eprintln!();
        println!("Accurate solve:\n{}", c.verdict());
        c.best
    } else if check {
        let mc = fdtd::check_mesh(&b.stackup, &v, &progress)?;
        eprintln!();
        println!("Grid check: {}", mc.verdict());
        mc.fine
    } else {
        fdtd::solve_with(&b.stackup, &v, &progress)?
    };
    eprintln!(
        "\r  solved: {} cells, {} steps, {:.1} s",
        r.cells,
        r.steps,
        t.elapsed().as_secs_f64()
    );
    println!(
        "Traces: {:.2} mil ({:.1} ohm) in, {:.2} mil ({:.1} ohm) out; stub {:.1} mil{}",
        r.w[0],
        r.z0[0],
        r.w[1],
        r.z0[1],
        r.stub,
        if r.stub > 0.5 {
            format!(
                " (bare quarter-wave {:.1} GHz)",
                fdtd::stub_resonance_ghz(r.stub, 3.8)
            )
        } else {
            String::new()
        }
    );
    if let Some(z) = r.z_via {
        println!("Via impedance: {z:.1} ohm (as a TDR would show it)");
    }
    let kind = if r.z_ref > 60.0 { "Sdd" } else { "S" };
    println!(
        "  {:>7}  {:>9}  {:>9}  {:>9}",
        "GHz",
        format!("{kind}11 dB"),
        format!("{kind}21 dB"),
        "deg"
    );
    let n = r.f.len();
    let ranged = r.err11.len() == n && r.err21.len() == n;
    for k in (0..n).filter(|k| (k + 1) % (n / 16).max(1) == 0) {
        println!(
            "  {:>7.2}  {:>9.2}  {:>9.3}  {:>9.1}{}",
            r.f[k],
            r.s11[k].db(),
            r.s21[k].db(),
            r.s21[k].deg(),
            if ranged {
                let (_, a, b) = fdtd::db_range(r.s11[k], r.err11[k]);
                let (_, c, d) = fdtd::db_range(r.s21[k], r.err21[k]);
                format!("   S11 {a:.1} to {b:.1}, S21 {c:.3} to {d:.3}")
            } else {
                String::new()
            }
        );
    }
    println!("Physics check: {}", fdtd::quality(&r).summary());
    if let Some((f, db)) = r.notch() {
        println!("Stub resonance: {kind}21 dips to {db:.1} dB at {f:.2} GHz");
    }
    let worst = (0..n)
        .max_by(|&a, &b| r.s11[a].abs().total_cmp(&r.s11[b].abs()))
        .unwrap_or(0);
    println!(
        "Worst return loss {:.1} dB at {:.2} GHz",
        r.s11[worst].db(),
        r.f[worst]
    );
    for note in &r.notes {
        println!("Note: {note}");
    }
    if let Some(len_mil) = feed {
        // The via with `len_mil` of its own traces each side: the traces'
        // loss from the 2D field solver (per leg for a pair).
        let lines: Vec<Option<trace_core::field::Line>> = [(v.from, r.w[0]), (v.to, r.w[1])]
            .iter()
            .map(|&(l, w)| {
                geo.get(l as usize)
                    .and_then(|g| trace_core::field::single(g, w))
            })
            .collect();
        println!(
            "With {:.0} mil ({:.2} in) of trace each side ({kind}21, dB):",
            len_mil,
            len_mil / 1000.0
        );
        println!(
            "  {:>7}  {:>9}  {:>9}  {:>9}",
            "GHz", "via", "traces", "total"
        );
        for k in (0..n).filter(|k| (k + 1) % (n / 8).max(1) == 0) {
            let f = r.f[k] * 1e9;
            let tr: f64 = lines
                .iter()
                .flatten()
                .map(|l| l.db_per_in(f, 0.02, Some(v.rough_um)) * len_mil / 1000.0)
                .sum();
            let via = r.s21[k].db();
            println!(
                "  {:>7.2}  {:>9.3}  {:>9.3}  {:>9.3}",
                r.f[k],
                via,
                -tr,
                via - tr
            );
        }
    }
    let title = format!(
        "{} via, {} to {}",
        Path::new(file)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned()),
        names(v.from),
        names(v.to)
    );
    if let Some(out) = &field_out {
        use trace_core::emfield::Quantity;
        let many = r.fields.len() > 1;
        for m in &r.fields {
            let path = if many {
                let (stem, ext) = out.rsplit_once('.').unwrap_or((out.as_str(), "svg"));
                format!("{stem}_{:.2}GHz.{ext}", m.f_ghz)
            } else {
                out.clone()
            };
            if path.to_ascii_lowercase().ends_with(".gif") {
                let k = match &field_plane {
                    Some(p) => p
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .filter(|&n| n < m.planes.len())
                        .or_else(|| {
                            let p = p.to_ascii_lowercase();
                            m.planes
                                .iter()
                                .position(|q| q.name.to_ascii_lowercase().contains(&p))
                        })
                        .ok_or(format!(
                            "no plane {p}; planes: {}",
                            m.planes
                                .iter()
                                .enumerate()
                                .map(|(i, q)| format!("{} {}", i + 1, q.name))
                                .collect::<Vec<_>>()
                                .join("; ")
                        ))?,
                    None => m
                        .planes
                        .iter()
                        .position(|q| matches!(q.cut, trace_core::emfield::Cut::Xz { .. }))
                        .unwrap_or(0),
                };
                std::fs::write(&path, m.gif(k, field_q, field_db, 24))?;
                println!(
                    "Saved {path}: {} of {} through one cycle at {:.2} GHz",
                    field_q.label(),
                    m.planes[k].name,
                    m.f_ghz
                );
            } else {
                std::fs::write(&path, m.svg(&Quantity::ALL, field_db))?;
                println!(
                    "Saved {path}: field maps at {:.2} GHz ({} planes), per 1 W in",
                    m.f_ghz,
                    m.planes.len()
                );
            }
            for (i, p) in m.planes.iter().enumerate() {
                let peak = |q: Quantity| {
                    p.values(q, None)
                        .into_iter()
                        .filter(|v| v.is_finite())
                        .fold(0.0f64, f64::max)
                };
                println!(
                    "  plane {}: {:<48} |E| {:>9} V/m  |H| {:>9} A/m  J {:>9} A/m",
                    i + 1,
                    p.name,
                    trace_core::emfield::si(peak(Quantity::E)),
                    trace_core::emfield::si(peak(Quantity::H)),
                    trace_core::emfield::si(peak(Quantity::Current))
                );
            }
        }
    }
    for o in outs {
        let low = o.to_ascii_lowercase();
        if low.ends_with(".svg") {
            std::fs::write(o, r.svg(&title))?;
        } else {
            std::fs::write(o, r.touchstone(&title))?;
        }
        println!("Saved {o}");
    }
    if fem_check {
        fem_compare(&b.stackup, &v, &r, fem_dof)?;
    }
    if oems_check {
        openems_compare(
            &b.stackup,
            &v,
            &r,
            &title,
            file,
            oems_cell,
            oems_dir.as_deref(),
            oems_read.as_deref(),
            oems_exe.as_deref(),
        )?;
    }
    Ok(())
}

/// Solves the via `v` with the 3D FEM solver (adaptive, second order) and
/// compares it with the FDTD result `r`.
fn fem_compare(
    stackup: &trace_core::Stackup,
    v: &trace_core::fdtd::ViaSetup,
    r: &trace_core::fdtd::ViaResult,
    max_dof: Option<usize>,
) -> Res {
    use trace_core::fem3d;
    let model = fem3d::via(stackup, v)?;
    if std::env::var_os("TRACE_FEM_DEBUG").is_some() {
        eprintln!("{model:#?}");
    }
    let fmax = v.fmax_ghz;
    let setup = fem3d::Setup {
        f_adapt: fmax,
        sweep: (1..=10).map(|k| fmax * k as f64 / 10.0).collect(),
        z_ref: Some(r.z_ref),
        max_unknowns: max_dof.unwrap_or(400_000),
        ..fem3d::Setup::default()
    };
    println!("3D FEM check (HFSS-style: adaptive tetrahedra, second-order elements, wave ports):");
    let t0 = std::time::Instant::now();
    let s = fem3d::solve(&model, &setup, |l| {
        eprintln!("  {l}");
        true
    })?;
    println!(
        "  {} after {} passes, {} tetrahedra, {} unknowns, {:.0} s; port Z0 {:.1} / {:.1} ohm",
        if s.converged {
            "converged"
        } else {
            "not converged"
        },
        s.passes.len(),
        s.tets,
        s.unknowns,
        t0.elapsed().as_secs_f64(),
        s.info[0][0].z0,
        s.info[0][1].z0
    );
    println!("  GHz     |S11| FDTD  FEM (dB)     |S21| FDTD  FEM (dB)");
    let mut d11: f64 = 0.0;
    let mut d21: f64 = 0.0;
    for (k, f) in s.f.iter().enumerate() {
        let i =
            r.f.iter()
                .enumerate()
                .min_by(|a, b| (a.1 - f).abs().total_cmp(&(b.1 - f).abs()))
                .map(|x| x.0)
                .unwrap_or(0);
        let db = |x: f64| 20.0 * x.max(1e-9).log10();
        let (a11, a21) = (r.s11[i].abs(), r.s21[i].abs());
        let (b11, b21) = (s.s[k][0].norm(), s.s[k][2].norm());
        d11 = d11.max((db(a11) - db(b11)).abs());
        d21 = d21.max((db(a21) - db(b21)).abs());
        println!(
            "  {:6.2}  {:8.2} {:8.2}        {:7.3} {:7.3}",
            f,
            db(a11),
            db(b11),
            db(a21),
            db(b21)
        );
    }
    println!("  largest difference: S11 {d11:.2} dB, S21 {d21:.3} dB (FEM copper is lossless)");
    Ok(())
}

/// Runs (or reads) openEMS on the via `v` and compares it with Trace's `r`.
#[allow(clippy::too_many_arguments)]
fn openems_compare(
    stackup: &trace_core::Stackup,
    v: &trace_core::fdtd::ViaSetup,
    r: &trace_core::fdtd::ViaResult,
    title: &str,
    file: &str,
    cell: f64,
    dir: Option<&str>,
    read: Option<&str>,
    exe: Option<&str>,
) -> Res {
    use trace_core::openems;
    let default_dir = format!(
        "{}_openems",
        Path::new(file)
            .file_stem()
            .map_or("via".into(), |s| s.to_string_lossy().into_owned())
    );
    let dir = PathBuf::from(read.or(dir).unwrap_or(&default_dir));
    let t = std::time::Instant::now();
    let o = if read.is_some() {
        let s = openems::setup_for(stackup, v, cell)?;
        openems::read(stackup, &s, &dir)?
    } else {
        let exe = openems::find_exe(exe.map(Path::new)).ok_or(
            "openEMS not found: install it (C:\\openEMS) or give --openems-exe PATH, \
             or set TRACE_OPENEMS",
        )?;
        println!("openEMS: {} in {}", exe.display(), dir.display());
        let last = std::cell::Cell::new(-1i32);
        let (o, _) = openems::check(stackup, v, cell, &dir, &exe, &|p| {
            let pc = (p * 100.0) as i32 / 5 * 5;
            if pc > last.get() {
                last.set(pc);
                eprint!("\r  openEMS running… {pc}%");
            }
        })?;
        eprintln!();
        o
    };
    println!(
        "openEMS: {:.0} s, {} cells, smallest cell {:.2} mil",
        t.elapsed().as_secs_f64(),
        o.cells,
        o.cell
    );
    for n in &o.notes {
        println!("  {n}");
    }
    if let Some(z) = o.z_via {
        println!("openEMS via impedance: {z:.1} ohm");
    }
    print!("{}", openems::table(r, &o, 12, &[6.0]));
    let c = openems::compare(r, &o);
    println!("Trace vs openEMS: {}", c.summary());
    std::fs::create_dir_all(&dir)?;
    let save = |name: &str, text: String| -> Res {
        let p = dir.join(name);
        std::fs::write(&p, text)?;
        println!("Saved {}", p.display());
        Ok(())
    };
    save(
        "compare.svg",
        openems::svg(r, &o, &format!("{title}: Trace vs openEMS")),
    )?;
    save("trace.s2p", r.touchstone(title))?;
    save("openems.s2p", o.touchstone(&format!("{title} (openEMS)")))?;
    save(
        "compare.txt",
        format!(
            "{title}\nTrace vs openEMS: {}\n{}",
            c.summary(),
            openems::table(r, &o, 40, &[6.0])
        ),
    )?;
    Ok(())
}

fn check(file: &str) -> Res {
    let b = board(file)?;
    let set = b.imported_constraints();
    if set.groups.is_empty() {
        println!("No length, matched-length or skew rules found.");
        return Ok(());
    }
    let mil = |l: trace_core::Length| format!("{:.1}", l.mil());
    let opt = |l: Option<trace_core::Length>| l.map_or("-".to_string(), mil);
    for g in &set.groups {
        let r = b.check(g);
        println!(
            "{}  [{}]  {} nets, {} failing{}",
            g.name,
            g.members.describe(),
            r.nets.len(),
            r.failing,
            if g.enabled { "" } else { "  (disabled)" }
        );
        let l = &g.limits;
        println!(
            "  limits (mil): min {} max {} match {} skew {} vias {}",
            opt(l.min_length),
            opt(l.max_length),
            opt(l.match_tolerance),
            opt(l.max_skew),
            l.max_vias.map_or("-".into(), |v| v.to_string())
        );
        for n in r.nets.iter().filter(|n| !n.problems.is_empty()).take(20) {
            let what: Vec<&str> = n.problems.iter().map(|p| p.label()).collect();
            println!(
                "    {:<28} {:>9} mil  skew {:>7}  delta {:>7}  {}",
                clip(&n.name, 28),
                mil(n.length),
                opt(n.skew),
                opt(n.match_delta),
                what.join(", ")
            );
        }
    }
    Ok(())
}

fn view_board(file: &str, out: Option<&str>) -> Res {
    let b = board(file)?;
    let name = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = match out {
        Some(o) => std::path::PathBuf::from(o),
        None => Path::new(file).with_extension("trace.html"),
    };
    std::fs::write(&out, view::render(&b, &name))?;
    println!("wrote {}", out.display());
    view::open(&out);
    Ok(())
}

fn fab(file: &str, out: &str) -> Res {
    let b = board(file)?;
    let opt = trace_export::FabOptions::from_board(&b);
    let path = Path::new(out);
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let files = trace_export::fab_files(&b, &stem, &opt);
    println!(
        "mask expansion {:.3} mm, paste expansion {:.3} mm, vias {}",
        opt.mask_expansion.mm(),
        opt.paste_expansion.mm(),
        match (opt.tent_top, opt.tent_bottom) {
            (true, true) => "tented",
            (false, false) => "open both sides",
            (true, false) => "tented on top only",
            (false, true) => "tented on the bottom only",
        }
    );
    if out.to_ascii_lowercase().ends_with(".zip") {
        std::fs::write(path, trace_export::fab_zip(&files)?)?;
    } else {
        std::fs::create_dir_all(path)?;
        for f in &files {
            std::fs::write(path.join(&f.name), &f.data)?;
        }
    }
    for f in &files {
        println!("  {:<40} {:>9} bytes", f.name, f.data.len());
    }
    println!("wrote {} files to {out}", files.len());
    Ok(())
}

/// The assembly drawing as a PDF.
fn assembly(file: &str, out: &str, paper: trace_export::Paper) -> Res {
    let b = board(file)?;
    let title = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let pdf = trace_export::assembly_pdf(&b, &title, paper);
    std::fs::write(out, &pdf)?;
    let side = |s| b.components.iter().filter(|c| c.side == s).count();
    println!(
        "Wrote {out}: {} parts on top, {} on the bottom, {} KB",
        side(trace_core::Side::Top),
        side(trace_core::Side::Bottom),
        pdf.len().div_ceil(1024)
    );
    Ok(())
}

/// The board and its parts as a STEP model.
fn step_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    let mut opt = trace_export::StepOptions::default();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--vias" => opt.vias = true,
            "--no-parts" => opt.parts = false,
            "--min-hole" => {
                i += 1;
                let v = rest.get(i).ok_or("--min-hole needs a size in mm")?;
                opt.min_hole = v
                    .trim_end_matches("mm")
                    .parse()
                    .map_err(|_| format!("bad --min-hole {v}"))?;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let b = board(file)?;
    let name = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let (text, r) = trace_export::to_step(&b, &name, &opt)?;
    std::fs::write(out, &text)?;
    println!(
        "Wrote {out}: board {:.2} mm thick, {} holes, {} cut-outs, {} parts, {} KB",
        r.thickness,
        r.holes,
        r.cutouts,
        r.parts,
        text.len().div_ceil(1024)
    );
    if r.skipped > 0 {
        println!(
            "  {} holes left out: they cross the board edge, a cut-out or another hole",
            r.skipped
        );
    }
    Ok(())
}

/// The fabrication drawing as a PDF.
fn fab_drawing(file: &str, out: &str, paper: trace_export::Paper) -> Res {
    let b = board(file)?;
    let title = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let pdf = trace_export::fab_drawing_pdf(&b, &title, paper);
    std::fs::write(out, &pdf)?;
    let holes = trace_export::fabdraw::drill_table(&b);
    println!(
        "Wrote {out}: {} hole sizes, {} holes, {} KB",
        holes.len(),
        holes.iter().map(|d| d.count).sum::<usize>(),
        pdf.len().div_ceil(1024)
    );
    Ok(())
}

fn odb(file: &str, out: &str) -> Res {
    let b = board(file)?;
    let opt = trace_export::FabOptions::from_board(&b);
    let path = Path::new(out);
    let job = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    let job = job
        .strip_suffix(".tgz")
        .or_else(|| job.strip_suffix(".TGZ"))
        .unwrap_or(&job)
        .to_string();
    let files = trace_export::odb_files(&b, &job, &opt);
    if out.to_ascii_lowercase().ends_with(".tgz") {
        std::fs::write(path, trace_export::odb_tgz(&files)?)?;
    } else {
        // The job's own folder is the one named.
        for f in &files {
            let rel = f.name.split_once('/').map(|r| r.1).unwrap_or(&f.name);
            let to = path.join(rel);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(to, &f.data)?;
        }
    }
    let layers = files
        .iter()
        .filter(|f| f.name.ends_with("/features"))
        .count();
    let bytes: usize = files.iter().map(|f| f.data.len()).sum();
    println!(
        "wrote ODB++ job {job} to {out}: {} files, {layers} feature layers, {} parts, {} nets, {bytes} bytes",
        files.len(),
        b.components.len(),
        b.nets.len()
    );
    Ok(())
}

fn convert(file: &str, out: &str) -> Res {
    let lower = out.to_ascii_lowercase();
    if lower.ends_with(".step") || lower.ends_with(".stp") {
        return step_cmd(file, out, &[]);
    }
    if out.to_ascii_lowercase().ends_with(".xml") || out.to_ascii_lowercase().ends_with(".cvg") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as IPC-2581".into());
        };
        let name = Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "board".into());
        let (text, n) = trace_export::to_ipc2581(&b, &name);
        std::fs::write(out, text)?;
        println!("wrote {out}: IPC-2581 revision C, {n} features");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".dxf") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .dxf".into());
        };
        let (text, n) = trace_export::to_dxf(&b);
        std::fs::write(out, text)?;
        println!("wrote {out}: {n} objects");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .kicad_pcb".into());
        };
        std::fs::write(out, trace_export::to_kicad(&b))?;
        println!("wrote {out}");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".svg") {
        let Design::Sheet(sh) = load(file)? else {
            return Err("only schematic sheets can be drawn as .svg".into());
        };
        std::fs::write(out, trace_export::sheet_svg(&sh))?;
        println!("wrote {out}");
        return Ok(());
    }
    if file.to_ascii_lowercase().ends_with(".tracesch") {
        // A Trace schematic: every sheet, as Altium sheets in folder `out`.
        let f = trace_core::SchFile::load(Path::new(file))?;
        let from = Path::new(file).parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(out)?;
        for s in &f.sheets {
            let src = from.join(&s.file);
            let stem = Path::new(&s.file)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let original = src.is_file() && s.file.to_ascii_lowercase().ends_with(".schdoc");
            let bytes = trace_altium::write_schdoc(&s.sheet, original.then_some(src.as_path()))?;
            let to = Path::new(out).join(format!("{stem}.SchDoc"));
            std::fs::write(&to, bytes)?;
            println!("wrote {}", to.display());
        }
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".schdoc") {
        let Design::Sheet(sh) = load(file)? else {
            return Err("only schematic sheets can be written as .SchDoc".into());
        };
        let original = file.to_ascii_lowercase().ends_with(".schdoc");
        let bytes = trace_altium::write_schdoc(&sh, original.then(|| std::path::Path::new(file)))?;
        std::fs::write(out, bytes)?;
        println!("wrote {out}");
        return Ok(());
    }
    if out.to_ascii_lowercase().ends_with(".pcbdoc") {
        let Design::Board(b) = load(file)? else {
            return Err("only boards can be written as .PcbDoc".into());
        };
        let name = std::path::Path::new(out)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if file.to_ascii_lowercase().ends_with(".pcbdoc") {
            // Saved over the original's records: nothing it holds is lost.
            let (bytes, report) =
                trace_export::to_altium_over(std::path::Path::new(file), &b, &name)?;
            std::fs::write(out, bytes)?;
            println!("wrote {out} ({})", report.summary());
        } else {
            std::fs::write(out, trace_export::to_altium(&b, &name)?)?;
            println!("wrote {out}");
        }
        let back = trace_altium::read_pcbdoc(out)?;
        for d in trace_export::altium_rt::compare(&b, &back) {
            println!("read back differs: {d}");
        }
        return Ok(());
    }
    let json = match load(file)? {
        Design::Board(b) => serde_json::to_string_pretty(&b)?,
        Design::Sheet(s) => serde_json::to_string_pretty(&s)?,
    };
    std::fs::write(out, json)?;
    println!("wrote {out}");
    Ok(())
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "~"
    }
}

/// Sort key so R2 comes before R10.
fn natural(s: &str) -> (String, u64) {
    let split = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    (s[..split].to_string(), s[split..].parse().unwrap_or(0))
}

/// Options shared by `trace stitch` and `trace fence`.
struct ViaOpts {
    via: trace_core::stitch::ViaSpec,
    rules: trace_core::stitch::Rules,
}

fn layer_of(b: &Board, l: &str) -> Result<u8, String> {
    let geo = b.layer_geometry();
    l.parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&n| n < geo.len())
        .or_else(|| geo.iter().position(|g| g.name.eq_ignore_ascii_case(l)))
        .map(|n| n as u8)
        .ok_or_else(|| {
            format!(
                "no copper layer {l}; layers: {}",
                geo.iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn mil_of(v: &str) -> Result<f64, String> {
    Length::parse(v)
        .map(|l| l.mil())
        .ok_or(format!("not a length: {v}"))
}

/// Reads the via and rule options it knows; returns the rest.
fn via_opts<'a>(b: &Board, rest: &[&'a str]) -> Result<(ViaOpts, Vec<&'a str>), String> {
    use trace_core::stitch::{Rules, ViaSpec};
    let mut o = ViaOpts {
        via: ViaSpec::of(b),
        rules: Rules::of(b),
    };
    let mut left = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--via" => {
                let v = val()?;
                let (p, d) = v
                    .split_once('/')
                    .ok_or("--via is pad/drill, such as 16/8 or 0.4mm/0.2mm")?;
                o.via.pad = mil_of(p)?;
                o.via.drill = mil_of(d)?;
                i += 1;
            }
            "--layers" => {
                let v = val()?;
                let (f, t) = v
                    .split_once('-')
                    .ok_or("--layers is from-to, such as 1-4 or TOP-BOTTOM")?;
                let (f, t) = (layer_of(b, f)?, layer_of(b, t)?);
                o.via.from = f.min(t);
                o.via.to = f.max(t);
                i += 1;
            }
            "--clearance" => {
                o.rules.clearance = mil_of(val()?)?;
                i += 1;
            }
            "--edge" => {
                o.rules.edge = mil_of(val()?)?;
                i += 1;
            }
            _ => left.push(a),
        }
        i += 1;
    }
    Ok((o, left))
}

fn net_of(b: &Board, name: &str) -> Result<u32, String> {
    trace_core::stitch::net_named(b, name).ok_or(format!("no net {name}"))
}

/// Applies the vias, pours touched pours again and saves.
fn save_with(
    mut b: Board,
    placed: &trace_core::stitch::Placed,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let edits = placed.edits(&b);
    for e in &edits {
        e.apply(&mut b);
    }
    if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
        e.apply(&mut b);
    }
    trace_core::connectivity::analyse(&mut b);
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        std::fs::write(out, trace_export::to_kicad(&b))?;
    } else {
        trace_core::file::save(&b, Path::new(out))?;
    }
    Ok(())
}

fn stitch_cmd(file: &str, net: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::stitch::{stitch, StitchGoal};
    let b = board(file)?;
    let (o, rest) = via_opts(&b, rest)?;
    let mut goal = StitchGoal {
        net: net_of(&b, net)?,
        via: o.via,
        rules: o.rules,
        grid: 100.0,
        stagger: false,
        area: None,
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--grid" => {
                goal.grid = mil_of(val()?)?;
                i += 1;
            }
            "--stagger" => goal.stagger = true,
            "--area" => {
                let v: Vec<f64> = val()?.split(',').map(mil_of).collect::<Result<_, _>>()?;
                if v.len() != 4 {
                    return Err("--area is x0,y0,x1,y1".into());
                }
                goal.area = Some([v[0], v[1], v[2], v[3]]);
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let p = stitch(&b, &goal)?;
    println!(
        "{}: {:.1}/{:.1} mil vias, layers {}-{}, every {:.0} mil{}",
        b.nets[goal.net as usize].name,
        goal.via.pad,
        goal.via.drill,
        goal.via.from + 1,
        goal.via.to + 1,
        goal.grid,
        if goal.stagger { ", staggered" } else { "" }
    );
    println!("{}", p.summary());
    save_with(b, &p, out)?;
    println!("saved {out}");
    Ok(())
}

fn fence_cmd(file: &str, nets: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::stitch::{fence, fence_pitch, ground_net, FenceGoal};
    let b = board(file)?;
    let (o, rest) = via_opts(&b, rest)?;
    let nets: Vec<u32> = nets
        .split(',')
        .map(|n| net_of(&b, n.trim()))
        .collect::<Result<_, _>>()?;
    let mut ground = None;
    let (mut ghz, mut pitch, mut gap, mut rows) = (6.0, None, None, 1);
    let mut on: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--ground" => ground = Some(net_of(&b, val()?)?),
            "--ghz" => {
                ghz = val()?
                    .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse()
                    .map_err(|_| "--ghz takes a number")?
            }
            "--pitch" => pitch = Some(mil_of(val()?)?),
            "--gap" => gap = Some(mil_of(val()?)?),
            "--rows" => rows = val()?.parse().map_err(|_| "--rows takes a number")?,
            "--on" => {
                on = val()?
                    .split(',')
                    .map(|l| layer_of(&b, l.trim()))
                    .collect::<Result<_, _>>()?
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 2;
    }
    let ground = ground
        .or_else(|| ground_net(&b))
        .ok_or("no ground net found; give --ground")?;
    // The highest effective Er the fenced tracks see sets the wavelength.
    let er = trace_core::stitch::fence_eeff(&b, &nets, &on);
    let mut rules = o.rules;
    // A fence via need only reach the ground plane.
    rules.min_layers = 1;
    let goal = FenceGoal {
        nets: nets.clone(),
        ground,
        pitch: pitch.unwrap_or_else(|| fence_pitch(ghz, er)),
        gap: gap.unwrap_or(rules.clearance),
        rows,
        layers: on,
        via: o.via,
        rules,
    };
    let p = fence(&b, &goal)?;
    println!(
        "fence of {} vias ({:.1}/{:.1} mil) beside {}: every {:.1} mil{}, {:.1} mil from the track edge",
        b.nets[ground as usize].name,
        goal.via.pad,
        goal.via.drill,
        nets.iter()
            .map(|&n| b.nets[n as usize].name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        goal.pitch,
        if pitch.is_none() {
            format!(" (a twentieth of a wavelength at {ghz} GHz, effective Er {er:.2})")
        } else {
            String::new()
        },
        goal.gap.max(goal.rules.clearance)
    );
    println!("{}", p.summary());
    save_with(b, &p, out)?;
    println!("saved {out}");
    Ok(())
}

fn nets_list(b: &Board, v: &str) -> Result<Vec<u32>, String> {
    v.split(',').map(|n| net_of(b, n.trim())).collect()
}

/// Applies edits, pours touched pours again and saves.
fn save_edits(
    mut b: Board,
    edits: Vec<trace_core::edit::Edit>,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for e in &edits {
        e.apply(&mut b);
    }
    if let Some(e) = trace_core::pour::repour_after(&b, &edits) {
        e.apply(&mut b);
    }
    trace_core::connectivity::analyse(&mut b);
    if out.to_ascii_lowercase().ends_with(".kicad_pcb") {
        std::fs::write(out, trace_export::to_kicad(&b))?;
    } else {
        trace_core::file::save(&b, Path::new(out))?;
    }
    Ok(())
}

fn autoroute_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::autoroute::{autoroute, unroute, Goal};
    let mut b = board(file)?;
    let mut goal = Goal::default();
    let mut rip = false;
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--nets" => {
                goal.nets = Some(nets_list(&b, val()?)?);
                i += 1;
            }
            "--layers" => {
                let v: Result<Vec<u8>, String> = val()?
                    .split(',')
                    .map(|x| {
                        x.trim()
                            .parse::<u8>()
                            .ok()
                            .filter(|&n| n >= 1)
                            .map(|n| n - 1)
                            .ok_or(format!("not a layer number: {x}"))
                    })
                    .collect();
                goal.layers = Some(v?);
                i += 1;
            }
            "--width" => {
                goal.width = Some(mil_of(val()?)?);
                i += 1;
            }
            "--clearance" => {
                goal.clearance = Some(mil_of(val()?)?);
                i += 1;
            }
            "--via" => {
                let v = val()?;
                let (d, h) = v
                    .split_once('/')
                    .ok_or(format!("--via wants pad/drill: {v}"))?;
                let num = |x: &str| {
                    x.trim()
                        .parse::<f64>()
                        .map_err(|_| format!("not a number: {x}"))
                };
                goal.via = Some((num(d)?, num(h)?));
                i += 1;
            }
            "--via-cost" => {
                goal.via_cost = val()?.parse().map_err(|_| "--via-cost wants mil")?;
                i += 1;
            }
            "--poured" => goal.skip_poured = false,
            "--unroute" => rip = true,
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    if rip {
        let nets = goal.nets.clone().ok_or("--unroute needs --nets")?;
        let mut n = 0;
        for net in nets {
            let edits = unroute(&b, net);
            n += edits.len();
            for e in &edits {
                e.apply(&mut b);
            }
        }
        trace_core::connectivity::analyse(&mut b);
        println!("took up {n} tracks, arcs and vias");
    }
    let t = std::time::Instant::now();
    let (edits, r) = autoroute(&b, &goal);
    println!(
        "{} (layers {}) in {:.1} s",
        r.summary(),
        r.layers
            .iter()
            .map(|l| (l + 1).to_string())
            .collect::<Vec<_>>()
            .join(","),
        t.elapsed().as_secs_f64()
    );
    for o in r.open.iter().take(20) {
        let p = &b.geometry.pads;
        let name = |i: usize| {
            let q = &p[i];
            let part = q
                .component
                .and_then(|c| b.components.get(c as usize))
                .map_or("", |c| c.designator.as_str());
            format!("{part}.{}", q.name)
        };
        println!(
            "  open: {} {} - {}",
            b.nets.get(o.net as usize).map_or("?", |n| n.name.as_str()),
            name(o.from),
            name(o.to)
        );
    }
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}

fn teardrops_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::teardrop::{add, remove, TeardropGoal};
    let b = board(file)?;
    let mut goal = TeardropGoal::of(&b);
    let mut take_off = false;
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        let num = |v: &str| v.parse::<f64>().map_err(|_| format!("not a number: {v}"));
        match a {
            "--nets" => {
                goal.nets = nets_list(&b, val()?)?;
                i += 1;
            }
            "--smd" => goal.smd_pads = true,
            "--no-vias" => goal.vias = false,
            "--no-th" => goal.th_pads = false,
            "--length" => {
                goal.length = num(val()?)?;
                i += 1;
            }
            "--width" => {
                goal.width = num(val()?)?.clamp(0.1, 1.0);
                i += 1;
            }
            "--clearance" => {
                goal.clearance = mil_of(val()?)?;
                i += 1;
            }
            "--remove" => take_off = true,
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    if take_off {
        let n = b.geometry.polygons.iter().filter(|p| p.teardrop).count();
        let edits: Vec<_> = remove(&b).into_iter().collect();
        println!("{n} teardrops removed");
        save_edits(b, edits, out)?;
    } else {
        let t = add(&b, &goal);
        println!("{}", t.summary());
        let edits = t.edits(&b);
        save_edits(b, edits, out)?;
    }
    println!("saved {out}");
    Ok(())
}

/// Part indices from a comma-separated list of designators.
fn part_list(b: &Board, parts: &str) -> Result<Vec<usize>, String> {
    parts
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            b.components
                .iter()
                .position(|c| c.designator.eq_ignore_ascii_case(d))
                .ok_or(format!("no part {d}"))
        })
        .collect()
}

fn reuse_cmd(file: &str, block: &str, anchor: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::reuse::{match_block, replicate};
    let b = board(file)?;
    let parts = part_list(&b, block)?;
    let anchor = *part_list(&b, anchor)?.first().ok_or("no anchor part")?;
    let mut margin = Length::from_mm(1.0);
    let mut clear = true;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--keep-routing" => clear = false,
            "--margin" => {
                let v = rest.get(i + 1).ok_or("--margin needs a length")?;
                margin = Length::parse(v).ok_or(format!("not a length: {v}"))?;
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let m = match_block(&b, &parts, anchor)?;
    for &(s, t) in &m.pairs {
        println!(
            "  {} -> {}",
            b.components[s].designator, b.components[t].designator
        );
    }
    let (edits, r) = replicate(&b, &m, margin, clear);
    println!("{}", r.summary());
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}

/// Each of the board's part models meshed, where the reader manages it in
/// half a minute.
fn meshes(b: &trace_core::Board) -> Vec<Option<trace_step::Mesh>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let steps: Vec<_> = b.geometry.models.iter().map(|m| m.step.clone()).collect();
    let n = steps.len();
    let out = Arc::new(Mutex::new((0..n).map(|_| None).collect::<Vec<_>>()));
    let next = Arc::new(AtomicUsize::new(0));
    let steps = Arc::new(steps);
    let workers = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 8);
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let (steps, out, next) = (steps.clone(), out.clone(), next.clone());
            std::thread::spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(step) = steps.get(i).cloned() else {
                    break;
                };
                // A model the reader chokes on must not hold up the others.
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let r = std::panic::catch_unwind(|| trace_step::mesh(&step).ok())
                        .ok()
                        .flatten();
                    let _ = tx.send(r);
                });
                let mesh = rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .ok()
                    .flatten();
                if let Ok(mut o) = out.lock() {
                    o[i] = mesh;
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let v = out.lock().map(|mut o| std::mem::take(&mut *o));
    v.unwrap_or_default()
}

fn clear3d_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::clear3d::{check, Goal, ModelMesh};
    let b = board(file)?;
    let mut goal = Goal::default();
    let mut models = true;
    let mut i = 0;
    let len = |i: usize, name: &str| -> Result<f64, String> {
        let v = rest.get(i + 1).ok_or(format!("{name} needs a length"))?;
        Ok(Length::parse(v).ok_or(format!("not a length: {v}"))?.mm())
    };
    while i < rest.len() {
        match rest[i] {
            "--no-models" => models = false,
            "--no-estimate" => goal.estimate = false,
            "--no-edge" => goal.edge = false,
            "--clearance" => {
                goal.clearance = len(i, "--clearance")?;
                i += 1;
            }
            "--max-top" => {
                goal.max_top = Some(len(i, "--max-top")?);
                i += 1;
            }
            "--max-bottom" => {
                goal.max_bottom = Some(len(i, "--max-bottom")?);
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let meshed = if models { meshes(&b) } else { Vec::new() };
    if models && !meshed.is_empty() {
        println!(
            "{} of {} part models read",
            meshed.iter().filter(|m| m.is_some()).count(),
            meshed.len()
        );
    }
    let refs: Vec<Option<ModelMesh>> = meshed
        .iter()
        .map(|m| {
            m.as_ref().map(|m| ModelMesh {
                positions: &m.positions,
                indices: &m.indices,
            })
        })
        .collect();
    let r = check(&b, &refs, &goal);
    for issue in &r.issues {
        let at = issue.at(&b);
        println!(
            "  {:<9} {}  at {:.2}, {:.2} mm",
            issue.kind(),
            issue.describe(&b),
            at[0],
            at[1]
        );
    }
    println!("{}", r.summary());
    Ok(())
}

fn nettie_cmd(file: &str, parts: &str, out: &str, tie: bool) -> Res {
    let mut b = board(file)?;
    let names: Vec<String> = part_list(&b, parts)?
        .into_iter()
        .map(|i| b.components[i].designator.clone())
        .collect();
    trace_core::nettie::set(&mut b, &names, tie);
    let ties = trace_core::nettie::ties(&b);
    let mut list: Vec<&str> = ties
        .iter()
        .map(|&i| b.components[i].designator.as_str())
        .collect();
    list.sort_unstable();
    println!(
        "net ties: {}",
        if list.is_empty() {
            "none".to_string()
        } else {
            list.join(" ")
        }
    );
    save_edits(b, Vec::new(), out)?;
    println!("saved {out}");
    Ok(())
}

fn testpoints_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::testpoint::{assign, mark, report_csv, Sides, TestGoal};
    let b = board(file)?;
    let mut goal = TestGoal::default();
    let (mut out, mut report) = (None, None);
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        let len = |v: &str| Length::parse(v).ok_or(format!("not a length: {v}"));
        match a {
            "--top" => goal.sides = Sides::Top,
            "--both" => goal.sides = Sides::Both,
            "--smd" => goal.smd_pads = true,
            "--no-vias" => goal.vias = false,
            "--no-th" => goal.th_pads = false,
            "--tented" => goal.tented = true,
            "--fresh" => goal.keep = false,
            "--size" | "--spacing" | "--clearance" | "--edge" => {
                let l = len(val()?)?;
                match a {
                    "--size" => goal.min_size = l,
                    "--spacing" => goal.spacing = l,
                    "--clearance" => goal.part_clearance = l,
                    _ => goal.edge = l,
                }
                i += 1;
            }
            "--report" => {
                report = Some(val()?);
                i += 1;
            }
            o if !o.starts_with("--") && out.is_none() => out = Some(o),
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let r = assign(&b, &goal);
    let (top, bottom) = r.points.iter().fold(
        (0, 0),
        |(t, b), p| {
            if p.top {
                (t + 1, b)
            } else {
                (t, b + 1)
            }
        },
    );
    println!(
        "{} of {} nets have a probe point ({:.1} %): {} on the top, {} on the bottom",
        r.nets - r.missing.len(),
        r.nets,
        r.coverage(),
        top,
        bottom
    );
    let mut why: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, w) in &r.missing {
        *why.entry(w).or_default() += 1;
    }
    for (w, n) in &why {
        println!("  {n} nets: {w}");
    }
    for (net, w) in r.missing.iter().take(20) {
        let name = b.nets.get(*net as usize).map_or("?", |n| n.name.as_str());
        println!("    {name}: {w}");
    }
    if r.missing.len() > 20 {
        println!("    ... and {} more (see --report)", r.missing.len() - 20);
    }
    if let Some(p) = report {
        std::fs::write(p, report_csv(&b, &r.points, &r.missing))?;
        println!("wrote {p}");
    }
    if let Some(o) = out {
        let edits: Vec<_> = mark(&b, &r.points).into_iter().collect();
        save_edits(b, edits, o)?;
        println!("saved {o}");
    }
    Ok(())
}

/// Sheets with their file names, and the Trace schematic they came from.
type Sheets = (
    Vec<(String, trace_core::Sheet)>,
    Option<trace_core::SchFile>,
);

/// The sheets of a `.SchDoc`, a `.PrjPcb` or a Trace schematic, with
/// their file names and, for a Trace schematic, the file itself.
fn sheets_of(file: &str) -> Result<Sheets, Box<dyn std::error::Error>> {
    let p = Path::new(file);
    let name = |q: &Path| {
        q.file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match p
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("tracesch") => {
            let f = trace_core::SchFile::load(p)?;
            let v = f
                .sheets
                .iter()
                .map(|s| (s.file.clone(), s.sheet.clone()))
                .collect();
            Ok((v, Some(f)))
        }
        Some("prjpcb") => {
            let dir = p.parent().unwrap_or(Path::new("."));
            let text = String::from_utf8_lossy(&std::fs::read(p)?).into_owned();
            let mut v = Vec::new();
            for l in text.lines() {
                let Some(doc) = l.trim().strip_prefix("DocumentPath=") else {
                    continue;
                };
                let q = dir.join(doc.replace('\\', std::path::MAIN_SEPARATOR_STR));
                if q.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("schdoc"))
                    && q.is_file()
                {
                    v.push((name(&q), trace_altium::read_schdoc(&q)?));
                }
            }
            if v.is_empty() {
                return Err(format!("{file} lists no schematic sheets that exist").into());
            }
            Ok((v, None))
        }
        Some("schdoc") => Ok((vec![(name(p), trace_altium::read_schdoc(p)?)], None)),
        _ => Err(format!("{file}: expected a .SchDoc, .PrjPcb or .tracesch").into()),
    }
}

fn swap_cmd(file: &str, part: &str, a: &str, b: &str, out: &str) -> Res {
    let b0 = board(file)?;
    let i = b0
        .components
        .iter()
        .position(|c| c.designator.eq_ignore_ascii_case(part))
        .ok_or(format!("no part {part}"))?;
    let s = trace_core::swap::swap_pads(&b0, &[((i, a.to_string()), (i, b.to_string()))])?;
    println!("{}", s.summary(&b0));
    save_edits(b0, s.edits, out)?;
    println!("saved {out}");
    Ok(())
}

/// How the sheets' nets join: `--scope`, else the Trace schematic's or
/// the project's setting.
fn scope_of(
    file: &str,
    saved: &Option<trace_core::SchFile>,
    given: Option<&str>,
) -> Result<trace_core::hier::Scope, Box<dyn std::error::Error>> {
    use trace_core::hier::Scope;
    if let Some(g) = given {
        return Scope::parse(g).ok_or_else(|| {
            format!("--scope takes automatic, flat, hierarchical, strict or global, not {g}").into()
        });
    }
    if let Some(f) = saved {
        return Ok(f.scope);
    }
    if file.to_ascii_lowercase().ends_with(".prjpcb") {
        return Ok(trace_altium::project_scope(file));
    }
    Ok(Scope::Automatic)
}

/// `--scope X` among the options, and the options left.
fn take_scope<'a>(rest: &[&'a str]) -> Result<(Option<&'a str>, Vec<&'a str>), String> {
    let mut left = Vec::new();
    let mut scope = None;
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--scope" {
            scope = Some(*rest.get(i + 1).ok_or("--scope needs a value")?);
            i += 2;
            continue;
        }
        left.push(rest[i]);
        i += 1;
    }
    Ok((scope, left))
}

fn erc_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::erc::{check_with, summary, Level};
    let (given, rest) = take_scope(rest)?;
    let all = rest.contains(&"--all");
    if let Some(o) = rest.iter().find(|a| **a != "--all") {
        return Err(format!("unknown option {o}").into());
    }
    let t = std::time::Instant::now();
    let (sheets, saved) = sheets_of(file)?;
    let scope = scope_of(file, &saved, given)?;
    let refs: Vec<&trace_core::Sheet> = sheets.iter().map(|s| &s.1).collect();
    let names: Vec<&str> = sheets.iter().map(|s| s.0.as_str()).collect();
    let v = check_with(&refs, &names, scope);
    let shown = if all { v.len() } else { v.len().min(60) };
    for i in &v[..shown] {
        println!(
            "{:7}  {:24}  {:>8.0} {:>8.0}  {}",
            if i.level == Level::Error {
                "error"
            } else {
                "warning"
            },
            sheets[i.sheet].0,
            i.at[0],
            i.at[1],
            i.text
        );
    }
    if shown < v.len() {
        println!("  ... {} more (--all lists them)", v.len() - shown);
    }
    let drawings: Vec<&trace_core::Drawing> = refs.iter().map(|s| &s.drawing).collect();
    println!(
        "{} sheets, nets joined {}: {} in {:.2} s",
        sheets.len(),
        scope.resolve(&drawings).label().to_lowercase(),
        summary(&v),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Prints the sheet tree and how many project nets the sheets make.
fn hierarchy_cmd(file: &str, rest: &[&str]) -> Res {
    use trace_core::hier;
    let (given, rest) = take_scope(rest)?;
    if let Some(o) = rest.first() {
        return Err(format!("unknown option {o}").into());
    }
    let (sheets, saved) = sheets_of(file)?;
    let scope = scope_of(file, &saved, given)?;
    let drawings: Vec<&trace_core::Drawing> = sheets.iter().map(|s| &s.1.drawing).collect();
    let names: Vec<&str> = sheets.iter().map(|s| s.0.as_str()).collect();
    let j = hier::join(&drawings, Some(&names), scope);
    for (s, depth) in j.tree.order() {
        let d = drawings[s];
        let count =
            |k: trace_core::schematic::TagKind| d.tags.iter().filter(|t| t.kind == k).count();
        println!(
            "{}{}  ({} sheet symbols, {} ports, {} sheet entries)",
            "  ".repeat(depth),
            names[s],
            d.links.len(),
            count(trace_core::schematic::TagKind::Port),
            count(trace_core::schematic::TagKind::SheetEntry),
        );
    }
    for (s, name) in names.iter().enumerate() {
        let ch: Vec<&str> = j
            .instances
            .iter()
            .filter(|i| i.sheet == s && !i.suffix.is_empty())
            .map(|i| i.suffix.as_str())
            .collect();
        if !ch.is_empty() {
            println!(
                "{name} is placed {} times; its parts are named R1_{} and so on",
                ch.len(),
                ch.join(", R1_")
            );
        }
    }
    for &(s, li) in &j.missing {
        println!(
            "missing: {} places {}",
            names[s], drawings[s].links[li].file
        );
    }
    let joined = |names: Option<&[&str]>| {
        trace_core::eco::netlist_in(&drawings, names, scope)
            .iter()
            .filter(|n| n.pins.len() >= 2)
            .count()
    };
    println!(
        "{}: {} nets join two or more pins ({} if every name joined across sheets); {} sheet entries and {} ports lead nowhere",
        scope.resolve(&drawings).label(),
        joined(Some(&names)),
        joined(None),
        j.lone_entries.len(),
        j.lone_ports.len()
    );
    Ok(())
}

fn annotate_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::annotate::{annotate, apply, AnnotateGoal, Order};
    let mut goal = AnnotateGoal::default();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--all" => goal.all = true,
            "--down" => goal.order = Order::DownThenAcross,
            "--start" => {
                goal.start = val()?.parse().map_err(|_| "--start takes a whole number")?;
                i += 1;
            }
            "--per-sheet" => {
                goal.sheet_step = Some(
                    val()?
                        .parse()
                        .map_err(|_| "--per-sheet takes a whole number")?,
                );
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    if !out.to_ascii_lowercase().ends_with(".tracesch") {
        return Err("the annotated sheets are saved as a Trace schematic (.tracesch)".into());
    }
    let (mut sheets, saved) = sheets_of(file)?;
    let refs: Vec<&trace_core::Sheet> = sheets.iter().map(|s| &s.1).collect();
    let r = annotate(&refs, &goal);
    for x in r.iter().take(40) {
        println!("{:24}  {} -> {}", sheets[x.sheet].0, x.from, x.to);
    }
    if r.len() > 40 {
        println!("  ... {} more", r.len() - 40);
    }
    let before: Vec<trace_core::schematic::Drawing> = sheets
        .iter()
        .map(|s| trace_core::schematic::Drawing {
            images: Vec::new(),
            ..s.1.drawing.clone()
        })
        .collect();
    {
        let mut m: Vec<&mut trace_core::Sheet> = sheets.iter_mut().map(|s| &mut s.1).collect();
        apply(&mut m, &r);
    }
    let mut f = saved.unwrap_or_else(|| trace_core::SchFile {
        project: Path::new(file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("prjpcb"))
            .then(|| {
                Path::new(file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .flatten(),
        sheets: sheets
            .iter()
            .zip(&before)
            .map(|((name, sh), d)| trace_core::schematic::SavedSheet {
                file: name.clone(),
                sheet: sh.clone(),
                synced: Some(d.clone()),
                original: Some(d.clone()),
            })
            .collect(),
        ..Default::default()
    });
    f.version = env!("CARGO_PKG_VERSION").to_string();
    for (s, (_, sh)) in f.sheets.iter_mut().zip(&sheets) {
        s.sheet = sh.clone();
    }
    // Numbered parts renamed: now -> as on the board.
    let renamed = trace_core::annotate::component_renames(&r);
    for map in [&mut f.renames, &mut f.original_renames] {
        let was: Vec<(String, String)> = renamed
            .iter()
            .map(|(from, to)| {
                let up = from.to_ascii_uppercase();
                (to.to_ascii_uppercase(), map.get(&up).cloned().unwrap_or(up))
            })
            .collect();
        for from in renamed.keys() {
            map.remove(&from.to_ascii_uppercase());
        }
        map.extend(was);
    }
    f.save(Path::new(out))?;
    println!("{} parts renamed; saved {out}", r.len());
    Ok(())
}

fn fanout_cmd(file: &str, parts: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::fanout::{fanout, Direction, FanoutGoal, Style};
    let b = board(file)?;
    let mut idx = Vec::new();
    for d in parts.split(',').map(str::trim).filter(|d| !d.is_empty()) {
        let i = b
            .components
            .iter()
            .position(|c| c.designator.eq_ignore_ascii_case(d))
            .ok_or(format!("no part {d}"))?;
        idx.push(i);
    }
    let mut goal = FanoutGoal::of(&b, idx);
    let (o, left) = via_opts(&b, rest)?;
    goal.via = o.via;
    goal.clearance = o.rules.clearance;
    let mut i = 0;
    while i < left.len() {
        let a = left[i];
        let val = || left.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--in-pad" => goal.style = Style::InPad,
            "--inward" => goal.direction = Direction::In,
            "--no-net" => goal.no_net = true,
            "--skip-rows" => {
                goal.skip_rows = val()?
                    .parse()
                    .map_err(|_| "--skip-rows takes a whole number")?;
                i += 1;
            }
            "--width" => {
                goal.width = Some(mil_of(val()?)?);
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let f = fanout(&b, &goal);
    println!("{}", f.summary());
    let edits = f.edits(&b);
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}

fn gloss_cmd(file: &str, out: &str, rest: &[&str]) -> Res {
    use trace_core::gloss::{gloss, GlossGoal};
    let b = board(file)?;
    let mut goal = GlossGoal::of(&b);
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        let val = || rest.get(i + 1).copied().ok_or(format!("{a} needs a value"));
        match a {
            "--nets" => {
                goal.nets = nets_list(&b, val()?)?;
                i += 1;
            }
            "--no-merge" => goal.merge = false,
            "--keep-dangling" => goal.dangling = false,
            "--corners" => goal.corners = true,
            "--clearance" => {
                goal.clearance = mil_of(val()?)?;
                i += 1;
            }
            o => return Err(format!("unknown option {o}").into()),
        }
        i += 1;
    }
    let g = gloss(&b, &goal);
    println!("{}", g.summary());
    let edits = g.edits(&b);
    save_edits(b, edits, out)?;
    println!("saved {out}");
    Ok(())
}
