//! The Fabrication Outputs window: Gerber X2 or X1, drill units and
//! merging, mask expansion and via tenting, which files go in, the ODB++
//! job, IPC-2581 and drawings alongside, a read-back check of the files,
//! and one zip for the fab house.

use eframe::egui;
use std::collections::BTreeSet;
use trace_core::{Board, Length};
use trace_export::{DrillUnits, FabFile, FabOptions};

#[derive(Default)]
pub struct FabWindow {
    pub open: bool,
    /// The settings; taken from the board's rules when the window opens.
    opt: Option<FabOptions>,
    /// The Gerber, drill and assembly files for the settings.
    files: Vec<FabFile>,
    stale: bool,
    /// Files left out, by name without the board's stem.
    off: BTreeSet<String>,
    odb: bool,
    ipc2581: bool,
    drawing: bool,
    assembly: bool,
    check: Option<trace_gerber::FabCheck>,
}

impl FabWindow {
    /// Opens the window with the board's mask and tenting rules.
    pub fn open_window(&mut self) {
        self.open = true;
        self.opt = None;
        self.stale = true;
        self.check = None;
    }

    /// Shows the window; returns a message for the status bar after a save.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        stem: &str,
    ) -> Option<(String, bool)> {
        let mut msg = None;
        let mut open = self.open;
        let mut o = self
            .opt
            .clone()
            .unwrap_or_else(|| FabOptions::from_board(board));
        egui::Window::new("Fabrication Outputs")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                settings(ui, &mut o);
                if self.opt.as_ref() != Some(&o) {
                    if self.opt.is_some() {
                        self.stale = true;
                        self.check = None;
                    }
                    self.opt = Some(o.clone());
                }
                if self.stale {
                    self.files = trace_export::fab_files(board, stem, &o);
                    self.stale = false;
                }
                ui.separator();
                self.file_list(ui, stem);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Also in the zip:");
                    ui.checkbox(&mut self.odb, "ODB++ job");
                    ui.checkbox(&mut self.ipc2581, "IPC-2581");
                    ui.checkbox(&mut self.drawing, "Fabrication drawing");
                    ui.checkbox(&mut self.assembly, "Assembly drawing");
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .button("Check")
                        .on_hover_text("Read the Gerber and drill files back, as a fab house's CAM would, and compare them with the board")
                        .clicked()
                    {
                        let src = self
                            .chosen(stem)
                            .map(|f| trace_gerber::SourceFile {
                                name: f.name.clone(),
                                data: f.data.clone(),
                            })
                            .collect();
                        self.check = Some(trace_gerber::check(board, src));
                    }
                    if ui.button("Save zip…").clicked() {
                        msg = self.save(board, stem, &o);
                    }
                });
                if let Some(c) = &self.check {
                    show_check(ui, c);
                }
            });
        self.open = open;
        msg
    }

    fn chosen<'a>(&'a self, stem: &'a str) -> impl Iterator<Item = &'a FabFile> + 'a {
        self.files
            .iter()
            .filter(move |f| !self.off.contains(short(&f.name, stem)))
    }

    fn file_list(&mut self, ui: &mut egui::Ui, stem: &str) {
        let total = self.files.len();
        let on = self.chosen(stem).count();
        ui.horizontal(|ui| {
            ui.strong(format!("Files ({on} of {total})"));
            if ui.small_button("All").clicked() {
                self.off.clear();
                self.check = None;
            }
        });
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("fabfiles")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for f in &self.files {
                            let key = short(&f.name, stem).to_string();
                            let mut keep = !self.off.contains(&key);
                            if ui.checkbox(&mut keep, &f.name).changed() {
                                if keep {
                                    self.off.remove(&key);
                                } else {
                                    self.off.insert(key);
                                }
                                self.check = None;
                            }
                            ui.weak(kind(&f.name));
                            ui.weak(size(f.data.len()));
                            ui.end_row();
                        }
                    });
            });
    }

    /// Asks where, then writes the chosen files and extras as one zip.
    fn save(&self, board: &Board, stem: &str, o: &FabOptions) -> Option<(String, bool)> {
        let path = rfd::FileDialog::new()
            .add_filter("Zip archive", &["zip"])
            .set_file_name(format!("{stem}-fab.zip"))
            .save_file()?;
        let mut files: Vec<FabFile> = self.chosen(stem).cloned().collect();
        let mut extra = |name: String, data: Vec<u8>| files.push(FabFile { name, data });
        if self.odb {
            match trace_export::to_odb(board, stem, o) {
                Ok(d) => extra(format!("{stem}-odb.tgz"), d),
                Err(e) => return Some((format!("Could not make the ODB++ job: {e}"), true)),
            }
        }
        if self.ipc2581 {
            extra(
                format!("{stem}.xml"),
                trace_export::to_ipc2581(board, stem).0.into_bytes(),
            );
        }
        if self.drawing {
            extra(
                format!("{stem}-fab.pdf"),
                trace_export::fab_drawing_pdf(board, stem, trace_export::Paper::A3),
            );
        }
        if self.assembly {
            extra(
                format!("{stem}-assembly.pdf"),
                trace_export::assembly_pdf(board, stem, trace_export::Paper::A3),
            );
        }
        let n = files.len();
        Some(
            match trace_export::fab_zip(&files).and_then(|z| std::fs::write(&path, z)) {
                Ok(()) => (
                    format!(
                        "Saved {n} fab files to {} (Gerber {}, drill in {}{})",
                        path.display(),
                        if o.gerber_x2 { "X2" } else { "X1" },
                        match o.drill_units {
                            DrillUnits::Metric => "mm",
                            DrillUnits::Inch => "inches",
                        },
                        match &self.check {
                            Some(c) if c.problems() > 0 => {
                                format!("; the check found {} differences", c.problems())
                            }
                            Some(_) => "; checked".into(),
                            None => String::new(),
                        }
                    ),
                    false,
                ),
                Err(e) => (format!("Could not save: {e}"), true),
            },
        )
    }
}

