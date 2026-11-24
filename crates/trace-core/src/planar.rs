//! Planar EM: a window of a board's outer layer (traces, pads, pours,
//! ground vias over the next layer's plane) solved by the 3D FEM, with
//! each two-pad part as a lumped port. The parts' values then go in as
//! loads on those ports, so a matching network can be re-tuned without
//! solving again.

use crate::fdtd::Cx;
use crate::fem3d::{self, Copper, Fill, Lumped, Mode, Model, Port, Shape, Sheet, Solid, Wall};
use crate::geometry::{LayerRef, PadShape};
use crate::Board;
use faer::c64;

const MIL: f64 = 25.4e-6;

/// A two-pad part in the window: its lumped port and value.
#[derive(Clone, Debug)]
pub struct Part {
    pub designator: String,
    pub value: String,
    /// Port index in the solved S (after the wave ports).
    pub port: usize,
}

/// The FEM model of `win` (x0, y0, x1, y1, mil) on copper layer `layer`
/// (the top or the bottom), with its parts. Notes say what was assumed.
pub fn model(
    board: &Board,
    win: [f64; 4],
    layer: u8,
    lossy: bool,
    rough_um: f64,
) -> Result<(Model, Vec<Part>, Vec<String>), String> {
    let st = crate::fdtd::stack(&board.stackup);
    let nl = st.sheet.len();
    if nl < 2 {
        return Err("the stack-up needs two copper layers".into());
    }
    let l = layer as usize;
    let (refl, top) = match l {
        0 => (1, true),
        x if x == nl - 1 => (nl - 2, false),
        _ => return Err("planar solves an outer layer (the top or the bottom)".into()),
    };
    let mut notes = vec![format!(
        "{} is the reference plane, taken as solid under the whole window",
        st.names[refl]
    )];
    // z in the model: the plane at 0, the signal layer at h, air above.
    let zof = |zs: f64| -> f64 {
        if top {
            (st.sheet[refl] - zs) * MIL
        } else {
            (zs - st.sheet[refl]) * MIL
        }
    };
    let h = zof(st.sheet[l]);
    let air = 5.0 * h;
    let (x0, y0) = (win[0].min(win[2]), win[1].min(win[3]));
    let (x1, y1) = (win[0].max(win[2]), win[1].max(win[3]));
    let (wx, wy) = ((x1 - x0) * MIL, (y1 - y0) * MIL);
    let xy = |x: f64, y: f64| -> [f64; 2] { [(x - x0) * MIL, (y - y0) * MIL] };
    let inwin = |x: f64, y: f64, m: f64| x >= x0 - m && x <= x1 + m && y >= y0 - m && y <= y1 + m;
    let mut solids = Vec::new();
    for s in &st.slabs {
        let (a, b) = (zof(s.0), zof(s.1));
        let (lo, hi) = (a.min(b).max(0.0), a.max(b).min(h));
        if hi > lo {
            solids.push(Solid {
                shape: Shape::Box {
                    lo: [0.0, 0.0, lo],
                    hi: [wx, wy, hi],
                },
                fill: Fill::Dielectric {
                    er: s.2,
                    tand: if lossy { s.3 } else { 0.0 },
                },
            });
        }
    }
    // Ground: nets named like GND, and the net with most copper on the
    // reference layer.
    let geo = &board.geometry;
    let mut on_ref = std::collections::HashMap::new();
    for p in &geo.polygons {
        if p.layer == LayerRef::Copper(refl as u8) {
            if let Some(n) = p.net {
                *on_ref.entry(n).or_insert(0usize) += p.outline.len();
            }
        }
    }
    let ref_net = on_ref.into_iter().max_by_key(|x| x.1).map(|x| x.0);
    let is_gnd = |n: Option<u32>| -> bool {
        n.is_some_and(|n| {
            Some(n) == ref_net
                || board.nets.get(n as usize).is_some_and(|x| {
                    let u = x.name.to_ascii_uppercase();
                    u.contains("GND") || u.contains("VSS")
                })
        })
    };
    let mnet = |n: Option<u32>| -> u32 {
        match n {
            _ if is_gnd(n) => 0,
            Some(n) => n + 1,
            None => u32::MAX - 1,
        }
    };
    let sheet = |poly: Vec<[f64; 2]>, net: u32| Sheet {
        axis: 2,
        at: h,
        poly,
        holes: Vec::new(),
        disc: None,
        net,
    };
    let on_layer = |r: &LayerRef| *r == LayerRef::Copper(layer) || *r == LayerRef::AllCopper;
    let mut sheets = Vec::new();
    let mut ports = Vec::new();
    let pname = |net: Option<u32>| {
        net.and_then(|n| board.nets.get(n as usize))
            .map_or("?".to_string(), |x| x.name.clone())
    };
    for t in geo.tracks.iter().filter(|t| on_layer(&t.layer) && !t.text) {
        let (a, b) = (
            [t.start.x.mil(), t.start.y.mil()],
            [t.end.x.mil(), t.end.y.mil()],
        );
        let w = t.width.mil();
        if !inwin(a[0], a[1], w) && !inwin(b[0], b[1], w) {
            continue;
        }
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0].hypot(d[1]).max(1e-9);
        let (ux, uy) = (d[0] / len, d[1] / len);
        let (nx, ny) = (-uy * w / 2.0, ux * w / 2.0);
        // Square ends half a width out (the round cap, near enough).
        let (ax, ay) = (a[0] - ux * w / 2.0, a[1] - uy * w / 2.0);
        let (bx, by) = (b[0] + ux * w / 2.0, b[1] + uy * w / 2.0);
        let poly = vec![
            xy(ax + nx, ay + ny),
            xy(ax - nx, ay - ny),
            xy(bx - nx, by - ny),
            xy(bx + nx, by + ny),
        ];
        let net = mnet(t.net);
        sheets.push(sheet(poly, net));
        // A track leaving the window: a wave port where it crosses.
        if net != 0 {
            for (p, q) in [(a, b), (b, a)] {
                if inwin(p[0], p[1], 0.0) || !inwin(q[0], q[1], 0.0) {
                    continue;
                }
                // p outside, q inside: the face it crosses.
                let (axis, high) = if p[0] < x0 {
                    (0, false)
                } else if p[0] > x1 {
                    (0, true)
                } else if p[1] < y0 {
                    (1, false)
                } else {
                    (1, true)
                };
                let along = if axis == 0 { 1 } else { 0 };
                let at = |s: f64| -> f64 {
                    // Where the track meets the face, along the other axis.
                    let face = if axis == 0 {
                        if high {
                            x1
                        } else {
                            x0
                        }
                    } else if high {
                        y1
                    } else {
                        y0
                    };
                    let (pa, qa) = (p[axis], q[axis]);
                    let k = ((face - pa) / (qa - pa)).clamp(0.0, 1.0);
                    p[along] + (q[along] - p[along]) * k + s
                };
                let half = w / 2.0 + 3.0 * h / MIL;
                let (lo_a, hi_a) = (
                    if along == 0 { x0 } else { y0 },
                    if along == 0 { x1 } else { y1 },
                );
                let (c0, c1) = ((at(-half)).max(lo_a), (at(half)).min(hi_a));
                ports.push(Port {
                    name: format!("P{} {}", ports.len() + 1, pname(t.net)),
                    axis,
                    high,
                    lo: [(c0 - lo_a) * MIL, 0.0],
                    hi: [(c1 - lo_a) * MIL, h + air],
                    mode: Mode::Tem { signal: net },
                    deembed: 0.0,
                });
            }
        }
    }
    // Pads.
    let mut parts_pads: std::collections::BTreeMap<u32, Vec<&crate::geometry::Pad>> =
        Default::default();
    for p in geo.pads.iter().filter(|p| on_layer(&p.layer)) {
        let (cx, cy) = (p.at.x.mil(), p.at.y.mil());
        let (sx, sy) = (p.size_x.mil(), p.size_y.mil());
        if !inwin(cx, cy, sx.max(sy)) {
            continue;
        }
        if let Some(c) = p.component {
            parts_pads.entry(c).or_default().push(p);
        }
        let net = mnet(p.net);
        if !p.outline.is_empty() {
            sheets.push(sheet(
                p.outline.iter().map(|q| xy(q.x.mil(), q.y.mil())).collect(),
                net,
            ));
        } else if p.shape == PadShape::Round && (sx - sy).abs() < 1e-6 {
            let mut s = sheet(Vec::new(), net);
            s.disc = Some((xy(cx, cy), sx / 2.0 * MIL));
            sheets.push(s);
        } else {
            let (c, sn) = (
                p.rotation_deg.to_radians().cos(),
                p.rotation_deg.to_radians().sin(),
            );
            let corner = |u: f64, v: f64| xy(cx + u * c - v * sn, cy + u * sn + v * c);
            sheets.push(sheet(
                vec![
                    corner(-sx / 2.0, -sy / 2.0),
                    corner(sx / 2.0, -sy / 2.0),
                    corner(sx / 2.0, sy / 2.0),
                    corner(-sx / 2.0, sy / 2.0),
                ],
                net,
            ));
        }
    }
    // Pours on the layer (holes joined in by zero-width cuts).
    for p in geo.polygons.iter().filter(|p| on_layer(&p.layer)) {
        if !p.outline.iter().any(|q| inwin(q.x.mil(), q.y.mil(), 0.0))
            && !p.outline.is_empty()
            && !{
                let (mut lx, mut ly, mut hx, mut hy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for q in &p.outline {
                    lx = lx.min(q.x.mil());
                    ly = ly.min(q.y.mil());
                    hx = hx.max(q.x.mil());
                    hy = hy.max(q.y.mil());
                }
                lx < x1 && hx > x0 && ly < y1 && hy > y0
            }
        {
            continue;
        }
        let f = crate::geometry::fracture(&p.outline, &p.holes);
        // fracture gives mm.
        let poly = f.iter().map(|q| xy(q[0] / 0.0254, q[1] / 0.0254)).collect();
        sheets.push(sheet(poly, mnet(p.net)));
    }
    // Vias: a ground via is a post from its pad to the plane; a signal via
    // that leaves the layer is left open (noted).
    let mut open_vias = 0;
    for v in &geo.vias {
        let (cx, cy) = (v.at.x.mil(), v.at.y.mil());
        if !inwin(cx, cy, 0.0) {
            continue;
        }
        let spans = |k: usize| (v.from_layer as usize) <= k && k <= v.to_layer as usize;
        if !spans(l) {
            continue;
        }
        let mut s = sheet(Vec::new(), mnet(v.net));
        s.disc = Some((xy(cx, cy), v.diameter.mil() / 2.0 * MIL));
        sheets.push(s);
        if is_gnd(v.net) && spans(refl) {
            solids.push(Solid {
                shape: Shape::Cyl {
                    axis: 2,
                    c: xy(cx, cy),
                    r: v.hole.mil() / 2.0 * MIL,
                    lo: 0.0,
                    hi: h,
                },
                fill: Fill::Pec { net: 0 },
            });
        } else if !is_gnd(v.net) {
            open_vias += 1;
        }
    }
    if open_vias > 0 {
        notes.push(format!(
            "{open_vias} signal vias leave the layer and are left open"
        ));
    }
    let (lumped, mut parts) = part_ports(board, &parts_pads, &xy, h, &mut notes);
    // Lumped port indices count after all the wave ports.
    let nw = ports.len();
    for (k, p) in parts.iter_mut().enumerate() {
        p.port = nw + k;
    }
    if ports.is_empty() {
        return Err("no trace leaves the window: nothing to drive (widen the window so the line in and out cross its edge)".into());
    }
    let m = Model {
        lo: [0.0; 3],
        hi: [wx, wy, h + air],
        walls: [Wall::Pec; 6],
        er: 1.0,
        solids,
        sheets,
        ports,
        copper: lossy.then_some(Copper {
            rough_um,
            walls: [false, false, false, false, true, false],
        }),
        lumped,
    };
    Ok((m, parts, notes))
}

