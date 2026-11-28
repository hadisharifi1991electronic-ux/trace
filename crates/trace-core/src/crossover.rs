//! An SMT RF/DC crossover in the X2AS class (5.08 mm square, 1.19 mm
//! thick, DC-6 GHz, 50 ohm), on a 50 ohm microstrip test board, as a 3D FEM
//! model, and its grade against that class's limits.
//!
//! The construction is Trace's own, from the public outline: the RF path
//! comes up the castellation in the middle of each side, crosses the body
//! as a 50 ohm stripline between an internal ground plane and the top
//! ground, and goes down the far castellation. The internal ground plane
//! (cleared round the RF castellations) shields the stripline from the DC
//! line, which runs on the board's top copper under the part, between its
//! ground corners. The corner castellations tie both of the part's ground
//! planes to the board's ground pads, which have vias to the board's ground
//! (the bottom of the box). Four wave ports: RF1, RF2 (x faces), DC1, DC2
//! (y faces), so RF-to-DC isolation comes with the RF path.

use crate::fem3d::{Copper, Fill, Mode, Model, Port, Shape, Sheet, Solid, Wall};

const MM: f64 = 1e-3;

/// The crossover and its test board, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossover {
    /// Body: square side and thickness.
    pub body: f64,
    pub thick: f64,
    /// Body dielectric.
    pub er: f64,
    pub tand: f64,
    /// The internal ground plane's height above the body's bottom.
    pub z_gnd: f64,
    /// The stripline's height above the body's bottom.
    pub z_strip: f64,
    /// Stripline width, and over `taper` at each end, the width there
    /// (tunes the castellation's capacitance out).
    pub w_strip: f64,
    pub w_end: f64,
    pub taper: f64,
    /// Castellation radius (RF and ground).
    pub r_cast: f64,
    /// Clearance radius round the RF castellations in the internal ground.
    pub clear: f64,
    /// Test board: laminate height, er, loss tangent; feed width and the
    /// length of feed each side of the part; DC line width.
    pub pcb_h: f64,
    pub pcb_er: f64,
    pub pcb_tand: f64,
    pub w_feed: f64,
    pub feed: f64,
    pub w_dc: f64,
    /// Board past the part (y), air over the part.
    pub margin: f64,
    pub air: f64,
    /// Copper loss (RMS roughness, µm); None: perfect conductor.
    pub rough_um: Option<f64>,
    /// Drive the DC line's ports too (RF-to-DC isolation); otherwise they
    /// are matched terminations.
    pub dc_ports: bool,
}

impl Default for Crossover {
    fn default() -> Self {
        Crossover {
            body: 5.08,
            thick: 1.19,
            er: 3.5,
            tand: 0.0018,
            z_gnd: 0.30,
            z_strip: 0.745,
            w_strip: 0.45,
            w_end: 0.45,
            taper: 0.0,
            r_cast: 0.26,
            clear: 0.55,
            pcb_h: 0.508,
            pcb_er: 3.55,
            pcb_tand: 0.0027,
            w_feed: 1.10,
            feed: 3.0,
            w_dc: 0.30,
            margin: 3.0,
            air: 3.0,
            rough_um: Some(0.5),
            dc_ports: false,
        }
    }
}

impl Crossover {
    /// Sets a dimension by name (mm, or the plain number for er and tand);
    /// false for an unknown name.
    pub fn set(&mut self, key: &str, v: f64) -> bool {
        let f = match key {
            "body" => &mut self.body,
            "thick" => &mut self.thick,
            "er" => &mut self.er,
            "tand" => &mut self.tand,
            "z_gnd" => &mut self.z_gnd,
            "z_strip" => &mut self.z_strip,
            "w_strip" => &mut self.w_strip,
            "w_end" => &mut self.w_end,
            "taper" => &mut self.taper,
            "r_cast" => &mut self.r_cast,
            "clear" => &mut self.clear,
            "pcb_h" => &mut self.pcb_h,
            "pcb_er" => &mut self.pcb_er,
            "pcb_tand" => &mut self.pcb_tand,
            "w_feed" => &mut self.w_feed,
            "feed" => &mut self.feed,
            "w_dc" => &mut self.w_dc,
            "margin" => &mut self.margin,
            "air" => &mut self.air,
            "rough" => {
                self.rough_um = if v < 0.0 { None } else { Some(v) };
                return true;
            }
            "dc_ports" => {
                self.dc_ports = v != 0.0;
                return true;
            }
            _ => return false,
        };
        *f = v;
        true
    }

