//! Reversible changes to a board, and an undo history built from them.

use crate::place::Placement;
use crate::{Arc, Board, Component, Length, Net, Pad, Point, Polygon, Pour, Track, Via};

/// The board's parts and their primitives, as a whole: what an edit that
/// changes footprints swaps out and back.
#[derive(Clone, Debug, Default)]
pub struct Prims {
    pub pads: Vec<Pad>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub polygons: Vec<Polygon>,
    pub areas: Vec<Polygon>,
    pub cutouts: Vec<Polygon>,
    pub components: Vec<Component>,
    /// The parts' 3D bodies, when the edit changes them; `None` leaves the
    /// board's as they are.
    pub bodies: Option<Vec<crate::Body3d>>,
}

impl Prims {
    pub fn of(board: &Board) -> Prims {
        let g = &board.geometry;
        Prims {
            pads: g.pads.clone(),
            tracks: g.tracks.clone(),
            arcs: g.arcs.clone(),
            polygons: g.polygons.clone(),
            areas: g.areas.clone(),
            cutouts: g.cutouts.clone(),
            components: board.components.clone(),
            bodies: Some(g.bodies.clone()),
        }
    }

    fn put(&self, board: &mut Board) {
        let g = &mut board.geometry;
        g.pads = self.pads.clone();
        g.tracks = self.tracks.clone();
        g.arcs = self.arcs.clone();
        g.polygons = self.polygons.clone();
        g.areas = self.areas.clone();
        g.cutouts = self.cutouts.clone();
        if let Some(b) = &self.bodies {
            g.bodies = b.clone();
        }
        board.components = self.components.clone();
    }
}

/// The board's drawn shapes: pour outlines, copper areas (poured copper,
/// fixed regions, keepouts), pour cutouts and the board outline. What
/// drawing or editing a shape swaps out and back.
#[derive(Clone, Debug, Default)]
pub struct Shapes {
    pub pours: Vec<Pour>,
    pub polygons: Vec<Polygon>,
    pub cutouts: Vec<Polygon>,
    pub outline: Vec<Point>,
}

impl Shapes {
    pub fn of(board: &Board) -> Shapes {
        let g = &board.geometry;
        Shapes {
            pours: g.pours.clone(),
            polygons: g.polygons.clone(),
            cutouts: g.cutouts.clone(),
            outline: g.outline.clone(),
        }
    }

    fn put(&self, board: &mut Board) {
        let g = &mut board.geometry;
        g.pours = self.pours.clone();
        g.polygons = self.polygons.clone();
        g.cutouts = self.cutouts.clone();
        g.outline = self.outline.clone();
    }
}

#[derive(Clone, Debug)]
pub enum Edit {
    /// Moves a part and its pads. Tracks stay where they are.
    MoveComponent {
        index: usize,
        dx: Length,
        dy: Length,
    },
    /// Moves, turns or flips a part with everything it owns.
    PlaceComponent {
        index: usize,
        from: Placement,
        to: Placement,
    },
    InsertTrack {
        index: usize,
        track: Track,
    },
    RemoveTrack {
        index: usize,
        track: Track,
    },
    InsertArc {
        index: usize,
        arc: Arc,
    },
    RemoveArc {
        index: usize,
        arc: Arc,
    },
    InsertVia {
        index: usize,
        via: Via,
    },
    RemoveVia {
        index: usize,
        via: Via,
    },
    /// Replaces every copper area, as pouring again does.
    SetPolygons {
        before: Vec<Polygon>,
        after: Vec<Polygon>,
    },
    /// Swaps the board's shapes (see `Shapes`). `area` (mil) is where
    /// copper changed, for pouring again.
    Shapes {
        before: Box<Shapes>,
        after: Box<Shapes>,
        area: Option<[f64; 4]>,
    },
    /// Changes from the schematic: the net list, which net each pad is on,
    /// and part designators.
    Netlist {
        nets_before: Vec<Net>,
        nets_after: Vec<Net>,
        /// (pad, net before, net after).
        pads: Vec<(usize, Option<u32>, Option<u32>)>,
        /// (part, designator before, designator after).
        names: Vec<(usize, String, String)>,
    },
    /// Changes the fabrication details of pads and vias (test points,
    /// tenting): (index, before, after).
    Fab {
        pads: Vec<(usize, crate::Fab, crate::Fab)>,
        vias: Vec<(usize, crate::Fab, crate::Fab)>,
    },
    /// Swaps every part and primitive list, as changing footprints does.
    /// `area` (mil) is where copper changed, for pouring again.
    Parts {
        before: Box<Prims>,
        after: Box<Prims>,
        area: Option<[f64; 4]>,
    },
}

