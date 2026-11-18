//! Thermal density maps: what the 3D thermal solve found, on cut planes
//! through the board, and the pictures made of them (a colour map with a
//! scale bar, as a still SVG page or an animated GIF that sweeps the cut
//! through the board).
//!
//! The thermal solve keeps the temperature and the conductivity (kx, ky,
//! kz) of every cell of every slab of the stack-up, the heat each part
//! puts into the board under its pads, and the heat each face cell gives
//! the air. From those come:
//! - the temperature T (°C);
//! - the temperature gradient |∇T| (K/mm), central differences across
//!   the grid and between the slabs;
//! - the heat flux density |q| = |k ∇T| (W/cm²), each component with the
//!   conductivity the solver used across that face (in plane: the cell's
//!   own kx, ky; through the board: the two half slabs in series, as the
//!   solver links them), so the flux is the heat really crossing;
//! - the power density (W/cm³): the heat the parts put into the board,
//!   per volume of the copper cells that take it;
//! - the heat to air (W/cm²): convection plus radiation from each cell of
//!   the outer faces, the heat leaving the board.
//!
//! Planes: each slab seen from above (each copper layer, and each
//! dielectric slab mid-way between them), and side views: a vertical cut
//! along x and one along y, through the hottest point by default, with the
//! board's thickness drawn taller so the layers show.

use crate::emfield::{base64, gif, png, Raster};
use std::fmt::Write;

/// One slab of the stack-up, top first.
#[derive(Clone, Debug)]
pub struct SlabInfo {
    pub name: String,
    /// Depth of its top below the board's top, and its thickness, mm.
    pub z_mm: f64,
    pub t_mm: f64,
    /// Copper layer index, or None for dielectric and mask.
    pub copper: Option<usize>,
}

/// The thermal solve's fields, per slab and grid cell (row by row from the
/// lower left, as the solve's `LayerMap`).
#[derive(Clone, Debug, Default)]
pub struct ThermField {
    pub slabs: Vec<SlabInfo>,
    pub origin: [f64; 2],
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub ambient_c: f64,
    /// °C, NaN off the board.
    pub temp: Vec<Vec<f32>>,
    /// W/m·K, [kx, ky, kz].
    pub k: Vec<Vec<[f32; 3]>>,
    /// W into each cell from the parts.
    pub source: Vec<Vec<f32>>,
    /// W/m² to the air from the top and bottom faces (0 under a part's
    /// body, NaN off the board).
    pub air_top: Vec<f32>,
    pub air_bot: Vec<f32>,
    /// Board outline (mm) and the powered parts' bodies (designator,
    /// corners in mm).
    pub outline: Vec<[f64; 2]>,
    pub parts: Vec<(String, Vec<[f64; 2]>)>,
}

/// What to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TQuantity {
    Temperature,
    Gradient,
    Flux,
    Power,
    Air,
}

impl TQuantity {
    pub const ALL: [TQuantity; 5] = [
        TQuantity::Temperature,
        TQuantity::Flux,
        TQuantity::Gradient,
        TQuantity::Power,
        TQuantity::Air,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TQuantity::Temperature => "temperature",
            TQuantity::Gradient => "temperature gradient |∇T|",
            TQuantity::Flux => "heat flux density |q|",
            TQuantity::Power => "power density (heat put in)",
            TQuantity::Air => "heat to air",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            TQuantity::Temperature => "T",
            TQuantity::Gradient => "gradient",
            TQuantity::Flux => "flux",
            TQuantity::Power => "power",
            TQuantity::Air => "air",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            TQuantity::Temperature => "°C",
            TQuantity::Gradient => "K/mm",
            TQuantity::Flux | TQuantity::Air => "W/cm²",
            TQuantity::Power => "W/cm³",
        }
    }

    /// Drawn on a log scale (decades below the top) rather than linear.
    pub fn log(self) -> bool {
        self != TQuantity::Temperature
    }

    pub fn parse(s: &str) -> Option<TQuantity> {
        match s.trim().to_ascii_lowercase().as_str() {
            "t" | "temp" | "temperature" => Some(TQuantity::Temperature),
            "g" | "grad" | "gradient" => Some(TQuantity::Gradient),
            "q" | "flux" | "heat" | "heatflux" => Some(TQuantity::Flux),
            "p" | "power" | "source" | "sources" => Some(TQuantity::Power),
            "air" | "loss" | "out" => Some(TQuantity::Air),
            _ => None,
        }
    }
}