    /// The names `set` takes, for help text.
    pub const KEYS: &'static [&'static str] = &[
        "body", "thick", "er", "tand", "z_gnd", "z_strip", "w_strip", "w_end", "taper", "r_cast",
        "clear", "pcb_h", "pcb_er", "pcb_tand", "w_feed", "feed", "w_dc", "margin", "air", "rough",
        "dc_ports",
    ];

    /// What is wrong with the dimensions, if anything.
    pub fn check(&self) -> Result<(), String> {
        let c = self;
        if !(c.body > 4.0 * c.r_cast && c.thick > 0.0 && c.er >= 1.0 && c.pcb_er >= 1.0) {
            return Err("body, thick, er: out of range".into());
        }
        if !(0.0 < c.z_gnd && c.z_gnd < c.z_strip && c.z_strip < c.thick) {
            return Err("need 0 < z_gnd < z_strip < thick".into());
        }
        if c.clear <= c.r_cast {
            return Err("clear must be larger than r_cast".into());
        }
        if c.w_feed >= c.body - 2.0 * (c.r_cast + 0.45)
            || c.w_dc >= c.body - 2.0 * (c.r_cast + 0.45)
        {
            return Err("the feed or DC line is too wide to pass the ground corners".into());
        }
        if c.taper < 0.0 || 2.0 * c.taper > c.body {
            return Err("taper: 0 to half the body".into());
        }
        if c.w_strip <= 0.0 || c.w_end <= 0.0 || c.pcb_h <= 0.0 || c.feed <= 0.0 {
            return Err("widths, pcb_h and feed must be positive".into());
        }
        Ok(())
    }

    /// The 3D FEM model.
    pub fn model(&self) -> Result<Model, String> {
        self.check()?;
        let c = *self;
        let (bx0, bx1) = (c.feed, c.feed + c.body);
        let (by0, by1) = (c.margin, c.margin + c.body);
        let (xmax, ymax) = (bx1 + c.feed, by1 + c.margin);
        let (xc, yc) = ((bx0 + bx1) / 2.0, (by0 + by1) / 2.0);
        let top = c.thick + c.air;
        let m = |v: f64| v * MM;
        let bx = |lo: [f64; 3], hi: [f64; 3], fill: Fill| Solid {
            shape: Shape::Box {
                lo: [m(lo[0]), m(lo[1]), m(lo[2])],
                hi: [m(hi[0]), m(hi[1]), m(hi[2])],
            },
            fill,
        };
        let cyl = |x: f64, y: f64, r: f64, z0: f64, z1: f64, net: u32| Solid {
            shape: Shape::Cyl {
                axis: 2,
                c: [m(x), m(y)],
                r: m(r),
                lo: m(z0),
                hi: m(z1),
            },
            fill: Fill::Pec { net },
        };
        let mut solids = vec![
            bx(
                [0.0, 0.0, -c.pcb_h],
                [xmax, ymax, 0.0],
                Fill::Dielectric {
                    er: c.pcb_er,
                    tand: c.pcb_tand,
                },
            ),
            bx(
                [bx0, by0, 0.0],
                [bx1, by1, c.thick],
                Fill::Dielectric {
                    er: c.er,
                    tand: c.tand,
                },
            ),
        ];
        // RF castellations: board to stripline (half outside the body, as
        // the solder fillet is).
        solids.push(cyl(bx0, yc, c.r_cast, 0.0, c.z_strip, 1));
        solids.push(cyl(bx1, yc, c.r_cast, 0.0, c.z_strip, 1));
        // Ground corners: castellations through the body, the board's pads
        // under them, and vias to the board's ground: one at the corner, one
        // out past it.
        let corners = [
            (bx0, by0, -1.0, -1.0),
            (bx1, by0, 1.0, -1.0),
            (bx0, by1, -1.0, 1.0),
            (bx1, by1, 1.0, 1.0),
        ];
        let (pad_out, pad_in, via_r, via_out) = (1.0, 0.45, 0.15, 0.7);
        let mut sheets = Vec::new();
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| -> Vec<[f64; 2]> {
            let (x0, x1) = (x0.min(x1), x0.max(x1));
            let (y0, y1) = (y0.min(y1), y0.max(y1));
            vec![
                [m(x0), m(y0)],
                [m(x1), m(y0)],
                [m(x1), m(y1)],
                [m(x0), m(y1)],
            ]
        };
        let sheet =
            |z: f64, poly: Vec<[f64; 2]>, holes: Vec<([f64; 2], [f64; 2], f64)>, net: u32| Sheet {
                axis: 2,
                at: m(z),
                poly,
                holes,
                disc: None,
                net,
            };
        for &(x, y, sx, sy) in &corners {
            solids.push(cyl(x, y, c.r_cast, 0.0, c.thick, 0));
            solids.push(cyl(x, y, via_r, -c.pcb_h, 0.0, 0));
            solids.push(cyl(
                x + sx * via_out,
                y + sy * via_out,
                via_r,
                -c.pcb_h,
                0.0,
                0,
            ));
            sheets.push(sheet(
                0.0,
                rect(
                    x + sx * pad_out,
                    y + sy * pad_out,
                    x - sx * pad_in,
                    y - sy * pad_in,
                ),
                Vec::new(),
                0,
            ));
        }
        // Board: the feeds run in under the castellations; the DC line runs
        // the board's length under the part.
        sheets.push(sheet(
            0.0,
            rect(
                0.0,
                yc - c.w_feed / 2.0,
                bx0 + c.r_cast,
                yc + c.w_feed / 2.0,
            ),
            Vec::new(),
            1,
        ));
        sheets.push(sheet(
            0.0,
            rect(
                bx1 - c.r_cast,
                yc - c.w_feed / 2.0,
                xmax,
                yc + c.w_feed / 2.0,
            ),
            Vec::new(),
            1,
        ));
        sheets.push(sheet(
            0.0,
            rect(xc - c.w_dc / 2.0, 0.0, xc + c.w_dc / 2.0, ymax),
            Vec::new(),
            2,
        ));
        // The part: internal ground (cleared round the RF castellations),
        // the stripline, the top ground.
        let body = rect(bx0, by0, bx1, by1);
        let hole = |x: f64| ([m(x), m(yc)], [m(x), m(yc)], m(c.clear));
        sheets.push(sheet(c.z_gnd, body.clone(), vec![hole(bx0), hole(bx1)], 0));
        // The stripline: its ends `w_end` wide for `taper`, the middle
        // `w_strip`, as one outline.
        let (we, ws) = (c.w_end / 2.0, c.w_strip / 2.0);
        let strip = if c.taper > 0.0 && (c.w_end - c.w_strip).abs() > 1e-9 {
            let (xa, xb) = (bx0 + c.taper, bx1 - c.taper);
            vec![
                [m(bx0), m(yc - we)],
                [m(xa), m(yc - we)],
                [m(xa), m(yc - ws)],
                [m(xb), m(yc - ws)],
                [m(xb), m(yc - we)],
                [m(bx1), m(yc - we)],
                [m(bx1), m(yc + we)],
                [m(xb), m(yc + we)],
                [m(xb), m(yc + ws)],
                [m(xa), m(yc + ws)],
                [m(xa), m(yc + we)],
                [m(bx0), m(yc + we)],
            ]
        } else {
            rect(bx0, yc - ws, bx1, yc + ws)
        };
        sheets.push(sheet(c.z_strip, strip, Vec::new(), 1));
        sheets.push(sheet(c.thick, body, Vec::new(), 0));
        // Reference planes at the part's edges. The DC ports are matched
        // terminations unless asked for (they cost two more solves).
        let port = |name: &str, axis: usize, high: bool, signal: u32| {
            let (span, deembed) = if axis == 0 {
                (ymax, c.feed)
            } else {
                (xmax, c.margin)
            };
            Port {
                name: name.into(),
                axis,
                high,
                lo: [0.0, m(-c.pcb_h)],
                hi: [m(span), m(top)],
                mode: Mode::Tem { signal },
                deembed: m(deembed),
                passive: signal == 2 && !c.dc_ports,
            }
        };
        Ok(Model {
            lo: [0.0, 0.0, m(-c.pcb_h)],
            hi: [m(xmax), m(ymax), m(top)],
            walls: [Wall::Pec; 6],
            er: 1.0,
            solids,
            sheets,
            ports: vec![
                port("RF1", 0, false, 1),
                port("RF2", 0, true, 1),
                port("DC1", 1, false, 2),
                port("DC2", 1, true, 2),
            ],
            copper: c.rough_um.map(|r| Copper {
                rough_um: r,
                // The bottom of the box is the board's ground plane.
                walls: [false, false, false, false, true, false],
            }),
            lumped: Vec::new(),
        })
    }
}

