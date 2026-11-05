//! STEP part models (AP203/AP214, as vendors and EDA libraries ship them)
//! turned into triangle meshes with their face colours, ready to draw.
//!
//! The B-rep geometry is read and tessellated by the `truck` crates. This
//! crate adds what they leave out: the colours in `STYLED_ITEM`s, the
//! placement of each part of an assembly, and the file's length unit.

use std::collections::{HashMap, HashSet};
use truck_meshalgo::prelude::{PolygonMesh, RobustMeshableShape};
use truck_stepio::r#in::Table;

/// A triangle mesh in millimetres, in the model's own frame.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
            )
        }))
    }
}

/// Grey for faces the file gives no colour.
pub const DEFAULT_COLOR: [u8; 3] = [168, 168, 172];

/// Reads a STEP file and tessellates every solid in it. Shells that cannot
/// be read are left out; an error means nothing could be read.
pub fn mesh(data: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(data);
    let ents = parse_entities(&text);
    let scale = length_unit(&text);
    let colors = colors(&ents);
    let placements = placements(&ents);

    let table = std::panic::catch_unwind(|| Table::from_step(&text))
        .map_err(|_| "the STEP reader failed on this file".to_string())?
        .ok_or("not a STEP file the reader understands")?;

    let mut out = Mesh::default();
    let mut ids: Vec<&u64> = table.shell.keys().collect();
    ids.sort();
    let mut read = 0;
    for &id in ids {
        let holder = &table.shell[&id];
        let shell_color = colors.get(&id).copied();
        let faces: Vec<u64> = ents.get(&id).map(|e| e.refs.clone()).unwrap_or_default();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cs = table.to_compressed_shell(holder).ok()?;
            let mut lo = [f64::MAX; 3];
            let mut hi = [f64::MIN; 3];
            for v in &cs.vertices {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            let size = if lo[0] > hi[0] {
                1.0
            } else {
                ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt()
            };
            let tol = (size * 0.004).clamp(0.002 / scale, 0.05 / scale);
            let pad = size * 0.1 + 0.01;
            let keep = (
                [lo[0] - pad, lo[1] - pad, lo[2] - pad],
                [hi[0] + pad, hi[1] + pad, hi[2] + pad],
            );
            Some((cs.robust_triangulation(tol), keep))
        }));
        let Ok(Some((meshed, keep))) = res else {
            continue;
        };
        read += 1;
        let per_face = meshed.faces.len() == faces.len();
        let transforms = placements
            .get(&id)
            .cloned()
            .unwrap_or_else(|| vec![IDENTITY]);
        for (fi, face) in meshed.faces.iter().enumerate() {
            let Some(poly) = &face.surface else { continue };
            let color = per_face
                .then(|| colors.get(&faces[fi]).copied())
                .flatten()
                .or(shell_color)
                .unwrap_or(DEFAULT_COLOR);
            for t in &transforms {
                push_polygon(&mut out, poly, t, scale, color, keep);
            }
        }
    }
    if read == 0 || out.indices.is_empty() {
        return Err("no solid in the file could be read".into());
    }
    Ok(out)
}

