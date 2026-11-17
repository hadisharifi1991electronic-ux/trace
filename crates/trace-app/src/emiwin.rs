//! The EMI check window: which nets are fast, the limit to meet; the
//! layout findings (with a button to show each on the board) and each
//! clock's estimated radiation.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::emi::sim::{RailSpec, SignalSpec, SimResult, SimSetup};
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
    /// The simulation: sources given, cables, rails, options, run, result.
    sim_signals: String,
    sim_cables: String,
    sim_rails: String,
    sim_cmos: bool,
    sim_planes: bool,
    sim_running: Option<mpsc::Receiver<Result<SimResult, String>>>,
    sim: Option<Result<SimResult, String>>,
    map_sel: usize,
    map_tex: Option<(usize, egui::TextureHandle)>,
    findings_overlay: Option<crate::overlay::Overlay>,
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
            sim_signals: String::new(),
            sim_cables: String::new(),
            sim_rails: String::new(),
            sim_cmos: true,
            sim_planes: true,
            sim_running: None,
            sim: None,
            map_sel: 0,
            map_tex: None,
            findings_overlay: None,
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
                        let o = on_board(&r, board);
                        self.findings_overlay = Some(o.clone());
                        self.overlay = Some(o);
                        self.map_tex = None;
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
        self.simulation(ui, board);
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

impl EmiWindow {
    fn sim_setup(&self) -> Result<SimSetup, String> {
        let mut s = SimSetup {
            limit: self.limit,
            margin_db: self.margin,
            cmos: self.sim_cmos,
            planes: self.sim_planes,
            ..Default::default()
        };
        let parts = |t: &str| -> Vec<String> {
            t.lines()
                .flat_map(|l| l.split(';'))
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        };
        for x in parts(&self.sim_signals) {
            s.signals.push(SignalSpec::parse(&x)?);
        }
        // Fast nets given above count as sources too.
        for x in parts(&self.nets) {
            s.signals.push(SignalSpec::parse(&x)?);
        }
        for x in parts(&self.sim_rails) {
            s.rails.push(RailSpec::parse(&x)?);
        }
        for x in parts(&self.sim_cables) {
            let (d, l) = x.split_once('=').ok_or(format!("{x}: J1=2m"))?;
            let l = l.trim().trim_end_matches('m');
            s.cables.push((
                d.trim().to_string(),
                l.parse().map_err(|_| format!("{x}: not metres"))?,
            ));
        }
        Ok(s)
    }