/// Two-pad parts: a lumped port across the gap between their pads, in the
/// plane `z` (model metres; `xy` maps board mil to the model). Part ports
/// count from 0 here; the caller adds the wave ports.
pub(crate) fn part_ports(
    board: &Board,
    parts_pads: &std::collections::BTreeMap<u32, Vec<&crate::geometry::Pad>>,
    xy: &dyn Fn(f64, f64) -> [f64; 2],
    z: f64,
    notes: &mut Vec<String>,
) -> (Vec<Lumped>, Vec<Part>) {
    // Two-pad parts: a lumped port across the gap between their pads.
    let mut lumped = Vec::new();
    let mut parts = Vec::new();
    for (c, pads) in parts_pads {
        let comp = &board.components[*c as usize];
        if pads.len() != 2 {
            continue;
        }
        let (a, b) = (pads[0], pads[1]);
        if a.net == b.net {
            continue;
        }
        let ca = [a.at.x.mil(), a.at.y.mil()];
        let cb = [b.at.x.mil(), b.at.y.mil()];
        let (dx, dy) = (cb[0] - ca[0], cb[1] - ca[1]);
        let dir = if dx.abs() >= dy.abs() { 0 } else { 1 };
        let o = 1 - dir;
        if [dx, dy][o].abs() > 0.2 * [dx, dy][dir].abs() {
            notes.push(format!("{} is not along x or y: left out", comp.designator));
            continue;
        }
        // Pad extents along each axis (0 or 90 degrees).
        let ext = |p: &crate::geometry::Pad| -> [f64; 2] {
            let r = (p.rotation_deg.rem_euclid(180.0) - 90.0).abs() < 45.0;
            let (sx, sy) = (p.size_x.mil(), p.size_y.mil());
            if r {
                [sy, sx]
            } else {
                [sx, sy]
            }
        };
        let (ea, eb) = (ext(a), ext(b));
        let (lo_p, hi_p, el, eh) = if [ca, cb][0][dir] <= [ca, cb][1][dir] {
            (ca, cb, ea, eb)
        } else {
            (cb, ca, eb, ea)
        };
        let mut g0 = lo_p[dir] + el[dir] / 2.0;
        let mut g1 = hi_p[dir] - eh[dir] / 2.0;
        if g1 - g0 < 1.0 {
            let m = (g0 + g1) / 2.0;
            g0 = m - 0.5;
            g1 = m + 0.5;
        }
        let wid = el[o].min(eh[o]);
        let mid = (lo_p[o] + hi_p[o]) / 2.0;
        let (mut lo, mut hi) = ([0.0; 2], [0.0; 2]);
        lo[dir] = g0;
        hi[dir] = g1;
        lo[o] = mid - wid / 2.0;
        hi[o] = mid + wid / 2.0;
        let (lo, hi) = (xy(lo[0], lo[1]), xy(hi[0], hi[1]));
        let value = ["Value", "VALUE", "value", "Comment"]
            .iter()
            .find_map(|k| comp.params.get(*k).cloned())
            .unwrap_or_default();
        parts.push(Part {
            designator: comp.designator.clone(),
            value,
            port: lumped.len(),
        });
        lumped.push(Lumped {
            name: comp.designator.clone(),
            z,
            lo,
            hi,
            dir,
            r: 50.0,
        });
    }
    (lumped, parts)
}

