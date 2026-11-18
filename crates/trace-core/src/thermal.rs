//! Steady-state 3D thermal analysis of a board with its parts and
//! heatsinks.
//!
//! The board is a finite-volume grid: square cells across it, and through
//! it one slab per copper layer and one or more per dielectric. Each
//! copper cell's conductivity comes from the copper really there (pours,
//! tracks, pads, via lands, sampled 4 x 4 per cell, with separate
//! in-plane values along x and y), and each via's plated barrel adds its
//! copper to the cells it passes through. A part dissipating power is a
//! two-resistor package (JEDEC JESD15-3): junction to case top (θJC) and
//! junction to board (θJB, into the copper under its pads). Its case
//! loses heat by convection and radiation, or through a thermal interface
//! (TIM plus contact resistance) into a plate-fin heatsink, whose base is
//! a grid of its own (so spreading in the base counts) with each fin a
//! one-dimensional fin with its exact efficiency. Air: natural convection
//! from correlations for horizontal or vertical plates and for the fin
//! channels (Bar-Cohen & Rohsenow), or forced flow (flat plate, and the
//! Teertstra-Yovanovich-Culham model in the channels), with air
//! properties at the film temperature, plus grey-body radiation. The
//! convection and radiation depend on the temperatures, so the solve
//! repeats until they settle; every solve checks that the heat leaving
//! equals the power put in.

#![allow(clippy::needless_range_loop)]

use crate::{Board, LayerKind, LayerRef, Pad};

const SIGMA: f64 = 5.670_374e-8;
const G: f64 = 9.81;
const MM: f64 = 1e-3;
const MIL_MM: f64 = 0.0254;

/// A plate-fin heatsink on a part.
#[derive(Clone, Debug)]
pub struct Heatsink {
    /// Base footprint, mm (length along the fins, width across them).
    pub length: f64,
    pub width: f64,
    /// Base thickness, mm.
    pub base: f64,
    /// Fin height above the base, mm; number of fins; fin thickness, mm.
    pub fin_height: f64,
    pub fins: usize,
    pub fin: f64,
    /// Conductivity, W/m·K (aluminium 6063: 200; copper: 390).
    pub k: f64,
    /// Thermal interface: thickness (mm) and conductivity (W/m·K), and
    /// the contact resistance of its two faces together (K·cm²/W).
    pub tim_mm: f64,
    pub tim_k: f64,
    pub contact: f64,
    /// Surface emissivity (anodised 0.85, bare 0.1).
    pub emissivity: f64,
}

impl Default for Heatsink {
    fn default() -> Self {
        Heatsink {
            length: 30.0,
            width: 30.0,
            base: 3.0,
            fin_height: 10.0,
            fins: 10,
            fin: 1.0,
            k: 200.0,
            tim_mm: 0.1,
            tim_k: 3.0,
            contact: 0.05,
            emissivity: 0.85,
        }
    }
}

impl Heatsink {
    /// Gap between fins, mm.
    pub fn spacing(&self) -> f64 {
        let n = self.fins.max(1) as f64;
        ((self.width - n * self.fin) / (n - 1.0).max(1.0)).max(0.05)
    }

    /// Parses "LxWxH[,fins=N][,fin=T][,base=B][,k=K][,tim=T@K][,contact=R][,e=E]"
    /// where H is the total height (base and fins), mm.
    pub fn parse(s: &str) -> std::result::Result<Heatsink, String> {
        let mut h = Heatsink::default();
        let mut parts = s.split(',');
        let dims = parts.next().unwrap_or("");
        let d: Vec<f64> = dims
            .split(['x', 'X', '*'])
            .map(|t| t.trim().trim_end_matches("mm").parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| format!("heatsink: not LxWxH: {dims}"))?;
        if d.len() != 3 || d.iter().any(|&v| v <= 0.0) {
            return Err(format!("heatsink: not LxWxH: {dims}"));
        }
        h.length = d[0];
        h.width = d[1];
        let total = d[2];
        let mut fins_given = false;
        for p in parts {
            let (k, v) = p
                .split_once('=')
                .ok_or(format!("heatsink: not key=value: {p}"))?;
            let num = |t: &str| -> std::result::Result<f64, String> {
                t.trim()
                    .trim_end_matches("mm")
                    .parse::<f64>()
                    .map_err(|_| format!("heatsink: not a number: {t}"))
            };
            match k.trim() {
                "fins" => {
                    h.fins = num(v)? as usize;
                    fins_given = true;
                }
                "fin" => h.fin = num(v)?,
                "base" => h.base = num(v)?,
                "k" => h.k = num(v)?,
                "tim" => {
                    let (t, kk) = v.split_once('@').unwrap_or((v, "3"));
                    h.tim_mm = num(t)?;
                    h.tim_k = num(kk)?;
                }
                "contact" => h.contact = num(v)?,
                "e" | "emissivity" => h.emissivity = num(v)?,
                _ => return Err(format!("heatsink: unknown {k}")),
            }
        }
        h.base = h.base.min(total * 0.8);
        h.fin_height = (total - h.base).max(0.0);
        if !fins_given {
            // A common pitch: fins about 2.5 times their thickness apart.
            h.fins = ((h.width + 2.5 * h.fin) / (3.5 * h.fin)).floor().max(2.0) as usize;
        }
        if h.fins as f64 * h.fin >= h.width {
            return Err("heatsink: the fins do not fit across the base".into());
        }
        Ok(h)
    }

    pub fn describe(&self) -> String {
        format!(
            "{:.0} x {:.0} mm base {:.1} mm, {} fins {:.1} mm thick, {:.1} mm tall, {:.1} mm apart, k {:.0}",
            self.length,
            self.width,
            self.base,
            self.fins,
            self.fin,
            self.fin_height,
            self.spacing(),
            self.k
        )
    }
}

/// A part dissipating power.
#[derive(Clone, Debug)]
pub struct Source {
    pub part: usize,
    pub watts: f64,
    /// Junction to case top and junction to board, K/W.
    pub theta_jc: f64,
    pub theta_jb: f64,
    pub heatsink: Option<Heatsink>,
}

/// How the air moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Air {
    /// Still air; the board lies flat (or stands on its edge).
    Natural { vertical: bool },
    /// Moving air along the board's x, m/s.
    Forced { speed: f64 },
}

#[derive(Clone, Debug)]
pub struct Setup {
    pub sources: Vec<Source>,
    pub ambient_c: f64,
    pub air: Air,
    /// Board surface emissivity (solder mask about 0.9).
    pub emissivity: f64,
    /// Cell size across the board, mm (0: about 160 cells along the
    /// longer side, 0.25 to 2 mm).
    pub cell_mm: f64,
    /// Dielectric conductivity in the plane and through it, W/m·K (FR-4
    /// about 0.8 and 0.3; None: from the material name).
    pub k_diel: Option<(f64, f64)>,
    /// Via plating thickness, mm.
    pub plating_mm: f64,
    /// A fixed heat transfer coefficient on both board faces, W/m²·K,
    /// with no radiation (for checks against exact answers).
    pub fixed_h: Option<f64>,
}

impl Setup {
    pub fn new() -> Setup {
        Setup {
            sources: Vec::new(),
            ambient_c: 25.0,
            air: Air::Natural { vertical: false },
            emissivity: 0.9,
            cell_mm: 0.0,
            k_diel: None,
            plating_mm: 0.025,
            fixed_h: None,
        }
    }
}

impl Default for Setup {
    fn default() -> Self {
        Setup::new()
    }
}

/// One part's result.
#[derive(Clone, Debug)]
pub struct PartTemp {
    pub part: usize,
    pub designator: String,
    pub watts: f64,
    pub junction_c: f64,
    pub case_c: f64,
    /// Mean board temperature under its pads.
    pub board_c: f64,
    /// Hottest point of its heatsink base, if it has one.
    pub heatsink_c: Option<f64>,
    /// Share of its power that leaves through the board (the rest through
    /// the case top).
    pub to_board: f64,
    /// Junction rise over ambient per watt, K/W.
    pub theta_ja: f64,
}

