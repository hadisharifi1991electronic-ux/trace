//! Impedance window: the board's stack-up (editable), a width and gap
//! calculator for each copper layer, and the impedance of every routed pair
//! checked against a target.

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use trace_core::impedance::{layer_geometry, LayerGeometry, NetImpedance, DEFAULT_ER};
use trace_core::{Board, LayerKind, Length, Stackup};

const FAIL: Color32 = Color32::from_rgb(240, 96, 96);
const PASS: Color32 = Color32::from_rgb(110, 200, 120);

pub const STORE_KEY: &str = "impedance";

/// What is kept per board between sessions. The design file is never changed.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    stackup: Stackup,
    se: f64,
    diff: f64,
    tol: f64,
    #[serde(default)]
    rules: Vec<Rule>,
}

/// A target for the nets whose names match a pattern, ahead of the defaults.
#[derive(Clone, Serialize, Deserialize)]
struct Rule {
    pattern: String,
    pair: bool,
    ohms: f64,
}

/// Case-insensitive match with `*` wildcards; no `*` means "contains".
fn matches(name: &str, pattern: &str) -> bool {
    let (n, p) = (
        name.to_ascii_uppercase(),
        pattern.trim().to_ascii_uppercase(),
    );
    if p.is_empty() {
        return false;
    }
    if !p.contains('*') {
        return n.contains(&p);
    }
    let parts: Vec<&str> = p.split('*').collect();
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(k) => rest = &rest[k + part.len()..],
                None => return false,
            }
        }
    }
    true
}

pub struct Imp {
    pub open: bool,
    saved: HashMap<String, Saved>,
    key: String,
    board_stackup: Stackup,
    stackup: Stackup,
    se: f64,
    diff: f64,
    /// Tolerance, percent.
    tol: f64,
    rules: Vec<Rule>,
    geo: Vec<LayerGeometry>,
    rows: Option<Vec<NetImpedance>>,
    mm: bool,
    filter: String,
    pairs_only: bool,
    off_only: bool,
    calc_layer: usize,
    calc_w: f64,
    calc_s: f64,
}

impl Imp {
    pub fn new(stored: Option<String>) -> Self {
        Imp {
            open: false,
            saved: stored
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            key: String::new(),
            board_stackup: Stackup::default(),
            stackup: Stackup::default(),
            se: 50.0,
            diff: 100.0,
            tol: 10.0,
            rules: Vec::new(),
            geo: Vec::new(),
            rows: None,
            mm: false,
            filter: String::new(),
            pairs_only: true,
            off_only: false,
            calc_layer: 0,
            calc_w: 5.0,
            calc_s: 5.0,
        }
    }

    /// The board's copper changed; results are worked out again.
    pub fn board_changed(&mut self) {
        self.rows = None;
    }

    pub fn to_store(&self) -> String {
        serde_json::to_string(&self.saved).unwrap_or_default()
    }

    pub fn load(&mut self, key: String, board: &Board) {
        self.board_stackup = board.stackup.clone();
        match self.saved.get(&key) {
            Some(s) => {
                self.stackup = s.stackup.clone();
                // Stack-ups saved before Trace kept loss tangents: take the
                // file's where the saved layer has none.
                for l in &mut self.stackup.layers {
                    if let Some(b) = board
                        .stackup
                        .layers
                        .iter()
                        .find(|b| b.name == l.name && b.kind == l.kind)
                    {
                        l.loss_tangent = l.loss_tangent.or(b.loss_tangent);
                        l.roughness_um = l.roughness_um.or(b.roughness_um);
                    }
                }
                self.se = s.se;
                self.diff = s.diff;
                self.tol = s.tol;
                self.rules = s.rules.clone();
            }
            None => {
                self.stackup = board.stackup.clone();
                self.se = 50.0;
                self.diff = 100.0;
                self.tol = 10.0;
                self.rules = default_rules(board);
            }
        }
        self.key = key;
        self.changed(false);
    }

    fn changed(&mut self, keep: bool) {
        self.geo = layer_geometry(&self.stackup);
        self.rows = None;
        if keep {
            self.saved.insert(
                self.key.clone(),
                Saved {
                    stackup: self.stackup.clone(),
                    se: self.se,
                    diff: self.diff,
                    tol: self.tol,
                    rules: self.rules.clone(),
                },
            );
        }
    }

