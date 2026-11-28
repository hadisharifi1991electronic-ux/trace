//! HFSS export: a 3D FEM model (`fem3d::Model`) and its setup written as an
//! IronPython script for ANSYS Electronics Desktop (Tools > Run Script),
//! which builds the same model Trace solves, so the two can be compared.
//!
//! - Units are mm. Every solid becomes a box or cylinder; where solids
//!   overlap the later one wins, as in Trace: each solid has the later
//!   solids it touches subtracted (originals kept), and a solid a later box
//!   holds whole is left out. The background (the model's `er`) is a
//!   "Region" box with every solid subtracted.
//! - Perfect-conductor solids are `pec` (or `copper` with copper loss);
//!   zero-thickness sheets get a Perfect E boundary (or a two-sided finite
//!   conductivity, copper with the model's roughness), grouped by net.
//! - Walls: the outer surface is Perfect E by default in HFSS; radiation
//!   walls get a Radiation boundary, copper walls a finite conductivity,
//!   on the faces found on that side of the box when the script runs.
//! - Wave ports are sheets on the box face, driven modal, one mode (two
//!   for a differential port), Zpi, renormalised to the setup's reference
//!   impedance, de-embedded as in Trace. Each mode has an integration line
//!   from the nearest ground to the signal, which fixes the sign of S.
//!   Lumped ports are sheets with their resistance and a line along the
//!   current. Ports are numbered in Trace's order.
//! - The setup: adaptive at `f_adapt` with Trace's ΔS and passes, first or
//!   second order basis as Trace's, and the sweep (interpolating or
//!   discrete). Set `RUN = True` at the top to also solve and write the
//!   Touchstone file next to the script.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::fem3d::{Fill, Mode, Model, Port, Setup, Shape, Sheet, Wall};

/// The script, and what the export assumed or could not carry over.
#[derive(Clone, Debug)]
pub struct Export {
    pub script: String,
    pub notes: Vec<String>,
}

/// mm, as an HFSS expression string (Python literal).
fn mm(v: f64) -> String {
    let x = v * 1e3;
    let mut s = format!("{:.6}", if x.abs() < 5e-7 { 0.0 } else { x });
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    format!("\"{s}mm\"")
}

/// A float literal for Python, mm.
fn mmf(v: f64) -> String {
    let s = mm(v);
    s[1..s.len() - 3].to_string()
}

fn pt(p: [f64; 3]) -> String {
    format!("[{}, {}, {}]", mm(p[0]), mm(p[1]), mm(p[2]))
}

/// The two axes of a plane normal to `axis`, in increasing order.
fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// A point (u, v) in the plane `axis` = `at` as x, y, z.
fn lift(axis: usize, at: f64, uv: [f64; 2]) -> [f64; 3] {
    let (a, b) = plane_axes(axis);
    let mut p = [0.0; 3];
    p[axis] = at;
    p[a] = uv[0];
    p[b] = uv[1];
    p
}

fn bbox(shape: &Shape) -> ([f64; 3], [f64; 3]) {
    match *shape {
        Shape::Box { lo, hi } => (
            [lo[0].min(hi[0]), lo[1].min(hi[1]), lo[2].min(hi[2])],
            [lo[0].max(hi[0]), lo[1].max(hi[1]), lo[2].max(hi[2])],
        ),
        Shape::Cyl { axis, c, r, lo, hi } => {
            let (a, b) = plane_axes(axis);
            let (mut l, mut h) = ([0.0; 3], [0.0; 3]);
            l[axis] = lo.min(hi);
            h[axis] = lo.max(hi);
            l[a] = c[0] - r;
            h[a] = c[0] + r;
            l[b] = c[1] - r;
            h[b] = c[1] + r;
            (l, h)
        }
    }
}

/// Positive-volume overlap of two boxes.
fn overlap(a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])) -> bool {
    (0..3).all(|k| a.0[k].max(b.0[k]) < a.1[k].min(b.1[k]) - 1e-12)
}

fn contains(outer: &([f64; 3], [f64; 3]), inner: &([f64; 3], [f64; 3])) -> bool {
    (0..3).all(|k| outer.0[k] <= inner.0[k] + 1e-12 && outer.1[k] >= inner.1[k] - 1e-12)
}

/// A polygon approximating a capsule hole (both ends, radius).
fn capsule(a: [f64; 2], b: [f64; 2], r: f64) -> Vec<[f64; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let th = dy.atan2(dx);
    let seg = 12;
    let mut out = Vec::new();
    // Around b from -90° to +90° of the axis, then around a.
    for (c, start) in [
        (b, th - std::f64::consts::FRAC_PI_2),
        (a, th + std::f64::consts::FRAC_PI_2),
    ] {
        for i in 0..=seg {
            let t = start + std::f64::consts::PI * i as f64 / seg as f64;
            out.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
        }
    }
    out
}

