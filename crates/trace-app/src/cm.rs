//! Constraint Manager window: groups of nets with length, match, skew and
//! via limits, checked live against the board, in a spreadsheet like
//! Allegro's.

use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use trace_core::constraints::{
    ConstraintSet, Group, GroupResult, Limits, MatchTarget, Members, Problem,
};
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

    /// The length a net should be tuned to by meanders on `track`, and
    /// why: the match target of a matched group (for a delay match, the
    /// length that brings its delay there at the speed of the track's
    /// layer), a minimum length, or its pair partner when the group limits
    /// skew. The largest of these wins.
    pub fn tune_target(
        &mut self,
        board: &Board,
        net: u32,
        track: &trace_core::Track,
    ) -> Option<(f64, String)> {
        let groups = self.set.groups.clone();
        let results = self.results(board).clone();
        let mut best: Option<(f64, String)> = None;
        let mut offer = |v: f64, why: String| {
            if best.as_ref().is_none_or(|(b, _)| v > *b) {
                best = Some((v, why));
            }
        };
        let length = board.net_length(net).mil();
        let ps_per_mil = match track.layer {
            trace_core::LayerRef::Copper(l) => board
                .layer_geometry()
                .get(l as usize)
                .map(|g| g.delay_ps_per_in(track.width.mil()) / 1000.0),
            _ => None,
        };
        for (g, r) in groups.iter().zip(&results) {
            if !g.enabled {
                continue;
            }
            let Some(me) = r.nets.iter().find(|n| n.net == net) else {
                continue;
            };
            if let Some(t) = r.target_value {
                let what = match &g.limits.match_target {
                    MatchTarget::Longest => "longest",
                    MatchTarget::Member(_) => "target member",
                    MatchTarget::Value(_) => "target",
                };
                if g.limits.by_delay {
                    if let Some(k) = ps_per_mil.filter(|k| *k > 0.0) {
                        offer(
                            length + (t - me.delay_ps) / k,
                            format!("{what} in {}: {t:.1} ps", g.name),
                        );
                    }
                } else {
                    offer(t, format!("{what} in {}", g.name));
                }
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

    /// Every failing net of every group, with what is wrong, for the
    /// design rule check list.
    pub fn failures(&mut self, board: &Board) -> Vec<(u32, String)> {
        let groups = self.set.groups.clone();
        let mut out = Vec::new();
        for (g, r) in groups.iter().zip(self.results(board).iter()) {
            for n in r.nets.iter().filter(|n| !n.problems.is_empty()) {
                let mut what: Vec<String> = Vec::new();
                for p in &n.problems {
                    what.push(match p {
                        Problem::Unmatched => {
                            let o = n.off.unwrap_or(0.0);
                            let unit = if g.limits.by_delay { "ps" } else { "mil" };
                            let tol = if g.limits.by_delay {
                                g.limits.match_tolerance_ps.unwrap_or(0.0)
                            } else {
                                g.limits.match_tolerance.map_or(0.0, |t| t.mil())
                            };
                            format!(
                                "{:.1} {unit} {} the match target (± {tol:.1})",
                                o.abs(),
                                if o < 0.0 { "short of" } else { "over" }
                            )
                        }
                        Problem::Skew => {
                            format!("pair skew {:.1} mil", n.skew.map_or(0.0, |s| s.mil()))
                        }
                        other => format!(
                            "{} ({:.1} mil, {} vias)",
                            other.label(),
                            n.length.mil(),
                            n.vias
                        ),
                    });
                }
                out.push((
                    n.net,
                    format!("{}: {} ({})", n.name, what.join(", "), g.name),
                ));
            }
        }
        out
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
                        format!("Match ± ({unit} or ps)"),
                        format!("Max skew ({unit})"),
                        "Max vias".into(),
                        format!("Length ({unit})"),
                        "Delay (ps)".into(),
                        "Off target".into(),
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
                        if l.by_delay {
                            edited |= ps(ui, (gi, 2), &mut l.match_tolerance_ps);
                        } else {
                            edited |= limit(ui, (gi, 2), &mut l.match_tolerance, mm);
                        }
                        edited |= limit(ui, (gi, 3), &mut l.max_skew, mm);
                        edited |= count(ui, (gi, 4), &mut l.max_vias);
                        edited |= target_cell(ui, gi, l, r, mm);
                        if ui
                            .checkbox(&mut l.by_delay, "by delay")
                            .on_hover_text(
                                "Match propagation delay (ps) instead of length, as Allegro's relative propagation delay: outer layers are faster than inner ones",
                            )
                            .changed()
                        {
                            // A set value means mil or ps; start again.
                            if matches!(l.match_target, MatchTarget::Value(_)) {
                                l.match_target = MatchTarget::Longest;
                            }
                            edited = true;
                        }
                        if r.nets.iter().any(|n| n.pair.is_some()) {
                            edited |= ui
                                .checkbox(&mut l.pairs_as_one, "pairs as one")
                                .on_hover_text(
                                    "Match each differential pair by the mean of its two nets, as one member",
                                )
                                .changed();
                        } else {
                            ui.label("");
                        }
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
                            ui.label(format!("{:.1}", n.delay_ps));
                            let by_delay = self.set.groups[gi].limits.by_delay;
                            ui.label(n.off.map_or(String::new(), |o| {
                                if by_delay {
                                    format!("{o:+.1} ps")
                                } else if self.mm {
                                    format!("{:+.3}", o * 0.0254)
                                } else {
                                    format!("{o:+.1}")
                                }
                            }));
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

/// The match target of a group: the longest member, one member, or a
/// set value. Returns whether it changed.
fn target_cell(ui: &mut egui::Ui, gi: usize, l: &mut Limits, r: &GroupResult, mm: bool) -> bool {
    let mut changed = false;
    let value = |v: f64| {
        if l.by_delay {
            format!("{v:.1} ps")
        } else if mm {
            format!("{:.3} mm", v * 0.0254)
        } else {
            format!("{v:.1}")
        }
    };
    let shown = match &l.match_target {
        MatchTarget::Longest => match r.target_value {
            Some(v) => format!("longest {}", value(v)),
            None => "longest".into(),
        },
        MatchTarget::Member(m) => m.clone(),
        MatchTarget::Value(_) => "set value".into(),
    };
    // Members to pick: pairs by name when pairs count as one, else nets.
    let mut names: Vec<String> = if l.pairs_as_one {
        let mut p: Vec<String> = r.nets.iter().filter_map(|n| n.pair.clone()).collect();
        p.dedup();
        p.extend(
            r.nets
                .iter()
                .filter(|n| n.pair.is_none())
                .map(|n| n.name.clone()),
        );
        p
    } else {
        r.nets.iter().map(|n| n.name.clone()).collect()
    };
    names.sort();
    names.dedup();
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(("cm_target", gi))
            .selected_text(shown)
            .width(130.0)
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut l.match_target, MatchTarget::Longest, "Longest member")
                    .changed();
                let set = MatchTarget::Value(r.target_value.unwrap_or(1000.0));
                if ui
                    .selectable_label(matches!(l.match_target, MatchTarget::Value(_)), "Set value")
                    .clicked()
                {
                    l.match_target = set;
                    changed = true;
                }
                ui.separator();
                for n in names.iter().take(300) {
                    changed |= ui
                        .selectable_value(&mut l.match_target, MatchTarget::Member(n.clone()), n)
                        .changed();
                }
            })
            .response
            .on_hover_text("What the members are matched to");
        if let MatchTarget::Value(v) = &mut l.match_target {
            let delay = l.by_delay;
            let k = if delay || !mm { 1.0 } else { 0.0254 };
            let shown = trim(format!("{:.3}", *v * k));
            if let Some(n) = edit(ui, (gi, 9), shown, |s| s.parse::<f64>().ok()) {
                *v = n / k;
                changed = true;
            }
        }
    });
    changed
}

