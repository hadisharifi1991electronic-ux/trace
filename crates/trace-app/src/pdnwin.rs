//! The PDN impedance window: a supply's impedance against frequency at
//! its loads, from the plane pair, the capacitors and the regulator, with
//! the target impedance, what-if capacitor changes and the optimiser.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::mpsc;
use trace_core::pdn::{
    self, band_text, farads_text, hz_text, ohm_text, CapModel, Curves, Model, Plan, Setup,
};
use trace_core::Board;

pub struct PdnWindow {
    pub open: bool,
    net: Option<u32>,
    ground: Option<u32>,
    volts: f64,
    ripple: f64,
    amps: f64,
    band_mhz: f64,
    fmax_mhz: f64,
    /// The chip's package and die capacitance, nF (0: left out), behind
    /// its package inductance, pH.
    die_nf: f64,
    pkg_ph: f64,
    /// Empty sites per chip the optimiser may fill.
    spares: usize,
    error: Option<String>,
    running: Option<mpsc::Receiver<Result<Model, String>>>,
    model: Option<Result<Model, String>>,
    /// Capacitors as now tried (one per `Model::decaps`).
    caps: Vec<Option<CapModel>>,
    built: Curves,
    bare: Curves,
    now: Curves,
    plan: Option<Plan>,
    load: usize,
    supplies: Option<Vec<u32>>,
    board_key: (usize, usize, usize),
    /// A sub-rail picked from the results: switch to it and analyse.
    jump: Option<u32>,
    auto_run: bool,
}

impl Default for PdnWindow {
    fn default() -> Self {
        PdnWindow {
            open: false,
            net: None,
            ground: None,
            volts: 1.0,
            ripple: 3.0,
            amps: 1.0,
            band_mhz: 100.0,
            fmax_mhz: 1000.0,
            die_nf: 0.0,
            pkg_ph: 50.0,
            spares: 0,
            error: None,
            running: None,
            model: None,
            caps: Vec::new(),
            built: Vec::new(),
            bare: Vec::new(),
            now: Vec::new(),
            plan: None,
            load: 0,
            supplies: None,
            board_key: (0, 0, 0),
            jump: None,
            auto_run: false,
        }
    }
}

