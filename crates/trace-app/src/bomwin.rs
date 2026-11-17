//! The BOM window: the bill of materials with each part number's
//! lifecycle, stock, price and datasheet, and what stops a build.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::bom::{self, Bom, Db, Level, Lifecycle};
use trace_core::Board;

pub const STORE_KEY: &str = "bom_mouser_key";

type Fetched = Result<(Vec<bom::PartData>, Vec<String>), String>;

enum Msg {
    Progress(usize, usize),
    Done(Fetched),
}

pub struct BomWindow {
    pub open: bool,
    boards: u64,
    bom: Option<Bom>,
    /// What the BOM was built from: parts, pads, variant, boards, data.
    built_for: Option<(usize, usize, Option<usize>, u64, usize)>,
    db: Db,
    cache_loaded: bool,
    files: Vec<String>,
    typed: String,
    pub mouser_key: String,
    again: bool,
    fetching: Option<mpsc::Receiver<Msg>>,
    progress: (usize, usize),
    status: Option<(String, bool)>,
    issues_only: bool,
    filter: String,
}

impl Default for BomWindow {
    fn default() -> Self {
        BomWindow {
            open: false,
            boards: 1,
            bom: None,
            built_for: None,
            db: Db::default(),
            cache_loaded: false,
            files: Vec::new(),
            typed: String::new(),
            mouser_key: String::new(),
            again: false,
            fetching: None,
            progress: (0, 0),
            status: None,
            issues_only: false,
            filter: String::new(),
        }
    }
}

/// What the window asks the app to do.
pub enum BomAct {
    /// Select these parts, showing the first.
    Select(Vec<usize>, String),
}

fn level_colour(l: Option<Level>) -> Color32 {
    match l {
        Some(Level::Error) => Color32::from_rgb(235, 100, 90),
        Some(Level::Warning) => Color32::from_rgb(230, 180, 70),
        Some(Level::Info) => Color32::from_rgb(140, 170, 220),
        None => Color32::GRAY,
    }
}

fn life_colour(l: Lifecycle) -> Color32 {
    match l {
        Lifecycle::Obsolete => level_colour(Some(Level::Error)),
        Lifecycle::Nrnd | Lifecycle::LastTimeBuy => level_colour(Some(Level::Warning)),
        Lifecycle::Active | Lifecycle::New => Color32::from_rgb(90, 200, 120),
        Lifecycle::Unknown => Color32::GRAY,
    }
}

impl BomWindow {
    pub fn with_key(mouser_key: String) -> Self {
        BomWindow {
            mouser_key,
            ..Default::default()
        }
    }

