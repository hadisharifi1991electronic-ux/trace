//! Working on many things of a sheet at once, as Altium's schematic editor
//! does: picking by box, moving, turning, mirroring and deleting a
//! selection, copying it to a clipboard sheet and pasting it back with new
//! designators. Also the power ports and No ERC markers placed by hand.

use crate::sch_edit::{inside_segment, same, Xf};
use crate::schematic::*;
use std::collections::HashMap;

/// Red, as Altium draws No ERC markers.
const NO_ERC_COLOR: Rgb = [255, 0, 0];

/// Objects of one sheet, by index into the drawing's lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchSel {
    pub parts: Vec<usize>,
    pub wires: Vec<usize>,
    pub tags: Vec<usize>,
    pub junctions: Vec<usize>,
    pub no_erc: Vec<usize>,
    pub buses: Vec<usize>,
    pub bus_entries: Vec<usize>,
    pub harnesses: Vec<usize>,
    pub signal_harnesses: Vec<usize>,
}

impl SchSel {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.parts.len()
            + self.wires.len()
            + self.tags.len()
            + self.junctions.len()
            + self.no_erc.len()
            + self.buses.len()
            + self.bus_entries.len()
            + self.harnesses.len()
            + self.signal_harnesses.len()
    }

    pub fn part(i: usize) -> SchSel {
        SchSel {
            parts: vec![i],
            ..Default::default()
        }
    }

    pub fn wire(i: usize) -> SchSel {
        SchSel {
            wires: vec![i],
            ..Default::default()
        }
    }

    pub fn tag(i: usize) -> SchSel {
        SchSel {
            tags: vec![i],
            ..Default::default()
        }
    }

    /// Adds what `other` holds, or takes it away where it is already in.
    pub fn toggle(&mut self, other: &SchSel) {
        fn flip(v: &mut Vec<usize>, o: &[usize]) {
            for &i in o {
                match v.iter().position(|&x| x == i) {
                    Some(k) => {
                        v.remove(k);
                    }
                    None => v.push(i),
                }
            }
        }
        flip(&mut self.parts, &other.parts);
        flip(&mut self.wires, &other.wires);
        flip(&mut self.tags, &other.tags);
        flip(&mut self.junctions, &other.junctions);
        flip(&mut self.no_erc, &other.no_erc);
        flip(&mut self.buses, &other.buses);
        flip(&mut self.bus_entries, &other.bus_entries);
        flip(&mut self.harnesses, &other.harnesses);
        flip(&mut self.signal_harnesses, &other.signal_harnesses);
    }

    /// Adds what `other` holds.
    pub fn add(&mut self, other: &SchSel) {
        fn join(v: &mut Vec<usize>, o: &[usize]) {
            for &i in o {
                if !v.contains(&i) {
                    v.push(i);
                }
            }
        }
        join(&mut self.parts, &other.parts);
        join(&mut self.wires, &other.wires);
        join(&mut self.tags, &other.tags);
        join(&mut self.junctions, &other.junctions);
        join(&mut self.no_erc, &other.no_erc);
        join(&mut self.buses, &other.buses);
        join(&mut self.bus_entries, &other.bus_entries);
        join(&mut self.harnesses, &other.harnesses);
        join(&mut self.signal_harnesses, &other.signal_harnesses);
    }

    /// Whether everything `other` holds is in.
    pub fn holds(&self, other: &SchSel) -> bool {
        other.parts.iter().all(|i| self.parts.contains(i))
            && other.wires.iter().all(|i| self.wires.contains(i))
            && other.tags.iter().all(|i| self.tags.contains(i))
            && other.junctions.iter().all(|i| self.junctions.contains(i))
            && other.no_erc.iter().all(|i| self.no_erc.contains(i))
            && other.buses.iter().all(|i| self.buses.contains(i))
            && other
                .bus_entries
                .iter()
                .all(|i| self.bus_entries.contains(i))
            && other.harnesses.iter().all(|i| self.harnesses.contains(i))
            && other
                .signal_harnesses
                .iter()
                .all(|i| self.signal_harnesses.contains(i))
    }

    /// Drops indices the drawing no longer has.
    pub fn clamp(&mut self, d: &Drawing) {
        self.parts.retain(|&i| i < d.parts.len());
        self.wires.retain(|&i| i < d.wires.len());
        self.tags.retain(|&i| i < d.tags.len());
        self.junctions.retain(|&i| i < d.junctions.len());
        self.no_erc.retain(|&i| i < d.no_erc.len());
        self.buses.retain(|&i| i < d.buses.len());
        self.bus_entries.retain(|&i| i < d.bus_entries.len());
        self.harnesses.retain(|&i| i < d.harnesses.len());
        self.signal_harnesses
            .retain(|&i| i < d.signal_harnesses.len());
    }
}