/// Which way a plane is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TCut {
    /// One slab, seen from above.
    Slab(usize),
    /// Vertical, along x, at `y` mm.
    Xz { y: f64 },
    /// Vertical, along y, at `x` mm.
    Yz { x: f64 },
}

/// A plane's values on a grid of rectangles: `u_edges` across (mm, left
/// to right), `v_edges` down (mm: y falling for a slab seen from above,
/// depth rising for a side view); values row by row from the top.
#[derive(Clone, Debug)]
pub struct TPlane {
    pub name: String,
    pub cut: TCut,
    pub u_edges: Vec<f64>,
    pub v_edges: Vec<f64>,
    pub vals: Vec<f64>,
    /// Copper slab of each row (side views: the copper lines drawn).
    pub copper_rows: Vec<bool>,
}

impl TPlane {
    fn nu(&self) -> usize {
        self.u_edges.len() - 1
    }
    fn nv(&self) -> usize {
        self.v_edges.len() - 1
    }
    /// The largest and smallest finite (and for log scales, positive)
    /// value.
    pub fn span(&self, log: bool) -> Option<(f64, f64)> {
        let mut r: Option<(f64, f64)> = None;
        for &v in &self.vals {
            if v.is_finite() && (!log || v > 0.0) {
                r = Some(r.map_or((v, v), |(a, b)| (a.min(v), b.max(v))));
            }
        }
        r
    }
}

/// How values map to colours.
#[derive(Clone, Copy, Debug)]
pub struct TScale {
    pub q: TQuantity,
    pub lo: f64,
    pub hi: f64,
}

impl TScale {
    /// 0 to 1 along the colour bar; None where there is nothing to draw.
    pub fn t(&self, v: f64) -> Option<f64> {
        if !v.is_finite() {
            return None;
        }
        if self.q.log() {
            if v <= 0.0 {
                return Some(0.0);
            }
            let d = (self.hi / self.lo).log10().max(1e-9);
            Some(((v / self.lo).log10() / d).clamp(0.0, 1.0))
        } else {
            Some(((v - self.lo) / (self.hi - self.lo).max(1e-9)).clamp(0.0, 1.0))
        }
    }

    pub fn colour(&self, v: f64) -> Option<[u8; 3]> {
        self.t(v).map(|t| {
            if self.q == TQuantity::Temperature {
                crate::thermal::heat_color(t)
            } else {
                crate::emfield::colour(t)
            }
        })
    }

    fn colour_at(&self, t: f64) -> [u8; 3] {
        if self.q == TQuantity::Temperature {
            crate::thermal::heat_color(t)
        } else {
            crate::emfield::colour(t)
        }
    }
}

/// Off the board.
const BACK: [u8; 3] = [236, 236, 240];

impl ThermField {
    fn per(&self) -> usize {
        self.cols * self.rows
    }

    pub fn thickness(&self) -> f64 {
        self.slabs.last().map_or(0.0, |s| s.z_mm + s.t_mm)
    }

    fn centre(&self, c: usize) -> [f64; 2] {
        [
            self.origin[0] + ((c % self.cols) as f64 + 0.5) * self.cell,
            self.origin[1] + ((c / self.cols) as f64 + 0.5) * self.cell,
        ]
    }

    /// The cell holding a point, mm.
    pub fn cell_of(&self, p: [f64; 2]) -> Option<usize> {
        let i = ((p[0] - self.origin[0]) / self.cell).floor();
        let j = ((p[1] - self.origin[1]) / self.cell).floor();
        (i >= 0.0 && j >= 0.0 && (i as usize) < self.cols && (j as usize) < self.rows)
            .then(|| j as usize * self.cols + i as usize)
    }

    /// The hottest cell: slab, cell, its centre (mm), °C.
    pub fn hottest(&self) -> (usize, usize, [f64; 2], f64) {
        let mut best = (0, 0, f64::MIN);
        for (s, t) in self.temp.iter().enumerate() {
            for (c, &v) in t.iter().enumerate() {
                if v.is_finite() && v as f64 > best.2 {
                    best = (s, c, v as f64);
                }
            }
        }
        (best.0, best.1, self.centre(best.1), best.2)
    }

