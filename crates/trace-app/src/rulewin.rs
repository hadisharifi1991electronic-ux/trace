//! The PCB Rules and Constraints Editor (Design > Rules): every rule of the
//! board in a tree by category and kind, each with its scopes, priority
//! and constraints. Changes are a draft until Apply, which makes them one
//! undoable step; DRC and the router follow at once.

use eframe::egui::{self, Color32, RichText};
use trace_core::edit::Edit;
use trace_core::rules::{self, Category, KindInfo, Unit};
use trace_core::{Board, Rule};

/// What the window asks the app to do.
pub enum RulesAct {
    Edit(String, Vec<Edit>),
    /// Select what a scope takes on the board, with a line saying what.
    Select(trace_core::select::Sel, String),
}

#[derive(Default)]
pub struct RulesWindow {
    pub open: bool,
    draft: Vec<Rule>,
    /// The board's rules the draft was taken from.
    taken: Option<Vec<Rule>>,
    picked: Option<usize>,
    filter: String,
    /// What a scope took when last asked: (rule, scope, text, counts).
    matches: Option<(usize, usize, String, rules::Matches)>,
    /// The net "which rules apply" was asked of.
    probe: String,
    /// Quick pick for each scope: the kind of term.
    pick_term: [usize; 2],
}

const TERMS: [(&str, &str); 7] = [
    ("Net", "InNet"),
    ("Net class", "InNetClass"),
    ("Diff pair", "InDifferentialPair"),
    ("Diff pair class", "InDifferentialPairClass"),
    ("Layer", "OnLayer"),
    ("Component", "InComponent"),
    ("Room", "WithinRoom"),
];

fn names_for(board: &Board, term: usize) -> Vec<String> {
    match term {
        0 => board.nets.iter().map(|n| n.name.clone()).collect(),
        1 => board.net_classes.iter().map(|c| c.name.clone()).collect(),
        2 => board.diff_pairs.iter().map(|p| p.name.clone()).collect(),
        3 => board
            .diff_pair_classes
            .iter()
            .map(|c| c.name.clone())
            .collect(),
        4 => board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect(),
        5 => board
            .components
            .iter()
            .map(|c| c.designator.clone())
            .collect(),
        _ => board
            .rules
            .iter()
            .filter(|r| r.kind.eq_ignore_ascii_case("RoomDefinition"))
            .map(|r| r.name.clone())
            .collect(),
    }
}

impl RulesWindow {
    /// Opens the editor, filtered to `kind` when one is named.
    pub fn open_at(&mut self, kind: Option<&str>) {
        self.open = true;
        if let Some(k) = kind {
            self.filter = k.to_string();
        }
    }

    /// Picks the highest priority rule of `kind`.
    pub fn pick_first(&mut self, board: &Board, kind: &str) {
        self.draft = board.rules.clone();
        self.taken = Some(board.rules.clone());
        self.picked = rules::of_kind(&self.draft, kind).first().copied();
    }

