//! Editing a schematic sheet: moving, turning and deleting parts, drawing
//! and deleting wires, placing net labels, renaming parts. Every edit keeps
//! the drawing and its nets in step, so the sheet can be drawn and probed
//! right after.

use crate::schematic::*;

/// How close two points must be to count as the same, in mil.
const TOL: f64 = 1.0;

/// The colours Altium draws signal harnesses and harness connectors in.
pub const HARNESS_LINE: Rgb = [128, 0, 128];
pub const HARNESS_FILL: Rgb = [220, 218, 255];

/// Altium's default wire colour (dark blue) and width.
pub const WIRE_COLOR: Rgb = [0, 0, 128];
const WIRE_WIDTH: f64 = 10.0;
const LABEL_COLOR: Rgb = [128, 0, 0];
const LABEL_SIZE: f64 = 100.0;

pub(crate) fn same(a: Pt, b: Pt) -> bool {
    (a[0] - b[0]).abs() <= TOL && (a[1] - b[1]).abs() <= TOL
}

/// Whether `p` lies on segment a-b away from its ends.
pub(crate) fn inside_segment(p: Pt, a: Pt, b: Pt) -> bool {
    if same(p, a) || same(p, b) {
        return false;
    }
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return false;
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2;
    if !(0.0..=1.0).contains(&t) {
        return false;
    }
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy <= TOL * TOL
}

/// Moves the ends of `line` sitting on `from` points to the matching `to`
/// points, bending the last segment so it stays square. Whether it
/// changed.
pub(crate) fn drag_line_ends(line: &mut Vec<Pt>, moves: &[(Pt, Pt)]) -> bool {
    let mut changed = false;
    for end in [false, true] {
        if line.len() < 2 {
            continue;
        }
        let k = if end { line.len() - 1 } else { 0 };
        let Some(&(_, to)) = moves.iter().find(|(a, _)| same(*a, line[k])) else {
            continue;
        };
        let next = if end { line[k - 1] } else { line[1] };
        let old = line[k];
        line[k] = to;
        // The corner that keeps the run along its old direction.
        let corner = if (next[1] - old[1]).abs() <= TOL {
            [to[0], next[1]]
        } else {
            [next[0], to[1]]
        };
        if !same(corner, to) && !same(corner, next) {
            if end {
                line.insert(k, corner);
            } else {
                line.insert(1, corner);
            }
        }
        changed = true;
    }
    changed
}

