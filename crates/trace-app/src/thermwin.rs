//! The thermal window: parts' power, package resistances and heatsinks,
//! the air; a 3D steady-state solve with Tj per part and the temperature
//! map of each copper layer.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::thermal::{self, Air, Heatsink, Setup, Source};
use trace_core::Board;

struct Row {
    part: usize,
    watts: f64,
    jc: f64,
    jb: f64,
    sink: bool,
    sink_text: String,
}

type Solved = Result<(thermal::Result, Option<Vec<(String, f64, f64)>>), String>;

pub struct ThermalWindow {
    pub open: bool,
    rows: Vec<Row>,
    pick: String,
    ambient: f64,
    speed: f64,
    vertical: bool,
    check: bool,
    error: Option<String>,
    running: Option<mpsc::Receiver<Solved>>,
    result: Option<thermal::Result>,
    bars: Option<Vec<(String, f64, f64)>>,
    layer: usize,
    texture: Option<(usize, egui::TextureHandle)>,
    save_path: String,
    note: Option<String>,
    /// The temperature map and junction temperatures for the board.
    pub overlay: Option<crate::overlay::Overlay>,
}

impl Default for ThermalWindow {
    fn default() -> Self {
        ThermalWindow {
            open: false,
            rows: Vec::new(),
            pick: String::new(),
            ambient: 25.0,
            speed: 0.0,
            vertical: false,
            check: false,
            error: None,
            running: None,
            result: None,
            bars: None,
            layer: 0,
            texture: None,
            save_path: String::new(),
            note: None,
            overlay: None,
        }
    }
}

impl ThermalWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, selected: Option<u32>) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    match r {
                        Ok((res, bars)) => {
                            self.layer = self.layer.min(res.maps.len().saturating_sub(1));
                            self.result = Some(res);
                            self.bars = bars;
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                    self.texture = None;
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("Thermal (3D, with heatsinks)")
            .open(&mut open)
            .default_width(820.0)
            .show(ctx, |ui| self.body(ui, ctx, board, selected));
        self.open = open;
    }

