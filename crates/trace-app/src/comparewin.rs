//! Design compare window: what changed between the open board and
//! another version of it.

use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::mpsc;
use trace_core::compare::{self, Comparison, Kind, Options};
use trace_core::Board;

type Loaded = Result<(PathBuf, Board), String>;

pub struct CompareWindow {
    pub open: bool,
    /// The other board, read in the background.
    other: Option<(PathBuf, Board)>,
    loading: Option<mpsc::Receiver<Loaded>>,
    /// The open board is the newer one (the other is the first).
    open_is_newer: bool,
    opt: Options,
    result: Option<Comparison>,
    error: Option<String>,
    filter: String,
    hide: Vec<&'static str>,
    /// A path typed in instead of picked.
    typed: String,
}

impl Default for CompareWindow {
    fn default() -> Self {
        CompareWindow {
            open: false,
            other: None,
            loading: None,
            open_is_newer: true,
            opt: Options::default(),
            result: None,
            error: None,
            filter: String::new(),
            hide: Vec::new(),
            typed: String::new(),
        }
    }
}

const AREAS: [&str; 8] = [
    "board", "stack-up", "part", "net", "routing", "class", "pair", "rule",
];

fn colour(k: Kind) -> Color32 {
    match k {
        Kind::Added => Color32::from_rgb(90, 200, 120),
        Kind::Removed => Color32::from_rgb(235, 100, 90),
        Kind::Changed => Color32::from_rgb(230, 180, 70),
        Kind::Renamed => Color32::from_rgb(120, 170, 240),
    }
}

