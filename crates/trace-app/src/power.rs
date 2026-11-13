//! The Power integrity window: DC voltage drop (IR drop) over a supply
//! net, from its source part to the loads, with a drop map per layer.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::mpsc;
use trace_core::pi::{self, Setup};
use trace_core::Board;

type Solved = Result<pi::Result, String>;

pub struct PowerWindow {
    pub open: bool,
    net: Option<u32>,
    source: Option<usize>,
    volts: f64,
    total: f64,
    plating: f64,
    /// Parts on the net: index, pads on the net, used as a load, amps.
    loads: Vec<(usize, usize, bool, f64)>,
    running: Option<mpsc::Receiver<Solved>>,
    result: Option<(Setup, Solved)>,
    layer: usize,
    texture: Option<(usize, egui::TextureHandle)>,
    /// Nets with copper pours and at least three part pads, by pad count.
    supplies: Option<Vec<u32>>,
    /// Which board the picks are for.
    board_key: (usize, usize, usize),
}

impl Default for PowerWindow {
    fn default() -> Self {
        PowerWindow {
            open: false,
            net: None,
            source: None,
            volts: 1.0,
            total: 1.0,
            plating: 0.8,
            loads: Vec::new(),
            running: None,
            result: None,
            layer: 0,
            texture: None,
            supplies: None,
            board_key: (0, 0, 0),
        }
    }
}

/// A guess at a supply's voltage from its name: "3p3V", "1V8", "VDD_1.2",
/// "12V" and the like.
fn volts_from_name(name: &str) -> Option<f64> {
    let s = name.to_ascii_uppercase().replace('P', ".");
    let b = s.as_bytes();
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.')) {
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.' || b[j] == b'V') {
            j += 1;
        }
        let t = &s[i..j];
        if !t.contains('V') {
            continue;
        }
        let v = if let Some((a, c)) = t.split_once('V') {
            if c.is_empty() {
                a.parse().ok()
            } else {
                format!("{a}.{c}").parse().ok()
            }
        } else {
            None
        };
        if let Some(v) = v.filter(|v: &f64| *v > 0.0 && *v <= 60.0) {
            return Some(v);
        }
    }
    None
}