    fn simulation(&mut self, ui: &mut egui::Ui, board: &Board) {
        ui.separator();
        ui.label(RichText::new("EMI simulation").strong());
        ui.label(
            "Every net and pair as a source (its IBIS model, your line, or a default for its kind), \
             radiated by its outer traces, by the cables on the connectors (common mode), by the \
             power planes' cavity edges (PDN model with the capacitors) and the decoupling loops, \
             against the limit above; with each source's share, the fixes and what each gains, and \
             the near field on the board.",
        );
        egui::Grid::new("emi-sim-in").num_columns(2).show(ui, |ui| {
            ui.label("Sources");
            ui.add_sized(
                [520.0, 20.0],
                egui::TextEdit::singleline(&mut self.sim_signals).hint_text(
                    "JESD*=12.288Gbps@30ps,0.8V; DEV_CLK*=245.76MHz; RX1_IN*=2.4GHz,0dBm",
                ),
            );
            ui.end_row();
            ui.label("Cables");
            ui.add_sized(
                [520.0, 20.0],
                egui::TextEdit::singleline(&mut self.sim_cables)
                    .hint_text("J1=2m; J5=0.3m (else by connector kind)"),
            );
            ui.end_row();
            ui.label("Rails");
            ui.add_sized(
                [520.0, 20.0],
                egui::TextEdit::singleline(&mut self.sim_rails)
                    .hint_text("VCCINT=2A@250MHz; VDD_1V8=0.2A"),
            );
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.sim_cmos, "single-ended data nets too");
            ui.checkbox(&mut self.sim_planes, "power planes");
            let busy = self.sim_running.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new("Simulate"))
                .clicked()
            {
                match self.sim_setup() {
                    Ok(s) => {
                        let b = board.clone();
                        let (tx, rx) = mpsc::channel();
                        let ctx = ui.ctx().clone();
                        let token = crate::jobs::start(&ctx, "emi-sim");
                        std::thread::spawn(move || {
                            let _g = token.install();
                            let r = emi::sim::simulate(&b, &s, &|_| {});
                            let _ = tx.send(r);
                            ctx.request_repaint();
                        });
                        self.sim_running = Some(rx);
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            if busy {
                ui.spinner();
                ui.label("Simulating…");
                if crate::jobs::cancel_button(ui, "emi-sim") {
                    self.sim_running = None;
                }
            }
        });
        if let Some(rx) = &self.sim_running {
            if let Ok(r) = rx.try_recv() {
                self.sim = Some(r);
                self.sim_running = None;
                self.map_tex = None;
            }
        }
        let Some(res) = &self.sim else {
            return;
        };
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                ui.colored_label(Color32::from_rgb(230, 110, 90), e);
                return;
            }
        };
        let (wf, wm) = r.worst;
        let col = if wm < 0.0 {
            Color32::from_rgb(230, 100, 80)
        } else if wm < r.margin_db {
            Color32::from_rgb(230, 170, 60)
        } else {
            Color32::from_rgb(90, 190, 130)
        };
        ui.label(
            RichText::new(format!(
                "Worst margin {wm:+.1} dB at {:.1} MHz against {} · {} sources ({} from IBIS or given)",
                wf / 1e6,
                r.limit_kind.name(),
                r.signals.len(),
                r.signals.iter().filter(|s| !s.assumed).count()
            ))
            .strong()
            .color(col),
        );
        spectrum_plot(ui, r);
        egui::CollapsingHeader::new("Contributions, worst first")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .id_salt("emi-contrib")
                    .show(ui, |ui| {
                        egui::Grid::new("emi-contrib-g")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for h in ["Margin", "At", "How", "Source"] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for (c, f, m) in r.ranked().into_iter().take(60) {
                                    let cc = if m < 0.0 {
                                        Color32::from_rgb(230, 100, 80)
                                    } else if m < r.margin_db {
                                        Color32::from_rgb(230, 170, 60)
                                    } else {
                                        ui.visuals().text_color()
                                    };
                                    ui.colored_label(cc, format!("{m:+.1} dB"));
                                    ui.label(format!("{:.1} MHz", f / 1e6));
                                    ui.label(c.mechanism.label());
                                    ui.label(&c.source);
                                    ui.end_row();
                                }
                            });
                    });
            });
        if !r.fixes.is_empty() {
            ui.label(
                RichText::new(format!("Fixes, by what each gains at {:.1} MHz", wf / 1e6)).strong(),
            );
            for x in &r.fixes {
                ui.label(format!("{:>5.1} dB  {}", x.gain_db, x.text));
            }
        }
        egui::CollapsingHeader::new(format!("Sources ({})", r.signals.len())).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .id_salt("emi-src")
                .show(ui, |ui| {
                    for x in &r.signals {
                        ui.label(format!(
                            "{}  {}  {:.3} MHz  {:.0} ps  {:.2}  ({})",
                            x.name,
                            x.kind.label(),
                            x.freq / 1e6,
                            x.rise * 1e12,
                            x.swing,
                            x.origin
                        ));
                    }
                });
        });
        for n in &r.notes {
            ui.weak(format!("note: {n}"));
        }
        // Near-field map: pick one, shown here and on the board.
        if let Some(m) = &r.map {
            ui.horizontal(|ui| {
                ui.label(format!("Near field at {:.1} MHz:", m.f / 1e6));
                self.map_sel = self.map_sel.min(m.maps.len().saturating_sub(1));
                egui::ComboBox::from_id_salt("emi-map")
                    .width(260.0)
                    .selected_text(m.maps.get(self.map_sel).map_or("", |x| x.0.as_str()))
                    .show_ui(ui, |ui| {
                        for (i, x) in m.maps.iter().enumerate() {
                            if ui.selectable_value(&mut self.map_sel, i, &x.0).changed() {
                                self.map_tex = None;
                            }
                        }
                    });
            });
            if self
                .map_tex
                .as_ref()
                .is_none_or(|(k, _)| *k != self.map_sel)
            {
                if let Some((name, unit, vals)) = m.maps.get(self.map_sel) {
                    let top = vals.iter().cloned().fold(0.0f32, f32::max) as f64;
                    let mut px = Vec::with_capacity(m.w * m.h);
                    for j in (0..m.h).rev() {
                        for i in 0..m.w {
                            let v = vals[j * m.w + i] as f64;
                            let t = trace_core::emfield::scale_t(v, top, 40.0, 20.0);
                            px.push(if t <= 0.02 {
                                Color32::TRANSPARENT
                            } else {
                                let c = trace_core::emfield::colour(t);
                                Color32::from_rgb(c[0], c[1], c[2])
                            });
                        }
                    }
                    let img = egui::ColorImage {
                        size: [m.w, m.h],
                        pixels: px,
                    };
                    let tex = ui
                        .ctx()
                        .load_texture("emi-near", img, egui::TextureOptions::LINEAR);
                    let (x1, y1) = (m.x0 + m.w as f64 * m.cell, m.y0 + m.h as f64 * m.cell);
                    let mut o = self.findings_overlay.clone().unwrap_or_default();
                    o.title = format!("EMI near field: {name}");
                    o.image = Some((
                        tex.clone(),
                        [
                            (m.x0 - m.cell / 2.0) as f32,
                            (m.y0 - m.cell / 2.0) as f32,
                            (x1 - m.cell / 2.0) as f32,
                            (y1 - m.cell / 2.0) as f32,
                        ],
                    ));
                    let c = |t: f64| {
                        let c = trace_core::emfield::colour(t);
                        Color32::from_rgb(c[0], c[1], c[2])
                    };
                    o.legend
                        .push((c(1.0), format!("{} {unit}", trace_core::emfield::si(top))));
                    o.legend.push((c(0.5), "−20 dB".into()));
                    o.legend.push((c(0.05), "−40 dB".into()));
                    o.on = true;
                    self.overlay = Some(o);
                    self.map_tex = Some((self.map_sel, tex));
                }
            }
            if let Some((_, tex)) = &self.map_tex {
                let w = ui.available_width().min(560.0);
                let h = w * m.h as f32 / m.w.max(1) as f32;
                ui.image((tex.id(), egui::vec2(w, h.min(520.0))));
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Save spectrum (SVG)…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .set_file_name("emi-spectrum.svg")
                    .save_file()
                {
                    let _ = std::fs::write(&p, r.svg("EMI"));
                    self.note = Some(format!("Saved {}", p.display()));
                }
            }
            if ui.button("Save near field (SVG)…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .set_file_name("emi-near.svg")
                    .save_file()
                {
                    if let Some(t) = r.map_svg(board, "EMI") {
                        let _ = std::fs::write(&p, t);
                        self.note = Some(format!("Saved {}", p.display()));
                    }
                }
            }
            if ui.button("Save report…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Text", &["txt"])
                    .set_file_name("emi-sim.txt")
                    .save_file()
                {
                    let _ = std::fs::write(&p, r.report());
                    self.note = Some(format!("Saved {}", p.display()));
                }
            }
        });
    }
}

