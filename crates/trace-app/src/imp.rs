//! Impedance window: the board's stack-up (editable), a width and gap
//! calculator for each copper layer, and the impedance of every routed pair
//! checked against a target.

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use trace_core::field::{Line, Pair, Picture};
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

/// Work done off the window's thread: the latest result for a key, and
/// the job running for another.
struct Bg<T> {
    done: Option<(u64, T)>,
    running: Option<(u64, mpsc::Receiver<T>)>,
}

impl<T: Send + 'static> Bg<T> {
    fn new() -> Self {
        Bg {
            done: None,
            running: None,
        }
    }

    /// The newest result (perhaps for an older key), starting the job
    /// `make` gives when nothing is running and the result is not for
    /// `key`.
    fn get<F: FnOnce() -> T + Send + 'static>(
        &mut self,
        key: u64,
        ctx: &egui::Context,
        make: impl FnOnce() -> F,
    ) -> Option<&T> {
        if let Some((k, rx)) = &self.running {
            match rx.try_recv() {
                Ok(v) => {
                    self.done = Some((*k, v));
                    self.running = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        if self.running.is_none() && self.done.as_ref().is_none_or(|d| d.0 != key) {
            let f = make();
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(f());
                ctx.request_repaint();
            });
            self.running = Some((key, rx));
            self.done.as_ref().map(|d| &d.1)
        } else {
            self.done.as_ref().map(|d| &d.1)
        }
    }

    fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn fresh(&self, key: u64) -> bool {
        self.done.as_ref().is_some_and(|d| d.0 == key)
    }
}

/// One layer's widths from the field solver: 50 Ω-style width, pair width
/// and gap, delay.
#[derive(Clone, Copy)]
struct LayerSolve {
    w: Option<f64>,
    pair: Option<(f64, f64)>,
    ps: f64,
}

/// The calculator's cross-section, solved.
struct Calc {
    line: Option<Line>,
    pair: Option<Pair>,
    picture: Option<Arc<Picture>>,
}

