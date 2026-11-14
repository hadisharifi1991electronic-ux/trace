//! Assembly drawings as PDF: the board seen from the top and from the
//! bottom with each part's body, pads, silk outline, designator and pin 1
//! mark, then the parts list.

use crate::fab::{natural, part_info};
use crate::pdf::{document, text_width, Page, Rgb};
use std::collections::{BTreeMap, BTreeSet};
use trace_core::{Board, LayerRef, Pad, PadShape, Point, Side};

/// Paper for the drawing, landscape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    A3,
    A4,
}

impl Paper {
    fn size(self) -> (f64, f64) {
        match self {
            Paper::A3 => (1190.55, 841.89),
            Paper::A4 => (841.89, 595.28),
        }
    }
}

pub(crate) const MARGIN: f64 = 24.0;
pub(crate) const BLOCK: f64 = 50.0;
pub(crate) const INK: Rgb = [20, 20, 20];
pub(crate) const GRAY: Rgb = [110, 110, 110];
const BODY: Rgb = [232, 239, 249];
const BODY_EDGE: Rgb = [120, 140, 165];
const PAD: Rgb = [196, 196, 196];
const SILK: Rgb = [60, 60, 60];
const PIN1: Rgb = [176, 24, 24];
const LABEL: Rgb = [15, 35, 80];

pub(crate) fn mm(p: Point) -> [f64; 2] {
    [p.x.mm(), p.y.mm()]
}