    fn add(&mut self, board: &Board, part: usize) {
        if part >= board.components.len() || self.rows.iter().any(|r| r.part == part) {
            return;
        }
        let (jc, jb) = thermal::default_thetas(board, part);
        self.rows.push(Row {
            part,
            watts: 1.0,
            jc: (jc * 10.0).round() / 10.0,
            jb: (jb * 10.0).round() / 10.0,
            sink: false,
            sink_text: "30x30x15,fins=8,fin=1.2,base=3".into(),
        });
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) {
        ui.label(
            "Parts that dissipate power, with their datasheet θJC (junction to case top) and θJB \
             (junction to board). The values filled in are guesses from the part's size: use the datasheet's.",
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new("Add the selected part"),
                )
                .clicked()
            {
                if let Some(s) = selected {
                    self.add(board, s as usize);
                }
            }
            ui.label("or designator");
            let r = ui.add(egui::TextEdit::singleline(&mut self.pick).desired_width(80.0));
            if (ui.button("Add").clicked()
                || r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                && !self.pick.trim().is_empty()
            {
                match board
                    .components
                    .iter()
                    .position(|c| c.designator.eq_ignore_ascii_case(self.pick.trim()))
                {
                    Some(p) => {
                        self.add(board, p);
                        self.pick.clear();
                    }
                    None => self.error = Some(format!("no part {}", self.pick.trim())),
                }
            }
        });
        let mut remove = None;
        egui::Grid::new("therm-rows")
            .num_columns(6)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for h in ["Part", "Power", "θJC", "θJB", "Heatsink", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (k, r) in self.rows.iter_mut().enumerate() {
                    ui.label(
                        board
                            .components
                            .get(r.part)
                            .map_or("?", |c| c.designator.as_str()),
                    );
                    ui.add(
                        egui::DragValue::new(&mut r.watts)
                            .speed(0.05)
                            .range(0.0..=500.0)
                            .suffix(" W"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut r.jc)
                            .speed(0.1)
                            .range(0.01..=500.0)
                            .suffix(" K/W"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut r.jb)
                            .speed(0.1)
                            .range(0.01..=500.0)
                            .suffix(" K/W"),
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut r.sink, "");
                        ui.add_enabled_ui(r.sink, |ui| {
                            ui.add_sized(
                                [270.0, 20.0],
                                egui::TextEdit::singleline(&mut r.sink_text),
                            )
                        })
                        .inner
                        .on_hover_text(
                            "L x W x total height (mm), fins=N, fin=thickness, base=thickness, \
                             k=W/mK (200 aluminium), tim=mm@W/mK, contact=K·cm²/W, e=emissivity",
                        );
                    });
                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                        remove = Some(k);
                    }
                    ui.end_row();
                }
            });
        if let Some(k) = remove {
            self.rows.remove(k);
        }
        ui.horizontal(|ui| {
            ui.label("Ambient");
            ui.add(
                egui::DragValue::new(&mut self.ambient)
                    .speed(0.5)
                    .range(-40.0..=125.0)
                    .suffix(" °C"),
            );
            ui.label("Air");
            ui.add(
                egui::DragValue::new(&mut self.speed)
                    .speed(0.1)
                    .range(0.0..=20.0)
                    .suffix(" m/s"),
            )
            .on_hover_text("0: still air; otherwise air moving along the board's x");
            ui.add_enabled(
                self.speed == 0.0,
                egui::Checkbox::new(&mut self.vertical, "board on its edge"),
            );
            ui.checkbox(&mut self.check, "grid check").on_hover_text(
                "Solves again on a 1.5 times finer grid and gives each junction an error bar",
            );
        });
        ui.horizontal(|ui| {
            let ok = !self.rows.is_empty() && self.running.is_none();
            if ui.add_enabled(ok, egui::Button::new("Solve")).clicked() {
                match self.setup() {
                    Ok(setup) => {
                        let b = board.clone();
                        let check = self.check;
                        let (tx, rx) = mpsc::channel();
                        let c = ctx.clone();
                        let token = crate::jobs::start(ui.ctx(), "thermal");
                        std::thread::spawn(move || {
                            let _g = token.install();
                            let r = if check {
                                thermal::check_mesh(&b, &setup).map(|m| (m.fine, Some(m.error)))
                            } else {
                                thermal::solve(&b, &setup).map(|r| (r, None))
                            };
                            let _ = tx.send(r);
                            c.request_repaint();
                        });
                        self.running = Some(rx);
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            if self.running.is_some() {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "thermal") {
                    self.running = None;
                }
                ui.label("Solving…");
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 110, 90), e);
        }
        let Some(r) = &self.result else {
            return;
        };
        ui.separator();
        egui::Grid::new("therm-res")
            .num_columns(8)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                for h in [
                    "Part", "W", "Tj", "Case", "Board", "Heatsink", "To board", "θJA",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for p in &r.parts {
                    ui.label(&p.designator);
                    ui.label(format!("{:.2}", p.watts));
                    let bar = self
                        .bars
                        .as_ref()
                        .and_then(|b| b.iter().find(|e| e.0 == p.designator))
                        .map_or(String::new(), |e| format!(" ± {:.1}", e.2));
                    let hot = p.junction_c > 105.0;
                    let txt = format!("{:.1}{bar} °C", p.junction_c);
                    if hot {
                        ui.colored_label(Color32::from_rgb(230, 90, 70), txt);
                    } else {
                        ui.label(txt);
                    }
                    ui.label(format!("{:.1} °C", p.case_c));
                    ui.label(format!("{:.1} °C", p.board_c));
                    ui.label(p.heatsink_c.map_or("-".into(), |t| format!("{t:.1} °C")));
                    ui.label(format!("{:.0} %", p.to_board * 100.0));
                    ui.label(format!("{:.1} K/W", p.theta_ja));
                    ui.end_row();
                }
            });
        ui.label(format!(
            "Hottest board point {:.1} °C; air film top {:.1} W/m²K ({:.1} with radiation), bottom {:.1} ({:.1}); heat out: board {:.2} W, cases {:.2} W, heatsinks {:.2} W (balance {:+.3} %)",
            r.board_max.0, r.h_top.0, r.h_top.1, r.h_bottom.0, r.h_bottom.1, r.out_board, r.out_cases, r.out_sinks, r.balance_pct
        ));
        for n in &r.notes {
            ui.weak(n);
        }
        ui.horizontal(|ui| {
            for (k, m) in r.maps.iter().enumerate() {
                if ui.selectable_label(self.layer == k, &m.name).clicked() {
                    self.layer = k;
                }
            }
        });
        let k = self.layer.min(r.maps.len().saturating_sub(1));
        if self.texture.as_ref().is_none_or(|t| t.0 != k) {
            let (lo, hi) = r
                .maps
                .iter()
                .flat_map(|m| m.t.iter())
                .filter(|v| !v.is_nan())
                .fold((f64::MAX, f64::MIN), |a, &v| {
                    (a.0.min(v as f64), a.1.max(v as f64))
                });
            let m = &r.maps[k];
            let mut px = Vec::with_capacity(r.cols * r.rows);
            for j in (0..r.rows).rev() {
                for i in 0..r.cols {
                    let v = m.t[j * r.cols + i];
                    px.push(if v.is_nan() {
                        Color32::TRANSPARENT
                    } else {
                        let c = thermal::heat_color((v as f64 - lo) / (hi - lo).max(1e-9));
                        Color32::from_rgb(c[0], c[1], c[2])
                    });
                }
            }
            let img = egui::ColorImage {
                size: [r.cols, r.rows],
                pixels: px,
            };
            let tex = ctx.load_texture("thermal-map", img, egui::TextureOptions::NEAREST);
            // The same map on the board, with each part's junction.
            let mil = 1.0 / 0.0254;
            let (x0, y0) = (r.origin[0] * mil, r.origin[1] * mil);
            let (x1, y1) = (
                x0 + r.cols as f64 * r.cell * mil,
                y0 + r.rows as f64 * r.cell * mil,
            );
            let hot = |t: f64| {
                let c = thermal::heat_color((t - lo) / (hi - lo).max(1e-9));
                Color32::from_rgb(c[0], c[1], c[2])
            };
            let markers = r
                .parts
                .iter()
                .filter_map(|p| {
                    let c = board.components.get(p.part)?;
                    Some(crate::overlay::Marker {
                        at: [c.x.mil(), c.y.mil()],
                        color: hot(p.junction_c),
                        label: format!("{} Tj {:.1} °C", p.designator, p.junction_c),
                    })
                })
                .collect();
            self.overlay = Some(crate::overlay::Overlay {
                title: format!("Thermal: {}", m.name),
                markers,
                nets: Vec::new(),
                image: Some((tex.clone(), [x0 as f32, y0 as f32, x1 as f32, y1 as f32])),
                legend: vec![
                    (hot(lo), format!("{lo:.1} °C")),
                    (hot((lo + hi) / 2.0), format!("{:.1} °C", (lo + hi) / 2.0)),
                    (hot(hi), format!("{hi:.1} °C")),
                ],
                on: true,
            });
            self.texture = Some((k, tex));
        }
        if let Some((_, tex)) = &self.texture {
            let w = ui.available_width().min(760.0);
            let h = w * r.rows as f32 / r.cols.max(1) as f32;
            let (w, h) = if h > 520.0 {
                (520.0 * r.cols as f32 / r.rows.max(1) as f32, 520.0)
            } else {
                (w, h)
            };
            ui.image((tex.id(), egui::vec2(w, h)));
        }
        ui.horizontal(|ui| {
            ui.label("Save to");
            ui.add(
                egui::TextEdit::singleline(&mut self.save_path)
                    .hint_text("folder\\name (map.svg, temps.csv, report.txt)")
                    .desired_width(320.0),
            );
            if ui.button("Save").clicked() && !self.save_path.trim().is_empty() {
                let base = self
                    .save_path
                    .trim()
                    .trim_end_matches(['/', '\\'])
                    .to_string();
                let base = base
                    .trim_end_matches(".svg")
                    .trim_end_matches(".csv")
                    .trim_end_matches(".txt")
                    .to_string();
                let w = |ext: &str, text: String| std::fs::write(format!("{base}{ext}"), text);
                let res = w(".svg", r.svg("Temperatures"))
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
    }

    fn setup(&self) -> Result<Setup, String> {
        let mut s = Setup::new();
        s.ambient_c = self.ambient;
        s.air = if self.speed > 0.0 {
            Air::Forced { speed: self.speed }
        } else {
            Air::Natural {
                vertical: self.vertical,
            }
        };
        for r in &self.rows {
            s.sources.push(Source {
                part: r.part,
                watts: r.watts,
                theta_jc: r.jc,
                theta_jb: r.jb,
                heatsink: if r.sink {
                    Some(Heatsink::parse(&r.sink_text)?)
                } else {
                    None
                },
            });
        }
        Ok(s)
    }
}