pub struct Imp {
    pub open: bool,
    /// Use the 2D field solver rather than formulas.
    field: bool,
    /// Bumped when the stack-up or board changes.
    gen: u64,
    field_rows: Bg<Vec<NetImpedance>>,
    field_layers: Bg<Vec<LayerSolve>>,
    calc: Bg<Calc>,
    /// Pour beside the calculator's track on its layer, and its gap.
    calc_ground: bool,
    calc_g: f64,
    show_field: bool,
    saved: HashMap<String, Saved>,
    key: String,
    board_stackup: Stackup,
    stackup: Stackup,
    stack_msg: Option<String>,
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
            field: true,
            gen: 0,
            field_rows: Bg::new(),
            field_layers: Bg::new(),
            calc: Bg::new(),
            calc_ground: false,
            calc_g: 6.0,
            show_field: true,
            saved: stored
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            key: String::new(),
            board_stackup: Stackup::default(),
            stackup: Stackup::default(),
            stack_msg: None,
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
        self.gen += 1;
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
        self.gen += 1;
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
        if self.field && self.field_rows.fresh(self.gen) {
            if let Some((_, r)) = &self.field_rows.done {
                return r;
            }
        }
        self.rows.as_ref().unwrap()
    }

    /// Starts (or polls) the field solver on the routed nets and on each
    /// layer's widths.
    fn solve_field(&mut self, ctx: &egui::Context, board: &Board) {
        if !self.field {
            return;
        }
        let gen = self.gen;
        let geo = &self.geo;
        self.field_rows.get(gen, ctx, || {
            let (b, geo) = (board.clone(), geo.clone());
            move || b.net_impedance_model(&geo, true)
        });
        let (se, diff) = (self.se, self.diff);
        let key = gen ^ ((se * 1000.0) as u64) << 20 ^ ((diff * 1000.0) as u64) << 40;
        self.field_layers.get(key, ctx, || {
            let geo = geo.clone();
            move || {
                let solve = |g: &LayerGeometry| {
                    let w = g.width_for_field(se);
                    let pw = g.width_for_field(diff / 2.0).map(|w| w * 0.8);
                    let pair = pw.and_then(|pw| Some((pw, g.gap_for_field(pw, diff)?)));
                    LayerSolve {
                        w,
                        pair,
                        ps: g.delay_field(w.unwrap_or(5.0)),
                    }
                };
                std::thread::scope(|sc| {
                    let hs: Vec<_> = geo.iter().map(|g| sc.spawn(move || solve(g))).collect();
                    hs.into_iter()
                        .map(|h| {
                            h.join().unwrap_or(LayerSolve {
                                w: None,
                                pair: None,
                                ps: f64::NAN,
                            })
                        })
                        .collect()
                })
            }
        });
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
        let ctx = ui.ctx().clone();
        self.solve_field(&ctx, board);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.field, "Field solver").on_hover_text(
                "Solve each cross-section numerically (2D field solver): every dielectric with \
                 its own Er and Df, the copper's thickness, the solder mask, and pour beside \
                 the tracks as coplanar ground. Off: closed-form formulas",
            );
            if self.field && (self.field_rows.busy() || self.field_layers.busy()) {
                ui.spinner();
                ui.label(RichText::new("solving…").weak());
            }
            ui.separator();
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
                self.stack_msg = None;
            }
            if ui
                .button("Load stack-up…")
                .on_hover_text("A stack-up file: one layer per line, top first: name, copper or dielectric, thickness, Er, Df")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Stack-up", &["txt", "csv"])
                    .pick_file()
                {
                    let r = std::fs::read_to_string(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|t| trace_core::stackfile::parse(&t))
                        .and_then(|st| trace_core::stackfile::apply_to(&mut self.stackup, &st));
                    self.stack_msg = Some(match r {
                        Ok(()) => {
                            edited = true;
                            format!("Loaded {}", p.display())
                        }
                        Err(e) => format!("Not loaded: {e}"),
                    });
                }
            }
            if ui.button("Save stack-up…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Stack-up", &["txt"])
                    .set_file_name("stackup.txt")
                    .save_file()
                {
                    let _ = std::fs::write(p, trace_core::stackfile::write(&self.stackup));
                }
            }
        });
        if let Some(m) = &self.stack_msg {
            ui.weak(m);
        }
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
                                        let solved = self
                                            .field
                                            .then(|| {
                                                self.field_layers
                                                    .done
                                                    .as_ref()
                                                    .and_then(|d| d.1.get(ci - 1).copied())
                                            })
                                            .flatten();
                                        let (w, pw, ps, delay) = match solved {
                                            Some(f) => (
                                                f.w,
                                                f.pair.map(|p| p.0),
                                                f.pair.map(|p| p.1),
                                                f.ps,
                                            ),
                                            None => {
                                                let w = g.width_for(self.se);
                                                let pw =
                                                    g.width_for(self.diff / 2.0).map(|w| w * 0.8);
                                                let ps = pw.and_then(|w| g.gap_for(w, self.diff));
                                                (w, pw, ps, g.delay_ps_per_in(w.unwrap_or(5.0)))
                                            }
                                        };
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
                                        ui.label(format!("{delay:.0}"));
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
            ui.checkbox(&mut self.calc_ground, "pour beside, gap")
                .on_hover_text("Ground copper on the track's own layer, this far from each edge (coplanar waveguide with ground)");
            let mut gg = self.calc_g * k;
            if ui
                .add_enabled(
                    self.calc_ground,
                    egui::DragValue::new(&mut gg).speed(0.01).range(0.01..=500.0),
                )
                .changed()
            {
                self.calc_g = gg / k;
            }
            if let Some(g) = self.geo.get(self.calc_layer).cloned() {
                let (w, sp) = (self.calc_w, self.calc_s);
                let side = self.calc_ground.then_some(self.calc_g);
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (self.gen, self.calc_layer, w.to_bits(), sp.to_bits(), side.map(f64::to_bits))
                        .hash(&mut h);
                    h.finish()
                };
                let solved = if self.field {
                    let gc = g.clone();
                    self.calc.get(key, &ctx, || {
                        move || {
                            let pair = trace_core::field::pair_picture(&gc, w, sp, side);
                            let line = trace_core::field::single_with(&gc, w, side);
                            Calc {
                                line,
                                picture: pair.as_ref().map(|p| p.1.clone()),
                                pair: pair.map(|p| p.0),
                            }
                        }
                    });
                    self.calc.done.as_ref().filter(|d| d.0 == key).map(|d| &d.1)
                } else {
                    None
                };
                let (z0, zd) = match solved {
                    Some(c) => (
                        c.line.as_ref().map_or(f64::NAN, |l| l.z0),
                        c.pair.as_ref().map_or(f64::NAN, |p| p.zdiff),
                    ),
                    None => (g.z0(w), g.zdiff(w, sp)),
                };
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
                    RichText::new(match (g.is_outer(), side.is_some()) {
                        (true, false) => "microstrip",
                        (false, false) => "stripline",
                        (true, true) => "coplanar, microstrip",
                        (false, true) => "coplanar, stripline",
                    })
                    .weak(),
                );
                if self.field && self.calc.busy() {
                    ui.spinner();
                }
            }
        });
        if self.field {
            let details = self.calc.done.as_ref().map(|d| &d.1);
            if let Some(Calc {
                line: Some(l),
                pair,
                picture,
            }) = details
            {
                let td = self
                    .geo
                    .get(self.calc_layer)
                    .and_then(|g| g.tan_d)
                    .unwrap_or(0.02);
                let rough = self.geo.get(self.calc_layer).and_then(|g| g.rough_um);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Er eff {:.2} · {:.0} ps/in · L {:.2} nH/in · C {:.2} pF/in · loss {:.3} dB/in at 1 GHz, {:.2} at 10 GHz",
                            l.eeff,
                            l.ps_per_in(),
                            l.l * 0.0254e9,
                            l.c * 0.0254e12,
                            l.db_per_in(1e9, td, rough),
                            l.db_per_in(1e10, td, rough)
                        ))
                        .small(),
                    );
                    if let Some(p) = pair {
                        ui.label(
                            RichText::new(format!(
                                "· pair: Z odd {:.1}, Z even {:.1}, Zcomm {:.1} Ω, near-end coupling {:.1} %",
                                p.z_odd,
                                p.z_even,
                                p.zcomm,
                                p.kb * 100.0
                            ))
                            .small(),
                        );
                    }
                    ui.checkbox(&mut self.show_field, "Show the field");
                });
                if self.show_field {
                    if let Some(pic) = picture.clone() {
                        draw_picture(ui, &pic, self.mm);
                    }
                }
            }
        }
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
                if self.field {
                    "2D field solver: each cross-section meshed and solved (finite volumes), with every dielectric's Er and Df, the copper's thickness, the solder mask, and pour beside a track (within 3 dielectric heights) as coplanar ground; loss by Wheeler's rule. Every copper layer is taken as a reference plane."
                } else {
                    "Closed-form models (Hammerstad–Jensen microstrip, Wheeler stripline, IPC-2141 coupling); every copper layer is taken as a reference plane and solder mask is ignored. Expect a few percent against a field solver."
                },
            )
            .weak()
            .small(),
        );
        pick
    }
}

