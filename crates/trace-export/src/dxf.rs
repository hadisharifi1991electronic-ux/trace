//! DXF export: every layer of the board as a DXF layer (AutoCAD R12, which
//! every CAD and laser or CNC tool reads), in millimetres. Tracks are
//! polylines with their width, arcs carry bulges, pads and vias are their
//! outlines, copper areas their outline and holes, and drill holes are
//! circles on their own layers.

use crate::fab::{
    mechanical_layers, plated, plot_copper, plot_mask, plot_other, plot_paste, Ap, FabOptions, Plot,
};
use std::fmt::Write;
use trace_core::{Board, LayerRef, Point, Side};

struct Dxf {
    layer: String,
    out: String,
    count: usize,
}

fn mm(v: i64) -> f64 {
    v as f64 / 1e6
}

impl Dxf {
    fn circle(&mut self, c: [f64; 2], r: f64) {
        let _ = write!(
            self.out,
            "0\nCIRCLE\n8\n{}\n10\n{:.4}\n20\n{:.4}\n30\n0.0\n40\n{:.4}\n",
            self.layer, c[0], c[1], r
        );
        self.count += 1;
    }

    /// A polyline through (point, bulge to the next point), `width` wide.
    fn poly(&mut self, pts: &[([f64; 2], f64)], closed: bool, width: f64) {
        if pts.len() < 2 {
            return;
        }
        let _ = write!(
            self.out,
            "0\nPOLYLINE\n8\n{}\n66\n1\n10\n0.0\n20\n0.0\n30\n0.0\n70\n{}\n40\n{:.4}\n41\n{:.4}\n",
            self.layer,
            if closed { 1 } else { 0 },
            width,
            width
        );
        for (p, bulge) in pts {
            let _ = write!(
                self.out,
                "0\nVERTEX\n8\n{}\n10\n{:.4}\n20\n{:.4}\n30\n0.0\n",
                self.layer, p[0], p[1]
            );
            if bulge.abs() > 1e-9 {
                let _ = write!(self.out, "42\n{bulge:.6}\n");
            }
        }
        let _ = write!(self.out, "0\nSEQEND\n8\n{}\n", self.layer);
        self.count += 1;
    }

    fn ring(&mut self, pts: &[Point]) {
        let v: Vec<([f64; 2], f64)> = pts.iter().map(|q| ([mm(q.x.0), mm(q.y.0)], 0.0)).collect();
        self.poly(&v, true, 0.0);
    }
}

impl Plot for Dxf {
    fn attrs(&mut self, _net: &str, _pin: &str) {}

    fn flash(&mut self, ap: Ap, _function: &'static str, at: Point) {
        let (x, y) = (mm(at.x.0), mm(at.y.0));
        let p = |dx: i64, dy: i64| [x + mm(dx), y + mm(dy)];
        match ap {
            Ap::Circle(d) => self.circle([x, y], mm(d) / 2.0),
            Ap::Rect(w, h) => {
                let (a, b) = (w / 2, h / 2);
                let v = [p(-a, -b), p(a, -b), p(a, b), p(-a, b)].map(|q| (q, 0.0));
                self.poly(&v, true, 0.0);
            }
            Ap::Obround(w, h) => {
                // Two straight sides joined by half circles (bulge 1).
                let v = if w >= h {
                    let (a, r) = ((w - h) / 2, h / 2);
                    [
                        (p(-a, -r), 0.0),
                        (p(a, -r), 1.0),
                        (p(a, r), 0.0),
                        (p(-a, r), 1.0),
                    ]
                } else {
                    let (a, r) = ((h - w) / 2, w / 2);
                    [
                        (p(r, -a), 0.0),
                        (p(r, a), 1.0),
                        (p(-r, a), 0.0),
                        (p(-r, -a), 1.0),
                    ]
                };
                self.poly(&v, true, 0.0);
            }
            Ap::Outline(pts, _) => {
                let v: Vec<([f64; 2], f64)> =
                    pts.iter().map(|&(dx, dy)| (p(dx, dy), 0.0)).collect();
                self.poly(&v, true, 0.0);
            }
            Ap::Bar(a, b, d) => {
                self.poly(&[(p(a.0, a.1), 0.0), (p(b.0, b.1), 0.0)], false, mm(d));
            }
        }
    }

    fn line(&mut self, d: i64, _function: &'static str, a: Point, b: Point) {
        self.poly(
            &[([mm(a.x.0), mm(a.y.0)], 0.0), ([mm(b.x.0), mm(b.y.0)], 0.0)],
            false,
            mm(d),
        );
    }

    fn arc(&mut self, d: i64, _function: &'static str, a: &trace_core::Arc) {
        let (c, r) = ([a.center.x.mm(), a.center.y.mm()], a.radius.mm());
        let mut sweep = a.end_deg - a.start_deg;
        if sweep <= 0.0 {
            sweep += 360.0;
        }
        let at = |deg: f64| {
            let t = deg.to_radians();
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        };
        if sweep >= 359.999 {
            // A full circle: two halves.
            let b = 1.0;
            self.poly(
                &[(at(a.start_deg), b), (at(a.start_deg + 180.0), b)],
                true,
                mm(d),
            );
            return;
        }
        let bulge = (sweep.to_radians() / 4.0).tan();
        self.poly(
            &[(at(a.start_deg), bulge), (at(a.end_deg), 0.0)],
            false,
            mm(d),
        );
    }

    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        self.ring(outline);
        for h in holes {
            self.ring(h);
        }
    }
}

