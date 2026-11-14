//! Editing many things at once: a selection of parts, tracks, arcs and
//! vias, and what can be done to it (move, turn, align and distribute,
//! delete, copy and paste), plus placed text and linear dimensions.
//!
//! Every operation returns edits for the undo history; the board itself is
//! never changed here.

use std::collections::BTreeSet;

use crate::edit::{Edit, Prims};
use crate::font::{self, Placing};
use crate::place::Placement;
use crate::{Arc, Board, Body3d, Component, LayerRef, Length, Pad, Point, Polygon, Track, Via};

/// What is selected, by index into the board's lists. Tracks and arcs here
/// are the board's own (no part owns them); a part's drawing goes with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub parts: BTreeSet<usize>,
    pub tracks: BTreeSet<usize>,
    pub arcs: BTreeSet<usize>,
    pub vias: BTreeSet<usize>,
}

fn mil(p: Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

fn pt(p: [f64; 2]) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

type Bounds = [f64; 4];

fn grow(b: &mut Option<Bounds>, p: [f64; 2], r: f64) {
    let n = [p[0] - r, p[1] - r, p[0] + r, p[1] + r];
    *b = Some(match *b {
        None => n,
        Some(o) => [
            o[0].min(n[0]),
            o[1].min(n[1]),
            o[2].max(n[2]),
            o[3].max(n[3]),
        ],
    });
}

fn join(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

fn inside(b: Bounds, box_: Bounds) -> bool {
    b[0] >= box_[0] && b[1] >= box_[1] && b[2] <= box_[2] && b[3] <= box_[3]
}

fn touches(b: Bounds, box_: Bounds) -> bool {
    b[0] <= box_[2] && b[2] >= box_[0] && b[1] <= box_[3] && b[3] >= box_[1]
}

/// A part's extent (mil): its pads and its drawing, leaving out text.
pub fn part_bounds(board: &Board, index: usize) -> Option<Bounds> {
    let me = Some(index as u32);
    let g = &board.geometry;
    let mut b = None;
    for p in g.pads.iter().filter(|p| p.component == me) {
        let r = p.size_x.mil().max(p.size_y.mil()) / 2.0;
        grow(&mut b, mil(p.at), r);
    }
    for t in g.tracks.iter().filter(|t| t.component == me && !t.text) {
        let w = t.width.mil() / 2.0;
        grow(&mut b, mil(t.start), w);
        grow(&mut b, mil(t.end), w);
    }
    for a in g.arcs.iter().filter(|a| a.component == me) {
        grow(&mut b, mil(a.center), a.radius.mil() + a.width.mil() / 2.0);
    }
    if b.is_none() {
        let c = board.components.get(index)?;
        grow(&mut b, [c.x.mil(), c.y.mil()], 5.0);
    }
    b
}

/// Every part's extent at once, in one pass over the board (as
/// `part_bounds` gives for one).
pub fn all_part_bounds(board: &Board) -> Vec<Option<Bounds>> {
    let n = board.components.len();
    let mut out: Vec<Option<Bounds>> = vec![None; n];
    let g = &board.geometry;
    let slot = |c: Option<u32>| c.map(|c| c as usize).filter(|&c| c < n);
    for p in &g.pads {
        if let Some(c) = slot(p.component) {
            grow(
                &mut out[c],
                mil(p.at),
                p.size_x.mil().max(p.size_y.mil()) / 2.0,
            );
        }
    }
    for t in g.tracks.iter().filter(|t| !t.text) {
        if let Some(c) = slot(t.component) {
            let w = t.width.mil() / 2.0;
            grow(&mut out[c], mil(t.start), w);
            grow(&mut out[c], mil(t.end), w);
        }
    }
    for a in &g.arcs {
        if let Some(c) = slot(a.component) {
            grow(
                &mut out[c],
                mil(a.center),
                a.radius.mil() + a.width.mil() / 2.0,
            );
        }
    }
    for (c, b) in out.iter_mut().enumerate() {
        if b.is_none() {
            let p = &board.components[c];
            grow(b, [p.x.mil(), p.y.mil()], 5.0);
        }
    }
    out
}

fn track_bounds(t: &Track) -> Bounds {
    let w = t.width.mil() / 2.0;
    let (a, b) = (mil(t.start), mil(t.end));
    [
        a[0].min(b[0]) - w,
        a[1].min(b[1]) - w,
        a[0].max(b[0]) + w,
        a[1].max(b[1]) + w,
    ]
}

fn arc_bounds(a: &Arc) -> Bounds {
    let r = a.radius.mil() + a.width.mil() / 2.0;
    let c = mil(a.center);
    [c[0] - r, c[1] - r, c[0] + r, c[1] + r]
}

fn via_bounds(v: &Via) -> Bounds {
    let r = v.diameter.mil() / 2.0;
    let c = mil(v.at);
    [c[0] - r, c[1] - r, c[0] + r, c[1] + r]
}

impl Sel {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
            && self.tracks.is_empty()
            && self.arcs.is_empty()
            && self.vias.is_empty()
    }

    pub fn len(&self) -> usize {
        self.parts.len() + self.tracks.len() + self.arcs.len() + self.vias.len()
    }

    pub fn clear(&mut self) {
        *self = Sel::default();
    }

    /// Everything that can be selected: every part, and the board's own
    /// tracks, arcs and vias.
    pub fn all(board: &Board) -> Sel {
        let g = &board.geometry;
        Sel {
            parts: (0..board.components.len()).collect(),
            tracks: (0..g.tracks.len())
                .filter(|&i| g.tracks[i].component.is_none())
                .collect(),
            arcs: (0..g.arcs.len())
                .filter(|&i| g.arcs[i].component.is_none())
                .collect(),
            vias: (0..g.vias.len()).collect(),
        }
    }

    /// What lies in a box (mil): wholly inside it, or, with `touching`, any
    /// part of it. Free text is picked up stroke by stroke.
    pub fn in_box(board: &Board, b: Bounds, touching: bool) -> Sel {
        let ok = |x: Bounds| {
            if touching {
                touches(x, b)
            } else {
                inside(x, b)
            }
        };
        let g = &board.geometry;
        let parts = all_part_bounds(board);
        Sel {
            parts: (0..board.components.len())
                .filter(|&i| parts[i].is_some_and(ok))
                .collect(),
            tracks: (0..g.tracks.len())
                .filter(|&i| g.tracks[i].component.is_none() && ok(track_bounds(&g.tracks[i])))
                .collect(),
            arcs: (0..g.arcs.len())
                .filter(|&i| g.arcs[i].component.is_none() && ok(arc_bounds(&g.arcs[i])))
                .collect(),
            vias: (0..g.vias.len())
                .filter(|&i| ok(via_bounds(&g.vias[i])))
                .collect(),
        }
    }

    /// Adds what `other` holds, or takes it away where both hold it
    /// (Ctrl+click).
    pub fn toggle(&mut self, other: &Sel) {
        fn flip(a: &mut BTreeSet<usize>, b: &BTreeSet<usize>) {
            for &i in b {
                if !a.remove(&i) {
                    a.insert(i);
                }
            }
        }
        flip(&mut self.parts, &other.parts);
        flip(&mut self.tracks, &other.tracks);
        flip(&mut self.arcs, &other.arcs);
        flip(&mut self.vias, &other.vias);
    }

    pub fn add(&mut self, other: &Sel) {
        self.parts.extend(&other.parts);
        self.tracks.extend(&other.tracks);
        self.arcs.extend(&other.arcs);
        self.vias.extend(&other.vias);
    }

    /// Drops indices the board no longer has (after an undo).
    pub fn keep_valid(&mut self, board: &Board) {
        let g = &board.geometry;
        self.parts.retain(|&i| i < board.components.len());
        self.tracks
            .retain(|&i| g.tracks.get(i).is_some_and(|t| t.component.is_none()));
        self.arcs
            .retain(|&i| g.arcs.get(i).is_some_and(|a| a.component.is_none()));
        self.vias.retain(|&i| i < g.vias.len());
    }

    /// The selection's extent (mil).
    pub fn bounds(&self, board: &Board) -> Option<Bounds> {
        let g = &board.geometry;
        let mut b = None;
        if self.parts.len() > 8 {
            let all = all_part_bounds(board);
            for &i in &self.parts {
                b = join(b, all.get(i).copied().flatten());
            }
        } else {
            for &i in &self.parts {
                b = join(b, part_bounds(board, i));
            }
        }
        for t in self.tracks.iter().filter_map(|&i| g.tracks.get(i)) {
            b = join(b, Some(track_bounds(t)));
        }
        for a in self.arcs.iter().filter_map(|&i| g.arcs.get(i)) {
            b = join(b, Some(arc_bounds(a)));
        }
        for v in self.vias.iter().filter_map(|&i| g.vias.get(i)) {
            b = join(b, Some(via_bounds(v)));
        }
        b
    }

    /// A few words on what is selected: "3 parts, 12 tracks".
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        for (n, one, many) in [
            (self.parts.len(), "part", "parts"),
            (self.tracks.len(), "track", "tracks"),
            (self.arcs.len(), "arc", "arcs"),
            (self.vias.len(), "via", "vias"),
        ] {
            if n > 0 {
                out.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        }
        if out.is_empty() {
            "nothing".into()
        } else {
            out.join(", ")
        }
    }
}

/// A change of position applied to every selected thing: a shift, or a
/// turn about a point.
#[derive(Clone, Copy)]
enum Xf {
    Shift([f64; 2]),
    Turn { about: [f64; 2], deg: f64 },
}

impl Xf {
    fn xy(&self, p: [f64; 2]) -> [f64; 2] {
        match *self {
            Xf::Shift(d) => [p[0] + d[0], p[1] + d[1]],
            Xf::Turn { about: c, deg } => {
                let (s, k) = deg.to_radians().sin_cos();
                let (x, y) = (p[0] - c[0], p[1] - c[1]);
                [c[0] + x * k - y * s, c[1] + x * s + y * k]
            }
        }
    }

    fn point(&self, p: Point) -> Point {
        pt(self.xy(mil(p)))
    }

    fn deg(&self) -> f64 {
        match *self {
            Xf::Shift(_) => 0.0,
            Xf::Turn { deg, .. } => deg,
        }
    }
}

fn turn(a: f64, d: f64) -> f64 {
    let r = (a + d).rem_euclid(360.0);
    // Keep whole angles whole.
    if (r - r.round()).abs() < 1e-9 {
        r.round() % 360.0
    } else {
        r
    }
}

/// The edits that put every selected thing where `f` sends it; tracks,
/// arcs and vias are replaced where they are in their lists, so the
/// selection still holds.
fn transform(
    board: &Board,
    sel: &Sel,
    f: impl Fn(usize) -> Option<Xf>,
    part: impl Fn(usize) -> Option<Xf>,
) -> Vec<Edit> {
    let g = &board.geometry;
    let mut edits = Vec::new();
    for &i in &sel.parts {
        let (Some(from), Some(x)) = (Placement::of(board, i), part(i)) else {
            continue;
        };
        let at = x.xy([from.x.mil(), from.y.mil()]);
        let to = Placement {
            x: Length::from_mil(at[0]),
            y: Length::from_mil(at[1]),
            rotation_deg: turn(from.rotation_deg, x.deg()),
            side: from.side,
        };
        if to != from {
            edits.push(Edit::PlaceComponent { index: i, from, to });
        }
    }
    for &i in &sel.tracks {
        let (Some(t), Some(x)) = (g.tracks.get(i), f(i)) else {
            continue;
        };
        let mut n = t.clone();
        n.start = x.point(t.start);
        n.end = x.point(t.end);
        if n.start != t.start || n.end != t.end {
            edits.push(Edit::RemoveTrack {
                index: i,
                track: t.clone(),
            });
            edits.push(Edit::InsertTrack { index: i, track: n });
        }
    }
    for &i in &sel.arcs {
        let (Some(a), Some(x)) = (g.arcs.get(i), f(i)) else {
            continue;
        };
        let mut n = a.clone();
        n.center = x.point(a.center);
        n.start_deg = turn(a.start_deg, x.deg());
        n.end_deg = turn(a.end_deg, x.deg());
        edits.push(Edit::RemoveArc {
            index: i,
            arc: a.clone(),
        });
        edits.push(Edit::InsertArc { index: i, arc: n });
    }
    for &i in &sel.vias {
        let (Some(v), Some(x)) = (g.vias.get(i), f(i)) else {
            continue;
        };
        let mut n = v.clone();
        n.at = x.point(v.at);
        if n.at != v.at {
            edits.push(Edit::RemoveVia {
                index: i,
                via: v.clone(),
            });
            edits.push(Edit::InsertVia { index: i, via: n });
        }
    }
    edits
}

/// Moves the selection by (dx, dy) mil.
pub fn move_by(board: &Board, sel: &Sel, dx: f64, dy: f64) -> Vec<Edit> {
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return Vec::new();
    }
    let x = Some(Xf::Shift([dx, dy]));
    transform(board, sel, |_| x, |_| x)
}