    fn edited(&self) -> bool {
        self.taken.as_ref().is_some_and(|t| *t != self.draft)
    }

    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<RulesAct> {
        if !self.open {
            return None;
        }
        // Take the board's rules afresh when they changed under us (undo,
        // another file) and nothing is being edited here.
        if self.taken.as_ref() != Some(&board.rules) && !self.edited() {
            self.draft = board.rules.clone();
            self.taken = Some(board.rules.clone());
            self.matches = None;
            if self.picked.is_some_and(|i| i >= self.draft.len()) {
                self.picked = None;
            }
        }
        let mut out = None;
        let mut open = self.open;
        egui::Window::new("PCB Rules and Constraints")
            .open(&mut open)
            .default_size([980.0, 620.0])
            .min_height(420.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Filter");
                    ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(160.0))
                        .on_hover_text("Show only kinds and rules whose name holds this text");
                    ui.separator();
                    let n = self.draft.len();
                    let on = self.draft.iter().filter(|r| r.enabled).count();
                    ui.label(format!("{n} rules, {on} enabled"));
                    if n == 0
                        && ui
                            .button("Add default rules")
                            .on_hover_text(
                                "Clearance, width, via style, mask, plane, fab and placement rules \
                                 for every net, sized from this board's narrowest track and most \
                                 common via. Apply puts them on the board",
                            )
                            .clicked()
                    {
                        self.draft = rules::defaults(board);
                        self.picked = None;
                    }
                });
                egui::TopBottomPanel::bottom("rules_footer")
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(4.0);
                        self.footer(ui, board, &mut out);
                    });
                egui::SidePanel::left("rules_tree")
                    .default_width(290.0)
                    .resizable(true)
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("rule_tree")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.tree(ui));
                    });
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("rule_body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(a) = self.body(ui, board) {
                                out = Some(a);
                            }
                        });
                });
            });
        self.open = open;
        out
    }

    fn shown(&self, k: &str, r: Option<&Rule>) -> bool {
        let f = self.filter.trim().to_ascii_lowercase();
        if f.is_empty() {
            return true;
        }
        let label = rules::info(k).map_or(k, |i| i.label).to_ascii_lowercase();
        k.to_ascii_lowercase().contains(&f)
            || label.contains(&f)
            || r.is_some_and(|r| {
                r.name.to_ascii_lowercase().contains(&f)
                    || r.scope.iter().any(|s| s.to_ascii_lowercase().contains(&f))
            })
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        // Kinds in the draft that Trace does not list.
        let mut extra: Vec<String> = Vec::new();
        for r in &self.draft {
            if rules::info(&r.kind).is_none()
                && !extra.iter().any(|k| k.eq_ignore_ascii_case(&r.kind))
            {
                extra.push(r.kind.clone());
            }
        }
        for cat in Category::ALL {
            let kinds: Vec<(String, Option<&'static KindInfo>)> = rules::KINDS
                .iter()
                .filter(|k| k.category == cat)
                .map(|k| (k.kind.to_string(), Some(k)))
                .chain(
                    extra
                        .iter()
                        .filter(|_| cat == Category::Other)
                        .map(|k| (k.clone(), None)),
                )
                .collect();
            if kinds.is_empty() {
                continue;
            }
            let count: usize = kinds
                .iter()
                .map(|(k, _)| rules::of_kind(&self.draft, k).len())
                .sum();
            let any = kinds.iter().any(|(k, _)| {
                self.shown(k, None)
                    || rules::of_kind(&self.draft, k)
                        .iter()
                        .any(|&i| self.shown(k, Some(&self.draft[i])))
            });
            if !any {
                continue;
            }
            egui::CollapsingHeader::new(
                RichText::new(format!("{} ({count})", cat.label())).strong(),
            )
            .id_salt(("cat", cat.label()))
            .default_open(count > 0 && cat != Category::Testpoint)
            .show(ui, |ui| {
                for (kind, info) in kinds {
                    self.kind_node(ui, &kind, info);
                }
            });
        }
    }

    fn kind_node(&mut self, ui: &mut egui::Ui, kind: &str, info: Option<&'static KindInfo>) {
        let idx = rules::of_kind(&self.draft, kind);
        let label = info.map_or(kind, |i| i.label);
        if !self.shown(kind, None) && !idx.iter().any(|&i| self.shown(kind, Some(&self.draft[i]))) {
            return;
        }
        let head = format!("{label} ({})", idx.len());
        let resp = egui::CollapsingHeader::new(head)
            .id_salt(("kind", kind.to_ascii_lowercase()))
            .default_open(!idx.is_empty() && !self.filter.is_empty())
            .show(ui, |ui| {
                for &i in &idx {
                    let r = &self.draft[i];
                    let text = format!("{}  {}", r.priority, r.name);
                    let mut t = RichText::new(text);
                    if !r.enabled {
                        t = t.weak().strikethrough();
                    }
                    let resp = ui
                        .selectable_label(self.picked == Some(i), t)
                        .on_hover_text(format!("{}\n{}", r.scope.join("  |  "), rules::summary(r)));
                    if resp.clicked() {
                        self.picked = Some(i);
                        self.matches = None;
                    }
                }
                if ui
                    .small_button("+ New rule")
                    .on_hover_text(info.map_or("A rule of this kind", |i| i.tip))
                    .clicked()
                {
                    let r = rules::new_rule(&self.draft, kind);
                    self.draft.push(r);
                    self.picked = Some(self.draft.len() - 1);
                    self.matches = None;
                }
            });
        if let Some(i) = info {
            resp.header_response
                .on_hover_text(format!("{}\nUsed by: {}", i.tip, i.used_by));
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board) -> Option<RulesAct> {
        let Some(i) = self.picked.filter(|&i| i < self.draft.len()) else {
            ui.label("Pick a rule on the left, or + New rule under a kind.");
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Of the enabled rules of a kind whose scope takes an object, the one with the \
                     smallest priority number applies. Scopes are Altium queries: InNet('X'), \
                     InNetClass('X'), InDifferentialPair('X'), OnLayer('X'), WithinRoom('X'), \
                     IsVia, IsPad..., joined with and, or, not.",
                )
                .weak(),
            );
            return None;
        };
        let mut out = None;
        let info = rules::info(&self.draft[i].kind);
        ui.horizontal(|ui| {
            ui.heading(info.map_or(self.draft[i].kind.as_str(), |k| k.label));
            if let Some(k) = info {
                ui.label(RichText::new(format!("used by {}", k.used_by)).weak());
            }
        });
        egui::Grid::new("rule_head")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.draft[i].name);
                ui.end_row();
                ui.label("Comment");
                let mut c = rules::param(&self.draft[i], "COMMENT")
                    .unwrap_or("")
                    .to_string();
                if ui.text_edit_singleline(&mut c).changed() {
                    rules::set_param(&mut self.draft[i], "COMMENT", c);
                }
                ui.end_row();
                ui.label("Enabled");
                ui.checkbox(&mut self.draft[i].enabled, "");
                ui.end_row();
                ui.label("Priority");
                ui.horizontal(|ui| {
                    let n = rules::of_kind(&self.draft, &self.draft[i].kind).len();
                    ui.label(format!("{} of {n}", self.draft[i].priority));
                    if ui
                        .small_button("Increase")
                        .on_hover_text("Smaller number: wins over more rules")
                        .clicked()
                    {
                        rules::shift(&mut self.draft, i, true);
                    }
                    if ui.small_button("Decrease").clicked() {
                        rules::shift(&mut self.draft, i, false);
                    }
                });
                ui.end_row();
            });
        ui.separator();
        let binary = info.is_some_and(|k| k.binary);
        let scopes = if binary { 2 } else { 1 };
        while self.draft[i].scope.len() < scopes {
            self.draft[i].scope.push("All".into());
        }
        for k in 0..scopes {
            let title = match (binary, k) {
                (false, _) => "Where the object matches",
                (true, 0) => "Where the first object matches",
                _ => "Where the second object matches",
            };
            ui.label(RichText::new(title).strong());
            ui.horizontal(|ui| {
                let s = &mut self.draft[i].scope[k];
                ui.add(
                    egui::TextEdit::singleline(s)
                        .desired_width(ui.available_width() - 210.0)
                        .font(egui::TextStyle::Monospace),
                );
                if ui.small_button("All").clicked() {
                    *s = "All".into();
                }
                if ui
                    .small_button("Test")
                    .on_hover_text("Count the nets, tracks, vias and pads this scope takes")
                    .clicked()
                {
                    let m = rules::matches(board, s);
                    self.matches = Some((i, k, s.clone(), m));
                }
                if ui
                    .small_button("Select")
                    .on_hover_text("Select what this scope takes on the board")
                    .clicked()
                {
                    let m = rules::matches(board, s);
                    let msg = format!(
                        "{}: {} tracks, {} vias, parts of {} pads",
                        s, m.tracks, m.vias, m.pads
                    );
                    out = Some(RulesAct::Select(m.sel.clone(), msg));
                    self.matches = Some((i, k, s.clone(), m));
                }
            });
            // Quick picks: a kind of term and a name.
            ui.horizontal(|ui| {
                let t = &mut self.pick_term[k];
                egui::ComboBox::from_id_salt(("term", k))
                    .selected_text(TERMS[*t].0)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (j, (label, _)) in TERMS.iter().enumerate() {
                            ui.selectable_value(t, j, *label);
                        }
                    });
                let term = *t;
                let names = names_for(board, term);
                egui::ComboBox::from_id_salt(("name", k))
                    .selected_text(format!("pick of {}", names.len()))
                    .width(180.0)
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        for n in names.iter().take(4000) {
                            if ui.selectable_label(false, n).clicked() {
                                self.draft[i].scope[k] = rules::scope_of(TERMS[term].1, n);
                            }
                        }
                    });
                ui.label(
                    RichText::new("sets the scope; type to join terms with and / or / not").weak(),
                );
            });
            match rules::check_scope(&self.draft[i].scope[k]) {
                Err(e) => {
                    ui.colored_label(
                        Color32::from_rgb(220, 70, 60),
                        format!("Does not read: {e}"),
                    );
                }
                Ok(unknown) if !unknown.is_empty() => {
                    ui.colored_label(
                        Color32::from_rgb(210, 160, 40),
                        format!(
                            "Kept for Altium; Trace does not judge {} (DRC takes the rule as a maybe there)",
                            unknown.join(", ")
                        ),
                    );
                }
                _ => {}
            }
            if let Some((ri, rk, text, m)) = &self.matches {
                if *ri == i && *rk == k && *text == self.draft[i].scope[k] {
                    ui.label(format!(
                        "Takes {} nets, {} tracks, {} vias, {} pads{}",
                        m.nets,
                        m.tracks,
                        m.vias,
                        m.pads,
                        if m.maybe > 0 {
                            format!(" ({} more it cannot tell)", m.maybe)
                        } else {
                            String::new()
                        }
                    ));
                }
            }
            ui.add_space(4.0);
        }
        ui.separator();
        ui.label(RichText::new("Constraints").strong());
        if let Some(k) = info {
            self.fields(ui, i, k);
            if !k.per_layer.is_empty() {
                self.layer_table(ui, board, i, k);
            }
        }
        egui::CollapsingHeader::new(format!("All parameters ({})", self.draft[i].params.len()))
            .id_salt("raw_params")
            .default_open(info.is_none())
            .show(ui, |ui| {
                let keys: Vec<String> = self.draft[i].params.keys().cloned().collect();
                let mut gone = None;
                egui::Grid::new("raw")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for key in keys {
                            ui.monospace(&key);
                            if let Some(v) = self.draft[i].params.get_mut(&key) {
                                ui.add(egui::TextEdit::singleline(v).desired_width(260.0));
                            }
                            if ui
                                .small_button("x")
                                .on_hover_text("Remove this parameter")
                                .clicked()
                            {
                                gone = Some(key);
                            }
                            ui.end_row();
                        }
                    });
                if let Some(k) = gone {
                    self.draft[i].params.remove(&k);
                }
            });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Duplicate").clicked() {
                self.picked = rules::duplicate(&mut self.draft, i);
            }
            if ui.button("Delete").clicked() {
                let kind = self.draft[i].kind.clone();
                self.draft.remove(i);
                rules::renumber(&mut self.draft, &kind);
                self.picked = None;
                self.matches = None;
            }
        });
        out
    }

    fn fields(&mut self, ui: &mut egui::Ui, i: usize, k: &KindInfo) {
        egui::Grid::new(("fields", k.kind))
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for fd in k.fields {
                    let r = &mut self.draft[i];
                    let cur = rules::param(r, fd.key).map(str::to_string);
                    ui.label(fd.label).on_hover_text(fd.key);
                    match fd.unit {
                        Unit::Len => {
                            let mut v = cur
                                .as_deref()
                                .and_then(trace_core::Length::parse)
                                .map(trace_core::Length::mil)
                                .or_else(|| {
                                    trace_core::Length::parse(fd.default)
                                        .map(trace_core::Length::mil)
                                })
                                .unwrap_or(0.0);
                            let resp = ui.add(
                                egui::DragValue::new(&mut v)
                                    .speed(0.1)
                                    .max_decimals(4)
                                    .suffix(" mil"),
                            );
                            if resp.changed() {
                                rules::set_param(r, fd.key, rules::fmt_mil(v));
                                if fd.key == "GAP" && r.kind.eq_ignore_ascii_case("Clearance") {
                                    rules::set_param(r, "GENERICCLEARANCE", rules::fmt_mil(v));
                                }
                            }
                            resp.on_hover_text(format!("{:.4} mm", v * 0.0254));
                        }
                        Unit::Num => {
                            let mut v: f64 = cur
                                .as_deref()
                                .and_then(|s| s.trim().parse().ok())
                                .or_else(|| fd.default.parse().ok())
                                .unwrap_or(0.0);
                            if ui.add(egui::DragValue::new(&mut v).speed(0.5)).changed() {
                                rules::set_param(r, fd.key, format!("{v}"));
                            }
                        }
                        Unit::Bool => {
                            let mut b = cur
                                .as_deref()
                                .unwrap_or(fd.default)
                                .eq_ignore_ascii_case("TRUE");
                            if ui.checkbox(&mut b, "").changed() {
                                rules::set_param(r, fd.key, if b { "TRUE" } else { "FALSE" });
                            }
                        }
                        // A value the list does not hold (Altium writes some as
                        // numbers): edited as text, so it is kept as written.
                        Unit::Choice(opts)
                            if cur.as_deref().is_some_and(|c| {
                                !opts.iter().any(|o| o.eq_ignore_ascii_case(c))
                            }) =>
                        {
                            let mut t = cur.unwrap_or_default();
                            if ui
                                .add(egui::TextEdit::singleline(&mut t).desired_width(160.0))
                                .changed()
                            {
                                rules::set_param(r, fd.key, t);
                            }
                        }
                        Unit::Choice(opts) => {
                            let now = cur.unwrap_or_else(|| fd.default.to_string());
                            egui::ComboBox::from_id_salt(("choice", fd.key))
                                .selected_text(&now)
                                .show_ui(ui, |ui| {
                                    for o in opts {
                                        if ui
                                            .selectable_label(now.eq_ignore_ascii_case(o), *o)
                                            .clicked()
                                        {
                                            rules::set_param(r, fd.key, *o);
                                        }
                                    }
                                });
                        }
                        Unit::Text => {
                            let mut s = cur.unwrap_or_default();
                            if ui
                                .add(egui::TextEdit::singleline(&mut s).desired_width(300.0))
                                .changed()
                            {
                                rules::set_param(r, fd.key, s);
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    }

    /// Per copper layer values (`TOPLAYER_PREFWIDTH`...): blank uses the
    /// rule's general value.
    fn layer_table(&mut self, ui: &mut egui::Ui, board: &Board, i: usize, k: &KindInfo) {
        ui.add_space(4.0);
        ui.label(RichText::new("Per layer (mil; blank takes the value above)").weak());
        let layers: Vec<String> = board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect();
        egui::Grid::new(("layers", k.kind))
            .num_columns(k.per_layer.len() + 1)
            .striped(true)
            .show(ui, |ui| {
                ui.label("Layer");
                for (_, label) in k.per_layer {
                    ui.label(*label);
                }
                ui.end_row();
                for (l, name) in layers.iter().enumerate() {
                    ui.label(name);
                    let lk = rules::layer_key(board, l);
                    for (suffix, _) in k.per_layer {
                        let key = format!("{lk}_{suffix}");
                        let r = &mut self.draft[i];
                        let mut s = rules::param(r, &key)
                            .and_then(trace_core::Length::parse)
                            .map(|v| {
                                format!("{:.3}", v.mil())
                                    .trim_end_matches('0')
                                    .trim_end_matches('.')
                                    .to_string()
                            })
                            .unwrap_or_default();
                        if ui
                            .add(egui::TextEdit::singleline(&mut s).desired_width(56.0))
                            .changed()
                        {
                            let t = s.trim();
                            if t.is_empty() {
                                let found = r
                                    .params
                                    .keys()
                                    .find(|x| x.eq_ignore_ascii_case(&key))
                                    .cloned();
                                if let Some(f) = found {
                                    r.params.remove(&f);
                                }
                            } else if let Some(v) = trace_core::Length::parse(t) {
                                rules::set_param(r, &key, rules::fmt_mil(v.mil()));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    }

    fn footer(&mut self, ui: &mut egui::Ui, board: &Board, out: &mut Option<RulesAct>) {
        let problems = rules::problems(&self.draft);
        ui.horizontal(|ui| {
            ui.label("Which rules apply to net");
            egui::ComboBox::from_id_salt("probe")
                .selected_text(if self.probe.is_empty() {
                    "pick a net"
                } else {
                    self.probe.as_str()
                })
                .width(180.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for n in board.nets.iter().take(4000) {
                        ui.selectable_value(&mut self.probe, n.name.clone(), &n.name);
                    }
                });
            ui.label(RichText::new("(as the draft stands)").weak());
        });
        if let Some(net) = board.nets.iter().position(|n| n.name == self.probe) {
            // The board without its geometry, under the draft rules.
            let b = Board {
                stackup: board.stackup.clone(),
                nets: board.nets.clone(),
                net_classes: board.net_classes.clone(),
                diff_pair_classes: board.diff_pair_classes.clone(),
                diff_pairs: board.diff_pairs.clone(),
                components: board.components.clone(),
                component_classes: board.component_classes.clone(),
                rules: self.draft.clone(),
                ..Default::default()
            };
            let report = rules::Resolver::new(&b).report(net as u32);
            egui::ScrollArea::vertical()
                .id_salt("probe_list")
                .max_height(70.0)
                .show(ui, |ui| {
                    egui::Grid::new("probe_grid")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for (kind, name, what) in report {
                                ui.label(rules::info(&kind).map_or(kind.as_str(), |k| k.label));
                                ui.label(RichText::new(name).strong());
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                });
        }
        ui.horizontal(|ui| {
            let edited = self.edited();
            if ui
                .add_enabled(edited, egui::Button::new(RichText::new("Apply").strong()))
                .on_hover_text(
                    "Put these rules on the board as one step (Ctrl+Z undoes it); DRC runs again",
                )
                .clicked()
            {
                let before = self.taken.clone().unwrap_or_default();
                *out = Some(RulesAct::Edit(
                    "Changed the design rules".into(),
                    vec![Edit::Rules {
                        before,
                        after: self.draft.clone(),
                    }],
                ));
                self.taken = Some(self.draft.clone());
            }
            if ui
                .add_enabled(edited, egui::Button::new("Revert"))
                .clicked()
            {
                self.draft = self.taken.clone().unwrap_or_default();
                self.picked = None;
                self.matches = None;
            }
            if edited {
                ui.label(RichText::new("Not applied yet").color(Color32::from_rgb(210, 160, 40)));
            }
            if !problems.is_empty() {
                ui.separator();
                let first = &problems[0];
                let name = self.draft.get(first.0).map_or("", |r| r.name.as_str());
                ui.colored_label(
                    Color32::from_rgb(220, 70, 60),
                    format!("{} problem(s): {name} {}", problems.len(), first.1),
                )
                .on_hover_text(
                    problems
                        .iter()
                        .map(|(i, p)| {
                            format!(
                                "{}: {p}",
                                self.draft.get(*i).map_or("", |r| r.name.as_str())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                if ui.small_button("Go").clicked() {
                    self.picked = Some(first.0);
                }
            }
        });
    }
}
