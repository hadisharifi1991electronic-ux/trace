//! STEP export (ISO 10303, AP214, millimetres) for mechanical CAD: the board
//! as a solid with its outline, cut-outs and drilled holes, and each part as
//! a box sized from its pads, all in one assembly with the parts named by
//! designator. Board bottom face at z = 0.

use std::fmt::Write;
use trace_core::body::{part_bodies, PartBody};
use trace_core::{Board, Point};

#[derive(Clone, Debug)]
pub struct StepOptions {
    /// Include the parts as boxes.
    pub parts: bool,
    /// Drill the via holes too (thousands of small holes make a heavy model).
    pub vias: bool,
    /// Leave out holes smaller than this (mm).
    pub min_hole: f64,
}

impl Default for StepOptions {
    fn default() -> Self {
        StepOptions {
            parts: true,
            vias: false,
            min_hole: 0.0,
        }
    }
}

/// What the export wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepReport {
    pub holes: usize,
    /// Holes left out: they cross the board edge, a cut-out or another hole.
    pub skipped: usize,
    pub cutouts: usize,
    pub parts: usize,
    pub thickness: f64,
}

/// An entity writer: each call adds `#n=...;` and gives back n.
struct W {
    out: String,
    n: usize,
}

fn f(v: f64) -> String {
    let v = if v.abs() < 5e-10 { 0.0 } else { v };
    let s = format!("{v:.6}");
    s.trim_end_matches('0').to_string()
}

/// A STEP string: apostrophes doubled, non-ASCII dropped.
fn text(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('\'', "''")
}

impl W {
    fn add(&mut self, e: impl AsRef<str>) -> usize {
        self.n += 1;
        let _ = writeln!(self.out, "#{}={};", self.n, e.as_ref());
        self.n
    }
    fn pt(&mut self, p: [f64; 3]) -> usize {
        self.add(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            f(p[0]),
            f(p[1]),
            f(p[2])
        ))
    }
    fn dir(&mut self, d: [f64; 3]) -> usize {
        self.add(format!(
            "DIRECTION('',({},{},{}))",
            f(d[0]),
            f(d[1]),
            f(d[2])
        ))
    }
    fn axis(&mut self, o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> usize {
        let (p, a, b) = (self.pt(o), self.dir(z), self.dir(x));
        self.add(format!("AXIS2_PLACEMENT_3D('',#{p},#{a},#{b})"))
    }
    fn vertex(&mut self, p: [f64; 3]) -> usize {
        let c = self.pt(p);
        self.add(format!("VERTEX_POINT('',#{c})"))
    }
    fn line_edge(&mut self, a: (usize, [f64; 3]), b: (usize, [f64; 3])) -> usize {
        let d = [b.1[0] - a.1[0], b.1[1] - a.1[1], b.1[2] - a.1[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-12);
        let p = self.pt(a.1);
        let dir = self.dir([d[0] / len, d[1] / len, d[2] / len]);
        let v = self.add(format!("VECTOR('',#{dir},1.)"));
        let l = self.add(format!("LINE('',#{p},#{v})"));
        self.add(format!("EDGE_CURVE('',#{},#{},#{l},.T.)", a.0, b.0))
    }
    fn face(&mut self, surface: usize, sense: bool, loops: &[(Vec<(usize, bool)>, bool)]) -> usize {
        let mut bounds = Vec::new();
        for (i, (edges, _)) in loops.iter().enumerate() {
            let oes: Vec<String> = edges
                .iter()
                .map(|&(e, s)| {
                    let id = self.add(format!(
                        "ORIENTED_EDGE('',*,*,#{e},{})",
                        if s { ".T." } else { ".F." }
                    ));
                    format!("#{id}")
                })
                .collect();
            let lp = self.add(format!("EDGE_LOOP('',({}))", oes.join(",")));
            let kind = if i == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            };
            bounds.push(format!("#{}", self.add(format!("{kind}('',#{lp},.T.)"))));
        }
        self.add(format!(
            "ADVANCED_FACE('',({}),#{surface},{})",
            bounds.join(","),
            if sense { ".T." } else { ".F." }
        ))
    }
    fn plane(&mut self, o: [f64; 3], n: [f64; 3], x: [f64; 3]) -> usize {
        let a = self.axis(o, n, x);
        self.add(format!("PLANE('',#{a})"))
    }
}