    /// The vertical heat flux density from slab `s` into slab `s + 1` at
    /// cell `c`, W/m² (positive downwards), as the solver links them.
    fn qz_below(&self, s: usize, c: usize) -> f64 {
        if s + 1 >= self.slabs.len() {
            return 0.0;
        }
        let (a, b) = (self.temp[s][c] as f64, self.temp[s + 1][c] as f64);
        if !a.is_finite() || !b.is_finite() {
            return 0.0;
        }
        let (ka, kb) = (self.k[s][c][2] as f64, self.k[s + 1][c][2] as f64);
        let r = self.slabs[s].t_mm * 1e-3 / (2.0 * ka.max(1e-9))
            + self.slabs[s + 1].t_mm * 1e-3 / (2.0 * kb.max(1e-9));
        (a - b) / r
    }

    /// dT/dx and dT/dy at a cell of a slab, K/m (one sided at the edge).
    fn d_inplane(&self, s: usize, c: usize) -> [f64; 2] {
        let t = &self.temp[s];
        let (i, j) = (c % self.cols, c / self.cols);
        let h = self.cell * 1e-3;
        let get = |ii: usize, jj: usize| {
            let v = t[jj * self.cols + ii] as f64;
            v.is_finite().then_some(v)
        };
        let here = t[c] as f64;
        let d = |m: Option<f64>, p: Option<f64>| match (m, p) {
            (Some(m), Some(p)) => (p - m) / (2.0 * h),
            (Some(m), None) => (here - m) / h,
            (None, Some(p)) => (p - here) / h,
            (None, None) => 0.0,
        };
        let xm = (i > 0).then(|| get(i - 1, j)).flatten();
        let xp = (i + 1 < self.cols).then(|| get(i + 1, j)).flatten();
        let ym = (j > 0).then(|| get(i, j - 1)).flatten();
        let yp = (j + 1 < self.rows).then(|| get(i, j + 1)).flatten();
        [d(xm, xp), d(ym, yp)]
    }

    /// The quantity at a cell of a slab (NaN off the board, or where it
    /// has no meaning: heat to air inside the board).
    pub fn value(&self, q: TQuantity, s: usize, c: usize) -> f64 {
        let t = self.temp[s][c] as f64;
        if !t.is_finite() {
            return f64::NAN;
        }
        match q {
            TQuantity::Temperature => t,
            TQuantity::Gradient | TQuantity::Flux => {
                let [dx, dy] = self.d_inplane(s, c);
                // Through the board: the flux above and below, averaged
                // (one side at the faces).
                let above = if s > 0 { self.qz_below(s - 1, c) } else { 0.0 };
                let below = self.qz_below(s, c);
                let n = (s > 0) as u8 as f64 + (s + 1 < self.slabs.len()) as u8 as f64;
                let qz = (above + below) / n.max(1.0);
                let k = self.k[s][c];
                if q == TQuantity::Flux {
                    let (qx, qy) = (k[0] as f64 * dx, k[1] as f64 * dy);
                    (qx * qx + qy * qy + qz * qz).sqrt() * 1e-4
                } else {
                    let dz = qz / (k[2] as f64).max(1e-9);
                    (dx * dx + dy * dy + dz * dz).sqrt() * 1e-3
                }
            }
            TQuantity::Power => {
                let v = self.cell * self.cell * self.slabs[s].t_mm * 1e-3; // cm³
                self.source[s][c] as f64 / v
            }
            TQuantity::Air => {
                let top_cu = self.slabs.iter().position(|x| x.copper.is_some());
                let bot_cu = self.slabs.iter().rposition(|x| x.copper.is_some());
                let top = top_cu.is_none_or(|k| s <= k);
                let bot = bot_cu.is_none_or(|k| s >= k);
                let a = if top && !bot {
                    self.air_top[c]
                } else if bot && !top {
                    self.air_bot[c]
                } else if top && bot {
                    self.air_top[c] + self.air_bot[c]
                } else {
                    return f64::NAN;
                };
                a as f64 * 1e-4
            }
        }
    }