/// Adds a face's triangles, leaving out any that stray outside the shell's
/// own bounds (`keep`), which the tessellator sometimes produces.
fn push_polygon(
    out: &mut Mesh,
    poly: &PolygonMesh,
    t: &Xform,
    scale: f64,
    color: [u8; 3],
    keep: ([f64; 3], [f64; 3]),
) {
    let pos = poly.positions();
    let nor = poly.normals();
    for tri in poly.faces().triangle_iter() {
        let inside = tri.iter().all(|v| {
            let q = pos[v.pos];
            (0..3).all(|k| q[k] >= keep.0[k] && q[k] <= keep.1[k])
        });
        if !inside {
            continue;
        }
        let i0 = out.positions.len() as u32;
        let p: Vec<[f64; 3]> = tri
            .iter()
            .map(|v| {
                let q = pos[v.pos];
                [q[0], q[1], q[2]]
            })
            .collect();
        let flat = {
            let (a, b, c) = (p[0], p[1], p[2]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ]
        };
        for (k, v) in tri.iter().enumerate() {
            let n: [f64; 3] = v
                .nor
                .and_then(|i| nor.get(i))
                .map_or(flat, |n| [n[0], n[1], n[2]]);
            let q = t.apply(p[k]);
            out.positions.push([
                (q[0] * scale) as f32,
                (q[1] * scale) as f32,
                (q[2] * scale) as f32,
            ]);
            let n = t.rotate(n);
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
            out.normals
                .push([(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32]);
            out.colors.push(color);
        }
        out.indices.extend_from_slice(&[i0, i0 + 1, i0 + 2]);
    }
}

// ---- A small reader for the parts of STEP that truck leaves out -----------

struct Entity {
    /// Type name; for a complex entity, every name in it joined by spaces.
    kind: String,
    /// The text inside the outer parentheses.
    args: String,
    /// Every `#n` reference in the arguments, in order.
    refs: Vec<u64>,
}

fn parse_entities(text: &str) -> HashMap<u64, Entity> {
    let mut out = HashMap::new();
    let data = match text.find("DATA;") {
        Some(i) => &text[i + 5..],
        None => text,
    };
    let b = data.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'#' {
            i += 1;
            continue;
        }
        // "#id ="
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let Ok(id) = data[i + 1..j].parse::<u64>() else {
            i = j.max(i + 1);
            continue;
        };
        let mut k = j;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        if k >= b.len() || b[k] != b'=' {
            i = j;
            continue;
        }
        // Find the closing ';' outside strings.
        let start = k + 1;
        let mut e = start;
        let mut quoted = false;
        while e < b.len() {
            match b[e] {
                b'\'' => quoted = !quoted,
                b';' if !quoted => break,
                _ => {}
            }
            e += 1;
        }
        let body = data[start..e.min(b.len())].trim();
        let (kind, args) = if let Some(rest) = body.strip_prefix('(') {
            // Complex entity: names of each part.
            let names: Vec<&str> = split_complex(rest);
            (names.join(" "), rest.to_string())
        } else if let Some(p) = body.find('(') {
            (
                body[..p].trim().to_string(),
                body[p + 1..body.rfind(')').unwrap_or(body.len())].to_string(),
            )
        } else {
            (body.to_string(), String::new())
        };
        let refs = refs_in(&args);
        out.insert(id, Entity { kind, args, refs });
        i = e + 1;
    }
    out
}

/// Names of the parts of a complex entity such as
/// `( REPRESENTATION_RELATIONSHIP(...) SHAPE_REPRESENTATION_RELATIONSHIP() )`.
fn split_complex(s: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let b = s.as_bytes();
    let (mut depth, mut i, mut quoted) = (0, 0, false);
    let mut name_start = None;
    while i < b.len() {
        let c = b[i];
        if quoted {
            if c == b'\'' {
                quoted = false;
            }
        } else if c == b'\'' {
            quoted = true;
        } else if c == b'(' {
            if depth == 0 {
                if let Some(s0) = name_start.take() {
                    names.push(s[s0..i].trim());
                }
            }
            depth += 1;
        } else if c == b')' {
            depth -= 1;
        } else if depth == 0 && (c.is_ascii_alphanumeric() || c == b'_') && name_start.is_none() {
            name_start = Some(i);
        }
        i += 1;
    }
    names
}