impl CompareWindow {
    /// Returns a spot (mil) and its text to show on the board.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        name: &str,
    ) -> Option<([f64; 2], String)> {
        if !self.open {
            return None;
        }
        if let Some(rx) = &self.loading {
            if let Ok(r) = rx.try_recv() {
                self.loading = None;
                match r {
                    Ok(o) => {
                        self.other = Some(o);
                        self.error = None;
                        self.run(board);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Compare boards")
            .open(&mut open)
            .default_width(720.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "What changed between this board and another version of it: parts, nets (pins joined or \
                         left, renamed), routing net by net, classes, pairs, rules and the stack-up.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    let busy = self.loading.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Choose the other board…"))
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter(
                                "Boards",
                                &["PcbDoc", "pcbdoc", "brd", trace_core::file::EXTENSION, "zip", "tgz"],
                            )
                            .pick_file()
                        {
                            self.read(p, ctx);
                        }
                    }
                    let typed = ui.add(
                        egui::TextEdit::singleline(&mut self.typed)
                            .hint_text("or paste its path")
                            .desired_width(220.0),
                    );
                    if typed.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !busy
                        && !self.typed.trim().is_empty()
                    {
                        let p = PathBuf::from(self.typed.trim().trim_matches('"'));
                        self.read(p, ctx);
                    }
                    if busy {
                        ui.spinner();
                        ui.label("reading…");
                    }
                    if let Some((p, _)) = &self.other {
                        ui.label(
                            p.file_name()
                                .map_or(String::new(), |n| n.to_string_lossy().to_string()),
                        );
                    }
                });
                ui.horizontal(|ui| {
                    let mut changed = ui
                        .checkbox(&mut self.open_is_newer, "This board is the newer one")
                        .on_hover_text("Off: the other board is the newer one, and the changes read the other way")
                        .changed();
                    ui.label("moved past");
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.opt.move_tol).speed(0.1).range(0.0..=1000.0).suffix(" mil"))
                        .changed();
                    ui.label("length past");
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.opt.length_tol).speed(0.5).range(0.0..=10000.0).suffix(" mil"))
                        .changed();
                    changed |= ui.checkbox(&mut self.opt.routing, "routing").changed();
                    if changed && self.other.is_some() {
                        self.run(board);
                    }
                    if self.other.is_some() && ui.button("Compare again").clicked() {
                        self.run(board);
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                let Some(r) = &self.result else {
                    return;
                };
                let (first, second) = match (&self.other, self.open_is_newer) {
                    (Some((p, _)), true) => (
                        p.file_name().map_or("other".into(), |n| n.to_string_lossy().to_string()),
                        name.to_string(),
                    ),
                    (Some((p, _)), false) => (
                        name.to_string(),
                        p.file_name().map_or("other".into(), |n| n.to_string_lossy().to_string()),
                    ),
                    _ => (String::new(), String::new()),
                };
                ui.separator();
                for (k, n) in [(0, &first), (1, &second)] {
                    let t = r.totals[k];
                    ui.label(format!(
                        "{} {n}: {} parts, {} nets, {} tracks, {} vias",
                        if k == 0 { "First" } else { "Second" },
                        t[0],
                        t[1],
                        t[2],
                        t[3]
                    ));
                }
                if r.changes.is_empty() {
                    ui.label(RichText::new("No differences.").strong().color(colour(Kind::Added)));
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for a in AREAS {
                        let n = r.count(a);
                        if n == 0 {
                            continue;
                        }
                        let mut on = !self.hide.contains(&a);
                        if ui.checkbox(&mut on, format!("{} {n}", compare::title(a))).changed() {
                            if on {
                                self.hide.retain(|x| *x != a);
                            } else {
                                self.hide.push(a);
                            }
                        }
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("filter: R12, GND, value")
                            .desired_width(140.0),
                    );
                });
                let f = self.filter.to_lowercase();
                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                    for a in AREAS {
                        if self.hide.contains(&a) {
                            continue;
                        }
                        let rows: Vec<&compare::Change> = r
                            .changes
                            .iter()
                            .filter(|c| c.area == a)
                            .filter(|c| {
                                f.is_empty()
                                    || c.what.to_lowercase().contains(&f)
                                    || c.detail.to_lowercase().contains(&f)
                            })
                            .collect();
                        if rows.is_empty() {
                            continue;
                        }
                        egui::CollapsingHeader::new(format!("{} ({})", compare::title(a), rows.len()))
                            .id_salt(a)
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::Grid::new(("cmp", a)).striped(true).num_columns(3).show(ui, |ui| {
                                    for c in rows.iter().take(2000) {
                                        ui.label(RichText::new(c.kind.word()).color(colour(c.kind)));
                                        // The spot on the open board.
                                        let spot = if self.open_is_newer { c.at } else { c.was.or(c.at) };
                                        match spot {
                                            Some(at) => {
                                                if ui.link(&c.what).on_hover_text("Show it on the board").clicked() {
                                                    out = Some((at, format!("{} {}: {}", c.what, c.kind.word(), c.detail)));
                                                }
                                            }
                                            None => {
                                                ui.label(&c.what);
                                            }
                                        }
                                        ui.label(RichText::new(&c.detail).small());
                                        ui.end_row();
                                    }
                                });
                            });
                    }
                });
                ui.horizontal(|ui| {
                    let save = |ext: &str, text: String| {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter(ext, &[ext])
                            .set_file_name(format!("compare.{ext}"))
                            .save_file()
                        {
                            let _ = std::fs::write(&path, text);
                        }
                    };
                    if ui.button("Save report…").clicked() {
                        save("txt", r.report(&first, &second));
                    }
                    if ui.button("Save CSV…").clicked() {
                        save("csv", r.csv());
                    }
                });
            });
        self.open = open;
        out
    }

    /// Reads the other board in the background.
    fn read(&mut self, p: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let r = crate::scene::Scene::load(p.clone()).map(|s| (p, s.board));
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.loading = Some(rx);
    }

    fn run(&mut self, board: &Board) {
        let Some((_, other)) = &self.other else {
            return;
        };
        self.result = Some(if self.open_is_newer {
            compare::compare(other, board, &self.opt)
        } else {
            compare::compare(board, other, &self.opt)
        });
    }
}