    /// The planes a full set of maps shows: every copper layer, the
    /// dielectric mid-way between each pair of them (with `all`: every
    /// dielectric slab), top first; then the two side views through `at`
    /// (mm; None: the hottest point).
    pub fn planes(&self, all: bool, at: Option<[f64; 2]>) -> Vec<TCut> {
        let mid = self.mid_slabs();
        let mut v: Vec<TCut> = (0..self.slabs.len())
            .filter(|&s| all || self.slabs[s].copper.is_some() || mid.contains(&s))
            .map(TCut::Slab)
            .collect();
        let p = at.unwrap_or_else(|| self.hottest().2);
        v.push(TCut::Xz { y: p[1] });
        v.push(TCut::Yz { x: p[0] });
        v
    }

    /// The dielectric slab mid-way between each pair of copper layers.
    pub fn mid_slabs(&self) -> Vec<usize> {
        let cu: Vec<usize> = (0..self.slabs.len())
            .filter(|&s| self.slabs[s].copper.is_some())
            .collect();
        cu.windows(2)
            .filter(|w| w[1] > w[0] + 1)
            .map(|w| (w[0] + w[1]) / 2)
            .map(|s| {
                if self.slabs[s].copper.is_some() {
                    s + 1
                } else {
                    s
                }
            })
            .collect()
    }

    /// One plane of one quantity.
    pub fn plane(&self, cut: TCut, q: TQuantity) -> TPlane {
        let (cols, rows) = (self.cols, self.rows);
        let x_edges: Vec<f64> = (0..=cols)
            .map(|i| self.origin[0] + i as f64 * self.cell)
            .collect();
        match cut {
            TCut::Slab(s) => {
                let s = s.min(self.slabs.len().saturating_sub(1));
                let v_edges: Vec<f64> = (0..=rows)
                    .map(|j| self.origin[1] + (rows - j) as f64 * self.cell)
                    .collect();
                let mut vals = Vec::with_capacity(self.per());
                for j in (0..rows).rev() {
                    for i in 0..cols {
                        vals.push(self.value(q, s, j * cols + i));
                    }
                }
                let sl = &self.slabs[s];
                let name = match sl.copper {
                    Some(_) => format!("{} (copper, {:.3} mm deep)", sl.name, sl.z_mm),
                    None => format!("{} ({:.3} mm deep)", sl.name, sl.z_mm + sl.t_mm / 2.0),
                };
                TPlane {
                    name,
                    cut,
                    u_edges: x_edges,
                    v_edges,
                    vals,
                    copper_rows: vec![sl.copper.is_some(); rows],
                }
            }
            TCut::Xz { .. } | TCut::Yz { .. } => {
                let along_x = matches!(cut, TCut::Xz { .. });
                let (n_u, u_edges) = if along_x {
                    (cols, x_edges)
                } else {
                    (
                        rows,
                        (0..=rows)
                            .map(|j| self.origin[1] + j as f64 * self.cell)
                            .collect(),
                    )
                };
                let fixed = match cut {
                    TCut::Xz { y } => ((y - self.origin[1]) / self.cell)
                        .floor()
                        .clamp(0.0, rows as f64 - 1.0)
                        as usize,
                    TCut::Yz { x } => ((x - self.origin[0]) / self.cell)
                        .floor()
                        .clamp(0.0, cols as f64 - 1.0)
                        as usize,
                    _ => 0,
                };
                let mut v_edges = vec![0.0];
                for sl in &self.slabs {
                    v_edges.push(sl.z_mm + sl.t_mm);
                }
                let mut vals = Vec::with_capacity(n_u * self.slabs.len());
                for s in 0..self.slabs.len() {
                    for u in 0..n_u {
                        let c = if along_x {
                            fixed * cols + u
                        } else {
                            u * cols + fixed
                        };
                        let v = if q == TQuantity::Air {
                            // Shown on the outer slabs only.
                            if s == 0 {
                                self.air_top[c] as f64 * 1e-4
                            } else if s + 1 == self.slabs.len() {
                                self.air_bot[c] as f64 * 1e-4
                            } else {
                                f64::NAN
                            }
                        } else {
                            self.value(q, s, c)
                        };
                        vals.push(v);
                    }
                }
                let at = if along_x {
                    self.origin[1] + (fixed as f64 + 0.5) * self.cell
                } else {
                    self.origin[0] + (fixed as f64 + 0.5) * self.cell
                };
                TPlane {
                    name: if along_x {
                        format!("side view along x at y = {at:.1} mm")
                    } else {
                        format!("side view along y at x = {at:.1} mm")
                    },
                    cut: if along_x {
                        TCut::Xz { y: at }
                    } else {
                        TCut::Yz { x: at }
                    },
                    u_edges,
                    v_edges,
                    vals,
                    copper_rows: self.slabs.iter().map(|s| s.copper.is_some()).collect(),
                }
            }
        }
    }