/// The temperature over one slab of the board.
#[derive(Clone, Debug)]
pub struct LayerMap {
    pub name: String,
    /// °C per cell, row by row from the bottom left; NaN off the board.
    pub t: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Result {
    pub parts: Vec<PartTemp>,
    /// One map per copper layer, top first.
    pub maps: Vec<LayerMap>,
    /// Grid: lower left corner (mm), cell (mm), columns, rows.
    pub origin: [f64; 2],
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub ambient_c: f64,
    /// Hottest board point: °C and where (mm), its layer.
    pub board_max: (f64, [f64; 2], usize),
    /// Heat transfer coefficients used on the top and bottom faces
    /// (convection only, and with radiation at the mean temperature).
    pub h_top: (f64, f64),
    pub h_bottom: (f64, f64),
    /// Heat leaving the board's faces and edges, the cases and the
    /// heatsinks, W; and the error of the balance against the power, %.
    pub out_board: f64,
    pub out_cases: f64,
    pub out_sinks: f64,
    pub balance_pct: f64,
    pub iterations: usize,
    pub nodes: usize,
    pub notes: Vec<String>,
    /// Temperature, conductivity and heat in every slab, for the density
    /// maps (`thermfield`).
    pub field: crate::thermfield::ThermField,
}

/// Air at one atmosphere: (k W/m·K, ν m²/s, α m²/s, Pr) at T kelvin
/// (Incropera, table A.4, linear between 250 and 450 K).
pub fn air(t: f64) -> (f64, f64, f64, f64) {
    const T: [f64; 5] = [250.0, 300.0, 350.0, 400.0, 450.0];
    const K: [f64; 5] = [22.3e-3, 26.3e-3, 30.0e-3, 33.8e-3, 37.3e-3];
    const NU: [f64; 5] = [11.44e-6, 15.89e-6, 20.92e-6, 26.41e-6, 32.39e-6];
    const AL: [f64; 5] = [15.9e-6, 22.5e-6, 29.9e-6, 38.3e-6, 47.2e-6];
    const PR: [f64; 5] = [0.720, 0.707, 0.700, 0.690, 0.686];
    let i = T.partition_point(|&x| x < t).clamp(1, 4);
    let f = (t - T[i - 1]) / (T[i] - T[i - 1]);
    let at = |v: &[f64; 5]| v[i - 1] + (v[i] - v[i - 1]) * f;
    (at(&K), at(&NU), at(&AL), at(&PR))
}

/// Natural or forced convection off a flat plate of size `l` (mm, along
/// the flow or the characteristic length A/P for a flat one), at surface
/// temperature `ts` and ambient `ta` (°C). `face`: 0 a horizontal face
/// looking up, 1 one looking down, 2 a vertical face of height `l`.
pub fn h_plate(air_flow: Air, face: u8, l_mm: f64, ts: f64, ta: f64) -> f64 {
    let dt = (ts - ta).abs().max(0.05);
    let tf = (ts + ta) / 2.0 + 273.15;
    let (k, nu, alpha, pr) = air(tf);
    let l = (l_mm * MM).max(1e-3);
    match air_flow {
        Air::Forced { speed } => {
            let re = speed.max(0.0) * l / nu;
            let nu_l = if re < 5e5 {
                0.664 * re.sqrt() * pr.powf(1.0 / 3.0)
            } else {
                (0.037 * re.powf(0.8) - 871.0) * pr.powf(1.0 / 3.0)
            };
            // Never below still air.
            let nat = h_plate(Air::Natural { vertical: false }, face, l_mm, ts, ta);
            (nu_l * k / l).max(nat)
        }
        Air::Natural { .. } => {
            let ra = G * dt * l.powi(3) / (tf * nu * alpha);
            // A hot face looking up (or a cold one looking down) lifts
            // its own air; looking down, the air must flow round the edge.
            let up = (face == 0) == (ts >= ta);
            let nu_l = match face {
                2 => {
                    let a = 0.387 * ra.powf(1.0 / 6.0)
                        / (1.0 + (0.492 / pr).powf(9.0 / 16.0)).powf(8.0 / 27.0);
                    (0.825 + a).powi(2)
                }
                _ if up => {
                    if ra < 1e7 {
                        0.54 * ra.max(1.0).powf(0.25)
                    } else {
                        0.15 * ra.powf(1.0 / 3.0)
                    }
                }
                _ => 0.52 * ra.max(1.0).powf(0.2),
            };
            nu_l.max(1.0) * k / l
        }
    }
}

/// Linearised radiation to surroundings at `ta`, W/m²·K.
pub fn h_rad(e: f64, ts: f64, ta: f64) -> f64 {
    let (a, b) = (ts + 273.15, ta + 273.15);
    e * SIGMA * (a * a + b * b) * (a + b)
}

/// Heat transfer coefficient between a fin channel's walls and the air,
/// W/m²·K: Bar-Cohen & Rohsenow for still air (channel `l` long in the
/// direction the air rises), Teertstra-Yovanovich-Culham for forced air.
pub fn h_channel(air_flow: Air, s_mm: f64, l_mm: f64, fin_mm: f64, ts: f64, ta: f64) -> f64 {
    let dt = (ts - ta).abs().max(0.05);
    let tf = (ts + ta) / 2.0 + 273.15;
    let (k, nu, alpha, pr) = air(tf);
    let (s, l) = (s_mm * MM, l_mm * MM);
    match air_flow {
        Air::Natural { .. } => {
            let el = G * dt * s.powi(4) / (tf * nu * alpha * l);
            let nu_s = (576.0 / (el * el) + 2.873 / el.sqrt()).powf(-0.5);
            nu_s * k / s
        }
        Air::Forced { speed } => {
            // The air speeds up between the fins.
            let u = speed * (s_mm + fin_mm) / s_mm;
            let re = u * s / nu * s / l;
            let a = (re * pr / 2.0).powi(-3);
            let b =
                (0.664 * re.sqrt() * pr.powf(1.0 / 3.0) * (1.0 + 3.65 / re.sqrt()).sqrt()).powi(-3);
            let nu_s = (a + b).powf(-1.0 / 3.0);
            let forced = nu_s * k / s;
            let still = h_channel(Air::Natural { vertical: false }, s_mm, l_mm, fin_mm, ts, ta);
            forced.max(still)
        }
    }
}

/// A straight fin's efficiency: tanh(mLc) / (mLc), Lc = H + t/2.
pub fn fin_efficiency(h: f64, k: f64, t_mm: f64, height_mm: f64) -> f64 {
    let m = (2.0 * h / (k * t_mm * MM)).sqrt();
    let lc = (height_mm + t_mm / 2.0) * MM;
    let x = m * lc;
    if x < 1e-9 {
        1.0
    } else {
        x.tanh() / x
    }
}

fn in_ring(v: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = v.len().wrapping_sub(1);
    for i in 0..v.len() {
        let (a, b) = (v[i], v[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Whether `p` (mm) lies on a pad's copper on copper layer `l`.
fn on_pad(pad: &Pad, l: u8, nl: usize, p: [f64; 2]) -> bool {
    let shape = |outline: &[crate::Point], kind: crate::PadShape, sx: f64, sy: f64, rot: f64| {
        if outline.len() >= 3 {
            let v: Vec<[f64; 2]> = outline.iter().map(|q| [q.x.mm(), q.y.mm()]).collect();
            return in_ring(&v, p);
        }
        let (s, c) = (-rot).to_radians().sin_cos();
        let (dx, dy) = (p[0] - pad.at.x.mm(), p[1] - pad.at.y.mm());
        let (x, y) = (dx * c - dy * s, dx * s + dy * c);
        let (w, h) = (sx / 2.0, sy / 2.0);
        match kind {
            crate::PadShape::Rect | crate::PadShape::Octagon => x.abs() <= w && y.abs() <= h,
            crate::PadShape::Round => {
                let r = w.min(h);
                let (ex, ey) = ((x.abs() - (w - r)).max(0.0), (y.abs() - (h - r)).max(0.0));
                ex * ex + ey * ey <= r * r
            }
        }
    };
    let on_layer = match pad.layer {
        LayerRef::Copper(k) => k == l,
        LayerRef::AllCopper => (l as usize) < nl,
        _ => false,
    };
    if !on_layer {
        return false;
    }
    if !pad.stack.is_empty() {
        return pad.stack.iter().any(|land| {
            land.layer == l
                && shape(
                    &land.outline,
                    land.shape,
                    land.size_x.mm(),
                    land.size_y.mm(),
                    land.rotation_deg,
                )
        });
    }
    shape(
        &pad.outline,
        pad.shape,
        pad.size_x.mm(),
        pad.size_y.mm(),
        pad.rotation_deg,
    )
}

/// Dielectric conductivity (in plane, through) from a material name.
fn k_material(name: Option<&str>) -> (f64, f64) {
    let n = name.unwrap_or("").to_ascii_uppercase();
    if n.contains("POLYIMIDE") || n.contains("KAPTON") || n.contains("PI ") || n == "PI" {
        (0.2, 0.12)
    } else if n.contains("RO4") || n.contains("ROGERS") {
        (0.69, 0.69)
    } else if n.contains("PTFE") || n.contains("RT/DUROID") || n.contains("TEFLON") {
        (0.25, 0.2)
    } else if n.contains("CERAMIC") || n.contains("ALUMINA") {
        (25.0, 25.0)
    } else if n.contains("IMS") || n.contains("ALUMINIUM") || n.contains("ALUMINUM") {
        (2.0, 2.0)
    } else {
        // FR-4 and most epoxy-glass laminates and prepregs.
        (0.81, 0.29)
    }
}

/// A slab of the board through its thickness.
#[derive(Clone, Debug)]
struct Slab {
    t: f64,
    name: String,
    /// Copper layer index, or None for dielectric / mask.
    copper: Option<usize>,
    k_in: f64,
    k_z: f64,
}

const K_CU: f64 = 390.0;

/// Solves the board's temperatures.
pub fn solve(board: &Board, setup: &Setup) -> std::result::Result<Result, String> {
    let g = &board.geometry;
    if setup.sources.is_empty() {
        return Err("no part dissipates power: give at least one --power".into());
    }
    let mut notes = Vec::new();
    // The board's extent and outline (mm).
    let outline: Vec<[f64; 2]> = g.outline.iter().map(|p| [p.x.mm(), p.y.mm()]).collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    let mut grow = |p: [f64; 2]| {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    };
    if outline.len() >= 3 {
        outline.iter().for_each(|&p| grow(p));
    } else {
        for p in &g.pads {
            grow([p.at.x.mm(), p.at.y.mm()]);
        }
        for t in &g.tracks {
            grow([t.start.x.mm(), t.start.y.mm()]);
            grow([t.end.x.mm(), t.end.y.mm()]);
        }
        notes.push("no board outline: the copper's extent is used".into());
    }
    if lo[0] >= hi[0] || lo[1] >= hi[1] {
        return Err("the board has no extent".into());
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let cell = if setup.cell_mm > 0.0 {
        setup.cell_mm
    } else {
        (span / 160.0).clamp(0.25, 2.0)
    };
    let cols = ((hi[0] - lo[0]) / cell).ceil() as usize + 1;
    let rows = ((hi[1] - lo[1]) / cell).ceil() as usize + 1;
    let origin = [lo[0] - cell * 0.5, lo[1] - cell * 0.5];
    let per = cols * rows;
    if per > 2_000_000 {
        return Err("the grid would be too large: use a larger cell".into());
    }
    let centre = |c: usize| {
        [
            origin[0] + ((c % cols) as f64 + 0.5) * cell,
            origin[1] + ((c / cols) as f64 + 0.5) * cell,
        ]
    };
    let slots: Vec<Vec<[f64; 2]>> = g
        .slots
        .iter()
        .map(|s| s.iter().map(|p| [p.x.mm(), p.y.mm()]).collect())
        .collect();
    let on_board: Vec<bool> = (0..per)
        .map(|c| {
            let p = centre(c);
            (outline.len() < 3 || in_ring(&outline, p))
                && !slots.iter().any(|s| s.len() >= 3 && in_ring(s, p))
        })
        .collect();
    // The stack-up's slabs, top first.
    let mut slabs: Vec<Slab> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut ci = 0usize;
    for l in &board.stackup.layers {
        let t = l.thickness.mm() * MM;
        match l.kind {
            LayerKind::Copper => {
                slabs.push(Slab {
                    t: if t > 0.0 { t } else { 0.035 * MM },
                    name: l.name.clone(),
                    copper: Some(ci),
                    k_in: 0.0,
                    k_z: 0.0,
                });
                names.push(l.name.clone());
                ci += 1;
            }
            LayerKind::Dielectric | LayerKind::SolderMask if t > 0.0 => {
                let (ki, kz) = if l.kind == LayerKind::SolderMask {
                    (0.25, 0.25)
                } else {
                    setup
                        .k_diel
                        .unwrap_or_else(|| k_material(l.material.as_deref()))
                };
                // Thick cores in slabs of at most 0.4 mm.
                let n = ((t / (0.4 * MM)).ceil() as usize).max(1);
                for k in 0..n {
                    slabs.push(Slab {
                        t: t / n as f64,
                        name: if n > 1 {
                            format!("{} ({}/{n})", l.name, k + 1)
                        } else {
                            l.name.clone()
                        },
                        copper: None,
                        k_in: ki,
                        k_z: kz,
                    });
                }
            }
            _ => {}
        }
    }
    let nl = ci;
    if nl == 0 {
        // No stack-up: a 1.6 mm two-layer FR-4 board.
        slabs = vec![Slab {
            t: 0.035 * MM,
            name: "Top".into(),
            copper: Some(0),
            k_in: 0.0,
            k_z: 0.0,
        }];
        for k in 0..3 {
            slabs.push(Slab {
                t: 0.53 * MM,
                name: format!("FR-4 ({}/3)", k + 1),
                copper: None,
                k_in: 0.81,
                k_z: 0.29,
            });
        }
        slabs.push(Slab {
            t: 0.035 * MM,
            name: "Bottom".into(),
            copper: Some(1),
            k_in: 0.0,
            k_z: 0.0,
        });
        names = vec!["Top".into(), "Bottom".into()];
        notes.push("no stack-up: a 1.6 mm two-layer FR-4 board".into());
    }
    let nl = names.len();
    let ns = slabs.len();
    // The resin between copper: the dielectric next to each copper slab.
    let resin: Vec<(f64, f64)> = (0..ns)
        .map(|s| {
            let near = (1..ns).find_map(|d| {
                [s.checked_sub(d), Some(s + d)]
                    .into_iter()
                    .flatten()
                    .find(|&q| q < ns && slabs[q].copper.is_none())
            });
            near.map_or((0.81, 0.29), |q| (slabs[q].k_in, slabs[q].k_z))
        })
        .collect();
    // Copper on each layer, sampled 4 x 4 per cell: in-plane kx, ky
    // (mean of the series-parallel and parallel-series bounds) and kz.
    const SUB: usize = 4;
    let fine_cols = cols * SUB;
    let fine_rows = rows * SUB;
    let fine = cell / SUB as f64;
    let mut kxyz: Vec<Vec<[f32; 3]>> = Vec::with_capacity(nl);
    let mut cu_share: Vec<f64> = Vec::new();
    for l in 0..nl {
        let mut cu = vec![false; fine_cols * fine_rows];
        let at = |i: usize, j: usize| {
            [
                origin[0] + (i as f64 + 0.5) * fine,
                origin[1] + (j as f64 + 0.5) * fine,
            ]
        };
        // Pours and fills: scanlines, even-odd over all rings.
        for p in &g.polygons {
            if p.layer != LayerRef::Copper(l as u8) {
                continue;
            }
            let rings: Vec<Vec<[f64; 2]>> = std::iter::once(&p.outline)
                .chain(&p.holes)
                .map(|r| r.iter().map(|q| [q.x.mm(), q.y.mm()]).collect())
                .collect();
            for j in 0..fine_rows {
                let y = at(0, j)[1];
                let mut xs: Vec<f64> = Vec::new();
                for r in &rings {
                    for k in 0..r.len() {
                        let (a, b) = (r[k], r[(k + 1) % r.len()]);
                        if (a[1] > y) != (b[1] > y) {
                            xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                        }
                    }
                }
                xs.sort_by(f64::total_cmp);
                for span in xs.chunks(2) {
                    if let [x0, x1] = *span {
                        let i0 = ((x0 - origin[0]) / fine - 0.5).ceil().max(0.0) as usize;
                        let i1 = ((x1 - origin[0]) / fine - 0.5).floor();
                        if i1 < 0.0 {
                            continue;
                        }
                        for i in i0..=(i1 as usize).min(fine_cols - 1) {
                            cu[j * fine_cols + i] = true;
                        }
                    }
                }
            }
        }
        let stroke = |a: [f64; 2], b: [f64; 2], w: f64, cu: &mut Vec<bool>| {
            let r = w / 2.0;
            let i0 = ((a[0].min(b[0]) - r - origin[0]) / fine).floor().max(0.0) as usize;
            let j0 = ((a[1].min(b[1]) - r - origin[1]) / fine).floor().max(0.0) as usize;
            let i1 = (((a[0].max(b[0]) + r - origin[0]) / fine).ceil() as usize).min(fine_cols - 1);
            let j1 = (((a[1].max(b[1]) + r - origin[1]) / fine).ceil() as usize).min(fine_rows - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if seg_dist(at(i, j), a, b) <= r.max(fine * 0.5) {
                        cu[j * fine_cols + i] = true;
                    }
                }
            }
        };
        for t in &g.tracks {
            if t.layer == LayerRef::Copper(l as u8) && !t.text {
                stroke(
                    [t.start.x.mm(), t.start.y.mm()],
                    [t.end.x.mm(), t.end.y.mm()],
                    t.width.mm(),
                    &mut cu,
                );
            }
        }
        for a in &g.arcs {
            if a.layer != LayerRef::Copper(l as u8) {
                continue;
            }
            let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
            if sweep == 0.0 {
                sweep = 360.0;
            }
            let n = ((sweep / 10.0).ceil() as usize).max(2);
            let (cx, cy, r) = (a.center.x.mm(), a.center.y.mm(), a.radius.mm());
            let pt = |k: usize| {
                let t = (a.start_deg + sweep * k as f64 / n as f64).to_radians();
                [cx + r * t.cos(), cy + r * t.sin()]
            };
            for k in 0..n {
                stroke(pt(k), pt(k + 1), a.width.mm(), &mut cu);
            }
        }
        for p in &g.pads {
            let r = p.size_x.mm().max(p.size_y.mm()) / 2.0 + fine;
            let r = p
                .stack
                .iter()
                .map(|s| s.size_x.mm().max(s.size_y.mm()) / 2.0 + fine)
                .fold(r, f64::max);
            let (px, py) = (p.at.x.mm(), p.at.y.mm());
            let i0 = ((px - r - origin[0]) / fine).floor().max(0.0) as usize;
            let j0 = ((py - r - origin[1]) / fine).floor().max(0.0) as usize;
            let i1 = (((px + r - origin[0]) / fine).ceil() as usize).min(fine_cols - 1);
            let j1 = (((py + r - origin[1]) / fine).ceil() as usize).min(fine_rows - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if on_pad(p, l as u8, nl, at(i, j)) {
                        cu[j * fine_cols + i] = true;
                    }
                }
            }
        }
        for v in &g.vias {
            let (a, b) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
            if (l as u8) < a || (l as u8) > b {
                continue;
            }
            let d = v
                .rings
                .iter()
                .find(|r| r.0 == l as u8)
                .map_or(v.diameter.mm(), |r| r.1.mm());
            let c = [v.at.x.mm(), v.at.y.mm()];
            stroke(c, c, d, &mut cu);
        }
        // Per cell: share of copper and the two bounds each way.
        let (ki, kz) = resin[slabs.iter().position(|s| s.copper == Some(l)).unwrap_or(0)];
        let mut k3 = vec![[0f32; 3]; per];
        let mut total = 0usize;
        let mut count = 0usize;
        for c in 0..per {
            if !on_board[c] {
                continue;
            }
            let (ic, jc) = (c % cols, c / cols);
            let k_of = |i: usize, j: usize| {
                if cu[(jc * SUB + j) * fine_cols + ic * SUB + i] {
                    K_CU
                } else {
                    ki
                }
            };
            let mut n_cu = 0;
            // Along x: rows in parallel, each a series of samples.
            let (mut sp_x, mut sp_y) = (0.0, 0.0);
            for j in 0..SUB {
                let mut r = 0.0;
                for i in 0..SUB {
                    r += 1.0 / k_of(i, j);
                    if k_of(i, j) == K_CU {
                        n_cu += 1;
                    }
                }
                sp_x += SUB as f64 / r;
            }
            sp_x /= SUB as f64;
            for i in 0..SUB {
                let mut r = 0.0;
                for j in 0..SUB {
                    r += 1.0 / k_of(i, j);
                }
                sp_y += SUB as f64 / r;
            }
            sp_y /= SUB as f64;
            // Columns in series, each a parallel set.
            let (mut ps_x, mut ps_y) = (0.0, 0.0);
            for i in 0..SUB {
                let mut s = 0.0;
                for j in 0..SUB {
                    s += k_of(i, j);
                }
                ps_x += 1.0 / (s / SUB as f64);
            }
            ps_x = SUB as f64 / ps_x;
            for j in 0..SUB {
                let mut s = 0.0;
                for i in 0..SUB {
                    s += k_of(i, j);
                }
                ps_y += 1.0 / (s / SUB as f64);
            }
            ps_y = SUB as f64 / ps_y;
            let f = n_cu as f64 / (SUB * SUB) as f64;
            k3[c] = [
                (sp_x * ps_x).sqrt() as f32,
                (sp_y * ps_y).sqrt() as f32,
                (f * K_CU + (1.0 - f) * kz) as f32,
            ];
            total += n_cu;
            count += SUB * SUB;
        }
        cu_share.push(total as f64 / count.max(1) as f64);
        kxyz.push(k3);
    }
    // Barrels: copper area per cell between the layers each via spans.
    let mut barrel: Vec<Vec<f64>> = vec![Vec::new(); ns];
    let cell_of = |p: [f64; 2]| -> Option<usize> {
        let i = ((p[0] - origin[0]) / cell).floor();
        let j = ((p[1] - origin[1]) / cell).floor();
        (i >= 0.0 && j >= 0.0 && (i as usize) < cols && (j as usize) < rows)
            .then(|| j as usize * cols + i as usize)
    };
    // The copper layer of each slab, and the span of slabs per copper layer.
    let slab_of_cu: Vec<usize> = (0..nl)
        .map(|l| slabs.iter().position(|s| s.copper == Some(l)).unwrap_or(0))
        .collect();
    let mut n_vias = 0;
    for v in &g.vias {
        let Some(c) = cell_of([v.at.x.mm(), v.at.y.mm()]) else {
            continue;
        };
        let (a, b) = (
            (v.from_layer.min(v.to_layer) as usize).min(nl - 1),
            (v.from_layer.max(v.to_layer) as usize).min(nl - 1),
        );
        if a == b {
            continue;
        }
        let r = v.hole.mm() / 2.0;
        let tp = setup.plating_mm.min(r);
        let area = std::f64::consts::PI * (r * r - (r - tp) * (r - tp)) * MM * MM;
        for s in slab_of_cu[a]..=slab_of_cu[b] {
            if barrel[s].is_empty() {
                barrel[s] = vec![0.0; per];
            }
            barrel[s][c] += area;
        }
        n_vias += 1;
    }
    // Through-hole pads' plated holes too.
    for p in &g.pads {
        if p.hole.mm() <= 0.0 || p.layer != LayerRef::AllCopper {
            continue;
        }
        let Some(c) = cell_of([p.at.x.mm(), p.at.y.mm()]) else {
            continue;
        };
        let r = p.hole.mm() / 2.0;
        let tp = setup.plating_mm.min(r);
        let area = std::f64::consts::PI * (r * r - (r - tp) * (r - tp)) * MM * MM;
        for s in 0..ns {
            if barrel[s].is_empty() {
                barrel[s] = vec![0.0; per];
            }
            barrel[s][c] += area;
        }
    }
    // Conductivity of every slab cell: [kx, ky, kz].
    let a_cell = (cell * MM) * (cell * MM);
    let k_at = |s: usize, c: usize| -> [f64; 3] {
        let sl = &slabs[s];
        let mut k = match sl.copper {
            Some(l) => {
                let v = kxyz[l][c];
                [v[0] as f64, v[1] as f64, v[2] as f64]
            }
            None => [sl.k_in, sl.k_in, sl.k_z],
        };
        if let Some(b) = barrel[s].get(c) {
            let f = (b / a_cell).min(1.0);
            k[2] = k[2] * (1.0 - f) + K_CU * f;
        }
        k
    };
    // Nodes: board cells in every slab, then each source's junction and
    // case, then the heatsink base cells.
    let mut id = vec![u32::MAX; per];
    let mut cells: Vec<usize> = Vec::new();
    for c in 0..per {
        if on_board[c] {
            id[c] = cells.len() as u32;
            cells.push(c);
        }
    }
    let nb = cells.len();
    if nb == 0 {
        return Err("no grid cell lies inside the board outline".into());
    }
    let node = |s: usize, c: usize| s * nb + id[c] as usize;
    let mut n_nodes = ns * nb;
    // Fixed links: (a, b, G).
    let mut links: Vec<(usize, usize, f64)> = Vec::new();
    let mut kcache: Vec<Vec<[f64; 3]>> = Vec::with_capacity(ns);
    for s in 0..ns {
        kcache.push(cells.iter().map(|&c| k_at(s, c)).collect());
    }
    let kc = |s: usize, c: usize| kcache[s][id[c] as usize];
    for s in 0..ns {
        let t = slabs[s].t;
        for &c in &cells {
            let (i, j) = (c % cols, c / cols);
            let k = kc(s, c);
            if i + 1 < cols && on_board[c + 1] {
                let k2 = kc(s, c + 1);
                let kf = 2.0 * k[0] * k2[0] / (k[0] + k2[0]);
                links.push((node(s, c), node(s, c + 1), kf * t));
            }
            if j + 1 < rows && on_board[c + cols] {
                let k2 = kc(s, c + cols);
                let kf = 2.0 * k[1] * k2[1] / (k[1] + k2[1]);
                links.push((node(s, c), node(s, c + cols), kf * t));
            }
            if s + 1 < ns {
                let k2 = kc(s + 1, c);
                let r = slabs[s].t / (2.0 * k[2]) + slabs[s + 1].t / (2.0 * k2[2]);
                links.push((node(s, c), node(s + 1, c), a_cell / r));
            }
        }
    }
    // Sources: pads and body of each.
    let bodies = crate::body::part_bodies(board);
    struct Src {
        j: usize,
        c: usize,
        /// Board nodes the junction reaches, with conductance shares.
        under: Vec<(usize, f64)>,
        /// Cells under the body on its side (no convection there).
        covered: Vec<usize>,
        bottom: bool,
        /// Case top area (m²) and its size (mm) for convection.
        top_area: f64,
        size_mm: f64,
        sink: Option<(Heatsink, usize, usize, usize)>, // heatsink, first node, nx, ny
    }
    let mut srcs: Vec<Src> = Vec::new();
    let mut total_w = 0.0;
    for s in &setup.sources {
        if s.part >= board.components.len() {
            return Err(format!("no part {}", s.part));
        }
        let comp = &board.components[s.part];
        let body = bodies.iter().find(|b| b.component == s.part);
        let bottom = body.map_or(comp.side == crate::Side::Bottom, |b| b.bottom);
        let layer = if bottom { nl - 1 } else { 0 };
        let surf = if bottom { ns - 1 } else { 0 };
        let cu_slab = slab_of_cu[layer];
        // Pads on the mounting layer: their cells, weighted by copper.
        let mut w: std::collections::HashMap<usize, f64> = std::collections::HashMap::new();
        for p in g.pads.iter().filter(|p| p.component == Some(s.part as u32)) {
            let r = p.size_x.mm().max(p.size_y.mm()) / 2.0 + cell;
            let (px, py) = (p.at.x.mm(), p.at.y.mm());
            let n = ((2.0 * r / (cell / 3.0)).ceil() as usize).max(1);
            for a in 0..n {
                for b in 0..n {
                    let q = [
                        px - r + (a as f64 + 0.5) * 2.0 * r / n as f64,
                        py - r + (b as f64 + 0.5) * 2.0 * r / n as f64,
                    ];
                    if on_pad(p, layer as u8, nl, q) {
                        if let Some(c) = cell_of(q).filter(|&c| on_board[c]) {
                            *w.entry(c).or_insert(0.0) += 1.0;
                        }
                    }
                }
            }
        }
        let corners: Vec<[f64; 2]> = body.map_or(Vec::new(), |b| b.corners.to_vec());
        let mut covered = Vec::new();
        if corners.len() >= 3 {
            for &c in &cells {
                if in_ring(&corners, centre(c)) {
                    covered.push(c);
                }
            }
        }
        if w.is_empty() {
            for &c in &covered {
                w.insert(c, 1.0);
            }
        }
        if w.is_empty() {
            let at = [comp.x.mm(), comp.y.mm()];
            match cell_of(at).filter(|&c| on_board[c]) {
                Some(c) => {
                    w.insert(c, 1.0);
                }
                None => return Err(format!("{} is off the board", comp.designator)),
            }
        }
        let sum: f64 = w.values().sum();
        let mut under: Vec<(usize, f64)> = w
            .into_iter()
            .map(|(c, v)| (node(cu_slab, c), v / sum))
            .collect();
        under.sort_by_key(|u| u.0);
        let _ = surf;
        // The case top: the body's area, else the pads' extent.
        let (top_area, size_mm) = if corners.len() >= 3 {
            let mut a = 0.0;
            for k in 0..corners.len() {
                let (p, q) = (corners[k], corners[(k + 1) % corners.len()]);
                a += p[0] * q[1] - q[0] * p[1];
            }
            let a = a.abs() / 2.0;
            (a * MM * MM, a.sqrt())
        } else {
            let a = (sum * (cell / 3.0).powi(2)).max(1.0);
            (a * MM * MM, a.sqrt())
        };
        let j = n_nodes;
        let c = n_nodes + 1;
        n_nodes += 2;
        links.push((j, c, 1.0 / s.theta_jc.max(1e-3)));
        let g_jb = 1.0 / s.theta_jb.max(1e-3);
        for &(n, f) in &under {
            links.push((j, n, g_jb * f));
        }
        let sink = s.heatsink.as_ref().map(|h| {
            let nx = ((h.length / 1.0).ceil() as usize).clamp(6, 40);
            let ny = ((h.width / 1.0).ceil() as usize).clamp(6, 40);
            let first = n_nodes;
            n_nodes += nx * ny;
            // Base conduction.
            let (dx, dy) = (h.length / nx as f64 * MM, h.width / ny as f64 * MM);
            let tb = h.base * MM;
            for a in 0..nx {
                for b in 0..ny {
                    let n = first + b * nx + a;
                    if a + 1 < nx {
                        links.push((n, n + 1, h.k * tb * dy / dx));
                    }
                    if b + 1 < ny {
                        links.push((n, n + nx, h.k * tb * dx / dy));
                    }
                }
            }
            // The interface over the package top, centred.
            let (pl, pw) = (size_mm.min(h.length), size_mm.min(h.width));
            let mut hit = Vec::new();
            for a in 0..nx {
                for b in 0..ny {
                    let x = (a as f64 + 0.5) / nx as f64 * h.length - h.length / 2.0;
                    let y = (b as f64 + 0.5) / ny as f64 * h.width - h.width / 2.0;
                    if x.abs() <= pl / 2.0 && y.abs() <= pw / 2.0 {
                        hit.push(first + b * nx + a);
                    }
                }
            }
            if hit.is_empty() {
                hit.push(first + (ny / 2) * nx + nx / 2);
            }
            let contact = (pl * pw).min(top_area / (MM * MM)) * MM * MM;
            let r_area = h.tim_mm * MM / h.tim_k + h.contact * 1e-4;
            let g_tim = contact / r_area;
            for &n in &hit {
                links.push((c, n, g_tim / hit.len() as f64));
            }
            // Through the base thickness, the fins see the base's far face:
            // half the base in series with each cell's fins (added below).
            (h.clone(), first, nx, ny)
        });
        total_w += s.watts;
        srcs.push(Src {
            j,
            c,
            under,
            covered,
            bottom,
            top_area,
            size_mm,
            sink,
        });
    }
    // Board faces that convect: top and bottom cells not under a source's
    // body on that side; edges.
    let mut no_top = vec![false; per];
    let mut no_bot = vec![false; per];
    for s in &srcs {
        for &c in &s.covered {
            if s.bottom {
                no_bot[c] = true;
            } else {
                no_top[c] = true;
            }
        }
    }
    let edge_cells: Vec<(usize, usize)> = cells
        .iter()
        .map(|&c| {
            let (i, j) = (c % cols, c / cols);
            let open = [
                i == 0 || !on_board[c - 1],
                i + 1 >= cols || !on_board[c + 1],
                j == 0 || !on_board[c - cols],
                j + 1 >= rows || !on_board[c + cols],
            ]
            .iter()
            .filter(|&&b| b)
            .count();
            (c, open)
        })
        .filter(|e| e.1 > 0)
        .collect();
    let board_area: f64 = nb as f64 * a_cell;
    let perimeter: f64 = edge_cells.iter().map(|e| e.1 as f64 * cell).sum::<f64>();
    let l_flat = (board_area / (MM * MM)) / perimeter.max(1.0);
    let l_long = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let l_x = hi[0] - lo[0];
    let thick: f64 = slabs.iter().map(|s| s.t).sum();
    let ta = setup.ambient_c;
    // Iterate: temperatures (rise over ambient), then h from them.
    let mut t = vec![10.0f64; n_nodes];
    let mut iterations = 0;
    let mut h_top = (0.0, 0.0);
    let mut h_bot = (0.0, 0.0);
    let mut diag = vec![0.0f64; n_nodes];
    let mut rhs = vec![0.0f64; n_nodes];
    let mut amb: Vec<(usize, f64)> = Vec::new();
    let board_l = match setup.air {
        Air::Forced { .. } => l_x,
        Air::Natural { vertical: true } => l_long,
        Air::Natural { vertical: false } => l_flat,
    };
    let mut kinds: Vec<u8> = Vec::new(); // 0 board, 1 case, 2 heatsink
    let mut out = (0.0, 0.0, 0.0);
    let mut balance = 0.0;
    for it in 0..60 {
        iterations = it + 1;
        // Mean face temperatures for convection.
        let mean = |s: usize| -> f64 {
            let v: f64 = cells.iter().map(|&c| t[node(s, c)]).sum();
            ta + v / nb as f64
        };
        let (mt, mb) = (mean(0), mean(ns - 1));
        let (ht, hb) = match setup.fixed_h {
            Some(h) => (h, h),
            None => {
                let vert = matches!(setup.air, Air::Natural { vertical: true });
                let ft = if vert { 2 } else { 0 };
                let fb = if vert { 2 } else { 1 };
                (
                    h_plate(setup.air, ft, board_l, mt, ta),
                    h_plate(setup.air, fb, board_l, mb, ta),
                )
            }
        };
        let e = if setup.fixed_h.is_some() {
            0.0
        } else {
            setup.emissivity
        };
        h_top = (ht, ht + h_rad(e, mt, ta));
        h_bot = (hb, hb + h_rad(e, mb, ta));
        amb.clear();
        kinds.clear();
        // Board faces: half a slab in series with the film.
        for &c in &cells {
            for (s, hc, blocked) in [(0usize, ht, no_top[c]), (ns - 1, hb, no_bot[c])] {
                if blocked {
                    continue;
                }
                let n = node(s, c);
                let h = hc + h_rad(e, ta + t[n], ta);
                let kz = kc(s, c)[2];
                let r = slabs[s].t / (2.0 * kz) + 1.0 / h;
                amb.push((n, a_cell / r));
                kinds.push(0);
            }
        }
        // Edges: each slab's open sides, with the side film (vertical).
        if setup.fixed_h.is_none() {
            let he = h_plate(setup.air, 2, thick / MM * 10.0, (mt + mb) / 2.0, ta);
            for &(c, open) in &edge_cells {
                for s in 0..ns {
                    let n = node(s, c);
                    let h = he + h_rad(e, ta + t[n], ta);
                    amb.push((n, h * slabs[s].t * cell * MM * open as f64));
                    kinds.push(0);
                }
            }
        }
        // Cases and heatsinks.
        for s in &srcs {
            let tc = ta + t[s.c];
            match &s.sink {
                None => {
                    let face = if s.bottom { 1 } else { 0 };
                    let h = h_plate(setup.air, face, s.size_mm / 4.0, tc, ta)
                        + h_rad(setup.emissivity, tc, ta);
                    amb.push((s.c, h * s.top_area));
                    kinds.push(1);
                }
                Some((hs, first, nx, ny)) => {
                    let n_cells = (nx * ny) as f64;
                    let mean_t = ta + (0..nx * ny).map(|k| t[first + k]).sum::<f64>() / n_cells;
                    let sp = hs.spacing();
                    // Air rises along the fin height when the board lies
                    // flat, along the fin length when it stands.
                    let (chan, flow_l) = match setup.air {
                        Air::Natural { vertical: false } => (hs.fin_height, hs.fin_height),
                        _ => (hs.length, hs.length),
                    };
                    let _ = chan;
                    let hch = h_channel(setup.air, sp, flow_l, hs.fin, mean_t, ta);
                    let eta = fin_efficiency(hch, hs.k, hs.fin, hs.fin_height);
                    // Per fin, per metre along it: both faces and the tip.
                    let per_fin = hch * eta * (2.0 * hs.fin_height + hs.fin) * MM;
                    let fins_per_m = hs.fins as f64 / (hs.width * MM);
                    let open_share = (1.0 - hs.fins as f64 * hs.fin / hs.width).max(0.0);
                    // Radiation from the outer envelope only.
                    let env = (hs.length * hs.width
                        + 2.0 * (hs.length + hs.width) * (hs.base + hs.fin_height))
                        * MM
                        * MM;
                    let hr = h_rad(hs.emissivity, mean_t, ta) * env;
                    let per_area = per_fin * fins_per_m + hch * open_share;
                    let a = hs.length * hs.width * MM * MM / n_cells;
                    // Half the base thickness to where the fins start.
                    let r_base = hs.base * MM / 2.0 / hs.k;
                    for k in 0..nx * ny {
                        let g_air = per_area * a + hr / n_cells;
                        let g = 1.0 / (1.0 / g_air + r_base / a);
                        amb.push((first + k, g));
                        kinds.push(2);
                    }
                }
            }
        }
        // Assemble and solve: sum G (Ti - Tj) + G_amb Ti = Q.
        diag.iter_mut().for_each(|d| *d = 0.0);
        rhs.iter_mut().for_each(|r| *r = 0.0);
        for &(a, b, gg) in &links {
            diag[a] += gg;
            diag[b] += gg;
        }
        for &(n, gg) in &amb {
            diag[n] += gg;
        }
        for (k, s) in setup.sources.iter().enumerate() {
            rhs[srcs[k].j] += s.watts;
        }
        let next = pcg(&diag, &links, &rhs, t.clone(), n_nodes);
        crate::cancel::check()?;
        let change = next
            .iter()
            .zip(&t)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        t = next;
        // Heat balance.
        out = (0.0, 0.0, 0.0);
        for (k, &(n, gg)) in amb.iter().enumerate() {
            let q = gg * t[n];
            match kinds[k] {
                0 => out.0 += q,
                1 => out.1 += q,
                _ => out.2 += q,
            }
        }
        balance = 100.0 * ((out.0 + out.1 + out.2) - total_w) / total_w;
        if change < 0.005 && it > 1 {
            break;
        }
    }
    // Results.
    let mut parts = Vec::new();
    for (k, s) in setup.sources.iter().enumerate() {
        let sr = &srcs[k];
        let tj = ta + t[sr.j];
        let tc = ta + t[sr.c];
        let tb = ta + sr.under.iter().map(|&(n, f)| t[n] * f).sum::<f64>();
        let to_board = (tj - tb) / s.theta_jb.max(1e-3) / s.watts.max(1e-12);
        let hs = sr.sink.as_ref().map(|(_, first, nx, ny)| {
            ta + (0..nx * ny).map(|q| t[first + q]).fold(f64::MIN, f64::max)
        });
        parts.push(PartTemp {
            part: s.part,
            designator: board.components[s.part].designator.clone(),
            watts: s.watts,
            junction_c: tj,
            case_c: tc,
            board_c: tb,
            heatsink_c: hs,
            to_board: to_board.clamp(0.0, 1.0),
            theta_ja: (tj - ta) / s.watts.max(1e-12),
        });
    }
    let mut maps = Vec::new();
    let mut board_max = (f64::MIN, [0.0, 0.0], 0);
    for l in 0..nl {
        let s = slab_of_cu[l];
        let mut m = vec![f32::NAN; per];
        for &c in &cells {
            let v = ta + t[node(s, c)];
            m[c] = v as f32;
            if v > board_max.0 {
                board_max = (v, centre(c), l);
            }
        }
        maps.push(LayerMap {
            name: names[l].clone(),
            t: m,
        });
    }
    notes.push(format!(
        "{} cells of {:.2} mm, {} slabs through {:.2} mm; copper cover {}; {} via barrels",
        nb,
        cell,
        ns,
        thick / MM,
        cu_share
            .iter()
            .zip(&names)
            .map(|(f, n)| format!("{n} {:.0}%", f * 100.0))
            .collect::<Vec<_>>()
            .join(", "),
        n_vias
    ));
    if iterations >= 60 {
        notes.push("the convection did not settle in 60 passes".into());
    }
    for s in &srcs {
        if let Some((hs, ..)) = &s.sink {
            let sp = hs.spacing();
            if sp < 2.0 && !matches!(setup.air, Air::Forced { .. }) {
                notes.push(format!(
                    "heatsink fins {sp:.1} mm apart choke still air (about 4 to 8 mm suits natural convection)"
                ));
            }
        }
    }
    if matches!(setup.air, Air::Forced { .. }) && srcs.iter().any(|s| s.sink.is_some()) {
        notes.push(
            "forced air: all the flow is taken to pass through the fins (ducted); air that goes round them lowers the gain"
                .into(),
        );
    }
    // Every slab's temperatures and conductivities, the heat the parts put
    // into the board and the heat its faces give the air: the density maps.
    let field = {
        let mut z = 0.0;
        let slab_info: Vec<crate::thermfield::SlabInfo> = slabs
            .iter()
            .map(|sl| {
                let i = crate::thermfield::SlabInfo {
                    name: sl.name.clone(),
                    z_mm: z,
                    t_mm: sl.t / MM,
                    copper: sl.copper,
                };
                z += sl.t / MM;
                i
            })
            .collect();
        let mut temp = vec![vec![f32::NAN; per]; ns];
        let mut k = vec![vec![[0f32; 3]; per]; ns];
        for s in 0..ns {
            for &c in &cells {
                temp[s][c] = (ta + t[node(s, c)]) as f32;
                let v = kc(s, c);
                k[s][c] = [v[0] as f32, v[1] as f32, v[2] as f32];
            }
        }
        // Heat into the board under each part (junction to board).
        let mut source = vec![vec![0f32; per]; ns];
        for (q, s) in setup.sources.iter().enumerate() {
            let sr = &srcs[q];
            let g_jb = 1.0 / s.theta_jb.max(1e-3);
            for &(n, f) in &sr.under {
                let (sl, c) = (n / nb, cells[n % nb]);
                source[sl][c] += (g_jb * f * (t[sr.j] - t[n])).max(0.0) as f32;
            }
        }
        // Heat to air from each face cell (convection and radiation).
        let mut air_top = vec![f32::NAN; per];
        let mut air_bot = vec![f32::NAN; per];
        let e = if setup.fixed_h.is_some() {
            0.0
        } else {
            setup.emissivity
        };
        for &c in &cells {
            for (s, hc, blocked, out) in [
                (0usize, h_top.0, no_top[c], &mut air_top),
                (ns - 1, h_bot.0, no_bot[c], &mut air_bot),
            ] {
                out[c] = 0.0;
                if blocked {
                    continue;
                }
                let n = node(s, c);
                let h = hc + h_rad(e, ta + t[n], ta);
                let r = slabs[s].t / (2.0 * kc(s, c)[2]) + 1.0 / h;
                out[c] = (t[n] / r) as f32;
            }
        }
        let bodies: Vec<(String, Vec<[f64; 2]>)> = setup
            .sources
            .iter()
            .map(|s| {
                let comp = &board.components[s.part];
                let corners = bodies
                    .iter()
                    .find(|b| b.component == s.part)
                    .map_or(Vec::new(), |b| b.corners.to_vec());
                (comp.designator.clone(), corners)
            })
            .collect();
        crate::thermfield::ThermField {
            slabs: slab_info,
            origin,
            cell,
            cols,
            rows,
            ambient_c: ta,
            temp,
            k,
            source,
            air_top,
            air_bot,
            outline: outline.clone(),
            parts: bodies,
        }
    };
    Ok(Result {
        parts,
        maps,
        origin,
        cell,
        cols,
        rows,
        ambient_c: ta,
        board_max,
        h_top,
        h_bottom: h_bot,
        out_board: out.0,
        out_cases: out.1,
        out_sinks: out.2,
        balance_pct: balance,
        iterations,
        nodes: n_nodes,
        notes,
        field,
    })
}

/// Conjugate gradients with a Jacobi preconditioner on the conductance
/// matrix (diagonal `diag`, off-diagonal -G for each link).
fn pcg(
    diag: &[f64],
    links: &[(usize, usize, f64)],
    rhs: &[f64],
    mut x: Vec<f64>,
    n: usize,
) -> Vec<f64> {
    let mul = |v: &[f64], out: &mut Vec<f64>| {
        for i in 0..n {
            out[i] = diag[i] * v[i];
        }
        for &(a, b, g) in links {
            out[a] -= g * v[b];
            out[b] -= g * v[a];
        }
    };
    let mut ax = vec![0.0; n];
    mul(&x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|i| rhs[i] - ax[i]).collect();
    let inv: Vec<f64> = diag
        .iter()
        .map(|d| if *d > 0.0 { 1.0 / d } else { 0.0 })
        .collect();
    let mut z: Vec<f64> = (0..n).map(|i| r[i] * inv[i]).collect();
    let mut p = z.clone();
    let mut rz: f64 = (0..n).map(|i| r[i] * z[i]).sum();
    let norm_b = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-30);
    let mut ap = vec![0.0; n];
    for k in 0..20_000 {
        if k % 50 == 0 && crate::cancel::cancelled() {
            break;
        }
        let rn = r.iter().map(|v| v * v).sum::<f64>().sqrt();
        if rn < 1e-10 * norm_b {
            break;
        }
        mul(&p, &mut ap);
        let pap: f64 = (0..n).map(|i| p[i] * ap[i]).sum();
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        for i in 0..n {
            z[i] = r[i] * inv[i];
        }
        let rz2: f64 = (0..n).map(|i| r[i] * z[i]).sum();
        let beta = rz2 / rz;
        rz = rz2;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
    }
    x
}

/// A grid check: the same solve on a grid 1.5 times finer, each part's
/// junction temperature compared, and an error bar for it (Roache's grid
/// convergence index with the method's second order, safety factor 3).
#[derive(Clone, Debug)]
pub struct MeshCheck {
    pub coarse: Result,
    pub fine: Result,
    /// Per part: junction °C on the fine grid and its error bar, K.
    pub error: Vec<(String, f64, f64)>,
}

pub fn check_mesh(board: &Board, setup: &Setup) -> std::result::Result<MeshCheck, String> {
    let coarse = solve(board, setup)?;
    let mut s = setup.clone();
    s.cell_mm = coarse.cell / 1.5;
    let fine = solve(board, &s)?;
    let r: f64 = 1.5;
    let error = coarse
        .parts
        .iter()
        .zip(&fine.parts)
        .map(|(a, b)| {
            let e = 3.0 * (b.junction_c - a.junction_c).abs() / (r * r - 1.0);
            (b.designator.clone(), b.junction_c, e)
        })
        .collect();
    Ok(MeshCheck {
        coarse,
        fine,
        error,
    })
}

/// Colour for a share 0 (ambient) to 1 (hottest): blue, cyan, green,
/// yellow, red.
pub fn heat_color(f: f64) -> [u8; 3] {
    let f = f.clamp(0.0, 1.0);
    let stops: [[f64; 3]; 5] = [
        [40.0, 60.0, 200.0],
        [30.0, 190.0, 220.0],
        [60.0, 200.0, 80.0],
        [240.0, 220.0, 50.0],
        [220.0, 40.0, 30.0],
    ];
    let x = f * 4.0;
    let i = (x.floor() as usize).min(3);
    let u = x - i as f64;
    let c = |k: usize| (stops[i][k] + (stops[i + 1][k] - stops[i][k]) * u) as u8;
    [c(0), c(1), c(2)]
}

impl Result {
    /// The temperature maps of the copper layers as one SVG, side by side.
    pub fn svg(&self, title: &str) -> String {
        let n = self.maps.len().max(1);
        let w_mm = self.cols as f64 * self.cell;
        let h_mm = self.rows as f64 * self.cell;
        let scale = (360.0 / w_mm.max(h_mm)).min(8.0);
        let (pw, ph) = (w_mm * scale, h_mm * scale);
        let per_row = n.min(3);
        let rows_n = n.div_ceil(per_row);
        let width = per_row as f64 * (pw + 20.0) + 20.0;
        let height = rows_n as f64 * (ph + 40.0) + 90.0;
        let tmax = self.board_max.0;
        // The colours run from the coolest board point to the hottest.
        let ta = self
            .maps
            .iter()
            .flat_map(|m| m.t.iter())
            .filter(|v| !v.is_nan())
            .fold(f64::MAX, |a, &v| a.min(v as f64))
            .min(tmax);
        let mut s = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" font-family=\"sans-serif\" font-size=\"12\">\n<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/>\n<text x=\"20\" y=\"22\" font-size=\"15\">{}</text>\n",
            xml(title)
        );
        for (k, m) in self.maps.iter().enumerate() {
            let ox = 20.0 + (k % per_row) as f64 * (pw + 20.0);
            let oy = 50.0 + (k / per_row) as f64 * (ph + 40.0);
            s += &format!(
                "<text x=\"{ox:.1}\" y=\"{:.1}\">{}</text>\n",
                oy - 6.0,
                xml(&m.name)
            );
            // The map as an embedded palette PNG, one pixel per cell.
            let mut idx = vec![0u8; self.cols * self.rows];
            for j in 0..self.rows {
                for i in 0..self.cols {
                    let v = m.t[(self.rows - 1 - j) * self.cols + i];
                    idx[j * self.cols + i] = if v.is_nan() {
                        0
                    } else {
                        1 + (((v as f64 - ta) / (tmax - ta).max(1e-9)).clamp(0.0, 1.0) * 62.0)
                            .round() as u8
                    };
                }
            }
            let png = palette_png(self.cols, self.rows, &idx);
            s += &format!(
                "<image x=\"{ox:.1}\" y=\"{oy:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" preserveAspectRatio=\"none\" style=\"image-rendering:pixelated\" href=\"data:image/png;base64,{}\"/>\n",
                base64(&png)
            );
        }
        // Scale bar.
        let y = height - 30.0;
        for k in 0..50 {
            let c = heat_color(k as f64 / 49.0);
            s += &format!(
                "<rect x=\"{:.1}\" y=\"{y:.1}\" width=\"4.2\" height=\"12\" fill=\"rgb({},{},{})\"/>\n",
                20.0 + k as f64 * 4.0,
                c[0],
                c[1],
                c[2]
            );
        }
        s += &format!(
            "<text x=\"20\" y=\"{:.1}\">{ta:.1} °C</text><text x=\"200\" y=\"{:.1}\" text-anchor=\"end\">{tmax:.1} °C</text>\n",
            y - 4.0,
            y - 4.0
        );
        let mut x = 240.0;
        for p in &self.parts {
            s += &format!(
                "<text x=\"{x:.0}\" y=\"{:.1}\">{}: Tj {:.1} °C</text>\n",
                y + 10.0,
                xml(&p.designator),
                p.junction_c
            );
            x += 130.0;
        }
        s += "</svg>\n";
        s
    }

    /// Every board cell's temperature on every copper layer, as CSV.
    pub fn csv(&self) -> String {
        let mut s = String::from("x_mm,y_mm");
        for m in &self.maps {
            s += &format!(",{}", m.name.replace(',', " "));
        }
        s.push('\n');
        for c in 0..self.cols * self.rows {
            if self.maps.first().is_none_or(|m| m.t[c].is_nan()) {
                continue;
            }
            let x = self.origin[0] + ((c % self.cols) as f64 + 0.5) * self.cell;
            let y = self.origin[1] + ((c / self.cols) as f64 + 0.5) * self.cell;
            s += &format!("{x:.3},{y:.3}");
            for m in &self.maps {
                s += &format!(",{:.2}", m.t[c]);
            }
            s.push('\n');
        }
        s
    }

    /// A plain-text report.
    pub fn report(&self) -> String {
        let mut s = String::new();
        s += &format!(
            "{:<10} {:>7} {:>9} {:>9} {:>9} {:>9} {:>8} {:>9}\n",
            "Part", "W", "Tj °C", "Tcase °C", "Tboard °C", "Tsink °C", "board %", "θJA K/W"
        );
        for p in &self.parts {
            s += &format!(
                "{:<10} {:>7.2} {:>9.1} {:>9.1} {:>9.1} {:>9} {:>8.0} {:>9.1}\n",
                p.designator,
                p.watts,
                p.junction_c,
                p.case_c,
                p.board_c,
                p.heatsink_c.map_or("-".into(), |t| format!("{t:.1}")),
                p.to_board * 100.0,
                p.theta_ja
            );
        }
        s += &format!(
            "Hottest board point {:.1} °C on {} at ({:.1}, {:.1}) mm; ambient {:.1} °C\n",
            self.board_max.0,
            self.maps
                .get(self.board_max.2)
                .map_or("?", |m| m.name.as_str()),
            self.board_max.1[0],
            self.board_max.1[1],
            self.ambient_c
        );
        s += &format!(
            "Air film: top {:.1} W/m²K ({:.1} with radiation), bottom {:.1} ({:.1})\n",
            self.h_top.0, self.h_top.1, self.h_bottom.0, self.h_bottom.1
        );
        s += &format!(
            "Heat out: board {:.2} W, cases {:.2} W, heatsinks {:.2} W; balance {:+.3} % in {} passes, {} nodes\n",
            self.out_board, self.out_cases, self.out_sinks, self.balance_pct, self.iterations, self.nodes
        );
        for n in &self.notes {
            s += &format!("Note: {n}\n");
        }
        s
    }
}

/// An 8-bit palette PNG (index 0 transparent, 1 to 63 the heat colours),
/// stored without compression.
fn palette_png(w: usize, h: usize, idx: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let chunk = |kind: &[u8], data: &[u8], out: &mut Vec<u8>| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend(crc(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 3, 0, 0, 0]);
    chunk(b"IHDR", &ihdr, &mut out);
    let mut plte = vec![255, 255, 255];
    let mut trns = vec![0u8];
    for k in 0..63 {
        plte.extend(heat_color(k as f64 / 62.0));
        trns.push(255);
    }
    chunk(b"PLTE", &plte, &mut out);
    chunk(b"tRNS", &trns, &mut out);
    // Raw rows (filter 0) in stored deflate blocks.
    let mut raw = Vec::with_capacity((w + 1) * h);
    for j in 0..h {
        raw.push(0);
        raw.extend_from_slice(&idx[j * w..(j + 1) * w]);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (k, b) in blocks.iter().enumerate() {
        z.push(if k + 1 == blocks.len() { 1 } else { 0 });
        let n = b.len() as u16;
        z.extend(n.to_le_bytes());
        z.extend((!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    if blocks.is_empty() {
        z.extend([1, 0, 0, 0xff, 0xff]);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    chunk(b"IDAT", &z, &mut out);
    chunk(b"IEND", &[], &mut out);
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Parses "U1=2.5W", "U1=2.5" or "U1=2.5W@jc0.5/jb8" style power entries.
pub fn parse_power(board: &Board, s: &str) -> std::result::Result<Source, String> {
    let (name, rest) = s
        .split_once('=')
        .ok_or(format!("--power: not PART=W: {s}"))?;
    let part = board
        .components
        .iter()
        .position(|c| c.designator.eq_ignore_ascii_case(name.trim()))
        .ok_or(format!("--power: no part {name}"))?;
    let (w, th) = rest.split_once('@').unwrap_or((rest, ""));
    let watts: f64 = w
        .trim()
        .trim_end_matches(['W', 'w'])
        .parse()
        .map_err(|_| format!("--power: not watts: {w}"))?;
    let mut src = Source {
        part,
        watts,
        theta_jc: f64::NAN,
        theta_jb: f64::NAN,
        heatsink: None,
    };
    for t in th.split(['/', ';']).filter(|t| !t.is_empty()) {
        let t = t.trim().to_ascii_lowercase();
        let num = |x: &str| -> std::result::Result<f64, String> {
            x.trim_start_matches('=')
                .parse::<f64>()
                .map_err(|_| format!("--power: not K/W: {x}"))
        };
        if let Some(v) = t.strip_prefix("jc") {
            src.theta_jc = num(v)?;
        } else if let Some(v) = t.strip_prefix("jb") {
            src.theta_jb = num(v)?;
        } else {
            return Err(format!("--power: unknown {t} (jcN or jbN)"));
        }
    }
    let (jc, jb) = default_thetas(board, part);
    if src.theta_jc.is_nan() {
        src.theta_jc = jc;
    }
    if src.theta_jb.is_nan() {
        src.theta_jb = jb;
    }
    Ok(src)
}

/// θJC and θJB guessed from a part's size and pads when the datasheet's
/// are not given: typical JEDEC values for packages of that body area,
/// lower with an exposed pad.
pub fn default_thetas(board: &Board, part: usize) -> (f64, f64) {
    let bodies = crate::body::part_bodies(board);
    let area = bodies
        .iter()
        .find(|b| b.component == part)
        .map_or(25.0, |b| {
            let c = &b.corners;
            let mut a = 0.0;
            for k in 0..4 {
                a += c[k][0] * c[(k + 1) % 4][1] - c[(k + 1) % 4][0] * c[k][1];
            }
            (a.abs() / 2.0).max(1.0)
        });
    // An exposed pad: one pad much larger than the others.
    let pads: Vec<f64> = board
        .geometry
        .pads
        .iter()
        .filter(|p| p.component == Some(part as u32))
        .map(|p| p.size_x.mm() * p.size_y.mm())
        .collect();
    let biggest = pads.iter().cloned().fold(0.0, f64::max);
    // The next largest pad (an equal one counts: five equal SOT-23 leads
    // are not an exposed pad).
    let others = {
        let mut v = pads.clone();
        v.sort_by(|a, b| b.total_cmp(a));
        v.get(1).copied().unwrap_or(0.0)
    };
    // One pad much larger than the rest: a QFN/DFN exposed pad, or the
    // tab of an SOT-223, DPAK or D2PAK (3 or 4 pads).
    let big_one = pads.len() >= 3 && biggest > 4.0 * others.max(1e-6) && biggest > 0.1 * area;
    // An exposed pad drawn as several pads (paste windows, split pads,
    // thermal vias as pads): copper near the body's centre, where leads
    // never are, covering a good part of the body.
    let central: f64 = {
        let comp = &board.components[part];
        let (cx, cy) = (comp.x.mm(), comp.y.mm());
        let reach = 0.3 * area.sqrt();
        board
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(part as u32))
            .filter(|p| (p.at.x.mm() - cx).hypot(p.at.y.mm() - cy) < reach)
            .map(|p| p.size_x.mm() * p.size_y.mm())
            .sum()
    };
    let epad = big_one || (pads.len() > 4 && central > 0.12 * area);
    // A ball grid: many round surface pads of one size.
    let round = board
        .geometry
        .pads
        .iter()
        .filter(|p| p.component == Some(part as u32))
        .filter(|p| {
            // Round, not oblong (an oval QFP lead is Round too).
            let (w, h) = (p.size_x.mm(), p.size_y.mm());
            p.hole.mm() <= 0.0
                && p.shape == crate::PadShape::Round
                && w.max(h) < 1.25 * w.min(h).max(1e-6)
        })
        .count();
    let smallest = pads.iter().cloned().fold(f64::MAX, f64::min);
    let bga = pads.len() >= 16 && round * 10 >= pads.len() * 9 && biggest < 2.0 * smallest;
    let kind = if bga {
        Package::Bga
    } else if epad {
        Package::ExposedPad
    } else {
        Package::Leaded
    };
    let _ = MIL_MM;
    package_thetas(kind, area.sqrt())
}

/// How a package's die reaches its case and the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Package {
    /// Ball grid (flip chip above about 10 mm): a thin lid or bare die on
    /// top, hundreds of balls into the board's planes.
    Bga,
    /// QFN, DFN, PowerPAD, SOT-223 and DPAK tabs: the die sits on a pad
    /// soldered to the board; the top is mould compound.
    ExposedPad,
    /// SOIC, TSSOP, SOT and the rest: heat leaves through the leads.
    Leaded,
}

/// Typical JEDEC (2s2p) θJC (to the case top) and θJB, K/W, for a body
/// `size` mm across, fitted to datasheet values: a 14 mm flip-chip BGA
/// about 1 and 4, a 5 mm QFN about 30 and 8, an SOIC-8 about 55 and 50, an SOT-23
/// about 80 and 50.
pub fn package_thetas(kind: Package, size: f64) -> (f64, f64) {
    let size = size.max(0.5);
    match kind {
        Package::Bga => (
            (14.0 / size).clamp(0.15, 6.0),
            (55.0 / size).clamp(1.5, 20.0),
        ),
        Package::ExposedPad => (
            (150.0 / size).clamp(5.0, 40.0),
            (40.0 / size).clamp(1.5, 20.0),
        ),
        Package::Leaded => (
            (250.0 / size).clamp(10.0, 80.0),
            (220.0 / size).clamp(10.0, 50.0),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_properties_and_fins() {
        let (k, nu, _, pr) = air(300.0);
        assert!((k - 0.0263).abs() < 1e-6 && (nu - 15.89e-6).abs() < 1e-9 && pr > 0.7);
        // A short stubby fin is nearly fully effective, a long thin one not.
        assert!(fin_efficiency(10.0, 200.0, 2.0, 5.0) > 0.99);
        assert!(fin_efficiency(100.0, 200.0, 0.5, 40.0) < 0.6);
        // Still air off a 100 mm plate, 30 K warm: a few W/m²K.
        let h = h_plate(Air::Natural { vertical: false }, 0, 25.0, 55.0, 25.0);
        assert!(h > 4.0 && h < 12.0, "{h}");
        let h = h_plate(Air::Forced { speed: 2.0 }, 0, 100.0, 55.0, 25.0);
        assert!(h > 10.0 && h < 30.0, "{h}");
    }

    #[test]
    fn package_guesses() {
        let (jc, jb) = package_thetas(Package::Bga, 14.0);
        assert!(
            (0.7..1.3).contains(&jc) && (3.0..5.0).contains(&jb),
            "{jc} {jb}"
        );
        let (jc, jb) = package_thetas(Package::ExposedPad, 5.0);
        assert!(
            (20.0..40.0).contains(&jc) && (5.0..12.0).contains(&jb),
            "{jc} {jb}"
        );
        // A 3 mm LFCSP LDO and an SOT-223 tab: θJB about 13 and 8.
        let (_, jb) = package_thetas(Package::ExposedPad, 3.0);
        assert!((8.0..16.0).contains(&jb), "{jb}");
        let (jc, jb) = package_thetas(Package::Leaded, 4.5);
        assert!(
            (40.0..70.0).contains(&jc) && (40.0..90.0).contains(&jb),
            "{jc} {jb}"
        );
    }

    #[test]
    fn heatsink_text() {
        let h = Heatsink::parse("40x40x20,fins=10,fin=1.2,base=4").unwrap();
        assert_eq!(h.fins, 10);
        assert!((h.fin_height - 16.0).abs() < 1e-9);
        assert!((h.spacing() - (40.0 - 12.0) / 9.0).abs() < 1e-9);
        assert!(Heatsink::parse("40x40").is_err());
    }
}
