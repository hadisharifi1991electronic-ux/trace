//! The EMI check window: which nets are fast, the limit to meet; the
//! layout findings (with a button to show each on the board) and each
//! clock's estimated radiation.

use eframe::egui::{self, Color32, RichText};
use trace_core::emi::{self, EmiResult, EmiSetup, FastNet, Limit, Severity};
use trace_core::Board;

pub struct EmiWindow {
    pub open: bool,
    nets: String,
    auto: bool,
    auto_mhz: f64,
    auto_ns: f64,
    auto_v: f64,
    limit: Limit,
    margin: f64,
    error: Option<String>,
    result: Option<EmiResult>,
    save_path: String,
    note: Option<String>,
    /// The findings for the board, when a check has run.
    pub overlay: Option<crate::overlay::Overlay>,
}

impl Default for EmiWindow {
    fn default() -> Self {
        EmiWindow {
            open: false,
            nets: String::new(),
            auto: true,
            auto_mhz: 50.0,
            auto_ns: 1.0,
            auto_v: 3.3,
            limit: Limit::FccB,
            margin: 6.0,
            error: None,
            result: None,
            save_path: String::new(),
            note: None,
            overlay: None,
        }
    }
}

/// Show a finding on the board: its net, where, and a line about it.
pub type Show = (Option<u32>, [f64; 2], String);

impl EmiWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<Show> {
        if !self.open {
            return None;
        }
        let mut act = None;
        let mut open = self.open;
        egui::Window::new("EMI check")
            .open(&mut open)
            .default_width(880.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(760.0)
                    .show(ui, |ui| act = self.body(ui, board));
            });
        self.open = open;
        act
    }

    fn setup(&self) -> Result<EmiSetup, String> {
        let mut s = EmiSetup {
            auto: self.auto,
            auto_freq: self.auto_mhz * 1e6,
            auto_rise: self.auto_ns * 1e-9,
            auto_volts: self.auto_v,
            limit: self.limit,
            margin_db: self.margin,
            ..Default::default()
        };
        for line in self.nets.lines().flat_map(|l| l.split(';')) {
            let line = line.trim();
            if !line.is_empty() {
                s.nets.push(FastNet::parse(line)?);
            }
        }
        Ok(s)
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board) -> Option<Show> {
        let mut act = None;
        ui.label(
            "Checks the fast nets (clocks, the nets you give, every pair) for tracks over plane gaps \
             and splits, layer changes with no return via, tracks near the edge and edge stitching, \
             and estimates what each clock radiates against the limit.",
        );
        ui.horizontal(|ui| {
            ui.label("Fast nets");
            ui.add_sized(
                [420.0, 20.0],
                egui::TextEdit::singleline(&mut self.nets)
                    .hint_text("CLK=25MHz@2ns,3.3V; SPI_SCK=20MHz"),
            )
            .on_hover_text("NET=FREQUENCY[@EDGE][,VOLTS], split by ;. Patterns with * and ? work.");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.auto, "clocks by name, taken at");
            ui.add_enabled_ui(self.auto, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.auto_mhz)
                        .speed(1.0)
                        .range(0.01..=5000.0)
                        .suffix(" MHz"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.auto_ns)
                        .speed(0.05)
                        .range(0.02..=50.0)
                        .suffix(" ns"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.auto_v)
                        .speed(0.05)
                        .range(0.1..=24.0)
                        .suffix(" V"),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label("Limit");
            egui::ComboBox::from_id_salt("emi-limit")
                .selected_text(self.limit.name())
                .show_ui(ui, |ui| {
                    for l in [Limit::FccB, Limit::FccA, Limit::CisprB, Limit::CisprA] {
                        ui.selectable_value(&mut self.limit, l, l.name());
                    }
                });
            ui.label("margin");
            ui.add(
                egui::DragValue::new(&mut self.margin)
                    .speed(0.5)
                    .range(0.0..=30.0)
                    .suffix(" dB"),
            );
            if ui.button("Check").clicked() {
                match self.setup() {
                    Ok(s) => {
                        let r = emi::check(board, &s);
                        self.overlay = Some(on_board(&r, board));
                        self.result = Some(r);
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 110, 90), e);
        }
        let Some(r) = &self.result else {
            return None;
        };
        ui.separator();
        let (hi, me, lo) = (
            r.count(Severity::High),
            r.count(Severity::Medium),
            r.count(Severity::Low),
        );
        ui.label(
            RichText::new(format!(
                "{} fast nets and pairs · {hi} high, {me} medium, {lo} low",
                r.fast_nets
            ))
            .strong()
            .color(if hi > 0 {
                Color32::from_rgb(230, 100, 80)
            } else if me > 0 {
                Color32::from_rgb(230, 170, 60)
            } else {
                Color32::from_rgb(90, 190, 130)
            }),
        );
        let net_id = |name: &str| {
            board
                .nets
                .iter()
                .position(|n| n.name == name)
                .map(|i| i as u32)
        };
        if !r.findings.is_empty() {
            egui::Grid::new("emi-find")
                .num_columns(5)
                .striped(true)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    for h in ["", "Kind", "Net", "What", ""] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for f in r.findings.iter().take(200) {
                        let c = match f.severity {
                            Severity::High => Color32::from_rgb(230, 100, 80),
                            Severity::Medium => Color32::from_rgb(230, 170, 60),
                            Severity::Low => ui.visuals().weak_text_color(),
                        };
                        ui.colored_label(c, f.severity.label());
                        ui.label(f.kind);
                        ui.label(&f.net);
                        ui.label(&f.text).on_hover_text(format!("Fix: {}", f.fix));
                        if ui.small_button("Show").clicked() {
                            act = Some((net_id(&f.net), f.at, format!("{}: {}", f.net, f.text)));
                        }
                        ui.end_row();
                    }
                });
        }
        if !r.emissions.is_empty() {
            ui.separator();
            ui.label(RichText::new("Estimated radiation (differential mode)").strong());
            egui::Grid::new("emi-rad")
                .num_columns(7)
                .striped(true)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    for h in [
                        "Net", "Clock", "Outer", "Loop", "Worst at", "Field", "Margin",
                    ] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for e in &r.emissions {
                        ui.label(&e.net);
                        ui.label(format!(
                            "{:.2} MHz, {:.2} ns{}",
                            e.freq / 1e6,
                            e.rise * 1e9,
                            if e.assumed { " (assumed)" } else { "" }
                        ));
                        ui.label(format!("{:.0} mil", e.outer));
                        ui.label(format!("{:.1} mm²", e.area_mm2));
                        match e.worst {
                            Some((f, d, l)) => {
                                ui.label(format!("{:.1} MHz", f / 1e6));
                                ui.label(format!("{d:.1} dBµV/m"));
                                let m = l - d;
                                let c = if m < 0.0 {
                                    Color32::from_rgb(230, 100, 80)
                                } else if m < r.margin_db {
                                    Color32::from_rgb(230, 170, 60)
                                } else {
                                    Color32::from_rgb(90, 190, 130)
                                };
                                ui.colored_label(c, format!("{m:+.1} dB"));
                            }
                            None => {
                                ui.label("-");
                                ui.label("-");
                                ui.label("-");
                            }
                        }
                        ui.end_row();
                    }
                });
        }
        for n in &r.notes {
            ui.weak(format!("note: {n}"));
        }
        ui.horizontal(|ui| {
            ui.label("Save to");
            ui.add_sized(
                [320.0, 20.0],
                egui::TextEdit::singleline(&mut self.save_path)
                    .hint_text("folder\\name (spectrum.svg, findings.csv, report.txt)"),
            );
            if ui.button("Save").clicked() && !self.save_path.trim().is_empty() {
                let base = self.save_path.trim().trim_end_matches(['/', '\\']);
                let base = base
                    .trim_end_matches(".svg")
                    .trim_end_matches(".csv")
                    .trim_end_matches(".txt")
                    .to_string();
                let w = |ext: &str, text: String| std::fs::write(format!("{base}{ext}"), text);
                let res = w(".svg", r.svg("EMI"))
                    .and_then(|_| w(".csv", r.csv()))
                    .and_then(|_| w(".txt", r.report()));
                self.note = Some(match res {
                    Ok(()) => format!("Wrote {base}.svg, .csv and .txt"),
                    Err(e) => format!("Could not save: {e}"),
                });
            }
        });
        if let Some(n) = &self.note {
            ui.weak(n);
        }
        act
    }
}