fn refs_in(args: &str) -> Vec<u64> {
    let b = args.as_bytes();
    let mut out = Vec::new();
    let mut quoted = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => quoted = !quoted,
            b'#' if !quoted => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if let Ok(n) = args[i + 1..j].parse() {
                    out.push(n);
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The numbers in an argument list, outside strings and references.
fn numbers_in(args: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut cur = String::new();
    let mut after_hash = false;
    for c in args.chars().chain(std::iter::once(',')) {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if c == '#' {
            after_hash = true;
            cur.clear();
            continue;
        }
        if c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'E' | 'e') {
            cur.push(c);
        } else {
            if !after_hash && !cur.is_empty() {
                if let Ok(v) = cur.parse::<f64>() {
                    out.push(v);
                }
            }
            cur.clear();
            after_hash = false;
        }
    }
    out
}

/// Millimetres per file length unit.
fn length_unit(text: &str) -> f64 {
    let t = text.to_ascii_uppercase();
    if t.contains("CONVERSION_BASED_UNIT('INCH'") || t.contains("CONVERSION_BASED_UNIT ( 'INCH'") {
        25.4
    } else if t.contains("SI_UNIT(.MILLI.,.METRE.)") || t.contains(".MILLI., .METRE.") {
        1.0
    } else if t.contains("SI_UNIT($,.METRE.)") || t.contains("SI_UNIT( $, .METRE. )") {
        1000.0
    } else {
        1.0
    }
}

/// Colour of each styled item, keyed by the shell or face it applies to.
fn colors(ents: &HashMap<u64, Entity>) -> HashMap<u64, [u8; 3]> {
    let mut out = HashMap::new();
    let mut styled: Vec<(&u64, &Entity)> = ents
        .iter()
        .filter(|(_, e)| e.kind == "STYLED_ITEM" || e.kind == "OVER_RIDING_STYLED_ITEM")
        .collect();
    // Overriding styles win: apply them last.
    styled.sort_by_key(|(id, e)| (e.kind != "STYLED_ITEM", **id));
    for (_, e) in styled {
        let Some(&item) = e.refs.last() else { continue };
        let Some(c) = e.refs[..e.refs.len() - 1]
            .iter()
            .find_map(|&s| find_color(ents, s, 0))
        else {
            continue;
        };
        for target in shells_or_faces(ents, item) {
            out.insert(target, c);
        }
    }
    out
}

fn find_color(ents: &HashMap<u64, Entity>, id: u64, depth: u32) -> Option<[u8; 3]> {
    let e = ents.get(&id)?;
    if depth > 8 || e.kind.contains("CURVE_STYLE") {
        return None;
    }
    if e.kind == "COLOUR_RGB" {
        let n = numbers_in(&e.args);
        if n.len() >= 3 {
            let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            return Some([c(n[0]), c(n[1]), c(n[2])]);
        }
    }
    if e.kind == "DRAUGHTING_PRE_DEFINED_COLOUR" {
        let name = e.args.trim().trim_matches('\'').to_ascii_lowercase();
        return Some(match name.as_str() {
            "red" => [220, 40, 40],
            "green" => [40, 180, 60],
            "blue" => [40, 70, 200],
            "yellow" => [230, 210, 40],
            "magenta" => [200, 40, 200],
            "cyan" => [40, 200, 210],
            "black" => [25, 25, 25],
            "white" => [235, 235, 235],
            _ => DEFAULT_COLOR,
        });
    }
    e.refs.iter().find_map(|&r| find_color(ents, r, depth + 1))
}

/// The shells (or single faces) an item stands for.
fn shells_or_faces(ents: &HashMap<u64, Entity>, id: u64) -> Vec<u64> {
    let Some(e) = ents.get(&id) else {
        return Vec::new();
    };
    match e.kind.as_str() {
        "CLOSED_SHELL" | "OPEN_SHELL" | "ADVANCED_FACE" | "FACE_SURFACE" => vec![id],
        "MANIFOLD_SOLID_BREP"
        | "BREP_WITH_VOIDS"
        | "SHELL_BASED_SURFACE_MODEL"
        | "FACETED_BREP" => e
            .refs
            .iter()
            .filter(|r| ents.get(r).is_some_and(|x| x.kind.ends_with("_SHELL")))
            .copied()
            .collect(),
        _ => Vec::new(),
    }
}

// ---- Assembly placement -----------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Xform {
    /// Row-major 3x3 rotation and a translation.
    r: [[f64; 3]; 3],
    t: [f64; 3],
}

const IDENTITY: Xform = Xform {
    r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    t: [0.0; 3],
};

impl Xform {
    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let mut q = self.rotate(p);
        for (k, v) in q.iter_mut().enumerate() {
            *v += self.t[k];
        }
        q
    }
    fn rotate(&self, p: [f64; 3]) -> [f64; 3] {
        let r = &self.r;
        [
            r[0][0] * p[0] + r[0][1] * p[1] + r[0][2] * p[2],
            r[1][0] * p[0] + r[1][1] * p[1] + r[1][2] * p[2],
            r[2][0] * p[0] + r[2][1] * p[1] + r[2][2] * p[2],
        ]
    }
    /// self ∘ o: apply o first.
    fn then_after(&self, o: &Xform) -> Xform {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.r[i][k] * o.r[k][j]).sum();
            }
        }
        Xform {
            r,
            t: self.apply(o.t),
        }
    }
    fn inverse(&self) -> Xform {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = self.r[j][i];
            }
        }
        let inv = Xform { r, t: [0.0; 3] };
        let t = inv.rotate(self.t);
        Xform {
            r,
            t: [-t[0], -t[1], -t[2]],
        }
    }
}

fn vec3(ents: &HashMap<u64, Entity>, id: Option<&u64>) -> Option<[f64; 3]> {
    let n = numbers_in(&ents.get(id?)?.args);
    (n.len() >= 3).then(|| [n[0], n[1], n[2]])
}

fn axis_placement(ents: &HashMap<u64, Entity>, id: u64) -> Xform {
    let Some(e) = ents.get(&id).filter(|e| e.kind == "AXIS2_PLACEMENT_3D") else {
        return IDENTITY;
    };
    let o = vec3(ents, e.refs.first()).unwrap_or([0.0; 3]);
    let norm = |v: [f64; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l < 1e-12 {
            None
        } else {
            Some([v[0] / l, v[1] / l, v[2] / l])
        }
    };
    let z = vec3(ents, e.refs.get(1))
        .and_then(norm)
        .unwrap_or([0.0, 0.0, 1.0]);
    let xr = vec3(ents, e.refs.get(2))
        .and_then(norm)
        .unwrap_or(if z[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        });
    // x = ref minus its z part, y = z × x.
    let d = xr[0] * z[0] + xr[1] * z[1] + xr[2] * z[2];
    let x = norm([xr[0] - d * z[0], xr[1] - d * z[1], xr[2] - d * z[2]]).unwrap_or([1.0, 0.0, 0.0]);
    let y = [
        z[1] * x[2] - z[2] * x[1],
        z[2] * x[0] - z[0] * x[2],
        z[0] * x[1] - z[1] * x[0],
    ];
    Xform {
        r: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
        t: o,
    }
}