/// A ring of the board: a polygon (outline or cut-out) or a round hole.
enum Ring {
    Poly(Vec<[f64; 2]>),
    Circle([f64; 2], f64),
}

/// An inner ring kept: centre, reach, and the polygon of a cut-out.
type Kept = ([f64; 2], f64, Option<Vec<[f64; 2]>>);

/// Twice the signed area (counter-clockwise positive).
fn area2(v: &[[f64; 2]]) -> f64 {
    let n = v.len();
    (0..n)
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

fn ring_mm(pts: &[Point]) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        let q = [p.x.mm(), p.y.mm()];
        if v.last()
            .is_none_or(|l| (l[0] - q[0]).hypot(l[1] - q[1]) > 1e-4)
        {
            v.push(q);
        }
    }
    while v.len() > 1 && (v[0][0] - v[v.len() - 1][0]).hypot(v[0][1] - v[v.len() - 1][1]) <= 1e-4 {
        v.pop();
    }
    v
}

fn inside(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut c = false;
    let n = v.len();
    for i in 0..n {
        let (a, b) = (v[i], v[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
    }
    c
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Distance from a point to a ring's edge.
fn edge_dist(v: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = v.len();
    (0..n)
        .map(|i| seg_dist(p, v[i], v[(i + 1) % n]))
        .fold(f64::MAX, f64::min)
}

/// An obround slot as a polygon, counter-clockwise.
fn slot_ring(c: [f64; 2], d: f64, len: f64, deg: f64) -> Vec<[f64; 2]> {
    let r = d / 2.0;
    let a = ((len - d) / 2.0).max(0.0);
    let (s, co) = deg.to_radians().sin_cos();
    let mut v = Vec::new();
    for (cx, from) in [(a, -90.0f64), (-a, 90.0)] {
        for k in 0..=12 {
            let t = (from + 180.0 * k as f64 / 12.0).to_radians();
            let (u, w) = (cx + r * t.cos(), r * t.sin());
            v.push([c[0] + u * co - w * s, c[1] + u * s + w * co]);
        }
    }
    v
}

/// One prism solid: an outer ring (counter-clockwise) with inner rings,
/// from z0 to z1. Returns the MANIFOLD_SOLID_BREP.
fn prism(w: &mut W, name: &str, rings: &[Ring], z0: f64, z1: f64) -> usize {
    let mut top_loops: Vec<(Vec<(usize, bool)>, bool)> = Vec::new();
    let mut bot_loops: Vec<(Vec<(usize, bool)>, bool)> = Vec::new();
    let mut faces = Vec::new();
    for (ri, ring) in rings.iter().enumerate() {
        match ring {
            Ring::Poly(v0) => {
                // Outer rings run counter-clockwise, inner ones clockwise.
                let mut v = v0.clone();
                if (area2(&v) > 0.0) != (ri == 0) {
                    v.reverse();
                }
                let n = v.len();
                let vb: Vec<(usize, [f64; 3])> = v
                    .iter()
                    .map(|p| {
                        let q = [p[0], p[1], z0];
                        (w.vertex(q), q)
                    })
                    .collect();
                let vt: Vec<(usize, [f64; 3])> = v
                    .iter()
                    .map(|p| {
                        let q = [p[0], p[1], z1];
                        (w.vertex(q), q)
                    })
                    .collect();
                let eb: Vec<usize> = (0..n)
                    .map(|i| w.line_edge(vb[i], vb[(i + 1) % n]))
                    .collect();
                let et: Vec<usize> = (0..n)
                    .map(|i| w.line_edge(vt[i], vt[(i + 1) % n]))
                    .collect();
                let ev: Vec<usize> = (0..n).map(|i| w.line_edge(vb[i], vt[i])).collect();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (a, b) = (v[i], v[j]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let l = dx.hypot(dy).max(1e-12);
                    let pl = w.plane(
                        [a[0], a[1], z0],
                        [dy / l, -dx / l, 0.0],
                        [dx / l, dy / l, 0.0],
                    );
                    let fc = w.face(
                        pl,
                        true,
                        &[(
                            vec![(eb[i], true), (ev[j], true), (et[i], false), (ev[i], false)],
                            true,
                        )],
                    );
                    faces.push(fc);
                }
                top_loops.push((et.iter().map(|&e| (e, true)).collect(), true));
                bot_loops.push((eb.iter().rev().map(|&e| (e, false)).collect(), true));
            }
            Ring::Circle(c, r) => {
                let (pb, pt) = ([c[0] + r, c[1], z0], [c[0] + r, c[1], z1]);
                let (vb, vt) = (w.vertex(pb), w.vertex(pt));
                let circle = |w: &mut W, z: f64, v: usize| {
                    let a = w.axis([c[0], c[1], z], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
                    let cc = w.add(format!("CIRCLE('',#{a},{})", f(*r)));
                    w.add(format!("EDGE_CURVE('',#{v},#{v},#{cc},.T.)"))
                };
                let cb = circle(w, z0, vb);
                let ct = circle(w, z1, vt);
                let seam = w.line_edge((vb, pb), (vt, pt));
                let ax = w.axis([c[0], c[1], z0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
                let cyl = w.add(format!("CYLINDRICAL_SURFACE('',#{ax},{})", f(*r)));
                let fc = w.face(
                    cyl,
                    false,
                    &[(
                        vec![(cb, false), (seam, true), (ct, true), (seam, false)],
                        true,
                    )],
                );
                faces.push(fc);
                top_loops.push((vec![(ct, false)], true));
                bot_loops.push((vec![(cb, true)], true));
            }
        }
    }
    let tp = w.plane([0.0, 0.0, z1], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let bp = w.plane([0.0, 0.0, z0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    faces.push(w.face(tp, true, &top_loops));
    faces.push(w.face(bp, true, &bot_loops));
    let list: Vec<String> = faces.iter().map(|x| format!("#{x}")).collect();
    let shell = w.add(format!("CLOSED_SHELL('',({}))", list.join(",")));
    w.add(format!("MANIFOLD_SOLID_BREP('{}',#{shell})", text(name)))
}

/// Shared contexts of the file.
struct Ctx {
    geo: usize,
    prod: usize,
    pdef: usize,
}

/// A product with one solid: returns (product definition, shape
/// representation, its origin axis).
fn product(w: &mut W, cx: &Ctx, name: &str, solid: Option<usize>) -> (usize, usize, usize) {
    let n = text(name);
    let p = w.add(format!("PRODUCT('{n}','{n}','',(#{}))", cx.prod));
    w.add(format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{p}))"));
    let pf = w.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{p})"));
    let pd = w.add(format!(
        "PRODUCT_DEFINITION('design','',#{pf},#{})",
        cx.pdef
    ));
    let pds = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    let origin = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let rep = match solid {
        Some(s) => w.add(format!(
            "ADVANCED_BREP_SHAPE_REPRESENTATION('{n}',(#{s},#{origin}),#{})",
            cx.geo
        )),
        None => w.add(format!(
            "SHAPE_REPRESENTATION('{n}',(#{origin}),#{})",
            cx.geo
        )),
    };
    w.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
    (pd, rep, origin)
}

/// Puts a child product in the assembly where it stands (no transform).
fn place(
    w: &mut W,
    n: usize,
    parent: (usize, usize, usize),
    child: (usize, usize, usize),
    name: &str,
) {
    let t = text(name);
    let nauo = w.add(format!(
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE('NAUO{n}','{t}','',#{},#{},$)",
        parent.0, child.0
    ));
    let pds = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{nauo})"));
    let idt = w.add(format!(
        "ITEM_DEFINED_TRANSFORMATION('','',#{},#{})",
        child.2, parent.2
    ));
    let rr = w.add(format!(
        "(REPRESENTATION_RELATIONSHIP('','',#{},#{})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt})SHAPE_REPRESENTATION_RELATIONSHIP())",
        child.1, parent.1
    ));
    w.add(format!(
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{pds})"
    ));
}

/// A colour on a solid.
fn style(w: &mut W, solid: usize, rgb: [u8; 3]) -> usize {
    let c = w.add(format!(
        "COLOUR_RGB('',{},{},{})",
        f(rgb[0] as f64 / 255.0),
        f(rgb[1] as f64 / 255.0),
        f(rgb[2] as f64 / 255.0)
    ));
    let fc = w.add(format!("FILL_AREA_STYLE_COLOUR('',#{c})"));
    let fa = w.add(format!("FILL_AREA_STYLE('',(#{fc}))"));
    let sf = w.add(format!("SURFACE_STYLE_FILL_AREA(#{fa})"));
    let ss = w.add(format!("SURFACE_SIDE_STYLE('',(#{sf}))"));
    let su = w.add(format!("SURFACE_STYLE_USAGE(.BOTH.,#{ss})"));
    let psa = w.add(format!("PRESENTATION_STYLE_ASSIGNMENT((#{su}))"));
    w.add(format!("STYLED_ITEM('color',(#{psa}),#{solid})"))
}

/// The board as an AP214 STEP file.
pub fn to_step(
    board: &Board,
    name: &str,
    opt: &StepOptions,
) -> Result<(String, StepReport), String> {
    let outline = ring_mm(&board.geometry.outline);
    if outline.len() < 3 || area2(&outline).abs() < 1e-6 {
        return Err("the board has no outline".into());
    }
    let mut rep = StepReport::default();
    let t = {
        let t = board.stackup.total_thickness().mm();
        if t > 0.05 {
            t
        } else {
            1.6
        }
    };
    rep.thickness = t;
    // Inner rings: cut-outs first, then holes, each kept only when clear
    // of the edge and of every ring before it.
    let mut rings = vec![Ring::Poly(outline.clone())];
    // (centre, radius) of each inner ring kept, for the overlap check.
    let mut kept: Vec<Kept> = Vec::new();
    let gap = 0.01;
    for s in &board.geometry.slots {
        let v = ring_mm(s);
        if v.len() < 3 || area2(&v).abs() < 1e-6 {
            continue;
        }
        let ok = v
            .iter()
            .all(|&p| inside(&outline, p) && edge_dist(&outline, p) > gap)
            && kept.iter().all(|(_, _, k)| {
                k.as_ref().is_some_and(|k| {
                    v.iter().all(|&p| !inside(k, p)) && k.iter().all(|&p| !inside(&v, p))
                })
            });
        if ok {
            let c = v.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
            let c = [c[0] / v.len() as f64, c[1] / v.len() as f64];
            let r = v
                .iter()
                .map(|p| (p[0] - c[0]).hypot(p[1] - c[1]))
                .fold(0.0, f64::max);
            kept.push((c, r, Some(v.clone())));
            rings.push(Ring::Poly(v));
            rep.cutouts += 1;
        }
    }
    let n_cut = kept.len();
    // Holes, largest first: (centre, diameter, slot length, slot angle).
    let mut holes: Vec<([f64; 2], f64, f64, f64)> = board
        .geometry
        .pads
        .iter()
        .filter(|p| p.hole.0 > 0)
        .map(|p| {
            (
                [p.at.x.mm(), p.at.y.mm()],
                p.hole.mm(),
                p.fab.slot.mm(),
                p.fab.slot_deg,
            )
        })
        .collect();
    if opt.vias {
        holes.extend(
            board
                .geometry
                .vias
                .iter()
                .filter(|v| v.hole.0 > 0)
                .map(|v| ([v.at.x.mm(), v.at.y.mm()], v.hole.mm(), 0.0, 0.0)),
        );
    }
    holes.retain(|h| h.1 >= opt.min_hole.max(0.01));
    holes.sort_by(|a, b| (b.1.max(b.2)).total_cmp(&a.1.max(a.2)));
    // A grid of kept holes, 2 mm cells, for the overlap check.
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
    let cell = |p: [f64; 2]| ((p[0] / 2.0).floor() as i64, (p[1] / 2.0).floor() as i64);
    for (c, d, slot, deg) in holes {
        let reach = d.max(slot) / 2.0;
        let clear_edge = inside(&outline, c) && edge_dist(&outline, c) > reach + gap;
        let clear_cut = kept[..n_cut].iter().all(|(_, _, k)| {
            k.as_ref()
                .is_none_or(|k| !inside(k, c) && edge_dist(k, c) > reach + gap)
        });
        let (gx, gy) = cell(c);
        let span = (reach / 2.0).ceil() as i64 + 3;
        let mut clear_holes = true;
        'g: for ix in gx - span..=gx + span {
            for iy in gy - span..=gy + span {
                for &k in grid.get(&(ix, iy)).into_iter().flatten() {
                    let (kc, kr, _) = &kept[k];
                    if (kc[0] - c[0]).hypot(kc[1] - c[1]) < kr + reach + gap {
                        clear_holes = false;
                        break 'g;
                    }
                }
            }
        }
        if !(clear_edge && clear_cut && clear_holes) {
            rep.skipped += 1;
            continue;
        }
        grid.entry((gx, gy)).or_default().push(kept.len());
        kept.push((c, reach, None));
        if slot > d + 1e-6 {
            rings.push(Ring::Poly(slot_ring(c, d, slot, deg)));
        } else {
            rings.push(Ring::Circle(c, d / 2.0));
        }
        rep.holes += 1;
    }

    let mut w = W {
        out: String::new(),
        n: 0,
    };
    let app = w.add("APPLICATION_CONTEXT('core data for automotive mechanical design processes')");
    w.add(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"
    ));
    let prod = w.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let pdef = w.add(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));
    let mm = w.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))");
    let rad = w.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
    let sr = w.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
    let unc = w.add(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{mm},'distance_accuracy_value','confusion accuracy')"
    ));
    let geo = w.add(format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr}))REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY'))"
    ));
    let cx = Ctx { geo, prod, pdef };
    let top = product(&mut w, &cx, name, None);
    let mut styled = Vec::new();
    let pcb = prism(&mut w, "PCB", &rings, 0.0, t);
    styled.push(style(&mut w, pcb, [26, 92, 52]));
    let bp = product(&mut w, &cx, &format!("{name}_PCB"), Some(pcb));
    let mut k = 1;
    place(&mut w, k, top, bp, "PCB");
    if opt.parts {
        let off = trace_core::variant::not_fitted(board);
        let bodies: Vec<PartBody> = part_bodies(board);
        for b in bodies.iter().filter(|b| !off.contains(&b.component)) {
            let d = &board.components[b.component].designator;
            let (z0, z1) = b.z(t);
            let ring = Ring::Poly(b.corners.to_vec());
            let s = prism(&mut w, d, &[ring], z0, z1);
            styled.push(style(&mut w, s, b.colour));
            let p = product(&mut w, &cx, d, Some(s));
            k += 1;
            place(&mut w, k, top, p, d);
            rep.parts += 1;
        }
    }
    let list: Vec<String> = styled.iter().map(|s| format!("#{s}")).collect();
    w.add(format!(
        "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{geo})",
        list.join(",")
    ));

    let mut s = String::new();
    s.push_str("ISO-10303-21;\nHEADER;\n");
    let _ = writeln!(
        s,
        "FILE_DESCRIPTION(('{} board and parts'),'2;1');",
        text(name)
    );
    let _ = writeln!(
        s,
        "FILE_NAME('{}.step','{}',(''),(''),'Trace {}','Trace','');",
        text(name),
        crate::assembly::today(),
        env!("CARGO_PKG_VERSION")
    );
    s.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n");
    s.push_str(&w.out);
    s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok((s, rep))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    #[test]
    fn writes_a_drilled_board_with_parts() {
        let mut b = Board::default();
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(2000.0, 0.0),
            Point::mil(2000.0, 1000.0),
            Point::mil(0.0, 1000.0),
        ];
        b.components.push(Component {
            designator: "J1".into(),
            footprint: "HDR".into(),
            library_ref: None,
            x: Length::from_mil(500.0),
            y: Length::from_mil(500.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        for (x, hole) in [(400.0, 40.0), (600.0, 40.0), (1990.0, 40.0), (605.0, 30.0)] {
            b.geometry.pads.push(Pad {
                layer: LayerRef::AllCopper,
                net: None,
                component: Some(0),
                name: "1".into(),
                at: Point::mil(x, 500.0),
                size_x: Length::from_mil(70.0),
                size_y: Length::from_mil(70.0),
                hole: Length::from_mil(hole),
                shape: PadShape::Round,
                rotation_deg: 0.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            });
        }
        let (s, r) = to_step(&b, "test", &StepOptions::default()).unwrap();
        assert!(s.starts_with("ISO-10303-21;"));
        assert!(s.ends_with("END-ISO-10303-21;\n"));
        // Two holes kept; one crosses the edge, one overlaps another.
        assert_eq!(r.holes, 2);
        assert_eq!(r.skipped, 2);
        assert_eq!(r.parts, 1);
        assert_eq!(s.matches("CYLINDRICAL_SURFACE").count(), 2);
        assert_eq!(s.matches("MANIFOLD_SOLID_BREP").count(), 2);
        assert!(s.contains("PRODUCT('J1','J1'"));
        assert!(s.contains("1.6"));
        // Each edge is used once each way.
        let edges = s.matches("EDGE_CURVE(").count();
        assert_eq!(s.matches("ORIENTED_EDGE(").count(), 2 * edges);
    }
}