/// Turns the selection by `deg` (counter-clockwise) about its centre, or
/// about `about` when given.
pub fn rotate(board: &Board, sel: &Sel, deg: f64, about: Option<[f64; 2]>) -> Vec<Edit> {
    let Some(c) = about.or_else(|| {
        sel.bounds(board)
            .map(|b| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0])
    }) else {
        return Vec::new();
    };
    let x = Some(Xf::Turn { about: c, deg });
    transform(board, sel, |_| x, |_| x)
}

/// Ways to line things up, as Altium's and Allegro's align menus offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Top,
    Bottom,
    /// Centres on one vertical line.
    CentreX,
    /// Centres on one horizontal line.
    CentreY,
    /// Equal gaps from left to right, the outer two staying put.
    SpreadX,
    /// Equal gaps from bottom to top.
    SpreadY,
}

impl Align {
    pub const ALL: [Align; 8] = [
        Align::Left,
        Align::Right,
        Align::Top,
        Align::Bottom,
        Align::CentreX,
        Align::CentreY,
        Align::SpreadX,
        Align::SpreadY,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Align::Left => "Align left",
            Align::Right => "Align right",
            Align::Top => "Align top",
            Align::Bottom => "Align bottom",
            Align::CentreX => "Align centres vertically",
            Align::CentreY => "Align centres horizontally",
            Align::SpreadX => "Distribute horizontally",
            Align::SpreadY => "Distribute vertically",
        }
    }
}

