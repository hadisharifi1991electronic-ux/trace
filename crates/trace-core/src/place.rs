//! Moving, rotating and flipping parts: everything a part owns (pads,
//! silk and other primitives, its 3D model) follows its placement.

use crate::{Board, LayerRef, Length, Point, Side};

/// Where a part sits: origin, rotation (degrees counter-clockwise) and side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: Length,
    pub y: Length,
    pub rotation_deg: f64,
    pub side: Side,
}

impl Placement {
    pub fn of(board: &Board, index: usize) -> Option<Placement> {
        let c = board.components.get(index)?;
        Some(Placement {
            x: c.x,
            y: c.y,
            rotation_deg: c.rotation_deg,
            side: c.side,
        })
    }

    /// Board coordinates (mil) to the part's own frame: rotation undone,
    /// and mirrored top to bottom (y) for the bottom side, as Altium and
    /// KiCad flip parts.
    pub fn to_local(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = (-self.rotation_deg).to_radians().sin_cos();
        let (dx, dy) = (p[0] - self.x.mil(), p[1] - self.y.mil());
        let (u, v) = (dx * c - dy * s, dx * s + dy * c);
        if self.side == Side::Bottom {
            [u, -v]
        } else {
            [u, v]
        }
    }

    pub fn to_board(&self, p: [f64; 2]) -> [f64; 2] {
        let [u, v] = if self.side == Side::Bottom {
            [p[0], -p[1]]
        } else {
            p
        };
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        [self.x.mil() + u * c - v * s, self.y.mil() + u * s + v * c]
    }
}

/// The change of a part's placement from `from` to `to`, applied to points,
/// directions and layers.
pub struct Move {
    from: Placement,
    to: Placement,
    flip: bool,
    copper: u8,
}

