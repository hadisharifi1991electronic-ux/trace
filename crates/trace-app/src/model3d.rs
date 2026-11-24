//! The realistic 3D model window: a solved structure (a via, a cut-out)
//! ray cast with copper, plated barrels, antipads and see-through
//! laminate; turned with the mouse, cut open, and painted with the 3D
//! FEM solver's fields (|E|, |H| on the cut, surface current on the
//! copper), animated through the cycle.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use trace_core::fem3d::{self, FieldGrid, Model};
use trace_core::render3d::{self, Overlay, Scene, View};

type Solved = Result<Option<FieldGrid>, String>;

struct Run {
    rx: mpsc::Receiver<Solved>,
    line: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

pub struct Model3dWindow {
    pub open: bool,
    title: String,
    model: Option<Arc<Model>>,
    scene: Option<Arc<Scene>>,
    fields: Option<Arc<FieldGrid>>,
    view: View,
    cut_on: bool,
    cut_axis: usize,
    cut_at: f64,
    animate: bool,
    phase: f64,
    f_ghz: f64,
    port: usize,
    dof: usize,
    run: Option<Run>,
    note: Option<String>,
    tex: Option<(String, egui::TextureHandle)>,
    /// Frames since the last mouse move: draw finely once it settles.
    still: u32,
}

impl Default for Model3dWindow {
    fn default() -> Self {
        Model3dWindow {
            open: false,
            title: String::new(),
            model: None,
            scene: None,
            fields: None,
            view: View::default(),
            cut_on: true,
            cut_axis: 1,
            cut_at: 0.0,
            animate: false,
            phase: 0.0,
            f_ghz: 6.0,
            port: 0,
            dof: 200_000,
            run: None,
            note: None,
            tex: None,
            still: 0,
        }
    }
}

impl Model3dWindow {
    /// Shows `model`; `f_ghz` is the frequency offered for the fields.
    pub fn set_model(&mut self, model: Model, title: &str, f_ghz: f64) {
        let sc = Scene::new(&model, 1.4, 1.0);
        let (ax, at) = sc.default_cut();
        self.cut_axis = ax;
        self.cut_at = at;
        self.scene = Some(Arc::new(sc));
        self.model = Some(Arc::new(model));
        self.fields = None;
        self.view.overlay = Overlay::None;
        self.view.phase = None;
        self.title = title.to_string();
        self.f_ghz = f_ghz;
        self.tex = None;
        self.note = None;
        self.open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if let Some(r) = &self.run {
            match r.rx.try_recv() {
                Ok(Ok(g)) => {
                    self.fields = g.map(Arc::new);
                    if self.view.overlay == Overlay::None {
                        self.view.overlay = Overlay::E;
                    }
                    self.run = None;
                    self.tex = None;
                }
                Ok(Err(e)) => {
                    self.note = Some(e);
                    self.run = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                }
                Err(_) => self.run = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("3D model")
            .open(&mut open)
            .default_size([900.0, 680.0])
            .resizable(true)
            .show(ctx, |ui| self.body(ui, ctx));
        self.open = open;
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(sc) = self.scene.clone() else {
            ui.label("Open a via or a cut-out first.");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.cut_on, "Cut open").on_hover_text(
                "Takes away the half of the model on your side of a plane through the \
                 signal via, so the barrel, the pads and the antipads show in section",
            );
            if self.cut_on {
                egui::ComboBox::from_id_salt("m3d-cut")
                    .selected_text(if self.cut_axis == 0 {
                        "across x"
                    } else {
                        "across y"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.cut_axis, 0, "across x");
                        ui.selectable_value(&mut self.cut_axis, 1, "across y");
                    });
                let (lo, hi) = (sc.lo[self.cut_axis], sc.hi[self.cut_axis]);
                ui.add(
                    egui::Slider::new(&mut self.cut_at, lo..=hi)
                        .suffix(" mil")
                        .max_decimals(1),
                );
            }
            ui.checkbox(&mut self.view.laminate, "Laminate");
            ui.checkbox(&mut self.view.shadows, "Shadows");
            if ui.button("Reset view").clicked() {
                let d = View::default();
                self.view.az = d.az;
                self.view.el = d.el;
                self.view.zoom = d.zoom;
                self.view.pan = d.pan;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Fields at");
            ui.add(
                egui::DragValue::new(&mut self.f_ghz)
                    .speed(0.1)
                    .range(0.01..=200.0)
                    .suffix(" GHz"),
            );
            let nports = self.model.as_ref().map_or(1, |m| m.ports.len().max(1));
            egui::ComboBox::from_id_salt("m3d-port")
                .selected_text(format!("P{} driven", self.port + 1))
                .show_ui(ui, |ui| {
                    for p in 0..nports {
                        ui.selectable_value(&mut self.port, p, format!("P{} driven", p + 1));
                    }
                });
            ui.add(
                egui::DragValue::new(&mut self.dof)
                    .speed(10_000)
                    .range(50_000..=2_000_000)
                    .prefix("up to ")
                    .suffix(" unknowns"),
            )
            .on_hover_text("Mesh limit of the 3D FEM solve: more is finer and slower, and takes more memory (about 10 kB each)");
            let busy = self.run.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new("Solve fields (3D FEM)"))
                .clicked()
            {
                self.start(ctx);
            }
            if let Some(r) = &self.run {
                ui.spinner();
                if ui.button("Stop").clicked() {
                    r.stop.store(true, Ordering::Relaxed);
                }
                if let Ok(l) = r.line.lock() {
                    ui.weak(l.as_str());
                }
            }
        });
        if self.fields.is_some() {
            ui.horizontal_wrapped(|ui| {
                for (o, n) in [
                    (Overlay::None, "Geometry"),
                    (Overlay::E, "|E|"),
                    (Overlay::H, "|H|"),
                    (Overlay::J, "Surface current"),
                ] {
                    ui.selectable_value(&mut self.view.overlay, o, n);
                }
                ui.add(
                    egui::Slider::new(&mut self.view.range_db, 10.0..=80.0)
                        .suffix(" dB")
                        .text("range"),
                );
                ui.checkbox(&mut self.animate, "Animate");
                let mut inst = self.view.phase.is_some();
                if ui.checkbox(&mut inst, "Instant").changed() {
                    self.view.phase = inst.then_some(self.phase);
                }
                if inst && !self.animate {
                    ui.add(egui::Slider::new(&mut self.phase, 0.0..=360.0).suffix(" deg"));
                    self.view.phase = Some(self.phase);
                }
            });
        }
        if let Some(n) = &self.note {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 100), n);
        }
        if self.animate && self.fields.is_some() {
            self.phase = (self.phase + 7.5) % 360.0;
            self.view.phase = Some(self.phase);
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        self.view.cut = self.cut_on.then_some((self.cut_axis, self.cut_at));
        self.view.title = self.title.clone();
        // The picture: drag to turn, right-drag to move, wheel to zoom.
        let avail = ui.available_size() - egui::vec2(0.0, 26.0);
        let size = egui::vec2(avail.x.max(200.0), avail.y.max(150.0));
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let mut moved = false;
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            self.view.az -= d.x as f64 * 0.4;
            self.view.el = (self.view.el + d.y as f64 * 0.4).clamp(-89.0, 89.0);
            moved = true;
        }
        if resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
        {
            let d = resp.drag_delta();
            let k = 2.0 / (self.view.zoom * size.y as f64);
            self.view.pan[0] -= d.x as f64 * k;
            self.view.pan[1] += d.y as f64 * k;
            moved = true;
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s != 0.0 {
                self.view.zoom = (self.view.zoom * (1.0 + s as f64 * 0.002)).clamp(0.2, 40.0);
                moved = true;
            }
        }
        if moved || self.animate {
            self.still = 0;
        } else {
            self.still = self.still.saturating_add(1);
        }
        let quick = self.still < 2;
        let ppp = ctx.pixels_per_point();
        let mut v = self.view.clone();
        let scale = if quick { 0.5 } else { 1.0 };
        v.w = ((size.x * ppp) as f64 * scale) as usize;
        v.h = ((size.y * ppp) as f64 * scale) as usize;
        v.samples = if quick { 1 } else { 2 };
        v.shadows = v.shadows && !quick;
        let key = format!(
            "{v:?}{:?}",
            self.fields.as_ref().map(|f| Arc::as_ptr(f) as usize)
        );
        if self.tex.as_ref().is_none_or(|t| t.0 != key) {
            let pic = render3d::render(&sc, self.fields.as_deref(), &v);
            let ci = egui::ColorImage::from_rgb([pic.w, pic.h], &pic.rgb);
            let tex = ctx.load_texture("model3d", ci, egui::TextureOptions::LINEAR);
            self.tex = Some((key, tex));
        }
        if quick && !moved {
            // Settle into the fine picture.
            ctx.request_repaint();
        }
        if let Some((_, t)) = &self.tex {
            ui.painter().image(
                t.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        ui.horizontal(|ui| {
            ui.weak("Drag to turn, right-drag to move, wheel to zoom.");
            if ui.button("Save picture (PNG)…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("PNG", &["png"])
                    .set_file_name("model3d.png")
                    .save_file()
                {
                    let mut v = self.view.clone();
                    v.w = 1800;
                    v.h = 1200;
                    v.samples = 3;
                    let pic = render3d::render(&sc, self.fields.as_deref(), &v);
                    let _ = std::fs::write(p, trace_core::emfield::png(&pic));
                }
            }
            if ui
                .button("Save animation (GIF)…")
                .on_hover_text(
                    "The field through one cycle, or one turn around the model without fields",
                )
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("GIF", &["gif"])
                    .set_file_name("model3d.gif")
                    .save_file()
                {
                    let mut v = self.view.clone();
                    v.w = 800;
                    v.h = 540;
                    v.samples = 1;
                    let fr = render3d::frames(&sc, self.fields.as_deref(), &v, 24);
                    let _ = std::fs::write(p, trace_core::emfield::gif_adaptive(&fr, 8));
                }
            }
            if let Some(f) = &self.fields {
                ui.weak(format!(
                    "Fields: {:.2} GHz, {} driven, grid {:.2} mil",
                    f.f_ghz,
                    f.port,
                    f.h / 25.4e-6
                ));
            }
        });
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let f = self.f_ghz;
        let setup = fem3d::Setup {
            f_adapt: f,
            sweep: vec![f],
            max_unknowns: self.dof,
            fields: Some(fem3d::FieldAsk {
                f_ghz: f,
                port: self.port,
                points: 1_200_000,
            }),
            ..fem3d::Setup::default()
        };
        let (tx, rx) = mpsc::channel();
        let line = Arc::new(Mutex::new("meshing".to_string()));
        let stop = Arc::new(AtomicBool::new(false));
        let (l2, s2, c) = (line.clone(), stop.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = fem3d::solve(&model, &setup, |m| {
                if let Ok(mut l) = l2.lock() {
                    *l = m.to_string();
                }
                c.request_repaint();
                !s2.load(Ordering::Relaxed)
            })
            .map(|s| s.fields);
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.note = None;
        self.run = Some(Run { rx, line, stop });
    }
}