/// A DXF layer name: letters, digits, spaces, `-` and `_` only.
fn layer_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() {
        "Layer".into()
    } else {
        t
    }
}

/// The board as one DXF file, a DXF layer per board layer, with the
/// number of objects written.
pub fn to_dxf(board: &Board) -> (String, usize) {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let opt = FabOptions::from_board(board);
    let names: Vec<String> = board
        .stackup
        .copper_layers()
        .map(|l| l.name.clone())
        .collect();
    // (layer, AutoCAD colour, entities)
    let mut layers: Vec<(String, u8, String)> = Vec::new();
    let mut total = 0;
    let mut run = |name: String, colour: u8, f: &dyn Fn(&mut Dxf)| {
        let mut d = Dxf {
            layer: layer_name(&name),
            out: String::new(),
            count: 0,
        };
        f(&mut d);
        if d.count > 0 {
            total += d.count;
            layers.push((d.layer, colour, d.out));
        }
    };
    run("Outline".into(), 7, &|d| {
        for o in std::iter::once(&board.geometry.outline).chain(&board.geometry.slots) {
            if o.len() >= 3 {
                d.ring(o);
            }
        }
    });
    for l in 0..n {
        let name = names
            .get(l as usize)
            .cloned()
            .unwrap_or_else(|| format!("Copper{}", l + 1));
        let colour = [1u8, 3, 4, 6, 2, 5][(l as usize) % 6];
        run(format!("L{}_{name}", l + 1), colour, &|d| {
            plot_copper(d, board, l, n)
        });
    }
    for (side, t) in [(Side::Top, "Top"), (Side::Bottom, "Bottom")] {
        let silk = if side == Side::Top {
            LayerRef::TopSilk
        } else {
            LayerRef::BottomSilk
        };
        run(format!("{t}Silk"), 7, &|d| plot_other(d, board, &silk));
        run(format!("{t}Mask"), 8, &|d| {
            plot_mask(d, board, side, n, &opt)
        });
        run(format!("{t}Paste"), 9, &|d| {
            plot_paste(d, board, side, n, &opt)
        });
    }
    for m in mechanical_layers(board) {
        run(format!("Mech{m}"), 30, &|d| {
            plot_other(d, board, &LayerRef::Mechanical(m))
        });
    }
    for (name, want_plated) in [("Drill_PTH", true), ("Drill_NPTH", false)] {
        run(name.into(), 2, &|d| {
            for p in board.geometry.pads.iter().filter(|p| p.hole.0 > 0) {
                if plated(p) == want_plated {
                    d.circle([p.at.x.mm(), p.at.y.mm()], mm(p.hole.0) / 2.0);
                }
            }
            if want_plated {
                for v in board.geometry.vias.iter().filter(|v| v.hole.0 > 0) {
                    d.circle([v.at.x.mm(), v.at.y.mm()], mm(v.hole.0) / 2.0);
                }
            }
        });
    }
    let mut s = String::new();
    s.push_str("999\nTrace ");
    s.push_str(env!("CARGO_PKG_VERSION"));
    s.push_str(" DXF, millimetres\n");
    s.push_str("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1009\n9\n$INSUNITS\n70\n4\n9\n$MEASUREMENT\n70\n1\n0\nENDSEC\n");
    s.push_str("0\nSECTION\n2\nTABLES\n");
    s.push_str("0\nTABLE\n2\nLTYPE\n70\n1\n0\nLTYPE\n2\nCONTINUOUS\n70\n0\n3\nSolid line\n72\n65\n73\n0\n40\n0.0\n0\nENDTAB\n");
    let _ = write!(s, "0\nTABLE\n2\nLAYER\n70\n{}\n", layers.len());
    for (name, colour, _) in &layers {
        let _ = write!(
            s,
            "0\nLAYER\n2\n{name}\n70\n0\n62\n{colour}\n6\nCONTINUOUS\n"
        );
    }
    s.push_str("0\nENDTAB\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
    for (_, _, body) in &layers {
        s.push_str(body);
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    (s, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    #[test]
    fn writes_layers_and_shapes() {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "Top Layer".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(1000.0, 0.0),
            Point::mil(1000.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        b.geometry.tracks.push(Track {
            layer: LayerRef::Copper(0),
            net: None,
            start: Point::mil(100.0, 100.0),
            end: Point::mil(500.0, 100.0),
            width: Length::from_mil(10.0),
            component: None,
            text: false,
        });
        b.geometry.vias.push(Via {
            net: None,
            from_layer: 0,
            to_layer: 0,
            at: Point::mil(500.0, 100.0),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: Vec::new(),
            fab: Default::default(),
        });
        let (s, n) = to_dxf(&b);
        assert!(s.starts_with("999\n"));
        assert!(s.ends_with("0\nEOF\n"));
        assert!(s.contains("\n2\nL1_Top_Layer\n"));
        assert!(s.contains("\n2\nOutline\n"));
        assert!(s.contains("\n2\nDrill_PTH\n"));
        // Outline, track, via pad, via hole.
        assert_eq!(n, 4);
        assert!(s.contains("40\n0.2540\n"));
    }
}
