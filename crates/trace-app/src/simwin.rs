//! The Signal simulation window: pick a driver and a receiver pin on the
//! selected net, their IBIS files and models, and see the waveforms and
//! the eye.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::collections::HashMap;
use std::path::PathBuf;
use trace_core::ibis::{self, Corner, Ibis};
use trace_core::sim::{self, End, Pins, Run, Setup, Term};
use trace_core::{Board, Stackup};

/// Whether two stack-ups give the same layers and materials.
fn same_stackup(a: &Stackup, b: &Stackup) -> bool {
    a.layers.len() == b.layers.len()
        && a.layers.iter().zip(&b.layers).all(|(x, y)| {
            x.name == y.name
                && x.kind == y.kind
                && x.thickness == y.thickness
                && x.dielectric_constant == y.dielectric_constant
                && x.loss_tangent == y.loss_tangent
                && x.roughness_um == y.roughness_um
        })
}

#[derive(Clone, Copy, PartialEq)]
enum TermKind {
    None,
    ToRef,
    Across,
}

pub struct SimWindow {
    pub open: bool,
    /// The net the pins were picked for.
    net: Option<u32>,
    /// Driver and receiver: pin as designator.pin, IBIS file, model.
    pins: [String; 2],
    files: [Option<PathBuf>; 2],
    models: [Option<String>; 2],
    loaded: HashMap<PathBuf, Result<Ibis, String>>,
    rate_g: f64,
    bits: usize,
    corner: Corner,
    term: TermKind,
    term_r: f64,
    term_v: f64,
    pair: bool,
    lossy: bool,
    /// Each layer's own Df and roughness where the stack-up gives them.
    from_stackup: bool,
    tan_d: f64,
    rough_um: f64,
    crosstalk: bool,
    result: Option<Result<(Run, String), String>>,
}

impl Default for SimWindow {
    fn default() -> Self {
        SimWindow {
            open: false,
            net: None,
            pins: [String::new(), String::new()],
            files: [None, None],
            models: [None, None],
            loaded: HashMap::new(),
            rate_g: 1.0,
            bits: 160,
            corner: Corner::Typ,
            term: TermKind::None,
            term_r: 50.0,
            term_v: 0.0,
            pair: true,
            lossy: true,
            from_stackup: true,
            tan_d: sim::Loss::default().tan_d,
            rough_um: sim::Loss::default().rough_um,
            crosstalk: true,
            result: None,
        }
    }
}