fn insert<T: Clone>(list: &mut Vec<T>, index: usize, item: &T) {
    list.insert(index.min(list.len()), item.clone());
}

fn remove<T>(list: &mut Vec<T>, index: usize) {
    if index < list.len() {
        list.remove(index);
    }
}

impl Edit {
    pub fn apply(&self, board: &mut Board) {
        match self {
            Edit::MoveComponent { index, dx, dy } => {
                if let Some(c) = board.components.get_mut(*index) {
                    c.x.0 += dx.0;
                    c.y.0 += dy.0;
                }
                for p in board
                    .geometry
                    .pads
                    .iter_mut()
                    .filter(|p| p.component == Some(*index as u32))
                {
                    p.at.x.0 += dx.0;
                    p.at.y.0 += dy.0;
                }
            }
            Edit::PlaceComponent { index, from, to } => {
                crate::place::place(board, *index, *from, *to)
            }
            Edit::InsertTrack { index, track } => insert(&mut board.geometry.tracks, *index, track),
            Edit::RemoveTrack { index, .. } => remove(&mut board.geometry.tracks, *index),
            Edit::InsertArc { index, arc } => insert(&mut board.geometry.arcs, *index, arc),
            Edit::RemoveArc { index, .. } => remove(&mut board.geometry.arcs, *index),
            Edit::InsertVia { index, via } => insert(&mut board.geometry.vias, *index, via),
            Edit::RemoveVia { index, .. } => remove(&mut board.geometry.vias, *index),
            Edit::SetPolygons { after, .. } => board.geometry.polygons = after.clone(),
            Edit::Parts { after, .. } => after.put(board),
            Edit::Fab { pads, vias } => {
                for &(i, _, f) in pads {
                    if let Some(p) = board.geometry.pads.get_mut(i) {
                        p.fab = f;
                    }
                }
                for &(i, _, f) in vias {
                    if let Some(v) = board.geometry.vias.get_mut(i) {
                        v.fab = f;
                    }
                }
            }
            Edit::Shapes { after, .. } => after.put(board),
            Edit::Netlist {
                nets_after,
                pads,
                names,
                ..
            } => {
                board.nets = nets_after.clone();
                for &(i, _, n) in pads {
                    if let Some(p) = board.geometry.pads.get_mut(i) {
                        p.net = n;
                    }
                }
                for (i, _, name) in names {
                    if let Some(c) = board.components.get_mut(*i) {
                        c.designator = name.clone();
                    }
                }
                // Variants name parts by designator: they follow.
                let renames: std::collections::BTreeMap<String, String> = names
                    .iter()
                    .filter(|(_, a, b)| a != b)
                    .map(|(_, a, b)| (a.clone(), b.clone()))
                    .collect();
                if !renames.is_empty() {
                    crate::variant::rename(board, &renames);
                }
            }
        }
    }

    pub fn inverse(&self) -> Edit {
        match self {
            Edit::MoveComponent { index, dx, dy } => Edit::MoveComponent {
                index: *index,
                dx: Length(-dx.0),
                dy: Length(-dy.0),
            },
            Edit::PlaceComponent { index, from, to } => Edit::PlaceComponent {
                index: *index,
                from: *to,
                to: *from,
            },
            Edit::InsertTrack { index, track } => Edit::RemoveTrack {
                index: *index,
                track: track.clone(),
            },
            Edit::RemoveTrack { index, track } => Edit::InsertTrack {
                index: *index,
                track: track.clone(),
            },
            Edit::InsertArc { index, arc } => Edit::RemoveArc {
                index: *index,
                arc: arc.clone(),
            },
            Edit::RemoveArc { index, arc } => Edit::InsertArc {
                index: *index,
                arc: arc.clone(),
            },
            Edit::InsertVia { index, via } => Edit::RemoveVia {
                index: *index,
                via: via.clone(),
            },
            Edit::RemoveVia { index, via } => Edit::InsertVia {
                index: *index,
                via: via.clone(),
            },
            Edit::SetPolygons { before, after } => Edit::SetPolygons {
                before: after.clone(),
                after: before.clone(),
            },
            Edit::Parts {
                before,
                after,
                area,
            } => Edit::Parts {
                before: after.clone(),
                after: before.clone(),
                area: *area,
            },
            Edit::Shapes {
                before,
                after,
                area,
            } => Edit::Shapes {
                before: after.clone(),
                after: before.clone(),
                area: *area,
            },
            Edit::Fab { pads, vias } => Edit::Fab {
                pads: pads.iter().map(|&(i, a, b)| (i, b, a)).collect(),
                vias: vias.iter().map(|&(i, a, b)| (i, b, a)).collect(),
            },
            Edit::Netlist {
                nets_before,
                nets_after,
                pads,
                names,
            } => Edit::Netlist {
                nets_before: nets_after.clone(),
                nets_after: nets_before.clone(),
                pads: pads.iter().map(|&(i, a, b)| (i, b, a)).collect(),
                names: names
                    .iter()
                    .map(|(i, a, b)| (*i, b.clone(), a.clone()))
                    .collect(),
            },
        }
    }
}