/// Lines up the selected parts and vias (each by its extent). Tracks and
/// arcs are left where they are.
pub fn align(board: &Board, sel: &Sel, how: Align) -> Vec<Edit> {
    // (is a part, index, extent)
    let mut items: Vec<(bool, usize, Bounds)> = Vec::new();
    let all = all_part_bounds(board);
    for &i in &sel.parts {
        if let Some(b) = all.get(i).copied().flatten() {
            items.push((true, i, b));
        }
    }
    for &i in &sel.vias {
        if let Some(v) = board.geometry.vias.get(i) {
            items.push((false, i, via_bounds(v)));
        }
    }
    if items.len() < 2 {
        return Vec::new();
    }
    let lo = |k: usize| items.iter().map(|t| t.2[k]).fold(f64::MAX, f64::min);
    let hi = |k: usize| items.iter().map(|t| t.2[k]).fold(f64::MIN, f64::max);
    let mid = |b: &Bounds, k: usize| (b[k] + b[k + 2]) / 2.0;
    let mut shift: Vec<[f64; 2]> = vec![[0.0; 2]; items.len()];
    match how {
        Align::Left => {
            let x = lo(0);
            for (s, t) in shift.iter_mut().zip(&items) {
                s[0] = x - t.2[0];
            }
        }
        Align::Right => {
            let x = hi(2);
            for (s, t) in shift.iter_mut().zip(&items) {
                s[0] = x - t.2[2];
            }
        }
        Align::Bottom => {
            let y = lo(1);
            for (s, t) in shift.iter_mut().zip(&items) {
                s[1] = y - t.2[1];
            }
        }
        Align::Top => {
            let y = hi(3);
            for (s, t) in shift.iter_mut().zip(&items) {
                s[1] = y - t.2[3];
            }
        }
        Align::CentreX | Align::CentreY => {
            let k = if how == Align::CentreX { 0 } else { 1 };
            let c = (lo(k) + hi(k + 2)) / 2.0;
            for (s, t) in shift.iter_mut().zip(&items) {
                s[k] = c - mid(&t.2, k);
            }
        }
        Align::SpreadX | Align::SpreadY => {
            let k = if how == Align::SpreadX { 0 } else { 1 };
            if items.len() < 3 {
                return Vec::new();
            }
            let mut order: Vec<usize> = (0..items.len()).collect();
            order.sort_by(|&a, &b| mid(&items[a].2, k).total_cmp(&mid(&items[b].2, k)));
            let first = items[order[0]].2[k];
            let last = items[*order.last().unwrap()].2[k + 2];
            let sizes: f64 = items.iter().map(|t| t.2[k + 2] - t.2[k]).sum();
            let gap = (last - first - sizes) / (items.len() - 1) as f64;
            let mut at = first;
            for &o in &order {
                shift[o][k] = at - items[o].2[k];
                at += items[o].2[k + 2] - items[o].2[k] + gap;
            }
        }
    }
    let find = |part: bool, i: usize| {
        items
            .iter()
            .position(|t| t.0 == part && t.1 == i)
            .map(|k| shift[k])
            .filter(|d| d[0].abs() > 1e-6 || d[1].abs() > 1e-6)
            .map(Xf::Shift)
    };
    let only = Sel {
        parts: sel.parts.clone(),
        vias: sel.vias.clone(),
        ..Sel::default()
    };
    transform(board, &only, |i| find(false, i), |i| find(true, i))
}

