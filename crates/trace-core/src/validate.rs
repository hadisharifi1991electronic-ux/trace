//! Accuracy checks: each solver against cases with a known answer.
//!
//! Exact references are closed-form solutions of the same problem (Cohn's
//! conformal maps for thin striplines, ρL/(wt) for a strip of copper).
//! Formula references (Hammerstad and Jensen, conformal CPW) are
//! themselves good to about 1%. The 3D solver has no closed form for a
//! via, so it is checked on what has one (a straight line must not
//! reflect, energy must be conserved), against the 2D solver (an
//! independent method for the same line), and by halving its cells.

use crate::fdtd::{self, ViaSetup};
use crate::impedance::LayerGeometry;
use crate::{LayerKind, Length, StackLayer, Stackup};
use std::f64::consts::PI;

/// One check.
#[derive(Clone, Debug)]
pub struct Check {
    /// "Field solver", "IR drop" or "3D via solver".
    pub group: &'static str,
    pub name: String,
    /// Where the expected value comes from.
    pub reference: &'static str,
    pub want: f64,
    pub got: f64,
    pub unit: &'static str,
    /// Allowed error: percent of `want`, or (when `abs`) in `unit`.
    pub limit: f64,
    pub abs: bool,
    /// Only an error above `want` counts (a bound, not a target).
    pub at_most: bool,
}

impl Check {
    /// The error, in percent (or in `unit` when `abs`).
    pub fn error(&self) -> f64 {
        if self.abs {
            self.got - self.want
        } else {
            100.0 * (self.got - self.want) / self.want
        }
    }
    pub fn pass(&self) -> bool {
        let e = self.error();
        self.got.is_finite() && e <= self.limit && (self.at_most || -e <= self.limit)
    }
}

fn agm(mut a: f64, mut g: f64) -> f64 {
    for _ in 0..40 {
        let (a2, g2) = ((a + g) / 2.0, (a * g).sqrt());
        a = a2;
        g = g2;
    }
    a
}

/// K(k)/K(k'), exactly.
fn kratio(k: f64) -> f64 {
    agm(1.0, k) / agm(1.0, (1.0 - k * k).sqrt())
}

fn geo(above: Option<f64>, below: Option<f64>, er: f64) -> LayerGeometry {
    LayerGeometry {
        copper: 1,
        name: "L".into(),
        t: 0.0,
        above,
        below,
        er,
        tan_d: None,
        rough_um: None,
        slabs_above: Vec::new(),
        slabs_below: Vec::new(),
        mask: None,
    }
}