    /// A colour scale shared by `cuts` for one quantity: temperature from
    /// the coolest to the hottest point shown; the rest `decades` deep
    /// below the largest value.
    pub fn scale(&self, q: TQuantity, cuts: &[TCut], decades: f64) -> Option<TScale> {
        let mut span: Option<(f64, f64)> = None;
        for &c in cuts {
            if let Some((a, b)) = self.plane(c, q).span(q.log()) {
                span = Some(span.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
            }
        }
        let (lo, hi) = span?;
        if q.log() {
            if hi <= 0.0 {
                return None;
            }
            Some(TScale {
                q,
                lo: hi / 10f64.powf(decades.max(0.5)),
                hi,
            })
        } else {
            Some(TScale {
                q,
                lo,
                hi: hi.max(lo + 0.01),
            })
        }
    }

    /// One pixel per grid cell of a slab seen from above, top row first,
    /// RGBA with transparency off the board (the board overlay).
    pub fn rgba(&self, s: usize, q: TQuantity, sc: &TScale) -> Vec<[u8; 4]> {
        let p = self.plane(TCut::Slab(s), q);
        p.vals
            .iter()
            .map(|&v| match sc.colour(v) {
                Some(c) if !(q.log() && v <= 0.0) => [c[0], c[1], c[2], 255],
                _ => [0, 0, 0, 0],
            })
            .collect()
    }
}

/// Pixels per mm across and down a picture.
#[derive(Clone, Copy, Debug)]
pub struct TFrame {
    pub px_u: f64,
    pub px_v: f64,
    pub w: usize,
    pub h: usize,
}

impl TPlane {
    /// About `width` pixels across; side views stretched to `min_h` high.
    pub fn frame(&self, width: usize, min_h: usize) -> TFrame {
        let su = (self.u_edges[self.nu()] - self.u_edges[0]).abs().max(1e-6);
        let sv = (self.v_edges[self.nv()] - self.v_edges[0]).abs().max(1e-6);
        let px = width as f64 / su;
        let mut pv = px;
        if !matches!(self.cut, TCut::Slab(_)) && sv * pv < min_h as f64 {
            pv = min_h as f64 / sv;
        }
        TFrame {
            px_u: px,
            px_v: pv,
            w: width,
            h: ((sv * pv).round() as usize).max(8),
        }
    }