impl PdnWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, selected: Option<u32>) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    if let Ok(m) = &r {
                        self.caps = m.as_built();
                        self.built = m.evaluate(&self.caps);
                        self.bare = m.evaluate(&vec![None; m.decaps.len()]);
                        self.now = self.built.clone();
                        self.load = 0;
                    }
                    self.model = Some(r);
                    self.plan = None;
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("PDN impedance")
            .open(&mut open)
            .default_width(860.0)
            .show(ctx, |ui| self.body(ui, board, selected));
        self.open = open;
    }

    /// Nets with copper pours and three or more part pads, not ground.
    fn supplies(&mut self, board: &Board) -> &[u32] {
        self.supplies.get_or_insert_with(|| {
            let mut pads = vec![0usize; board.nets.len()];
            for p in &board.geometry.pads {
                if let (Some(n), Some(_)) = (p.net, p.component) {
                    if let Some(k) = pads.get_mut(n as usize) {
                        *k += 1;
                    }
                }
            }
            let mut poured = vec![false; board.nets.len()];
            for p in &board.geometry.polygons {
                if let Some(k) = p.net.and_then(|n| poured.get_mut(n as usize)) {
                    *k = true;
                }
            }
            let mut v: Vec<u32> = (0..board.nets.len() as u32)
                .filter(|&n| {
                    let name = board.nets[n as usize].name.to_ascii_uppercase();
                    poured[n as usize]
                        && pads[n as usize] >= 3
                        && !name.contains("GND")
                        && !name.contains("VSS")
                        && !name.contains("EARTH")
                })
                .collect();
            v.sort_by(|a, b| pads[*b as usize].cmp(&pads[*a as usize]));
            v
        })
    }

    fn pick_net(&mut self, board: &Board, net: u32) {
        self.net = Some(net);
        self.ground = pdn::guess_ground(board, net);
        if let Some(v) = pdn::volts_in_name(&board.nets[net as usize].name) {
            self.volts = v;
        }
        self.model = None;
        self.plan = None;
    }

    fn setup(&self, net: u32) -> Setup {
        let mut s = Setup::new(net);
        s.ground = self.ground;
        s.volts = Some(self.volts);
        s.ripple_pct = self.ripple;
        s.amps = self.amps;
        s.target_fmax = self.band_mhz * 1e6;
        s.fmax = (self.fmax_mhz * 1e6).max(s.fmin * 10.0);
        s.die = (self.die_nf > 0.0).then_some(pdn::OnChip {
            farads: self.die_nf * 1e-9,
            esr: 5e-3,
            pkg_l: self.pkg_ph * 1e-12,
        });
        s.spares = self.spares;
        s
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board, selected: Option<u32>) {
        let key = (
            board as *const Board as usize,
            board.nets.len(),
            board.components.len(),
        );
        if key != self.board_key {
            *self = PdnWindow {
                open: true,
                board_key: key,
                ..Default::default()
            };
        }
        if let Some(n) = self.jump.take() {
            self.pick_net(board, n);
            self.auto_run = true;
        }
        let supplies = self.supplies(board).to_vec();
        if self.net.is_none() {
            if let Some(n) = selected
                .filter(|s| supplies.contains(s))
                .or(supplies.first().copied())
                .or(selected)
            {
                self.pick_net(board, n);
            }
        }
        let Some(net) = self.net else {
            ui.label("This board has no supply nets on a plane or pour. Select a net on the board, then come back here.");
            return;
        };
        let name = |n: u32| board.nets[n as usize].name.clone();
        ui.horizontal(|ui| {
            ui.label("Supply");
            let mut pick = None;
            egui::ComboBox::from_id_salt("pdn-net")
                .selected_text(name(net))
                .height(400.0)
                .show_ui(ui, |ui| {
                    if let Some(s) = selected.filter(|s| !supplies.contains(s)) {
                        if ui
                            .selectable_label(false, format!("{} (selected)", name(s)))
                            .clicked()
                        {
                            pick = Some(s);
                        }
                    }
                    for &n in &supplies {
                        if ui.selectable_label(n == net, name(n)).clicked() {
                            pick = Some(n);
                        }
                    }
                });
            if let Some(n) = pick.filter(|&n| n != net) {
                self.pick_net(board, n);
            }
            ui.label("over");
            let grounds: Vec<u32> = (0..board.nets.len() as u32)
                .filter(|&n| {
                    let u = name(n).to_ascii_uppercase();
                    u.contains("GND") || u.contains("VSS") || Some(n) == self.ground
                })
                .collect();
            egui::ComboBox::from_id_salt("pdn-gnd")
                .selected_text(self.ground.map(name).unwrap_or_else(|| "ground?".into()))
                .height(300.0)
                .show_ui(ui, |ui| {
                    for n in grounds {
                        if ui
                            .selectable_label(Some(n) == self.ground, name(n))
                            .clicked()
                        {
                            self.ground = Some(n);
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Supply");
            ui.add(
                egui::DragValue::new(&mut self.volts)
                    .speed(0.01)
                    .range(0.1..=60.0)
                    .suffix(" V"),
            );
            ui.label("ripple");
            ui.add(
                egui::DragValue::new(&mut self.ripple)
                    .speed(0.1)
                    .range(0.1..=20.0)
                    .suffix(" %"),
            );
            ui.label("current step");
            ui.add(
                egui::DragValue::new(&mut self.amps)
                    .speed(0.05)
                    .range(0.01..=500.0)
                    .suffix(" A"),
            );
            let target = self.volts * self.ripple / 100.0 / self.amps;
            ui.label(RichText::new(format!("target {}", ohm_text(target))).strong());
            ui.label("up to");
            ui.add(
                egui::DragValue::new(&mut self.band_mhz)
                    .speed(1.0)
                    .range(1.0..=5000.0)
                    .suffix(" MHz"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Chip capacitance");
            ui.add(
                egui::DragValue::new(&mut self.die_nf)
                    .speed(1.0)
                    .range(0.0..=10000.0)
                    .suffix(" nF"),
            )
            .on_hover_text("The chip's own capacitance inside its package (on-package and on-die), from its datasheet or IBIS/PDN model; 0 leaves it out");
            ui.label("through");
            ui.add(
                egui::DragValue::new(&mut self.pkg_ph)
                    .speed(1.0)
                    .range(1.0..=5000.0)
                    .suffix(" pH"),
            )
            .on_hover_text("The package's inductance from the balls to that capacitance");
            ui.label("new sites per chip");
            ui.add(egui::DragValue::new(&mut self.spares).range(0..=12))
                .on_hover_text("Empty spots round each chip where Optimise may add a capacitor");
        });
        ui.horizontal(|ui| {
            ui.label("Sweep to");
            ui.add(
                egui::DragValue::new(&mut self.fmax_mhz)
                    .speed(10.0)
                    .range(10.0..=5000.0)
                    .suffix(" MHz"),
            );
            let busy = self.running.is_some();
            let go = std::mem::take(&mut self.auto_run) && !busy;
            if ui
                .add_enabled(!busy, egui::Button::new("Analyse"))
                .on_hover_text(
                    "Solve the plane pair under the supply with its capacitors and regulator",
                )
                .clicked()
                || go
            {
                let (tx, rx) = mpsc::channel();
                let b = board.clone();
                let s = self.setup(net);
                let token = crate::jobs::start(ui.ctx(), "pdn");
                std::thread::spawn(move || {
                    let _g = token.install();
                    let _ = tx.send(pdn::model(&b, &s));
                });
                self.running = Some(rx);
            }
            if busy {
                ui.spinner();
                if crate::jobs::cancel_button(ui, "pdn") {
                    self.running = None;
                }
                ui.label("Solving the plane pair…");
            }
        });
        let Some(model) = &self.model else {
            ui.label(
                RichText::new("Pick the supply and press Analyse. Capacitors are the two-pad parts between the supply and ground; parts without a value get a typical one for their case.")
                    .weak()
                    .small(),
            );
            return;
        };
        let m = match model {
            Ok(m) => m,
            Err(e) => {
                ui.colored_label(Color32::LIGHT_RED, e);
                return;
            }
        };
        let des = |i: usize| board.components[i].designator.clone();
        let c = &m.cavity;
        ui.separator();
        ui.label(format!(
            "{} over {} ({}): {:.2} mil, εr {:.2}, {:.2} in², {:.2} nF of plane; {} capacitors{}; regulator {}",
            c.power_layer,
            c.ground_layer,
            c.ground_net,
            c.d_mil,
            c.er,
            c.area_in2,
            c.farads * 1e9,
            m.decaps.iter().filter(|d| !d.spare).count(),
            match m.decaps.iter().filter(|d| d.spare).count() {
                0 => String::new(),
                n => format!(" (+{n} new sites)"),
            },
            m.vrm
                .map(|v| if m.vrm_bead {
                    format!("fed by {} (bead)", des(v))
                } else {
                    des(v)
                })
                .unwrap_or_else(|| "none".into())
        ));
        for n in &m.notes {
            ui.label(RichText::new(n).weak().small());
        }
        if !m.sub_rails.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Sub-rails");
                for f in &m.sub_rails {
                    if ui
                        .button(&f.net_name)
                        .on_hover_text(format!(
                            "Fed through {} {}; analyse it",
                            f.designator, f.value
                        ))
                        .clicked()
                    {
                        self.jump = Some(f.net);
                    }
                }
            });
        }
        if m.loads.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Seen from");
                for (i, &l) in m.loads.iter().enumerate() {
                    ui.selectable_value(&mut self.load, i, des(l));
                }
            });
        }
        let li = self.load.min(m.loads.len() - 1);
        let mut series: Vec<(&str, Color32, &Vec<f64>)> = vec![
            ("planes only", Color32::GRAY, &self.bare[li]),
            ("as built", Color32::from_rgb(70, 140, 240), &self.built[li]),
        ];
        let changed = self.caps != m.as_built();
        if changed {
            series.push(("tried", Color32::from_rgb(60, 190, 110), &self.now[li]));
        }
        plot(ui, m, &series);
        if let Some((r, f, l)) = m.worst(&self.now) {
            let text = if r <= 1.0 {
                format!(
                    "Under the target up to {} (worst {r:.2}× at {}).",
                    hz_text(m.band),
                    hz_text(f)
                )
            } else {
                let bands: Vec<String> = m
                    .over(&self.now)
                    .iter()
                    .map(|(a, b)| band_text(*a, *b))
                    .collect();
                format!(
                    "Over the target: {r:.2}× at {} at {}; over at {}",
                    hz_text(f),
                    des(m.loads[l]),
                    bands.join(", ")
                )
            };
            ui.label(RichText::new(text).color(if r <= 1.0 {
                Color32::from_rgb(60, 190, 110)
            } else {
                Color32::LIGHT_RED
            }));
        }
        let mut redo = false;
        ui.horizontal(|ui| {
            if ui
                .button("Optimise")
                .on_hover_text("Pick values for the capacitor sites (within each case's range) to meet the target, then take off the ones not needed")
                .clicked()
            {
                let plan = pdn::optimise(m, &self.caps);
                self.caps = plan.caps.clone();
                self.plan = Some(plan);
                redo = true;
            }
            if ui
                .add_enabled(changed, egui::Button::new("Back to as built"))
                .clicked()
            {
                self.caps = m.as_built();
                self.plan = None;
                redo = true;
            }
            if ui
                .button("Save chart…")
                .on_hover_text("The curves as an SVG picture")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .set_file_name(format!("{} impedance.svg", board.nets[net as usize].name))
                    .save_file()
                {
                    let title = format!("{} impedance", board.nets[net as usize].name);
                    let svg = pdn::svg(m, &self.built, changed.then_some(&self.now), &title);
                    if let Err(e) = std::fs::write(&path, svg) {
                        self.error = Some(e.to_string());
                    }
                }
            }
            if let Some(p) = &self.plan {
                ui.label(p.summary());
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        ui.label(
            RichText::new("Tick a capacitor off, or pick another value, to see what it does; the board itself is not changed.")
                .weak()
                .small(),
        );
        egui::ScrollArea::vertical()
            .min_scrolled_height(220.0)
            .max_height(260.0)
            .show(ui, |ui| {
                egui::Grid::new("pdn-caps")
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Part").strong());
                        ui.label(RichText::new("Case").strong());
                        ui.label(RichText::new("On board").strong());
                        ui.label(RichText::new("Tried").strong());
                        ui.label(RichText::new("Mounting").strong());
                        ui.end_row();
                        for (k, d) in m.decaps.iter().enumerate() {
                            let mut on = self.caps[k].is_some();
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut on, &d.designator).changed() {
                                    self.caps[k] = on.then(|| self.caps[k].unwrap_or(d.model));
                                    redo = true;
                                }
                            });
                            ui.label(d.package.label());
                            let shown = if d.spare {
                                "new site".to_string()
                            } else if d.assumed {
                                format!("{} (assumed)", farads_text(d.model.farads))
                            } else {
                                farads_text(d.model.farads)
                            };
                            ui.label(shown).on_hover_text(&d.value);
                            let now = self.caps[k];
                            egui::ComboBox::from_id_salt(("pdn-v", k))
                                .selected_text(now.map_or("off".into(), |c| farads_text(c.farads)))
                                .show_ui(ui, |ui| {
                                    for &v in
                                        pdn::VALUES.iter().filter(|&&v| v <= d.package.max_farads())
                                    {
                                        if ui
                                            .selectable_label(
                                                now.map(|c| c.farads) == Some(v),
                                                farads_text(v),
                                            )
                                            .clicked()
                                        {
                                            self.caps[k] = Some(CapModel::mlcc(v, d.package));
                                            redo = true;
                                        }
                                    }
                                });
                            ui.label(format!("{:.2} nH", (d.mount + d.model.esl) * 1e9));
                            ui.end_row();
                        }
                    });
            });
        if redo {
            self.now = m.evaluate(&self.caps);
        }
    }
}

