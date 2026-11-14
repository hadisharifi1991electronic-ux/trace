//! The fabrication drawing as a PDF: the board outline with its overall
//! dimensions and every drilled hole marked with a symbol for its size, the
//! drill table, the layer stack-up and the fabrication notes, as Allegro's
//! and Altium's drawing templates lay them out.

use crate::assembly::{fit, mm, title_block, today, Paper, Sheet, BLOCK, GRAY, INK, MARGIN};
use crate::fab::plated;
use crate::pdf::{document, text_width, Page, Rgb};
use std::collections::BTreeMap;
use trace_core::{Board, LayerKind};

const DIM: Rgb = [30, 70, 150];
const HOLE: Rgb = [170, 40, 40];
const ROW: Rgb = [200, 200, 205];

/// One drill: size (nm), plated, layers spanned (1-based), how many.
#[derive(Clone, Debug, PartialEq)]
pub struct Drill {
    pub size: i64,
    pub plated: bool,
    pub span: (u8, u8),
    pub count: usize,
    pub via: bool,
    /// Where each hole is (mm).
    pub at: Vec<[f64; 2]>,
}

/// The board's drills, one row per size, plating and span, smallest first.
pub fn drill_table(board: &Board) -> Vec<Drill> {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let mut m: BTreeMap<(i64, bool, u8, u8), Drill> = BTreeMap::new();
    for p in board.geometry.pads.iter().filter(|p| p.hole.0 > 0) {
        let pl = plated(p);
        let d = m.entry((p.hole.0, pl, 1, n)).or_insert(Drill {
            size: p.hole.0,
            plated: pl,
            span: (1, n),
            count: 0,
            via: false,
            at: Vec::new(),
        });
        d.count += 1;
        d.at.push(mm(p.at));
    }
    for v in board.geometry.vias.iter().filter(|v| v.hole.0 > 0) {
        let span = (v.from_layer.min(n - 1) + 1, v.to_layer.min(n - 1) + 1);
        let pl = !v.fab.unplated;
        let d = m.entry((v.hole.0, pl, span.0, span.1)).or_insert(Drill {
            size: v.hole.0,
            plated: pl,
            span,
            count: 0,
            via: true,
            at: Vec::new(),
        });
        d.count += 1;
        d.at.push(mm(v.at));
    }
    m.into_values().collect()
}

/// Draws drill symbol `k` (0..) centred on `c`, `r` points across.
fn symbol(p: &mut Page, k: usize, c: [f64; 2], r: f64, color: Rgb) {
    let w = (r * 0.18).clamp(0.25, 0.8);
    let poly = |p: &mut Page, n: usize, rot: f64| {
        let v: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let t = rot + std::f64::consts::TAU * i as f64 / n as f64;
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect();
        p.polygon(&v, None, Some((w, color)));
    };
    let line = |p: &mut Page, a: [f64; 2], b: [f64; 2]| p.polyline(&[a, b], w, color);
    let base = k % 10;
    match base {
        0 => poly(p, 24, 0.0),
        1 => poly(p, 4, std::f64::consts::FRAC_PI_4),
        2 => poly(p, 3, std::f64::consts::FRAC_PI_2),
        3 => poly(p, 4, 0.0),
        4 => poly(p, 6, 0.0),
        5 => {
            line(p, [c[0] - r, c[1]], [c[0] + r, c[1]]);
            line(p, [c[0], c[1] - r], [c[0], c[1] + r]);
        }
        6 => {
            let d = r * 0.7;
            line(p, [c[0] - d, c[1] - d], [c[0] + d, c[1] + d]);
            line(p, [c[0] - d, c[1] + d], [c[0] + d, c[1] - d]);
        }
        7 => poly(p, 3, -std::f64::consts::FRAC_PI_2),
        8 => poly(p, 8, std::f64::consts::PI / 8.0),
        _ => poly(p, 5, std::f64::consts::FRAC_PI_2),
    }
    // Past ten sizes, a dot or a bar inside tells the symbols apart.
    match k / 10 {
        0 => {}
        1 => p.dot(c, r * 0.3, color),
        _ => line(p, [c[0] - r * 0.5, c[1]], [c[0] + r * 0.5, c[1]]),
    }
}