/// An editable delay in ps; empty means none.
fn ps(ui: &mut egui::Ui, id: (usize, u8), v: &mut Option<f64>) -> bool {
    let shown = v.map_or(String::new(), |p| trim(format!("{p:.2}")));
    edit(ui, id, shown, |s| {
        if s.is_empty() {
            return Some(None);
        }
        s.trim_end_matches("ps")
            .trim()
            .parse::<f64>()
            .ok()
            .map(Some)
    })
    .map(|n| *v = n)
    .is_some()
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

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    #[test]
    fn tunes_a_delay_match_by_the_speed_of_the_tracks_layer() {
        let mut b = Board::default();
        let layer = |name: &str, kind: LayerKind, t: f64, er: Option<f64>| StackLayer {
            name: name.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(t),
            dielectric_constant: er,
            material: None,
        };
        b.stackup.layers = vec![
            layer("Top", LayerKind::Copper, 1.4, None),
            layer("D1", LayerKind::Dielectric, 4.0, Some(4.0)),
            layer("In1", LayerKind::Copper, 0.7, None),
            layer("D2", LayerKind::Dielectric, 4.0, Some(4.0)),
            layer("Bottom", LayerKind::Copper, 1.4, None),
        ];
        for (name, len) in [("A", 1000.0), ("B", 900.0)] {
            let n = b.nets.len() as u32;
            b.nets.push(Net {
                name: name.into(),
                routed_length: Length::from_mil(len),
                pieces: 1,
                ..Default::default()
            });
            b.geometry.tracks.push(Track {
                layer: LayerRef::Copper(1),
                net: Some(n),
                start: Point::default(),
                end: Point {
                    x: Length::from_mil(len),
                    y: Length(0),
                },
                width: Length::from_mil(5.0),
                component: None,
                text: false,
            });
        }
        let mut cm = Cm::new(None);
        cm.set.groups.push(Group {
            name: "g".into(),
            members: Members::Nets(vec!["A".into(), "B".into()]),
            limits: Limits {
                by_delay: true,
                match_tolerance_ps: Some(1.0),
                ..Default::default()
            },
            enabled: true,
            source: String::new(),
        });
        let t = b.geometry.tracks[1].clone();
        let (target, why) = cm.tune_target(&b, 1, &t).expect("a target");
        // Same layer, so the same speed: B goes to A's length.
        assert!((target - 1000.0).abs() < 0.01, "{target} {why}");
        assert!(why.contains("ps"), "{why}");
    }
}
