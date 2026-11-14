//! The Autoroute window: routes the open connections of every net (or the
//! selected one) on the layers chosen, in the background.

use eframe::egui::{self, RichText};
use std::sync::mpsc::{channel, Receiver};
use trace_core::autoroute::{autoroute, routing_layers, Goal, Report};
use trace_core::edit::Edit;
use trace_core::Board;

type Done = (Vec<Edit>, Report, String);

#[derive(Default)]
pub struct AutoWindow {
    pub open: bool,
    /// Only the selected net.
    one: bool,
    /// Layers ticked, by copper layer; filled for each board.
    layers: Vec<bool>,
    via_cost: Option<f64>,
    job: Option<Receiver<Done>>,
    /// Tracks and vias on the board when the job started.
    started: (usize, usize),
    last: Option<(Report, String)>,
}

/// What the window asks the app to do.
pub enum AutoAct {
    Edit(String, Vec<Edit>),
    ShowNet(u32),
}

impl AutoWindow {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) -> Option<AutoAct> {
        if !self.open {
            return None;
        }
        let names: Vec<String> = board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect();
        if self.layers.len() != names.len() {
            let on = routing_layers(board);
            self.layers = (0..names.len()).map(|l| on.contains(&(l as u8))).collect();
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("Autoroute")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let busy = self.job.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    ui.radio_value(&mut self.one, false, "Every open connection")
                        .on_hover_text("Nets with a copper pour are left to the pour");
                    let sel = selected
                        .and_then(|n| board.nets.get(n as usize))
                        .map_or("no net selected".to_string(), |n| n.name.clone());
                    ui.add_enabled(
                        selected.is_some(),
                        egui::RadioButton::new(self.one, format!("The selected net ({sel})")),
                    )
                    .clicked()
                    .then(|| self.one = true);
                    ui.label("Layers (planes are left out):");
                    ui.horizontal_wrapped(|ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.checkbox(&mut self.layers[i], n);
                        }
                    });
                    let mut cost = self.via_cost.unwrap_or(Goal::default().via_cost);
                    ui.horizontal(|ui| {
                        ui.label("A via costs as much as");
                        if ui
                            .add(
                                egui::DragValue::new(&mut cost)
                                    .speed(1.0)
                                    .range(0.0..=1000.0)
                                    .suffix(" mil of track"),
                            )
                            .changed()
                        {
                            self.via_cost = Some(cost);
                        }
                    });
                    ui.label(
                        RichText::new(
                            "Track width and clearance follow each net's rules; vias are the \
                             board's most used.",
                        )
                        .small()
                        .weak(),
                    );
                    let layers: Vec<u8> = (0..self.layers.len())
                        .filter(|&i| self.layers[i])
                        .map(|i| i as u8)
                        .collect();
                    let ready = !layers.is_empty() && (!self.one || selected.is_some());
                    if ui.add_enabled(ready, egui::Button::new("Route")).clicked() {
                        let goal = Goal {
                            nets: if self.one { selected.map(|n| vec![n]) } else { None },
                            layers: Some(layers),
                            via_cost: cost,
                            ..Goal::default()
                        };
                        let b = board.clone();
                        let (tx, rx) = channel();
                        let ctx2 = ctx.clone();
                        std::thread::spawn(move || {
                            let t = std::time::Instant::now();
                            let (edits, r) = autoroute(&b, &goal);
                            let took = format!("{:.1} s", t.elapsed().as_secs_f64());
                            let _ = tx.send((edits, r, took));
                            ctx2.request_repaint();
                        });
                        self.job = Some(rx);
                        self.started = (board.geometry.tracks.len(), board.geometry.vias.len());
                        self.last = None;
                    }
                });
                if let Some(rx) = &self.job {
                    match rx.try_recv() {
                        Ok((edits, r, mut took)) => {
                            let now = (board.geometry.tracks.len(), board.geometry.vias.len());
                            if now != self.started {
                                took.push_str("; the board changed meanwhile, so nothing was added: route again");
                            } else if !edits.is_empty() {
                                out = Some(AutoAct::Edit(
                                    format!("Autorouted {} connections", r.routed),
                                    edits,
                                ));
                            }
                            self.last = Some((r, took));
                            self.job = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Routing…");
                            });
                        }
                        Err(_) => self.job = None,
                    }
                }
                if let Some((r, took)) = &self.last {
                    ui.separator();
                    ui.label(RichText::new(format!("{} ({took})", r.summary())).strong());
                    if !r.open.is_empty() {
                        ui.label("Left open (click to show the net):");
                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                for o in &r.open {
                                    let pad = |i: usize| {
                                        board.geometry.pads.get(i).map_or("?".to_string(), |p| {
                                            let part = p
                                                .component
                                                .and_then(|c| board.components.get(c as usize))
                                                .map_or("", |c| c.designator.as_str());
                                            format!("{part}.{}", p.name)
                                        })
                                    };
                                    let net = board
                                        .nets
                                        .get(o.net as usize)
                                        .map_or("?", |n| n.name.as_str());
                                    if ui
                                        .link(format!("{net}: {} to {}", pad(o.from), pad(o.to)))
                                        .clicked()
                                    {
                                        out = Some(AutoAct::ShowNet(o.net));
                                    }
                                }
                            });
                    }
                }
            });
        self.open = open;
        out
    }
}
