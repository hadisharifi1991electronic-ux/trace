//! The menu bar in Altium Designer's order: File, Edit, View, Project,
//! Place, Design, Tools, Route, Reports, Window, Help. Every command Trace
//! has is in one of them.

use crate::{router, App, ExportKind, Tool};
use eframe::egui::{self, RichText};

/// A menu item: enabled or greyed, with its key and a hover note. True when
/// clicked; the menu closes.
fn item(ui: &mut egui::Ui, on: bool, label: &str, key: &str, tip: &str) -> bool {
    let mut b = egui::Button::new(label);
    if !key.is_empty() {
        b = b.shortcut_text(key);
    }
    let mut r = ui.add_enabled(on, b);
    if !tip.is_empty() {
        r = r.on_hover_text(tip);
    }
    if r.clicked() {
        ui.close_menu();
        true
    } else {
        false
    }
}

impl App {
    pub(crate) fn menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::bar(ui, |ui| {
            let has = self.scene.is_some();
            ui.menu_button("File", |ui| self.file_menu(ui, ctx, has));
            ui.menu_button("Edit", |ui| self.edit_menu(ui, has));
            ui.menu_button("View", |ui| self.view_menu(ui, ctx, has));
            ui.menu_button("Project", |ui| self.project_menu(ui, has));
            ui.menu_button("Place", |ui| self.place_menu(ui, has));
            ui.menu_button("Design", |ui| self.design_menu(ui, has));
            ui.menu_button("Tools", |ui| self.tools_menu(ui, has));
            ui.menu_button("Route", |ui| self.route_menu(ui, has));
            ui.menu_button("Reports", |ui| self.reports_menu(ui, has));
            ui.menu_button("Window", |ui| self.window_menu(ui, has));
            ui.menu_button("Help", |ui| {
                if item(ui, true, "Keyboard shortcuts", "", "") {
                    self.studio.keys_open = true;
                }
                if item(ui, true, "Home page", "", "") {
                    self.go_home();
                }
                ui.separator();
                ui.label(concat!("Trace Studio ", env!("CARGO_PKG_VERSION"), ", open-source PCB design"));
                ui.label(RichText::new("Opens Altium .PrjPcb, .PcbDoc, .SchDoc, .PcbLib and .IntLib,\nAllegro .brd (16.0 and newer), Gerber and drill files and ODB++.\nSaves Trace, KiCad and Altium boards.").weak());
            });
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, has: bool) {
        if item(
            ui,
            true,
            "Open…",
            "Ctrl+O",
            "A project, board, schematic or library",
        ) {
            self.open_dialog();
        }
        if item(ui, true, "Open Gerber or ODB++ folder…", "", "") {
            self.open_folder_dialog();
        }
        ui.menu_button("Recent documents", |ui| {
            if self.recent.is_empty() {
                ui.label(RichText::new("No recent documents").weak());
            }
            for p in self.recent.clone() {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if ui
                    .button(name)
                    .on_hover_text(p.display().to_string())
                    .clicked()
                {
                    ui.close_menu();
                    self.open(p);
                }
            }
        });
        ui.separator();
        if item(ui, has, "Save as Trace board…", "", "") {
            self.save_as();
        }
        if item(ui, has, "Save as Altium board…", "", "") {
            self.save_altium();
        }
        if item(ui, has, "Save as KiCad board…", "", "") {
            self.save_kicad();
        }
        ui.separator();
        ui.add_enabled_ui(has, |ui| {
            ui.menu_button("Fabrication outputs", |ui| {
                if item(ui, true, "Gerber, drill and BOM…", "", "Gerber X1 per layer, Excellon drill, BOM and pick and place") {
                    self.export_fab();
                }
                if item(ui, true, "ODB++…", "", "One ODB++ job (.tgz): every layer, the drills, the parts, the nets and the stack-up") {
                    self.export_odb();
                }
                if item(ui, true, "IPC-2581…", "", "One IPC-2581 revision C file: layers, drills, stack-up, parts, nets and BOM") {
                    self.export_file(ExportKind::Ipc2581);
                }
                if item(ui, true, "Fabrication drawing (PDF)…", "", "Outline with drill symbols and dimensions, drill table, notes and the stack-up (A3)") {
                    self.export_file(ExportKind::FabDrawing);
                }
            });
            ui.menu_button("Assembly outputs", |ui| {
                if item(ui, true, "Assembly drawing (PDF)…", "", "Top and bottom views with part bodies, designators and pin 1 marks, then the parts list (A3)") {
                    self.export_assembly();
                }
            });
            ui.menu_button("Export", |ui| {
                if item(ui, true, "STEP (3D)…", "", "The board solid with its holes and the parts, for mechanical CAD") {
                    self.export_file(ExportKind::Step);
                }
                if item(ui, true, "DXF…", "", "Every layer as a DXF layer, in millimetres") {
                    self.export_file(ExportKind::Dxf);
                }
            });
        });
        ui.separator();
        if item(ui, true, "Exit", "Alt+F4", "") {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        let undo = self
            .router
            .history
            .undo_label()
            .map(|l| format!("Undo {l}"));
        if item(
            ui,
            undo.is_some(),
            undo.as_deref().unwrap_or("Undo"),
            "Ctrl+Z",
            "",
        ) {
            self.undo();
        }
        let redo = self
            .router
            .history
            .redo_label()
            .map(|l| format!("Redo {l}"));
        if item(
            ui,
            redo.is_some(),
            redo.as_deref().unwrap_or("Redo"),
            "Ctrl+Y",
            "",
        ) {
            self.redo();
        }
        ui.separator();
        let any = !self.edit.sel.is_empty();
        if item(ui, any, "Cut", "Ctrl+X", "") {
            self.copy_sel(true);
        }
        if item(ui, any, "Copy", "Ctrl+C", "") {
            self.copy_sel(false);
        }
        if item(
            ui,
            has && self.edit.clip.is_some(),
            "Paste",
            "Ctrl+V",
            "Pasted parts get new designators and no nets",
        ) {
            self.paste_clip();
        }
        if item(ui, any, "Delete", "Del", "") {
            self.delete_sel();
        }
        if item(
            ui,
            self.picked.is_some() && !any,
            "Delete clicked track",
            "Del",
            "",
        ) {
            self.delete_picked();
        }
        ui.separator();
        if item(ui, has, "Select all", "Ctrl+A", "") {
            self.select_all();
        }
        ui.separator();
        if item(ui, any, "Rotate selection 90°", "Ctrl+R", "") {
            self.rotate_sel(90.0);
        }
        ui.add_enabled_ui(any, |ui| {
            ui.menu_button("Align", |ui| {
                for (k, how) in trace_core::select::Align::ALL.into_iter().enumerate() {
                    if k == 4 || k == 6 {
                        ui.separator();
                    }
                    if ui.button(how.label()).clicked() {
                        ui.close_menu();
                        self.align_sel(how);
                    }
                }
            });
        });
        ui.separator();
        if item(
            ui,
            has,
            "Find a net or part",
            "",
            "Type in the Navigator panel's search box",
        ) {
            self.studio.show(crate::studio::Pane::Navigator);
        }
        if item(ui, has, "Find Similar Objects…", "Shift+F", "Select everything like the selected part, track, arc or via: same net, layer, width, footprint…") {
            self.find_similar();
        }
        if item(
            ui,
            has,
            "PCB Filter (query)…",
            "",
            "Select by a query such as IsVia And HoleSize < 8mil",
        ) {
            self.studio.show(crate::studio::Pane::Filter);
        }
        ui.menu_button("Selection memory", |ui| {
            ui.label(
                RichText::new("Ctrl+1..8 stores the selection, Alt+1..8 brings it back").weak(),
            );
            for n in 0..8 {
                let what = self.panes.memory[n].as_ref().map(|s| s.describe());
                if item(
                    ui,
                    what.is_some(),
                    &format!("{}: {}", n + 1, what.as_deref().unwrap_or("empty")),
                    &format!("Alt+{}", n + 1),
                    "",
                ) {
                    if let Some(s) = self.panes.memory[n].clone() {
                        self.edit.sel = s;
                        if let Some(scene) = &self.scene {
                            self.edit.sel.keep_valid(&scene.board);
                        }
                    }
                }
            }
        });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, has: bool) {
        let board = has && !self.mode_sch && !self.mode_fp;
        if item(ui, has, "Fit document", "F", "") {
            self.go_board();
            self.needs_fit = true;
        }
        if item(ui, board, "Zoom in", "", "") {
            self.zoom_by(1.5);
        }
        if item(ui, board, "Zoom out", "", "") {
            self.zoom_by(1.0 / 1.5);
        }
        if item(
            ui,
            board,
            if self.view.flip {
                "View from top"
            } else {
                "View from bottom"
            },
            "B",
            "",
        ) {
            self.view.flip = !self.view.flip;
            self.needs_fit = true;
        }
        ui.separator();
        if item(ui, has, "2D layout", "", "") {
            self.go_board();
            self.mode3d = false;
        }
        if item(
            ui,
            has,
            "3D layout",
            "3",
            "The assembled board with its part models",
        ) {
            self.go_board();
            self.mode3d = true;
        }
        let via = self.edit.sel.vias.iter().next().copied();
        if item(
            ui,
            board && via.is_some(),
            "3D model of the selected via",
            "",
            "A cut-out about the via (and its pair partner) through every layer; \
             right-click a via on the board for the same",
        ) {
            if let (Some(v), Some(scene)) = (via, &self.scene) {
                self.message = Some(self.model3d.open_via(&scene.board, v));
            }
        }
        if item(
            ui,
            board,
            "3D model of an area…",
            "",
            "Drag a box on the board; it opens as a 3D cut-out through every layer",
        ) {
            self.go_board();
            self.mode3d = false;
            self.box3d = true;
            self.message = Some((
                "Drag a box over the area to see in 3D (Escape cancels).".into(),
                false,
            ));
        }
        if item(
            ui,
            board,
            if self.single.is_some() {
                "All layers"
            } else {
                "Single layer mode"
            },
            "Shift+S",
            "",
        ) {
            let on = self.single.is_none();
            self.set_single(on);
        }
        ui.separator();
        ui.menu_button("Panels", |ui| self.panels_menu(ui));
        ui.checkbox(&mut self.show_pairs, "Differential pairs table");
        ui.separator();
        if item(
            ui,
            true,
            if self.studio.mm {
                "Units: switch to mil"
            } else {
                "Units: switch to mm"
            },
            "Q",
            "",
        ) {
            self.studio.mm = !self.studio.mm;
        }
        ui.menu_button("Theme", |ui| {
            for (dark, name) in [(true, "Dark"), (false, "Light")] {
                if ui.radio(self.studio.dark == dark, name).clicked() {
                    ui.close_menu();
                    self.studio.dark = dark;
                    crate::studio::apply_theme(ctx, dark);
                }
            }
        });
    }

    fn project_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(ui, !self.schem.is_empty(), "Schematic sheets", "S", "Annotate, check (ERC), wire and Update PCB are in the Navigator panel while a sheet shows") {
            self.go_schematic();
        }
        if item(
            ui,
            has || !self.fped.is_empty(),
            "Footprint library",
            "",
            "The board's footprints, or open a .PcbLib or .tracelib",
        ) {
            self.studio.home = false;
            if !self.mode_fp {
                self.open_footprints();
            }
        }
        if item(
            ui,
            has,
            "Assembly variants…",
            "",
            "Parts left off or changed in each build of the board",
        ) {
            self.variants.open = true;
        }
        if item(
            ui,
            has,
            "Compare with another version…",
            "",
            "What changed between this board and another file of it: parts, nets, routing, rules, stack-up",
        ) {
            self.compare.open = true;
        }
        ui.separator();
        if item(ui, true, "Projects panel", "", "") {
            self.studio.show(crate::studio::Pane::Projects);
        }
    }

    fn place_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(ui, has, "Track (interactive routing)", "P, T", "") {
            self.run_command("Track (interactive routing)");
        }
        if item(
            ui,
            has,
            "Differential pair",
            "P, I",
            "Route both nets of a pair together",
        ) {
            self.run_command("Differential pair routing");
        }
        if item(
            ui,
            self.router.active.is_some(),
            "Via",
            "V",
            "While routing: a via, carrying on one layer down (Shift+V up)",
        ) {
            self.place_via(false);
        }
        ui.separator();
        if item(ui, has, "String (text)", "P, S", "") {
            self.set_tool(Tool::Text);
        }
        if item(ui, has, "Dimension", "P, D", "") {
            self.set_tool(Tool::Dimension);
        }
        if item(
            ui,
            has,
            "Polygon pour, region, keepout, cutout",
            "P, G",
            "Draw and edit shapes; a pour's thermal relief style",
        ) {
            self.set_tool(Tool::Shape);
        }
        ui.separator();
        if item(
            ui,
            has,
            "Component from a footprint library…",
            "P, C",
            "Open the footprint editor and place a footprint",
        ) {
            self.run_command("Component from a footprint library");
        }
        if item(ui, has, "Via stitching and fences…", "", "Fill a ground net's pours with stitching vias, or line a net's tracks with a via fence") {
            self.stitch.open = true;
        }
        if item(
            ui,
            has,
            "Test points…",
            "",
            "One probe point per net for bed-of-nails or flying-probe test",
        ) {
            self.tests.open = true;
        }
        if item(
            ui,
            has,
            "Teardrops…",
            "",
            "Teardrops where tracks meet vias and pads",
        ) {
            self.tidy.open = true;
        }
    }