fn material_name(er: f64, tand: f64) -> String {
    let f = |x: f64, d: usize| format!("{x:.d$}").replace('.', "p").replace('-', "m");
    format!("trace_er{}_tand{}", f(er, 4), f(tand, 5))
}

/// Where a port's signal conductor crosses the port face: the middle of
/// its cut through the face, from the sheets (or failing them the
/// solids) of `net` that reach the face.
fn signal_point(model: &Model, port: &Port, at: f64, net: u32) -> Option<[f64; 3]> {
    let a = port.axis;
    let tol = 1e-9 + 1e-6 * (model.hi[a] - model.lo[a]).abs();
    let mut best: Option<([f64; 3], f64)> = None;
    for s in model.sheets.iter().filter(|s| s.net == net && s.axis != a) {
        // The sheet's in-plane axes; which of them is the port axis.
        let (u, v) = plane_axes(s.axis);
        let (ia, it) = if u == a { (0, 1) } else { (1, 0) };
        let t_axis = if u == a { v } else { u };
        let pts: Vec<[f64; 2]> = match s.disc {
            Some((c, r)) => (0..32)
                .map(|i| {
                    let t = std::f64::consts::TAU * i as f64 / 32.0;
                    [c[0] + r * t.cos(), c[1] + r * t.sin()]
                })
                .collect(),
            None => s.poly.clone(),
        };
        let mut ts: Vec<f64> = Vec::new();
        for i in 0..pts.len() {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            let (pa, qa) = (p[ia] - at, q[ia] - at);
            if pa.abs() <= tol {
                ts.push(p[it]);
            }
            if (pa < -tol && qa > tol) || (pa > tol && qa < -tol) {
                let f = pa / (pa - qa);
                ts.push(p[it] + f * (q[it] - p[it]));
            }
        }
        if ts.is_empty() {
            continue;
        }
        let lo = ts.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mut p = [0.0; 3];
        p[a] = at;
        p[s.axis] = s.at;
        p[t_axis] = (lo + hi) / 2.0;
        // The widest cut wins (the line, not a stub touching the face).
        if best.is_none_or(|b| hi - lo > b.1) {
            best = Some((p, hi - lo));
        }
    }
    if let Some((p, _)) = best {
        return Some(p);
    }
    for so in &model.solids {
        if so.fill != (Fill::Pec { net }) {
            continue;
        }
        let (lo, hi) = bbox(&so.shape);
        if lo[a] <= at + tol && hi[a] >= at - tol {
            let mut p = [0.0; 3];
            for k in 0..3 {
                p[k] = (lo[k] + hi[k]) / 2.0;
            }
            p[a] = at;
            return Some(p);
        }
    }
    None
}

/// The nearest ground from `p` straight along `k` (either way), inside
/// the port face: ground sheets normal to `k` that cover `p`, ground
/// solids' faces, other nets given as `extra`, and perfect-conductor box
/// walls. The point on it.
fn ground_point(model: &Model, p: [f64; 3], k: usize, grounds: &[u32]) -> Option<[f64; 3]> {
    let mut best: Option<f64> = None;
    let mut take = |z: f64| {
        if (z - p[k]).abs() > 1e-12 && best.is_none_or(|b| (z - p[k]).abs() < (b - p[k]).abs()) {
            best = Some(z);
        }
    };
    let (u, v) = plane_axes(k);
    for s in model
        .sheets
        .iter()
        .filter(|s| s.axis == k && grounds.contains(&s.net))
    {
        let q = [p[u], p[v]];
        let inside = match s.disc {
            Some((c, r)) => (q[0] - c[0]).hypot(q[1] - c[1]) <= r,
            None => point_in(&s.poly, q),
        };
        let in_hole = s.holes.iter().any(|&(a, b, r)| seg_dist(q, a, b) < r);
        if inside && !in_hole {
            take(s.at);
        }
    }
    for so in &model.solids {
        if let Fill::Pec { net } = so.fill {
            if grounds.contains(&net) {
                let (lo, hi) = bbox(&so.shape);
                let covers = (0..3)
                    .filter(|&j| j != k)
                    .all(|j| lo[j] <= p[j] && hi[j] >= p[j]);
                if covers {
                    take(lo[k]);
                    take(hi[k]);
                }
            }
        }
    }
    if model.walls[2 * k] == Wall::Pec {
        take(model.lo[k]);
    }
    if model.walls[2 * k + 1] == Wall::Pec {
        take(model.hi[k]);
    }
    best.map(|z| {
        let mut g = p;
        g[k] = z;
        g
    })
}

