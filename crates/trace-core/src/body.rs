//! Part bodies estimated from their pads: a box over each part, as tall as
//! parts of its kind usually are. Used where no 3D model is at hand: the
//! STEP export and the 3D clearance check.

use crate::{Board, LayerRef, Side};

/// One part's body: a box standing on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct PartBody {
    pub component: usize,
    /// The footprint's corners on the board (mm), counter-clockwise.
    pub corners: [[f64; 2]; 4],
    /// Height above the board surface it stands on (mm).
    pub height: f64,
    pub bottom: bool,
    /// Typical colour of such a body.
    pub colour: [u8; 3],
}

impl PartBody {
    /// Its z range (mm) on a board `t` thick with the bottom face at 0.
    pub fn z(&self, t: f64) -> (f64, f64) {
        if self.bottom {
            (-self.height, 0.0)
        } else {
            (t, t + self.height)
        }
    }
}

/// Designator prefixes of things that have no body: fiducials, test
/// points, holes, logos, jumpers and net ties.
pub fn bodiless(designator: &str) -> bool {
    let p: String = designator
        .trim()
        .to_ascii_uppercase()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    matches!(
        p.as_str(),
        "FID" | "FD" | "TP" | "MH" | "H" | "MK" | "LOGO" | "J_TP" | "SJ" | "JP" | "NT"
    )
}