impl PowerWindow {
    pub fn show(&mut self, ctx: &egui::Context, board: &Board, selected: Option<u32>) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    // Start on the layer with the most copper.
                    if let Ok(r) = &r {
                        self.layer = (0..r.maps.len())
                            .max_by_key(|&i| r.maps[i].v.iter().filter(|v| !v.is_nan()).count())
                            .unwrap_or(0);
                    }
                    if let Some(res) = &mut self.result {
                        res.1 = r;
                    }
                    self.running = None;
                    self.texture = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        let mut open = self.open;
        egui::Window::new("Power integrity (IR drop)")
            .open(&mut open)
            .default_width(820.0)
            .show(ctx, |ui| self.body(ui, ctx, board, selected));
        self.open = open;
    }

    fn supplies(&mut self, board: &Board) -> &[u32] {
        self.supplies.get_or_insert_with(|| {
            let mut pads = vec![0usize; board.nets.len()];
            for p in &board.geometry.pads {
                if let (Some(n), Some(_)) = (p.net, p.component) {
                    if let Some(k) = pads.get_mut(n as usize) {
                        *k += 1;
                    }
                }
            }
            let mut poured = vec![false; board.nets.len()];
            for p in &board.geometry.polygons {
                if let Some(k) = p.net.and_then(|n| poured.get_mut(n as usize)) {
                    *k = true;
                }
            }
            let mut v: Vec<u32> = (0..board.nets.len() as u32)
                .filter(|&n| {
                    let name = board.nets[n as usize].name.to_ascii_uppercase();
                    poured[n as usize]
                        && pads[n as usize] >= 3
                        && !name.contains("GND")
                        && !name.contains("VSS")
                        && !name.contains("EARTH")
                })
                .collect();
            v.sort_by(|a, b| pads[*b as usize].cmp(&pads[*a as usize]));
            v
        })
    }

    fn pick_net(&mut self, board: &Board, net: u32) {
        self.net = Some(net);
        self.source = pi::guess_source(board, net);
        if let Some(v) = volts_from_name(&board.nets[net as usize].name) {
            self.volts = v;
        }
        let chosen: Vec<usize> = self
            .source
            .map(|s| {
                pi::default_loads(board, net, s)
                    .into_iter()
                    .map(|p| p.0)
                    .collect()
            })
            .unwrap_or_default();
        self.loads = pi::parts_on(board, net)
            .into_iter()
            .map(|(p, n)| (p, n, chosen.contains(&p), 0.0))
            .collect();
        self.share();
        self.result = None;
        self.texture = None;
    }

    /// Shares the total current among the chosen loads by pad count.
    fn share(&mut self) {
        let pads: usize = self
            .loads
            .iter()
            .filter(|l| l.2 && Some(l.0) != self.source)
            .map(|l| l.1)
            .sum();
        for l in &mut self.loads {
            l.3 = if l.2 && Some(l.0) != self.source && pads > 0 {
                self.total * l.1 as f64 / pads as f64
            } else {
                0.0
            };
        }
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        board: &Board,
        selected: Option<u32>,
    ) {
        let key = (
            board as *const Board as usize,
            board.nets.len(),
            board.components.len(),
        );
        if key != self.board_key {
            *self = PowerWindow {
                open: true,
                board_key: key,
                ..Default::default()
            };
        }
        let supplies = self.supplies(board).to_vec();
        if self.net.is_none() {
            let first = selected
                .filter(|s| supplies.contains(s))
                .or(supplies.first().copied())
                .or(selected);
            if let Some(n) = first {
                self.pick_net(board, n);
            }
        }
        let Some(net) = self.net else {
            ui.label("This board has no supply nets with copper pours. Select a net on the board, then come back here.");
            return;
        };
        let des = |i: usize| board.components[i].designator.clone();
        ui.horizontal(|ui| {
            ui.label("Net");
            let mut pick = None;
            egui::ComboBox::from_id_salt("pi-net")
                .selected_text(&board.nets[net as usize].name)
                .height(400.0)
                .show_ui(ui, |ui| {
                    if let Some(s) = selected.filter(|s| !supplies.contains(s)) {
                        if ui
                            .selectable_label(
                                false,
                                format!("{} (selected)", board.nets[s as usize].name),
                            )
                            .clicked()
                        {
                            pick = Some(s);
                        }
                    }
                    for &n in &supplies {
                        if ui
                            .selectable_label(n == net, &board.nets[n as usize].name)
                            .clicked()
                        {
                            pick = Some(n);
                        }
                    }
                });
            if let Some(n) = pick.filter(|&n| n != net) {
                self.pick_net(board, n);
            }
            ui.separator();
            ui.label("Source");
            let mut src = self.source;
            egui::ComboBox::from_id_salt("pi-src")
                .selected_text(src.map(des).unwrap_or_else(|| "pick one".into()))
                .height(400.0)
                .show_ui(ui, |ui| {
                    for l in &self.loads {
                        ui.selectable_value(&mut src, Some(l.0), des(l.0));
                    }
                });
            if src != self.source {
                self.source = src;
                self.share();
            }
            ui.separator();
            ui.label("Volts");
            ui.add(
                egui::DragValue::new(&mut self.volts)
                    .speed(0.01)
                    .range(0.1..=60.0)
                    .max_decimals(3),
            );
            ui.label("Plating (mil)");
            ui.add(
                egui::DragValue::new(&mut self.plating)
                    .speed(0.05)
                    .range(0.2..=3.0)
                    .max_decimals(2),
            )
            .on_hover_text("Copper thickness of the via barrels");
        });
        ui.horizontal(|ui| {
            ui.label("Total current (A)");
            ui.add(egui::DragValue::new(&mut self.total).speed(0.05).range(0.0..=200.0).max_decimals(3));
            if ui
                .button("Share by pad count")
                .on_hover_text("Split the total among the ticked loads by how many of their pads are on the net")
                .clicked()
            {
                self.share();
            }
            ui.weak("Capacitors and test points draw no DC current, so they start unticked.");
        });
        ui.collapsing(
            format!("Loads ({} parts on the net)", self.loads.len()),
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .id_salt("pi-loads")
                    .show(ui, |ui| {
                        egui::Grid::new("pi-load-grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("Part");
                                ui.strong("Pads");
                                ui.strong("Load");
                                ui.strong("Amps");
                                ui.end_row();
                                for l in &mut self.loads {
                                    if Some(l.0) == self.source {
                                        continue;
                                    }
                                    ui.label(des(l.0));
                                    ui.label(l.1.to_string());
                                    ui.checkbox(&mut l.2, "");
                                    ui.add_enabled(
                                        l.2,
                                        egui::DragValue::new(&mut l.3)
                                            .speed(0.01)
                                            .range(0.0..=200.0)
                                            .max_decimals(3),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
            },
        );
        ui.horizontal(|ui| {
            let ready = self.source.is_some() && self.running.is_none();
            if ui.add_enabled(ready, egui::Button::new("Solve")).clicked() {
                let mut setup = Setup::new(net, self.source.unwrap_or(0), self.volts);
                setup.plating_mil = self.plating;
                setup.loads = self
                    .loads
                    .iter()
                    .filter(|l| l.2 && l.3 > 0.0 && Some(l.0) != self.source)
                    .map(|l| (l.0, l.3))
                    .collect();
                let b = board.clone();
                let s = setup.clone();
                let (tx, rx) = mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(pi::solve(&b, &s));
                    c.request_repaint();
                });
                self.running = Some(rx);
                self.result = Some((setup, Err(String::new())));
            }
            if self.running.is_some() {
                ui.spinner();
                ui.label("Solving…");
            }
            if let Some((setup, Ok(r))) = &self.result {
                if ui.button("Save maps (SVG)…").clicked() {
                    save_svgs(board, setup, r);
                }
            }
        });
        let Some((setup, Ok(r))) = &self.result else {
            if let Some((_, Err(e))) = &self.result {
                if !e.is_empty() {
                    ui.colored_label(Color32::from_rgb(230, 110, 90), e);
                }
            }
            return;
        };
        ui.separator();
        let total: f64 = r.loads.iter().map(|l| l.amps).sum();
        ui.label(
            RichText::new(format!(
                "Largest drop {:.2} mV ({:.2} %) with {:.3} A, copper loss {:.1} mW",
                r.max_drop * 1000.0,
                r.max_drop / r.volts * 100.0,
                total,
                r.loss_w * 1000.0
            ))
            .strong(),
        );
        let mut info = format!("Grid {:.2} mil, {} vias carrying current", r.cell, r.vias);
        if let Some((p, a)) = r.worst_via {
            info += &format!(
                "; busiest via at ({:.0}, {:.0}) mil carries {a:.3} A",
                p[0], p[1]
            );
        }
        if let Some((p, a, j)) = r.worst_track {
            info += &format!(
                "; densest track at ({:.0}, {:.0}) mil, {a:.3} A, {j:.1} A/mm²",
                p[0], p[1]
            );
        }
        ui.weak(info);
        for n in &r.notes {
            ui.colored_label(Color32::from_rgb(230, 180, 90), n);
        }
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            let mut rows: Vec<_> = r.loads.iter().collect();
            rows.sort_by(|a, b| a.v_min.total_cmp(&b.v_min));
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .id_salt("pi-res")
                .show(ui, |ui| {
                    egui::Grid::new("pi-res-grid").striped(true).show(ui, |ui| {
                        ui.strong("Load");
                        ui.strong("A");
                        ui.strong("V min");
                        ui.strong("Drop mV");
                        ui.end_row();
                        for l in rows {
                            ui.label(des(l.part));
                            ui.label(format!("{:.3}", l.amps));
                            if l.connected {
                                ui.label(format!("{:.4}", l.v_min));
                                let d = r.volts - l.v_min;
                                let [cr, cg, cb] = pi::drop_color(d / r.max_drop.max(1e-12));
                                ui.colored_label(
                                    Color32::from_rgb(cr, cg, cb),
                                    format!("{:.2}", d * 1000.0),
                                );
                            } else {
                                ui.label("—");
                                ui.weak("not joined");
                            }
                            ui.end_row();
                        }
                    });
                });
            let ui = &mut cols[1];
            if r.maps.is_empty() {
                return;
            }
            self.layer = self.layer.min(r.maps.len() - 1);
            ui.horizontal_wrapped(|ui| {
                for (i, m) in r.maps.iter().enumerate() {
                    if ui.selectable_label(self.layer == i, &m.name).clicked() && self.layer != i {
                        self.layer = i;
                        self.texture = None;
                    }
                }
            });
            let map = &r.maps[self.layer];
            if self.texture.as_ref().is_none_or(|t| t.0 != self.layer) {
                let mut px = vec![0u8; r.cols * r.rows * 4];
                for j in 0..r.rows {
                    for i in 0..r.cols {
                        let v = map.v[j * r.cols + i];
                        if v.is_nan() {
                            continue;
                        }
                        let [cr, cg, cb] =
                            pi::drop_color((r.volts - v as f64) / r.max_drop.max(1e-12));
                        let k = ((r.rows - 1 - j) * r.cols + i) * 4;
                        px[k..k + 4].copy_from_slice(&[cr, cg, cb, 255]);
                    }
                }
                let img = egui::ColorImage::from_rgba_unmultiplied([r.cols, r.rows], &px);
                self.texture = Some((
                    self.layer,
                    ctx.load_texture("pi-map", img, egui::TextureOptions::NEAREST),
                ));
            }
            let Some((_, tex)) = &self.texture else {
                return;
            };
            let w = ui.available_width().max(200.0);
            let h = w * r.rows as f32 / r.cols as f32;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(w, h.min(520.0)), egui::Sense::hover());
            let scale = (rect.width() / r.cols as f32).min(rect.height() / r.rows as f32);
            let size = egui::vec2(r.cols as f32 * scale, r.rows as f32 * scale);
            let img = egui::Rect::from_min_size(rect.min, size);
            let p = ui.painter_at(rect);
            p.rect_filled(img, 0.0, Color32::from_rgb(15, 18, 24));
            p.image(
                tex.id(),
                img,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let to_screen = |q: [f64; 2]| {
                egui::pos2(
                    img.min.x + ((q[0] - r.origin[0]) / r.cell) as f32 * scale,
                    img.max.y - ((q[1] - r.origin[1]) / r.cell) as f32 * scale,
                )
            };
            for (q, label) in marks(board, setup, r) {
                let at = to_screen(q);
                p.circle_stroke(at, 4.0, Stroke::new(1.5_f32, Color32::WHITE));
                p.text(
                    at + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    label,
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
            // Scale bar.
            ui.horizontal(|ui| {
                let (bar, _) =
                    ui.allocate_exact_size(egui::vec2(160.0, 12.0), egui::Sense::hover());
                for k in 0..32 {
                    let [cr, cg, cb] = pi::drop_color(k as f64 / 31.0);
                    let x0 = bar.min.x + bar.width() * k as f32 / 32.0;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, bar.min.y),
                            egui::pos2(x0 + bar.width() / 32.0 + 0.5, bar.max.y),
                        ),
                        0.0,
                        Color32::from_rgb(cr, cg, cb),
                    );
                }
                ui.label(format!("drop 0 to {:.2} mV", r.max_drop * 1000.0));
            });
        });
    }
}