fn mil(p: Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

fn point(p: [f64; 2]) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

impl Move {
    pub fn new(board: &Board, from: Placement, to: Placement) -> Move {
        Move {
            from,
            to,
            flip: from.side != to.side,
            copper: board.stackup.copper_layers().count().max(1) as u8,
        }
    }

    pub fn point(&self, p: Point) -> Point {
        point(self.to.to_board(self.from.to_local(mil(p))))
    }

    pub fn xy(&self, p: [f64; 2]) -> [f64; 2] {
        self.to.to_board(self.from.to_local(p))
    }

    /// A direction given as an angle in degrees.
    pub fn angle(&self, deg: f64) -> f64 {
        let r = deg.to_radians();
        let o = self.xy([0.0, 0.0]);
        let q = self.xy([r.cos(), r.sin()]);
        (q[1] - o[1])
            .atan2(q[0] - o[0])
            .to_degrees()
            .rem_euclid(360.0)
    }

    /// Whether the part changes side, so its copper goes to the other
    /// outer layer and arcs turn the other way.
    pub fn flips(&self) -> bool {
        self.flip
    }

    pub fn copper(&self, l: u8) -> u8 {
        if self.flip {
            self.copper - 1 - l.min(self.copper - 1)
        } else {
            l
        }
    }

    pub fn layer(&self, l: &LayerRef) -> LayerRef {
        if !self.flip {
            return l.clone();
        }
        match l {
            LayerRef::Copper(i) => LayerRef::Copper(self.copper(*i)),
            LayerRef::TopSilk => LayerRef::BottomSilk,
            LayerRef::BottomSilk => LayerRef::TopSilk,
            // Altium's paste (35, 36) and solder mask (37, 38) layers.
            LayerRef::Other(35) => LayerRef::Other(36),
            LayerRef::Other(36) => LayerRef::Other(35),
            LayerRef::Other(37) => LayerRef::Other(38),
            LayerRef::Other(38) => LayerRef::Other(37),
            other => other.clone(),
        }
    }
}

/// Puts part `index`, now at `from`, at `to`, with everything it owns.
pub fn place(board: &mut Board, index: usize, from: Placement, to: Placement) {
    let m = Move::new(board, from, to);
    let me = Some(index as u32);
    if let Some(c) = board.components.get_mut(index) {
        c.x = to.x;
        c.y = to.y;
        c.rotation_deg = to.rotation_deg;
        c.side = to.side;
    }
    let g = &mut board.geometry;
    for p in g.pads.iter_mut().filter(|p| p.component == me) {
        p.at = m.point(p.at);
        p.rotation_deg = m.angle(p.rotation_deg);
        p.layer = m.layer(&p.layer);
        for q in &mut p.outline {
            *q = m.point(*q);
        }
        for l in &mut p.stack {
            l.layer = m.copper(l.layer);
            l.rotation_deg = m.angle(l.rotation_deg);
            for q in &mut l.outline {
                *q = m.point(*q);
            }
        }
        if m.flips() {
            p.stack.sort_by_key(|l| l.layer);
        }
    }
    for t in g.tracks.iter_mut().filter(|t| t.component == me) {
        t.start = m.point(t.start);
        t.end = m.point(t.end);
        t.layer = m.layer(&t.layer);
    }
    for a in g.arcs.iter_mut().filter(|a| a.component == me) {
        let (s, e) = (m.angle(a.start_deg), m.angle(a.end_deg));
        // A mirror turns an arc the other way round.
        (a.start_deg, a.end_deg) = if m.flips() { (e, s) } else { (s, e) };
        a.center = m.point(a.center);
        a.layer = m.layer(&a.layer);
    }
    for f in g
        .polygons
        .iter_mut()
        .chain(g.areas.iter_mut())
        .filter(|f| f.component == me)
    {
        for q in f.outline.iter_mut().chain(f.holes.iter_mut().flatten()) {
            *q = m.point(*q);
        }
        f.layer = m.layer(&f.layer);
    }
    for b in g.bodies.iter_mut().filter(|b| b.component == me) {
        b.at = m.point(b.at);
        b.rotation_deg = m.angle(b.rotation_deg);
        if m.flips() {
            b.bottom = !b.bottom;
        }
    }
}

/// The board area (mil) a placement change touches: the part's pads where
/// they were and where they go. For pouring again.
pub fn area(board: &Board, index: usize, from: Placement, to: Placement) -> Option<[f64; 4]> {
    // The board has the part at `to`.
    let back = Move::new(board, to, from);
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in board
        .geometry
        .pads
        .iter()
        .filter(|p| p.component == Some(index as u32))
    {
        let r = p.size_x.mil().max(p.size_y.mil());
        for c in [mil(p.at), back.xy(mil(p.at))] {
            b = [
                b[0].min(c[0] - r),
                b[1].min(c[1] - r),
                b[2].max(c[0] + r),
                b[3].max(c[1] + r),
            ];
        }
    }
    (b[0] <= b[2]).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn pt(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        }
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
                material: None,
            });
        }
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "0603".into(),
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
            net: Some(0),
            component: Some(0),
            name: "1".into(),
            at: pt(130.0, 100.0),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(30.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.tracks.push(Track {
            layer: LayerRef::TopSilk,
            net: None,
            start: pt(80.0, 120.0),
            end: pt(120.0, 120.0),
            width: Length::from_mil(5.0),
            component: Some(0),
            text: false,
        });
        b
    }

    fn close(p: Point, x: f64, y: f64) -> bool {
        (p.x.mil() - x).abs() < 1e-3 && (p.y.mil() - y).abs() < 1e-3
    }

    #[test]
    fn moves_rotates_and_flips_with_its_pads_and_silk() {
        let mut b = board();
        let from = Placement::of(&b, 0).unwrap();
        let to = Placement {
            x: Length::from_mil(200.0),
            y: Length::from_mil(50.0),
            rotation_deg: 90.0,
            side: Side::Top,
        };
        place(&mut b, 0, from, to);
        let p = &b.geometry.pads[0];
        // 30 mil right of the origin, turned 90°: 30 mil up.
        assert!(close(p.at, 200.0, 80.0), "{:?}", p.at);
        assert!((p.rotation_deg - 90.0).abs() < 1e-6);
        let t = &b.geometry.tracks[0];
        assert!(
            close(t.start, 180.0, 30.0) && close(t.end, 180.0, 70.0),
            "{t:?}"
        );
        // To the bottom: mirrored (top to bottom, then a half turn: left to
        // right), on the bottom layers.
        let flipped = Placement {
            side: Side::Bottom,
            rotation_deg: 180.0,
            ..to
        };
        place(&mut b, 0, to, flipped);
        let p = &b.geometry.pads[0];
        assert!(close(p.at, 170.0, 50.0), "{:?}", p.at);
        assert_eq!(p.layer, LayerRef::Copper(1));
        assert_eq!(b.geometry.tracks[0].layer, LayerRef::BottomSilk);
        assert_eq!(b.components[0].side, Side::Bottom);
        // And back where it started.
        place(&mut b, 0, flipped, from);
        let p = &b.geometry.pads[0];
        assert!(close(p.at, 130.0, 100.0) && p.layer == LayerRef::Copper(0));
        let t = &b.geometry.tracks[0];
        assert!(close(t.start, 80.0, 120.0) && t.layer == LayerRef::TopSilk);
    }
}
