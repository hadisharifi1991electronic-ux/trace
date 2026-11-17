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
    // Accurate mode: three grids from 2 mil, extrapolated with error
    // bars, against a solve on a finer grid than any of them. Honest bars
    // hold the finer answer (it is nearer the limit than the grids used).
    let mut acc = via.clone();
    acc.cell = 2.0;
    acc.points = 20;
    let tol = fdtd::Tolerance {
        s11_db: 0.0,
        s21_db: 0.0,
        max_runs: 3,
        ..fdtd::Tolerance::default()
    };
    let a = fdtd::converge(&st, &acc, &tol, &share(0.75, 0.85));
    let mut finer = acc.clone();
    finer.cell = 0.7;
    let f = fdtd::solve_with(&st, &finer, &share(0.85, 0.95));
    if let (Ok(a), Ok(f)) = (a, f) {
        let b = &a.best;
        let n = b.f.len().min(f.f.len());
        let outside = (0..n)
            .filter(|&k| {
                let d11 = b.s11[k].add(f.s11[k].scale(-1.0)).abs();
                let d21 = b.s21[k].add(f.s21[k].scale(-1.0)).abs();
                d11 > b.err11[k] + 1e-4 || d21 > b.err21[k] + 1e-4
            })
            .count();
        out.push(Check {
            group: g,
            name: "Accurate mode (2, 1.4, 1 mil, extrapolated): points where a 0.7 mil solve falls outside the error bars".into(),
            reference: "honesty (the error bars must hold a finer grid's answer)",
            want: 0.0,
            got: 100.0 * outside as f64 / n.max(1) as f64,
            unit: "%",
            limit: 10.0,
            abs: true,
            at_most: true,
        });
        let q = fdtd::quality(b);
        out.push(Check {
            group: g,
            name: "Accurate mode: impulse energy before t = 0".into(),
            reference: "exact (a via is causal; band limit allows 2%)",
            want: 0.0,
            got: 100.0 * q.noncausal,
            unit: "%",
            limit: 2.0,
            abs: true,
            at_most: true,
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
    let c = fdtd::solve_with(&sb, &stub, &share(0.95, 0.97));
    stub.cell = 1.0;
    let f = fdtd::solve_with(&sb, &stub, &share(0.97, 1.0));
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

/// Modified Bessel functions I1 and K1 (Abramowitz & Stegun 9.8.3,
/// 9.8.7, 9.8.8), good to about 1e-7.
fn bessel_k1(x: f64) -> f64 {
    if x <= 2.0 {
        let t = x / 3.75;
        let t2 = t * t;
        let i1 = x
            * (0.5
                + t2 * (0.878_905_94
                    + t2 * (0.514_988_69
                        + t2 * (0.150_849_34
                            + t2 * (0.026_587_33 + t2 * (0.003_015_32 + t2 * 0.000_324_11))))));
        let y = x * x / 4.0;
        (x * (x / 2.0).ln() * i1
            + 1.0
            + y * (0.154_431_44
                + y * (-0.672_785_79
                    + y * (-0.181_568_97
                        + y * (-0.019_194_02 + y * (-0.001_104_04 + y * -0.000_046_86))))))
            / x
    } else {
        let y = 2.0 / x;
        (-x).exp() / x.sqrt()
            * (1.253_314_14
                + y * (0.234_986_19
                    + y * (-0.036_556_20
                        + y * (0.015_042_68
                            + y * (-0.007_803_53 + y * (0.003_256_14 + y * -0.000_682_45))))))
    }
}

/// A board for the thermal checks: `copper` (thickness µm, full pour)
/// per copper layer, dielectrics `diel_mm` between, `w` by `h` mm.
fn thermal_board(copper: &[(f64, bool)], diel_mm: &[f64], w: f64, h: f64) -> crate::Board {
    use crate::*;
    let mut b = Board::default();
    let corner = [
        Point::mil(0.0, 0.0),
        Point {
            x: Length::from_mm(w),
            y: Length::from_mm(0.0),
        },
        Point {
            x: Length::from_mm(w),
            y: Length::from_mm(h),
        },
        Point {
            x: Length::from_mm(0.0),
            y: Length::from_mm(h),
        },
    ];
    b.geometry.outline = corner.to_vec();
    for (i, &(um, full)) in copper.iter().enumerate() {
        if i > 0 {
            b.stackup.layers.push(StackLayer {
                name: format!("D{i}"),
                altium_id: None,
                kind: LayerKind::Dielectric,
                thickness: Length::from_mm(diel_mm[i - 1]),
                dielectric_constant: Some(4.2),
                loss_tangent: None,
                roughness_um: None,
                material: Some("FR-4".into()),
            });
        }
        b.stackup.layers.push(StackLayer {
            name: format!("L{}", i + 1),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length::from_mm(um / 1000.0),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
        if full {
            b.geometry.polygons.push(Polygon {
                layer: LayerRef::Copper(i as u8),
                net: None,
                outline: corner.to_vec(),
                holes: Vec::new(),
                pour: None,
                component: None,
                teardrop: false,
            });
        }
    }
    b
}

fn thermal_part(b: &mut crate::Board, name: &str, x: f64, y: f64) -> usize {
    use crate::*;
    b.components.push(Component {
        designator: name.into(),
        footprint: String::new(),
        library_ref: None,
        x: Length::from_mm(x),
        y: Length::from_mm(y),
        rotation_deg: 0.0,
        side: Side::Top,
        sheet: None,
        params: Default::default(),
    });
    b.components.len() - 1
}

fn thermal_pad(b: &mut crate::Board, part: usize, x: f64, y: f64, w: f64, h: f64, round: bool) {
    use crate::*;
    b.geometry.pads.push(Pad {
        layer: LayerRef::Copper(0),
        net: None,
        component: Some(part as u32),
        name: "1".into(),
        at: Point {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
        },
        size_x: Length::from_mm(w),
        size_y: Length::from_mm(h),
        hole: Length::from_mm(0.0),
        shape: if round {
            PadShape::Round
        } else {
            PadShape::Rect
        },
        rotation_deg: 0.0,
        outline: Vec::new(),
        stack: Vec::new(),
        fab: Default::default(),
    });
}

/// The thermal solver against exact answers and a JEDEC package.
pub fn thermal_checks() -> Vec<Check> {
    use crate::thermal::{self, Air, Setup, Source};
    let g = "3D thermal solver";
    let mut out = Vec::new();
    // 1. A disc of uniform heat on a large thin plate cooled on both
    // faces: θ(0) = q''/(2h) (1 - m a K1(m a)), m = sqrt(2h / (k t)). The
    // plate: 70 µm of copper on 20 µm of laminate (and an empty copper
    // layer under it), so the copper is one sheet through its thickness.
    let mut b = thermal_board(&[(70.0, true), (35.0, false)], &[0.02], 200.0, 200.0);
    let p = thermal_part(&mut b, "TP1", 100.0, 100.0);
    let a = 5.0;
    thermal_pad(&mut b, p, 100.0, 100.0, 2.0 * a, 2.0 * a, true);
    let h = 25.0;
    let mut s = Setup::new();
    s.fixed_h = Some(h);
    s.cell_mm = 1.0;
    s.k_diel = Some((0.81, 0.29));
    s.sources.push(Source {
        part: p,
        watts: 1.0,
        theta_jc: 1e9,
        // A weak link makes the flux into the copper uniform over the
        // disc (a strong one would hold the disc at one temperature).
        theta_jb: 1e5,
        heatsink: None,
    });
    let kt = 390.0 * 70e-6 + 0.81 * 0.055e-3;
    let m = (2.0 * h / kt).sqrt();
    let am = a * 1e-3;
    let q = 1.0 / (PI * am * am);
    let exact = q / (2.0 * h) * (1.0 - m * am * bessel_k1(m * am));
    let coarse = thermal::solve(&b, &s);
    if let Ok(r) = &coarse {
        out.push(Check {
            group: g,
            name: "Heated disc (r 5 mm, 1 W) on a 70 µm copper plate, h 25 both faces: centre rise"
                .into(),
            reference: "exact (fin equation on a plate: Bessel K1)",
            want: exact,
            got: r.board_max.0 - r.ambient_c,
            unit: "K",
            limit: 3.0,
            abs: false,
            at_most: false,
        });
        out.push(Check {
            group: g,
            name: "Heated disc: heat out of the faces against the power in".into(),
            reference: "exact (energy balance)",
            want: 0.0,
            got: r.balance_pct.abs(),
            unit: "%",
            limit: 0.1,
            abs: true,
            at_most: true,
        });
    }
    s.cell_mm = 0.67;
    if let (Ok(c), Ok(f)) = (&coarse, thermal::solve(&b, &s)) {
        out.push(Check {
            group: g,
            name: "Heated disc: centre rise, 1 mm cells vs 0.67 mm".into(),
            reference: "convergence (the finer grid)",
            want: f.board_max.0 - f.ambient_c,
            got: c.board_max.0 - c.ambient_c,
            unit: "K",
            limit: 2.0,
            abs: false,
            at_most: false,
        });
    }
    // 2. A straight fin: the closed-form efficiency against a fine finite
    // difference solution of k t θ'' = 2 h θ with a convecting tip.
    {
        let (hh, k, t, len) = (40.0, 200.0, 1.0e-3, 30.0e-3);
        let n = 4000;
        let dx = len / n as f64;
        // Tridiagonal: root θ = 1.
        let mut a = vec![0.0; n + 1];
        let mut bb = vec![0.0; n + 1];
        let mut c = vec![0.0; n + 1];
        let mut d = vec![0.0; n + 1];
        bb[0] = 1.0;
        d[0] = 1.0;
        for i in 1..n {
            a[i] = k * t / dx;
            c[i] = k * t / dx;
            bb[i] = -(2.0 * k * t / dx + 2.0 * hh * dx);
        }
        a[n] = k * t / dx;
        bb[n] = -(k * t / dx + hh * dx + hh * t);
        for i in 1..=n {
            let w = a[i] / bb[i - 1];
            bb[i] -= w * c[i - 1];
            d[i] -= w * d[i - 1];
        }
        let mut th = vec![0.0; n + 1];
        th[n] = d[n] / bb[n];
        for i in (0..n).rev() {
            th[i] = (d[i] - c[i] * th[i + 1]) / bb[i];
        }
        let q_root = k * t * (th[0] - th[1]) / dx + hh * dx * th[0];
        let ideal = hh * (2.0 * len + t);
        let eta_fd = q_root / ideal;
        let eta = thermal::fin_efficiency(hh, k, t * 1e3, len * 1e3);
        out.push(Check {
            group: g,
            name: "Heatsink fin (1 x 30 mm aluminium, h 40): efficiency".into(),
            reference: "exact (finite differences, 4000 steps)",
            want: eta_fd,
            got: eta,
            unit: "",
            limit: 1.0,
            abs: false,
            at_most: false,
        });
    }
    // 3. A SOIC-8 on the JEDEC JESD51-7 2s2p board (114.3 x 76.2 mm,
    // 2 oz outer, 1 oz planes, 1.6 mm) in still air, from its datasheet
    // θJC(top) 67.6 and θJB 59.6 K/W (TI TPS2066A, D package): the
    // datasheet's θJA is 119.3 K/W. A two-resistor model is good to about
    // 20% on θJA, so the check allows 25%.
    let mut b = thermal_board(
        &[(70.0, false), (35.0, true), (35.0, true), (70.0, false)],
        &[0.71, 0.71, 0.0001_f64.max(0.71)],
        114.3,
        76.2,
    );
    let (cx, cy) = (57.15, 38.1);
    let p = thermal_part(&mut b, "U1", cx, cy);
    for k in 0..4 {
        let x = cx - 1.905 + 1.27 * k as f64;
        for sy in [-2.7, 2.7] {
            thermal_pad(&mut b, p, x, cy + sy, 0.6, 1.55, false);
            // JESD51-7 fan-out: a 0.25 mm trace from each lead outward.
            let y1 = if sy < 0.0 { cy - 25.0 } else { cy + 25.0 };
            b.geometry.tracks.push(crate::Track {
                layer: crate::LayerRef::Copper(0),
                net: None,
                start: crate::Point {
                    x: Length::from_mm(x),
                    y: Length::from_mm(cy + sy),
                },
                end: crate::Point {
                    x: Length::from_mm(x),
                    y: Length::from_mm(y1),
                },
                width: Length::from_mm(0.25),
                component: None,
                text: false,
            });
        }
    }
    let mut s = Setup::new();
    s.air = Air::Natural { vertical: false };
    s.cell_mm = 0.5;
    s.sources.push(Source {
        part: p,
        watts: 0.5,
        theta_jc: 67.6,
        theta_jb: 59.6,
        heatsink: None,
    });
    // 4. A 3 x 3 mm SON-8 with an exposed pad soldered to four thermal
    // vias into the planes (JESD51-5), same board: TI TPS2066A DRB,
    // θJC(top) 53, θJB 14.2, θJA 47.5 K/W.
    let mut b2 = thermal_board(
        &[(70.0, false), (35.0, true), (35.0, true), (70.0, false)],
        &[0.71, 0.71, 0.71],
        114.3,
        76.2,
    );
    let p2 = thermal_part(&mut b2, "U1", cx, cy);
    thermal_pad(&mut b2, p2, cx, cy, 1.65, 2.4, false);
    for k in 0..4 {
        let x = cx - 0.975 + 0.65 * k as f64;
        for sy in [-1.45, 1.45] {
            thermal_pad(&mut b2, p2, x, cy + sy, 0.3, 0.6, false);
            let y1 = if sy < 0.0 { cy - 25.0 } else { cy + 25.0 };
            b2.geometry.tracks.push(crate::Track {
                layer: crate::LayerRef::Copper(0),
                net: None,
                start: crate::Point {
                    x: Length::from_mm(x),
                    y: Length::from_mm(cy + sy),
                },
                end: crate::Point {
                    x: Length::from_mm(x),
                    y: Length::from_mm(y1),
                },
                width: Length::from_mm(0.25),
                component: None,
                text: false,
            });
        }
    }
    for (dx, dy) in [(-0.4, -0.6), (0.4, -0.6), (-0.4, 0.6), (0.4, 0.6)] {
        b2.geometry.vias.push(crate::Via {
            net: None,
            from_layer: 0,
            to_layer: 3,
            at: crate::Point {
                x: Length::from_mm(cx + dx),
                y: Length::from_mm(cy + dy),
            },
            diameter: Length::from_mm(0.6),
            hole: Length::from_mm(0.33),
            rings: Vec::new(),
            fab: Default::default(),
        });
    }
    let mut s2 = s.clone();
    s2.sources[0].part = p2;
    s2.sources[0].theta_jc = 53.0;
    s2.sources[0].theta_jb = 14.2;
    if let Ok(r) = thermal::solve(&b2, &s2) {
        out.push(Check {
            group: g,
            name: "SON-8 with exposed pad and 4 thermal vias, JEDEC 2s2p, still air: θJA".into(),
            reference: "datasheet (TI TPS2066A DRB: 47.5 K/W, JESD51-7/-5)",
            want: 47.5,
            got: r.parts[0].theta_ja,
            unit: "K/W",
            limit: 25.0,
            abs: false,
            at_most: false,
        });
    }
    if let Ok(r) = thermal::solve(&b, &s) {
        out.push(Check {
            group: g,
            name: "SOIC-8 on the JEDEC 2s2p board in still air: θJA from θJC and θJB".into(),
            reference: "datasheet (TI TPS2066A D: 119.3 K/W, JESD51-7)",
            want: 119.3,
            got: r.parts[0].theta_ja,
            unit: "K/W",
            limit: 25.0,
            abs: false,
            at_most: false,
        });
    }
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