    fn len(&self, mil: f64) -> String {
        if !mil.is_finite() {
            return "–".into();
        }
        if self.mm {
            format!("{:.3}", mil * 0.0254)
        } else {
            format!("{mil:.2}")
        }
    }

    fn off(&self, z: f64, target: f64) -> bool {
        !z.is_finite() || ((z - target) / target).abs() * 100.0 > self.tol
    }

    /// The target for a net: the first matching rule, else the default.
    fn target(&self, name: &str, pair: bool) -> f64 {
        self.rules
            .iter()
            .find(|r| r.pair == pair && r.ohms > 0.0 && matches(name, &r.pattern))
            .map(|r| r.ohms)
            .unwrap_or(if pair { self.diff } else { self.se })
    }

    /// Width and gap for a pair carrying `net` on each copper layer, from
    /// the stack-up and the pair's target, where they can be worked out.
    /// The stack-up as edited here, for the board loaded last.
    pub fn stackup(&self) -> &Stackup {
        &self.stackup
    }

    pub fn pair_sizes(&mut self, net: &str, copper: usize) -> Vec<Option<(f64, f64)>> {
        if self.geo.is_empty() {
            self.geo = layer_geometry(&self.stackup);
        }
        let t = self.target(net, true);
        (0..copper)
            .map(|i| {
                let g = self.geo.iter().find(|g| g.copper as usize == i)?;
                let w = g.width_for(t / 2.0)? * 0.8;
                let s = g.gap_for(w, t)?;
                Some((w, s))
            })
            .collect()
    }

    /// Pair nets off target, for the status bar.
    pub fn failing(&mut self, board: &Board) -> usize {
        self.rows(board);
        let rows = self.rows.as_ref().unwrap();
        rows.iter()
            .filter(|r| {
                r.zdiff.is_some_and(|z| {
                    r.length > 100.0
                        && self.off(z, self.target(&board.nets[r.net as usize].name, true))
                })
            })
            .map(|r| r.net)
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    fn rows(&mut self, board: &Board) -> &Vec<NetImpedance> {
        if self.rows.is_none() {
            if self.geo.is_empty() {
                self.geo = layer_geometry(&self.stackup);
            }
            self.rows = Some(board.net_impedance_with(&self.geo));
        }
        self.rows.as_ref().unwrap()
    }

    /// Shows the window; returns a net the person clicked.
    pub fn show(&mut self, ctx: &egui::Context, board: &Board) -> Option<u32> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut pick = None;
        egui::Window::new("Impedance")
            .open(&mut open)
            .default_size([1000.0, 640.0])
            .resizable(true)
            .show(ctx, |ui| pick = self.body(ui, board));
        self.open = open;
        pick
    }