/// The X2AS-class limits: (from GHz, to GHz, return loss at least dB,
/// insertion loss at most dB).
pub const LIMITS: [(f64, f64, f64, f64); 3] = [
    (0.0, 2.5, 20.0, 0.05),
    (2.5, 3.5, 17.0, 0.10),
    (3.5, 6.0, 15.0, 0.15),
];

/// One band's worst figures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub lo: f64,
    pub hi: f64,
    pub rl_min: f64,
    pub rl_limit: f64,
    pub il_max: f64,
    pub il_limit: f64,
}

impl Band {
    pub fn pass(&self) -> bool {
        self.rl_min >= self.rl_limit && self.il_max <= self.il_limit
    }
}

/// The worst return loss (of S11 and S22) and insertion loss (S21) in each
/// band, from (GHz, |S11|, |S21|, |S22|) points.
pub fn grade(points: &[(f64, f64, f64, f64)]) -> Vec<Band> {
    LIMITS
        .iter()
        .filter_map(|&(lo, hi, rl, il)| {
            let pts: Vec<_> = points
                .iter()
                .filter(|p| p.0 > 0.0 && p.0 >= lo - 1e-9 && p.0 <= hi + 1e-9)
                .collect();
            if pts.is_empty() {
                return None;
            }
            let db = |x: f64| -20.0 * x.max(1e-12).log10();
            Some(Band {
                lo,
                hi,
                rl_min: pts
                    .iter()
                    .map(|p| db(p.1).min(db(p.3)))
                    .fold(f64::INFINITY, f64::min),
                rl_limit: rl,
                il_max: pts
                    .iter()
                    .map(|p| db(p.2))
                    .fold(f64::NEG_INFINITY, f64::max),
                il_limit: il,
            })
        })
        .collect()
}