/// Whether segment a-b crosses the box lo-hi (Liang-Barsky clipping).
fn segment_meets_box(a: Pt, b: Pt, lo: Pt, hi: Pt) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, a[0] - lo[0]),
        (dx, hi[0] - a[0]),
        (-dy, a[1] - lo[1]),
        (dy, hi[1] - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// The two red strokes of a No ERC marker at `at`, as the Altium reader
/// draws them.
pub fn no_erc_drawing(at: Pt) -> Drawing {
    let mut d = Drawing::default();
    d.no_erc.push(at);
    for (dx, dy) in [(1.0, 1.0), (1.0, -1.0)] {
        d.shapes.push(SchShape {
            geom: SchGeom::Line(vec![
                [at[0] - 30.0 * dx, at[1] - 30.0 * dy],
                [at[0] + 30.0 * dx, at[1] + 30.0 * dy],
            ]),
            width: 10.0,
            stroke: Some(NO_ERC_COLOR),
            fill: None,
            dashed: false,
            part: None,
            wire: None,
            tag: None,
        });
    }
    d
}

fn free(s: &SchShape) -> bool {
    s.part.is_none() && s.wire.is_none() && s.tag.is_none()
}

/// Whether shape `s` is the dot of a junction at `j`.
fn is_dot(s: &SchShape, j: Pt) -> bool {
    free(s)
        && s.stroke.is_none()
        && matches!(&s.geom, SchGeom::Arc { c, r, .. } if same(*c, j) && (r[0] - 20.0).abs() < 1.0)
}

/// Whether shape `s` is a stroke of a No ERC marker at `m`.
fn is_cross(s: &SchShape, m: Pt) -> bool {
    free(s)
        && matches!(&s.geom, SchGeom::Line(v) if v.len() == 2
            && same([(v[0][0] + v[1][0]) / 2.0, (v[0][1] + v[1][1]) / 2.0], m)
            && ((v[0][0] - v[1][0]).abs() - (v[0][1] - v[1][1]).abs()).abs() < 1.0)
}

impl Drawing {
    /// What lies in the box a-b: wholly inside, or (`touching`) anything
    /// that reaches into it. Sheet entries go with their sheet symbol and
    /// are not picked.
    pub fn select_box(&self, a: Pt, b: Pt, touching: bool) -> SchSel {
        let lo = [a[0].min(b[0]), a[1].min(b[1])];
        let hi = [a[0].max(b[0]), a[1].max(b[1])];
        let inside = |p: &Pt| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
        let mut sel = SchSel::default();
        for (i, q) in self.parts.iter().enumerate() {
            let hit = if touching {
                q.min[0] <= hi[0] && q.max[0] >= lo[0] && q.min[1] <= hi[1] && q.max[1] >= lo[1]
            } else {
                inside(&q.min) && inside(&q.max)
            };
            if hit {
                sel.parts.push(i);
            }
        }
        for (i, w) in self.wires.iter().enumerate() {
            let hit = if touching {
                w.windows(2).any(|s| segment_meets_box(s[0], s[1], lo, hi))
            } else {
                w.iter().all(inside)
            };
            if hit {
                sel.wires.push(i);
            }
        }
        for (i, t) in self.tags.iter().enumerate() {
            if t.kind == TagKind::SheetEntry || t.at.is_empty() {
                continue;
            }
            let hit = if touching {
                t.at.iter().any(inside)
            } else {
                t.at.iter().all(inside)
            };
            if hit {
                sel.tags.push(i);
            }
        }
        sel.junctions = (0..self.junctions.len())
            .filter(|&i| inside(&self.junctions[i]))
            .collect();
        sel.no_erc = (0..self.no_erc.len())
            .filter(|&i| inside(&self.no_erc[i]))
            .collect();
        let line_hit = |w: &[Pt]| {
            if touching {
                w.windows(2).any(|s| segment_meets_box(s[0], s[1], lo, hi))
            } else {
                w.iter().all(inside)
            }
        };
        sel.buses = (0..self.buses.len())
            .filter(|&i| line_hit(&self.buses[i]))
            .collect();
        sel.bus_entries = (0..self.bus_entries.len())
            .filter(|&i| line_hit(&self.bus_entries[i]))
            .collect();
        sel.signal_harnesses = (0..self.signal_harnesses.len())
            .filter(|&i| line_hit(&self.signal_harnesses[i]))
            .collect();
        sel.harnesses = (0..self.harnesses.len())
            .filter(|&i| {
                let h = &self.harnesses[i];
                if touching {
                    h.min[0] <= hi[0] && h.max[0] >= lo[0] && h.min[1] <= hi[1] && h.max[1] >= lo[1]
                } else {
                    inside(&h.min) && inside(&h.max)
                }
            })
            .collect();
        sel
    }

    /// Everything that can be selected.
    pub fn select_all(&self) -> SchSel {
        SchSel {
            parts: (0..self.parts.len()).collect(),
            wires: (0..self.wires.len()).collect(),
            tags: (0..self.tags.len())
                .filter(|&i| self.tags[i].kind != TagKind::SheetEntry)
                .collect(),
            junctions: (0..self.junctions.len()).collect(),
            no_erc: (0..self.no_erc.len()).collect(),
            buses: (0..self.buses.len()).collect(),
            bus_entries: (0..self.bus_entries.len()).collect(),
            harnesses: (0..self.harnesses.len()).collect(),
            signal_harnesses: (0..self.signal_harnesses.len()).collect(),
        }
    }

    /// The box around a selection.
    pub fn sel_bounds(&self, sel: &SchSel) -> Option<(Pt, Pt)> {
        let mut pts: Vec<Pt> = Vec::new();
        for &i in &sel.parts {
            if let Some(q) = self.parts.get(i) {
                pts.push(q.min);
                pts.push(q.max);
            }
            pts.extend(
                self.pins
                    .iter()
                    .filter(|p| p.part == i as u32)
                    .map(|p| p.at),
            );
        }
        for &w in &sel.wires {
            pts.extend(self.wires.get(w).into_iter().flatten().copied());
        }
        for &t in &sel.tags {
            pts.extend(
                self.tags
                    .get(t)
                    .into_iter()
                    .flat_map(|t| t.at.iter().copied()),
            );
        }
        pts.extend(
            sel.junctions
                .iter()
                .filter_map(|&j| self.junctions.get(j).copied()),
        );
        pts.extend(
            sel.no_erc
                .iter()
                .filter_map(|&j| self.no_erc.get(j).copied()),
        );
        for &b in &sel.buses {
            pts.extend(self.buses.get(b).into_iter().flatten().copied());
        }
        for &b in &sel.bus_entries {
            pts.extend(self.bus_entries.get(b).into_iter().flatten().copied());
        }
        for &b in &sel.signal_harnesses {
            pts.extend(self.signal_harnesses.get(b).into_iter().flatten().copied());
        }
        for &h in &sel.harnesses {
            if let Some(h) = self.harnesses.get(h) {
                pts.push(h.min);
                pts.push(h.max);
            }
        }
        let first = *pts.first()?;
        Some(pts.iter().fold((first, first), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        }))
    }

    /// The middle of a selection on a 50 mil grid, which turns and
    /// mirrors go about, so pins stay on the grid.
    pub fn sel_centre(&self, sel: &SchSel) -> Option<Pt> {
        let (lo, hi) = self.sel_bounds(sel)?;
        Some(crate::sch_edit::snap(
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
            50.0,
        ))
    }

    /// Moves, turns or mirrors a selection, dragging: wires that are not
    /// selected but end on what moves stretch to follow, bending to stay
    /// square.
    pub fn xf_sel(&mut self, sel: &SchSel, xf: Xf) {
        self.xf_sel_mode(sel, xf, true);
    }

    /// Moves, turns or mirrors a selection. With `drag` the wiring stays
    /// connected (Altium's Drag); without, only the selection moves and
    /// its connections break where they leave it (Altium's Move).
    pub fn xf_sel_mode(&mut self, sel: &SchSel, xf: Xf, drag: bool) {
        let mut moves: Vec<(Pt, Pt)> = Vec::new();
        for &i in &sel.parts {
            if i < self.parts.len() {
                moves.extend(self.xf_part(i, xf));
            }
        }
        // A selected wire's end on the pin of a part that stays keeps
        // hold of it: the wire bends back to the pin.
        let stays = |d: &Drawing, p: Pt| {
            drag && d
                .pins
                .iter()
                .any(|q| !sel.parts.contains(&(q.part as usize)) && same(q.at, p))
        };
        let mut back: Vec<(Pt, Pt)> = Vec::new();
        let mut held: Vec<usize> = Vec::new();
        for &w in &sel.wires {
            let Some(wire) = self.wires.get(w) else {
                continue;
            };
            let (a, b) = (wire[0], wire[wire.len() - 1]);
            for e in [a, b] {
                if stays(self, e) {
                    back.push((xf.apply(e), e));
                    held.push(w);
                } else {
                    moves.push((e, xf.apply(e)));
                }
            }
            let wire = &mut self.wires[w];
            wire.iter_mut().for_each(|p| *p = xf.apply(*p));
            self.sync_wire(w);
        }
        if !back.is_empty() {
            let others: Vec<usize> = (0..self.wires.len())
                .filter(|w| !held.contains(w))
                .collect();
            self.drag_ends(&back, &others);
        }
        for &t in &sel.tags {
            let Some(tag) = self.tags.get(t) else {
                continue;
            };
            // Ports keep their shape and only shift with the rest.
            let txf = match (tag.kind, xf) {
                (TagKind::Port, Xf::Turn(_) | Xf::MirrorX(_) | Xf::MirrorY(_)) => {
                    let a = tag.at[0];
                    let b = xf.apply(a);
                    Xf::Shift([b[0] - a[0], b[1] - a[1]])
                }
                _ => xf,
            };
            let kind = tag.kind;
            let graphic = tag.graphic.map(|g| g as usize);
            for a in &mut self.tags[t].at {
                let to = txf.apply(*a);
                moves.push((*a, to));
                *a = to;
            }
            for s in self.shapes.iter_mut().filter(|s| s.tag == Some(t as u32)) {
                txf.geom(&mut s.geom);
            }
            if let Some(g) = graphic.and_then(|g| self.texts.get_mut(g)) {
                if g.part.is_none() {
                    txf.text(g);
                }
            }
            if kind == TagKind::Power && !matches!(txf, Xf::Shift(_)) {
                self.upright_power_text(t);
            }
        }
        for &j in &sel.junctions {
            let Some(&at) = self.junctions.get(j) else {
                continue;
            };
            let to = xf.apply(at);
            if let Some(s) = self.shapes.iter_mut().find(|s| is_dot(s, at)) {
                xf.geom(&mut s.geom);
            }
            self.junctions[j] = to;
            moves.push((at, to));
        }
        for &m in &sel.no_erc {
            let Some(&at) = self.no_erc.get(m) else {
                continue;
            };
            let to = xf.apply(at);
            for s in self.shapes.iter_mut().filter(|s| is_cross(s, at)) {
                Xf::Shift([to[0] - at[0], to[1] - at[1]]).geom(&mut s.geom);
            }
            self.no_erc[m] = to;
        }
        // Buses, bus entries and signal harnesses move whole.
        for &b in &sel.buses {
            if let Some(line) = self.buses.get_mut(b) {
                line.iter_mut().for_each(|p| *p = xf.apply(*p));
            }
        }
        for &b in &sel.bus_entries {
            if let Some(e) = self.bus_entries.get_mut(b) {
                e.iter_mut().for_each(|p| *p = xf.apply(*p));
            }
        }
        for &b in &sel.signal_harnesses {
            if let Some(line) = self.signal_harnesses.get_mut(b) {
                for e in [0, line.len().saturating_sub(1)] {
                    moves.push((line[e], xf.apply(line[e])));
                }
                line.iter_mut().for_each(|p| *p = xf.apply(*p));
            }
        }
        // Harness connectors keep reading upright, as ports do: a turn
        // only carries them round; mirrors swap their sides.
        for &h in &sel.harnesses {
            let Some(c) = self.harnesses.get(h).cloned() else {
                continue;
            };
            let mut n = c.clone();
            let height = c.max[1] - c.min[1];
            let (a, b) = (xf.apply(c.min), xf.apply(c.max));
            let (lo, hi) = (
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
            );
            match xf {
                Xf::Turn(_) => {
                    let mid = xf.apply([(c.min[0] + c.max[0]) / 2.0, (c.min[1] + c.max[1]) / 2.0]);
                    let (w2, h2) = ((c.max[0] - c.min[0]) / 2.0, height / 2.0);
                    n.min = [mid[0] - w2, mid[1] - h2];
                    n.max = [mid[0] + w2, mid[1] + h2];
                }
                Xf::MirrorX(_) => {
                    (n.min, n.max) = (lo, hi);
                    n.entries
                        .iter_mut()
                        .for_each(|e| e.side = 1 - e.side.min(1));
                }
                Xf::MirrorY(_) => {
                    (n.min, n.max) = (lo, hi);
                    n.entries.iter_mut().for_each(|e| e.dist = height - e.dist);
                    n.primary = height - c.primary;
                }
                Xf::Shift(_) => (n.min, n.max) = (lo, hi),
            }
            for (e, ne) in c.entries.iter().zip(&n.entries) {
                moves.push((c.entry_at(e), n.entry_at(ne)));
            }
            moves.push((c.bundle_at(), n.bundle_at()));
            self.harnesses[h] = n;
        }
        if drag {
            self.drag_ends(&moves, &sel.wires);
            for (i, line) in self.signal_harnesses.iter_mut().enumerate() {
                if !sel.signal_harnesses.contains(&i) {
                    crate::sch_edit::drag_line_ends(line, &moves);
                }
            }
        }
        let mut near: Vec<Pt> = moves.iter().flat_map(|&(a, b)| [a, b]).collect();
        near.dedup_by(|a, b| same(*a, *b));
        self.fix_junctions(&near);
        self.connect();
    }

    /// A power port's name reads left to right after a turn, on the far
    /// side of its symbol, as Altium draws it.
    fn upright_power_text(&mut self, t: usize) {
        let Some(tag) = self.tags.get(t) else { return };
        let loc = tag.at[0];
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for s in self.shapes.iter().filter(|s| s.tag == Some(t as u32)) {
            let pts: Vec<Pt> = match &s.geom {
                SchGeom::Line(v) | SchGeom::Poly(v) => v.clone(),
                SchGeom::Arc { c, .. } => vec![*c],
                SchGeom::Rect { min, max, .. } => vec![*min, *max],
            };
            for p in pts {
                sx += p[0] - loc[0];
                sy += p[1] - loc[1];
                n += 1.0;
            }
        }
        if n == 0.0 {
            return;
        }
        let just = if sx.abs() >= sy.abs() {
            if sx >= 0.0 {
                3
            } else {
                5
            }
        } else if sy >= 0.0 {
            1
        } else {
            7
        };
        if let Some(g) = tag
            .graphic
            .and_then(|g| self.texts.get_mut(g as usize))
            .filter(|g| g.part.is_none())
        {
            g.rot = 0;
            g.just = just;
        }
    }

    /// How many ways meet at `p`: one per wire end and pin end, two per
    /// wire running through.
    fn arms(&self, p: Pt) -> usize {
        let mut n = self.pins.iter().filter(|q| same(q.at, p)).count();
        for w in &self.wires {
            for (k, s) in w.windows(2).enumerate() {
                if inside_segment(p, s[0], s[1]) {
                    n += 2;
                } else {
                    if k == 0 && same(s[0], p) {
                        n += 1;
                    }
                    if k + 2 == w.len() && same(s[1], p) {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Junctions about the points `near`: a dot where three or more ways
    /// meet with a wire running through, none where fewer than three meet.
    pub fn fix_junctions(&mut self, near: &[Pt]) {
        let mut gone = Vec::new();
        for (i, &j) in self.junctions.iter().enumerate() {
            if near.iter().any(|&p| same(p, j)) && self.arms(j) < 3 {
                gone.push(i);
            }
        }
        for &i in gone.iter().rev() {
            self.delete_junction(i);
        }
        for &p in near {
            let through = self
                .wires
                .iter()
                .any(|w| w.windows(2).any(|s| inside_segment(p, s[0], s[1])));
            if through && self.arms(p) >= 3 {
                self.add_junction(p);
            }
        }
    }

    pub fn delete_junction(&mut self, i: usize) {
        let Some(&at) = self.junctions.get(i) else {
            return;
        };
        self.junctions.remove(i);
        if let Some(k) = self.shapes.iter().position(|s| is_dot(s, at)) {
            self.shapes.remove(k);
        }
    }

    /// Places a No ERC marker at `at`: the pin or wire there is not checked.
    pub fn add_no_erc(&mut self, at: Pt) -> usize {
        let g = no_erc_drawing(at);
        self.shapes.extend(g.shapes);
        self.no_erc.push(at);
        self.no_erc.len() - 1
    }

    pub fn delete_no_erc(&mut self, i: usize) {
        let Some(&at) = self.no_erc.get(i) else {
            return;
        };
        self.no_erc.remove(i);
        self.shapes.retain(|s| !is_cross(s, at));
    }

    /// Adds a drawn net tag (a power port drawn by a reader): its symbol,
    /// name and tag. Returns the tag's index.
    pub fn add_drawn_tag(&mut self, g: &Drawing) -> Option<usize> {
        let tag = g.tags.first()?.clone();
        let t = self.tags.len() as u32;
        let base = self.texts.len() as u32;
        self.shapes.extend(g.shapes.iter().cloned().map(|mut s| {
            s.part = None;
            s.wire = None;
            s.tag = Some(t);
            s
        }));
        self.texts.extend(g.texts.iter().cloned().map(|mut x| {
            x.part = None;
            x
        }));
        self.tags.push(NetTag {
            graphic: tag.graphic.map(|k| k + base),
            ..tag
        });
        let at = self.tags[t as usize].at.clone();
        self.fix_junctions(&at);
        self.connect();
        Some(t as usize)
    }

    /// Deletes everything selected.
    pub fn delete_sel(&mut self, sel: &SchSel) {
        let down = |v: &[usize]| {
            let mut v = v.to_vec();
            v.sort_unstable();
            v.dedup();
            v.reverse();
            v
        };
        let mut near: Vec<Pt> = Vec::new();
        for t in down(&sel.tags) {
            if let Some(tag) = self.tags.get(t) {
                near.extend(tag.at.iter().copied());
            }
            self.delete_tag(t);
        }
        for m in down(&sel.no_erc) {
            self.delete_no_erc(m);
        }
        for j in down(&sel.junctions) {
            self.delete_junction(j);
        }
        for w in down(&sel.wires) {
            if let Some(wire) = self.wires.get(w) {
                near.extend(wire.iter().copied());
            }
            self.delete_wire(w);
        }
        for b in down(&sel.buses) {
            if b < self.buses.len() {
                self.buses.remove(b);
            }
        }
        for b in down(&sel.bus_entries) {
            if b < self.bus_entries.len() {
                self.bus_entries.remove(b);
            }
        }
        for b in down(&sel.harnesses) {
            if b < self.harnesses.len() {
                self.harnesses.remove(b);
            }
        }
        for b in down(&sel.signal_harnesses) {
            if b < self.signal_harnesses.len() {
                self.signal_harnesses.remove(b);
            }
        }
        for p in down(&sel.parts) {
            near.extend(
                self.pins
                    .iter()
                    .filter(|q| q.part == p as u32)
                    .map(|q| q.at),
            );
            self.delete_part(p);
        }
        self.fix_junctions(&near);
        self.connect();
    }
}

impl Sheet {
    /// A sheet holding only the selection, to paste elsewhere.
    pub fn clip(&self, sel: &SchSel) -> Sheet {
        let d = &self.drawing;
        let mut out = Sheet {
            name: self.name.clone(),
            ..Default::default()
        };
        let o = &mut out.drawing;
        o.size = d.size;
        o.paper = d.paper;
        let mut parts: HashMap<u32, u32> = HashMap::new();
        for &i in &sel.parts {
            let Some(q) = d.parts.get(i) else { continue };
            parts.insert(i as u32, o.parts.len() as u32);
            out.symbols
                .push(self.symbols.get(q.symbol).cloned().unwrap_or_default());
            o.parts.push(SchPart {
                symbol: out.symbols.len() - 1,
                ..q.clone()
            });
        }
        let mut wires: HashMap<u32, u32> = HashMap::new();
        for &w in &sel.wires {
            if let Some(wire) = d.wires.get(w) {
                wires.insert(w as u32, o.wires.len() as u32);
                o.wires.push(wire.clone());
            }
        }
        let mut tags: HashMap<u32, u32> = HashMap::new();
        for &t in &sel.tags {
            if d.tags.get(t).is_some() {
                tags.insert(t as u32, tags.len() as u32);
            }
        }
        let dots: Vec<Pt> = sel
            .junctions
            .iter()
            .filter_map(|&j| d.junctions.get(j).copied())
            .collect();
        let crosses: Vec<Pt> = sel
            .no_erc
            .iter()
            .filter_map(|&j| d.no_erc.get(j).copied())
            .collect();
        for s in &d.shapes {
            let keep = match (s.part, s.wire, s.tag) {
                (Some(p), _, _) => parts.get(&p).map(|&n| (Some(n), None, None)),
                (_, Some(w), _) => wires.get(&w).map(|&n| (None, Some(n), None)),
                (_, _, Some(t)) => tags.get(&t).map(|&n| (None, None, Some(n))),
                _ => (dots.iter().any(|&j| is_dot(s, j))
                    || crosses.iter().any(|&m| is_cross(s, m)))
                .then_some((None, None, None)),
            };
            if let Some((part, wire, tag)) = keep {
                o.shapes.push(SchShape {
                    part,
                    wire,
                    tag,
                    ..s.clone()
                });
            }
        }
        let mut text_map: HashMap<u32, u32> = HashMap::new();
        let tag_texts: Vec<u32> = sel
            .tags
            .iter()
            .filter_map(|&t| d.tags.get(t).and_then(|t| t.graphic))
            .collect();
        for (i, t) in d.texts.iter().enumerate() {
            let part = match t.part {
                Some(p) => match parts.get(&p) {
                    Some(&n) => Some(n),
                    None => continue,
                },
                None if tag_texts.contains(&(i as u32)) => None,
                None => continue,
            };
            text_map.insert(i as u32, o.texts.len() as u32);
            o.texts.push(SchText { part, ..t.clone() });
        }
        for p in &d.pins {
            if let Some(&n) = parts.get(&p.part) {
                o.pins.push(SchPin {
                    part: n,
                    ..p.clone()
                });
            }
        }
        for &t in &sel.tags {
            if let Some(tag) = d.tags.get(t) {
                o.tags.push(NetTag {
                    graphic: tag.graphic.and_then(|g| text_map.get(&g).copied()),
                    ..tag.clone()
                });
            }
        }
        o.junctions = dots;
        o.no_erc = crosses;
        o.buses = sel
            .buses
            .iter()
            .filter_map(|&i| d.buses.get(i).cloned())
            .collect();
        o.bus_entries = sel
            .bus_entries
            .iter()
            .filter_map(|&i| d.bus_entries.get(i).copied())
            .collect();
        o.harnesses = sel
            .harnesses
            .iter()
            .filter_map(|&i| d.harnesses.get(i).cloned())
            .collect();
        o.signal_harnesses = sel
            .signal_harnesses
            .iter()
            .filter_map(|&i| d.signal_harnesses.get(i).cloned())
            .collect();
        o.connect();
        out
    }

    /// Pastes a clipboard sheet `d` mil from where it was copied, its parts
    /// renamed by `names` (old designator -> new; gates of one part keep
    /// sharing a designator). Returns what was pasted, for moving it.
    pub fn paste_clip(&mut self, clip: &Sheet, d: Pt, names: &HashMap<String, String>) -> SchSel {
        let c = &clip.drawing;
        let xf = Xf::Shift(d);
        let sym0 = self.symbols.len();
        let (p0, w0, t0, x0) = (
            self.drawing.parts.len() as u32,
            self.drawing.wires.len() as u32,
            self.drawing.tags.len() as u32,
            self.drawing.texts.len() as u32,
        );
        let (j0, m0) = (self.drawing.junctions.len(), self.drawing.no_erc.len());
        let (b0, e0, h0, s0) = (
            self.drawing.buses.len(),
            self.drawing.bus_entries.len(),
            self.drawing.harnesses.len(),
            self.drawing.signal_harnesses.len(),
        );
        for s in &clip.symbols {
            let mut s = s.clone();
            if let Some(n) = names.get(&s.designator) {
                s.designator = n.clone();
            }
            self.symbols.push(s);
        }
        let dr = &mut self.drawing;
        for q in &c.parts {
            let (a, b) = (xf.apply(q.min), xf.apply(q.max));
            dr.parts.push(SchPart {
                symbol: q.symbol + sym0,
                min: a,
                max: b,
                ..q.clone()
            });
        }
        for s in &c.shapes {
            let mut s = s.clone();
            xf.geom(&mut s.geom);
            s.part = s.part.map(|p| p + p0);
            s.wire = s.wire.map(|w| w + w0);
            s.tag = s.tag.map(|t| t + t0);
            dr.shapes.push(s);
        }
        for t in &c.texts {
            let mut t = t.clone();
            xf.text(&mut t);
            t.part = t.part.map(|p| p + p0);
            dr.texts.push(t);
        }
        for p in &c.pins {
            dr.pins.push(SchPin {
                at: xf.apply(p.at),
                base: xf.apply(p.base),
                part: p.part + p0,
                ..p.clone()
            });
        }
        for w in &c.wires {
            dr.wires.push(w.iter().map(|&p| xf.apply(p)).collect());
        }
        for t in &c.tags {
            dr.tags.push(NetTag {
                at: t.at.iter().map(|&p| xf.apply(p)).collect(),
                graphic: t.graphic.map(|g| g + x0),
                ..t.clone()
            });
        }
        dr.junctions
            .extend(c.junctions.iter().map(|&p| xf.apply(p)));
        dr.no_erc.extend(c.no_erc.iter().map(|&p| xf.apply(p)));
        let line = |v: &Vec<Pt>| v.iter().map(|&p| xf.apply(p)).collect::<Vec<Pt>>();
        dr.buses.extend(c.buses.iter().map(line));
        dr.signal_harnesses
            .extend(c.signal_harnesses.iter().map(line));
        dr.bus_entries.extend(
            c.bus_entries
                .iter()
                .map(|e| [xf.apply(e[0]), xf.apply(e[1])]),
        );
        dr.harnesses.extend(c.harnesses.iter().map(|h| Harness {
            min: xf.apply(h.min),
            max: xf.apply(h.max),
            ..h.clone()
        }));
        // New designators, drawn texts too.
        for k in 0..c.parts.len() {
            let i = p0 as usize + k;
            let old = dr.parts[i].designator.clone();
            if let Some(n) = names.get(&old) {
                dr.rename_part(i, n);
            }
        }
        let sel = SchSel {
            parts: (p0 as usize..dr.parts.len()).collect(),
            wires: (w0 as usize..dr.wires.len()).collect(),
            tags: (t0 as usize..dr.tags.len()).collect(),
            junctions: (j0..dr.junctions.len()).collect(),
            no_erc: (m0..dr.no_erc.len()).collect(),
            buses: (b0..dr.buses.len()).collect(),
            bus_entries: (e0..dr.bus_entries.len()).collect(),
            harnesses: (h0..dr.harnesses.len()).collect(),
            signal_harnesses: (s0..dr.signal_harnesses.len()).collect(),
        };
        let mut near: Vec<Pt> = dr.pins[dr.pins.len() - c.pins.len()..]
            .iter()
            .map(|p| p.at)
            .collect();
        for w in &dr.wires[w0 as usize..] {
            near.extend(w.first().copied());
            near.extend(w.last().copied());
        }
        dr.fix_junctions(&near);
        dr.connect();
        sel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two resistors side by side, R1 wired to R2, and a wire from R2 off
    /// to the right.
    fn sheet() -> Sheet {
        let mut s = Sheet::default();
        s.drawing.size = [11000.0, 8500.0];
        for (k, x) in [(0usize, 0.0), (1, 1000.0)] {
            let des = format!("R{}", k + 1);
            s.symbols.push(Symbol {
                designator: des.clone(),
                library_ref: "RES".into(),
                value: Some("10k".into()),
                ..Default::default()
            });
            let d = &mut s.drawing;
            d.parts.push(SchPart {
                designator: des.clone(),
                symbol: k,
                min: [x - 100.0, -50.0],
                max: [x + 100.0, 50.0],
                gate: 0,
            });
            for (n, dx) in [("1", -200.0), ("2", 200.0)] {
                d.pins.push(SchPin {
                    at: [x + dx, 0.0],
                    base: [x + dx / 2.0, 0.0],
                    part: k as u32,
                    name: String::new(),
                    number: n.into(),
                    ..Default::default()
                });
            }
            d.shapes.push(SchShape {
                geom: SchGeom::Rect {
                    min: [x - 100.0, -50.0],
                    max: [x + 100.0, 50.0],
                    r: [0.0, 0.0],
                },
                width: 5.0,
                stroke: Some([0, 0, 0]),
                fill: None,
                dashed: false,
                part: Some(k as u32),
                wire: None,
                tag: None,
            });
            d.texts.push(SchText {
                at: [x - 100.0, 60.0],
                text: des,
                size: 100.0,
                color: [0, 0, 0],
                rot: 0,
                just: 0,
                bold: false,
                italic: false,
                wrap: None,
                part: Some(k as u32),
            });
        }
        s.drawing.add_wire(vec![[200.0, 0.0], [800.0, 0.0]]);
        s.drawing.add_wire(vec![[1200.0, 0.0], [1600.0, 0.0]]);
        s
    }

    #[test]
    fn box_picks_inside_or_touching() {
        let s = sheet();
        let d = &s.drawing;
        let inside = d.select_box([-300.0, -100.0], [900.0, 100.0], false);
        assert_eq!(inside.parts, vec![0]);
        assert_eq!(inside.wires, vec![0]);
        let touch = d.select_box([700.0, -100.0], [1100.0, 100.0], true);
        assert_eq!(touch.parts, vec![1]);
        assert_eq!(touch.wires, vec![0]);
        assert_eq!(d.select_all().len(), 4);
    }

    #[test]
    fn moving_a_group_keeps_its_wiring_and_stretches_the_rest() {
        let mut s = sheet();
        let sel = s
            .drawing
            .select_box([-300.0, -100.0], [900.0, 100.0], false);
        s.drawing.xf_sel(&sel, Xf::Shift([0.0, 500.0]));
        let d = &s.drawing;
        // The selected wire moved whole.
        assert_eq!(d.wires[0].first(), Some(&[200.0, 500.0]));
        // R2 did not move, and the wire keeps hold of its pin, bending
        // down to it square.
        assert_eq!(d.pins[2].at, [800.0, 0.0]);
        assert_eq!(d.wires[0].last(), Some(&[800.0, 0.0]));
        assert!(d.wires[0]
            .windows(2)
            .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1]));
        let net = d.nets.iter().find(|n| n.wires.contains(&0)).unwrap();
        let mut pins = net.pins.clone();
        pins.sort();
        assert_eq!(pins, vec![1, 2]);
    }

    #[test]
    fn turning_and_mirroring_a_part() {
        let mut s = sheet();
        let sel = SchSel::part(1);
        s.drawing.xf_sel(&sel, Xf::MirrorX(1000.0));
        // Pins swap sides; the wires on them follow.
        assert_eq!(s.drawing.pins[2].at, [1200.0, 0.0]);
        assert_eq!(s.drawing.pins[3].at, [800.0, 0.0]);
        assert_eq!(s.drawing.wires[0].last(), Some(&[1200.0, 0.0]));
        // The designator stays over the body, now right-aligned.
        assert_eq!(s.drawing.texts[1].just, 2);
        assert_eq!(s.drawing.texts[1].at, [1100.0, 60.0]);
        let c = s.drawing.sel_centre(&sel).unwrap();
        s.drawing.xf_sel(&sel, Xf::Turn(c));
        assert_eq!(s.drawing.pins[2].at, [1000.0, 200.0]);
        for w in &s.drawing.wires {
            assert!(w
                .windows(2)
                .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1]));
        }
    }

    #[test]
    fn copy_and_paste_gets_new_designators_and_its_own_wiring() {
        let mut s = sheet();
        let sel = s
            .drawing
            .select_box([-300.0, -100.0], [1300.0, 100.0], false);
        let clip = s.clip(&sel);
        assert_eq!(clip.drawing.parts.len(), 2);
        assert_eq!(clip.drawing.wires.len(), 1);
        let names: HashMap<String, String> = [("R1", "R3"), ("R2", "R4")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let new = s.paste_clip(&clip, [0.0, -1000.0], &names);
        assert_eq!(new.parts, vec![2, 3]);
        let d = &s.drawing;
        assert_eq!(d.parts[2].designator, "R3");
        assert_eq!(s.symbols[d.parts[3].symbol].designator, "R4");
        assert!(d
            .texts
            .iter()
            .any(|t| t.text == "R4" && t.at == [900.0, -940.0]));
        // The copy's wire joins the copy's pins, not the originals.
        let net = d.nets.iter().find(|n| n.wires.contains(&2)).unwrap();
        let mut pins = net.pins.clone();
        pins.sort();
        assert_eq!(pins, vec![5, 6]);
        // Deleting the copy leaves the sheet as it was.
        s.drawing.delete_sel(&new);
        assert_eq!(s.drawing.parts.len(), 2);
        assert_eq!(s.drawing.wires.len(), 2);
    }

    #[test]
    fn tees_get_junctions_and_lose_them() {
        let mut s = sheet();
        let w = s
            .drawing
            .add_wire(vec![[500.0, 0.0], [500.0, -400.0]])
            .unwrap();
        assert_eq!(s.drawing.junctions, vec![[500.0, 0.0]]);
        // Moving the branch off the wire takes the dot away.
        s.drawing.xf_sel(&SchSel::wire(w), Xf::Shift([0.0, -100.0]));
        assert!(s.drawing.junctions.is_empty());
        // and back puts it there again.
        s.drawing.xf_sel(&SchSel::wire(w), Xf::Shift([0.0, 100.0]));
        assert_eq!(s.drawing.junctions, vec![[500.0, 0.0]]);
        assert_eq!(
            s.drawing
                .shapes
                .iter()
                .filter(|sh| is_dot(sh, [500.0, 0.0]))
                .count(),
            1
        );
    }

    #[test]
    fn no_erc_markers_place_move_and_go() {
        let mut s = sheet();
        let m = s.drawing.add_no_erc([1600.0, 0.0]);
        assert_eq!(
            s.drawing
                .shapes
                .iter()
                .filter(|sh| is_cross(sh, [1600.0, 0.0]))
                .count(),
            2
        );
        let sel = SchSel {
            no_erc: vec![m],
            ..Default::default()
        };
        s.drawing.xf_sel(&sel, Xf::Shift([100.0, 0.0]));
        assert_eq!(s.drawing.no_erc, vec![[1700.0, 0.0]]);
        assert_eq!(
            s.drawing
                .shapes
                .iter()
                .filter(|sh| is_cross(sh, [1700.0, 0.0]))
                .count(),
            2
        );
        s.drawing.delete_sel(&sel);
        assert!(s.drawing.no_erc.is_empty());
        assert!(!s
            .drawing
            .shapes
            .iter()
            .any(|sh| sh.stroke == Some(NO_ERC_COLOR)));
    }

    #[test]
    fn move_breaks_connections_and_drag_keeps_them() {
        // Move: R2 goes, the wires stay where they were.
        let mut s = sheet();
        s.drawing
            .xf_sel_mode(&SchSel::part(1), Xf::Shift([0.0, 300.0]), false);
        let d = &s.drawing;
        assert_eq!(d.pins[2].at, [800.0, 300.0]);
        assert_eq!(d.wires[0], vec![[200.0, 0.0], [800.0, 0.0]]);
        assert_eq!(d.wires[1], vec![[1200.0, 0.0], [1600.0, 0.0]]);
        let net = d.nets.iter().find(|n| n.wires.contains(&0)).unwrap();
        assert_eq!(net.pins, vec![1], "R2 left the net");
        // Drag: the wires bend to follow, square.
        let mut s = sheet();
        s.drawing
            .xf_sel_mode(&SchSel::part(1), Xf::Shift([0.0, 300.0]), true);
        let d = &s.drawing;
        assert_eq!(d.wires[0].last(), Some(&[800.0, 300.0]));
        assert_eq!(d.wires[1].first(), Some(&[1200.0, 300.0]));
        assert!(d.wires.iter().all(|w| w
            .windows(2)
            .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1])));
        let net = d.nets.iter().find(|n| n.wires.contains(&0)).unwrap();
        let mut pins = net.pins.clone();
        pins.sort();
        assert_eq!(pins, vec![1, 2]);
    }

    #[test]
    fn dragging_a_wire_segment_keeps_its_ends() {
        let mut s = sheet();
        // R1 to R2 the long way round: down, across, up.
        let d = &mut s.drawing;
        d.delete_wire(0);
        let w = d
            .add_wire(vec![
                [200.0, 0.0],
                [200.0, -300.0],
                [800.0, -300.0],
                [800.0, 0.0],
            ])
            .unwrap();
        // A branch tees onto the bottom run.
        let t = d.add_wire(vec![[500.0, -300.0], [500.0, -600.0]]).unwrap();
        assert_eq!(d.junctions, vec![[500.0, -300.0]]);
        // The bottom run drags only up and down, however the pointer goes.
        d.drag_segment(w, 1, [70.0, -200.0]);
        assert_eq!(
            d.wires[w],
            vec![[200.0, 0.0], [200.0, -500.0], [800.0, -500.0], [800.0, 0.0]]
        );
        assert_eq!(d.wires[t][0], [500.0, -500.0], "the branch follows");
        assert_eq!(d.junctions, vec![[500.0, -500.0]]);
        // An end segment on a pin keeps the pin with a new leg.
        d.drag_segment(w, 0, [-100.0, 0.0]);
        assert_eq!(d.wires[w][0], [200.0, 0.0]);
        assert_eq!(d.wires[w][1], [100.0, 0.0]);
        let net = d
            .nets
            .iter()
            .find(|n| n.wires.contains(&(w as u32)))
            .unwrap();
        let mut pins = net.pins.clone();
        pins.sort();
        assert_eq!(pins, vec![1, 2]);
        assert!(d.wires[w]
            .windows(2)
            .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1]));
    }

    #[test]
    fn harness_moves_with_its_wires_and_buses_copy() {
        let mut s = sheet();
        let d = &mut s.drawing;
        let h = d.add_harness(Harness::new([2000.0, 400.0], "I2C", &["SDA", "SCL"], true));
        let e = d.harnesses[h].entry_at(&d.harnesses[h].entries[0]);
        let w = d.add_wire(vec![[e[0] - 300.0, e[1]], e]).unwrap();
        let b = d.harnesses[h].bundle_at();
        let sh = d.add_signal_harness(vec![b, [b[0] + 500.0, b[1]]]).unwrap();
        let sel = SchSel {
            harnesses: vec![h],
            ..Default::default()
        };
        d.xf_sel(&sel, Xf::Shift([0.0, 200.0]));
        let e2 = d.harnesses[h].entry_at(&d.harnesses[h].entries[0]);
        assert_eq!(e2, [e[0], e[1] + 200.0]);
        assert_eq!(d.wires[w].last(), Some(&e2), "the wire follows its entry");
        assert_eq!(d.signal_harnesses[sh][0], d.harnesses[h].bundle_at());
        // Mirrored, the entries go to the other edge.
        let c = d.sel_centre(&sel).unwrap();
        d.xf_sel(&sel, Xf::MirrorX(c[0]));
        assert!(d.harnesses[h].entries_right());
        // A bus and an entry copy and paste with the rest.
        let bus = d.add_bus(vec![[0.0, -1000.0], [1000.0, -1000.0]]).unwrap();
        d.add_bus_entry([100.0, -1000.0], [200.0, -900.0]);
        let sel = d.select_box([-100.0, -1100.0], [1100.0, -800.0], false);
        assert_eq!(sel.buses, vec![bus]);
        assert_eq!(sel.bus_entries, vec![0]);
        let clip = s.clip(&sel);
        let new = s.paste_clip(&clip, [0.0, -500.0], &HashMap::new());
        assert_eq!(new.buses, vec![1]);
        assert_eq!(
            s.drawing.bus_entries[1],
            [[100.0, -1500.0], [200.0, -1400.0]]
        );
        s.drawing.delete_sel(&new);
        assert_eq!(s.drawing.buses.len(), 1);
    }

    #[test]
    fn bus_breakout_names_every_member() {
        let mut s = sheet();
        let d = &mut s.drawing;
        let b = d.add_bus(vec![[0.0, 1000.0], [1000.0, 1000.0]]).unwrap();
        let n = d.bus_breakout(b, "D[0..3]", [100.0, 1000.0], 100.0, 300.0, false);
        assert_eq!(n, 4);
        assert_eq!(d.bus_entries.len(), 4);
        assert_eq!(d.bus_entries[0], [[100.0, 1000.0], [200.0, 1100.0]]);
        for m in ["D0", "D1", "D2", "D3", "D[0..3]"] {
            assert!(d.tags.iter().any(|t| t.text == m), "{m}");
        }
        let d3 = d
            .nets
            .iter()
            .find(|n| n.name.as_deref() == Some("D3"))
            .unwrap();
        assert_eq!(d3.wires.len(), 1);
    }
}