/// The source and the heaviest loads, each at the middle of its pads on
/// the net.
fn marks(board: &Board, setup: &Setup, r: &pi::Result) -> Vec<([f64; 2], String)> {
    let mid = |part: usize| -> Option<[f64; 2]> {
        let ps: Vec<[f64; 2]> = board
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(part as u32) && p.net == Some(setup.net))
            .map(|p| [p.at.x.mil(), p.at.y.mil()])
            .collect();
        (!ps.is_empty()).then(|| {
            let n = ps.len() as f64;
            [
                ps.iter().map(|p| p[0]).sum::<f64>() / n,
                ps.iter().map(|p| p[1]).sum::<f64>() / n,
            ]
        })
    };
    let des = |i: usize| board.components[i].designator.clone();
    let mut out = Vec::new();
    if let Some(p) = mid(setup.source) {
        out.push((p, format!("{} (source)", des(setup.source))));
    }
    let mut heavy: Vec<_> = r.loads.iter().collect();
    heavy.sort_by(|a, b| b.amps.total_cmp(&a.amps));
    for l in heavy.into_iter().take(12) {
        if let Some(p) = mid(l.part) {
            out.push((p, des(l.part)));
        }
    }
    out
}

fn save_svgs(board: &Board, setup: &Setup, r: &pi::Result) {
    let name = board.nets[setup.net as usize].name.clone();
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect()
    };
    let Some(dir) = rfd::FileDialog::new()
        .set_title("Folder for the drop maps")
        .pick_folder()
    else {
        return;
    };
    let m = marks(board, setup, r);
    for map in &r.maps {
        let f = dir.join(format!("ir-{}-{}.svg", safe(&name), safe(&map.name)));
        let _ = std::fs::write(
            f,
            r.svg(map, 1200.0, &format!("{name} on {}", map.name), &m),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::volts_from_name;

    #[test]
    fn volts_from_names() {
        assert_eq!(volts_from_name("3p3V_Utility"), Some(3.3));
        assert_eq!(volts_from_name("1p0V_DAC"), Some(1.0));
        assert_eq!(volts_from_name("+1V8"), Some(1.8));
        assert_eq!(volts_from_name("12V_FMC"), Some(12.0));
        assert_eq!(volts_from_name("DVDD"), None);
    }
}