/// The parts' pads on `net`, as designator.pin.
fn part_pins(board: &Board, net: u32) -> Vec<String> {
    let mut v: Vec<String> = board
        .geometry
        .pads
        .iter()
        .filter(|p| p.net == Some(net))
        .filter_map(|p| {
            let c = board.components.get(p.component? as usize)?;
            Some(format!("{}.{}", c.designator, p.name))
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

impl SimWindow {
    /// `stackup` is the stack-up as the Impedance window has it (edits
    /// included).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        stackup: &Stackup,
        selected: Option<u32>,
    ) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Signal simulation (IBIS)")
            .open(&mut open)
            .default_width(760.0)
            .show(ctx, |ui| self.body(ui, board, stackup, selected));
        self.open = open;
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board, stackup: &Stackup, selected: Option<u32>) {
        let Some(net) = selected else {
            ui.label("Select a net on the board (click a track or pad), then come back here.");
            return;
        };
        let pins = part_pins(board, net);
        if self.net != Some(net) {
            // A new net: its first two part pins.
            self.net = Some(net);
            self.pins = [
                pins.first().cloned().unwrap_or_default(),
                pins.get(1).cloned().unwrap_or_default(),
            ];
            self.result = None;
        }
        let name = board
            .nets
            .get(net as usize)
            .map_or("?", |n| n.name.as_str());
        let partner = board.diff_pairs.iter().find_map(|p| {
            if p.positive == name {
                Some(p.negative.clone())
            } else if p.negative == name {
                Some(p.positive.clone())
            } else {
                None
            }
        });
        ui.horizontal(|ui| {
            ui.label("Net");
            ui.label(RichText::new(name).strong());
            if let Some(p) = &partner {
                ui.checkbox(&mut self.pair, format!("as a pair with {p}"))
                    .on_hover_text("Drive both nets of the pair, the partner the other way, and look at the difference at the receiver");
            }
        });
        egui::Grid::new("sim_ends").num_columns(4).show(ui, |ui| {
            for (k, label) in ["Driver", "Receiver"].iter().enumerate() {
                ui.label(*label);
                egui::ComboBox::from_id_salt(("sim_pin", k))
                    .width(120.0)
                    .selected_text(if self.pins[k].is_empty() {
                        "pick a pin"
                    } else {
                        &self.pins[k]
                    })
                    .show_ui(ui, |ui| {
                        for p in &pins {
                            ui.selectable_value(&mut self.pins[k], p.clone(), p);
                        }
                    });
                let shown = self.files[k]
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or("IBIS file…".to_string(), |f| {
                        f.to_string_lossy().into_owned()
                    });
                if ui
                    .button(shown)
                    .on_hover_text("The .ibs file with this part's buffer models")
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("IBIS", &["ibs", "IBS"])
                        .pick_file()
                    {
                        let r = ibis::load(&p);
                        self.loaded.insert(p.clone(), r);
                        self.files[k] = Some(p.clone());
                        // The receiver often comes from the same file.
                        if k == 0 && self.files[1].is_none() {
                            self.files[1] = Some(p);
                        }
                        self.models[k] = None;
                    }
                }
                let models: Vec<String> = self.files[k]
                    .as_ref()
                    .and_then(|p| self.loaded.get(p))
                    .and_then(|r| r.as_ref().ok())
                    .map(|f| {
                        f.models
                            .iter()
                            .filter(|m| k == 1 || m.kind.drives())
                            .map(|m| m.name.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt(("sim_model", k))
                    .width(150.0)
                    .selected_text(
                        self.models[k]
                            .clone()
                            .unwrap_or_else(|| "from [Pin]".into()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.models[k], None, "from [Pin]")
                            .on_hover_text("The model the IBIS file's pin table gives this pin");
                        for m in &models {
                            ui.selectable_value(&mut self.models[k], Some(m.clone()), m);
                        }
                    });
                ui.end_row();
            }
        });
        for k in 0..2 {
            if let Some(Err(e)) = self.files[k].as_ref().and_then(|p| self.loaded.get(p)) {
                ui.colored_label(Color32::from_rgb(255, 120, 100), e);
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Rate");
            ui.add(
                egui::DragValue::new(&mut self.rate_g)
                    .range(0.001..=56.0)
                    .speed(0.01)
                    .suffix(" Gb/s"),
            );
            ui.label("Bits");
            ui.add(egui::DragValue::new(&mut self.bits).range(16..=2000));
            egui::ComboBox::from_id_salt("sim_corner")
                .selected_text(self.corner.label())
                .show_ui(ui, |ui| {
                    for c in [Corner::Typ, Corner::Min, Corner::Max] {
                        ui.selectable_value(&mut self.corner, c, c.label());
                    }
                });
            ui.label("Termination");
            egui::ComboBox::from_id_salt("sim_term")
                .selected_text(match self.term {
                    TermKind::None => "none",
                    TermKind::ToRef => "to a voltage",
                    TermKind::Across => "across the pair",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.term, TermKind::None, "none");
                    ui.selectable_value(&mut self.term, TermKind::ToRef, "to a voltage");
                    ui.selectable_value(&mut self.term, TermKind::Across, "across the pair");
                });
            if self.term != TermKind::None {
                ui.add(
                    egui::DragValue::new(&mut self.term_r)
                        .range(1.0..=10000.0)
                        .suffix(" Ω"),
                );
            }
            if self.term == TermKind::ToRef {
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut self.term_v)
                        .range(-5.0..=5.0)
                        .speed(0.01)
                        .suffix(" V"),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.lossy, "Track loss")
                .on_hover_text("Copper (DC, skin effect, roughness) and dielectric loss");
            ui.add_enabled_ui(self.lossy, |ui| {
                ui.checkbox(&mut self.from_stackup, "From the stack-up")
                    .on_hover_text(
                        "Each layer's own Df and roughness where the stack-up (Impedance window) \
                     gives them; the values here for the rest. Off: the values here everywhere.",
                    );
                ui.label("Df");
                ui.add(
                    egui::DragValue::new(&mut self.tan_d)
                        .range(0.0..=0.1)
                        .speed(0.0005)
                        .max_decimals(4),
                )
                .on_hover_text("Dielectric loss tangent");
                egui::ComboBox::from_id_salt("sim_material")
                    .selected_text("material")
                    .show_ui(ui, |ui| {
                        for (name, df) in [
                            ("FR-4 (0.020)", 0.02),
                            ("Mid-loss FR-4 (0.010)", 0.01),
                            ("Low loss, e.g. Megtron 4 (0.005)", 0.005),
                            ("Very low loss, e.g. Megtron 6 (0.002)", 0.002),
                            ("PTFE / Rogers 4350B (0.0037)", 0.0037),
                        ] {
                            if ui.selectable_label(false, name).clicked() {
                                self.tan_d = df;
                            }
                        }
                    });
                ui.label("Roughness");
                ui.add(
                    egui::DragValue::new(&mut self.rough_um)
                        .range(0.0..=10.0)
                        .speed(0.05)
                        .suffix(" µm"),
                )
                .on_hover_text(
                    "Copper surface roughness, RMS (about 1 µm for standard foil, 0.4 for HVLP)",
                );
            });
            ui.checkbox(&mut self.crosstalk, "Crosstalk").on_hover_text(
                "Drive the nets running beside the route (with the driver and receiver models) \
                 and add their near- and far-end crosstalk",
            );
        });
        ui.horizontal(|ui| {
            if ui.button("Simulate").clicked() {
                self.result = Some(if same_stackup(&board.stackup, stackup) {
                    self.run(board, partner.is_some())
                } else {
                    let mut b = board.clone();
                    b.stackup = stackup.clone();
                    self.run(&b, partner.is_some())
                });
            }
            if let Some(Ok((run, title))) = &self.result {
                if ui.button("Save picture…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("SVG", &["svg"])
                        .set_file_name(format!("{}.svg", name.replace(['/', '\\'], "_")))
                        .save_file()
                    {
                        let _ = std::fs::write(p, sim::svg(run, title));
                    }
                }
            }
        });
        match &self.result {
            None => {
                ui.label(
                    RichText::new(
                        "Lines from the stack-up, vias and stubs, with copper and dielectric loss; \
                         IBIS buffers with their package; a PRBS7 pattern; crosstalk from the nets \
                         beside the route.",
                    )
                    .weak(),
                );
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::from_rgb(255, 120, 100), e);
            }
            Some(Ok((run, _))) => results(ui, run),
        }
    }

    fn run(&mut self, board: &Board, has_partner: bool) -> Result<(Run, String), String> {
        let pad_of = |s: &str| -> Result<usize, String> {
            let (d, p) = s
                .split_once('.')
                .ok_or("pick a driver and a receiver pin")?;
            sim::find_pad(board, d, p).ok_or_else(|| format!("no pad {s}"))
        };
        let (dp, rp) = (pad_of(&self.pins[0])?, pad_of(&self.pins[1])?);
        if dp == rp {
            return Err("the driver and receiver are the same pin".into());
        }
        let file = |k: usize| -> Result<&Ibis, String> {
            let p = self.files[k]
                .as_ref()
                .ok_or("load an IBIS file for the driver and the receiver")?;
            self.loaded
                .get(p)
                .ok_or("load the IBIS file again")?
                .as_ref()
                .map_err(|e| e.clone())
        };
        let pin_name = |s: &str| s.split_once('.').map_or("", |x| x.1).to_string();
        let (dm, dpkg) = sim::pin_model(
            file(0)?,
            &pin_name(&self.pins[0]),
            self.models[0].as_deref(),
            true,
        )?;
        let (rm, rpkg) = sim::pin_model(
            file(1)?,
            &pin_name(&self.pins[1]),
            self.models[1].as_deref(),
            false,
        )?;
        let pair = self.pair && has_partner;
        let pins = Pins {
            driver: dp,
            receiver: rp,
            driver_n: pair.then(|| sim::partner_pad(board, dp)).flatten(),
            receiver_n: pair.then(|| sim::partner_pad(board, rp)).flatten(),
        };
        let setup = Setup {
            corner: self.corner,
            rate: self.rate_g * 1e9,
            bits: self.bits,
            term: match self.term {
                TermKind::None => Term::None,
                TermKind::ToRef => Term::ToRef {
                    r: self.term_r,
                    v: self.term_v,
                },
                TermKind::Across => Term::Across(self.term_r),
            },
            loss: self.lossy.then_some(sim::Loss {
                tan_d: self.tan_d,
                rough_um: self.rough_um,
                stackup_df: self.from_stackup,
                stackup_rough: self.from_stackup,
            }),
            crosstalk: self.crosstalk,
        };
        let run = sim::simulate(
            board,
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
        let title = format!(
            "{} ({}) to {} ({}), {:.3} Gb/s, {}",
            self.pins[0],
            dm.name,
            self.pins[1],
            rm.name,
            self.rate_g,
            self.corner.label()
        );
        Ok((run, title))
    }
}

