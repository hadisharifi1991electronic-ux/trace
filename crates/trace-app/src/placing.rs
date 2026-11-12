//! Moving a part with the mouse, as in Altium: press on it and drag;
//! Space turns it 90°, L flips it to the other side, Escape puts it back.
//! Tracks that end on its pads stretch to follow, and a ratsnest shows
//! where its unrouted pads connect.

use std::collections::HashMap;
use trace_core::edit::Edit;
use trace_core::place::{Move, Placement};
use trace_core::{Board, LayerRef, Length, Point, Side, Track};

type P = [f64; 2];

fn xy(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

pub struct Moving {
    pub index: usize,
    pub from: Placement,
    pub to: Placement,
    /// The point of the part under the cursor, in the part's own frame:
    /// it stays under the cursor as the part turns and flips.
    grab: P,
    /// Board tracks with an end on one of the part's pads: track, whether
    /// it is the start, and the pad.
    pub ends: Vec<(usize, bool, usize)>,
    /// Copper of each of the part's nets outside it, for the ratsnest.
    targets: HashMap<u32, Vec<P>>,
}

/// Whether pad `p` has copper on copper layer `l`.
fn on_layer(p: &trace_core::Pad, l: u8) -> bool {
    match p.layer {
        LayerRef::AllCopper => p.stack.is_empty() || p.stack.iter().any(|s| s.layer == l),
        LayerRef::Copper(i) => i == l,
        _ => false,
    }
}

impl Moving {
    pub fn start(board: &Board, index: usize, cursor: P) -> Option<Moving> {
        let from = Placement::of(board, index)?;
        let me = Some(index as u32);
        let g = &board.geometry;
        let pads: Vec<usize> = (0..g.pads.len())
            .filter(|&i| g.pads[i].component == me)
            .collect();
        let mut ends = Vec::new();
        for (ti, t) in g.tracks.iter().enumerate() {
            let LayerRef::Copper(l) = t.layer else {
                continue;
            };
            if t.component.is_some() {
                continue;
            }
            for (start, e) in [(true, t.start), (false, t.end)] {
                let e = xy(e);
                if let Some(&pi) = pads.iter().find(|&&pi| {
                    let p = &g.pads[pi];
                    let c = xy(p.at);
                    p.net == t.net && on_layer(p, l) && (c[0] - e[0]).hypot(c[1] - e[1]) < 0.5
                }) {
                    ends.push((ti, start, pi));
                }
            }
        }
        let nets: Vec<u32> = pads.iter().filter_map(|&i| g.pads[i].net).collect();
        let mut targets: HashMap<u32, Vec<P>> = HashMap::new();
        let mut add = |net: Option<u32>, p: P| {
            if let Some(n) = net.filter(|n| nets.contains(n)) {
                targets.entry(n).or_default().push(p);
            }
        };
        for p in g.pads.iter().filter(|p| p.component != me) {
            add(p.net, xy(p.at));
        }
        for v in &g.vias {
            add(v.net, xy(v.at));
        }
        for (ti, t) in g.tracks.iter().enumerate() {
            if !ends.iter().any(|e| e.0 == ti) {
                add(t.net, xy(t.start));
                add(t.net, xy(t.end));
            }
        }
        Some(Moving {
            index,
            from,
            to: from,
            grab: from.to_local(cursor),
            ends,
            targets,
        })
    }

    /// Keeps the grabbed point under the cursor.
    pub fn follow(&mut self, cursor: P) {
        let frame = Placement {
            x: Length(0),
            y: Length(0),
            ..self.to
        };
        let d = frame.to_board(self.grab);
        self.to.x = Length::from_mil(cursor[0] - d[0]);
        self.to.y = Length::from_mil(cursor[1] - d[1]);
    }

    /// Turns the part 90° counter-clockwise about the cursor.
    pub fn rotate(&mut self, cursor: P) {
        self.to.rotation_deg = (self.to.rotation_deg + 90.0).rem_euclid(360.0);
        self.follow(cursor);
    }

    /// Flips the part to the other side of the board, about the cursor,
    /// mirrored left to right (a flip top to bottom and a half turn).
    pub fn flip(&mut self, cursor: P) {
        self.to.side = match self.to.side {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        };
        self.to.rotation_deg = (self.to.rotation_deg + 180.0).rem_euclid(360.0);
        self.follow(cursor);
    }

    pub fn moved(&self) -> bool {
        self.to != self.from
    }

    pub fn transform(&self, board: &Board) -> Move {
        Move::new(board, self.from, self.to)
    }

    /// The tracks that stretch to stay on the part's pads, as they would
    /// be: index and new track. A pad that changes layer (a surface pad
    /// flipped to the other side) leaves its track where it is.
    pub fn stretched(&self, board: &Board) -> Vec<(usize, Track)> {
        let m = self.transform(board);
        let g = &board.geometry;
        let mut out: Vec<(usize, Track)> = Vec::new();
        for &(ti, start, pi) in &self.ends {
            let (t, p) = (&g.tracks[ti], &g.pads[pi]);
            let LayerRef::Copper(l) = t.layer else {
                continue;
            };
            let mut moved = p.clone();
            moved.layer = m.layer(&p.layer);
            for s in &mut moved.stack {
                s.layer = m.copper(s.layer);
            }
            if !on_layer(&moved, l) {
                continue;
            }
            let at = m.point(p.at);
            // Both ends of one track can sit on the part.
            let i = match out.iter().position(|(i, _)| *i == ti) {
                Some(i) => i,
                None => {
                    out.push((ti, t.clone()));
                    out.len() - 1
                }
            };
            if start {
                out[i].1.start = at;
            } else {
                out[i].1.end = at;
            }
        }
        out
    }

    /// From each of the part's pads that no track reaches, a line to the
    /// nearest copper of its net, where the pad will be.
    pub fn ratsnest(&self, board: &Board) -> Vec<(P, P)> {
        let m = self.transform(board);
        let g = &board.geometry;
        let me = Some(self.index as u32);
        let mut out = Vec::new();
        for (pi, p) in g.pads.iter().enumerate() {
            if p.component != me || self.ends.iter().any(|e| e.2 == pi) {
                continue;
            }
            let Some(list) = p.net.and_then(|n| self.targets.get(&n)) else {
                continue;
            };
            let at = m.xy(xy(p.at));
            let near = list.iter().min_by(|a, b| {
                let da = (a[0] - at[0]).hypot(a[1] - at[1]);
                let db = (b[0] - at[0]).hypot(b[1] - at[1]);
                da.total_cmp(&db)
            });
            if let Some(&q) = near {
                out.push((at, q));
            }
        }
        out
    }

    /// The edits that place the part where it is now, with the tracks
    /// that follow it.
    pub fn edits(&self, board: &Board) -> Vec<Edit> {
        let mut out = vec![Edit::PlaceComponent {
            index: self.index,
            from: self.from,
            to: self.to,
        }];
        for (i, t) in self.stretched(board) {
            out.push(Edit::RemoveTrack {
                index: i,
                track: board.geometry.tracks[i].clone(),
            });
            out.push(Edit::InsertTrack { index: i, track: t });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    fn pt(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        }
    }

    #[test]
    fn a_track_on_a_moved_pad_stretches_and_undo_puts_it_back() {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "Top".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            material: None,
        });
        b.components.push(Component {
            designator: "R1".into(),
            footprint: String::new(),
            library_ref: None,
            x: Length::from_mil(0.0),
            y: Length::from_mil(0.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        b.geometry.pads.push(Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: Some(0),
            name: "1".into(),
            at: pt(10.0, 0.0),
            size_x: Length::from_mil(10.0),
            size_y: Length::from_mil(10.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: Some(0),
            start: pt(10.0, 0.0),
            end: pt(200.0, 0.0),
            width: Length::from_mil(6.0),
            component: None,
            text: false,
        });
        let mut m = Moving::start(&b, 0, [0.0, 0.0]).unwrap();
        assert_eq!(m.ends, vec![(0, true, 0)]);
        m.follow([50.0, 20.0]);
        let mut h = trace_core::edit::History::default();
        let edits = m.edits(&b);
        h.apply(&mut b, "Moved R1", edits);
        let t = &b.geometry.tracks[0];
        assert_eq!((t.start, t.end), (pt(60.0, 20.0), pt(200.0, 0.0)));
        assert_eq!(b.geometry.pads[0].at, pt(60.0, 20.0));
        h.undo(&mut b);
        assert_eq!(b.geometry.tracks[0].start, pt(10.0, 0.0));
        assert_eq!(b.geometry.pads[0].at, pt(10.0, 0.0));
    }
}