/// The grade as text, one band a line.
pub fn report(bands: &[Band]) -> String {
    let mut o = String::new();
    for b in bands {
        o.push_str(&format!(
            "{:>4.1}-{:<4.1} GHz  RL {:>5.1} dB (>= {:>4.1})  IL {:.3} dB (<= {:.2})  {}\n",
            b.lo,
            b.hi,
            b.rl_min,
            b.rl_limit,
            b.il_max,
            b.il_limit,
            if b.pass() { "pass" } else { "FAIL" }
        ));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_is_consistent() {
        let c = Crossover::default();
        let m = c.model().unwrap();
        assert_eq!(m.ports.len(), 4);
        // Every solid and sheet inside the box (the castellations' outer
        // halves sit over the board, inside it).
        for s in &m.solids {
            let (lo, hi) = match s.shape {
                Shape::Box { lo, hi } => (lo, hi),
                Shape::Cyl { c, r, lo, hi, .. } => {
                    ([c[0] - r, c[1] - r, lo], [c[0] + r, c[1] + r, hi])
                }
            };
            for k in 0..3 {
                assert!(
                    lo[k] >= m.lo[k] - 1e-12 && hi[k] <= m.hi[k] + 1e-12,
                    "{s:?}"
                );
            }
        }
        // The RF feeds reach the x faces, the DC line the y faces.
        let x_touch = |net: u32| {
            m.sheets.iter().filter(|s| s.net == net).any(|s| {
                s.poly
                    .iter()
                    .any(|p| p[0] <= 1e-12 || p[0] >= m.hi[0] - 1e-12)
            })
        };
        let y_touch = |net: u32| {
            m.sheets.iter().filter(|s| s.net == net).any(|s| {
                s.poly
                    .iter()
                    .any(|p| p[1] <= 1e-12 || p[1] >= m.hi[1] - 1e-12)
            })
        };
        assert!(x_touch(1) && !y_touch(1));
        assert!(y_touch(2) && !x_touch(2));
        let mut bad = c;
        bad.z_strip = 2.0;
        assert!(bad.model().is_err());
        assert!(c.clone().set("w_strip", 0.5));
        assert!(!c.clone().set("nope", 1.0));
    }

    #[test]
    fn tapered_strip() {
        let c = Crossover {
            taper: 0.6,
            w_end: 0.35,
            ..Crossover::default()
        };
        let m = c.model().unwrap();
        let strip = m.sheets.iter().find(|s| s.net == 1 && s.at > 0.0).unwrap();
        assert_eq!(strip.poly.len(), 12);
    }

    #[test]
    fn grades() {
        // A perfect through line passes; 0.2 dB loss at 5 GHz fails.
        let g = grade(&[
            (1.0, 0.01, 1.0, 0.01),
            (3.0, 0.05, 0.995, 0.05),
            (5.0, 0.1, 0.977, 0.1),
        ]);
        assert_eq!(g.len(), 3);
        assert!(g[0].pass() && g[1].pass());
        assert!(!g[2].pass(), "{}", report(&g));
    }
}