fn point_in(poly: &[[f64; 2]], q: [f64; 2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

fn seg_dist(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (q[0] - a[0] - t * dx).hypot(q[1] - a[1] - t * dy)
}

/// The integration line of a quasi-TEM port: from the nearest ground to the
/// signal, across the port face (along the sheet's normal; failing that
/// the other in-face axis).
fn int_line(
    model: &Model,
    port: &Port,
    at: f64,
    signal: u32,
    grounds: &[u32],
) -> Option<([f64; 3], [f64; 3])> {
    let p = signal_point(model, port, at, signal)?;
    let (u, v) = plane_axes(port.axis);
    // Prefer the axis a sheet of the signal lies normal to (z for traces).
    let normal = model
        .sheets
        .iter()
        .find(|s| s.net == signal && s.axis != port.axis)
        .map(|s| s.axis);
    let order: Vec<usize> = match normal {
        Some(n) => vec![n, if n == u { v } else { u }],
        None => vec![v, u],
    };
    for k in order {
        if let Some(g) = ground_point(model, p, k, grounds) {
            return Some((g, p));
        }
    }
    None
}

/// The HFSS script for `model` solved as `setup`.
pub fn script(model: &Model, setup: &Setup, title: &str) -> Export {
    let mut notes = Vec::new();
    let mut o = String::new();
    let w = &mut o;
    let _ = writeln!(w, "# -*- coding: utf-8 -*-");
    let _ = writeln!(w, "# {}", title.replace('\n', " "));
    let _ = writeln!(
        w,
        "# Written by Trace {} from its 3D FEM model. Run it in ANSYS Electronics",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(w, "# Desktop: Tools > Run Script. Units: mm.");
    let _ = writeln!(w, "import os");
    let _ = writeln!(w, "import ScriptEnv");
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "# True: also solve and write the Touchstone file beside this script."
    );
    let _ = writeln!(w, "RUN = False");
    let _ = writeln!(w);
    let _ = writeln!(w, "ScriptEnv.Initialize(\"Ansoft.ElectronicsDesktop\")");
    let _ = writeln!(w, "oDesktop.RestoreWindow()");
    let _ = writeln!(w, "oProject = oDesktop.NewProject()");
    let _ = writeln!(
        w,
        "oProject.InsertDesign(\"HFSS\", \"Trace\", \"DrivenModal\", \"\")"
    );
    let _ = writeln!(w, "oDesign = oProject.SetActiveDesign(\"Trace\")");
    let _ = writeln!(w, "oEditor = oDesign.SetActiveEditor(\"3D Modeler\")");
    let _ = writeln!(
        w,
        "oEditor.SetModelUnits([\"NAME:Units Parameter\", \"Units:=\", \"mm\", \"Rescale:=\", False])"
    );
    let _ = writeln!(w, "oDefs = oProject.GetDefinitionManager()");
    let _ = writeln!(w, "oBnd = oDesign.GetModule(\"BoundarySetup\")");
    let _ = writeln!(w, "oAna = oDesign.GetModule(\"AnalysisSetup\")");
    w.push_str(PY_HELPERS);
    let _ = writeln!(w);

    // Materials.
    let mut mats: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let bg = if (model.er - 1.0).abs() < 1e-9 {
        "vacuum".to_string()
    } else {
        let n = material_name(model.er, 0.0);
        mats.insert(n.clone(), (model.er, 0.0));
        n
    };
    for s in &model.solids {
        if let Fill::Dielectric { er, tand } = s.fill {
            if (er - 1.0).abs() > 1e-9 || tand > 0.0 {
                mats.insert(material_name(er, tand), (er, tand));
            }
        }
    }
    let _ = writeln!(w, "# Materials");
    for (n, (er, tand)) in &mats {
        let _ = writeln!(w, "material(\"{n}\", {er}, {tand})");
    }
    let _ = writeln!(w);

    // Solids, later wins.
    let boxes: Vec<_> = model.solids.iter().map(|s| bbox(&s.shape)).collect();
    let skip: Vec<bool> = (0..model.solids.len())
        .map(|i| {
            (i + 1..model.solids.len()).any(|j| {
                matches!(model.solids[j].shape, Shape::Box { .. }) && contains(&boxes[j], &boxes[i])
            })
        })
        .collect();
    let (blo, bhi) = (model.lo, model.hi);
    let names: Vec<String> = (0..model.solids.len()).map(|i| format!("S{i}")).collect();
    let pec_mat = if model.copper.is_some() {
        "copper"
    } else {
        "pec"
    };
    let _ = writeln!(w, "# Solids (where they overlap, the later one wins)");
    let mut outside = 0;
    for (i, s) in model.solids.iter().enumerate() {
        if skip[i] {
            continue;
        }
        let (lo, hi) = boxes[i];
        if (0..3).any(|k| lo[k] < blo[k] - 1e-9 || hi[k] > bhi[k] + 1e-9) {
            outside += 1;
        }
        let (mat, solve) = match s.fill {
            Fill::Dielectric { er, tand } => {
                if (er - 1.0).abs() < 1e-9 && tand == 0.0 {
                    ("vacuum".to_string(), true)
                } else {
                    (material_name(er, tand), true)
                }
            }
            Fill::Pec { .. } => (pec_mat.to_string(), false),
        };
        let solve = if solve { "True" } else { "False" };
        match s.shape {
            Shape::Box { .. } => {
                let _ = writeln!(
                    w,
                    "box(\"{}\", {}, {}, \"{mat}\", {solve})",
                    names[i],
                    pt(lo),
                    pt([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]])
                );
            }
            Shape::Cyl {
                axis,
                c,
                r,
                lo: a0,
                hi: a1,
            } => {
                let base = lift(axis, a0.min(a1), c);
                let _ = writeln!(
                    w,
                    "cyl(\"{}\", \"{}\", {}, {}, {}, \"{mat}\", {solve})",
                    names[i],
                    ["X", "Y", "Z"][axis],
                    pt(base),
                    mm(r),
                    mm((a1 - a0).abs())
                );
            }
        }
    }
    if outside > 0 {
        notes.push(format!(
            "{outside} solid(s) reach past the model box: HFSS solves them where they lie, Trace only inside the box"
        ));
    }
    for i in 0..model.solids.len() {
        if skip[i] {
            continue;
        }
        let tools: Vec<&str> = (i + 1..model.solids.len())
            .filter(|&j| !skip[j] && overlap(&boxes[i], &boxes[j]))
            .map(|j| names[j].as_str())
            .collect();
        if !tools.is_empty() {
            let _ = writeln!(w, "subtract(\"{}\", {:?})", names[i], tools);
        }
    }
    let kept: Vec<&str> = (0..model.solids.len())
        .filter(|&i| !skip[i])
        .map(|i| names[i].as_str())
        .collect();
    let _ = writeln!(
        w,
        "box(\"Region\", {}, {}, \"{bg}\", True)",
        pt(blo),
        pt([bhi[0] - blo[0], bhi[1] - blo[1], bhi[2] - blo[2]])
    );
    if !kept.is_empty() {
        let _ = writeln!(w, "subtract(\"Region\", {kept:?})");
    }
    let _ = writeln!(w);

    // Sheets, grouped by net.
    let _ = writeln!(w, "# Copper sheets (zero thickness)");
    let mut by_net: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (i, s) in model.sheets.iter().enumerate() {
        let name = format!("C{i}_net{}", s.net);
        write_sheet(w, &name, s);
        by_net.entry(s.net).or_default().push(name);
    }
    for (net, objs) in &by_net {
        match model.copper {
            Some(c) => {
                let _ = writeln!(
                    w,
                    "oBnd.AssignFiniteCond([\"NAME:Cu_net{net}\", \"Objects:=\", {objs:?}, \"UseMaterial:=\", True, \"Material:=\", \"copper\", \"UseThickness:=\", False, \"Roughness:=\", \"{}um\", \"InfGroundPlane:=\", False, \"IsTwoSided:=\", True, \"IsInternal:=\", True])",
                    c.rough_um
                );
            }
            None => {
                let _ = writeln!(
                    w,
                    "oBnd.AssignPerfectE([\"NAME:PerfE_net{net}\", \"Objects:=\", {objs:?}, \"InfGroundPlane:=\", False])"
                );
            }
        }
    }
    if model.copper.is_some() && !model.sheets.is_empty() {
        notes.push(
            "copper roughness goes to HFSS's Groisse model; Trace uses Hammerstad, so the loss differs a little"
                .into(),
        );
    }
    let _ = writeln!(w);

    // Walls.
    let _ = writeln!(
        w,
        "# Walls (the outer surface is Perfect E unless set here)"
    );
    let side = ["xlo", "xhi", "ylo", "yhi", "zlo", "zhi"];
    for f in 0..6 {
        let (k, at) = (f / 2, if f % 2 == 0 { blo[f / 2] } else { bhi[f / 2] });
        match model.walls[f] {
            Wall::Radiation => {
                let _ = writeln!(
                    w,
                    "oBnd.AssignRadiation([\"NAME:Rad_{}\", \"Faces:=\", faces_on({k}, {}), \"IsFssReference:=\", False, \"IsForPML:=\", False])",
                    side[f],
                    mmf(at)
                );
            }
            Wall::Pec => {
                if let Some(c) = model.copper.filter(|c| c.walls[f]) {
                    let _ = writeln!(
                        w,
                        "oBnd.AssignFiniteCond([\"NAME:Cu_{}\", \"Faces:=\", faces_on({k}, {}), \"UseMaterial:=\", True, \"Material:=\", \"copper\", \"UseThickness:=\", False, \"Roughness:=\", \"{}um\", \"InfGroundPlane:=\", False, \"IsTwoSided:=\", False, \"IsInternal:=\", False])",
                        side[f],
                        mmf(at),
                        c.rough_um
                    );
                }
            }
        }
    }
    let _ = writeln!(w);

    // Ports.
    let zref = setup.z_ref;
    let renorm = |w: &mut String| match zref {
        Some(z) => {
            let _ = write!(w, "\"CharImp:=\", \"Zpi\", \"RenormImp:=\", \"{z}ohm\"");
        }
        None => {
            let _ = write!(w, "\"CharImp:=\", \"Zpi\"");
        }
    };
    let renorm_all = if zref.is_some() { "True" } else { "False" };
    // Ground: net 0, and for a single-ended port every net it does not drive.
    let _ = writeln!(w, "# Ports (numbered in Trace's order)");
    let mut idx = 0;
    for p in &model.ports {
        idx += 1;
        let at = if p.high { bhi[p.axis] } else { blo[p.axis] };
        let face = format!("Port{idx}_{}", py_name(&p.name));
        let rect = vec![p.lo, [p.hi[0], p.lo[1]], p.hi, [p.lo[0], p.hi[1]]];
        let _ = writeln!(w, "poly(\"{face}\", {})", poly_pts(p.axis, at, &rect));
        let lines: Vec<Option<([f64; 3], [f64; 3])>> = match p.mode {
            Mode::Tem { signal } => vec![int_line(model, p, at, signal, &[0])],
            Mode::Diff { plus, minus } => {
                let odd = match (
                    signal_point(model, p, at, minus),
                    signal_point(model, p, at, plus),
                ) {
                    (Some(a), Some(b)) => Some((a, b)),
                    _ => None,
                };
                vec![odd, int_line(model, p, at, plus, &[0])]
            }
            Mode::Te10 => {
                // Across the short side of the port, through its middle.
                let (u, v) = plane_axes(p.axis);
                let (du, dv) = (p.hi[0] - p.lo[0], p.hi[1] - p.lo[1]);
                let mid = [(p.lo[0] + p.hi[0]) / 2.0, (p.lo[1] + p.hi[1]) / 2.0];
                let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
                a[p.axis] = at;
                b[p.axis] = at;
                if du >= dv {
                    a[u] = mid[0];
                    b[u] = mid[0];
                    a[v] = p.lo[1];
                    b[v] = p.hi[1];
                } else {
                    a[v] = mid[1];
                    b[v] = mid[1];
                    a[u] = p.lo[0];
                    b[u] = p.hi[0];
                }
                vec![Some((a, b))]
            }
        };
        let _ = write!(
            w,
            "oBnd.AssignWavePort([\"NAME:{idx}_{}\", \"Objects:=\", [\"{face}\"], \"NumModes:=\", {}, \"RenormalizeAllTerminals:=\", {renorm_all}, \"UseLineModeAlignment:=\", False, \"DoDeembed:=\", {}, \"DeembedDist:=\", {}, [\"NAME:Modes\"",
            py_name(&p.name),
            lines.len(),
            if p.deembed != 0.0 { "True" } else { "False" },
            mm(p.deembed)
        );
        for (m, l) in lines.iter().enumerate() {
            let _ = write!(w, ", [\"NAME:Mode{}\", \"ModeNum:=\", {}, ", m + 1, m + 1);
            match l {
                Some((a, b)) => {
                    let _ = write!(
                        w,
                        "\"UseIntLine:=\", True, [\"NAME:IntLine\", \"Start:=\", {}, \"End:=\", {}], ",
                        pt(*a),
                        pt(*b)
                    );
                }
                None => {
                    notes.push(format!(
                        "port {}: no integration line found (no conductor of its net reaches the face); set one in HFSS for the sign of S",
                        p.name
                    ));
                    let _ = write!(w, "\"UseIntLine:=\", False, ");
                }
            }
            let _ = write!(w, "\"AlignmentGroup:=\", 0, ");
            renorm(w);
            w.push(']');
        }
        let _ = writeln!(
            w,
            "], \"ShowReporterFilter:=\", False, \"ReporterFilter:=\", [True], \"UseAnalyticAlignment:=\", False])"
        );
        if matches!(p.mode, Mode::Diff { .. }) {
            notes.push(format!(
                "port {}: two modes; check mode 1 is the odd mode (its line runs minus to plus): Trace's S is that mode",
                p.name
            ));
        }
        if p.passive {
            notes.push(format!(
                "port {} is a matched termination in Trace (not in S); HFSS adds it to S: terminate it in 50 ohm or drop its rows",
                p.name
            ));
        }
    }
    for l in &model.lumped {
        idx += 1;
        let face = format!("Port{idx}_{}", py_name(&l.name));
        let rect = vec![l.lo, [l.hi[0], l.lo[1]], l.hi, [l.lo[0], l.hi[1]]];
        let _ = writeln!(w, "poly(\"{face}\", {})", poly_pts(2, l.z, &rect));
        let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
        a[2] = l.z;
        b[2] = l.z;
        let o = 1 - l.dir.min(1);
        a[l.dir] = l.lo[l.dir];
        b[l.dir] = l.hi[l.dir];
        a[o] = (l.lo[o] + l.hi[o]) / 2.0;
        b[o] = a[o];
        let _ = write!(
            w,
            "oBnd.AssignLumpedPort([\"NAME:{idx}_{}\", \"Objects:=\", [\"{face}\"], \"RenormalizeAllTerminals:=\", {renorm_all}, \"DoDeembed:=\", False, [\"NAME:Modes\", [\"NAME:Mode1\", \"ModeNum:=\", 1, \"UseIntLine:=\", True, [\"NAME:IntLine\", \"Start:=\", {}, \"End:=\", {}], \"AlignmentGroup:=\", 0, ",
            py_name(&l.name),
            pt(a),
            pt(b)
        );
        renorm(w);
        let _ = writeln!(
            w,
            "]], \"ShowReporterFilter:=\", False, \"ReporterFilter:=\", [True], \"Impedance:=\", \"{}ohm\"])",
            l.r
        );
    }
    if !setup.loads.is_empty() {
        notes.push(
            "the parts' values are loads on their lumped ports in Trace; in HFSS, terminate those ports in the circuit or post-processing"
                .into(),
        );
    }
    let _ = writeln!(w);

    // Setup and sweep.
    let basis = if setup.order >= 2 { 1 } else { 0 };
    let _ = writeln!(w, "# Adaptive setup and sweep");
    let _ = writeln!(
        w,
        "oAna.InsertSetup(\"HfssDriven\", [\"NAME:Setup1\", \"SolveType:=\", \"Single\", \"Frequency:=\", \"{}GHz\", \"MaxDeltaS:=\", {}, \"UseMatrixConv:=\", False, \"MaximumPasses:=\", {}, \"MinimumPasses:=\", {}, \"MinimumConvergedPasses:=\", 1, \"PercentRefinement:=\", {}, \"IsEnabled:=\", True, \"BasisOrder:=\", {basis}, \"DoLambdaRefine:=\", True, \"DoMaterialLambda:=\", True, \"SetLambdaTarget:=\", False, \"Target:=\", 0.3333, \"UseMaxTetIncrease:=\", False, \"PortAccuracy:=\", 2, \"UseABCOnPort:=\", False, \"SetPortMinMaxTri:=\", False, \"EnableSolverDomains:=\", False, \"SaveRadFieldsOnly:=\", False, \"SaveAnyFields:=\", True, \"NoAdditionalRefinementOnImport:=\", False])",
        setup.f_adapt,
        setup.delta_s,
        setup.max_passes,
        setup.min_passes.max(1),
        (setup.refine * 100.0).round().max(1.0)
    );
    let mut sweep: Vec<f64> = setup.sweep.iter().cloned().filter(|f| *f > 0.0).collect();
    sweep.sort_by(|a, b| a.total_cmp(b));
    sweep.dedup();
    let dc = setup.sweep.iter().any(|f| *f <= 0.0);
    let mut sweep_name = "LastAdaptive".to_string();
    if !sweep.is_empty() {
        let n = sweep.len();
        let even = n > 2 && {
            let d = (sweep[n - 1] - sweep[0]) / (n - 1) as f64;
            sweep
                .iter()
                .enumerate()
                .all(|(i, f)| (f - (sweep[0] + d * i as f64)).abs() < 1e-6 * sweep[n - 1])
        };
        let interp = setup.interp_tol > 0.0;
        let range = if even || n <= 2 {
            format!(
                "\"RangeType:=\", \"LinearCount\", \"RangeStart:=\", \"{}GHz\", \"RangeEnd:=\", \"{}GHz\", \"RangeCount:=\", {n}",
                sweep[0],
                sweep[n - 1]
            )
        } else {
            let list: Vec<String> = sweep.iter().map(|f| format!("\"{f}GHz\"")).collect();
            format!(
                "\"RangeType:=\", \"SinglePoints\", \"RangeStart:=\", \"{}GHz\", \"RangeEnd:=\", \"{}GHz\", [\"NAME:SweepRanges\"{}]",
                sweep[0],
                sweep[n - 1],
                list.iter()
                    .map(|f| format!(", [\"NAME:Subrange\", \"RangeType:=\", \"SinglePoints\", \"RangeStart:=\", {f}, \"RangeEnd:=\", {f}]"))
                    .collect::<String>()
            )
        };
        let kind = if interp {
            format!(
                "\"Type:=\", \"Interpolating\", \"InterpTolerance:=\", {}, \"InterpMaxSolns:=\", {}, \"InterpMinSolns:=\", 0, \"InterpMinSubranges:=\", 1, \"ExtrapToDC:=\", {}, \"InterpUseS:=\", True, \"InterpUsePortImped:=\", False, \"InterpUsePropConst:=\", True, \"UseDerivativeConvergence:=\", False, \"InterpDerivTolerance:=\", 0.2, \"UseFullBasis:=\", True, \"EnforcePassivity:=\", True, \"PassivityErrorTolerance:=\", 0.0001",
                (setup.interp_tol * 100.0).max(0.1),
                setup.interp_max.max(50),
                if dc { "True" } else { "False" }
            )
        } else {
            "\"Type:=\", \"Discrete\"".to_string()
        };
        let _ = writeln!(
            w,
            "oAna.InsertFrequencySweep(\"Setup1\", [\"NAME:Sweep1\", \"IsEnabled:=\", True, {range}, {kind}, \"SaveFields:=\", False, \"SaveRadFields:=\", False])"
        );
        sweep_name = "Sweep1".into();
        if dc && !interp {
            notes.push("a discrete sweep cannot solve DC: the 0 GHz point is left out".into());
        }
    }
    let _ = writeln!(w);
    let _ = writeln!(w, "oDesign.ValidateDesign()");
    let _ = writeln!(w, "if RUN:");
    let _ = writeln!(w, "    oDesign.AnalyzeAll()");
    let _ = writeln!(
        w,
        "    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), \"trace_hfss.s%dp\" % {idx})"
    );
    let _ = writeln!(
        w,
        "    oDesign.GetModule(\"Solutions\").ExportNetworkData(\"\", [\"Setup1:{sweep_name}\"], 3, out, [\"All\"], True, {}, \"S\", -1, 0, 15, True, True, False)",
        zref.unwrap_or(50.0)
    );
    Export { script: o, notes }
}

/// A sheet: a covered polygon or a circle, minus its holes.
fn write_sheet(w: &mut String, name: &str, s: &Sheet) {
    match s.disc {
        Some((c, r)) => {
            let _ = writeln!(
                w,
                "circle(\"{name}\", \"{}\", {}, {})",
                ["X", "Y", "Z"][s.axis],
                pt(lift(s.axis, s.at, c)),
                mm(r)
            );
        }
        None => {
            let _ = writeln!(w, "poly(\"{name}\", {})", poly_pts(s.axis, s.at, &s.poly));
        }
    }
    let mut holes = Vec::new();
    for (k, &(a, b, r)) in s.holes.iter().enumerate() {
        let hn = format!("{name}_h{k}");
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-12 {
            let _ = writeln!(
                w,
                "circle(\"{hn}\", \"{}\", {}, {})",
                ["X", "Y", "Z"][s.axis],
                pt(lift(s.axis, s.at, a)),
                mm(r)
            );
        } else {
            let _ = writeln!(
                w,
                "poly(\"{hn}\", {})",
                poly_pts(s.axis, s.at, &capsule(a, b, r))
            );
        }
        holes.push(hn);
    }
    if !holes.is_empty() {
        let _ = writeln!(w, "subtract(\"{name}\", {holes:?}, False)");
    }
}

fn poly_pts(axis: usize, at: f64, pts: &[[f64; 2]]) -> String {
    let v: Vec<String> = pts.iter().map(|&q| pt(lift(axis, at, q))).collect();
    format!("[{}]", v.join(", "))
}

fn py_name(s: &str) -> String {
    let n: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if n.is_empty() {
        "P".into()
    } else {
        n
    }
}

/// The script's helpers (IronPython 2.7: no f-strings).
const PY_HELPERS: &str = r#"

def attrs(name, mat, solve):
    return ["NAME:Attributes", "Name:=", name, "Flags:=", "", "Color:=", "(143 175 143)",
            "Transparency:=", 0.6, "PartCoordinateSystem:=", "Global", "UDMId:=", "",
            "MaterialValue:=", "\"" + mat + "\"", "SolveInside:=", solve]

def material(name, er, tand):
    if not oDefs.DoesMaterialExist(name):
        oDefs.AddMaterial(["NAME:" + name, "CoordinateSystemType:=", "Cartesian",
                           "BulkOrSurfaceType:=", 1, ["NAME:PhysicsTypes", "set:=", ["Electromagnetic"]],
                           "permittivity:=", str(er), "dielectric_loss_tangent:=", str(tand)])

def box(name, lo, size, mat, solve):
    oEditor.CreateBox(["NAME:BoxParameters", "XPosition:=", lo[0], "YPosition:=", lo[1], "ZPosition:=", lo[2],
                       "XSize:=", size[0], "YSize:=", size[1], "ZSize:=", size[2]], attrs(name, mat, solve))

def cyl(name, axis, base, r, h, mat, solve):
    oEditor.CreateCylinder(["NAME:CylinderParameters", "XCenter:=", base[0], "YCenter:=", base[1], "ZCenter:=", base[2],
                            "Radius:=", r, "Height:=", h, "WhichAxis:=", axis, "NumSides:=", "0"], attrs(name, mat, solve))

def poly(name, pts):
    pts = pts + [pts[0]]
    points = ["NAME:PolylinePoints"] + [["NAME:PLPoint", "X:=", p[0], "Y:=", p[1], "Z:=", p[2]] for p in pts]
    segs = ["NAME:PolylineSegments"] + [["NAME:PLSegment", "SegmentType:=", "Line", "StartIndex:=", i, "NoOfPoints:=", 2]
                                        for i in range(len(pts) - 1)]
    oEditor.CreatePolyline(["NAME:PolylineParameters", "IsPolylineCovered:=", True, "IsPolylineClosed:=", True,
                            points, segs,
                            ["NAME:PolylineXSection", "XSectionType:=", "None", "XSectionOrient:=", "Auto",
                             "XSectionWidth:=", "0mm", "XSectionTopWidth:=", "0mm", "XSectionHeight:=", "0mm",
                             "XSectionNumSegments:=", "0", "XSectionBendType:=", "Corner"]],
                           attrs(name, "vacuum", True))

def circle(name, axis, c, r):
    oEditor.CreateCircle(["NAME:CircleParameters", "IsCovered:=", True, "XCenter:=", c[0], "YCenter:=", c[1],
                          "ZCenter:=", c[2], "Radius:=", r, "WhichAxis:=", axis, "NumSegments:=", "0"],
                         attrs(name, "vacuum", True))

def subtract(blank, tools, keep=True):
    try:
        oEditor.Subtract(["NAME:Selections", "Blank Parts:=", blank, "Tool Parts:=", ",".join(tools)],
                         ["NAME:SubtractParameters", "KeepOriginals:=", keep])
    except Exception:
        # Nothing of it left: the later solids hold it whole.
        oEditor.Delete(["NAME:Selections", "Selections:=", blank])

def faces_on(axis, at):
    out = []
    for o in oEditor.GetObjectsInGroup("Solids"):
        for f in oEditor.GetFaceIDs(o):
            try:
                c = oEditor.GetFaceCenter(int(f))
            except Exception:
                continue
            if abs(float(c[axis]) - at) < 1e-6:
                out.append(int(f))
    return out
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fem3d::{self, LineKind};

    fn balanced(s: &str) -> bool {
        let mut depth = 0i64;
        let mut in_str = false;
        let mut prev = ' ';
        for c in s.chars() {
            if c == '"' && prev != '\\' {
                in_str = !in_str;
            } else if !in_str {
                match c {
                    '[' | '(' => depth += 1,
                    ']' | ')' => depth -= 1,
                    _ => {}
                }
                if depth < 0 {
                    return false;
                }
            }
            prev = c;
        }
        depth == 0 && !in_str
    }

    #[test]
    fn microstrip_script() {
        let m = fem3d::line(LineKind::Microstrip, 10.0, 5.0, 4.0, 0.02, 300.0);
        let setup = fem3d::Setup {
            f_adapt: 6.0,
            sweep: (0..11).map(|i| i as f64 * 0.6).collect(),
            ..fem3d::Setup::default()
        };
        let e = script(&m, &setup, "microstrip");
        let s = &e.script;
        assert!(balanced(s), "brackets");
        assert_eq!(s.matches("AssignWavePort").count(), 2);
        assert!(s.contains("material(\"trace_er4p0000_tand0p02000\", 4, 0.02)"));
        assert!(s.contains("AssignPerfectE([\"NAME:PerfE_net1\""));
        // 0 GHz is extrapolated; 10 points from 0.6 to 6 GHz, evenly.
        assert!(s.contains(
            "\"RangeStart:=\", \"0.6GHz\", \"RangeEnd:=\", \"6GHz\", \"RangeCount:=\", 10"
        ));
        assert!(s.contains("\"ExtrapToDC:=\", True"));
        // The integration line: from the ground wall (z = 0) up to the strip
        // (5 mil = 0.127 mm), in the middle of the strip.
        let x = (10.0 + 50.0) / 2.0 * 0.0254;
        let want = format!(
            "\"Start:=\", [{}, \"0mm\", \"0mm\"], \"End:=\", [{}, \"0mm\", \"0.127mm\"]",
            mm(x * 1e-3),
            mm(x * 1e-3)
        );
        assert!(s.contains(&want), "{want}\n{s}");
        assert!(e.notes.is_empty(), "{:?}", e.notes);
    }

    #[test]
    fn later_solid_wins() {
        let mut m = fem3d::line(LineKind::Stripline, 10.0, 20.0, 4.0, 0.0, 100.0);
        // A solid wholly inside a later box is left out.
        m.solids.insert(
            0,
            fem3d::Solid {
                shape: Shape::Box {
                    lo: [0.0, 0.0, 0.0],
                    hi: [1e-4, 1e-4, 1e-4],
                },
                fill: Fill::Dielectric { er: 2.0, tand: 0.0 },
            },
        );
        let e = script(&m, &fem3d::Setup::default(), "t");
        assert!(!e.script.contains("box(\"S0\""));
        assert!(e.script.contains("box(\"S1\""));
        assert!(e.script.contains("subtract(\"Region\", [\"S1\"])"));
    }

    #[test]
    fn mm_format() {
        assert_eq!(mm(0.000127), "\"0.127mm\"");
        assert_eq!(mm(0.0), "\"0mm\"");
        assert_eq!(mm(-0.0015), "\"-1.5mm\"");
        assert_eq!(mmf(0.0025), "2.5");
    }
}