    fn load_file(&mut self, path: &str) {
        let path = path.trim().trim_matches('"');
        match std::fs::read_to_string(path) {
            Err(e) => self.status = Some((format!("Could not read {path}: {e}"), true)),
            Ok(text) => {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map_or(path.to_string(), |n| n.to_string_lossy().to_string());
                match bom::parts_csv(&text, &name) {
                    Ok(p) => {
                        let n = self.db.add(p);
                        self.status = Some((format!("{n} part numbers from {name}"), false));
                        if !self.files.contains(&name) {
                            self.files.push(name);
                        }
                        self.built_for = None;
                    }
                    Err(e) => self.status = Some((format!("{name}: {e}"), true)),
                }
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<BomAct> {
        if !self.open {
            return None;
        }
        if !self.cache_loaded {
            self.cache_loaded = true;
            let c = bom::load_cache();
            self.db.add(c.parts.into_values().collect());
        }
        if let Some(rx) = &self.fetching {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(d, n) => self.progress = (d, n),
                    Msg::Done(r) => {
                        self.fetching = None;
                        match r {
                            Ok((got, notes)) => {
                                let n = got.len();
                                let mut cache = bom::load_cache();
                                cache.add(got.clone());
                                let saved = bom::save_cache(&cache);
                                self.db.add(got);
                                self.built_for = None;
                                self.status = Some((
                                    format!(
                                        "Mouser: {n} found{}{}",
                                        if notes.is_empty() {
                                            String::new()
                                        } else {
                                            format!(", {} not found", notes.len())
                                        },
                                        match saved {
                                            Ok(()) => String::new(),
                                            Err(e) => format!(" (cache not saved: {e})"),
                                        }
                                    ),
                                    false,
                                ));
                            }
                            Err(e) => self.status = Some((e, true)),
                        }
                        break;
                    }
                }
            }
        }
        let stamp = (
            board.components.len(),
            board.geometry.pads.len(),
            board.variant,
            self.boards,
            self.db.parts.len(),
        );
        if self.built_for != Some(stamp) {
            let mut b = bom::build(board);
            bom::check(&mut b, &self.db, self.boards);
            self.bom = Some(b);
            self.built_for = Some(stamp);
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Bill of materials")
            .open(&mut open)
            .default_width(980.0)
            .default_height(620.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Parts grouped by part number, value and footprint, with each part number's \
                         lifecycle, stock and price from a parts file, earlier lookups, or Mouser.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    ui.label("Boards");
                    ui.add(egui::DragValue::new(&mut self.boards).range(1..=1_000_000).speed(1.0));
                    ui.separator();
                    if ui
                        .button("Load parts file…")
                        .on_hover_text(
                            "CSV with MPN and any of Manufacturer, Lifecycle, Stock, Price or Price@1, \
                             Price@100 ..., Currency, Datasheet (a distributor's export works)",
                        )
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("CSV", &["csv", "txt"])
                            .pick_file()
                        {
                            self.load_file(&p.display().to_string());
                        }
                    }
                    let t = ui.add(
                        egui::TextEdit::singleline(&mut self.typed)
                            .hint_text("or paste its path")
                            .desired_width(200.0),
                    );
                    if t.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.typed.trim().is_empty()
                    {
                        let p = std::mem::take(&mut self.typed);
                        self.load_file(&p);
                    }
                    if !self.files.is_empty() {
                        ui.label(RichText::new(self.files.join(", ")).small());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Mouser API key");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mouser_key)
                            .password(true)
                            .desired_width(260.0),
                    )
                    .on_hover_text("Free from mouser.com/api-hub (Search API). Kept on this computer.");
                    ui.checkbox(&mut self.again, "look up again")
                        .on_hover_text("Also the part numbers already known (fresh stock and prices)");
                    let busy = self.fetching.is_some();
                    let want = self
                        .bom
                        .as_ref()
                        .map(|b| bom::to_look_up(b, &self.db, self.again))
                        .unwrap_or_default();
                    if ui
                        .add_enabled(
                            !busy && !want.is_empty() && !self.mouser_key.trim().is_empty(),
                            egui::Button::new(format!("Look up {} at Mouser", want.len())),
                        )
                        .clicked()
                    {
                        let (tx, rx) = mpsc::channel();
                        let key = self.mouser_key.clone();
                        let c = ctx.clone();
                        self.progress = (0, want.len());
                        let token = crate::jobs::start(ui.ctx(), "bom");
                        std::thread::spawn(move || {
                            let _g = token.install();
                            let t2 = tx.clone();
                            let c2 = c.clone();
                            let r = bom::mouser_fetch(&key, &want, &bom::today(), move |d, n| {
                                let _ = t2.send(Msg::Progress(d, n));
                                c2.request_repaint();
                            });
                            let _ = tx.send(Msg::Done(r));
                            c.request_repaint();
                        });
                        self.fetching = Some(rx);
                    }
                    if busy {
                        ui.spinner();
                        if crate::jobs::cancel_button(ui, "bom") {
                            self.fetching = None;
                        }
                        ui.label(format!("{} of {}", self.progress.0, self.progress.1));
                    }
                    if ui
                        .small_button("Forget lookups")
                        .on_hover_text("Clear the part data kept from earlier lookups")
                        .clicked()
                    {
                        let _ = bom::save_cache(&Db::default());
                        self.db = Db::default();
                        self.files.clear();
                        self.built_for = None;
                    }
                });
                if let Some((s, bad)) = &self.status {
                    ui.colored_label(if *bad { Color32::LIGHT_RED } else { Color32::GRAY }, s);
                }
                let Some(b) = &self.bom else {
                    return;
                };
                ui.separator();
                let c = bom::cost(b);
                let with = b.lines.iter().filter(|l| l.data.is_some()).count();
                ui.label(
                    RichText::new(format!(
                        "{}{} lines, {} parts per board; part data for {with} of {} lines{}",
                        if b.variant.is_empty() {
                            String::new()
                        } else {
                            format!("Variant {}: ", b.variant)
                        },
                        b.lines.len(),
                        b.parts(),
                        b.lines.len(),
                        if c.total > 0.0 {
                            format!(
                                "; {:.2} {} per board, {:.2} for {}{}",
                                c.per_board,
                                c.currency,
                                c.total,
                                b.boards,
                                if c.unpriced > 0 {
                                    format!(" ({} lines unpriced)", c.unpriced)
                                } else {
                                    String::new()
                                }
                            )
                        } else {
                            String::new()
                        }
                    ))
                    .strong(),
                );
                if !b.not_fitted.is_empty() || !b.left_out.is_empty() || !b.stacked.is_empty() {
                    let alts: Vec<String> = b.stacked.iter().map(|(_, d)| d.clone()).collect();
                    ui.label(
                        RichText::new(format!(
                            "Not fitted: {}   Not bought: {}   Alternative footprints left off: {}",
                            bom::trim_list(&b.not_fitted, 80),
                            bom::trim_list(&b.left_out, 80),
                            bom::trim_list(&alts, 80)
                        ))
                        .small(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    for (f, n) in b.counts() {
                        ui.label(
                            RichText::new(format!("{n} {}", f.word()))
                                .color(level_colour(Some(f.level()))),
                        );
                        ui.label("·");
                    }
                    ui.checkbox(&mut self.issues_only, "only lines with problems");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("filter: C12, 100n, Murata")
                            .desired_width(160.0),
                    );
                });
                let f = self.filter.to_lowercase();
                egui::ScrollArea::both().max_height(420.0).show(ui, |ui| {
                    egui::Grid::new("bom_grid").striped(true).num_columns(11).show(ui, |ui| {
                        for h in [
                            "#", "Qty", "Designators", "Value", "Maker", "Part number", "Life",
                            "Stock", "Price", "Problems", "",
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (i, l) in b.lines.iter().enumerate() {
                            if self.issues_only && l.worst().is_none_or(|w| w == Level::Info) {
                                continue;
                            }
                            if !f.is_empty() {
                                let hay = format!(
                                    "{} {} {} {} {}",
                                    l.designators.join(" "),
                                    l.value,
                                    l.manufacturer,
                                    l.mpn,
                                    l.description
                                )
                                .to_lowercase();
                                if !hay.contains(&f) {
                                    continue;
                                }
                            }
                            ui.label(format!("{}", i + 1));
                            ui.label(format!("{}", l.qty()));
                            if ui
                                .link(bom::trim_list(&l.designators, 30))
                                .on_hover_text(l.designators.join(" "))
                                .clicked()
                            {
                                out = Some(BomAct::Select(l.parts.clone(), l.designators[0].clone()));
                            }
                            ui.label(&l.value).on_hover_text(&l.description);
                            ui.label(&l.manufacturer);
                            let d = l.data.as_ref();
                            match d.map(|d| d.url.as_str()).filter(|u| !u.is_empty()) {
                                Some(u) => {
                                    ui.hyperlink_to(&l.mpn, u);
                                }
                                None => {
                                    ui.label(if l.code {
                                        RichText::new(&l.mpn).italics()
                                    } else {
                                        RichText::new(&l.mpn)
                                    });
                                }
                            }
                            match d {
                                Some(d) => {
                                    ui.label(RichText::new(d.lifecycle.word()).color(life_colour(d.lifecycle)));
                                    ui.label(d.stock.map_or(String::new(), |s| s.to_string()));
                                }
                                None => {
                                    ui.label("");
                                    ui.label("");
                                }
                            }
                            match bom::line_cost(l, b.boards) {
                                Some((u, t)) => {
                                    ui.label(format!("{u:.3} · {t:.2}")).on_hover_text("Unit price at this quantity · line total");
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            let words: Vec<&str> = l
                                .issues
                                .iter()
                                .map(|i| i.flag.word())
                                .collect();
                            let r = ui.label(
                                RichText::new(words.join(", "))
                                    .small()
                                    .color(level_colour(l.worst())),
                            );
                            if !l.issues.is_empty() {
                                r.on_hover_text(
                                    l.issues
                                        .iter()
                                        .map(|i| i.text.as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                );
                            }
                            if let Some(ds) = d.map(|d| d.datasheet.as_str()).filter(|s| !s.is_empty()) {
                                ui.hyperlink_to("datasheet", ds);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let save = |name: &str, ext: &str, text: String| {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter(ext, &[ext])
                            .set_file_name(name)
                            .save_file()
                        {
                            let _ = std::fs::write(&path, text);
                        }
                    };
                    if ui.button("Save BOM (CSV)…").clicked() {
                        save("bom.csv", "csv", b.csv());
                    }
                    if ui.button("Save report…").clicked() {
                        save("bom.txt", "txt", b.report());
                    }
                    if ui
                        .button("Save parts file…")
                        .on_hover_text("One row per part number to fill in (lifecycle, stock, prices) and load back")
                        .clicked()
                    {
                        save("parts.csv", "csv", b.parts_template());
                    }
                });
            });
        self.open = open;
        out
    }
}
