//! Constraint Manager window: groups of nets with length, match, skew and
//! via limits, checked live against the board, in a spreadsheet like
//! Allegro's.

use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use trace_core::constraints::{ConstraintSet, Group, GroupResult, Limits, Members};
use trace_core::{Board, Length};

const FAIL: Color32 = Color32::from_rgb(240, 96, 96);
const PASS: Color32 = Color32::from_rgb(110, 200, 120);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    NetClass,
    PairClass,
    Pattern,
    PairPattern,
    AllPairs,
    AllNets,
}

impl Kind {
    const ALL: [Kind; 6] = [
        Kind::PairClass,
        Kind::NetClass,
        Kind::PairPattern,
        Kind::Pattern,
        Kind::AllPairs,
        Kind::AllNets,
    ];
    fn label(self) -> &'static str {
        match self {
            Kind::NetClass => "Net class",
            Kind::PairClass => "Pair class",
            Kind::Pattern => "Nets named",
            Kind::PairPattern => "Pairs named",
            Kind::AllPairs => "All pairs",
            Kind::AllNets => "All nets",
        }
    }
}

pub struct Cm {
    pub open: bool,
    pub set: ConstraintSet,
    /// Per board path, kept between sessions.
    pub saved: HashMap<String, ConstraintSet>,
    board_key: String,
    results: Option<Vec<GroupResult>>,
    mm: bool,
    only_failing: bool,
    expanded: Vec<bool>,
    new_kind: Kind,
    new_value: String,
    new_name: String,
}

pub const STORE_KEY: &str = "constraints";

impl Cm {
    pub fn new(stored: Option<String>) -> Self {
        Cm {
            open: false,
            set: ConstraintSet::default(),
            saved: stored
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            board_key: String::new(),
            results: None,
            mm: false,
            only_failing: false,
            expanded: Vec::new(),
            new_kind: Kind::PairClass,
            new_value: String::new(),
            new_name: String::new(),
        }
    }

    /// The board's copper changed; results are worked out again.
    pub fn board_changed(&mut self) {
        self.results = None;
    }

    pub fn to_store(&self) -> String {
        serde_json::to_string(&self.saved).unwrap_or_default()
    }

