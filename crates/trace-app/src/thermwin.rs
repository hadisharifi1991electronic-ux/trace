//! The thermal window: parts' power, package resistances and heatsinks,
//! the air; a 3D steady-state solve with Tj per part and the temperature
//! map of each copper layer.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::thermal::{self, Air, Heatsink, Setup, Source};
use trace_core::thermfield::{Sweep, TCut, TQuantity, TScale};
use trace_core::Board;

struct Row {
    part: usize,
    watts: f64,
    jc: f64,
    jb: f64,
    sink: bool,
    sink_text: String,
}

/// A supply whose copper loss (IR drop) heats the board.
struct RailRow {
    net: String,
    amps: f64,
    /// 0: from the net's name.
    volts: f64,
    source: String,
}

/// A drawn map: size, scale, plane name, value span.
type MapView = ([f32; 2], TScale, String, Option<(f64, f64)>);

type Solved = Result<
    (
        thermal::Result,
        Option<Vec<(String, f64, f64)>>,
        Option<String>,
    ),
    String,
>;

pub struct ThermalWindow {
    pub open: bool,
    rows: Vec<Row>,
    rails: Vec<RailRow>,
    /// Hot copper's resistivity fed back into the IR drop.
    couple: bool,
    /// The coupled solve's summary (rails cold and hot, passes).
    ir_text: Option<String>,
    /// Solve on the next frame (`TRACE_OPEN=thermal:...`).
    solve_now: bool,
    pick: String,
    ambient: f64,
    speed: f64,
    vertical: bool,
    check: bool,
    error: Option<String>,
    running: Option<mpsc::Receiver<Solved>>,
    result: Option<thermal::Result>,
    bars: Option<Vec<(String, f64, f64)>>,
    /// The plane shown (index into the field's planes), the quantity and
    /// the depth of the log scale.
    layer: usize,
    q: TQuantity,
    decades: f64,
    texture: Option<((usize, usize, i32), egui::TextureHandle)>,
    /// What the texture shows: size, scale, plane name, value span.
    view: Option<MapView>,
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
            rails: Vec::new(),
            couple: true,
            ir_text: None,
            solve_now: false,
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
            q: TQuantity::Temperature,
            decades: 3.0,
            texture: None,
            view: None,
            save_path: String::new(),
            note: None,
            overlay: None,
        }
    }
}