const COLOURS: [Color32; 4] = [
    Color32::from_rgb(255, 208, 96),
    Color32::from_rgb(94, 200, 255),
    Color32::from_rgb(255, 122, 168),
    Color32::from_rgb(155, 226, 122),
];

fn results(ui: &mut egui::Ui, run: &Run) {
    let e = &run.eye;
    let good = e.height > 0.0 && e.width > 0.0;
    ui.label(
        RichText::new(format!(
            "Eye height {:.3} V, width {:.1} ps ({:.0} % of the {:.1} ps unit interval)",
            e.height,
            e.width * 1e12,
            e.width / e.ui * 100.0,
            e.ui * 1e12
        ))
        .strong()
        .color(if good {
            Color32::from_rgb(140, 220, 140)
        } else {
            Color32::from_rgb(255, 120, 100)
        }),
    );
    ui.label(format!(
        "Route delay {:.1} ps · driver switching {:.0} / {:.0} ps · step {:.2} ps",
        run.delay * 1e12,
        run.switch.0 * 1e12,
        run.switch.1 * 1e12,
        run.dt * 1e12
    ));
    if run.loss_db > 0.0 {
        ui.label(format!(
            "Loss {:.2} dB at {:.2} GHz (half the bit rate); {}",
            run.loss_db,
            0.5 / e.ui / 1e9,
            run.materials
        ));
    }
    if let Some(q) = &run.quiet {
        ui.label(format!(
            "Crosstalk up to {:.1} mV at the receiver; without it the eye is {:.3} V, {:.0} % UI",
            run.noise * 1e3,
            q.height,
            q.width / q.ui * 100.0
        ));
        for a in &run.aggressors {
            ui.label(
                RichText::new(format!(
                    "  {} driven from {}{}: {:.0} mil beside the route, Kb {:.3}",
                    a.net,
                    a.from,
                    if a.inverted { " (inverted)" } else { "" },
                    a.coupled,
                    a.kb
                ))
                .weak(),
            );
        }
    }
    for n in &run.notes {
        ui.label(RichText::new(n).weak());
    }
    // Waveforms: 16 bits from the first.
    let n = ((16.0 * e.ui / run.dt) as usize).max(2);
    let first = (run.start / run.dt) as usize;
    let series: Vec<(&[f32], Color32, &str)> = run
        .probes
        .iter()
        .enumerate()
        .map(|(k, p)| {
            (
                &p.v[first.min(p.v.len())..(first + n).min(p.v.len())],
                COLOURS[k % 4],
                p.name.as_str(),
            )
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        for (_, c, name) in &series {
            ui.colored_label(*c, *name);
        }
    });
    ui.horizontal(|ui| {
        let traces: Vec<(&[f32], Color32)> = series.iter().map(|s| (s.0, s.1)).collect();
        plot(
            ui,
            egui::vec2(470.0, 240.0),
            &traces,
            n,
            16.0 * e.ui * 1e9,
            "ns",
            None,
        );
        let eye: Vec<(&[f32], Color32)> = e
            .traces
            .iter()
            .map(|t| {
                (
                    t.as_slice(),
                    Color32::from_rgba_unmultiplied(94, 200, 255, 70),
                )
            })
            .collect();
        plot(
            ui,
            egui::vec2(260.0, 240.0),
            &eye,
            e.samples,
            2.0 * e.ui * 1e12,
            "ps",
            Some(e.threshold),
        );
    });
}