/// What a part's value is as a load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Load {
    Open,
    Short,
    R(f64),
    L(f64),
    C(f64),
}

/// "1.5nH", "2.2 pF", "0R", "49.9", "10k", "DNP", "open": the part's
/// load (the designator's letter says L, C or R when the value has no
/// unit).
pub fn parse_load(designator: &str, value: &str) -> Option<Load> {
    let v = value
        .trim()
        .to_ascii_lowercase()
        .replace(['µ', 'μ'], "u")
        .replace(' ', "");
    if v.is_empty() {
        return None;
    }
    if ["dnp", "dnf", "nc", "open", "nf"].contains(&v.as_str()) {
        return Some(Load::Open);
    }
    if ["short", "0", "0r", "0ohm", "0ω"].contains(&v.as_str()) {
        return Some(Load::Short);
    }
    let end = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let num: f64 = v[..end].parse().ok()?;
    let rest = &v[end..];
    let (exp, tail) = match rest.chars().next() {
        Some('p') => (-12, &rest[1..]),
        Some('n') => (-9, &rest[1..]),
        Some('u') => (-6, &rest[1..]),
        Some('m') => (-3, &rest[1..]),
        Some('k') => (3, &rest[1..]),
        _ => (0, rest),
    };
    // 4n7: the prefix stands for the point.
    let (num, tail) = match tail.find(|c: char| !c.is_ascii_digit()) {
        Some(0) | None if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() => {
            (format!("{}.{}", &v[..end], tail).parse().unwrap_or(num), "")
        }
        _ => (num, tail),
    };
    // Scaled as text, so 1.5n is exactly 1.5e-9.
    let x: f64 = format!("{num}e{exp}").parse().unwrap_or(num);
    let kind = match tail.chars().next() {
        Some('h') => 'l',
        Some('f') => 'c',
        Some('r') | Some('ω') | Some('o') => 'r',
        _ => designator
            .chars()
            .next()
            .map_or('r', |c| c.to_ascii_lowercase()),
    };
    Some(match kind {
        'l' => Load::L(x),
        'c' => Load::C(x),
        _ if x == 0.0 => Load::Short,
        _ => Load::R(x),
    })
}