/// The spectrum against the limit: the total, the top sources and the
/// limit, on a log frequency axis.
fn spectrum_plot(ui: &mut egui::Ui, r: &SimResult) {
    use eframe::egui::{pos2, vec2, Stroke};
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().min(820.0), 260.0),
        egui::Sense::hover(),
    );
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let lo = (r.limit.iter().cloned().fold(f64::MAX, f64::min) - 60.0).floor();
    let hi = (r
        .limit
        .iter()
        .chain(r.total.iter())
        .cloned()
        .fold(f64::MIN, f64::max)
        + 10.0)
        .ceil();
    let (l, rt, t, b) = (
        rect.left() + 44.0,
        rect.right() - 8.0,
        rect.top() + 8.0,
        rect.bottom() - 22.0,
    );
    let fx = |f: f64| l + (rt - l) * ((f / 30e6).ln() / (6e9f64 / 30e6).ln()) as f32;
    let fy = |d: f64| b - (b - t) * ((d.max(lo) - lo) / (hi - lo)) as f32;
    let grid = ui.visuals().weak_text_color().gamma_multiply(0.3);
    let font = egui::FontId::proportional(10.0);
    let mut d = (lo / 10.0).ceil() * 10.0;
    while d <= hi {
        p.line_segment([pos2(l, fy(d)), pos2(rt, fy(d))], Stroke::new(0.5f32, grid));
        p.text(
            pos2(l - 4.0, fy(d)),
            egui::Align2::RIGHT_CENTER,
            format!("{d:.0}"),
            font.clone(),
            ui.visuals().weak_text_color(),
        );
        d += 10.0;
    }
    for f in [30e6, 100e6, 300e6, 1e9, 3e9, 6e9] {
        p.line_segment([pos2(fx(f), t), pos2(fx(f), b)], Stroke::new(0.5f32, grid));
        let lab = if f >= 1e9 {
            format!("{} GHz", f / 1e9)
        } else {
            format!("{} MHz", f / 1e6)
        };
        p.text(
            pos2(fx(f), b + 2.0),
            egui::Align2::CENTER_TOP,
            lab,
            font.clone(),
            ui.visuals().weak_text_color(),
        );
    }
    let cols = [
        Color32::from_rgb(42, 111, 219),
        Color32::from_rgb(224, 102, 61),
        Color32::from_rgb(58, 164, 91),
        Color32::from_rgb(155, 89, 182),
        Color32::from_rgb(212, 160, 23),
    ];
    for (i, (c, _, _)) in r.ranked().into_iter().take(5).enumerate() {
        for (k, &v) in c.db.iter().enumerate() {
            if v > lo {
                let x = fx(r.freqs[k]);
                p.line_segment(
                    [pos2(x, b), pos2(x, fy(v))],
                    Stroke::new(1.0f32, cols[i].gamma_multiply(0.7)),
                );
            }
        }
        p.text(
            pos2(rt - 4.0, t + 4.0 + 12.0 * i as f32),
            egui::Align2::RIGHT_TOP,
            format!("{} ({})", c.source, c.mechanism.label()),
            font.clone(),
            cols[i],
        );
    }
    let poly = |v: &[f64]| -> Vec<egui::Pos2> {
        v.iter()
            .enumerate()
            .filter(|(_, d)| **d > -199.0)
            .map(|(k, &d)| pos2(fx(r.freqs[k]), fy(d)))
            .collect()
    };
    p.add(egui::Shape::line(
        poly(&r.total),
        Stroke::new(1.0f32, ui.visuals().text_color()),
    ));
    p.add(egui::Shape::line(
        poly(&r.limit),
        Stroke::new(2.0f32, Color32::from_rgb(214, 39, 40)),
    ));
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
