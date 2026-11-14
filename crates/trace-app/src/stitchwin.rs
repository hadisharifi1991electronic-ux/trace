//! The Via stitching window: ground vias over a net's pours and planes, or
//! a fence of them beside the selected net's tracks.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use trace_core::edit::Edit;
use trace_core::stitch::{self, FenceGoal, Placed, Rules, StitchGoal, ViaSpec};
use trace_core::{Board, LayerRef};

type Done = Result<(Placed, Vec<Edit>), String>;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Stitch,
    Fence,
}

pub struct StitchWindow {
    pub open: bool,
    mode: Mode,
    /// Nets with copper areas, largest first.
    area_nets: Vec<u32>,
    net: Option<u32>,
    via: ViaSpec,
    rules: Rules,
    grid: f64,
    stagger: bool,
    ghz: f64,
    own_pitch: bool,
    pitch: f64,
    gap: f64,
    rows: usize,
    board_key: usize,
    running: Option<mpsc::Receiver<Done>>,
    last: Option<Result<String, String>>,
}

impl Default for StitchWindow {
    fn default() -> Self {
        StitchWindow {
            open: false,
            mode: Mode::Stitch,
            area_nets: Vec::new(),
            net: None,
            via: ViaSpec {
                drill: 10.0,
                pad: 20.0,
                from: 0,
                to: 1,
            },
            rules: Rules {
                clearance: 6.0,
                edge: 20.0,
                min_layers: 2,
            },
            grid: 100.0,
            stagger: false,
            ghz: 6.0,
            own_pitch: false,
            pitch: 50.0,
            gap: 6.0,
            rows: 1,
            board_key: 0,
            running: None,
            last: None,
        }
    }
}

/// The selected net and its pair partner, if it has one.
fn with_partner(board: &Board, n: u32) -> Vec<u32> {
    let name = &board.nets[n as usize].name;
    let mut out = vec![n];
    for p in &board.diff_pairs {
        let other = if &p.positive == name {
            &p.negative
        } else if &p.negative == name {
            &p.positive
        } else {
            continue;
        };
        if let Some(o) = stitch::net_named(board, other) {
            out.push(o);
        }
    }
    out
}