    /// Loads the constraints for a newly opened board: the ones kept from
    /// last time, or else those read from the board's own rules.
    pub fn load(&mut self, key: String, board: &Board) {
        self.set = self
            .saved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| board.imported_constraints());
        self.board_key = key;
        self.results = None;
        self.expanded = vec![false; self.set.groups.len()];
    }

    fn changed(&mut self) {
        self.results = None;
        self.expanded.resize(self.set.groups.len(), false);
        self.saved.insert(self.board_key.clone(), self.set.clone());
    }

    /// The length a net should be tuned to, and why: the longest member
    /// of a matched group, a minimum length, or its pair partner when the
    /// group limits skew. The largest of these wins.
    pub fn tune_target(&mut self, board: &Board, net: u32) -> Option<(f64, String)> {
        let groups = self.set.groups.clone();
        let results = self.results(board).clone();
        let mut best: Option<(f64, String)> = None;
        let mut offer = |v: f64, why: String| {
            if best.as_ref().is_none_or(|(b, _)| v > *b) {
                best = Some((v, why));
            }
        };
        for (g, r) in groups.iter().zip(&results) {
            if !g.enabled {
                continue;
            }
            let Some(me) = r.nets.iter().find(|n| n.net == net) else {
                continue;
            };
            if let (Some(_), Some(t)) = (g.limits.match_tolerance, r.target) {
                offer(t.mil(), format!("longest in {}", g.name));
            }
            if let Some(m) = g.limits.min_length {
                offer(m.mil(), format!("minimum of {}", g.name));
            }
            if g.limits.max_skew.is_some() {
                if let Some(p) = r
                    .nets
                    .iter()
                    .find(|n| n.net != net && n.pair.is_some() && n.pair == me.pair)
                {
                    offer(p.length.mil(), format!("partner {}", p.name));
                }
            }
        }
        best
    }

    /// Failing nets over all groups, for the status bar.
    pub fn failing(&mut self, board: &Board) -> usize {
        self.results(board).iter().map(|r| r.failing).sum()
    }

    fn results(&mut self, board: &Board) -> &Vec<GroupResult> {
        if self.results.is_none() {
            self.results = Some(self.set.groups.iter().map(|g| board.check(g)).collect());
        }
        self.results.as_ref().unwrap()
    }

    fn fmt(&self, l: Length) -> String {
        if self.mm {
            format!("{:.3}", l.mm())
        } else {
            format!("{:.1}", l.mil())
        }
    }

    /// Shows the window. Returns a net to select on the board, if one was
    /// clicked.
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<u32> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut pick = None;
        egui::Window::new("Constraint Manager")
            .open(&mut open)
            .default_size([1180.0, 540.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| pick = self.body(ui, board));
        self.open = open;
        pick
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board) -> Option<u32> {
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.label("Units");
            ui.selectable_value(&mut self.mm, false, "mil");
            ui.selectable_value(&mut self.mm, true, "mm");
            ui.separator();
            ui.checkbox(&mut self.only_failing, "Only failing nets");
            ui.separator();
            if ui
                .button("Reset to board rules")
                .on_hover_text(
                    "Replace these groups with the ones read from the board's design rules",
                )
                .clicked()
            {
                self.set = board.imported_constraints();
                self.changed();
            }
            let total: usize = self.results(board).iter().map(|r| r.failing).sum();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if total == 0 {
                    ui.label(RichText::new("All constraints met").color(PASS));
                } else {
                    ui.label(
                        RichText::new(format!("{total} nets failing"))
                            .color(FAIL)
                            .strong(),
                    );
                }
            });
        });
        ui.separator();
        self.add_form(ui, board);
        ui.separator();
        if self.set.groups.is_empty() {
            ui.label(
                RichText::new(
                    "No constraint groups yet. The board has no length or matched-length rules; add a group above.",
                )
                .weak(),
            );
            return None;
        }
        let unit = if self.mm { "mm" } else { "mil" };
        let mut delete = None;
        let mut edited = false;
        let results = self.results(board).clone();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("cm_grid")
                .striped(true)
                .min_col_width(54.0)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Group / net".to_string(),
                        "Members".into(),
                        format!("Min ({unit})"),
                        format!("Max ({unit})"),
                        format!("Match ± ({unit})"),
                        format!("Max skew ({unit})"),
                        "Max vias".into(),
                        format!("Length ({unit})"),
                        format!("Delta ({unit})"),
                        format!("Skew ({unit})"),
                        "Vias".into(),
                        "Status".into(),
                        String::new(),
                    ] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (gi, r) in results.iter().enumerate() {
                        let open = self.expanded.get(gi).copied().unwrap_or(false);
                        let g = &mut self.set.groups[gi];
                        ui.horizontal(|ui| {
                            if ui.small_button(if open { "−" } else { "+" }).clicked() {
                                self.expanded[gi] = !open;
                            }
                            if ui.checkbox(&mut g.enabled, "").changed() {
                                edited = true;
                            }
                            ui.label(RichText::new(&g.name).strong()).on_hover_text(
                                if g.source.is_empty() {
                                    "Added in Trace"
                                } else {
                                    &g.source
                                },
                            );
                        });
                        ui.label(g.members.describe());
                        let mm = self.mm;
                        let l = &mut g.limits;
                        edited |= limit(ui, (gi, 0), &mut l.min_length, mm);
                        edited |= limit(ui, (gi, 1), &mut l.max_length, mm);
                        edited |= limit(ui, (gi, 2), &mut l.match_tolerance, mm);
                        edited |= limit(ui, (gi, 3), &mut l.max_skew, mm);
                        edited |= count(ui, (gi, 4), &mut l.max_vias);
                        let target = r.target.map(|t| {
                            if mm {
                                format!("{:.3}", t.mm())
                            } else {
                                format!("{:.1}", t.mil())
                            }
                        });
                        ui.label(
                            RichText::new(target.map_or(String::new(), |t| format!("longest {t}")))
                                .weak(),
                        );
                        ui.label("");
                        ui.label("");
                        ui.label(format!("{} nets", r.nets.len()));
                        if r.nets.is_empty() {
                            ui.label(RichText::new("no nets found").weak());
                        } else if !g.enabled {
                            ui.label(RichText::new("off").weak());
                        } else if r.failing == 0 {
                            ui.label(RichText::new("pass").color(PASS));
                        } else {
                            ui.label(RichText::new(format!("{} failing", r.failing)).color(FAIL));
                        }
                        if ui
                            .small_button("✖")
                            .on_hover_text("Delete this group")
                            .clicked()
                        {
                            delete = Some(gi);
                        }
                        ui.end_row();
                        if !open {
                            continue;
                        }
                        let mut rows: Vec<_> = r
                            .nets
                            .iter()
                            .filter(|n| !self.only_failing || !n.problems.is_empty())
                            .collect();
                        // Failing first, then by pair and name.
                        rows.sort_by(|a, b| {
                            b.problems
                                .is_empty()
                                .cmp(&a.problems.is_empty())
                                .reverse()
                                .then(a.pair.cmp(&b.pair))
                                .then(a.name.cmp(&b.name))
                        });
                        for n in rows {
                            let name = RichText::new(format!("    {}", n.name)).monospace();
                            if ui
                                .add(egui::Label::new(name).sense(egui::Sense::click()))
                                .on_hover_text("Show on the board")
                                .clicked()
                            {
                                pick = Some(n.net);
                            }
                            ui.label(RichText::new(n.pair.clone().unwrap_or_default()).weak());
                            for _ in 0..5 {
                                ui.label("");
                            }
                            ui.label(self.fmt(n.length));
                            ui.label(n.match_delta.map_or(String::new(), |d| self.fmt(d)));
                            ui.label(n.skew.map_or(String::new(), |s| self.fmt(s)));
                            ui.label(n.vias.to_string());
                            if !n.problems.is_empty() {
                                let what: Vec<&str> =
                                    n.problems.iter().map(|p| p.label()).collect();
                                ui.label(RichText::new(what.join(", ")).color(FAIL));
                            } else if n.open {
                                ui.label(RichText::new("not fully routed").weak());
                            } else {
                                ui.label(RichText::new("ok").color(PASS));
                            }
                            ui.label("");
                            ui.end_row();
                        }
                    }
                });
        });
        if let Some(i) = delete {
            self.set.groups.remove(i);
            self.expanded.remove(i);
            edited = true;
        }
        if edited {
            self.changed();
        }
        pick
    }

    fn add_form(&mut self, ui: &mut egui::Ui, board: &Board) {
        ui.horizontal(|ui| {
            ui.label("New group:");
            egui::ComboBox::from_id_salt("cm_kind")
                .selected_text(self.new_kind.label())
                .show_ui(ui, |ui| {
                    for k in Kind::ALL {
                        ui.selectable_value(&mut self.new_kind, k, k.label());
                    }
                });
            match self.new_kind {
                Kind::NetClass | Kind::PairClass => {
                    let names: Vec<String> = if self.new_kind == Kind::NetClass {
                        board.net_classes.iter().map(|c| c.name.clone()).collect()
                    } else {
                        board
                            .diff_pair_classes
                            .iter()
                            .map(|c| c.name.clone())
                            .collect()
                    };
                    if names.is_empty() {
                        ui.label(RichText::new("the board has none").weak());
                    } else {
                        egui::ComboBox::from_id_salt("cm_class")
                            .selected_text(if self.new_value.is_empty() {
                                "pick one"
                            } else {
                                &self.new_value
                            })
                            .show_ui(ui, |ui| {
                                for n in names {
                                    ui.selectable_value(&mut self.new_value, n.clone(), n);
                                }
                            });
                    }
                }
                Kind::Pattern | Kind::PairPattern => {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_value)
                            .hint_text("e.g. RX*_P or DDR_DQ*")
                            .desired_width(160.0),
                    );
                }
                _ => {}
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("name (optional)")
                    .desired_width(140.0),
            );
            let members = match self.new_kind {
                Kind::NetClass if !self.new_value.is_empty() => {
                    Some(Members::NetClass(self.new_value.clone()))
                }
                Kind::PairClass if !self.new_value.is_empty() => {
                    Some(Members::PairClass(self.new_value.clone()))
                }
                Kind::Pattern if !self.new_value.trim().is_empty() => {
                    Some(Members::Pattern(self.new_value.trim().to_string()))
                }
                Kind::PairPattern if !self.new_value.trim().is_empty() => {
                    Some(Members::PairPattern(self.new_value.trim().to_string()))
                }
                Kind::AllPairs => Some(Members::AllPairs),
                Kind::AllNets => Some(Members::AllNets),
                _ => None,
            };
            let count = members.as_ref().map_or(0, |m| board.members(m).len());
            if members.is_some() {
                ui.label(RichText::new(format!("{count} nets")).weak());
            }
            if ui
                .add_enabled(members.is_some() && count > 0, egui::Button::new("Add"))
                .clicked()
            {
                let members = members.unwrap();
                let name = if self.new_name.trim().is_empty() {
                    members.describe()
                } else {
                    self.new_name.trim().to_string()
                };
                let limits = if members.is_pairs() {
                    Limits {
                        max_skew: Some(Length::from_mil(5.0)),
                        ..Default::default()
                    }
                } else {
                    Limits::default()
                };
                self.set.groups.push(Group {
                    name,
                    members,
                    limits,
                    enabled: true,
                    source: String::new(),
                });
                self.changed();
                if let Some(e) = self.expanded.last_mut() {
                    *e = true;
                }
                self.new_value.clear();
                self.new_name.clear();
            }
        });
    }
}