/// Undo and redo stacks. Each step is a group of edits applied in order.
#[derive(Default)]
pub struct History {
    undo: Vec<(String, Vec<Edit>)>,
    redo: Vec<(String, Vec<Edit>)>,
}

impl History {
    /// Applies a group of edits as one undoable step.
    pub fn apply(&mut self, board: &mut Board, label: impl Into<String>, edits: Vec<Edit>) {
        if edits.is_empty() {
            return;
        }
        for e in &edits {
            e.apply(board);
        }
        self.undo.push((label.into(), edits));
        self.redo.clear();
    }

    /// Applies one more edit as part of the last step, so one undo takes
    /// both back.
    pub fn append(&mut self, board: &mut Board, edit: Edit) {
        edit.apply(board);
        if let Some((_, edits)) = self.undo.last_mut() {
            edits.push(edit);
        }
    }

    /// Joins the last step into the one before it, which keeps its label.
    pub fn merge_last(&mut self) {
        if self.undo.len() >= 2 {
            let (_, edits) = self.undo.pop().unwrap();
            if let Some((_, before)) = self.undo.last_mut() {
                before.extend(edits);
            }
        }
    }

    pub fn undo(&mut self, board: &mut Board) -> Option<String> {
        let (label, edits) = self.undo.pop()?;
        for e in edits.iter().rev() {
            e.inverse().apply(board);
        }
        self.redo.push((label.clone(), edits));
        Some(label)
    }

    pub fn redo(&mut self, board: &mut Board) -> Option<String> {
        let (label, edits) = self.redo.pop()?;
        for e in &edits {
            e.apply(board);
        }
        self.undo.push((label.clone(), edits));
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn board() -> Board {
        let mut b = Board::default();
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "0402".into(),
            library_ref: None,
            x: Length::from_mil(100.0),
            y: Length::from_mil(100.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        b.geometry.pads.push(Pad {
            layer: LayerRef::Copper(0),
            net: None,
            component: Some(0),
            name: "1".into(),
            at: Point {
                x: Length::from_mil(90.0),
                y: Length::from_mil(100.0),
            },
            size_x: Length::from_mil(10.0),
            size_y: Length::from_mil(10.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b
    }

    fn track(x: f64) -> Track {
        Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: None,
            start: Point::default(),
            end: Point {
                x: Length::from_mil(x),
                y: Length(0),
            },
            width: Length::from_mil(5.0),
            text: false,
        }
    }

    #[test]
    fn move_component_takes_pads_and_undoes() {
        let mut b = board();
        let mut h = History::default();
        h.apply(
            &mut b,
            "Move R1",
            vec![Edit::MoveComponent {
                index: 0,
                dx: Length::from_mil(10.0),
                dy: Length::from_mil(-5.0),
            }],
        );
        assert_eq!(b.components[0].x, Length::from_mil(110.0));
        assert_eq!(b.geometry.pads[0].at.y, Length::from_mil(95.0));
        assert_eq!(h.undo(&mut b).as_deref(), Some("Move R1"));
        assert_eq!(b.geometry.pads[0].at.x, Length::from_mil(90.0));
        h.redo(&mut b);
        assert_eq!(b.geometry.pads[0].at.x, Length::from_mil(100.0));
    }

    #[test]
    fn track_groups_undo_in_reverse() {
        let mut b = board();
        b.geometry.tracks = vec![track(1.0), track(2.0), track(3.0)];
        let mut h = History::default();
        h.apply(
            &mut b,
            "Delete",
            vec![Edit::RemoveTrack {
                index: 1,
                track: track(2.0),
            }],
        );
        h.apply(
            &mut b,
            "Route",
            vec![
                Edit::InsertTrack {
                    index: 2,
                    track: track(9.0),
                },
                Edit::InsertTrack {
                    index: 3,
                    track: track(8.0),
                },
            ],
        );
        assert_eq!(b.geometry.tracks.len(), 4);
        h.undo(&mut b);
        h.undo(&mut b);
        let xs: Vec<f64> = b.geometry.tracks.iter().map(|t| t.end.x.mil()).collect();
        assert_eq!(xs, vec![1.0, 2.0, 3.0]);
        assert!(h.undo(&mut b).is_none());
    }
}