/// A body for each part with pads, except those that have none.
pub fn part_bodies(board: &Board) -> Vec<PartBody> {
    let g = &board.geometry;
    let last = board.stackup.copper_layers().count().max(1) as u8 - 1;
    let mut pads_of: Vec<Vec<usize>> = vec![Vec::new(); board.components.len()];
    for (i, p) in g.pads.iter().enumerate() {
        if let Some(c) = p.component.and_then(|c| pads_of.get_mut(c as usize)) {
            c.push(i);
        }
    }
    let mut out = Vec::new();
    for (ci, comp) in board.components.iter().enumerate() {
        let pads = &pads_of[ci];
        if pads.is_empty() || bodiless(&comp.designator) {
            continue;
        }
        let prefix: String = comp
            .designator
            .to_ascii_uppercase()
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let rot = comp.rotation_deg;
        let (s, c) = (-rot.to_radians()).sin_cos();
        // Pad extents in the part's own frame (mm).
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        let mut centres = Vec::with_capacity(pads.len());
        let mut through = false;
        let mut bottom_only = last > 0;
        for &pi in pads {
            let p = &g.pads[pi];
            let (x, y) = (p.at.x.mm(), p.at.y.mm());
            let (u, v) = (x * c - y * s, x * s + y * c);
            let rel = (p.rotation_deg - rot).rem_euclid(180.0);
            let (mut hx, mut hy) = (p.size_x.mm() / 2.0, p.size_y.mm() / 2.0);
            if (rel - 90.0).abs() < 30.0 {
                std::mem::swap(&mut hx, &mut hy);
            } else if (30.0..=150.0).contains(&rel) {
                let m = hx.max(hy);
                hx = m;
                hy = m;
            }
            lo = [lo[0].min(u - hx), lo[1].min(v - hy)];
            hi = [hi[0].max(u + hx), hi[1].max(v + hy)];
            centres.push([u, v, hx.max(hy) * 2.0]);
            through |= p.hole.0 > 0;
            if p.layer != LayerRef::Copper(last) {
                bottom_only = false;
            }
        }
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        if !(w > 0.1 && h > 0.1) || w.max(h) > 120.0 {
            continue;
        }
        let bottom = comp.side == Side::Bottom || (bottom_only && !through);
        let (box_lo, box_hi, height);
        let mut colour = [28, 28, 30];
        if pads.len() <= 3 && !through {
            // Chip parts: the body spans the terminals, a little narrower.
            let (sx, sy) = if w >= h {
                (0.0, h * 0.06)
            } else {
                (w * 0.06, 0.0)
            };
            box_lo = [lo[0] + sx, lo[1] + sy];
            box_hi = [hi[0] - sx, hi[1] - sy];
            height = (w.min(h) * 0.55).clamp(0.25, 2.5);
            colour = match prefix.as_str() {
                "C" => [196, 160, 110],
                "L" | "FB" => [70, 70, 74],
                "D" | "LED" => [40, 40, 44],
                _ => [24, 24, 26],
            };
        } else {
            // Leads around the edge (SOIC, QFP) sit outside the body; an
            // array of pads (BGA, QFN centre pad) sits under it.
            let inner = centres.iter().any(|q| {
                q[0] > lo[0] + w * 0.3
                    && q[0] < hi[0] - w * 0.3
                    && q[1] > lo[1] + h * 0.3
                    && q[1] < hi[1] - h * 0.3
            });
            let lead = centres.iter().map(|q| q[2]).fold(0.0f64, f64::max);
            if through {
                box_lo = [lo[0] - 0.3, lo[1] - 0.3];
                box_hi = [hi[0] + 0.3, hi[1] + 0.3];
            } else if inner || pads.len() > 64 {
                box_lo = [lo[0] - 0.2, lo[1] - 0.2];
                box_hi = [hi[0] + 0.2, hi[1] + 0.2];
            } else {
                let i = (lead * 0.7).min(w.min(h) * 0.25);
                let on_x = centres
                    .iter()
                    .any(|q| (q[0] - lo[0]).abs() < lead || (hi[0] - q[0]).abs() < lead);
                let on_y = centres
                    .iter()
                    .any(|q| (q[1] - lo[1]).abs() < lead || (hi[1] - q[1]).abs() < lead);
                let ix = if on_x { i } else { 0.0 };
                let iy = if on_y { i } else { 0.0 };
                box_lo = [lo[0] + ix, lo[1] + iy];
                box_hi = [hi[0] - ix, hi[1] - iy];
            }
            let area = (box_hi[0] - box_lo[0]) * (box_hi[1] - box_lo[1]);
            height = if through {
                (area.sqrt() * 0.35).clamp(2.0, 8.0)
            } else {
                (0.6 + area.sqrt() * 0.06).clamp(0.6, 2.6)
            };
            if matches!(prefix.as_str(), "J" | "P" | "CN" | "CON" | "X") {
                colour = [205, 198, 180];
            }
        }
        let back = |u: f64, v: f64| [u * c + v * s, -u * s + v * c];
        out.push(PartBody {
            component: ci,
            corners: [
                back(box_lo[0], box_lo[1]),
                back(box_hi[0], box_lo[1]),
                back(box_hi[0], box_hi[1]),
                back(box_lo[0], box_hi[1]),
            ],
            height,
            bottom,
            colour,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn boxes_a_chip_resistor() {
        let mut b = Board::default();
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "0603".into(),
            library_ref: None,
            x: Length::from_mm(10.0),
            y: Length::from_mm(10.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        b.components.push(Component {
            designator: "TP1".into(),
            ..b.components[0].clone()
        });
        for (c, x) in [(0, 9.2), (0, 10.8), (1, 20.0)] {
            b.geometry.pads.push(Pad {
                layer: LayerRef::Copper(0),
                net: None,
                component: Some(c),
                name: "1".into(),
                at: Point::mil(x / 0.0254, 10.0 / 0.0254),
                size_x: Length::from_mm(0.8),
                size_y: Length::from_mm(0.9),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 0.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            });
        }
        let v = part_bodies(&b);
        assert_eq!(v.len(), 1);
        let r = &v[0];
        assert!((r.corners[0][0] - 8.8).abs() < 1e-9);
        assert!((r.corners[2][0] - 11.2).abs() < 1e-9);
        assert!(r.height > 0.4 && r.height < 0.6);
        assert_eq!(r.z(1.6), (1.6, 1.6 + r.height));
    }
}