/// The 2D field solver against exact and textbook values.
pub fn field_checks() -> Vec<Check> {
    let mut out = Vec::new();
    let er = 4.2;
    let k0 = 30.0 * PI / f64::sqrt(er);
    let g = "Field solver";
    for (w, b) in [(5.0, 10.0), (8.0, 12.0), (3.0, 16.0), (12.0, 10.0)] {
        let l = geo(Some(b / 2.0), Some(b / 2.0), er);
        if let Some(line) = crate::field::single(&l, w) {
            let k = 1.0 / (PI * w / (2.0 * b)).cosh();
            out.push(Check {
                group: g,
                name: format!("Stripline {w} mil wide, planes {b} mil apart: Z0"),
                reference: "exact (Cohn, thin strip)",
                want: k0 * kratio(k),
                got: line.z0,
                unit: "Ω",
                limit: 1.0,
                abs: false,
                at_most: false,
            });
        }
    }
    if let Some(line) = crate::field::single(&geo(Some(5.0), Some(5.0), er), 5.0) {
        out.push(Check {
            group: g,
            name: "Stripline delay".into(),
            reference: "exact (√Er / c)",
            want: er.sqrt() / 2.997_924_58e8 * 0.0254 * 1e12,
            got: line.ps_per_in(),
            unit: "ps/in",
            limit: 0.5,
            abs: false,
            at_most: false,
        });
    }
    for (w, s, b) in [(5.0, 5.0, 10.0), (4.0, 8.0, 12.0)] {
        let l = geo(Some(b / 2.0), Some(b / 2.0), er);
        if let Some(p) = crate::field::pair(&l, w, s) {
            let a = (PI * w / (2.0 * b)).tanh();
            let c = (PI * (w + s) / (2.0 * b)).tanh();
            // Z = 30π/√Er · K(k')/K(k), k = a·c (even), a/c (odd).
            let ze = k0 / kratio(a * c);
            let zo = k0 / kratio(a / c);
            out.push(Check {
                group: g,
                name: format!("Stripline pair {w}/{s} mil, planes {b} mil apart: Zodd"),
                reference: "exact (Cohn, thin strips)",
                want: zo,
                got: p.z_odd,
                unit: "Ω",
                limit: 1.5,
                abs: false,
                at_most: false,
            });
            out.push(Check {
                group: g,
                name: format!("Stripline pair {w}/{s} mil, planes {b} mil apart: Zeven"),
                reference: "exact (Cohn, thin strips)",
                want: ze,
                got: p.z_even,
                unit: "Ω",
                limit: 1.5,
                abs: false,
                at_most: false,
            });
        }
    }
    for (w, h) in [(10.0, 5.0), (5.0, 5.0), (20.0, 10.0)] {
        let l = geo(None, Some(h), er);
        if let Some(line) = crate::field::single(&l, w) {
            out.push(Check {
                group: g,
                name: format!("Microstrip {w} mil wide, {h} mil high: Z0"),
                reference: "formula (Hammerstad and Jensen, about 0.2%)",
                want: crate::impedance::microstrip(w, 0.0, h, er),
                got: line.z0,
                unit: "Ω",
                limit: 2.0,
                abs: false,
                at_most: false,
            });
            out.push(Check {
                group: g,
                name: format!("Microstrip {w} mil wide, {h} mil high: Eeff"),
                reference: "formula (Hammerstad and Jensen, about 0.2%)",
                want: crate::impedance::microstrip_eeff(w, h, er),
                got: line.eeff,
                unit: "",
                limit: 2.0,
                abs: false,
                at_most: false,
            });
        }
    }
    // Grounded coplanar waveguide, by conformal mapping.
    let l = geo(None, Some(10.0), er);
    if let Some(cpw) = crate::field::single_with(&l, 10.0, Some(5.0)) {
        let k = 10.0 / 20.0;
        let k3 = (PI * 10.0 / 40.0).tanh() / (PI * 20.0 / 40.0).tanh();
        let q = kratio(k3) / kratio(k);
        let ee = (1.0 + er * q) / (1.0 + q);
        out.push(Check {
            group: g,
            name: "Grounded CPW 10 mil, gaps 5 mil, 10 mil high: Z0".into(),
            reference: "formula (conformal mapping, about 1-2%)",
            want: 60.0 * PI / ee.sqrt() / (kratio(k) + kratio(k3)),
            got: cpw.z0,
            unit: "Ω",
            limit: 3.0,
            abs: false,
            at_most: false,
        });
    }
    out
}

