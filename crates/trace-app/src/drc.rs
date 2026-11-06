//! The Design Rule Check window: runs the board's clearance, short-circuit,
//! width and unrouted-net rules in the background, after every edit when
//! Live is on, and lists what fails. A click takes the board view there.

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{Receiver, TryRecvError};
use trace_core::drc::{Kind, Violation};
use trace_core::Board;

/// At most this many violations are listed; a board far off its rules
/// does not need all of them to be fixed one by one.
const LIMIT: usize = 5000;

/// What a background check sends back: the violations and seconds taken.
type Found = (Vec<Violation>, f64);

pub struct Drc {
    pub open: bool,
    /// Check again after every edit.
    pub live: bool,
    /// Draw a marker on the board at each violation.
    pub markers: bool,
    pub list: Vec<Violation>,
    /// The board revision the list is for; `None` before the first check.
    checked: Option<u64>,
    running: Option<(u64, Receiver<Found>)>,
    took: f64,
    /// The row last clicked.
    pub current: Option<usize>,
    /// Which kinds are listed; the last is length constraints.
    show: [bool; 5],
    /// The board has no clearance rules of its own.
    no_rules: bool,
}

const KINDS: [Kind; 4] = [Kind::Short, Kind::Clearance, Kind::Width, Kind::Unrouted];

/// A row the user clicked: a violation, or a net that fails a length
/// constraint.
pub enum Pick {
    Violation(usize),
    Net(u32),
}

const LENGTH: Color32 = Color32::from_rgb(200, 150, 255);

pub fn color(kind: Kind) -> Color32 {
    match kind {
        Kind::Short => Color32::from_rgb(255, 70, 70),
        Kind::Clearance => Color32::from_rgb(255, 150, 60),
        Kind::Width => Color32::from_rgb(240, 210, 80),
        Kind::Unrouted => Color32::from_rgb(120, 180, 255),
    }
}

impl Default for Drc {
    fn default() -> Self {
        Drc {
            open: false,
            live: true,
            markers: true,
            list: Vec::new(),
            checked: None,
            running: None,
            took: 0.0,
            current: None,
            show: [true; 5],
            no_rules: false,
        }
    }
}

impl Drc {
    /// A new board: forget the old results.
    pub fn reset(&mut self) {
        self.list.clear();
        self.checked = None;
        self.running = None;
        self.current = None;
    }

    /// The board changed in a way the revision number does not show (pours
    /// poured again): check it again.
    pub fn invalidate(&mut self) {
        self.checked = None;
    }

    pub fn checking(&self) -> bool {
        self.running.is_some()
    }

    /// The number of violations, once checked.
    pub fn count(&self) -> Option<usize> {
        self.checked.map(|_| self.list.len())
    }