/// |Z| against frequency, both on log scales, with the target.
fn plot(ui: &mut egui::Ui, m: &Model, series: &[(&str, Color32, &Vec<f64>)]) {
    let w = ui.available_width().max(300.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 320.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let plot = rect
        .shrink2(egui::vec2(40.0, 18.0))
        .translate(egui::vec2(18.0, -4.0));
    let (f0, f1) = (
        m.freqs.first().copied().unwrap_or(1e5).log10(),
        m.freqs.last().copied().unwrap_or(1e9).log10(),
    );
    let all = series.iter().flat_map(|s| s.2.iter().copied());
    let (mut z0, mut z1) = all.fold((f64::MAX, f64::MIN), |a, z| (a.0.min(z), a.1.max(z)));
    if let Some(t) = m.target {
        z0 = z0.min(t);
        z1 = z1.max(t);
    }
    let (z0, z1) = (
        z0.max(1e-6).log10().floor(),
        z1.max(1e-5)
            .log10()
            .ceil()
            .max(z0.max(1e-6).log10().floor() + 1.0),
    );
    let x = |f: f64| plot.left() + plot.width() * ((f.log10() - f0) / (f1 - f0)) as f32;
    let y = |z: f64| {
        plot.bottom()
            - plot.height() * ((z.max(1e-9).log10() - z0) / (z1 - z0)).clamp(0.0, 1.0) as f32
    };
    let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text = ui.visuals().weak_text_color();
    let font = egui::FontId::proportional(11.0);
    let mut e = z0;
    while e <= z1 + 1e-9 {
        let z = 10f64.powf(e);
        p.line_segment(
            [
                egui::pos2(plot.left(), y(z)),
                egui::pos2(plot.right(), y(z)),
            ],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(plot.left() - 4.0, y(z)),
            egui::Align2::RIGHT_CENTER,
            ohm_text(z),
            font.clone(),
            text,
        );
        e += 1.0;
    }
    let mut e = f0.ceil();
    while e <= f1 + 1e-9 {
        let f = 10f64.powf(e);
        p.line_segment(
            [
                egui::pos2(x(f), plot.top()),
                egui::pos2(x(f), plot.bottom()),
            ],
            Stroke::new(1.0_f32, grid),
        );
        p.text(
            egui::pos2(x(f), plot.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            hz_text(f),
            font.clone(),
            text,
        );
        e += 1.0;
    }
    let red = Color32::from_rgb(230, 80, 70);
    if let Some(t) = m.target {
        let band = m.band.min(*m.freqs.last().unwrap_or(&m.band));
        p.line_segment(
            [egui::pos2(plot.left(), y(t)), egui::pos2(x(band), y(t))],
            Stroke::new(1.5_f32, red),
        );
    }
    for (i, (label, colour, z)) in series.iter().enumerate() {
        let pts: Vec<egui::Pos2> = m
            .freqs
            .iter()
            .zip(z.iter())
            .map(|(&f, &v)| egui::pos2(x(f), y(v)))
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8_f32, *colour)));
        p.text(
            egui::pos2(plot.right() - 4.0, plot.top() + 4.0 + 16.0 * i as f32),
            egui::Align2::RIGHT_TOP,
            *label,
            egui::FontId::proportional(12.0),
            *colour,
        );
    }
    if m.target.is_some() {
        p.text(
            egui::pos2(
                plot.right() - 4.0,
                plot.top() + 4.0 + 16.0 * series.len() as f32,
            ),
            egui::Align2::RIGHT_TOP,
            "target",
            egui::FontId::proportional(12.0),
            red,
        );
    }
    if let Some(pos) = resp.hover_pos() {
        if plot.contains(pos) {
            let fq = 10f64.powf(f0 + ((pos.x - plot.left()) / plot.width()) as f64 * (f1 - f0));
            if let Some(k) = (0..m.freqs.len())
                .min_by(|&a, &b| (m.freqs[a] - fq).abs().total_cmp(&(m.freqs[b] - fq).abs()))
            {
                p.line_segment(
                    [
                        egui::pos2(x(m.freqs[k]), plot.top()),
                        egui::pos2(x(m.freqs[k]), plot.bottom()),
                    ],
                    Stroke::new(1.0_f32, text),
                );
                let vals: Vec<String> = series
                    .iter()
                    .map(|(l, _, z)| format!("{l} {}", ohm_text(z[k])))
                    .collect();
                p.text(
                    egui::pos2(x(m.freqs[k]) + 6.0, plot.bottom() - 6.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}  {}", hz_text(m.freqs[k]), vals.join("  ")),
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
        }
    }
}