fn severity_colour(s: Severity) -> Color32 {
    match s {
        Severity::High => Color32::from_rgb(235, 70, 50),
        Severity::Medium => Color32::from_rgb(240, 170, 40),
        Severity::Low => Color32::from_rgb(120, 170, 240),
    }
}

/// The findings as markers on the board, and the nets with high
/// findings drawn in red.
fn on_board(r: &EmiResult, board: &Board) -> crate::overlay::Overlay {
    use crate::overlay::{Marker, Overlay};
    let mut nets: Vec<(u32, Color32)> = Vec::new();
    for f in r.findings.iter().filter(|f| f.severity == Severity::High) {
        if let Some(i) = board.nets.iter().position(|n| n.name == f.net) {
            if !nets.iter().any(|x| x.0 == i as u32) {
                nets.push((i as u32, Color32::from_rgba_unmultiplied(235, 70, 50, 200)));
            }
        }
    }
    Overlay {
        title: format!("EMI ({} findings)", r.findings.len()),
        markers: r
            .findings
            .iter()
            .map(|f| Marker {
                at: f.at,
                color: severity_colour(f.severity),
                label: format!("{} {}", f.kind, f.net),
            })
            .collect(),
        nets,
        image: None,
        legend: vec![
            (
                severity_colour(Severity::High),
                "high (net drawn red)".into(),
            ),
            (severity_colour(Severity::Medium), "medium".into()),
            (severity_colour(Severity::Low), "low".into()),
        ],
        on: true,
    }
}