    /// Starts a check of `board` at `revision` when one is due: Live is on
    /// (or the window is open) and the list is older than the board. One
    /// check runs at a time; the next starts when it is done.
    pub fn tick(&mut self, board: &mut Board, revision: u64, ctx: &egui::Context) {
        if let Some((rev, rx)) = &self.running {
            match rx.try_recv() {
                Ok((list, took)) => {
                    let rev = *rev;
                    self.running = None;
                    self.list = list;
                    self.took = took;
                    self.checked = Some(rev);
                    self.current = None;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.running = None,
            }
        }
        if !(self.live || self.open) || self.checked == Some(revision) {
            return;
        }
        self.run(board, revision, ctx);
    }

    fn run(&mut self, board: &mut Board, revision: u64, ctx: &egui::Context) {
        self.no_rules = !board
            .rules
            .iter()
            .any(|r| r.enabled && r.kind.eq_ignore_ascii_case("Clearance"));
        // A copy without the 3D models, which the check does not need.
        let models = std::mem::take(&mut board.geometry.models);
        let copy = board.clone();
        board.geometry.models = models;
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let list = trace_core::drc::check(&copy, LIMIT);
            let _ = tx.send((list, t.elapsed().as_secs_f64()));
            ctx.request_repaint();
        });
        self.running = Some((revision, rx));
    }

    /// The panel docked under the board, as Altium's Messages panel;
    /// returns the index of a violation the user clicked.
    /// `lengths` are the nets failing the Constraint Manager's length
    /// rules, listed with the rest.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        board: &Board,
        lengths: &[(u32, String)],
    ) -> Option<Pick> {
        if !self.open {
            return None;
        }
        let mut pick = None;
        egui::TopBottomPanel::bottom("drc")
            .resizable(true)
            .default_height(190.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Design Rule Check").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("Close")
                            .on_hover_text("Close (D)")
                            .clicked()
                        {
                            self.open = false;
                        }
                    });
                });
                pick = self.body(ui, board, lengths);
            });
        pick
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        board: &Board,
        lengths: &[(u32, String)],
    ) -> Option<Pick> {
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.live, "Live")
                .on_hover_text("Check the board again after every edit");
            ui.checkbox(&mut self.markers, "Markers on the board");
            ui.separator();
            if self.running.is_some() {
                ui.spinner();
                ui.label("Checking…");
            } else if self.checked.is_some() {
                let n = self.list.len();
                let text = if n == 0 {
                    "No violations".to_string()
                } else if n >= LIMIT {
                    format!("{n}+ violations")
                } else if n == 1 {
                    "1 violation".to_string()
                } else {
                    format!("{n} violations")
                };
                ui.label(RichText::new(text).strong());
                ui.label(RichText::new(format!("checked in {:.1} s", self.took)).weak());
            }
            if !self.live && self.running.is_none() && ui.button("Check now").clicked() {
                self.checked = None;
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (i, k) in KINDS.into_iter().enumerate() {
                let n = self.list.iter().filter(|v| v.kind == k).count();
                let text = RichText::new(format!("{} {n}", k.label())).color(if n > 0 {
                    color(k)
                } else {
                    Color32::GRAY
                });
                ui.toggle_value(&mut self.show[i], text);
            }
            let n = lengths.len();
            let text = RichText::new(format!("Length {n}")).color(if n > 0 {
                LENGTH
            } else {
                Color32::GRAY
            });
            ui.toggle_value(&mut self.show[4], text).on_hover_text(
                "Nets failing the Constraint Manager's length, match, skew and via rules",
            );
        });
        if self.no_rules {
            ui.label(
                RichText::new(
                    "This board has no clearance rules of its own (Gerber, ODB++ and Allegro boards have none), so only copper of different nets that overlaps is reported.",
                )
                .weak(),
            );
        }
        ui.separator();
        if self.checked.is_some() && self.list.is_empty() && lengths.is_empty() {
            ui.label("Clearance, short circuit, width, unrouted net and length rules all pass.");
            return None;
        }
        enum Row {
            V(usize),
            L(usize),
        }
        let mut rows: Vec<Row> = (0..self.list.len())
            .filter(|&i| {
                let k = self.list[i].kind;
                KINDS
                    .iter()
                    .position(|&x| x == k)
                    .is_some_and(|j| self.show[j])
            })
            .map(Row::V)
            .collect();
        if self.show[4] {
            rows.extend((0..lengths.len()).map(Row::L));
        }
        let copper: Vec<String> = board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect();
        let row_h = 20.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for row in &rows[range] {
                    let i = match *row {
                        Row::V(i) => i,
                        Row::L(j) => {
                            let (net, text) = &lengths[j];
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [92.0, row_h - 4.0],
                                    egui::Label::new(RichText::new("Length").color(LENGTH)),
                                );
                                ui.add_sized([92.0, row_h - 4.0], egui::Label::new(""));
                                if ui
                                    .selectable_label(false, text)
                                    .on_hover_text("Select the net on the board")
                                    .clicked()
                                {
                                    pick = Some(Pick::Net(*net));
                                }
                            });
                            continue;
                        }
                    };
                    let v = &self.list[i];
                    let layer = v
                        .layer
                        .and_then(|l| copper.get(l as usize).cloned())
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [92.0, row_h - 4.0],
                            egui::Label::new(RichText::new(v.kind.label()).color(color(v.kind))),
                        );
                        ui.add_sized(
                            [92.0, row_h - 4.0],
                            egui::Label::new(RichText::new(layer).weak()).truncate(),
                        );
                        let text = trace_core::drc::describe(board, v);
                        let sel = self.current == Some(i);
                        if ui
                            .selectable_label(sel, text)
                            .on_hover_text("Show it on the board")
                            .clicked()
                        {
                            self.current = Some(i);
                            pick = Some(Pick::Violation(i));
                        }
                    });
                }
            });
        pick
    }
}