    /// The plane in colour, with the board outline and the powered parts
    /// drawn over a view from above, and the copper layers over a side
    /// view. Temperature is interpolated between cell centres; the
    /// densities are drawn per cell, as solved.
    pub fn draw(&self, f: &ThermField, sc: &TScale, fr: TFrame) -> Raster {
        let (nu, nv) = (self.nu(), self.nv());
        let du = self.u_edges[1] - self.u_edges[0];
        let flip = self.v_edges[nv] < self.v_edges[0];
        let v_of = |y: usize| {
            let d = (y as f64 + 0.5) / fr.px_v;
            if flip {
                self.v_edges[0] - d
            } else {
                self.v_edges[0] + d
            }
        };
        let row_of = |v: f64| -> usize {
            // Edges are monotonic; find the band holding v.
            let k = if flip {
                self.v_edges.partition_point(|&e| e > v)
            } else {
                self.v_edges.partition_point(|&e| e <= v)
            };
            k.saturating_sub(1).min(nv - 1)
        };
        let smooth = sc.q == TQuantity::Temperature && matches!(self.cut, TCut::Slab(_));
        let mut r = Raster::new(fr.w, fr.h, BACK);
        for y in 0..fr.h {
            let v = v_of(y);
            let j = row_of(v);
            for x in 0..fr.w {
                let u = self.u_edges[0] + (x as f64 + 0.5) / fr.px_u;
                let fu = (u - self.u_edges[0]) / du;
                let i = (fu.floor() as usize).min(nu - 1);
                let mut val = self.vals[j * nu + i];
                if smooth && val.is_finite() {
                    // Bilinear between the four nearest centres (rows run
                    // at the cell size too in a view from above).
                    let fv = ((self.v_edges[0] - v) / du - 0.5).max(0.0);
                    let fu2 = (fu - 0.5).max(0.0);
                    let (i0, j0) = (
                        (fu2.floor() as usize).min(nu - 1),
                        (fv.floor() as usize).min(nv - 1),
                    );
                    let (i1, j1) = ((i0 + 1).min(nu - 1), (j0 + 1).min(nv - 1));
                    let (a, b) = (fu2 - i0 as f64, fv - j0 as f64);
                    let g = |ii: usize, jj: usize| self.vals[jj * nu + ii];
                    let q = [g(i0, j0), g(i1, j0), g(i0, j1), g(i1, j1)];
                    if q.iter().all(|v| v.is_finite()) {
                        val = q[0] * (1.0 - a) * (1.0 - b)
                            + q[1] * a * (1.0 - b)
                            + q[2] * (1.0 - a) * b
                            + q[3] * a * b;
                    }
                }
                let c = if val.is_nan() {
                    BACK
                } else {
                    sc.colour(val).unwrap_or(BACK)
                };
                r.put(x as i64, y as i64, c);
            }
        }
        let to_px = |p: [f64; 2]| {
            [
                (p[0] - self.u_edges[0]) * fr.px_u,
                if flip {
                    (self.v_edges[0] - p[1]) * fr.px_v
                } else {
                    (p[1] - self.v_edges[0]) * fr.px_v
                },
            ]
        };
        match self.cut {
            TCut::Slab(_) => {
                let ring = |r: &mut Raster, pts: &[[f64; 2]], c: [u8; 3]| {
                    for k in 0..pts.len() {
                        r.line(to_px(pts[k]), to_px(pts[(k + 1) % pts.len()]), c);
                    }
                };
                if f.outline.len() >= 3 {
                    ring(&mut r, &f.outline, [60, 60, 70]);
                }
                for (name, corners) in &f.parts {
                    if corners.len() >= 3 {
                        ring(&mut r, corners, [255, 255, 255]);
                        let p = to_px(corners[0]);
                        r.text([p[0] + 2.0, p[1] - 3.0], 7.0, name, [255, 255, 255]);
                    }
                }
            }
            _ => {
                // Copper slabs: thin lines at their faces.
                for (s, &cu) in self.copper_rows.iter().enumerate() {
                    if cu {
                        for v in [self.v_edges[s], self.v_edges[s + 1]] {
                            let y = to_px([0.0, v])[1];
                            for x in (0..fr.w).step_by(3) {
                                r.put(x as i64, y.round() as i64, [200, 120, 40]);
                            }
                        }
                    }
                }
            }
        }
        r
    }
}

/// A number with 3 figures.
fn num(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let a = v.abs();
    if !(1e-3..1e5).contains(&a) {
        format!("{v:.2e}")
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

/// A vertical scale bar `h` px high with its ticks.
pub fn scale_bar(sc: &TScale, h: usize) -> Raster {
    let w = 150;
    let mut r = Raster::new(w, h, [255, 255, 255]);
    let (y0, y1) = (26usize, h.saturating_sub(10).max(40));
    for y in y0..y1 {
        let t = 1.0 - (y - y0) as f64 / (y1 - y0) as f64;
        r.fill(4, y, 22, y + 1, sc.colour_at(t));
    }
    let ticks: Vec<(f64, f64)> = if sc.q.log() {
        let d = (sc.hi / sc.lo).log10();
        let n = d.ceil() as usize;
        (0..=n)
            .map(|k| (k as f64 / d.max(1e-9), sc.hi / 10f64.powi(k as i32)))
            .filter(|t| t.0 <= 1.0 + 1e-9)
            .collect()
    } else {
        (0..=5)
            .map(|k| {
                let f = k as f64 / 5.0;
                (f, sc.hi - f * (sc.hi - sc.lo))
            })
            .collect()
    };
    for (f, v) in ticks {
        let y = y0 as f64 + f * (y1 - y0) as f64;
        r.line([22.0, y], [27.0, y], [0, 0, 0]);
        r.text([30.0, y + 4.0], 8.0, &num(v), [0, 0, 0]);
    }
    r.text([4.0, 10.0], 8.0, sc.q.unit(), [0, 0, 0]);
    r
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// How a GIF moves the cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sweep {
    /// Down through the slabs, top view.
    Depth,
    /// A side view along x, stepping across y.
    Y,
    /// A side view along y, stepping across x.
    X,
}

impl Sweep {
    pub fn parse(s: &str) -> Option<Sweep> {
        match s.trim().to_ascii_lowercase().as_str() {
            "depth" | "z" | "layers" => Some(Sweep::Depth),
            "y" => Some(Sweep::Y),
            "x" => Some(Sweep::X),
            _ => None,
        }
    }
}

impl ThermField {
    /// A still page: for each quantity, the planes `cuts` on one shared
    /// scale (densities `decades` deep).
    pub fn svg(
        &self,
        title: &str,
        quantities: &[TQuantity],
        cuts: &[TCut],
        decades: f64,
    ) -> String {
        let width = 380usize;
        let per_row = 3;
        let mut body = String::new();
        let mut y = 74.0;
        let page_w = per_row as f64 * (width as f64 + 30.0) + 190.0;
        for &q in quantities {
            // Only planes with something to show for this quantity.
            let shown: Vec<(TCut, TPlane)> = cuts
                .iter()
                .map(|&c| (c, self.plane(c, q)))
                .filter(|(_, p)| p.span(q.log()).is_some_and(|(_, hi)| !q.log() || hi > 0.0))
                .collect();
            let Some(sc) = self.scale(q, &shown.iter().map(|s| s.0).collect::<Vec<_>>(), decades)
            else {
                continue;
            };
            let _ = writeln!(
                body,
                r#"<text x="20" y="{:.0}" font-size="16" font-weight="bold">{} ({}){}</text>"#,
                y + 4.0,
                q.label(),
                q.unit(),
                if q.log() {
                    format!(", log scale {decades:.0} decades")
                } else {
                    String::new()
                }
            );
            y += 18.0;
            let y_sec = y;
            let mut col = 0;
            let mut row_h: f64 = 0.0;
            for (_, p) in &shown {
                let fr = p.frame(width, 120);
                let img = p.draw(self, &sc, fr);
                let x = 20.0 + col as f64 * (width as f64 + 30.0);
                let (lo, hi) = p.span(q.log()).unwrap_or((0.0, 0.0));
                let _ = writeln!(
                    body,
                    r#"<text x="{x:.0}" y="{:.0}" font-size="11">{}</text>"#,
                    y + 12.0,
                    esc(&p.name)
                );
                let _ = writeln!(
                    body,
                    r#"<image x="{x:.0}" y="{:.0}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                    y + 16.0,
                    img.w,
                    img.h,
                    base64(&png(&img))
                );
                let _ = writeln!(
                    body,
                    r#"<text x="{x:.0}" y="{:.0}" font-size="10" fill="gray">{} to {} {}{}</text>"#,
                    y + 16.0 + img.h as f64 + 12.0,
                    num(lo),
                    num(hi),
                    q.unit(),
                    if fr.px_v > fr.px_u * 1.01 {
                        format!(", thickness drawn {:.0}× taller", fr.px_v / fr.px_u)
                    } else {
                        String::new()
                    }
                );
                row_h = row_h.max(img.h as f64 + 36.0);
                col += 1;
                if col == per_row {
                    col = 0;
                    y += row_h;
                    row_h = 0.0;
                }
            }
            if col != 0 {
                y += row_h;
            }
            let bar = scale_bar(&sc, ((y - y_sec).max(160.0) as usize).min(400));
            let bx = 20.0 + per_row as f64 * (width as f64 + 30.0);
            let _ = writeln!(
                body,
                r#"<image x="{bx:.0}" y="{:.0}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                y_sec + 16.0,
                bar.w,
                bar.h,
                base64(&png(&bar))
            );
            y += 20.0;
        }
        let hot = self.hottest();
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_w:.0}" height="{:.0}" font-family="sans-serif">
<rect width="100%" height="100%" fill="white"/>
<text x="20" y="28" font-size="18" font-weight="bold">{}</text>
<text x="20" y="48" font-size="11" fill="gray">Steady state, ambient {:.1} °C; hottest {:.1} °C in {} at ({:.1}, {:.1}) mm. Heat flux k∇T with the conductivity the solver used; power density = heat the parts put into the board; heat to air = convection + radiation.</text>
{body}</svg>
"#,
            y + 10.0,
            esc(title),
            self.ambient_c,
            hot.3,
            esc(&self
                .slabs
                .get(hot.0)
                .map_or(String::new(), |s| s.name.clone())),
            hot.2[0],
            hot.2[1]
        )
    }

    /// An animated GIF: the cut swept through the board (down the slabs,
    /// or a side view stepping across), on one shared scale.
    pub fn gif(&self, q: TQuantity, sweep: Sweep, decades: f64, frames: usize) -> Vec<u8> {
        let cuts: Vec<TCut> = match sweep {
            Sweep::Depth => (0..self.slabs.len()).map(TCut::Slab).collect(),
            Sweep::Y | Sweep::X => {
                let n = frames.clamp(2, 120);
                let (a, len) = if sweep == Sweep::Y {
                    (self.origin[1], self.rows as f64 * self.cell)
                } else {
                    (self.origin[0], self.cols as f64 * self.cell)
                };
                (0..n)
                    .map(|k| {
                        let p = a + (k as f64 + 0.5) / n as f64 * len;
                        if sweep == Sweep::Y {
                            TCut::Xz { y: p }
                        } else {
                            TCut::Yz { x: p }
                        }
                    })
                    .collect()
            }
        };
        let Some(sc) = self.scale(q, &cuts, decades) else {
            return gif(&[Raster::new(8, 8, BACK)], 10);
        };
        let first = self.plane(cuts[0], q);
        let fr = first.frame(640, 220);
        let bar = scale_bar(&sc, fr.h.clamp(160, 480));
        let (w, h) = (fr.w + bar.w + 10, fr.h.max(bar.h) + 30);
        let pics: Vec<Raster> = cuts
            .iter()
            .map(|&c| {
                let p = self.plane(c, q);
                let img = p.draw(self, &sc, fr);
                let mut page = Raster::new(w, h, [255, 255, 255]);
                page.text(
                    [6.0, 18.0],
                    11.0,
                    &format!("{}  {}", q.short(), p.name),
                    [0, 0, 0],
                );
                page.blit(&img, 0, 28);
                page.blit(&bar, fr.w + 10, 28);
                page
            })
            .collect();
        gif(&pics, if sweep == Sweep::Depth { 25 } else { 10 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1D bar: heat flows down a uniform slab stack; the flux through
    /// every slab is the same and equals the power over the area.
    #[test]
    fn uniform_flux_through_slabs() {
        let (cols, rows, ns) = (3, 3, 4);
        let per = cols * rows;
        let k = 0.5f32;
        let t_mm = 0.5;
        // q = 1e4 W/m² (1 W/cm²): ΔT per slab centre to centre = q t / k.
        let q = 1e4;
        let dt = q * t_mm * 1e-3 / k as f64;
        let f = ThermField {
            slabs: (0..ns)
                .map(|s| SlabInfo {
                    name: format!("s{s}"),
                    z_mm: s as f64 * t_mm,
                    t_mm,
                    copper: if s == 0 || s == ns - 1 {
                        Some(s.min(1))
                    } else {
                        None
                    },
                })
                .collect(),
            origin: [0.0, 0.0],
            cell: 1.0,
            cols,
            rows,
            ambient_c: 25.0,
            temp: (0..ns)
                .map(|s| vec![(100.0 - dt * s as f64) as f32; per])
                .collect(),
            k: vec![vec![[k, k, k]; per]; ns],
            source: vec![vec![0.0; per]; ns],
            air_top: vec![0.0; per],
            air_bot: vec![1.0e4; per],
            outline: Vec::new(),
            parts: Vec::new(),
        };
        for s in 0..ns {
            let v = f.value(TQuantity::Flux, s, 4);
            assert!((v - 1.0).abs() < 1e-3, "slab {s}: {v} W/cm²");
            let g = f.value(TQuantity::Gradient, s, 4);
            assert!((g - dt / t_mm).abs() < 1e-2, "slab {s}: {g} K/mm");
        }
        assert!((f.value(TQuantity::Air, ns - 1, 4) - 1.0).abs() < 1e-9);
        assert!(f.value(TQuantity::Air, 1, 4).is_nan());
        let cuts = f.planes(true, None);
        assert_eq!(cuts.len(), ns + 2);
        let svg = f.svg("t", &TQuantity::ALL, &cuts, 3.0);
        assert!(svg.contains("heat flux density"));
        let g = f.gif(TQuantity::Temperature, Sweep::Depth, 3.0, 0);
        assert_eq!(&g[..6], b"GIF89a");
    }
}