/// Draws traces of `n` samples each over a span labelled in `unit`.
fn plot(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    traces: &[(&[f32], Color32)],
    n: usize,
    span: f64,
    unit: &str,
    level: Option<f64>,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_rgb(22, 24, 29));
    let (lo, hi) = traces
        .iter()
        .flat_map(|t| t.0.iter())
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    if lo > hi {
        return;
    }
    let pad = 0.08 * (hi - lo).max(0.1);
    let (lo, hi) = (lo - pad, hi + pad);
    let inner = rect
        .shrink2(egui::vec2(34.0, 14.0))
        .translate(egui::vec2(14.0, -4.0));
    let x = |i: usize| inner.left() + inner.width() * i as f32 / (n.max(2) - 1) as f32;
    let y = |v: f32| inner.bottom() - inner.height() * (v - lo) / (hi - lo);
    let grid = Stroke::new(1.0_f32, Color32::from_gray(50));
    let font = egui::FontId::proportional(10.0);
    for i in 0..=4 {
        let v = lo + (hi - lo) * i as f32 / 4.0;
        painter.line_segment(
            [
                egui::pos2(inner.left(), y(v)),
                egui::pos2(inner.right(), y(v)),
            ],
            grid,
        );
        painter.text(
            egui::pos2(inner.left() - 3.0, y(v)),
            egui::Align2::RIGHT_CENTER,
            format!("{v:.2}"),
            font.clone(),
            Color32::GRAY,
        );
        let t = span * i as f64 / 4.0;
        let xx = inner.left() + inner.width() * i as f32 / 4.0;
        painter.text(
            egui::pos2(xx, inner.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            format!("{t:.1} {unit}"),
            font.clone(),
            Color32::GRAY,
        );
    }
    if let Some(l) = level {
        let l = l as f32;
        if l > lo && l < hi {
            painter.line_segment(
                [
                    egui::pos2(inner.left(), y(l)),
                    egui::pos2(inner.right(), y(l)),
                ],
                Stroke::new(1.0_f32, Color32::from_rgb(120, 90, 60)),
            );
        }
    }
    for (t, c) in traces {
        let step = (t.len() / 900).max(1);
        let pts: Vec<egui::Pos2> = t
            .iter()
            .enumerate()
            .step_by(step)
            .map(|(i, &v)| egui::pos2(x(i), y(v)))
            .collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, Stroke::new(1.2_f32, *c)));
        }
    }
}