fn rot(v: [f64; 2], deg: f64) -> [f64; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

/// The copper outline of a pad, mm, board coordinates.
fn pad_shape(p: &Pad) -> Vec<[f64; 2]> {
    if p.outline.len() >= 3 {
        return p.outline.iter().map(|&q| mm(q)).collect();
    }
    let at = mm(p.at);
    let (w, h) = (p.size_x.mm(), p.size_y.mm());
    let local: Vec<[f64; 2]> = match p.shape {
        PadShape::Rect => vec![
            [-w / 2.0, -h / 2.0],
            [w / 2.0, -h / 2.0],
            [w / 2.0, h / 2.0],
            [-w / 2.0, h / 2.0],
        ],
        PadShape::Octagon => {
            let c = w.min(h) * 0.29;
            let (x, y) = (w / 2.0, h / 2.0);
            vec![
                [-x + c, -y],
                [x - c, -y],
                [x, -y + c],
                [x, y - c],
                [x - c, y],
                [-x + c, y],
                [-x, y - c],
                [-x, -y + c],
            ]
        }
        PadShape::Round => {
            // A circle, or a stadium along the longer side.
            let r = w.min(h) / 2.0;
            let (dx, dy) = if w >= h {
                (w / 2.0 - r, 0.0)
            } else {
                (0.0, h / 2.0 - r)
            };
            let mut v = Vec::with_capacity(26);
            for half in 0..2 {
                let (cx, cy, a0) = if half == 0 {
                    (dx, dy, if w >= h { -90.0 } else { 0.0 })
                } else {
                    (-dx, -dy, if w >= h { 90.0 } else { 180.0 })
                };
                for k in 0..=12 {
                    let a = (a0 + 180.0 * k as f64 / 12.0_f64).to_radians();
                    v.push([cx + r * a.cos(), cy + r * a.sin()]);
                }
            }
            v
        }
    };
    local
        .into_iter()
        .map(|q| {
            let r = rot(q, p.rotation_deg);
            [at[0] + r[0], at[1] + r[1]]
        })
        .collect()
}

fn arc_points(c: [f64; 2], r: f64, a0: f64, a1: f64) -> Vec<[f64; 2]> {
    let mut a1 = a1;
    while a1 <= a0 {
        a1 += 360.0;
    }
    let steps = (((a1 - a0) / 10.0).ceil() as usize).clamp(2, 72);
    (0..=steps)
        .map(|k| {
            let a = (a0 + (a1 - a0) * k as f64 / steps as f64).to_radians();
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// What one part looks like on the drawing, mm, board coordinates.
struct PartArt {
    designator: String,
    /// The body's corners.
    body: [[f64; 2]; 4],
    center: [f64; 2],
    /// Body size along and across the part's own x axis, and the angle of
    /// its longer side.
    long: f64,
    short: f64,
    angle: f64,
    pads: Vec<Vec<[f64; 2]>>,
    silk: Vec<(Vec<[f64; 2]>, f64)>,
    silk_fill: Vec<Vec<[f64; 2]>>,
    pin1: Option<([f64; 2], f64)>,
}

fn parts_on(board: &Board, side: Side) -> Vec<PartArt> {
    let g = &board.geometry;
    let silk_layer = if side == Side::Top {
        LayerRef::TopSilk
    } else {
        LayerRef::BottomSilk
    };
    let n = board.components.len();
    let mut pads: Vec<Vec<&Pad>> = vec![Vec::new(); n];
    for p in &g.pads {
        if let Some(c) = p.component.map(|c| c as usize).filter(|&c| c < n) {
            pads[c].push(p);
        }
    }
    let mut silk: Vec<Vec<(Vec<[f64; 2]>, f64)>> = vec![Vec::new(); n];
    for t in &g.tracks {
        if let Some(c) = t.component.map(|c| c as usize).filter(|&c| c < n) {
            if !t.text && t.layer == silk_layer {
                silk[c].push((vec![mm(t.start), mm(t.end)], t.width.mm()));
            }
        }
    }
    for a in &g.arcs {
        if let Some(c) = a.component.map(|c| c as usize).filter(|&c| c < n) {
            if a.layer == silk_layer {
                silk[c].push((
                    arc_points(mm(a.center), a.radius.mm(), a.start_deg, a.end_deg),
                    a.width.mm(),
                ));
            }
        }
    }
    let mut fills: Vec<Vec<Vec<[f64; 2]>>> = vec![Vec::new(); n];
    for a in &g.areas {
        if let Some(c) = a.component.map(|c| c as usize).filter(|&c| c < n) {
            if a.layer == silk_layer && a.outline.len() >= 3 {
                fills[c].push(a.outline.iter().map(|&q| mm(q)).collect());
            }
        }
    }
    let mut out = Vec::new();
    for (i, c) in board.components.iter().enumerate() {
        if c.side != side {
            continue;
        }
        let o = [c.x.mm(), c.y.mm()];
        let r = c.rotation_deg;
        let shapes: Vec<Vec<[f64; 2]>> = pads[i].iter().map(|p| pad_shape(p)).collect();
        // Extents in the part's own frame.
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        let mut grow = |q: [f64; 2], pad: f64| {
            let l = rot([q[0] - o[0], q[1] - o[1]], -r);
            for k in 0..2 {
                lo[k] = lo[k].min(l[k] - pad);
                hi[k] = hi[k].max(l[k] + pad);
            }
        };
        for s in &shapes {
            for &q in s {
                grow(q, 0.0);
            }
        }
        for (line, w) in &silk[i] {
            for &q in line {
                grow(q, w / 2.0);
            }
        }
        if lo[0] > hi[0] {
            continue;
        }
        let corner = |x: f64, y: f64| {
            let q = rot([x, y], r);
            [o[0] + q[0], o[1] + q[1]]
        };
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        let mid = corner((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        let pin1 = pads[i]
            .iter()
            .find(|p| p.name == "1" || p.name.eq_ignore_ascii_case("A1"))
            .filter(|_| pads[i].len() > 2 || polar(&c.designator))
            .map(|p| (mm(p.at), p.size_x.mm().min(p.size_y.mm())));
        out.push(PartArt {
            designator: c.designator.clone(),
            body: [
                corner(lo[0], lo[1]),
                corner(hi[0], lo[1]),
                corner(hi[0], hi[1]),
                corner(lo[0], hi[1]),
            ],
            center: mid,
            long: w.max(h),
            short: w.min(h),
            angle: if w >= h { r } else { r + 90.0 },
            pads: shapes,
            silk: std::mem::take(&mut silk[i]),
            silk_fill: std::mem::take(&mut fills[i]),
            pin1,
        });
    }
    out
}

/// Two-pin parts whose way round matters: diodes and LEDs.
fn polar(designator: &str) -> bool {
    let prefix: String = designator
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    matches!(prefix.as_str(), "D" | "LED" | "CR" | "VD")
}

/// Turns an angle so text along it reads from the left or from below.
fn readable(a: f64) -> f64 {
    let a = (a + 90.0).rem_euclid(180.0) - 90.0;
    if a <= -89.5 {
        90.0
    } else {
        a
    }
}

pub(crate) fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Year, month and day of a day counted from 1970-01-01 (Howard
/// Hinnant's method).
pub(crate) fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub(crate) struct Sheet<'a> {
    pub title: &'a str,
    pub what: String,
    pub contents: String,
    pub note: String,
    pub date: &'a str,
    pub index: usize,
    pub count: usize,
}

/// The title block along the bottom of a page.
pub(crate) fn title_block(p: &mut Page, s: &Sheet) {
    let (x0, x1) = (MARGIN, p.width - MARGIN);
    let (y0, y1) = (MARGIN, MARGIN + BLOCK);
    let w = x1 - x0;
    let cuts = [0.0, 0.46, 0.70, 0.82, 1.0].map(|f| x0 + w * f);
    p.polygon(
        &[[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        None,
        Some((1.0, INK)),
    );
    for &x in &cuts[1..4] {
        p.polyline(&[[x, y0], [x, y1]], 0.6, INK);
    }
    let cell = |p: &mut Page, i: usize, label: &str, big: &str, bold: bool, small: &str| {
        let x = cuts[i] + 6.0;
        let room = cuts[i + 1] - cuts[i] - 12.0;
        p.text([x, y1 - 10.0], 6.5, false, 0.0, GRAY, label);
        let size = (room / text_width(big, bold).max(1e-6)).min(if bold { 15.0 } else { 11.0 });
        p.text([x, y1 - 12.0 - 1.05 * size], size, bold, 0.0, INK, big);
        p.text(
            [x, y0 + 6.0],
            7.5,
            false,
            0.0,
            GRAY,
            &fit(small, room / 7.5),
        );
    };
    cell(p, 0, "BOARD", s.title, true, &s.what);
    cell(p, 1, "CONTENTS", &s.contents, false, &s.note);
    cell(p, 2, "DATE", s.date, false, "");
    cell(
        p,
        3,
        "SHEET",
        &format!("{} of {}", s.index, s.count),
        false,
        &format!("Made with Trace {}", env!("CARGO_PKG_VERSION")),
    );
}

/// `s` cut with "..." to at most `room` wide at size 1.
pub(crate) fn fit(s: &str, room: f64) -> String {
    if text_width(s, false) <= room {
        return s.into();
    }
    let mut t: String = s.into();
    while !t.is_empty() && text_width(&t, false) + text_width("...", false) > room {
        t.pop();
    }
    t + "..."
}

/// Board outline and every part on one side, with its title block.
fn view(board: &Board, side: Side, paper: Paper, sheet: &Sheet) -> Page {
    let (pw, ph) = paper.size();
    let mut page = Page::new(pw, ph);
    let g = &board.geometry;
    let parts = parts_on(board, side);
    let mut pts: Vec<[f64; 2]> = g.outline.iter().map(|&q| mm(q)).collect();
    if pts.len() < 3 {
        pts = g.pads.iter().map(|p| mm(p.at)).collect();
        for c in &board.components {
            pts.push([c.x.mm(), c.y.mm()]);
        }
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in &pts {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    if lo[0] > hi[0] {
        lo = [0.0; 2];
        hi = [100.0; 2];
    }
    let area = (
        MARGIN + 8.0,
        MARGIN + BLOCK + 16.0,
        pw - MARGIN - 8.0,
        ph - MARGIN - 8.0,
    );
    let (bw, bh) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let s = ((area.2 - area.0) / bw).min((area.3 - area.1) / bh);
    let ox = (area.0 + area.2) / 2.0;
    let oy = (area.1 + area.3) / 2.0;
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let flip = if side == Side::Bottom { -1.0 } else { 1.0 };
    let tf = |q: [f64; 2]| [ox + flip * s * (q[0] - cx), oy + s * (q[1] - cy)];
    let tfv = |v: &[[f64; 2]]| v.iter().map(|&q| tf(q)).collect::<Vec<_>>();

    // Frame.
    page.polygon(
        &[
            [MARGIN, MARGIN],
            [pw - MARGIN, MARGIN],
            [pw - MARGIN, ph - MARGIN],
            [MARGIN, ph - MARGIN],
        ],
        None,
        Some((1.0, INK)),
    );
    for o in std::iter::once(&g.outline).chain(&g.slots) {
        if o.len() >= 3 {
            let v: Vec<[f64; 2]> = o.iter().map(|&q| mm(q)).collect();
            page.polygon(&tfv(&v), None, Some((0.9, INK)));
        }
    }
    for a in &parts {
        page.polygon(&tfv(&a.body), Some(BODY), Some((0.4, BODY_EDGE)));
    }
    for a in &parts {
        for pad in &a.pads {
            page.polygon(&tfv(pad), Some(PAD), None);
        }
    }
    for a in &parts {
        for (line, w) in &a.silk {
            page.polyline(&tfv(line), (w * s).clamp(0.25, 1.2), SILK);
        }
        for f in &a.silk_fill {
            page.polygon(&tfv(f), Some(SILK), None);
        }
    }
    for a in &parts {
        if let Some((at, size)) = a.pin1 {
            page.dot(tf(at), (size * s * 0.3).clamp(0.7, 3.0), PIN1);
        }
    }
    for a in &parts {
        let one = text_width(&a.designator, true).max(1e-6);
        let size = (a.long * s * 0.85 / one)
            .min(a.short * s * 0.75)
            .clamp(2.5, 10.0);
        let angle = if flip < 0.0 { 180.0 - a.angle } else { a.angle };
        page.text_centered(
            tf(a.center),
            size,
            true,
            readable(angle),
            LABEL,
            &a.designator,
        );
    }
    title_block(&mut page, sheet);
    page
}

/// One line of the parts list.
struct Row {
    qty: usize,
    designators: Vec<String>,
    value: String,
    footprint: String,
    side: &'static str,
}

fn rows(board: &Board) -> Vec<Row> {
    let mut groups: BTreeMap<(String, String), (Vec<String>, BTreeSet<bool>)> = BTreeMap::new();
    for c in &board.components {
        let [value, ..] = part_info(c);
        let e = groups.entry((value, c.footprint.clone())).or_default();
        e.0.push(c.designator.clone());
        e.1.insert(c.side == Side::Top);
    }
    let mut out: Vec<Row> = groups
        .into_iter()
        .map(|((value, footprint), (mut d, sides))| {
            d.sort_by(|a, b| natural(a, b));
            Row {
                qty: d.len(),
                designators: d,
                value,
                footprint,
                side: match (sides.contains(&true), sides.contains(&false)) {
                    (true, true) => "Both",
                    (true, false) => "Top",
                    _ => "Bottom",
                },
            }
        })
        .collect();
    out.sort_by(|a, b| natural(&a.designators[0], &b.designators[0]));
    out
}

const FONT: f64 = 8.0;
const LINE: f64 = 10.5;

/// Column left edges and widths of the parts list, as fractions.
const COLS: [(&str, f64); 6] = [
    ("Item", 0.04),
    ("Qty", 0.04),
    ("Designators", 0.40),
    ("Value", 0.20),
    ("Footprint", 0.25),
    ("Side", 0.07),
];

/// Designators wrapped to `room` wide at the list's size.
fn wrap(d: &[String], room: f64) -> Vec<String> {
    let mut lines = vec![String::new()];
    for s in d {
        let last = lines.last_mut().unwrap();
        let next = if last.is_empty() {
            s.clone()
        } else {
            format!("{last}, {s}")
        };
        if !last.is_empty() && text_width(&next, false) * FONT > room {
            last.push(',');
            lines.push(s.clone());
        } else {
            *last = next;
        }
    }
    lines
}

/// The parts list split into pages: each page's rows, with their lines.
fn list_pages(rows: &[Row], paper: Paper) -> Vec<Vec<(usize, Vec<String>)>> {
    let (pw, ph) = paper.size();
    let width = pw - 2.0 * MARGIN - 16.0;
    let room = width * COLS[2].1 - 8.0;
    let top = ph - MARGIN - 40.0 - LINE * 1.4;
    let bottom = MARGIN + BLOCK + 16.0;
    let fits = ((top - bottom) / LINE).floor().max(1.0) as usize;
    let mut pages = vec![Vec::new()];
    let mut used = 0;
    for (i, r) in rows.iter().enumerate() {
        let lines = wrap(&r.designators, room);
        if used + lines.len() > fits && used > 0 {
            pages.push(Vec::new());
            used = 0;
        }
        used += lines.len();
        pages.last_mut().unwrap().push((i, lines));
    }
    pages
}

fn list(rows: &[Row], page_rows: &[(usize, Vec<String>)], paper: Paper, sheet: &Sheet) -> Page {
    let (pw, ph) = paper.size();
    let mut page = Page::new(pw, ph);
    page.polygon(
        &[
            [MARGIN, MARGIN],
            [pw - MARGIN, MARGIN],
            [pw - MARGIN, ph - MARGIN],
            [MARGIN, ph - MARGIN],
        ],
        None,
        Some((1.0, INK)),
    );
    let x0 = MARGIN + 8.0;
    let width = pw - 2.0 * MARGIN - 16.0;
    let mut xs = vec![x0];
    for (_, f) in COLS {
        xs.push(xs.last().unwrap() + f * width);
    }
    page.text([x0, ph - MARGIN - 24.0], 13.0, true, 0.0, INK, "Parts list");
    let mut y = ph - MARGIN - 40.0;
    page.polygon(
        &[
            [x0, y - LINE * 1.4],
            [x0 + width, y - LINE * 1.4],
            [x0 + width, y],
            [x0, y],
        ],
        Some([228, 232, 240]),
        None,
    );
    for (k, (name, _)) in COLS.iter().enumerate() {
        page.text([xs[k] + 4.0, y - LINE * 1.0], FONT, true, 0.0, INK, name);
    }
    y -= LINE * 1.4;
    for (n, (i, lines)) in page_rows.iter().enumerate() {
        let r = &rows[*i];
        let h = LINE * lines.len() as f64;
        if n % 2 == 1 {
            page.polygon(
                &[[x0, y - h], [x0 + width, y - h], [x0 + width, y], [x0, y]],
                Some([246, 247, 250]),
                None,
            );
        }
        let base = y - LINE + 2.8;
        let cell = |page: &mut Page, k: usize, s: &str| {
            let room = (xs[k + 1] - xs[k] - 8.0) / FONT;
            page.text([xs[k] + 4.0, base], FONT, false, 0.0, INK, &fit(s, room));
        };
        cell(&mut page, 0, &(i + 1).to_string());
        cell(&mut page, 1, &r.qty.to_string());
        for (j, l) in lines.iter().enumerate() {
            page.text(
                [xs[2] + 4.0, base - LINE * j as f64],
                FONT,
                false,
                0.0,
                INK,
                l,
            );
        }
        cell(&mut page, 3, &r.value);
        cell(&mut page, 4, &r.footprint);
        cell(&mut page, 5, r.side);
        y -= h;
        page.polyline(&[[x0, y], [x0 + width, y]], 0.3, [200, 200, 205]);
    }
    title_block(&mut page, sheet);
    page
}

/// The assembly drawing of `board` as a PDF: the top side, the bottom
/// side (seen from below, so mirrored) when it has parts, and the parts
/// list.
pub fn assembly_pdf(board: &Board, title: &str, paper: Paper) -> Vec<u8> {
    let date = today();
    let count = |side| board.components.iter().filter(|c| c.side == side).count();
    let (top, bottom) = (count(Side::Top), count(Side::Bottom));
    let mut sides = vec![Side::Top];
    if bottom > 0 {
        sides.push(Side::Bottom);
    }
    let rows = rows(board);
    let lists = if rows.is_empty() {
        Vec::new()
    } else {
        list_pages(&rows, paper)
    };
    let total = sides.len() + lists.len();
    let mut pages = Vec::new();
    for (i, &side) in sides.iter().enumerate() {
        let n = if side == Side::Top { top } else { bottom };
        let sheet = Sheet {
            title,
            what: if side == Side::Top {
                "Assembly drawing, top side".into()
            } else {
                "Assembly drawing, bottom side seen from below (mirrored)".into()
            },
            contents: format!("{n} part{}", if n == 1 { "" } else { "s" }),
            note: "Red dot: pin 1 (diodes, and parts with 3 or more pins)".into(),
            date: &date,
            index: i + 1,
            count: total,
        };
        pages.push(view(board, side, paper, &sheet));
    }
    for (k, pr) in lists.iter().enumerate() {
        let sheet = Sheet {
            title,
            what: "Assembly drawing, parts list".into(),
            contents: format!("{} parts, {} lines", board.components.len(), rows.len()),
            note: if lists.len() > 1 {
                format!("List page {} of {}", k + 1, lists.len())
            } else {
                String::new()
            },
            date: &date,
            index: sides.len() + k + 1,
            count: total,
        };
        pages.push(list(&rows, pr, paper, &sheet));
    }
    document(&pages, &format!("{title} assembly drawing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use trace_core::*;

    fn part(d: &str, x: f64, side: Side, rot: f64) -> Component {
        Component {
            designator: d.into(),
            footprint: "R0603".into(),
            library_ref: None,
            x: Length::from_mm(x),
            y: Length::from_mm(10.0),
            rotation_deg: rot,
            side,
            sheet: None,
            params: [("Value".to_string(), "10k".to_string())].into(),
        }
    }

    fn pad(c: u32, name: &str, x: f64) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: None,
            component: Some(c),
            name: name.into(),
            at: Point::mil(x / 0.0254, 10.0 / 0.0254),
            size_x: Length::from_mm(0.8),
            size_y: Length::from_mm(0.9),
            hole: Length::from_mm(0.0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    fn contents(pdf: &[u8]) -> String {
        let mut all = String::new();
        let mut at = 0;
        while let Some(i) = pdf[at..].windows(10).position(|w| w == b">>\nstream\n") {
            let s = at + i + 10;
            let e = s + pdf[s..].windows(9).position(|w| w == b"endstream").unwrap();
            let mut z = flate2::read::ZlibDecoder::new(&pdf[s..e - 1]);
            let _ = z.read_to_string(&mut all);
            at = e;
        }
        all
    }

    #[test]
    fn drawing_has_both_sides_and_the_parts_list() {
        let mut b = Board::default();
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(1000.0, 0.0),
            Point::mil(1000.0, 800.0),
            Point::mil(0.0, 800.0),
        ];
        b.components = vec![
            part("R1", 5.0, Side::Top, 0.0),
            part("R2", 10.0, Side::Top, 90.0),
            part("R10", 15.0, Side::Bottom, 0.0),
        ];
        for (i, c) in b.components.iter().enumerate() {
            let x = c.x.mm();
            for (name, d) in [("1", -0.8), ("2", 0.8)] {
                let mut p = pad(i as u32, name, x);
                let o = rot([d, 0.0], c.rotation_deg);
                p.at = Point::mil((x + o[0]) / 0.0254, (10.0 + o[1]) / 0.0254);
                b.geometry.pads.push(p);
            }
        }
        let pdf = assembly_pdf(&b, "Test board", Paper::A3);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(text.contains("/Count 3"), "top, bottom and the list");
        let c = contents(&pdf);
        for d in [
            "(R1)",
            "(R2)",
            "(R10)",
            "(R1, R2, R10)",
            "(3 of 3)",
            "(Both)",
        ] {
            assert!(c.contains(d), "{d}");
        }
        // R2 is turned, so its designator is too.
        assert!(c.contains("0 1 -1 0"));
    }

    #[test]
    fn a_long_list_runs_onto_more_pages() {
        let mut b = Board::default();
        for i in 0..200 {
            let mut c = part(&format!("C{i}"), 1.0, Side::Top, 0.0);
            c.params.insert("Value".into(), format!("{i}n"));
            b.components.push(c);
        }
        let pdf = assembly_pdf(&b, "B", Paper::A4);
        let text = String::from_utf8_lossy(&pdf).into_owned();
        let pages = text.matches("/Type /Page ").count();
        assert!(pages >= 4, "{pages}");
        assert!(contents(&pdf).contains(&format!("({pages} of {pages})")));
    }

    #[test]
    fn dates_and_angles() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_770), (2026, 11, 13));
        assert_eq!(readable(180.0), 0.0);
        assert_eq!(readable(270.0), 90.0);
        assert_eq!(readable(-90.0), 90.0);
        assert!(polar("D12") && polar("LED3") && !polar("DS1") && !polar("R4"));
    }
}