/// The calculator's pair cross-section: the odd mode's potential, the
/// dielectric slabs and the copper.
fn draw_picture(ui: &mut egui::Ui, pic: &Picture, mm: bool) {
    let [x0, x1, y0, y1] = pic.view();
    let width = ui.available_width().clamp(200.0, 900.0);
    let height = (width * ((y1 - y0) / (x1 - x0)) as f32).clamp(80.0, 260.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let sx = |x: f64| rect.left() + ((x - x0) / (x1 - x0)) as f32 * width;
    let sy = |y: f64| rect.bottom() - ((y - y0) / (y1 - y0)) as f32 * height;
    let mut mesh = egui::Mesh::default();
    let (cols, rows) = (140usize, ((140.0 * height / width) as usize).max(20));
    for r in 0..rows {
        for c in 0..cols {
            let (fx, fy) = (
                x0 + (x1 - x0) * (c as f64 + 0.5) / cols as f64,
                y1 - (y1 - y0) * (r as f64 + 0.5) / rows as f64,
            );
            let [cr, cg, cb] = Picture::color(pic.at(fx, fy));
            let cell = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + c as f32 * width / cols as f32,
                    rect.top() + r as f32 * height / rows as f32,
                ),
                egui::vec2(width / cols as f32 + 0.5, height / rows as f32 + 0.5),
            );
            mesh.add_colored_rect(cell, Color32::from_rgb(cr, cg, cb));
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    for &(_, b, _) in &pic.slabs {
        if b > y0 && b < y1 {
            painter.line_segment(
                [
                    egui::pos2(rect.left(), sy(b)),
                    egui::pos2(rect.right(), sy(b)),
                ],
                egui::Stroke::new(0.5_f32, Color32::from_white_alpha(70)),
            );
        }
    }
    for c in &pic.copper {
        let r = egui::Rect::from_min_max(
            egui::pos2(sx(c[0].max(x0)), sy(c[3].min(y1))),
            egui::pos2(
                sx(c[1].min(x1)),
                sy(c[2].max(y0)).max(sy(c[3].min(y1)) + 1.5),
            ),
        );
        painter.rect_filled(r, 0.0, Color32::from_rgb(217, 130, 43));
    }
    let shown = pic.slabs.iter().filter(|s| {
        s.2 > 1.0 && (s.0 + s.1) / 2.0 < y1 && (s.1 - s.0) / (y1 - y0) * height as f64 > 12.0
    });
    for &(a, b, er) in shown {
        painter.text(
            egui::pos2(rect.left() + 4.0, sy((a + b) / 2.0)),
            egui::Align2::LEFT_CENTER,
            format!("Er {er:.2}"),
            egui::FontId::proportional(11.0),
            Color32::from_white_alpha(200),
        );
    }
    if let Some(p) = resp.hover_pos() {
        let x = x0 + ((p.x - rect.left()) / width) as f64 * (x1 - x0);
        let y = y0 + ((rect.bottom() - p.y) / height) as f64 * (y1 - y0);
        let k = if mm { 0.0254 } else { 1.0 };
        resp.on_hover_text(format!(
            "{:.3}, {:.3} {}: {:+.3} V (odd mode, ±1 V)",
            x * k,
            y * k,
            if mm { "mm" } else { "mil" },
            pic.at(x, y)
        ));
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