fn frame(p: &mut Page) {
    let (w, h) = (p.width, p.height);
    p.polygon(
        &[
            [MARGIN, MARGIN],
            [w - MARGIN, MARGIN],
            [w - MARGIN, h - MARGIN],
            [MARGIN, h - MARGIN],
        ],
        None,
        Some((1.0, INK)),
    );
}

/// A dimension line between two points (page units), with its text.
fn dimension(p: &mut Page, a: [f64; 2], b: [f64; 2], text: &str, vertical: bool) {
    p.polyline(&[a, b], 0.5, DIM);
    let arrow = |p: &mut Page, tip: [f64; 2], dir: [f64; 2]| {
        let (l, s) = (5.0, 1.8);
        let n = [-dir[1], dir[0]];
        p.polygon(
            &[
                tip,
                [
                    tip[0] - dir[0] * l + n[0] * s,
                    tip[1] - dir[1] * l + n[1] * s,
                ],
                [
                    tip[0] - dir[0] * l - n[0] * s,
                    tip[1] - dir[1] * l - n[1] * s,
                ],
            ],
            Some(DIM),
            None,
        );
    };
    let d = [(b[0] - a[0]), (b[1] - a[1])];
    let len = d[0].hypot(d[1]).max(1e-9);
    let u = [d[0] / len, d[1] / len];
    arrow(p, b, u);
    arrow(p, a, [-u[0], -u[1]]);
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let size = 8.0;
    if vertical {
        p.text_centered([mid[0] - 7.0, mid[1]], size, false, 90.0, DIM, text);
    } else {
        p.text_centered([mid[0], mid[1] + 7.0], size, false, 0.0, DIM, text);
    }
}

fn size_text(nm: i64) -> String {
    format!(
        "{:.3} mm ({:.1} mil)",
        nm as f64 / 1e6,
        nm as f64 / 25_400.0
    )
}