/// Drops repeated points and middle points of straight runs.
pub(crate) fn tidy_line(line: &mut Vec<Pt>) {
    line.dedup_by(|a, b| same(*a, *b));
    let mut i = 1;
    while i + 1 < line.len() {
        let (a, b, c) = (line[i - 1], line[i], line[i + 1]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        let dot = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]);
        if cross.abs() < 1e-6 && dot >= 0.0 {
            line.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Snaps a point to a grid.
pub fn snap(p: Pt, grid: f64) -> Pt {
    [(p[0] / grid).round() * grid, (p[1] / grid).round() * grid]
}

/// A move of sheet objects: a shift, a quarter turn counter-clockwise
/// about a point, or a mirror about a vertical (`MirrorX`, flips x) or
/// horizontal (`MirrorY`, flips y) line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Xf {
    Shift(Pt),
    Turn(Pt),
    MirrorX(f64),
    MirrorY(f64),
}

impl Xf {
    pub fn apply(&self, p: Pt) -> Pt {
        match *self {
            Xf::Shift(d) => [p[0] + d[0], p[1] + d[1]],
            Xf::Turn(c) => [c[0] - (p[1] - c[1]), c[1] + (p[0] - c[0])],
            Xf::MirrorX(x) => [2.0 * x - p[0], p[1]],
            Xf::MirrorY(y) => [p[0], 2.0 * y - p[1]],
        }
    }

    /// Puts a text through the move: its anchor, and its turn and
    /// justification so it still reads left to right or bottom to top
    /// and covers the mirrored place.
    pub(crate) fn text(&self, t: &mut SchText) {
        t.at = self.apply(t.at);
        let col_flip = |j: u8| j / 3 * 3 + (2 - j % 3);
        let row_flip = |j: u8| (2 - (j / 3).min(2)) * 3 + j % 3;
        match self {
            Xf::Shift(_) => {}
            Xf::Turn(_) => {
                // A half turn draws upright with the anchor on the
                // opposite corner, as Altium does.
                let r = t.rot + 1;
                (t.rot, t.just) = if r >= 2 {
                    (r - 2, 8 - t.just)
                } else {
                    (r, t.just)
                };
            }
            Xf::MirrorX(_) => {
                t.just = if t.rot.is_multiple_of(2) {
                    col_flip(t.just)
                } else {
                    row_flip(t.just)
                }
            }
            Xf::MirrorY(_) => {
                t.just = if t.rot.is_multiple_of(2) {
                    row_flip(t.just)
                } else {
                    col_flip(t.just)
                }
            }
        }
    }

    pub(crate) fn geom(&self, g: &mut SchGeom) {
        let f = |p: Pt| self.apply(p);
        match g {
            SchGeom::Line(v) | SchGeom::Poly(v) => v.iter_mut().for_each(|p| *p = f(*p)),
            SchGeom::Rect { min, max, r } => {
                let (a, b) = (f(*min), f(*max));
                *min = [a[0].min(b[0]), a[1].min(b[1])];
                *max = [a[0].max(b[0]), a[1].max(b[1])];
                if matches!(self, Xf::Turn(_)) {
                    r.swap(0, 1);
                }
            }
            SchGeom::Arc { c, r, start, end } => {
                *c = f(*c);
                let full = *end - *start >= 359.99;
                match self {
                    Xf::Shift(_) => {}
                    Xf::Turn(_) => {
                        r.swap(0, 1);
                        if !full {
                            *start += 90.0;
                            *end += 90.0;
                        }
                    }
                    Xf::MirrorX(_) if !full => {
                        (*start, *end) = (180.0 - *end, 180.0 - *start);
                    }
                    Xf::MirrorY(_) if !full => {
                        (*start, *end) = (-*end, -*start);
                    }
                    _ => {}
                }
                if !full {
                    let span = *end - *start;
                    *start = start.rem_euclid(360.0);
                    *end = *start + span;
                }
            }
        }
    }
}

impl Drawing {
    /// What the buses, bus entries, harness connectors and signal
    /// harnesses look like, drawn from their lists: shapes and texts to
    /// draw over the sheet.
    pub fn bus_harness_graphics(&self) -> (Vec<SchShape>, Vec<SchText>) {
        let line = |pts: Vec<Pt>, width: f64, c: Rgb| SchShape {
            geom: SchGeom::Line(pts),
            width,
            stroke: Some(c),
            fill: None,
            dashed: false,
            part: None,
            wire: None,
            tag: None,
        };
        let text = |at: Pt, t: &str, just: u8, c: Rgb| SchText {
            at,
            text: t.to_string(),
            size: 80.0,
            color: c,
            rot: 0,
            just,
            bold: false,
            italic: false,
            wrap: None,
            part: None,
        };
        let mut shapes = Vec::new();
        let mut texts = Vec::new();
        for b in &self.buses {
            shapes.push(line(b.clone(), 30.0, WIRE_COLOR));
        }
        for e in &self.bus_entries {
            shapes.push(line(e.to_vec(), WIRE_WIDTH, WIRE_COLOR));
        }
        for sh in &self.signal_harnesses {
            shapes.push(line(sh.clone(), 40.0, HARNESS_LINE));
        }
        for h in &self.harnesses {
            let (x0, x1, top, bot) = (h.min[0], h.max[0], h.max[1], h.min[1]);
            let y = h.max[1] - h.primary;
            let tip = 50.0_f64.min((x1 - x0) / 3.0);
            let body = if h.entries_right() {
                vec![
                    [x1, top],
                    [x0 + tip, top],
                    [x0, y],
                    [x0 + tip, bot],
                    [x1, bot],
                ]
            } else {
                vec![
                    [x0, top],
                    [x1 - tip, top],
                    [x1, y],
                    [x1 - tip, bot],
                    [x0, bot],
                ]
            };
            shapes.push(SchShape {
                geom: SchGeom::Poly(body),
                width: WIRE_WIDTH,
                stroke: Some(HARNESS_LINE),
                fill: Some(HARNESS_FILL),
                dashed: false,
                part: None,
                wire: None,
                tag: None,
            });
            for e in &h.entries {
                let at = h.entry_at(e);
                let inward = if e.side == 1 { -1.0 } else { 1.0 };
                shapes.push(line(
                    vec![at, [at[0] + 40.0 * inward, at[1]]],
                    WIRE_WIDTH,
                    HARNESS_LINE,
                ));
                let just = if e.side == 1 { 5 } else { 3 };
                texts.push(text(
                    [at[0] + 60.0 * inward, at[1]],
                    &e.name,
                    just,
                    HARNESS_LINE,
                ));
            }
            if !h.kind.is_empty() {
                texts.push(text([x0, top + 20.0], &h.kind, 0, HARNESS_LINE));
            }
        }
        (shapes, texts)
    }

    /// Puts part `i` through `xf` with its graphics, text and pins; wires
    /// that end on its pins follow.
    fn map_part(&mut self, i: usize, xf: Xf) {
        let moves = self.xf_part(i, xf);
        self.drag_ends(&moves, &[]);
        self.connect();
    }

    /// Moves part `i` without touching wires; returns where its pins went.
    pub(crate) fn xf_part(&mut self, i: usize, xf: Xf) -> Vec<(Pt, Pt)> {
        let me = Some(i as u32);
        for s in self.shapes.iter_mut().filter(|s| s.part == me) {
            xf.geom(&mut s.geom);
        }
        for t in self.texts.iter_mut().filter(|t| t.part == me) {
            xf.text(t);
        }
        let mut moves = Vec::new();
        for p in self.pins.iter_mut().filter(|p| p.part == i as u32) {
            let to = xf.apply(p.at);
            moves.push((p.at, to));
            p.at = to;
            p.base = xf.apply(p.base);
        }
        if let Some(q) = self.parts.get_mut(i) {
            let (a, b) = (xf.apply(q.min), xf.apply(q.max));
            q.min = [a[0].min(b[0]), a[1].min(b[1])];
            q.max = [a[0].max(b[0]), a[1].max(b[1])];
        }
        moves
    }

    /// Moves wire ends sitting on `from` points to the matching `to`
    /// points, bending the last segment so wires stay square.
    pub(crate) fn drag_ends(&mut self, moves: &[(Pt, Pt)], skip: &[usize]) {
        for w in 0..self.wires.len() {
            if skip.contains(&w) {
                continue;
            }
            if drag_line_ends(&mut self.wires[w], moves) {
                self.sync_wire(w);
            }
        }
    }

    /// Drags segment `k` of wire `w` (from point k to k+1) by `d`, as
    /// Altium drags a wire segment: a level or upright segment moves only
    /// across itself, its neighbours stretch to follow, and an end that
    /// was connected keeps its place with a new leg back to it. Wires that
    /// end on the segment follow it.
    pub fn drag_segment(&mut self, w: usize, k: usize, d: Pt) {
        let Some(wire) = self.wires.get(w) else {
            return;
        };
        if k + 1 >= wire.len() {
            return;
        }
        let (a, b) = (wire[k], wire[k + 1]);
        let d = if (a[1] - b[1]).abs() <= TOL {
            [0.0, d[1]]
        } else if (a[0] - b[0]).abs() <= TOL {
            [d[0], 0.0]
        } else {
            d
        };
        if d == [0.0, 0.0] {
            return;
        }
        let mv = |p: Pt| [p[0] + d[0], p[1] + d[1]];
        // Tees and wire ends on the segment go with it.
        let mut moves: Vec<(Pt, Pt)> = Vec::new();
        for (oi, o) in self.wires.iter().enumerate() {
            if oi == w {
                continue;
            }
            for &e in [o.first(), o.last()].into_iter().flatten() {
                if same(e, a) || same(e, b) || inside_segment(e, a, b) {
                    moves.push((e, mv(e)));
                }
            }
        }
        let held = |dr: &Drawing, p: Pt| {
            dr.pins.iter().any(|q| same(q.at, p))
                || dr.tags.iter().any(|t| t.at.iter().any(|&x| same(x, p)))
                || dr
                    .harnesses
                    .iter()
                    .any(|h| h.entries.iter().any(|e| same(h.entry_at(e), p)))
                || dr
                    .bus_entries
                    .iter()
                    .any(|e| same(e[0], p) || same(e[1], p))
        };
        let last = wire.len() - 1;
        let keep_start = k == 0 && held(self, a);
        let keep_end = k + 1 == last && held(self, b);
        let wire = &mut self.wires[w];
        wire[k] = mv(a);
        wire[k + 1] = mv(b);
        if keep_end {
            wire.push(b);
        }
        if keep_start {
            wire.insert(0, a);
        }
        tidy_line(wire);
        self.sync_wire(w);
        self.drag_ends(&moves, &[w]);
        let near: Vec<Pt> = [a, b, mv(a), mv(b)]
            .into_iter()
            .chain(moves.iter().flat_map(|m| [m.0, m.1]))
            .collect();
        self.fix_junctions(&near);
        self.connect();
    }

    /// Drags corner or end `k` of wire `w` by `d`: the segments either
    /// side stretch to it. Other wires ending there go along.
    pub fn drag_vertex(&mut self, w: usize, k: usize, d: Pt) {
        let Some(&p) = self.wires.get(w).and_then(|x| x.get(k)) else {
            return;
        };
        let to = [p[0] + d[0], p[1] + d[1]];
        self.wires[w][k] = to;
        tidy_line(&mut self.wires[w]);
        self.sync_wire(w);
        self.drag_ends(&[(p, to)], &[w]);
        self.fix_junctions(&[p, to]);
        self.connect();
    }

    /// Adds a bus through `pts`. Returns its index.
    pub fn add_bus(&mut self, pts: Vec<Pt>) -> Option<usize> {
        let mut pts = pts;
        pts.dedup_by(|a, b| same(*a, *b));
        if pts.len() < 2 {
            return None;
        }
        self.buses.push(pts);
        Some(self.buses.len() - 1)
    }

    /// Adds a bus entry from `on_bus` out to `to`. Returns its index.
    pub fn add_bus_entry(&mut self, on_bus: Pt, to: Pt) -> usize {
        self.bus_entries.push([on_bus, to]);
        self.connect();
        self.bus_entries.len() - 1
    }

    /// Adds a harness connector. Returns its index.
    pub fn add_harness(&mut self, h: Harness) -> usize {
        self.harnesses.push(h);
        self.connect();
        self.harnesses.len() - 1
    }

    /// Adds a signal harness through `pts`. Returns its index.
    pub fn add_signal_harness(&mut self, pts: Vec<Pt>) -> Option<usize> {
        let mut pts = pts;
        pts.dedup_by(|a, b| same(*a, *b));
        if pts.len() < 2 {
            return None;
        }
        self.signal_harnesses.push(pts);
        self.connect();
        Some(self.signal_harnesses.len() - 1)
    }

    /// Lays out a bus breakout: for each member of `name` (D[0..7]), a
    /// bus entry off bus `b` every `pitch` mil from `start` along it, a
    /// wire `len` mil on from the entry and a net label naming the
    /// member, plus a label naming the bus. Entries lean towards +x and
    /// +y unless `flip`. Returns how many members it placed.
    pub fn bus_breakout(
        &mut self,
        b: usize,
        name: &str,
        start: Pt,
        pitch: f64,
        len: f64,
        flip: bool,
    ) -> usize {
        let Some(members) = bus_members(name) else {
            return 0;
        };
        let Some(bus) = self.buses.get(b).cloned() else {
            return 0;
        };
        // The bus segment `start` is on, and its direction.
        let Some(seg) = bus
            .windows(2)
            .find(|s| inside_segment(start, s[0], s[1]) || same(start, s[0]) || same(start, s[1]))
        else {
            return 0;
        };
        let len_ab = ((seg[1][0] - seg[0][0]).powi(2) + (seg[1][1] - seg[0][1]).powi(2)).sqrt();
        if len_ab < TOL {
            return 0;
        }
        let dir = [
            (seg[1][0] - seg[0][0]) / len_ab,
            (seg[1][1] - seg[0][1]) / len_ab,
        ];
        // Out from the bus on the side `flip` picks, leaning along it.
        let s = if flip { -1.0 } else { 1.0 };
        let n = [-dir[1] * s, dir[0] * s];
        for (i, m) in members.iter().enumerate() {
            let on = [
                start[0] + dir[0] * pitch * i as f64,
                start[1] + dir[1] * pitch * i as f64,
            ];
            let e = [
                on[0] + (n[0] + dir[0]) * 100.0,
                on[1] + (n[1] + dir[1]) * 100.0,
            ];
            self.bus_entries.push([on, e]);
            let out = [e[0] + n[0] * len, e[1] + n[1] * len];
            self.add_wire(vec![e, out]);
            // On the wire a little way out, along it as Altium writes
            // labels on upright wires.
            let at = [e[0] + n[0] * 50.0, e[1] + n[1] * 50.0];
            let t = self.add_label(at, m);
            if n[0].abs() < 0.5 {
                if let Some(g) = self.tags[t]
                    .graphic
                    .and_then(|g| self.texts.get_mut(g as usize))
                {
                    g.rot = 1;
                    // Ends at the wire start on wires running down.
                    if n[1] < 0.0 {
                        g.just = 2;
                    }
                }
            }
        }
        let at = [bus[0][0], bus[0][1]];
        self.add_label(at, name);
        self.connect();
        members.len()
    }

    /// Redraws wire `w` from its points.
    pub(crate) fn sync_wire(&mut self, w: usize) {
        let pts = self.wires[w].clone();
        if let Some(s) = self.shapes.iter_mut().find(|s| s.wire == Some(w as u32)) {
            s.geom = SchGeom::Line(pts);
        }
    }

    /// Moves part `i` by `d` mil, dragging the wires on its pins.
    pub fn move_part(&mut self, i: usize, d: Pt) {
        self.map_part(i, Xf::Shift(d));
    }

    /// Turns part `i` a quarter turn counter-clockwise about the middle of
    /// its body (on a 50 mil grid, so pins stay on the grid).
    pub fn turn_part(&mut self, i: usize) {
        let Some(q) = self.parts.get(i) else { return };
        let c = snap(
            [(q.min[0] + q.max[0]) / 2.0, (q.min[1] + q.max[1]) / 2.0],
            50.0,
        );
        self.map_part(i, Xf::Turn(c));
    }

    /// Removes part `i` and everything drawn for it. Wires to its pins stay.
    pub fn delete_part(&mut self, i: usize) {
        if i >= self.parts.len() {
            return;
        }
        let me = i as u32;
        let fix = |p: &mut Option<u32>| {
            if let Some(v) = p {
                if *v > me {
                    *v -= 1;
                }
            }
        };
        // Texts first: labels point at them by index.
        let keep: Vec<bool> = self.texts.iter().map(|t| t.part != Some(me)).collect();
        self.retain_texts(&keep);
        self.shapes.retain(|s| s.part != Some(me));
        self.shapes.iter_mut().for_each(|s| fix(&mut s.part));
        self.texts.iter_mut().for_each(|t| fix(&mut t.part));
        self.pins.retain(|p| p.part != me);
        for p in &mut self.pins {
            if p.part > me {
                p.part -= 1;
            }
        }
        self.parts.remove(i);
        self.connect();
    }

    /// Keeps the texts marked true, renumbering label links.
    fn retain_texts(&mut self, keep: &[bool]) {
        let mut new_index = Vec::with_capacity(keep.len());
        let mut n = 0u32;
        for &k in keep {
            new_index.push(k.then_some(n));
            n += k as u32;
        }
        let mut it = keep.iter();
        self.texts.retain(|_| *it.next().unwrap_or(&true));
        for t in &mut self.tags {
            t.graphic = t
                .graphic
                .and_then(|g| new_index.get(g as usize).copied().flatten());
        }
    }

    /// Adds a wire through `pts`. Where its ends land on the middle of
    /// another wire, or another wire's end lands on its middle, a junction
    /// joins them. Returns the wire's index.
    pub fn add_wire(&mut self, pts: Vec<Pt>) -> Option<usize> {
        let mut pts: Vec<Pt> = pts;
        pts.dedup_by(|a, b| same(*a, *b));
        if pts.len() < 2 {
            return None;
        }
        let w = self.wires.len();
        let mut dots = Vec::new();
        for &end in [pts[0], pts[pts.len() - 1]].iter() {
            if self
                .wires
                .iter()
                .any(|o| o.windows(2).any(|s| inside_segment(end, s[0], s[1])))
            {
                dots.push(end);
            }
        }
        for o in &self.wires {
            for &end in [o[0], o[o.len() - 1]].iter() {
                if pts.windows(2).any(|s| inside_segment(end, s[0], s[1])) {
                    dots.push(end);
                }
            }
        }
        self.shapes.push(SchShape {
            geom: SchGeom::Line(pts.clone()),
            width: WIRE_WIDTH,
            stroke: Some(WIRE_COLOR),
            fill: None,
            dashed: false,
            part: None,
            wire: Some(w as u32),
            tag: None,
        });
        self.wires.push(pts);
        for d in dots {
            self.add_junction(d);
        }
        self.connect();
        Some(w)
    }

    pub(crate) fn add_junction(&mut self, at: Pt) {
        if self.junctions.iter().any(|&j| same(j, at)) {
            return;
        }
        self.junctions.push(at);
        self.shapes.push(SchShape {
            geom: SchGeom::Arc {
                c: at,
                r: [20.0, 20.0],
                start: 0.0,
                end: 360.0,
            },
            width: 0.0,
            stroke: None,
            fill: Some(WIRE_COLOR),
            dashed: false,
            part: None,
            wire: None,
            tag: None,
        });
    }

    /// Removes wire `w`, and junctions left with fewer than three wire
    /// ends or crossings about them.
    pub fn delete_wire(&mut self, w: usize) {
        if w >= self.wires.len() {
            return;
        }
        let me = w as u32;
        self.wires.remove(w);
        self.shapes.retain(|s| s.wire != Some(me));
        for s in &mut self.shapes {
            if let Some(v) = &mut s.wire {
                if *v > me {
                    *v -= 1;
                }
            }
        }
        // A junction stays while two wires still meet there.
        let wires = &self.wires;
        let touching = |j: Pt| {
            wires
                .iter()
                .filter(|o| {
                    o.windows(2)
                        .any(|s| inside_segment(j, s[0], s[1]) || same(j, s[0]) || same(j, s[1]))
                })
                .count()
        };
        let gone: Vec<Pt> = self
            .junctions
            .iter()
            .copied()
            .filter(|&j| touching(j) < 2)
            .collect();
        for j in gone {
            self.junctions.retain(|&k| !same(k, j));
            self.shapes.retain(|s| {
                !matches!(&s.geom, SchGeom::Arc { c, .. } if s.part.is_none() && s.wire.is_none() && s.stroke.is_none() && same(*c, j))
            });
        }
        self.connect();
    }

    /// Places a net label reading `text` at `at`. Returns its index.
    pub fn add_label(&mut self, at: Pt, text: &str) -> usize {
        let ti = self.texts.len() as u32;
        self.texts.push(SchText {
            at,
            text: text.to_string(),
            size: LABEL_SIZE,
            color: LABEL_COLOR,
            rot: 0,
            just: 0,
            bold: false,
            italic: false,
            wrap: None,
            part: None,
        });
        self.tags.push(NetTag {
            at: vec![at],
            text: text.to_string(),
            kind: TagKind::Label,
            graphic: Some(ti),
        });
        self.connect();
        self.tags.len() - 1
    }

    /// Changes the text of net label or port `t`.
    pub fn rename_tag(&mut self, t: usize, text: &str) {
        let Some(tag) = self.tags.get_mut(t) else {
            return;
        };
        let old = std::mem::replace(&mut tag.text, text.to_string());
        if let Some(g) = tag.graphic.and_then(|g| self.texts.get_mut(g as usize)) {
            if g.text == old {
                g.text = text.to_string();
            }
        }
        self.connect();
    }

    /// Removes tag `t` and its text, where it has one.
    pub fn delete_tag(&mut self, t: usize) {
        if t >= self.tags.len() {
            return;
        }
        let tag = self.tags.remove(t);
        // Its name goes with it, and the symbol of a port or power port
        // (sheet entries stay with their sheet symbol's drawing).
        let named = matches!(tag.kind, TagKind::Label)
            || (matches!(tag.kind, TagKind::Power | TagKind::Port)
                && self.shapes.iter().any(|s| s.tag == Some(t as u32)));
        if let Some(g) = tag.graphic.filter(|_| named) {
            let keep: Vec<bool> = (0..self.texts.len()).map(|i| i != g as usize).collect();
            self.retain_texts(&keep);
        }
        let me = t as u32;
        self.shapes.retain(|s| s.tag != Some(me));
        for s in &mut self.shapes {
            if let Some(v) = &mut s.tag {
                if *v > me {
                    *v -= 1;
                }
            }
        }
        self.connect();
    }

    /// A shift for a copy of part `i` that lands it clear of everything:
    /// no pin of the copy on a wire, pin or label, and its body over no
    /// other part. Tries right, below, left and above, further out each
    /// round, on a 100 mil grid.
    pub fn clear_spot(&self, i: usize) -> Pt {
        let Some(q) = self.parts.get(i) else {
            return [0.0, 0.0];
        };
        let (w, h) = (q.max[0] - q.min[0], q.max[1] - q.min[1]);
        let step = |v: f64| ((v + 200.0) / 100.0).ceil() * 100.0;
        let (sx, sy) = (step(w), step(h));
        let pins: Vec<Pt> = self
            .pins
            .iter()
            .filter(|p| p.part == i as u32)
            .map(|p| p.at)
            .collect();
        let clear = |d: Pt| -> bool {
            let (lo, hi) = (
                [q.min[0] + d[0] - 20.0, q.min[1] + d[1] - 20.0],
                [q.max[0] + d[0] + 20.0, q.max[1] + d[1] + 20.0],
            );
            let inside = |p: Pt| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
            if self.size[0] > 0.0
                && (lo[0] < 0.0 || lo[1] < 0.0 || hi[0] > self.size[0] || hi[1] > self.size[1])
            {
                return false;
            }
            if self.parts.iter().any(|o| {
                o.min[0] <= hi[0] && o.max[0] >= lo[0] && o.min[1] <= hi[1] && o.max[1] >= lo[1]
            }) {
                return false;
            }
            for p in &pins {
                let p = [p[0] + d[0], p[1] + d[1]];
                if self.pins.iter().any(|o| same(o.at, p))
                    || self.tags.iter().any(|t| t.at.iter().any(|&a| same(a, p)))
                    || self.wires.iter().any(|w| {
                        w.windows(2).any(|s| {
                            same(s[0], p) || same(s[1], p) || inside_segment(p, s[0], s[1])
                        })
                    })
                {
                    return false;
                }
            }
            // Nor over a note or text frame (filled boxes of the sheet).
            let sheet_area = self.size[0] * self.size[1];
            if self.shapes.iter().any(|sh| {
                let SchGeom::Rect { min, max, .. } = &sh.geom else {
                    return false;
                };
                sh.part.is_none()
                    && sh.fill.is_some()
                    && (max[0] - min[0]) * (max[1] - min[1]) < sheet_area / 4.0
                    && min[0] <= hi[0]
                    && max[0] >= lo[0]
                    && min[1] <= hi[1]
                    && max[1] >= lo[1]
            }) {
                return false;
            }
            // No wire runs through the body.
            !self.wires.iter().any(|w| w.iter().any(|&p| inside(p)))
        };
        for k in 1..=12 {
            let k = k as f64;
            for d in [[sx * k, 0.0], [0.0, -sy * k], [-sx * k, 0.0], [0.0, sy * k]] {
                if clear(d) {
                    return d;
                }
            }
        }
        [sx, 0.0]
    }

    /// Adds a copy of part `i` moved by `d` mil and named `designator`,
    /// with its graphics, text and pins (no wires). Returns its index.
    pub fn copy_part(&mut self, i: usize, d: Pt, designator: &str) -> Option<usize> {
        let q = self.parts.get(i)?.clone();
        let from = q.designator.clone();
        let pieces = self.pieces(i);
        Some(self.paste(pieces, q, d, &from, designator))
    }

    /// Adds part `i` of another drawing (a library symbol), `d` mil from
    /// where it is there, as part `designator`. A designator drawn with a
    /// part letter (U?A) keeps its letter.
    pub fn paste_part(
        &mut self,
        src: &Drawing,
        i: usize,
        d: Pt,
        designator: &str,
    ) -> Option<usize> {
        let q = src.parts.get(i)?.clone();
        let from = q.designator.clone();
        Some(self.paste(src.pieces(i), q, d, &from, designator))
    }

    /// The shapes, texts and pins of part `i`.
    fn pieces(&self, i: usize) -> (Vec<SchShape>, Vec<SchText>, Vec<SchPin>) {
        let me = Some(i as u32);
        (
            self.shapes
                .iter()
                .filter(|s| s.part == me)
                .cloned()
                .collect(),
            self.texts
                .iter()
                .filter(|t| t.part == me)
                .cloned()
                .collect(),
            self.pins
                .iter()
                .filter(|p| p.part == i as u32)
                .cloned()
                .collect(),
        )
    }

    fn paste(
        &mut self,
        (shapes, texts, pins): (Vec<SchShape>, Vec<SchText>, Vec<SchPin>),
        q: SchPart,
        d: Pt,
        from: &str,
        designator: &str,
    ) -> usize {
        let new = self.parts.len() as u32;
        let f = |p: Pt| [p[0] + d[0], p[1] + d[1]];
        self.shapes.extend(shapes.into_iter().map(|mut s| {
            Xf::Shift(d).geom(&mut s.geom);
            s.part = Some(new);
            s
        }));
        self.texts.extend(texts.into_iter().map(|mut t| {
            t.at = f(t.at);
            t.part = Some(new);
            if t.text == from {
                t.text = designator.to_string();
            } else if let Some(letter) = t
                .text
                .strip_prefix(from)
                .filter(|l| l.len() == 1 && l.chars().all(|c| c.is_ascii_uppercase()))
            {
                t.text = format!("{designator}{letter}");
            }
            t
        }));
        self.pins.extend(pins.into_iter().map(|mut p| {
            p.at = f(p.at);
            p.base = f(p.base);
            p.part = new;
            p
        }));
        self.parts.push(SchPart {
            designator: designator.to_string(),
            min: f(q.min),
            max: f(q.max),
            ..q
        });
        self.connect();
        new as usize
    }

    /// Renames part `i` and the designator drawn for it.
    pub fn rename_part(&mut self, i: usize, designator: &str) {
        let Some(q) = self.parts.get_mut(i) else {
            return;
        };
        let old = std::mem::replace(&mut q.designator, designator.to_string());
        for t in self.texts.iter_mut().filter(|t| t.part == Some(i as u32)) {
            if t.text == old {
                t.text = designator.to_string();
            } else if let Some(letter) = t
                .text
                .strip_prefix(old.as_str())
                .filter(|l| l.len() == 1 && l.chars().all(|c| c.is_ascii_uppercase()))
            {
                t.text = format!("{designator}{letter}");
            }
        }
    }
}

impl Sheet {
    /// Adds a copy of part `i` as a new part `designator`, `d` mil away,
    /// with its own symbol entry (footprint and value kept).
    pub fn copy_part(&mut self, i: usize, d: Pt, designator: &str) -> Option<usize> {
        let sym = self.drawing.parts.get(i)?.symbol;
        let mut s = self.symbols.get(sym).cloned().unwrap_or_default();
        s.designator = designator.to_string();
        self.symbols.push(s);
        let n = self.drawing.copy_part(i, d, designator)?;
        self.drawing.parts[n].symbol = self.symbols.len() - 1;
        Some(n)
    }

    /// Places a library symbol with its origin at `at`, as part
    /// `designator`, with its own symbol entry.
    pub fn place_symbol(&mut self, lib: &LibSymbol, at: Pt, designator: &str) -> Option<usize> {
        let mut s = lib.symbol.clone();
        s.designator = designator.to_string();
        if s.library_ref.is_empty() {
            s.library_ref = lib.name.clone();
        }
        self.symbols.push(s);
        let n = self.drawing.paste_part(&lib.drawing, 0, at, designator)?;
        self.drawing.parts[n].symbol = self.symbols.len() - 1;
        self.drawing.parts[n].gate = if lib.parts > 1 { lib.part } else { 0 };
        Some(n)
    }

    /// Sets the value shown for part `i` (its comment, in Altium).
    pub fn set_value(&mut self, i: usize, value: &str) {
        let Some(q) = self.drawing.parts.get(i) else {
            return;
        };
        let Some(sym) = self.symbols.get_mut(q.symbol) else {
            return;
        };
        let old = sym.value.replace(value.to_string());
        if let Some(old) = old.filter(|o| !o.is_empty()) {
            for t in self
                .drawing
                .texts
                .iter_mut()
                .filter(|t| t.part == Some(i as u32) && t.text == old)
            {
                t.text = value.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-pin part at the origin, pins sticking out left and right.
    fn sheet() -> Drawing {
        let mut d = Drawing {
            parts: vec![SchPart {
                designator: "R1".into(),
                symbol: 0,
                min: [-100.0, -50.0],
                max: [100.0, 50.0],
                ..Default::default()
            }],
            pins: vec![
                SchPin {
                    at: [-200.0, 0.0],
                    base: [-100.0, 0.0],
                    part: 0,
                    name: String::new(),
                    number: "1".into(),
                    ..Default::default()
                },
                SchPin {
                    at: [200.0, 0.0],
                    base: [100.0, 0.0],
                    part: 0,
                    name: String::new(),
                    number: "2".into(),
                    ..Default::default()
                },
            ],
            texts: vec![SchText {
                at: [0.0, 60.0],
                text: "R1".into(),
                size: 100.0,
                color: [0, 0, 0],
                rot: 0,
                just: 0,
                bold: false,
                italic: false,
                wrap: None,
                part: Some(0),
            }],
            ..Default::default()
        };
        d.add_wire(vec![[200.0, 0.0], [500.0, 0.0]]);
        d
    }

    #[test]
    fn wires_follow_a_moved_part() {
        let mut d = sheet();
        d.move_part(0, [0.0, 100.0]);
        assert_eq!(d.pins[1].at, [200.0, 100.0]);
        // The wire bends down to where it was.
        assert_eq!(d.wires[0].first(), Some(&[200.0, 100.0]));
        assert_eq!(d.wires[0].last(), Some(&[500.0, 0.0]));
        assert!(d.wires[0]
            .windows(2)
            .all(|s| s[0][0] == s[1][0] || s[0][1] == s[1][1]));
        let net = d.nets.iter().find(|n| n.wires.contains(&0)).unwrap();
        assert_eq!(net.pins, vec![1]);
    }

    #[test]
    fn turning_keeps_pins_on_the_grid() {
        let mut d = sheet();
        d.turn_part(0);
        assert_eq!(d.pins[0].at, [0.0, -200.0]);
        assert_eq!(d.pins[1].at, [0.0, 200.0]);
        assert_eq!(d.texts[0].rot, 1);
        assert_eq!(d.wires[0].first(), Some(&[0.0, 200.0]));
    }

    #[test]
    fn junctions_labels_and_deletes() {
        let mut d = sheet();
        // A tee onto the middle of the wire gets a junction.
        let w = d.add_wire(vec![[350.0, 0.0], [350.0, 300.0]]).unwrap();
        assert_eq!(d.junctions, vec![[350.0, 0.0]]);
        let l = d.add_label([350.0, 300.0], "SIG");
        let net = d
            .nets
            .iter()
            .find(|n| n.tags.contains(&(l as u32)))
            .unwrap();
        assert_eq!(net.name.as_deref(), Some("SIG"));
        assert_eq!(net.pins, vec![1]);
        d.rename_tag(l, "OUT");
        assert_eq!(d.texts[d.tags[l].graphic.unwrap() as usize].text, "OUT");
        d.delete_wire(w);
        assert!(d.junctions.is_empty());
        assert!(!d
            .shapes
            .iter()
            .any(|s| matches!(s.geom, SchGeom::Arc { .. })));
        d.rename_part(0, "R7");
        assert_eq!(d.texts[0].text, "R7");
        d.delete_part(0);
        assert!(d.pins.is_empty() && d.parts.is_empty());
        // The label's text survives the part's text going.
        assert_eq!(d.texts.len(), 1);
        assert_eq!(d.tags[0].graphic, Some(0));
        d.delete_tag(0);
        assert!(d.texts.is_empty() && d.tags.is_empty());
    }
}