/// Removes the parts `gone` and all they own from `board`, renumbering
/// the parts after them, in one pass.
fn remove_parts(board: &mut Board, gone: &BTreeSet<usize>) {
    let n = board.components.len();
    let mut map: Vec<Option<u32>> = Vec::with_capacity(n);
    let mut k = 0u32;
    for i in 0..n {
        if gone.contains(&i) {
            map.push(None);
        } else {
            map.push(Some(k));
            k += 1;
        }
    }
    // Some(None): owned by a part that goes. None: keep as it is.
    let fix = |c: &mut Option<u32>| -> bool {
        match *c {
            Some(i) if (i as usize) < n => match map[i as usize] {
                Some(j) => {
                    *c = Some(j);
                    true
                }
                None => false,
            },
            _ => true,
        }
    };
    let g = &mut board.geometry;
    g.pads.retain_mut(|p| fix(&mut p.component));
    g.tracks.retain_mut(|t| fix(&mut t.component));
    g.arcs.retain_mut(|a| fix(&mut a.component));
    g.polygons.retain_mut(|p| fix(&mut p.component));
    g.areas.retain_mut(|p| fix(&mut p.component));
    g.cutouts.retain_mut(|p| fix(&mut p.component));
    g.bodies.retain_mut(|b| fix(&mut b.component));
    let mut i = 0;
    board.components.retain(|_| {
        i += 1;
        !gone.contains(&(i - 1))
    });
}

fn grown(b: Option<Bounds>, by: f64) -> Option<Bounds> {
    b.map(|b| [b[0] - by, b[1] - by, b[2] + by, b[3] + by])
}

/// Deletes the selection.
pub fn delete(board: &Board, sel: &Sel) -> Vec<Edit> {
    let g = &board.geometry;
    let mut edits = Vec::new();
    if sel.parts.is_empty() {
        for &i in sel.tracks.iter().rev() {
            if let Some(t) = g.tracks.get(i) {
                edits.push(Edit::RemoveTrack {
                    index: i,
                    track: t.clone(),
                });
            }
        }
        for &i in sel.arcs.iter().rev() {
            if let Some(a) = g.arcs.get(i) {
                edits.push(Edit::RemoveArc {
                    index: i,
                    arc: a.clone(),
                });
            }
        }
    } else {
        // Parts renumber what follows them, so this goes as one swap of
        // every list.
        let mut b = board.clone();
        for &i in sel.tracks.iter().rev() {
            if i < b.geometry.tracks.len() {
                b.geometry.tracks.remove(i);
            }
        }
        for &i in sel.arcs.iter().rev() {
            if i < b.geometry.arcs.len() {
                b.geometry.arcs.remove(i);
            }
        }
        remove_parts(&mut b, &sel.parts);
        edits.push(Edit::Parts {
            before: Box::new(Prims::of(board)),
            after: Box::new(Prims::of(&b)),
            area: grown(sel.bounds(board), 50.0),
        });
    }
    for &i in sel.vias.iter().rev() {
        if let Some(v) = g.vias.get(i) {
            edits.push(Edit::RemoveVia {
                index: i,
                via: v.clone(),
            });
        }
    }
    edits
}

