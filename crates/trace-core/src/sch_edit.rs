//! Editing a schematic sheet: moving, turning and deleting parts, drawing
//! and deleting wires, placing net labels, renaming parts. Every edit keeps
//! the drawing and its nets in step, so the sheet can be drawn and probed
//! right after.

use crate::schematic::*;

/// How close two points must be to count as the same, in mil.
const TOL: f64 = 0.6;

/// Altium's default wire colour (dark blue) and width.
pub const WIRE_COLOR: Rgb = [0, 0, 128];
const WIRE_WIDTH: f64 = 10.0;
const LABEL_COLOR: Rgb = [128, 0, 0];
const LABEL_SIZE: f64 = 100.0;

fn same(a: Pt, b: Pt) -> bool {
    (a[0] - b[0]).abs() <= TOL && (a[1] - b[1]).abs() <= TOL
}

/// Whether `p` lies on segment a-b away from its ends.
fn inside_segment(p: Pt, a: Pt, b: Pt) -> bool {
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

/// Snaps a point to a grid.
pub fn snap(p: Pt, grid: f64) -> Pt {
    [(p[0] / grid).round() * grid, (p[1] / grid).round() * grid]
}

fn map_geom(g: &mut SchGeom, f: &dyn Fn(Pt) -> Pt, turn: bool) {
    match g {
        SchGeom::Line(v) | SchGeom::Poly(v) => v.iter_mut().for_each(|p| *p = f(*p)),
        SchGeom::Rect { min, max, r } => {
            let (a, b) = (f(*min), f(*max));
            *min = [a[0].min(b[0]), a[1].min(b[1])];
            *max = [a[0].max(b[0]), a[1].max(b[1])];
            if turn {
                r.swap(0, 1);
            }
        }
        SchGeom::Arc { c, r, start, end } => {
            *c = f(*c);
            if turn {
                r.swap(0, 1);
                *start += 90.0;
                *end += 90.0;
            }
        }
    }
}

impl Drawing {
    /// Puts part `i` through `f` (a shift, or a quarter turn when `turn`),
    /// with its graphics, text and pins; wires that end on its pins follow.
    fn map_part(&mut self, i: usize, f: &dyn Fn(Pt) -> Pt, turn: bool) {
        let me = Some(i as u32);
        for s in self.shapes.iter_mut().filter(|s| s.part == me) {
            map_geom(&mut s.geom, f, turn);
        }
        for t in self.texts.iter_mut().filter(|t| t.part == me) {
            t.at = f(t.at);
            if turn {
                // Text stays readable: a half turn draws upright with the
                // anchor on the opposite corner, as Altium does.
                let r = t.rot + 1;
                (t.rot, t.just) = if r >= 2 {
                    (r - 2, 8 - t.just)
                } else {
                    (r, t.just)
                };
            }
        }
        let mut moves = Vec::new();
        for p in self.pins.iter_mut().filter(|p| p.part == i as u32) {
            let to = f(p.at);
            moves.push((p.at, to));
            p.at = to;
            p.base = f(p.base);
        }
        if let Some(q) = self.parts.get_mut(i) {
            let (a, b) = (f(q.min), f(q.max));
            q.min = [a[0].min(b[0]), a[1].min(b[1])];
            q.max = [a[0].max(b[0]), a[1].max(b[1])];
        }
        self.drag_ends(&moves);
        self.connect();
    }

    /// Moves wire ends sitting on `from` points to the matching `to`
    /// points, bending the last segment so wires stay square.
    fn drag_ends(&mut self, moves: &[(Pt, Pt)]) {
        for w in 0..self.wires.len() {
            let mut changed = false;
            for end in [false, true] {
                let wire = &mut self.wires[w];
                if wire.len() < 2 {
                    continue;
                }
                let k = if end { wire.len() - 1 } else { 0 };
                let Some(&(_, to)) = moves.iter().find(|(a, _)| same(*a, wire[k])) else {
                    continue;
                };
                let next = if end { wire[k - 1] } else { wire[1] };
                let old = wire[k];
                wire[k] = to;
                // The corner that keeps the run along its old direction.
                let corner = if (next[1] - old[1]).abs() <= TOL {
                    [to[0], next[1]]
                } else {
                    [next[0], to[1]]
                };
                if !same(corner, to) && !same(corner, next) {
                    if end {
                        wire.insert(k, corner);
                    } else {
                        wire.insert(1, corner);
                    }
                }
                changed = true;
            }
            if changed {
                self.sync_wire(w);
            }
        }
    }

    /// Redraws wire `w` from its points.
    fn sync_wire(&mut self, w: usize) {
        let pts = self.wires[w].clone();
        if let Some(s) = self.shapes.iter_mut().find(|s| s.wire == Some(w as u32)) {
            s.geom = SchGeom::Line(pts);
        }
    }

    /// Moves part `i` by `d` mil, dragging the wires on its pins.
    pub fn move_part(&mut self, i: usize, d: Pt) {
        self.map_part(i, &|p| [p[0] + d[0], p[1] + d[1]], false);
    }

    /// Turns part `i` a quarter turn counter-clockwise about the middle of
    /// its body (on a 50 mil grid, so pins stay on the grid).
    pub fn turn_part(&mut self, i: usize) {
        let Some(q) = self.parts.get(i) else { return };
        let c = snap(
            [(q.min[0] + q.max[0]) / 2.0, (q.min[1] + q.max[1]) / 2.0],
            50.0,
        );
        self.map_part(i, &|p| [c[0] - (p[1] - c[1]), c[1] + (p[0] - c[0])], true);
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
        });
        self.wires.push(pts);
        for d in dots {
            self.add_junction(d);
        }
        self.connect();
        Some(w)
    }

    fn add_junction(&mut self, at: Pt) {
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
        // A port or power port keeps its drawing; a label is only its text.
        if let Some(g) = tag.graphic.filter(|_| tag.kind == TagKind::Label) {
            let keep: Vec<bool> = (0..self.texts.len()).map(|i| i != g as usize).collect();
            self.retain_texts(&keep);
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
            map_geom(&mut s.geom, &f, false);
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
        for t in self
            .texts
            .iter_mut()
            .filter(|t| t.part == Some(i as u32) && t.text == old)
        {
            t.text = designator.to_string();
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
            }],
            pins: vec![
                SchPin {
                    at: [-200.0, 0.0],
                    base: [-100.0, 0.0],
                    part: 0,
                    name: String::new(),
                    number: "1".into(),
                },
                SchPin {
                    at: [200.0, 0.0],
                    base: [100.0, 0.0],
                    part: 0,
                    name: String::new(),
                    number: "2".into(),
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