/// An editable length limit; empty means no limit. Returns whether it changed.
fn limit(ui: &mut egui::Ui, id: (usize, u8), v: &mut Option<Length>, mm: bool) -> bool {
    let shown = v.map_or(String::new(), |l| {
        if mm {
            trim(format!("{:.4}", l.mm()))
        } else {
            trim(format!("{:.2}", l.mil()))
        }
    });
    edit(ui, id, shown, |s| {
        if s.is_empty() {
            return Some(None);
        }
        let s = if s.chars().any(|c| c.is_ascii_alphabetic()) {
            s.to_string()
        } else {
            format!("{s}{}", if mm { "mm" } else { "mil" })
        };
        Length::parse(&s).map(Some)
    })
    .map(|n| *v = n)
    .is_some()
}

fn count(ui: &mut egui::Ui, id: (usize, u8), v: &mut Option<usize>) -> bool {
    let shown = v.map_or(String::new(), |n| n.to_string());
    edit(ui, id, shown, |s| {
        if s.is_empty() {
            Some(None)
        } else {
            s.parse().ok().map(Some)
        }
    })
    .map(|n| *v = n)
    .is_some()
}

fn trim(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A small text cell that commits on Enter or when it loses focus.
fn edit<T>(
    ui: &mut egui::Ui,
    id: (usize, u8),
    shown: String,
    parse: impl Fn(&str) -> Option<T>,
) -> Option<T> {
    let key = egui::Id::new(("cm_edit", id));
    let mut text: String = ui
        .data_mut(|d| d.get_temp::<String>(key))
        .unwrap_or_else(|| shown.clone());
    let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(64.0));
    let mut out = None;
    if resp.has_focus() || resp.changed() {
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    }
    if resp.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(key));
        if text.trim() != shown {
            out = parse(text.trim());
        }
    }
    out
}
