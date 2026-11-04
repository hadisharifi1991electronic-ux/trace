//! Reversible changes to a board, and an undo history built from them.

use crate::{Arc, Board, Length, Track, Via};

#[derive(Clone, Debug)]
pub enum Edit {
    /// Moves a part and its pads. Tracks stay where they are.
    MoveComponent {
        index: usize,
        dx: Length,
        dy: Length,
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
            Edit::InsertTrack { index, track } => insert(&mut board.geometry.tracks, *index, track),
            Edit::RemoveTrack { index, .. } => remove(&mut board.geometry.tracks, *index),
            Edit::InsertArc { index, arc } => insert(&mut board.geometry.arcs, *index, arc),
            Edit::RemoveArc { index, .. } => remove(&mut board.geometry.arcs, *index),
            Edit::InsertVia { index, via } => insert(&mut board.geometry.vias, *index, via),
            Edit::RemoveVia { index, .. } => remove(&mut board.geometry.vias, *index),
        }
    }

    pub fn inverse(&self) -> Edit {
        match self {
            Edit::MoveComponent { index, dx, dy } => Edit::MoveComponent {
                index: *index,
                dx: Length(-dx.0),
                dy: Length(-dy.0),
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
        });
        b
    }

    fn track(x: f64) -> Track {
        Track {
            layer: LayerRef::Copper(0),
            net: None,
            start: Point::default(),
            end: Point {
                x: Length::from_mil(x),
                y: Length(0),
            },
            width: Length::from_mil(5.0),
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