impl ThermalWindow {
    /// Opens with these rails (`NET=AMPS[@SOURCE][:VOLTS]`, `;` between)
    /// and solves at once (for screenshots).
    pub fn preset(&mut self, spec: &str) {
        for r in spec.split(';').filter(|r| !r.trim().is_empty()) {
            let (net, rest) = r.split_once('=').unwrap_or((r, "1"));
            let (rest, volts) = rest.split_once(':').unwrap_or((rest, "0"));
            let (amps, source) = rest.split_once('@').unwrap_or((rest, ""));
            self.rails.push(RailRow {
                net: net.trim().into(),
                amps: amps
                    .trim()
                    .trim_end_matches(['A', 'a'])
                    .parse()
                    .unwrap_or(1.0),
                volts: volts
                    .trim()
                    .trim_end_matches(['V', 'v'])
                    .parse()
                    .unwrap_or(0.0),
                source: source.trim().into(),
            });
        }
        self.open = true;
        self.solve_now = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, board: &Board, selected: Option<u32>) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    match r {
                        Ok((res, bars, ir)) => {
                            self.layer = 0;
                            self.result = Some(res);
                            self.bars = bars;
                            self.ir_text = ir;
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
        // Supplies whose copper loss heats the board.
        ui.horizontal(|ui| {
            ui.label(RichText::new("IR-drop heating").strong())
                .on_hover_text(
                "Each rail's DC current is solved through its planes, tracks and vias; the I²R \
                 lost in the copper heats the board where it is lost, and the hotter copper's \
                 resistivity (+0.393 %/K) feeds back into the drop until it settles. Loads share \
                 the current by pad count.",
            );
            if ui.button("Add rail").clicked() {
                self.rails.push(RailRow {
                    net: String::new(),
                    amps: 1.0,
                    volts: 0.0,
                    source: String::new(),
                });
            }
            ui.add_enabled(
                !self.rails.is_empty(),
                egui::Checkbox::new(&mut self.couple, "resistivity follows temperature"),
            );
        });
        let mut drop_rail = None;
        if !self.rails.is_empty() {
            egui::Grid::new("therm-rails")
                .num_columns(5)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for h in ["Net", "Current", "Volts", "Source", ""] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (k, r) in self.rails.iter_mut().enumerate() {
                        ui.add(egui::TextEdit::singleline(&mut r.net).desired_width(140.0));
                        ui.add(
                            egui::DragValue::new(&mut r.amps)
                                .speed(0.05)
                                .range(0.0..=500.0)
                                .suffix(" A"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut r.volts)
                                .speed(0.01)
                                .range(0.0..=60.0)
                                .suffix(" V"),
                        )
                        .on_hover_text("0: read from the net's name (1V8 = 1.8 V), else 1 V");
                        ui.add(
                            egui::TextEdit::singleline(&mut r.source)
                                .desired_width(70.0)
                                .hint_text("guess"),
                        );
                        if ui.small_button("✖").on_hover_text("Remove").clicked() {
                            drop_rail = Some(k);
                        }
                        ui.end_row();
                    }
                });
        }
        if let Some(k) = drop_rail {
            self.rails.remove(k);
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
            let ok = (!self.rows.is_empty() || !self.rails.is_empty()) && self.running.is_none();
            let now = std::mem::take(&mut self.solve_now) && ok;
            if ui.add_enabled(ok, egui::Button::new("Solve")).clicked() || now {
                let rails: Result<Vec<_>, String> = self
                    .rails
                    .iter()
                    .filter(|r| !r.net.trim().is_empty())
                    .map(|r| {
                        trace_core::etherm::rail(
                            board,
                            &r.net,
                            r.amps,
                            Some(r.source.as_str()),
                            Some(r.volts),
                        )
                    })
                    .collect();
                match self.setup().and_then(|s| rails.map(|r| (s, r))) {
                    Ok((setup, rails)) => {
                        let b = board.clone();
                        let check = self.check;
                        let couple = self.couple;
                        let (tx, rx) = mpsc::channel();
                        let c = ctx.clone();
                        let token = crate::jobs::start(ui.ctx(), "thermal");
                        std::thread::spawn(move || {
                            let _g = token.install();
                            let r = if !rails.is_empty() {
                                let mut es = trace_core::etherm::Setup::new(rails, setup);
                                es.couple = couple;
                                trace_core::etherm::solve(&b, &es).map(|r| {
                                    let text = r.report(&b);
                                    let ir: String = text
                                        .lines()
                                        .take_while(|l| !l.starts_with("Part "))
                                        .map(|l| format!("{l}\n"))
                                        .collect();
                                    (r.thermal, None, Some(ir))
                                })
                            } else if check {
                                thermal::check_mesh(&b, &setup)
                                    .map(|m| (m.fine, Some(m.error), None))
                            } else {
                                thermal::solve(&b, &setup).map(|r| (r, None, None))
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
        if let Some(t) = &self.ir_text {
            ui.label(RichText::new(t).monospace().size(11.0));
        }
        for n in &r.notes {
            ui.weak(n);
        }
        // Density maps: a quantity on any slab (from above) or a side view.
        let cuts = r.field.planes(true, None);
        ui.horizontal_wrapped(|ui| {
            ui.label("Show");
            egui::ComboBox::from_id_salt("therm-q")
                .selected_text(self.q.label())
                .show_ui(ui, |ui| {
                    for q in TQuantity::ALL {
                        ui.selectable_value(
                            &mut self.q,
                            q,
                            format!("{} ({})", q.label(), q.unit()),
                        );
                    }
                });
            ui.label("on");
            let name = |c: &TCut| match *c {
                TCut::Slab(s) => {
                    let sl = &r.field.slabs[s];
                    if sl.copper.is_some() {
                        sl.name.clone()
                    } else {
                        format!("  {} (dielectric)", sl.name)
                    }
                }
                TCut::Xz { .. } => "Side view along x (through the hottest point)".into(),
                TCut::Yz { .. } => "Side view along y (through the hottest point)".into(),
            };
            self.layer = self.layer.min(cuts.len() - 1);
            egui::ComboBox::from_id_salt("therm-plane")
                .selected_text(name(&cuts[self.layer]))
                .height(420.0)
                .show_ui(ui, |ui| {
                    for (k, c) in cuts.iter().enumerate() {
                        ui.selectable_value(&mut self.layer, k, name(c));
                    }
                });
            if ui
                .small_button("▲")
                .on_hover_text("The plane above")
                .clicked()
            {
                self.layer = self.layer.saturating_sub(1);
            }
            if ui
                .small_button("▼")
                .on_hover_text("The plane below")
                .clicked()
            {
                self.layer = (self.layer + 1).min(cuts.len() - 1);
            }
            if self.q.log() {
                ui.add(
                    egui::Slider::new(&mut self.decades, 1.0..=6.0)
                        .step_by(0.5)
                        .suffix(" decades"),
                )
                .on_hover_text(
                    "How far below the largest value the colour scale reaches (log scale)",
                );
            }
        });
        let k = self.layer;
        let key = (k, self.q as usize, (self.decades * 10.0) as i32);
        if self.texture.as_ref().is_none_or(|t| t.0 != key) {
            let f = &r.field;
            let cut = cuts[k];
            // One scale for every plane of this quantity, so moving
            // through the board compares like with like.
            let sc = f.scale(self.q, &cuts, self.decades);
            let plane = f.plane(cut, self.q);
            let tex = sc.map(|sc| {
                let fr = plane.frame(760, 220);
                let img = plane.draw(f, &sc, fr);
                let px: Vec<Color32> = img
                    .rgb
                    .chunks(3)
                    .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
                    .collect();
                let tex = ctx.load_texture(
                    "thermal-map",
                    egui::ColorImage {
                        size: [img.w, img.h],
                        pixels: px,
                    },
                    egui::TextureOptions::LINEAR,
                );
                (tex, [img.w as f32, img.h as f32], sc)
            });
            // The same map on the board (a slab), with each part's
            // junction; a side view keeps the board showing the top layer.
            let slab = match cut {
                TCut::Slab(s) => s,
                _ => 0,
            };
            if let Some(sc) = sc {
                let rgba = f.rgba(slab, self.q, &sc);
                let px: Vec<Color32> = rgba
                    .iter()
                    .map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
                    .collect();
                let btex = ctx.load_texture(
                    "thermal-board",
                    egui::ColorImage {
                        size: [f.cols, f.rows],
                        pixels: px,
                    },
                    egui::TextureOptions::NEAREST,
                );
                let mil = 1.0 / 0.0254;
                let (x0, y0) = (f.origin[0] * mil, f.origin[1] * mil);
                let (x1, y1) = (
                    x0 + f.cols as f64 * f.cell * mil,
                    y0 + f.rows as f64 * f.cell * mil,
                );
                let col = |v: f64| {
                    let c = sc.colour(v).unwrap_or([128, 128, 128]);
                    Color32::from_rgb(c[0], c[1], c[2])
                };
                let (lo, hi) = (sc.lo, sc.hi);
                let mid = if self.q.log() {
                    (lo * hi).sqrt()
                } else {
                    (lo + hi) / 2.0
                };
                let tcol = |t: f64| {
                    let tsc = f
                        .scale(TQuantity::Temperature, &cuts, 3.0)
                        .and_then(|s| s.colour(t))
                        .unwrap_or([200, 200, 200]);
                    Color32::from_rgb(tsc[0], tsc[1], tsc[2])
                };
                let markers = r
                    .parts
                    .iter()
                    .filter_map(|p| {
                        let c = board.components.get(p.part)?;
                        Some(crate::overlay::Marker {
                            at: [c.x.mil(), c.y.mil()],
                            color: tcol(p.junction_c),
                            label: format!("{} Tj {:.1} °C", p.designator, p.junction_c),
                        })
                    })
                    .collect();
                let fmt = |v: f64| {
                    if self.q.log() {
                        format!("{} {}", trace_core::emfield::si(v), self.q.unit())
                    } else {
                        format!("{v:.1} {}", self.q.unit())
                    }
                };
                self.overlay = Some(crate::overlay::Overlay {
                    title: format!("Thermal: {} on {}", self.q.label(), f.slabs[slab].name),
                    markers,
                    nets: Vec::new(),
                    image: Some((btex, [x0 as f32, y0 as f32, x1 as f32, y1 as f32])),
                    legend: vec![(col(lo), fmt(lo)), (col(mid), fmt(mid)), (col(hi), fmt(hi))],
                    on: true,
                });
            }
            match tex {
                Some((tex, size, sc)) => {
                    self.texture = Some((key, tex));
                    self.view = Some((size, sc, plane.name.clone(), plane.span(self.q.log())));
                }
                None => {
                    self.texture = None;
                    self.view = None;
                }
            }
        }
        match (&self.texture, &self.view) {
            (Some((_, tex)), Some((size, sc, name, span))) => {
                let w = ui.available_width().min(760.0);
                let mut h = w * size[1] / size[0].max(1.0);
                let mut w = w;
                if h > 520.0 {
                    w *= 520.0 / h;
                    h = 520.0;
                }
                ui.horizontal(|ui| {
                    ui.strong(name);
                    if let Some((lo, hi)) = span {
                        ui.weak(format!(
                            "{} to {} {}",
                            trace_core::emfield::si(*lo),
                            trace_core::emfield::si(*hi),
                            self.q.unit()
                        ));
                    }
                });
                ui.horizontal(|ui| {
                    ui.image((tex.id(), egui::vec2(w, h)));
                    // Scale bar.
                    let (rect, _) = ui
                        .allocate_exact_size(egui::vec2(90.0, h.max(140.0)), egui::Sense::hover());
                    let p = ui.painter();
                    let bar = egui::Rect::from_min_size(
                        rect.min + egui::vec2(4.0, 8.0),
                        egui::vec2(14.0, rect.height() - 16.0),
                    );
                    let n = 64;
                    for i in 0..n {
                        let t = 1.0 - i as f64 / n as f64;
                        let v = if sc.q.log() {
                            sc.lo * (sc.hi / sc.lo).powf(t)
                        } else {
                            sc.lo + t * (sc.hi - sc.lo)
                        };
                        let c = sc.colour(v).unwrap_or([0, 0, 0]);
                        let y0 = bar.top() + bar.height() * i as f32 / n as f32;
                        p.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(bar.left(), y0),
                                egui::pos2(bar.right(), y0 + bar.height() / n as f32 + 0.5),
                            ),
                            0.0,
                            Color32::from_rgb(c[0], c[1], c[2]),
                        );
                    }
                    let lab = |v: f64| {
                        if sc.q.log() {
                            trace_core::emfield::si(v)
                        } else {
                            format!("{v:.1}")
                        }
                    };
                    let tc = ui.visuals().text_color();
                    let fid = egui::FontId::proportional(11.0);
                    p.text(
                        egui::pos2(bar.right() + 4.0, bar.top()),
                        egui::Align2::LEFT_CENTER,
                        lab(sc.hi),
                        fid.clone(),
                        tc,
                    );
                    p.text(
                        egui::pos2(bar.right() + 4.0, bar.bottom()),
                        egui::Align2::LEFT_CENTER,
                        lab(sc.lo),
                        fid.clone(),
                        tc,
                    );
                    p.text(
                        egui::pos2(bar.right() + 4.0, bar.center().y),
                        egui::Align2::LEFT_CENTER,
                        sc.q.unit(),
                        fid,
                        tc,
                    );
                });
            }
            _ => {
                ui.weak(format!("No {} on this plane.", self.q.label()));
            }
        }
        ui.horizontal(|ui| {
            ui.label("Save to");
            ui.add(
                egui::TextEdit::singleline(&mut self.save_path)
                    .hint_text("folder\\name (maps.svg, maps.gif, temps.csv, report.txt)")
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
                let cuts = r.field.planes(false, None);
                let res = w(
                    ".svg",
                    r.field.svg("Thermal density maps", &TQuantity::ALL, &cuts, self.decades),
                )
                .and_then(|_| w(".csv", r.csv()))
                .and_then(|_| w(".txt", r.report()))
                .and_then(|_| {
                    std::fs::write(
                        format!("{base}.gif"),
                        r.field.gif(self.q, Sweep::Depth, self.decades, 40),
                    )
                });
                self.note = Some(match res {
                    Ok(()) => format!(
                        "Wrote {base}.svg (every quantity, every layer and side views), .gif ({} down through the layers), .csv and .txt",
                        self.q.label()
                    ),
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