/// A part as copied: the part and everything it owns.
#[derive(Clone, Debug)]
pub struct PartCopy {
    pub part: Component,
    pub pads: Vec<Pad>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub polygons: Vec<Polygon>,
    pub areas: Vec<Polygon>,
    pub cutouts: Vec<Polygon>,
    pub bodies: Vec<Body3d>,
}

/// What Copy keeps, for Paste.
#[derive(Clone, Debug, Default)]
pub struct Clip {
    pub parts: Vec<PartCopy>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub vias: Vec<Via>,
    /// The centre of what was copied (mil); Paste puts it at the cursor.
    pub origin: [f64; 2],
}

impl Clip {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
            && self.tracks.is_empty()
            && self.arcs.is_empty()
            && self.vias.is_empty()
    }
}

/// Copies the selection.
pub fn copy(board: &Board, sel: &Sel) -> Clip {
    let g = &board.geometry;
    // Where each selected part lands in the copy.
    let mut slot = vec![usize::MAX; board.components.len()];
    let mut parts: Vec<PartCopy> = Vec::new();
    for &i in &sel.parts {
        if let Some(c) = board.components.get(i) {
            slot[i] = parts.len();
            parts.push(PartCopy {
                part: c.clone(),
                pads: Vec::new(),
                tracks: Vec::new(),
                arcs: Vec::new(),
                polygons: Vec::new(),
                areas: Vec::new(),
                cutouts: Vec::new(),
                bodies: Vec::new(),
            });
        }
    }
    let of = |c: Option<u32>| {
        c.and_then(|c| slot.get(c as usize).copied())
            .filter(|&k| k != usize::MAX)
    };
    for p in &g.pads {
        if let Some(k) = of(p.component) {
            parts[k].pads.push(p.clone());
        }
    }
    for t in &g.tracks {
        if let Some(k) = of(t.component) {
            parts[k].tracks.push(t.clone());
        }
    }
    for a in &g.arcs {
        if let Some(k) = of(a.component) {
            parts[k].arcs.push(a.clone());
        }
    }
    for p in g.polygons.iter().filter(|p| p.pour.is_none()) {
        if let Some(k) = of(p.component) {
            parts[k].polygons.push(p.clone());
        }
    }
    for p in &g.areas {
        if let Some(k) = of(p.component) {
            parts[k].areas.push(p.clone());
        }
    }
    for p in &g.cutouts {
        if let Some(k) = of(p.component) {
            parts[k].cutouts.push(p.clone());
        }
    }
    for b in &g.bodies {
        if let Some(k) = of(b.component) {
            parts[k].bodies.push(b.clone());
        }
    }
    let origin = sel
        .bounds(board)
        .map(|b| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0])
        .unwrap_or_default();
    Clip {
        parts,
        tracks: sel
            .tracks
            .iter()
            .filter_map(|&i| g.tracks.get(i).cloned())
            .collect(),
        arcs: sel
            .arcs
            .iter()
            .filter_map(|&i| g.arcs.get(i).cloned())
            .collect(),
        vias: sel
            .vias
            .iter()
            .filter_map(|&i| g.vias.get(i).cloned())
            .collect(),
        origin,
    }
}

/// A designator not yet on the board, in the series of `like`: R12 gives
/// the next free R number.
pub fn next_designator(taken: &BTreeSet<String>, like: &str) -> String {
    let stem = like.trim_end_matches(|c: char| c.is_ascii_digit());
    let stem = if stem.is_empty() { "X" } else { stem };
    let up = stem.to_ascii_uppercase();
    let mut n = taken
        .iter()
        .filter_map(|d| {
            let s = d.trim_end_matches(|c: char| c.is_ascii_digit());
            (s == up && s.len() < d.len())
                .then(|| d[s.len()..].parse::<u64>().ok())
                .flatten()
        })
        .max()
        .unwrap_or(0)
        + 1;
    loop {
        let name = format!("{stem}{n}");
        if !taken.contains(&name.to_ascii_uppercase()) {
            return name;
        }
        n += 1;
    }
}

/// What a paste put on the board.
pub struct Pasted {
    pub edits: Vec<Edit>,
    /// The new things, to select once the edits are applied.
    pub sel: Sel,
    /// Old designator to new.
    pub renamed: Vec<(String, String)>,
}