    fn body(&mut self, ui: &mut egui::Ui, board: &Board) -> Option<u32> {
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label("Single-ended target");
            edited |= ui
                .add(
                    egui::DragValue::new(&mut self.se)
                        .range(10.0..=200.0)
                        .suffix(" Ω"),
                )
                .changed();
            ui.label("Differential target");
            edited |= ui
                .add(
                    egui::DragValue::new(&mut self.diff)
                        .range(20.0..=300.0)
                        .suffix(" Ω"),
                )
                .changed();
            ui.label("±");
            edited |= ui
                .add(
                    egui::DragValue::new(&mut self.tol)
                        .range(1.0..=50.0)
                        .suffix(" %"),
                )
                .changed();
            ui.separator();
            ui.selectable_value(&mut self.mm, false, "mil");
            ui.selectable_value(&mut self.mm, true, "mm");
            ui.separator();
            if ui
                .button("Reset stack-up to board")
                .on_hover_text("Undo your stack-up edits")
                .clicked()
            {
                self.stackup = self.board_stackup.clone();
                edited = true;
            }
        });
        egui::CollapsingHeader::new(format!("Per-net targets ({})", self.rules.len()))
            .id_salt("imprules")
            .show(ui, |ui| {
                ui.label(
                    RichText::new("The first matching rule wins. Use * as a wildcard; without one the pattern matches anywhere in the name.")
                        .weak()
                        .small(),
                );
                let mut del = None;
                egui::Grid::new("rulegrid").spacing([8.0, 3.0]).show(ui, |ui| {
                    for (i, r) in self.rules.iter_mut().enumerate() {
                        edited |= ui
                            .add(egui::TextEdit::singleline(&mut r.pattern).desired_width(160.0))
                            .changed();
                        egui::ComboBox::from_id_salt(("rulekind", i))
                            .width(90.0)
                            .selected_text(if r.pair { "Pair (Zdiff)" } else { "Single (Z0)" })
                            .show_ui(ui, |ui| {
                                edited |= ui.selectable_value(&mut r.pair, true, "Pair (Zdiff)").changed();
                                edited |= ui.selectable_value(&mut r.pair, false, "Single (Z0)").changed();
                            });
                        edited |= ui
                            .add(egui::DragValue::new(&mut r.ohms).range(10.0..=300.0).suffix(" Ω"))
                            .changed();
                        if ui.small_button("✖").on_hover_text("Remove").clicked() {
                            del = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = del {
                    self.rules.remove(i);
                    edited = true;
                }
                if ui.button("Add rule").clicked() {
                    self.rules.push(Rule {
                        pattern: String::new(),
                        pair: true,
                        ohms: 90.0,
                    });
                    edited = true;
                }
            });
        if self.stackup.layers.is_empty() {
            ui.label(
                RichText::new(
                    "This board has no stack-up (Gerber and some Allegro files do not carry one).",
                )
                .color(FAIL),
            );
            return None;
        }
        ui.separator();
        let unit = if self.mm { "mm" } else { "mil" };
        // Stack-up, editable, with what each copper layer needs for the targets.
        ui.label(RichText::new("Stack-up").strong());
        egui::ScrollArea::vertical()
            .id_salt("stack")
            .max_height(260.0)
            .show(ui, |ui| {
                egui::Grid::new("stackgrid")
                    .striped(true)
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        for h in [
                            "Layer".to_string(),
                            "Type".into(),
                            format!("Thickness ({unit})"),
                            "Er".into(),
                            "Df".into(),
                            "Roughness µm".into(),
                            format!("{:.0} Ω width", self.se),
                            format!("{:.0} Ω pair w / gap", self.diff),
                            "Delay ps/in".into(),
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        let mut ci = 0usize;
                        let k = if self.mm { 0.0254 } else { 1.0 };
                        for li in 0..self.stackup.layers.len() {
                            let l = &mut self.stackup.layers[li];
                            if !matches!(l.kind, LayerKind::Copper | LayerKind::Dielectric) {
                                continue;
                            }
                            ui.label(&l.name);
                            ui.label(match l.kind {
                                LayerKind::Copper => "copper",
                                _ => l.material.as_deref().unwrap_or("dielectric"),
                            });
                            let mut t = l.thickness.mil() * k;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut t)
                                        .speed(0.01)
                                        .range(0.0..=1000.0)
                                        .max_decimals(4),
                                )
                                .changed()
                            {
                                l.thickness = Length::from_mil(t / k);
                                edited = true;
                            }
                            if l.kind == LayerKind::Dielectric {
                                let mut er = l.dielectric_constant.unwrap_or(DEFAULT_ER);
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut er)
                                            .speed(0.01)
                                            .range(1.0..=20.0)
                                            .max_decimals(3),
                                    )
                                    .changed()
                                {
                                    l.dielectric_constant = Some(er);
                                    edited = true;
                                }
                                match l.loss_tangent {
                                    Some(mut df) => {
                                        if ui
                                            .add(
                                                egui::DragValue::new(&mut df)
                                                    .speed(0.0005)
                                                    .range(0.0..=0.2)
                                                    .max_decimals(4),
                                            )
                                            .on_hover_text("Loss tangent, used by the signal simulation")
                                            .changed()
                                        {
                                            l.loss_tangent = Some(df);
                                            edited = true;
                                        }
                                    }
                                    None => {
                                        if ui
                                            .small_button("–")
                                            .on_hover_text(
                                                "The file gives no loss tangent; the simulation's \
                                                 own value is used. Click to set one (FR-4: 0.02)",
                                            )
                                            .clicked()
                                        {
                                            l.loss_tangent = Some(0.02);
                                            edited = true;
                                        }
                                    }
                                }
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                ui.label("");
                            } else {
                                ui.label("");
                                ui.label("");
                                match l.roughness_um {
                                    Some(mut r) => {
                                        if ui
                                            .add(
                                                egui::DragValue::new(&mut r)
                                                    .speed(0.02)
                                                    .range(0.0..=10.0)
                                                    .max_decimals(2),
                                            )
                                            .on_hover_text(
                                                "Copper roughness (RMS), used by the signal simulation",
                                            )
                                            .changed()
                                        {
                                            l.roughness_um = Some(r);
                                            edited = true;
                                        }
                                    }
                                    None => {
                                        if ui
                                            .small_button("–")
                                            .on_hover_text(
                                                "No roughness given; the simulation's own value is \
                                                 used. Click to set one (standard foil about 1 µm, \
                                                 HVLP 0.4 µm)",
                                            )
                                            .clicked()
                                        {
                                            l.roughness_um = Some(1.0);
                                            edited = true;
                                        }
                                    }
                                }
                                let g = self.geo.get(ci).cloned();
                                ci += 1;
                                match g {
                                    Some(g) => {
                                        let w = g.width_for(self.se);
                                        let pw = g.width_for(self.diff / 2.0).map(|w| w * 0.8);
                                        let ps = pw.and_then(|w| g.gap_for(w, self.diff));
                                        let f = |v: Option<f64>| {
                                            v.map(|x| {
                                                if self.mm {
                                                    format!("{:.3}", x * 0.0254)
                                                } else {
                                                    format!("{x:.2}")
                                                }
                                            })
                                            .unwrap_or_else(|| "–".into())
                                        };
                                        ui.label(f(w));
                                        ui.label(match (pw, ps) {
                                            (Some(_), Some(_)) => format!("{} / {}", f(pw), f(ps)),
                                            _ => "–".into(),
                                        });
                                        ui.label(format!(
                                            "{:.0}",
                                            g.delay_ps_per_in(w.unwrap_or(5.0))
                                        ));
                                    }
                                    None => {
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        if edited {
            self.changed(true);
        }
        ui.separator();
        // A quick calculator.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Calculator").strong());
            let names: Vec<String> = self.geo.iter().map(|g| g.name.clone()).collect();
            self.calc_layer = self.calc_layer.min(names.len().saturating_sub(1));
            egui::ComboBox::from_id_salt("calc_layer")
                .selected_text(names.get(self.calc_layer).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut self.calc_layer, i, n);
                    }
                });
            let k = if self.mm { 0.0254 } else { 1.0 };
            let mut w = self.calc_w * k;
            let mut s = self.calc_s * k;
            ui.label("width");
            if ui
                .add(egui::DragValue::new(&mut w).speed(0.01).range(0.01..=500.0))
                .changed()
            {
                self.calc_w = w / k;
            }
            ui.label("gap");
            if ui
                .add(egui::DragValue::new(&mut s).speed(0.01).range(0.01..=500.0))
                .changed()
            {
                self.calc_s = s / k;
            }
            ui.label(unit);
            if let Some(g) = self.geo.get(self.calc_layer) {
                let z0 = g.z0(self.calc_w);
                let zd = g.zdiff(self.calc_w, self.calc_s);
                ui.separator();
                ui.label(
                    RichText::new(format!("Z0 {z0:.1} Ω"))
                        .strong()
                        .color(if self.off(z0, self.se) { FAIL } else { PASS }),
                );
                ui.label(
                    RichText::new(format!("Zdiff {zd:.1} Ω"))
                        .strong()
                        .color(if self.off(zd, self.diff) { FAIL } else { PASS }),
                );
                ui.label(
                    RichText::new(if g.is_outer() {
                        "microstrip"
                    } else {
                        "stripline"
                    })
                    .weak(),
                );
            }
        });
        ui.separator();
        // Routed nets.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Routed nets").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("filter, e.g. USB or DDR*")
                    .desired_width(180.0),
            );
            ui.checkbox(&mut self.pairs_only, "Differential pairs only");
            ui.checkbox(&mut self.off_only, "Off target only");
        });
        let names: Vec<String> = self.geo.iter().map(|g| g.name.clone()).collect();
        let (pairs_only, off_only) = (self.pairs_only, self.off_only);
        let filter = self.filter.trim().to_ascii_uppercase();
        let rows: Vec<NetImpedance> = self
            .rows(board)
            .iter()
            .filter(|r| r.length > 20.0)
            .filter(|r| !pairs_only || r.zdiff.is_some())
            .filter(|r| filter.is_empty() || matches(&board.nets[r.net as usize].name, &filter))
            .cloned()
            .collect();
        let rows: Vec<NetImpedance> = rows
            .into_iter()
            .filter(|r| {
                let name = &board.nets[r.net as usize].name;
                !off_only
                    || match r.zdiff {
                        Some(z) => self.off(z, self.target(name, true)),
                        None => self.off(r.z0, self.target(name, false)),
                    }
            })
            .collect();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("nets")
            .max_height((ui.available_height() - 28.0).max(120.0))
            .show(ui, |ui| {
                egui::Grid::new("impnets")
                    .striped(true)
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        for h in [
                            "Net".to_string(),
                            "Layer".into(),
                            format!("Width ({unit})"),
                            format!("Gap ({unit})"),
                            "Z0 (Ω)".into(),
                            "Zdiff (Ω)".into(),
                            format!("Length ({unit})"),
                            "Target (Ω)".into(),
                            "Status".into(),
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for r in rows.iter().take(2000) {
                            let name = &board.nets[r.net as usize].name;
                            if ui.link(name).clicked() {
                                pick = Some(r.net);
                            }
                            ui.label(names.get(r.layer as usize).cloned().unwrap_or_default());
                            ui.label(self.len(r.width));
                            ui.label(r.gap.map(|g| self.len(g)).unwrap_or_default());
                            ui.label(format!("{:.1}", r.z0));
                            ui.label(r.zdiff.map(|z| format!("{z:.1}")).unwrap_or_default());
                            ui.label(self.len(r.length));
                            let z = r.zdiff.unwrap_or(r.z0);
                            let t = self.target(name, r.zdiff.is_some());
                            ui.label(format!("{t:.0}"));
                            let (bad, what) =
                                (self.off(z, t), format!("{:+.0}%", (z - t) / t * 100.0));
                            ui.label(
                                RichText::new(if bad {
                                    format!("off {what}")
                                } else {
                                    "ok".into()
                                })
                                .color(if bad {
                                    FAIL
                                } else {
                                    PASS
                                }),
                            );
                            ui.end_row();
                        }
                    });
                if rows.len() > 2000 {
                    ui.label(
                        RichText::new(format!("{} more rows; use the filter.", rows.len() - 2000))
                            .weak(),
                    );
                }
                if rows.is_empty() {
                    ui.label(RichText::new("No routed nets match.").weak());
                }
            });
        ui.label(
            RichText::new(
                "Closed-form models (Hammerstad–Jensen microstrip, Wheeler stripline, IPC-2141 coupling); every copper layer is taken as a reference plane and solder mask is ignored. Expect a few percent against a field solver.",
            )
            .weak()
            .small(),
        );
        pick
    }
}

/// Common interfaces whose pairs are not 100 Ω, when the board has them.
fn default_rules(board: &Board) -> Vec<Rule> {
    let has = |p: &str| board.nets.iter().any(|n| matches(&n.name, p));
    let mut v = Vec::new();
    for (p, ohms) in [
        ("USB", 90.0),
        ("PCIE", 85.0),
        ("HDMI", 100.0),
        ("SATA", 100.0),
    ] {
        if ohms != 100.0 && has(p) {
            v.push(Rule {
                pattern: p.into(),
                pair: true,
                ohms,
            });
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn patterns() {
        assert!(matches("FX3_USB_D_P", "usb"));
        assert!(matches("DDR_DQ0", "DDR*"));
        assert!(!matches("XDDR_DQ0", "DDR*"));
        assert!(matches("TXRF1_2BB_N", "TX*_N"));
        assert!(matches("A_B_C", "*B*"));
        assert!(!matches("TXRF1_2BB_P", "TX*_N"));
        assert!(!matches("X", ""));
    }
}