impl StitchWindow {
    /// Shows the window; returns the edits to apply when vias are placed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) -> Option<(String, Vec<Edit>)> {
        if !self.open {
            return None;
        }
        let key = board as *const Board as usize;
        if key != self.board_key {
            self.board_key = key;
            self.via = ViaSpec::of(board);
            self.rules = Rules::of(board);
            self.gap = self.rules.clearance;
            let mut area: std::collections::HashMap<u32, f64> = Default::default();
            for p in &board.geometry.polygons {
                if let (Some(n), LayerRef::Copper(_)) = (p.net, &p.layer) {
                    let v = &p.outline;
                    let mut a = 0.0;
                    for i in 0..v.len() {
                        let (p0, p1) = (v[i], v[(i + 1) % v.len()]);
                        a += p0.x.mil() * p1.y.mil() - p1.x.mil() * p0.y.mil();
                    }
                    *area.entry(n).or_default() += a.abs() / 2.0;
                }
            }
            let mut nets: Vec<(u32, f64)> = area.into_iter().collect();
            nets.sort_by(|a, b| b.1.total_cmp(&a.1));
            self.area_nets = nets.into_iter().map(|n| n.0).collect();
            self.net = stitch::ground_net(board).or(self.area_nets.first().copied());
            self.last = None;
        }
        let mut out = None;
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(Ok((p, edits))) => {
                    self.running = None;
                    let label = match self.mode {
                        Mode::Stitch => format!("Stitched {} vias", p.vias.len()),
                        Mode::Fence => format!("Fenced with {} vias", p.vias.len()),
                    };
                    self.last = Some(Ok(p.summary()));
                    if !p.vias.is_empty() {
                        out = Some((label, edits));
                    }
                }
                Ok(Err(e)) => {
                    self.running = None;
                    self.last = Some(Err(e));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("Via stitching and fences")
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| self.body(ui, ctx, board, selected));
        self.open = open;
        out
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Stitch, "Stitch a net's copper");
            ui.selectable_value(&mut self.mode, Mode::Fence, "Fence the selected net");
        });
        ui.separator();
        let names: Vec<String> = board
            .layer_geometry()
            .iter()
            .map(|g| g.name.clone())
            .collect();
        let n_layers = names.len().max(2) as u8;
        let mil = |ui: &mut egui::Ui, v: &mut f64, lo: f64, hi: f64| {
            ui.add(
                egui::DragValue::new(v)
                    .speed(0.5)
                    .range(lo..=hi)
                    .max_decimals(1)
                    .suffix(" mil"),
            );
        };
        let fenced: Vec<u32> = selected.map(|s| with_partner(board, s)).unwrap_or_default();
        egui::Grid::new("stitch-grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(if self.mode == Mode::Stitch {
                    "Net"
                } else {
                    "Vias join"
                });
                let name = |n: Option<u32>| {
                    n.and_then(|n| board.nets.get(n as usize))
                        .map_or("pick one".to_string(), |n| n.name.clone())
                };
                egui::ComboBox::from_id_salt("stitch-net")
                    .selected_text(name(self.net))
                    .height(360.0)
                    .show_ui(ui, |ui| {
                        for &n in &self.area_nets {
                            ui.selectable_value(&mut self.net, Some(n), name(Some(n)));
                        }
                    })
                    .response
                    .on_hover_text("Nets with copper pours or planes, largest first");
                ui.end_row();
                if self.mode == Mode::Fence {
                    ui.label("Fenced");
                    if fenced.is_empty() {
                        ui.colored_label(
                            Color32::from_rgb(230, 180, 90),
                            "Click a net on the board first",
                        );
                    } else {
                        ui.label(
                            fenced
                                .iter()
                                .map(|&n| board.nets[n as usize].name.as_str())
                                .collect::<Vec<_>>()
                                .join(" and "),
                        );
                    }
                    ui.end_row();
                }
                ui.label("Via pad / drill");
                ui.horizontal(|ui| {
                    mil(ui, &mut self.via.pad, 4.0, 200.0);
                    mil(ui, &mut self.via.drill, 2.0, self.via.pad);
                });
                ui.end_row();
                ui.label("Layers");
                ui.horizontal(|ui| {
                    for (id, l) in [("st-from", &mut self.via.from), ("st-to", &mut self.via.to)] {
                        egui::ComboBox::from_id_salt(id)
                            .selected_text(names.get(*l as usize).cloned().unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (i, n) in names.iter().enumerate() {
                                    ui.selectable_value(l, i as u8, n);
                                }
                            });
                    }
                });
                ui.end_row();
                ui.label("Clearance");
                mil(ui, &mut self.rules.clearance, 1.0, 100.0);
                ui.end_row();
                ui.label("From board edge");
                mil(ui, &mut self.rules.edge, 0.0, 500.0);
                ui.end_row();
                match self.mode {
                    Mode::Stitch => {
                        ui.label("Grid");
                        ui.horizontal(|ui| {
                            mil(ui, &mut self.grid, 10.0, 2000.0);
                            ui.checkbox(&mut self.stagger, "staggered");
                        });
                        ui.end_row();
                    }
                    Mode::Fence => {
                        ui.label("Via pitch");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.own_pitch, false, "λ/20 at");
                            ui.add_enabled(
                                !self.own_pitch,
                                egui::DragValue::new(&mut self.ghz)
                                    .speed(0.25)
                                    .range(0.1..=100.0)
                                    .suffix(" GHz"),
                            );
                            ui.radio_value(&mut self.own_pitch, true, "");
                            ui.add_enabled_ui(self.own_pitch, |ui| {
                                mil(ui, &mut self.pitch, 5.0, 2000.0)
                            });
                        });
                        ui.end_row();
                        ui.label("Gap to track");
                        mil(ui, &mut self.gap, 1.0, 200.0);
                        ui.end_row();
                        ui.label("Rows each side");
                        ui.add(egui::DragValue::new(&mut self.rows).range(1..=4));
                        ui.end_row();
                    }
                }
            });
        if self.via.from > self.via.to {
            std::mem::swap(&mut self.via.from, &mut self.via.to);
        }
        self.via.to = self.via.to.min(n_layers - 1);
        // λ/20 in the highest Er beside the fenced tracks' layers.
        let geo = board.layer_geometry();
        let er = {
            let used: Vec<u8> = board
                .geometry
                .tracks
                .iter()
                .filter(|t| t.net.is_some_and(|n| fenced.contains(&n)))
                .filter_map(|t| match t.layer {
                    LayerRef::Copper(l) => Some(l),
                    _ => None,
                })
                .collect();
            let e = geo
                .iter()
                .filter(|g| used.is_empty() || used.contains(&g.copper))
                .map(|g| g.er)
                .fold(1.0, f64::max);
            if e > 1.0 {
                e
            } else {
                4.0
            }
        };
        if self.mode == Mode::Fence && !self.own_pitch {
            self.pitch = stitch::fence_pitch(self.ghz, er);
            ui.weak(format!(
                "Vias every {:.1} mil: a twentieth of a wavelength at {} GHz in Er {:.2}.",
                self.pitch, self.ghz, er
            ));
        }
        let can = self.running.is_none()
            && self.net.is_some()
            && (self.mode == Mode::Stitch || !fenced.is_empty());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can, egui::Button::new("Place vias"))
                .on_hover_text(
                    "Each via keeps the clearance on every layer it passes, stays off the board \
                     edge, keepouts and cutouts, and lands on the net's copper (two layers for \
                     stitching). Planes of other nets get a clearance hole; pours are poured again. \
                     Ctrl+Z undoes it.",
                )
                .clicked()
            {
                let b = board.clone();
                let (mode, net) = (self.mode, self.net.unwrap_or(0));
                let (via, mut rules) = (self.via.clone(), self.rules.clone());
                let (grid, stagger, pitch, gap, rows) =
                    (self.grid, self.stagger, self.pitch, self.gap, self.rows);
                let fenced = fenced.clone();
                let (tx, rx) = mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let r = match mode {
                        Mode::Stitch => stitch::stitch(
                            &b,
                            &StitchGoal {
                                net,
                                via,
                                rules,
                                grid,
                                stagger,
                                area: None,
                            },
                        ),
                        Mode::Fence => {
                            rules.min_layers = 1;
                            stitch::fence(
                                &b,
                                &FenceGoal {
                                    nets: fenced,
                                    ground: net,
                                    via,
                                    rules,
                                    pitch,
                                    gap,
                                    rows,
                                    layers: Vec::new(),
                                },
                            )
                        }
                    };
                    let _ = tx.send(r.map(|p| {
                        let e = p.edits(&b);
                        (p, e)
                    }));
                    c.request_repaint();
                });
                self.running = Some(rx);
            }
            if self.running.is_some() {
                ui.spinner();
                ui.label("Placing…");
            }
        });
        match &self.last {
            Some(Ok(s)) => {
                ui.label(RichText::new(s).strong());
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::from_rgb(230, 110, 90), e);
            }
            None => {}
        }
    }
}