/// Pastes `clip` with its centre at `at` (mil). Pasted parts get new
/// designators and no nets, since the schematic does not know them;
/// tracks, arcs and vias keep their nets when `keep_nets`, so a copied via
/// pattern stays on its net.
pub fn paste(board: &Board, clip: &Clip, at: [f64; 2], keep_nets: bool) -> Pasted {
    let d = [at[0] - clip.origin[0], at[1] - clip.origin[1]];
    let x = Xf::Shift(d);
    let net = |n: Option<u32>| if keep_nets { n } else { None };
    let g = &board.geometry;
    let mut sel = Sel::default();
    let mut edits = Vec::new();
    let shift_track = |t: &Track| {
        let mut n = t.clone();
        n.start = x.point(t.start);
        n.end = x.point(t.end);
        n
    };
    let shift_arc = |a: &Arc| {
        let mut n = a.clone();
        n.center = x.point(a.center);
        n
    };
    let shift_poly = |p: &Polygon| {
        let mut n = p.clone();
        for q in n.outline.iter_mut().chain(n.holes.iter_mut().flatten()) {
            *q = x.point(*q);
        }
        n
    };
    let mut renamed = Vec::new();
    if clip.parts.is_empty() {
        for (k, t) in clip.tracks.iter().enumerate() {
            let mut n = shift_track(t);
            n.net = net(t.net);
            let index = g.tracks.len() + k;
            sel.tracks.insert(index);
            edits.push(Edit::InsertTrack { index, track: n });
        }
        for (k, a) in clip.arcs.iter().enumerate() {
            let mut n = shift_arc(a);
            n.net = net(a.net);
            let index = g.arcs.len() + k;
            sel.arcs.insert(index);
            edits.push(Edit::InsertArc { index, arc: n });
        }
    } else {
        let mut b = board.clone();
        let mut taken: BTreeSet<String> = board
            .components
            .iter()
            .map(|c| c.designator.to_ascii_uppercase())
            .collect();
        let models = board.geometry.models.len() as u32;
        for p in &clip.parts {
            let me = Some(b.components.len() as u32);
            sel.parts.insert(b.components.len());
            let mut c = p.part.clone();
            let name = next_designator(&taken, &c.designator);
            taken.insert(name.to_ascii_uppercase());
            renamed.push((c.designator.clone(), name.clone()));
            c.designator = name;
            let o = x.xy([c.x.mil(), c.y.mil()]);
            c.x = Length::from_mil(o[0]);
            c.y = Length::from_mil(o[1]);
            b.components.push(c);
            let bg = &mut b.geometry;
            for pad in &p.pads {
                let mut n = pad.clone();
                n.at = x.point(pad.at);
                for q in &mut n.outline {
                    *q = x.point(*q);
                }
                for l in &mut n.stack {
                    for q in &mut l.outline {
                        *q = x.point(*q);
                    }
                }
                n.net = None;
                n.component = me;
                bg.pads.push(n);
            }
            for t in &p.tracks {
                let mut n = shift_track(t);
                n.net = None;
                n.component = me;
                bg.tracks.push(n);
            }
            for a in &p.arcs {
                let mut n = shift_arc(a);
                n.net = None;
                n.component = me;
                bg.arcs.push(n);
            }
            for (from, to) in [
                (&p.polygons, &mut bg.polygons),
                (&p.areas, &mut bg.areas),
                (&p.cutouts, &mut bg.cutouts),
            ] {
                for f in from {
                    let mut n = shift_poly(f);
                    n.net = None;
                    n.component = me;
                    to.push(n);
                }
            }
            for body in p.bodies.iter().filter(|m| m.model < models) {
                let mut n = body.clone();
                n.at = x.point(body.at);
                n.component = me;
                bg.bodies.push(n);
            }
        }
        for t in &clip.tracks {
            let mut n = shift_track(t);
            n.net = net(t.net);
            sel.tracks.insert(b.geometry.tracks.len());
            b.geometry.tracks.push(n);
        }
        for a in &clip.arcs {
            let mut n = shift_arc(a);
            n.net = net(a.net);
            sel.arcs.insert(b.geometry.arcs.len());
            b.geometry.arcs.push(n);
        }
        let mut area = None;
        let all = all_part_bounds(&b);
        for &i in &sel.parts {
            area = join(area, all[i]);
        }
        edits.push(Edit::Parts {
            before: Box::new(Prims::of(board)),
            after: Box::new(Prims::of(&b)),
            area: grown(area, 50.0),
        });
    }
    for (k, v) in clip.vias.iter().enumerate() {
        let mut n = v.clone();
        n.at = x.point(v.at);
        n.net = net(v.net);
        let index = g.vias.len() + k;
        sel.vias.insert(index);
        edits.push(Edit::InsertVia { index, via: n });
    }
    Pasted {
        edits,
        sel,
        renamed,
    }
}

/// How placed text sits.
#[derive(Clone, Debug)]
pub struct TextStyle {
    pub layer: LayerRef,
    /// Cap height (mil).
    pub height: f64,
    /// Stroke width (mil).
    pub width: f64,
    pub rotation_deg: f64,
    /// Read from below: on bottom layers.
    pub mirror: bool,
}

impl TextStyle {
    pub fn on(layer: LayerRef) -> TextStyle {
        let mirror = matches!(layer, LayerRef::BottomSilk);
        TextStyle {
            layer,
            height: 60.0,
            width: 8.0,
            rotation_deg: 0.0,
            mirror,
        }
    }
}

/// The length of a line of text (mil, stroke centres) at cap height `h`.
pub fn text_length(text: &str, h: f64) -> f64 {
    let (lines, _) = font::strokes(text);
    let (lo, hi) = lines
        .iter()
        .flatten()
        .fold((f64::MAX, f64::MIN), |a, q| (a.0.min(q[0]), a.1.max(q[0])));
    if lo > hi {
        0.0
    } else {
        (hi - lo) * h
    }
}

/// Text as strokes, its baseline starting at `at` (mil), or centred on
/// `at` along the baseline when `centred`.
pub fn text(s: &str, at: [f64; 2], style: &TextStyle, centred: bool) -> Vec<Track> {
    let mut start = at;
    if centred {
        let half = text_length(s, style.height) / 2.0;
        let (sn, cs) = style.rotation_deg.to_radians().sin_cos();
        let k = if style.mirror { -1.0 } else { 1.0 };
        start = [at[0] - k * half * cs, at[1] - k * half * sn];
    }
    let p = Placing {
        at: pt(start),
        height: Length::from_mil(style.height),
        width: Length::from_mil(style.width),
        rotation_deg: style.rotation_deg,
        mirror: style.mirror,
        length: None,
    };
    font::text_tracks(s, &p, style.layer.clone(), None)
}