fn settings(ui: &mut egui::Ui, o: &mut FabOptions) {
    egui::Grid::new("fabopt").num_columns(2).show(ui, |ui| {
        ui.label("Gerber");
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.gerber_x2, true, "X2").on_hover_text(
                "Layer functions, nets, parts and pins in the files; read by current CAM",
            );
            ui.radio_value(&mut o.gerber_x2, false, "X1 (RS-274X)")
                .on_hover_text(
                    "No attributes, for older CAM; add the IPC-356 netlist so nets can be checked",
                );
        });
        ui.end_row();
        ui.label("Drill units");
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.drill_units, DrillUnits::Metric, "mm");
            ui.radio_value(&mut o.drill_units, DrillUnits::Inch, "inch");
        });
        ui.end_row();
        ui.label("Through holes");
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.drill_merge, false, "Plated and non-plated files");
            ui.radio_value(&mut o.drill_merge, true, "One file");
        });
        ui.end_row();
        ui.label("Mask expansion");
        length(ui, &mut o.mask_expansion, 0.0..=0.5);
        ui.end_row();
        ui.label("Paste expansion");
        length(ui, &mut o.paste_expansion, -0.2..=0.2);
        ui.end_row();
        ui.label("Tent vias");
        ui.horizontal(|ui| {
            ui.checkbox(&mut o.tent_top, "Top");
            ui.checkbox(&mut o.tent_bottom, "Bottom");
        });
        ui.end_row();
    });
    ui.weak("Gerber coordinates are mm in 4.6 format, so nothing is rounded. Mask, paste and tenting start from the board's rules.");
}

fn length(ui: &mut egui::Ui, v: &mut Length, range: std::ops::RangeInclusive<f64>) {
    let mut mm = v.mm();
    let r = ui.add(
        egui::DragValue::new(&mut mm)
            .range(range)
            .speed(0.001)
            .fixed_decimals(3)
            .suffix(" mm"),
    );
    if r.changed() {
        *v = Length::from_mm(mm);
    }
    ui.weak(format!("{:.1} mil", v.mil()));
}

fn show_check(ui: &mut egui::Ui, c: &trace_gerber::FabCheck) {
    let ok = egui::Color32::from_rgb(64, 190, 160);
    let bad = ui.visuals().error_fg_color;
    ui.separator();
    match c.problems() {
        0 => ui.colored_label(ok, "The files match the board."),
        n => ui.colored_label(
            bad,
            format!("{n} items differ between the board and the files."),
        ),
    };
    egui::Grid::new("fabcheck")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("");
            ui.strong("Item");
            ui.strong("Board");
            ui.strong("Files");
            ui.end_row();
            for l in &c.lines {
                if l.ok {
                    ui.colored_label(ok, "✔");
                } else {
                    ui.colored_label(bad, "✖");
                }
                ui.label(&l.item);
                ui.label(&l.board);
                let r = ui.label(&l.files);
                if !l.note.is_empty() {
                    r.on_hover_text(&l.note);
                }
                ui.end_row();
                if !l.note.is_empty() && !l.ok {
                    ui.label("");
                    ui.colored_label(bad, &l.note);
                    ui.end_row();
                }
            }
        });
    egui::CollapsingHeader::new(format!("How each file was read ({})", c.files.len()))
        .id_salt("fabcheck-files")
        .show(ui, |ui| {
            egui::Grid::new("fabcheck-roles")
                .num_columns(2)
                .show(ui, |ui| {
                    for f in &c.files {
                        ui.label(&f.file);
                        ui.weak(&f.role);
                        ui.end_row();
                    }
                });
            for w in &c.warnings {
                ui.weak(format!("Warning: {w}"));
            }
        });
}

/// A file's name without the board's stem, as the key for leaving it out.
fn short<'a>(name: &'a str, stem: &str) -> &'a str {
    name.strip_prefix(stem).unwrap_or(name)
}

fn kind(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_uppercase();
    match ext.as_str() {
        "DRL" => "Drill",
        "IPC" => "IPC-356 netlist",
        "CSV" if name.ends_with("-BOM.csv") => "Bill of materials",
        "CSV" if name.ends_with("-PickPlace.csv") => "Pick and place",
        "CSV" => "Report",
        "GTL" | "GBL" => "Copper",
        "GTS" | "GBS" => "Solder mask",
        "GTP" | "GBP" => "Paste",
        "GTO" | "GBO" => "Silkscreen",
        "GKO" => "Board outline",
        e if e.starts_with("GM") => "Mechanical",
        e if e.starts_with('G') => "Copper (inner)",
        _ => "",
    }
}

fn size(bytes: usize) -> String {
    if bytes < 10_000 {
        format!("{bytes} B")
    } else if bytes < 10_000_000 {
        format!("{} KB", bytes / 1000)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}