/// Where each shell sits in the top-level model, once per instance.
fn placements(ents: &HashMap<u64, Entity>) -> HashMap<u64, Vec<Xform>> {
    // Representations joined without a transform share a frame: group them.
    let mut group: HashMap<u64, u64> = HashMap::new();
    fn find(g: &mut HashMap<u64, u64>, x: u64) -> u64 {
        let p = *g.get(&x).unwrap_or(&x);
        if p == x {
            return x;
        }
        let r = find(g, p);
        g.insert(x, r);
        r
    }
    // (child, parent, transform child -> parent)
    let mut links: Vec<(u64, u64, Xform)> = Vec::new();
    for e in ents.values() {
        if !e.kind.contains("REPRESENTATION_RELATIONSHIP") {
            continue;
        }
        let (Some(&r1), Some(&r2)) = (e.refs.first(), e.refs.get(1)) else {
            continue;
        };
        let transform = e.refs.get(2).and_then(|t| ents.get(t)).and_then(|t| {
            (t.kind == "ITEM_DEFINED_TRANSFORMATION").then(|| {
                let from = axis_placement(ents, *t.refs.first().unwrap_or(&0));
                let to = axis_placement(ents, *t.refs.get(1).unwrap_or(&0));
                to.then_after(&from.inverse())
            })
        });
        match transform {
            Some(x) => links.push((r1, r2, x)),
            None => {
                let (a, b) = (find(&mut group, r1), find(&mut group, r2));
                if a != b {
                    group.insert(a, b);
                }
            }
        }
    }
    if links.is_empty() {
        return HashMap::new();
    }
    // Shells held by each representation group.
    let mut shells_of: HashMap<u64, Vec<u64>> = HashMap::new();
    let reps: Vec<u64> = ents
        .iter()
        .filter(|(_, e)| e.kind.ends_with("REPRESENTATION") && !e.kind.contains("RELATIONSHIP"))
        .map(|(id, _)| *id)
        .collect();
    for rep in reps {
        let g = find(&mut group, rep);
        let e = &ents[&rep];
        for item in &e.refs {
            let s = shells_or_faces(ents, *item);
            shells_of.entry(g).or_default().extend(s);
        }
    }
    let mut children: HashMap<u64, Vec<(u64, Xform)>> = HashMap::new();
    let mut is_child = HashSet::new();
    for (c, p, x) in &links {
        let (c, p) = (find(&mut group, *c), find(&mut group, *p));
        children.entry(p).or_default().push((c, *x));
        is_child.insert(c);
    }
    let roots: Vec<u64> = children
        .keys()
        .chain(shells_of.keys())
        .copied()
        .filter(|g| !is_child.contains(g))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut out: HashMap<u64, Vec<Xform>> = HashMap::new();
    fn walk(
        g: u64,
        x: Xform,
        depth: u32,
        children: &HashMap<u64, Vec<(u64, Xform)>>,
        shells_of: &HashMap<u64, Vec<u64>>,
        out: &mut HashMap<u64, Vec<Xform>>,
    ) {
        if depth > 16 {
            return;
        }
        for s in shells_of.get(&g).into_iter().flatten() {
            out.entry(*s).or_default().push(x);
        }
        for (c, cx) in children.get(&g).into_iter().flatten() {
            walk(*c, x.then_after(cx), depth + 1, children, shells_of, out);
        }
    }
    for r in roots {
        walk(r, IDENTITY, 0, &children, &shells_of, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_entities() {
        let t = "DATA;\n#1=CARTESIAN_POINT('',(1.,2.,3.));\n#2=( A(#1) B('x;y') );\nENDSEC;";
        let e = parse_entities(t);
        assert_eq!(e[&1].kind, "CARTESIAN_POINT");
        assert_eq!(numbers_in(&e[&1].args), vec![1.0, 2.0, 3.0]);
        assert_eq!(e[&2].kind, "A B");
        assert_eq!(e[&2].refs, vec![1]);
    }

    #[test]
    fn inverse_undoes() {
        let a = Xform {
            r: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            t: [1.0, 2.0, 3.0],
        };
        let p = a.inverse().apply(a.apply([4.0, 5.0, 6.0]));
        assert!((p[0] - 4.0).abs() < 1e-9 && (p[1] - 5.0).abs() < 1e-9);
    }
}