/// Edits adding tracks at the end of the board's list.
pub fn add_tracks(board: &Board, tracks: Vec<Track>) -> (Vec<Edit>, Sel) {
    let n = board.geometry.tracks.len();
    let mut sel = Sel::default();
    let edits = tracks
        .into_iter()
        .enumerate()
        .map(|(k, track)| {
            sel.tracks.insert(n + k);
            Edit::InsertTrack {
                index: n + k,
                track,
            }
        })
        .collect();
    (edits, sel)
}

/// Units a dimension reads in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Mil,
    Mm,
}

/// A linear dimension from `a` to `b` (mil): the dimension line `offset`
/// to the left of a→b, extension lines down to the points, arrowheads, and
/// the length written above the line.
pub fn dimension(
    a: [f64; 2],
    b: [f64; 2],
    offset: f64,
    style: &TextStyle,
    units: Units,
) -> Vec<Track> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len < 1e-6 {
        return Vec::new();
    }
    let u = [d[0] / len, d[1] / len];
    let n = [-u[1], u[0]];
    let side = if offset < 0.0 { -1.0 } else { 1.0 };
    let off = |p: [f64; 2], k: f64| [p[0] + n[0] * k, p[1] + n[1] * k];
    let along = |p: [f64; 2], k: f64| [p[0] + u[0] * k, p[1] + u[1] * k];
    let (a2, b2) = (off(a, offset), off(b, offset));
    let w = Length::from_mil(style.width);
    let line = |p: [f64; 2], q: [f64; 2]| Track {
        layer: style.layer.clone(),
        net: None,
        start: pt(p),
        end: pt(q),
        width: w,
        component: None,
        text: false,
    };
    let over = side * (style.height * 0.4).max(style.width * 2.0);
    let gap = side * style.width.max(2.0);
    let mut out = vec![
        line(a2, b2),
        line(off(a, gap), off(a2, over)),
        line(off(b, gap), off(b2, over)),
    ];
    // Arrowheads, pointing out to the extension lines; outside them when
    // the dimension is too short to hold them.
    let head = (style.height * 0.6).min(len / 3.0).max(style.width * 2.0);
    let spread = head * 20f64.to_radians().tan();
    for (p, k) in [(a2, 1.0), (b2, -1.0)] {
        let base = along(p, k * head);
        out.push(line(p, off(base, spread)));
        out.push(line(p, off(base, -spread)));
    }
    let value = match units {
        Units::Mil => format!("{:.1} mil", len),
        Units::Mm => format!("{:.3} mm", len * 0.0254),
    };
    // The text reads left to right (or bottom to top), above the line.
    let mut deg = u[1].atan2(u[0]).to_degrees();
    if deg > 90.0 + 1e-6 || deg <= -90.0 + 1e-6 {
        deg += 180.0;
    }
    let mid = [(a2[0] + b2[0]) / 2.0, (a2[1] + b2[1]) / 2.0];
    // "Above" the line in the text's own frame.
    let up = [-(deg.to_radians().sin()), deg.to_radians().cos()];
    let lift = style.height * 0.35 + style.width;
    let up_side = if (up[0] * n[0] + up[1] * n[1]) * side >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let base = if up_side > 0.0 {
        [mid[0] + up[0] * lift, mid[1] + up[1] * lift]
    } else {
        let k = -(lift + style.height);
        [mid[0] + up[0] * k, mid[1] + up[1] * k]
    };
    let ts = TextStyle {
        rotation_deg: turn(deg, 0.0),
        ..style.clone()
    };
    out.extend(text(&value, base, &ts, true));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn p(x: f64, y: f64) -> Point {
        pt([x, y])
    }

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["Top", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for (k, x) in [0.0, 300.0, 1000.0].into_iter().enumerate() {
            b.components.push(Component {
                designator: format!("R{}", k + 1),
                footprint: "0603".into(),
                library_ref: None,
                x: Length::from_mil(x),
                y: Length::from_mil(k as f64 * 10.0),
                rotation_deg: 0.0,
                side: Side::Top,
                sheet: None,
                params: Default::default(),
            });
            for dx in [-30.0, 30.0] {
                b.geometry.pads.push(Pad {
                    layer: LayerRef::Copper(0),
                    net: Some(0),
                    component: Some(k as u32),
                    name: if dx < 0.0 { "1".into() } else { "2".into() },
                    at: p(x + dx, k as f64 * 10.0),
                    size_x: Length::from_mil(30.0),
                    size_y: Length::from_mil(30.0),
                    hole: Length(0),
                    shape: PadShape::Rect,
                    rotation_deg: 0.0,
                    outline: Vec::new(),
                    stack: Vec::new(),
                    fab: Default::default(),
                });
            }
        }
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: p(30.0, 0.0),
            end: p(270.0, 10.0),
            width: Length::from_mil(10.0),
            component: None,
            text: false,
        });
        b.geometry.vias.push(Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: p(500.0, 500.0),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: Vec::new(),
            fab: Default::default(),
        });
        b
    }

    fn run(b: &Board, edits: &[Edit]) -> Board {
        let mut c = b.clone();
        for e in edits {
            e.apply(&mut c);
        }
        c
    }

    #[test]
    fn box_move_rotate_and_undo() {
        let b = board();
        let sel = Sel::in_box(&b, [-100.0, -100.0, 400.0, 100.0], false);
        assert_eq!(sel.parts, BTreeSet::from([0, 1]));
        assert_eq!(sel.tracks, BTreeSet::from([0]));
        assert!(sel.vias.is_empty());
        let e = move_by(&b, &sel, 100.0, -50.0);
        let m = run(&b, &e);
        assert!((m.components[1].x.mil() - 400.0).abs() < 1e-6);
        assert!((m.geometry.tracks[0].start.x.mil() - 130.0).abs() < 1e-6);
        assert!((m.geometry.pads[0].at.y.mil() + 50.0).abs() < 1e-6);
        // Undo gives the board back.
        let mut u = m.clone();
        for x in e.iter().rev() {
            x.inverse().apply(&mut u);
        }
        assert_eq!(u.geometry.tracks[0].start, b.geometry.tracks[0].start);
        assert_eq!(u.components[1].x, b.components[1].x);
        // A quarter turn about the selection's centre.
        let r = run(&b, &rotate(&b, &sel, 90.0, Some([0.0, 0.0])));
        assert_eq!(r.components[1].rotation_deg, 90.0);
        assert!((r.components[1].y.mil() - 300.0).abs() < 1e-6);
        let touch = Sel::in_box(&b, [250.0, -5.0, 260.0, 5.0], true);
        assert_eq!(touch.parts, BTreeSet::from([1]));
    }

    #[test]
    fn align_and_spread() {
        let b = board();
        let sel = Sel {
            parts: BTreeSet::from([0, 1, 2]),
            ..Sel::default()
        };
        let t = run(&b, &align(&b, &sel, Align::Bottom));
        for c in &t.components {
            assert!(c.y.mil().abs() < 1e-6);
        }
        // Parts 0..=2 span -45..1045 with widths 90: gaps of 410.
        let s = run(&b, &align(&b, &sel, Align::SpreadX));
        assert!((s.components[1].x.mil() - 500.0).abs() < 1e-6);
        assert!((s.components[2].x.mil() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn delete_copy_paste() {
        let b = board();
        let sel = Sel {
            parts: BTreeSet::from([0]),
            vias: BTreeSet::from([0]),
            ..Sel::default()
        };
        let d = run(&b, &delete(&b, &sel));
        assert_eq!(d.components.len(), 2);
        assert_eq!(d.components[0].designator, "R2");
        assert_eq!(d.geometry.pads.len(), 4);
        assert!(d.geometry.pads.iter().all(|p| p.component != Some(2)));
        assert!(d.geometry.vias.is_empty());
        let clip = copy(&b, &sel);
        let pasted = paste(&b, &clip, [0.0, 1000.0], false);
        let q = run(&b, &pasted.edits);
        assert_eq!(q.components.len(), 4);
        assert_eq!(q.components[3].designator, "R4");
        assert_eq!(pasted.renamed, vec![("R1".to_string(), "R4".to_string())]);
        let new: Vec<_> = q
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(3))
            .collect();
        assert_eq!(new.len(), 2);
        assert!(new.iter().all(|p| p.net.is_none()));
        assert_eq!(q.geometry.vias.len(), 2);
        assert_eq!(pasted.sel.parts, BTreeSet::from([3]));
        assert_eq!(pasted.sel.vias, BTreeSet::from([1]));
        // Only vias and tracks keep their nets.
        let v = copy(
            &b,
            &Sel {
                vias: BTreeSet::from([0]),
                ..Sel::default()
            },
        );
        let pv = run(&b, &paste(&b, &v, [0.0, 0.0], true).edits);
        assert_eq!(pv.geometry.vias[1].net, Some(0));
        assert!((pv.geometry.vias[1].at.x.mil()).abs() < 1e-6);
    }

    #[test]
    fn names_and_dimensions() {
        let taken = BTreeSet::from(["R1".to_string(), "R7".to_string(), "C2".to_string()]);
        assert_eq!(next_designator(&taken, "R1"), "R8");
        assert_eq!(next_designator(&taken, "U3"), "U1");
        assert_eq!(next_designator(&taken, "TP"), "TP1");
        let st = TextStyle::on(LayerRef::Mechanical(1));
        let t = dimension([0.0, 0.0], [1000.0, 0.0], 100.0, &st, Units::Mil);
        assert!(t.iter().any(|t| t.text));
        // The dimension line itself.
        assert_eq!(t[0].start, p(0.0, 100.0));
        assert_eq!(t[0].end, p(1000.0, 100.0));
        // The label sits above the line, centred on it.
        let label: Vec<_> = t.iter().filter(|t| t.text).collect();
        let (lo, hi) = label.iter().fold((f64::MAX, f64::MIN), |a, t| {
            (a.0.min(t.start.x.mil()), a.1.max(t.start.x.mil()))
        });
        assert!(((lo + hi) / 2.0 - 500.0).abs() < 40.0, "{lo} {hi}");
        assert!(label.iter().all(|t| t.start.y.mil() > 100.0));
    }
}