/// The drawing page: outline, holes, dimensions and the drill table.
fn drawing(board: &Board, paper: Paper, drills: &[Drill], sheet: &Sheet) -> Page {
    let (pw, ph) = paper_size(paper);
    let mut page = Page::new(pw, ph);
    frame(&mut page);
    let g = &board.geometry;
    let pts: Vec<[f64; 2]> = g.outline.iter().map(|&q| mm(q)).collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in pts.iter().chain(drills.iter().flat_map(|d| d.at.iter())) {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    if lo[0] > hi[0] {
        lo = [0.0; 2];
        hi = [100.0; 2];
    }
    // The drill table takes the right-hand third.
    let table_w = (pw * 0.34).min(360.0);
    let area = (
        MARGIN + 40.0,
        MARGIN + BLOCK + 40.0,
        pw - MARGIN - table_w - 30.0,
        ph - MARGIN - 30.0,
    );
    let (bw, bh) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let s = ((area.2 - area.0) / bw).min((area.3 - area.1) / bh);
    let ox = area.0 + ((area.2 - area.0) - bw * s) / 2.0;
    let oy = area.1 + ((area.3 - area.1) - bh * s) / 2.0;
    let tf = |q: [f64; 2]| [ox + s * (q[0] - lo[0]), oy + s * (q[1] - lo[1])];
    for o in std::iter::once(&g.outline).chain(&g.slots) {
        if o.len() >= 3 {
            let v: Vec<[f64; 2]> = o.iter().map(|&q| tf(mm(q))).collect();
            page.polygon(&v, None, Some((1.0, INK)));
        }
    }
    let r = (s * 0.5).clamp(1.4, 4.0);
    for (k, d) in drills.iter().enumerate() {
        for &at in &d.at {
            symbol(&mut page, k, tf(at), r, HOLE);
        }
    }
    // Overall size.
    if g.outline.len() >= 3 {
        let (a, b) = (tf(lo), tf(hi));
        let y = a[1] - 18.0;
        dimension(
            &mut page,
            [a[0], y],
            [b[0], y],
            &format!("{:.2} mm ({:.3} in)", bw, bw / 25.4),
            false,
        );
        let x = a[0] - 18.0;
        dimension(
            &mut page,
            [x, a[1]],
            [x, b[1]],
            &format!("{:.2} mm ({:.3} in)", bh, bh / 25.4),
            true,
        );
        for (p0, p1) in [
            ([a[0], a[1] - 4.0], [a[0], y - 4.0]),
            ([b[0], a[1] - 4.0], [b[0], y - 4.0]),
        ] {
            page.polyline(&[p0, p1], 0.3, DIM);
        }
        for (p0, p1) in [
            ([a[0] - 4.0, a[1]], [x - 4.0, a[1]]),
            ([a[0] - 4.0, b[1]], [x - 4.0, b[1]]),
        ] {
            page.polyline(&[p0, p1], 0.3, DIM);
        }
    }
    // Drill table.
    let x0 = pw - MARGIN - table_w - 10.0;
    let mut y = ph - MARGIN - 20.0;
    page.text([x0, y], 11.0, true, 0.0, INK, "DRILL TABLE");
    y -= 16.0;
    let cols = [0.0, 0.12, 0.52, 0.70, 0.86];
    let head = ["", "Finished size", "Plated", "Layers", "Qty"];
    for (c, h) in cols.iter().zip(head) {
        page.text([x0 + c * table_w, y], 7.5, true, 0.0, GRAY, h);
    }
    y -= 4.0;
    page.polyline(&[[x0, y], [x0 + table_w, y]], 0.6, INK);
    let row_h = 14.0;
    let room = ((y - (MARGIN + BLOCK + 30.0)) / row_h).floor().max(1.0) as usize;
    let shown = drills.len().min(room.saturating_sub(1).max(1));
    for (k, d) in drills.iter().enumerate().take(shown) {
        let yc = y - row_h / 2.0;
        symbol(&mut page, k, [x0 + 8.0, yc], 3.5, HOLE);
        let span =
            if d.span.0 == 1 && d.span.1 as usize == board.stackup.copper_layers().count().max(1) {
                "all".to_string()
            } else {
                format!("{}-{}", d.span.0, d.span.1)
            };
        let cells = [
            size_text(d.size),
            if d.plated {
                if d.via { "yes (via)" } else { "yes" }.to_string()
            } else {
                "no".to_string()
            },
            span,
            d.count.to_string(),
        ];
        for (c, t) in cols[1..].iter().zip(cells.iter()) {
            page.text([x0 + c * table_w, yc - 3.0], 8.0, false, 0.0, INK, t);
        }
        y -= row_h;
        page.polyline(&[[x0, y], [x0 + table_w, y]], 0.3, ROW);
    }
    if shown < drills.len() {
        page.text(
            [x0, y - 12.0],
            8.0,
            false,
            0.0,
            GRAY,
            &format!(
                "... {} more sizes (see the drill files)",
                drills.len() - shown
            ),
        );
        y -= 12.0;
    }
    let total: usize = drills.iter().map(|d| d.count).sum();
    page.text(
        [x0, y - 14.0],
        8.0,
        true,
        0.0,
        INK,
        &format!("{total} holes in {} sizes", drills.len()),
    );
    title_block(&mut page, sheet);
    page
}

fn paper_size(p: Paper) -> (f64, f64) {
    match p {
        Paper::A3 => (1190.55, 841.89),
        Paper::A4 => (841.89, 595.28),
    }
}

/// The notes the drawing carries, from the board's stack-up and rules.
pub fn notes(board: &Board) -> Vec<String> {
    let st = &board.stackup;
    let copper = st.copper_layers().count().max(1);
    let thick = st.total_thickness();
    let mut v = Vec::new();
    let material: Vec<String> = {
        let mut m: Vec<String> = st
            .layers
            .iter()
            .filter(|l| l.kind == LayerKind::Dielectric)
            .filter_map(|l| l.material.clone())
            .filter(|m| !m.trim().is_empty())
            .collect();
        m.sort();
        m.dedup();
        m
    };
    v.push(format!(
        "Material: {}, {copper} copper layers, see the stack-up for the order of layers.",
        if material.is_empty() {
            "FR-4, Tg 170 °C or better".to_string()
        } else {
            material.join(", ")
        }
    ));
    if thick.0 > 0 {
        v.push(format!(
            "Finished thickness {:.2} mm ({:.0} mil) ±10 %, measured over copper and mask.",
            thick.mm(),
            thick.mil()
        ));
    }
    v.push("Fabricate to IPC-6012 class 2; inspect to IPC-A-600 class 2.".into());
    let c = trace_core::route::board_clearance(board);
    let w = trace_core::route::board_width(board);
    v.push(format!(
        "Smallest track {w:.1} mil ({:.3} mm) and gap {c:.1} mil ({:.3} mm).",
        w * 0.0254,
        c * 0.0254
    ));
    let min_hole = board
        .geometry
        .pads
        .iter()
        .map(|p| p.hole.0)
        .chain(board.geometry.vias.iter().map(|v| v.hole.0))
        .filter(|&h| h > 0)
        .min();
    if let Some(h) = min_hole {
        v.push(format!(
            "Hole sizes are finished sizes; smallest {}. Plated holes ±0.08 mm, unplated ±0.05 mm.",
            size_text(h)
        ));
    }
    if !board.diff_pairs.is_empty()
        || board
            .rules
            .iter()
            .any(|r| r.kind.eq_ignore_ascii_case("Impedance"))
    {
        v.push("Controlled impedance: hold the impedances in the stack-up and design rules to ±10 %; the fabricator may adjust track widths to meet them.".into());
    }
    if board
        .geometry
        .vias
        .iter()
        .any(|v| v.fab.back_top.is_some() || v.fab.back_bottom.is_some())
    {
        v.push(
            "Backdrill the vias listed in the backdrill files to the depths given there.".into(),
        );
    }
    v.push(
        "Solder mask: green LPI both sides. Silkscreen: white epoxy ink, both sides where used."
            .into(),
    );
    v.push("Surface finish: ENIG.".into());
    v.push("Electrical test 100 % of nets against the IPC-D-356 netlist supplied.".into());
    v.push("Dimensions in millimetres unless stated; the Gerber, drill and ODB++ or IPC-2581 data rule over this drawing.".into());
    v
}

/// The stack-up and notes page.
fn stack_page(board: &Board, paper: Paper, sheet: &Sheet) -> Page {
    let (pw, ph) = paper_size(paper);
    let mut page = Page::new(pw, ph);
    frame(&mut page);
    let x0 = MARGIN + 30.0;
    let width = pw - 2.0 * MARGIN - 60.0;
    let mut y = ph - MARGIN - 30.0;
    page.text([x0, y], 12.0, true, 0.0, INK, "LAYER STACK-UP");
    y -= 18.0;
    let cols = [0.0, 0.06, 0.30, 0.44, 0.66, 0.78, 0.89];
    let head = ["#", "Layer", "Type", "Material", "Thickness", "Er", "Df"];
    for (c, h) in cols.iter().zip(head) {
        page.text([x0 + c * width, y], 8.0, true, 0.0, GRAY, h);
    }
    y -= 4.0;
    page.polyline(&[[x0, y], [x0 + width, y]], 0.6, INK);
    let rows: Vec<&trace_core::StackLayer> = board
        .stackup
        .layers
        .iter()
        .filter(|l| matches!(l.kind, LayerKind::Copper | LayerKind::Dielectric))
        .collect();
    let row_h = 13.0;
    let max_rows = ((y - ph * 0.42) / row_h).floor().max(4.0) as usize;
    let mut cu = 0;
    for l in rows.iter().take(max_rows) {
        let yc = y - row_h / 2.0 - 3.0;
        let copper = l.kind == LayerKind::Copper;
        if copper {
            cu += 1;
            page.polygon(
                &[
                    [x0, y],
                    [x0 + width, y],
                    [x0 + width, y - row_h],
                    [x0, y - row_h],
                ],
                Some([250, 236, 214]),
                None,
            );
        }
        let cells = [
            if copper {
                cu.to_string()
            } else {
                String::new()
            },
            l.name.clone(),
            if copper { "Copper" } else { "Dielectric" }.to_string(),
            l.material.clone().unwrap_or_default(),
            if l.thickness.0 > 0 {
                format!("{:.3} mm ({:.2} mil)", l.thickness.mm(), l.thickness.mil())
            } else {
                "-".into()
            },
            l.dielectric_constant
                .map(|e| format!("{e:.2}"))
                .unwrap_or_default(),
            l.loss_tangent
                .map(|d| format!("{d:.4}"))
                .unwrap_or_default(),
        ];
        for (k, (c, t)) in cols.iter().zip(cells.iter()).enumerate() {
            let room = cols.get(k + 1).copied().unwrap_or(1.0) - c;
            page.text(
                [x0 + c * width, yc],
                8.0,
                false,
                0.0,
                INK,
                &fit(t, room * width / 8.0 - 0.5),
            );
        }
        y -= row_h;
        page.polyline(&[[x0, y], [x0 + width, y]], 0.3, ROW);
    }
    if rows.len() > max_rows {
        y -= 12.0;
        page.text(
            [x0, y],
            8.0,
            false,
            0.0,
            GRAY,
            &format!("... {} more layers", rows.len() - max_rows),
        );
    }
    let t = board.stackup.total_thickness();
    if t.0 > 0 {
        y -= 14.0;
        page.text(
            [x0, y],
            8.5,
            true,
            0.0,
            INK,
            &format!("Total {:.3} mm ({:.1} mil)", t.mm(), t.mil()),
        );
    }
    y -= 30.0;
    page.text([x0, y], 12.0, true, 0.0, INK, "NOTES");
    y -= 16.0;
    for (k, n) in notes(board).iter().enumerate() {
        // Wrap each note to the width.
        let mut line = String::new();
        let mut first = true;
        let size = 9.0;
        let room = (width - 20.0) / size;
        let mut lines = Vec::new();
        for w in n.split(' ') {
            let try_ = if line.is_empty() {
                w.to_string()
            } else {
                format!("{line} {w}")
            };
            if text_width(&try_, false) > room && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line = w.to_string();
            } else {
                line = try_;
            }
        }
        lines.push(line);
        for l in lines {
            if y < MARGIN + BLOCK + 20.0 {
                break;
            }
            if first {
                page.text([x0, y], size, true, 0.0, INK, &format!("{}.", k + 1));
                first = false;
            }
            page.text([x0 + 20.0, y], size, false, 0.0, INK, &l);
            y -= size * 1.35;
        }
        y -= 3.0;
    }
    title_block(&mut page, sheet);
    page
}