/// The IR drop solver against ρL/(wt).
pub fn ir_checks() -> Vec<Check> {
    use crate::*;
    let mut out = Vec::new();
    let mut b = Board::default();
    for i in 0..2 {
        if i > 0 {
            b.stackup.layers.push(StackLayer {
                name: "D1".into(),
                altium_id: None,
                kind: LayerKind::Dielectric,
                thickness: Length::from_mil(10.0),
                dielectric_constant: Some(4.2),
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.stackup.layers.push(StackLayer {
            name: format!("L{i}"),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length::from_mil(1.4),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
    }
    b.nets.push(Net {
        name: "VDD".into(),
        ..Default::default()
    });
    for d in ["U1", "U2"] {
        b.components.push(Component {
            designator: d.into(),
            footprint: String::new(),
            library_ref: None,
            x: Length::from_mil(0.0),
            y: Length::from_mil(0.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
    }
    let pad = |part: u32, x: f64, y: f64, w: f64, h: f64| Pad {
        layer: LayerRef::Copper(0),
        net: Some(0),
        component: Some(part),
        name: "1".into(),
        at: Point::mil(x, y),
        size_x: Length::from_mil(w),
        size_y: Length::from_mil(h),
        hole: Length::from_mil(0.0),
        shape: PadShape::Rect,
        rotation_deg: 0.0,
        outline: Vec::new(),
        stack: Vec::new(),
        fab: Default::default(),
    };
    let r_cu = crate::pi::RHO;
    let mil = 25.4e-6;
    // A pour 2000 by 200 mil, fed and loaded across its ends.
    let mut pour = b.clone();
    pour.geometry.polygons.push(Polygon {
        teardrop: false,
        layer: LayerRef::Copper(0),
        net: Some(0),
        outline: vec![
            Point::mil(0.0, 0.0),
            Point::mil(2000.0, 0.0),
            Point::mil(2000.0, 200.0),
            Point::mil(0.0, 200.0),
        ],
        holes: Vec::new(),
        pour: None,
        component: None,
    });
    pour.geometry.pads.push(pad(0, 10.0, 100.0, 20.0, 200.0));
    pour.geometry.pads.push(pad(1, 1990.0, 100.0, 20.0, 200.0));
    let mut s = crate::pi::Setup::new(0, 0, 1.0);
    s.loads.push((1, 2.0));
    if let Ok(r) = crate::pi::solve(&pour, &s) {
        out.push(Check {
            group: "IR drop",
            name: "Pour 1980 by 200 mil, 1 oz, 2 A: drop".into(),
            reference: "exact (I·ρL/wt)",
            want: 1e3 * 2.0 * r_cu * 1980.0 / (200.0 * 1.4 * mil),
            got: 1e3 * (1.0 - r.loads[0].v_mean),
            unit: "mV",
            limit: 3.0,
            abs: false,
            at_most: false,
        });
    }
    let mut track = b.clone();
    track.geometry.tracks.push(Track {
        layer: LayerRef::Copper(0),
        net: Some(0),
        start: Point::mil(0.0, 0.0),
        end: Point::mil(3000.0, 0.0),
        width: Length::from_mil(4.0),
        component: None,
        text: false,
    });
    track.geometry.pads.push(pad(0, 0.0, 0.0, 4.0, 4.0));
    track.geometry.pads.push(pad(1, 3000.0, 0.0, 4.0, 4.0));
    let mut s = crate::pi::Setup::new(0, 0, 1.0);
    s.loads.push((1, 0.5));
    if let Ok(r) = crate::pi::solve(&track, &s) {
        out.push(Check {
            group: "IR drop",
            name: "Track 3000 by 4 mil, 1 oz, 0.5 A: drop".into(),
            reference: "exact (I·ρL/wt)",
            want: 1e3 * 0.5 * r_cu * 3000.0 / (4.0 * 1.4 * mil),
            got: 1e3 * (1.0 - r.loads[0].v_min),
            unit: "mV",
            limit: 2.0,
            abs: false,
            at_most: false,
        });
    }
    out
}

/// A board of equal 5 mil cores, Er 3.6, no loss: `kinds` per copper
/// layer, 'S' signal or 'G' plane.
pub fn board_of(kinds: &str, core: f64) -> Stackup {
    let mut s = Stackup::default();
    for (i, k) in kinds.chars().enumerate() {
        if i > 0 {
            s.layers.push(StackLayer {
                name: format!("D{i}"),
                altium_id: None,
                kind: LayerKind::Dielectric,
                thickness: Length::from_mil(core),
                dielectric_constant: Some(3.6),
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        s.layers.push(StackLayer {
            name: if k == 'G' {
                format!("GND{i}")
            } else {
                format!("SIG{i}")
            },
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length::from_mil(0.0),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
    }
    s
}

/// The 3D via solver: what has a known answer, the 2D solver, and the
/// change when its cells are halved. `progress` gets 0 to 1. Takes a few
/// minutes.
pub fn via_checks(progress: &dyn Fn(f64)) -> Vec<Check> {
    let g = "3D via solver";
    let mut out = Vec::new();
    let st = board_of("SGGS", 5.0);
    let mut v = ViaSetup::new(0, 0);
    v.fmax_ghz = 20.0;
    v.points = 40;
    let share = |a: f64, b: f64| move |p: f64| progress(a + (b - a) * p);
    if let Ok(r) = fdtd::solve_with(&st, &v, &share(0.0, 0.15)) {
        let worst = r.s11.iter().map(|s| s.db()).fold(f64::MIN, f64::max);
        let il = r.s21.iter().map(|s| s.db().abs()).fold(0.0, f64::max);
        out.push(Check {
            group: g,
            name: "Straight microstrip (no via), to 20 GHz: worst reflection".into(),
            reference: "exact (a uniform line does not reflect; at most −30 dB)",
            want: -30.0,
            got: worst,
            unit: "dB",
            limit: 0.0,
            abs: true,
            at_most: true,
        });
        out.push(Check {
            group: g,
            name: "Straight microstrip (no via), to 20 GHz: worst |S21| error".into(),
            reference: "exact (0 dB, lossless)",
            want: 0.0,
            got: il,
            unit: "dB",
            limit: 0.1,
            abs: true,
            at_most: true,
        });
        out.push(Check {
            group: g,
            name: "Microstrip impedance in the 3D grid".into(),
            reference: "independent (2D field solver)",
            want: r.z0[0],
            got: r.z_line[0],
            unit: "Ω",
            limit: 3.0,
            abs: false,
            at_most: false,
        });
    }
    // A through via, solved, then again with half the cells.
    let mut via = ViaSetup::new(0, 3);
    via.fmax_ghz = 20.0;
    via.points = 40;
    via.cell = 2.0;
    let coarse = fdtd::solve_with(&st, &via, &share(0.15, 0.3));
    if let Ok(r) = &coarse {
        let p = r
            .s11
            .iter()
            .zip(&r.s21)
            .map(|(a, b)| a.abs().powi(2) + b.abs().powi(2))
            .fold(0.0, f64::max);
        out.push(Check {
            group: g,
            name: "Through via, to 20 GHz: worst |S11|² + |S21|²".into(),
            reference: "exact (≤ 1: a lossless via makes no energy)",
            want: 1.0,
            got: p,
            unit: "",
            limit: 2.0,
            abs: false,
            at_most: true,
        });
        out.push(Check {
            group: g,
            name: "Stripline impedance in the 3D grid".into(),
            reference: "independent (2D field solver)",
            want: r.z0[1],
            got: r.z_line[1],
            unit: "Ω",
            limit: 3.0,
            abs: false,
            at_most: false,
        });
    }
    via.cell = 1.0;
    let fine = fdtd::solve_with(&st, &via, &share(0.3, 0.75));
    if let (Ok(c), Ok(f)) = (&coarse, &fine) {
        let k = c.f.len() * 3 / 4;
        out.push(Check {
            group: g,
            name: format!(
                "Through via: S11 at {:.0} GHz, 2 mil cells vs 1 mil",
                c.f[k]
            ),
            reference: "convergence (the finer grid)",
            want: f.s11[k].db(),
            got: c.s11[k].db(),
            unit: "dB",
            limit: 2.0,
            abs: true,
            at_most: false,
        });
        out.push(Check {
            group: g,
            name: format!(
                "Through via: S21 at {:.0} GHz, 2 mil cells vs 1 mil",
                c.f[k]
            ),
            reference: "convergence (the finer grid)",
            want: f.s21[k].db(),
            got: c.s21[k].db(),
            unit: "dB",
            limit: 0.1,
            abs: true,
            at_most: false,
        });
    }
    // A stub's notch, coarse and fine.
    let sb = board_of("SGSGGGGS", 12.0);
    let mut stub = ViaSetup::new(0, 2);
    stub.fmax_ghz = 30.0;
    stub.points = 120;
    stub.drill = 12.0;
    stub.pad = 24.0;
    stub.antipad = 40.0;
    stub.return_dist = 45.0;
    stub.cell = 2.0;
    let c = fdtd::solve_with(&sb, &stub, &share(0.75, 0.82));
    stub.cell = 1.0;
    let f = fdtd::solve_with(&sb, &stub, &share(0.82, 1.0));
    if let (Ok(c), Ok(f)) = (c, f) {
        if let (Some(nc), Some(nf)) = (c.notch(), f.notch()) {
            out.push(Check {
                group: g,
                name: "60 mil via stub: notch frequency, 2 mil cells vs 1 mil".into(),
                reference: "convergence (the finer grid)",
                want: nf.0,
                got: nc.0,
                unit: "GHz",
                limit: 3.0,
                abs: false,
                at_most: false,
            });
        }
    }
    progress(1.0);
    out
}

/// The checks as a text table.
pub fn report(checks: &[Check]) -> String {
    let mut s = String::new();
    let mut group = "";
    for c in checks {
        if c.group != group {
            group = c.group;
            s += &format!("\n{group}\n");
        }
        let err = if c.abs {
            format!("{:+.3} {}", c.error(), c.unit)
        } else {
            format!("{:+.2}%", c.error())
        };
        let lim = if c.at_most {
            if c.abs {
                format!("at most {:+} {}", c.limit, c.unit)
            } else {
                format!("at most {:+}%", c.limit)
            }
        } else if c.abs {
            format!("±{} {}", c.limit, c.unit)
        } else {
            format!("±{}%", c.limit)
        };
        s += &format!(
            "  {} {}\n      want {:.4} {}, got {:.4}: error {err} (allowed {lim}); {}\n",
            if c.pass() { "PASS" } else { "FAIL" },
            c.name,
            c.want,
            c.unit,
            c.got,
            c.reference
        );
    }
    let n = checks.iter().filter(|c| c.pass()).count();
    s += &format!("\n{n} of {} checks pass\n", checks.len());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_and_ir_checks_pass() {
        let mut c = field_checks();
        c.extend(ir_checks());
        assert!(c.len() >= 15);
        let failed: Vec<_> = c.iter().filter(|c| !c.pass()).collect();
        assert!(failed.is_empty(), "{}", report(&c));
    }
}