impl Load {
    /// Reflection on reference `r` ohms at `f_ghz`.
    pub fn gamma(&self, f_ghz: f64, r: f64) -> c64 {
        let w = 2.0 * std::f64::consts::PI * f_ghz * 1e9;
        let z = match *self {
            Load::Open => return c64::new(1.0, 0.0),
            Load::Short => return c64::new(-1.0, 0.0),
            Load::R(x) => c64::new(x, 0.0),
            Load::L(l) => c64::new(0.0, w * l),
            Load::C(c) => c64::new(0.0, -1.0 / (w * c).max(1e-30)),
        };
        (z - r) / (z + r)
    }

    pub fn text(&self) -> String {
        match *self {
            Load::Open => "open".into(),
            Load::Short => "short".into(),
            Load::R(x) => format!("{x} Ω"),
            Load::L(l) => format!("{:.3} nH", l * 1e9),
            Load::C(c) => format!("{:.3} pF", c * 1e12),
        }
    }
}

/// The wave ports' S at each frequency with the parts' loads on their
/// lumped ports (`loads[k]` for part k; None leaves that port open).
pub fn with_loads(sol: &fem3d::Solution, parts: &[Part], loads: &[Option<Load>]) -> Vec<Vec<Cx>> {
    let n = sol.n();
    sol.f
        .iter()
        .zip(&sol.s)
        .map(|(&f, s)| {
            let l: Vec<(usize, c64)> = parts
                .iter()
                .zip(loads)
                .map(|(p, ld)| {
                    let r = sol.z_ref.unwrap_or(50.0);
                    (p.port, ld.unwrap_or(Load::Open).gamma(f, r))
                })
                .collect();
            fem3d::terminate(s, n, &l)
                .into_iter()
                .map(|z| Cx::new(z.re, z.im))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_values() {
        assert_eq!(parse_load("L3", "1.5nH"), Some(Load::L(1.5e-9)));
        assert_eq!(parse_load("C1", "2.2pF"), Some(Load::C(2.2e-12)));
        assert_eq!(parse_load("C1", "4p7"), Some(Load::C(4.7e-12)));
        assert_eq!(parse_load("R2", "0R"), Some(Load::Short));
        assert_eq!(parse_load("R2", "49.9"), Some(Load::R(49.9)));
        assert_eq!(parse_load("C9", "DNP"), Some(Load::Open));
        let g = Load::R(50.0).gamma(1.0, 50.0);
        assert!(g.norm() < 1e-12);
    }
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use crate::{Component, Length, Net, Pad, Point, Track};

    /// A 50 Ω microstrip broken by C1 (two pads, one each side of a gap).
    fn board() -> Board {
        let mut b = Board {
            stackup: crate::stackfile::parse(
                "TOP, copper, 1.4mil\nCore, dielectric, 10mil, 3.5, 0.0\nL2, copper, 1.4mil\n",
            )
            .expect("stack-up"),
            ..Default::default()
        };
        for name in ["RF_IN", "RF_OUT"] {
            b.nets.push(Net {
                name: name.into(),
                routed_length: Length(0),
                via_length: Length(0),
                vias: 0,
                pads: 0,
                pieces: 1,
                has_pour: false,
            });
        }
        let mut c = Component {
            designator: "C1".into(),
            footprint: String::new(),
            library_ref: None,
            x: Length(0),
            y: Length(0),
            rotation_deg: 0.0,
            side: crate::Side::Top,
            sheet: None,
            params: Default::default(),
        };
        c.params.insert("Value".into(), "1pF".into());
        b.components.push(c);
        let pad = |name: &str, x: f64, net: u32| Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(0),
            name: name.into(),
            at: Point::mil(x, 50.0),
            size_x: Length::from_mil(16.0),
            size_y: Length::from_mil(22.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        };
        b.geometry.pads.push(pad("1", 88.0, 0));
        b.geometry.pads.push(pad("2", 112.0, 1));
        let tr = |x0: f64, x1: f64, net: u32| Track {
            layer: LayerRef::Copper(0),
            net: Some(net),
            start: Point::mil(x0, 50.0),
            end: Point::mil(x1, 50.0),
            width: Length::from_mil(22.0),
            component: None,
            text: false,
        };
        // Square caps add half a width: stop short of the pads' centres.
        b.geometry.tracks.push(tr(-20.0, 80.0, 0));
        b.geometry.tracks.push(tr(120.0, 220.0, 1));
        b
    }

    #[test]
    fn a_series_capacitor_in_a_microstrip() {
        let (m, parts, _) = model(&board(), [0.0, 0.0, 200.0, 100.0], 0, false, 0.0).unwrap();
        assert_eq!(m.ports.len(), 2);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].port, 2);
        let setup = fem3d::Setup {
            f_adapt: 4.0,
            sweep: vec![1.0, 2.0, 4.0],
            max_unknowns: 60_000,
            z_ref: Some(50.0),
            ..fem3d::Setup::default()
        };
        let sol = fem3d::solve(&m, &setup, |_| true).unwrap();
        let short = with_loads(&sol, &parts, &[Some(Load::Short)]);
        let open = with_loads(&sol, &parts, &[Some(Load::Open)]);
        let cap = with_loads(&sol, &parts, &[parse_load("C1", "1pF")]);
        for (k, &f) in sol.f.iter().enumerate() {
            // A series C between two 50 Ω lines.
            let x = 1.0 / (2.0 * std::f64::consts::PI * f * 1e9 * 1e-12);
            let want = 100.0 / (100.0f64.powi(2) + x * x).sqrt();
            let (s21, o21, c21) = (short[k][2].abs(), open[k][2].abs(), cap[k][2].abs());
            assert!(s21 > 0.9, "{f} GHz short S21 {s21}");
            assert!(o21 < 0.35, "{f} GHz open S21 {o21}");
            assert!(
                (c21 - want).abs() < 0.1,
                "{f} GHz 1 pF S21 {c21} want {want}"
            );
        }
    }
}