    fn design_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(
            ui,
            has,
            "Rules…",
            "",
            "PCB Rules and Constraints Editor: clearance, width, via style, pairs, mask, plane, fab and placement rules by category, with scopes and priorities. DRC and the router follow them",
        ) {
            self.rules_win.open_at(None);
        }
        if item(
            ui,
            has,
            "Rules and constraints (Constraint Manager)…",
            "C",
            "Length, match, skew and via limits, by net, class and pair",
        ) {
            self.cm.open = true;
        }
        if item(
            ui,
            has,
            "Layer stack and impedance…",
            "I",
            "Stack-up, widths for target impedance, and each pair's impedance",
        ) {
            self.imp.open = true;
        }
        if item(
            ui,
            has,
            "Rigid-flex zones and bends…",
            "",
            "Areas with their own layers, where the flex bends, and the IPC-2223 flex checks",
        ) {
            self.flex.open = true;
        }
        ui.separator();
        if item(
            ui,
            has,
            "Copy block layout (reuse)…",
            "",
            "Place and route another instance of a block as one already laid out",
        ) {
            self.reuse.open = true;
        }
        let sel_n = self.edit.sel.parts.len();
        if item(ui, has && sel_n > 0, "Net tie: mark selected parts", "", "Their own copper may join their nets: the rule check does not call it a short. Click again to unmark") {
            self.toggle_net_ties();
        }
        if item(
            ui,
            has,
            "Pin and gate swap…",
            "",
            "Two pins or two gates of a part trade nets; the schematic changes to match",
        ) {
            self.swap.open = true;
        }
        ui.separator();
        if item(ui, has, "Backdrill long via stubs", "", "Every via whose stub is longer than its rule allows (the back drilling rule, or 10 mil on pair nets)") {
            if let Some(scene) = &self.scene {
                let edits = trace_core::backdrill::plan(&scene.board);
                if edits.is_empty() {
                    self.message = Some(("No via stub is longer than its rule allows".into(), false));
                } else {
                    let n = edits.len() / 2;
                    self.apply_edits(&format!("Backdrilled {n} vias"), edits);
                }
            }
        }
        let drilled = self.scene.as_ref().is_some_and(|s| {
            s.board
                .geometry
                .vias
                .iter()
                .any(|v| v.fab.back_top.is_some() || v.fab.back_bottom.is_some())
        });
        if item(ui, drilled, "Remove backdrills", "", "") {
            if let Some(scene) = &self.scene {
                let edits = trace_core::backdrill::clear(&scene.board);
                let n = edits.len() / 2;
                self.apply_edits(&format!("Removed {n} backdrills"), edits);
            }
        }
        ui.separator();
        if item(
            ui,
            has,
            "Make a panel…",
            "",
            "Copies of the board in a panel, with rails, tabs and mouse bites or V-score",
        ) {
            self.panel.open = true;
        }
    }

    fn tools_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(ui, has, "Design Rule Check…", "D", "Clearance, shorts, width, unrouted nets, manufacturing and SI checks, live as you edit") {
            self.drc.open = true;
        }
        if item(
            ui,
            has,
            "3D clearance…",
            "",
            "Part bodies that touch or come too close, Height rules, bodies over the board edge",
        ) {
            self.clear3d.open = true;
        }
        ui.separator();
        if item(
            ui,
            has,
            "Teardrops and gloss…",
            "",
            "Teardrops; tidy routing: merge straight joins, cut dangling tracks, chamfer corners",
        ) {
            self.tidy.open = true;
        }
        if item(ui, has, "Test points…", "", "") {
            self.tests.open = true;
        }
        ui.separator();
        ui.add_enabled_ui(has, |ui| {
            ui.menu_button("Signal and power integrity", |ui| {
                if item(ui, true, "Signal simulation (IBIS)…", "", "Waveforms and the eye of the selected net, driven and received by IBIS models") {
                    self.sim.open = true;
                }
                if item(ui, true, "Power integrity (IR drop)…", "", "DC voltage drop over a supply net's pours, tracks and vias") {
                    self.power.open = true;
                }
                if item(ui, true, "PDN impedance and decaps…", "", "Supply impedance from 100 kHz to 1 GHz from the planes, decaps and VRM, with a decap optimiser") {
                    self.pdn.open = true;
                }
                if item(ui, true, "Thermal (3D, heatsinks)…", "", "Steady-state temperatures: board copper and vias, JEDEC two-resistor packages, plate-fin heatsinks, natural or forced air and radiation; Tj per part and maps per layer") {
                    self.thermal.open = true;
                }
                if item(ui, true, "Bus simulation (DDR, parallel)…", "", "Every bit of a bus: its eye with the nets beside it switching, its valid window against the strobe, setup/hold margins, the receiver mask; as routed, trained and deskewed") {
                    self.bus_sim.open = true;
                }
                if item(ui, true, "EMI check…", "", "Fast nets over plane gaps and splits, layer changes with no return via, tracks near the edge, edge stitching, and each clock's radiation against the FCC or CISPR limit") {
                    self.emi.open = true;
                }
                if item(ui, true, "Via S-parameters (3D)…", "", "3D full-wave solve of a via or via pair: return and insertion loss, stub resonance, Touchstone") {
                    self.via.open = true;
                }
                if item(ui, true, "Smith chart…", "", "S11 of Touchstone files or of the last via solve on the impedance chart: Z, VSWR and return loss at markers") {
                    self.smith.open = true;
                }
                if item(ui, true, "Pair cross-sections…", "", "Track widths and gaps on a layer that give the target Zdiff, ranked by loss") {
                    self.pairs.open = true;
                }
                if item(ui, true, "Impedance check…", "", "Every controlled trace and via kind against its target impedance (pairs, coax nets, rules)") {
                    self.zcheck.open = true;
                }
                if item(ui, true, "Path loss…", "", "The routed path between two pins, traces, vias and parts cascaded: insertion and return loss to your GHz, Touchstone") {
                    self.pathloss.open = true;
                }
                if item(ui, true, "RF report…", "", "Every RF path from the coaxial connectors to the chips: insertion and return loss in the band, where each reflects and loses most") {
                    self.rf.open = true;
                }
            });
        });
        ui.separator();
        if item(ui, has || !self.fped.is_empty(), "Footprint editor", "", "") {
            self.studio.home = false;
            if !self.mode_fp {
                self.open_footprints();
            }
        }
        if item(
            ui,
            has,
            "Measure distance",
            "Ctrl+M",
            "Click two points; they snap to pads, vias and track ends",
        ) {
            self.go_board();
            self.set_tool(Tool::Measure);
        }
    }

    fn route_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(ui, has, "Interactive routing", "Ctrl+W", "") {
            self.run_command("Interactive routing");
        }
        if item(ui, has, "Interactive differential pair routing", "U, I", "") {
            self.run_command("Differential pair routing");
        }
        if item(
            ui,
            has,
            "Interactive length tuning",
            "T",
            "Meanders to the target length",
        ) {
            self.set_tool(Tool::Tune);
        }
        ui.separator();
        if item(
            ui,
            has,
            "Auto route…",
            "U, A",
            "Route the open connections with tracks and vias, around what is already on the board",
        ) {
            self.autoroute.open = true;
        }
        if item(
            ui,
            has,
            "Fanout…",
            "U, F",
            "Fan out BGA parts: a dogbone to a via between the balls, or a via in each pad",
        ) {
            self.fan.open = true;
        }
        if item(
            ui,
            has,
            "Gloss…",
            "",
            "Tidy routing: merge straight joins, cut dangling tracks, chamfer square corners",
        ) {
            self.tidy.open = true;
        }
        ui.separator();
        if item(
            ui,
            self.selected.is_some() && has,
            "Unroute selected net",
            "Shift+Del",
            "",
        ) {
            if let (Some(n), Some(scene)) = (self.selected, &self.scene) {
                let edits = router::unroute(&scene.board, n);
                let name = scene.board.nets[n as usize].name.clone();
                self.apply_edits(&format!("Unrouted {name}"), edits);
            }
        }
    }

    fn reports_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(
            ui,
            has,
            "Board information",
            "",
            "Size, layers, parts, nets, tracks and vias in the Properties panel",
        ) {
            self.edit.sel.clear();
            self.picked = None;
            self.selected = None;
            self.go_board();
            self.studio.show(crate::studio::Pane::Properties);
        }
        if item(
            ui,
            has,
            "Differential pair report…",
            "",
            "Every pair's lengths, skew and impedance, as text",
        ) {
            self.export_pairs();
        }
        if item(
            ui,
            has,
            "Bill of materials with part data…",
            "",
            "Parts by part number with lifecycle, stock and price; obsolete, out of stock and missing part numbers",
        ) {
            self.bom.open = true;
        }
        if item(ui, has, "Design rule violations", "", "") {
            self.drc.open = true;
        }
        if item(ui, has, "Measure distance", "Ctrl+M", "") {
            self.go_board();
            self.set_tool(Tool::Measure);
        }
        ui.separator();
        if item(ui, true, "Messages panel", "", "") {
            self.studio.show(crate::studio::Pane::Messages);
        }
    }

    fn window_menu(&mut self, ui: &mut egui::Ui, has: bool) {
        if item(ui, true, "Home page", "", "") {
            self.go_home();
        }
        if item(ui, has, "Board", "", "") {
            self.go_board();
        }
        if item(ui, !self.schem.is_empty(), "Schematic", "", "") {
            self.go_schematic();
        }
        if item(ui, !self.fped.is_empty(), "Footprints", "", "") {
            self.studio.home = false;
            self.mode_fp = true;
        }
        ui.separator();
        ui.menu_button("Panels", |ui| self.panels_menu(ui));
        if item(ui, true, "Reset panel layout", "", "") {
            self.studio.reset_layout();
        }
    }
}