/// The fabrication drawing of `board` as a PDF: the drill drawing with
/// the drill table, then the stack-up and notes.
pub fn fab_drawing_pdf(board: &Board, title: &str, paper: Paper) -> Vec<u8> {
    let date = today();
    let drills = drill_table(board);
    let total: usize = drills.iter().map(|d| d.count).sum();
    let s1 = Sheet {
        title,
        what: "Fabrication drawing: drill drawing, seen from the top".into(),
        contents: format!("{total} holes, {} sizes", drills.len()),
        note: "Each hole is marked with its size's symbol from the drill table".into(),
        date: &date,
        index: 1,
        count: 2,
    };
    let s2 = Sheet {
        title,
        what: "Fabrication drawing: stack-up and notes".into(),
        contents: format!(
            "{} copper layers",
            board.stackup.copper_layers().count().max(1)
        ),
        note: String::new(),
        date: &date,
        index: 2,
        count: 2,
    };
    let pages = vec![
        drawing(board, paper, &drills, &s1),
        stack_page(board, paper, &s2),
    ];
    document(&pages, &format!("{title} fabrication drawing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    #[test]
    fn groups_drills_and_writes_two_pages() {
        let mut b = Board::default();
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(2000.0, 0.0),
            Point::mil(2000.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        for (x, d) in [(100.0, 10.0), (200.0, 10.0), (300.0, 40.0)] {
            b.geometry.vias.push(Via {
                net: None,
                from_layer: 0,
                to_layer: 1,
                at: Point::mil(x, 500.0),
                diameter: Length::from_mil(d + 10.0),
                hole: Length::from_mil(d),
                rings: Vec::new(),
                fab: Default::default(),
            });
        }
        let t = drill_table(&b);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].count, 2);
        let pdf = fab_drawing_pdf(&b, "demo", Paper::A3);
        assert!(pdf.starts_with(b"%PDF"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 2"));
        assert!(!notes(&b).is_empty());
    }
}
