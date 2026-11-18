//! A library of chip inductor and capacitor parasitics: self-resonant
//! frequency, Q and ESR/ESL for 0201, 0402 (and larger) parts, so that
//! RF paths see real parts rather than ideal L and C.
//!
//! A capacitor is C in series with its ESL and an ESR that grows with
//! frequency (electrode skin loss plus the dielectric's loss tangent).
//! An inductor is L in series with a resistance that grows with
//! frequency (DC resistance plus skin loss), with its winding
//! capacitance across the whole; that capacitance sets the SRF.
//!
//! Families are fitted to typical (not minimum) datasheet values of the
//! common Murata and Coilcraft series. A part is matched to a family by
//! its part number or value text; otherwise a generic family for its
//! case is used. A vendor .s2p (`--model`) still wins over any of this.

use crate::fdtd::Cx;
use crate::pdn::Package;

const TAU: f64 = std::f64::consts::TAU;

/// Capacitor or inductor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cap,
    Ind,
}

/// One series of parts and its fitted parasitics.
#[derive(Clone, Copy, Debug)]
pub struct Family {
    /// "Murata LQW15AN", "generic 0402 C0G".
    pub name: &'static str,
    /// Part number prefixes that pick this family (upper case).
    pub keys: &'static [&'static str],
    pub kind: Kind,
    pub case: Package,
    /// "wire-wound", "multilayer", "thin-film", "C0G", "X7R".
    pub tech: &'static str,
    // Capacitors.
    /// Series inductance as mounted on its datasheet fixture, H.
    pub esl: f64,
    /// Electrode loss at 1 GHz, Ω (grows as √f).
    pub r1: f64,
    /// Dielectric loss tangent.
    pub tan_d: f64,
    // Inductors.
    /// Winding capacitance of a 10 nH part, F, and how it scales with L
    /// (C ∝ L^cp_exp).
    pub cp10: f64,
    pub cp_exp: f64,
    /// Q of a 10 nH part at `f_ref` GHz.
    pub q_ref: f64,
    pub f_ref: f64,
    /// DC resistance of a 10 nH part, Ω, and its scaling with L.
    pub rdc10: f64,
    pub rdc_exp: f64,
}

const fn cap(
    name: &'static str,
    keys: &'static [&'static str],
    case: Package,
    tech: &'static str,
    esl_nh: f64,
    r1: f64,
    tan_d: f64,
) -> Family {
    Family {
        name,
        keys,
        kind: Kind::Cap,
        case,
        tech,
        esl: esl_nh * 1e-9,
        r1,
        tan_d,
        cp10: 0.0,
        cp_exp: 0.0,
        q_ref: 0.0,
        f_ref: 1.0,
        rdc10: 0.0,
        rdc_exp: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
const fn ind(
    name: &'static str,
    keys: &'static [&'static str],
    case: Package,
    tech: &'static str,
    srf10_ghz: f64,
    cp_exp: f64,
    q_ref: f64,
    f_ref: f64,
    rdc10: f64,
) -> Family {
    // C from the SRF of the 10 nH part: 1 / ((2π f)² L).
    let w = TAU * srf10_ghz * 1e9;
    Family {
        name,
        keys,
        kind: Kind::Ind,
        case,
        tech,
        esl: 0.0,
        r1: 0.0,
        tan_d: 0.0,
        cp10: 1.0 / (w * w * 10e-9),
        cp_exp,
        q_ref,
        f_ref,
        rdc10,
        rdc_exp: 0.85,
    }
}

use Package::{P0201, P0402, P0603, P0805};

/// The library. Named families first; the generic ones (no keys) last.
pub static FAMILIES: &[Family] = &[
    // Capacitors: high-Q RF C0G.
    cap(
        "Murata GJM03 (high-Q)",
        &["GJM03"],
        P0201,
        "C0G",
        0.18,
        0.12,
        2e-4,
    ),
    cap(
        "Murata GJM15 (high-Q)",
        &["GJM15"],
        P0402,
        "C0G",
        0.25,
        0.08,
        2e-4,
    ),
    cap(
        "Murata GQM18 (high-Q)",
        &["GQM18"],
        P0603,
        "C0G",
        0.40,
        0.06,
        2e-4,
    ),
    cap(
        "Murata GQM22 (high-Q)",
        &["GQM22"],
        P0805,
        "C0G",
        0.55,
        0.05,
        2e-4,
    ),
    // General-purpose MLCC series.
    cap("Murata GRM03", &["GRM03"], P0201, "C0G", 0.20, 0.20, 1e-3),
    cap("Murata GRM15", &["GRM15"], P0402, "C0G", 0.30, 0.15, 1e-3),
    cap("Murata GRM18", &["GRM18"], P0603, "C0G", 0.45, 0.12, 1e-3),
    // Inductors.
    ind(
        "Coilcraft 0201HL (wire-wound)",
        &["0201HL"],
        P0201,
        "wire-wound",
        4.6,
        0.10,
        35.0,
        1.0,
        0.45,
    ),
    ind(
        "Coilcraft 0201DS (wire-wound)",
        &["0201DS"],
        P0201,
        "wire-wound",
        4.4,
        0.10,
        30.0,
        1.0,
        0.50,
    ),
    ind(
        "Coilcraft 0402HP (wire-wound)",
        &["0402HP", "0402CS"],
        P0402,
        "wire-wound",
        3.9,
        0.05,
        55.0,
        1.0,
        0.20,
    ),
    ind(
        "Coilcraft 0402DC (wire-wound)",
        &["0402DC"],
        P0402,
        "wire-wound",
        4.2,
        0.05,
        60.0,
        1.0,
        0.18,
    ),
    ind(
        "Murata LQW15AN (wire-wound)",
        &["LQW15"],
        P0402,
        "wire-wound",
        4.4,
        0.08,
        42.0,
        1.0,
        0.29,
    ),
    ind(
        "Murata LQW03AW (wire-wound)",
        &["LQW03"],
        P0201,
        "wire-wound",
        5.0,
        0.10,
        30.0,
        1.0,
        0.60,
    ),
    ind(
        "Murata LQG15HS (multilayer)",
        &["LQG15"],
        P0402,
        "multilayer",
        4.0,
        0.15,
        18.0,
        1.0,
        0.45,
    ),
    ind(
        "Murata LQP15MN (thin-film)",
        &["LQP15"],
        P0402,
        "thin-film",
        3.6,
        0.20,
        20.0,
        1.0,
        0.90,
    ),
    ind(
        "Murata LQP03TN (thin-film)",
        &["LQP03TN", "LQP03TQ"],
        P0201,
        "thin-film",
        4.4,
        0.20,
        20.0,
        1.0,
        1.10,
    ),
    ind(
        "Murata LQP03HQ (thin-film, high-Q)",
        &["LQP03HQ", "LQP03"],
        P0201,
        "thin-film",
        4.8,
        0.20,
        30.0,
        1.0,
        0.75,
    ),
    ind(
        "Coilcraft 0603HP (wire-wound)",
        &["0603HP", "0603CS"],
        P0603,
        "wire-wound",
        4.0,
        0.05,
        65.0,
        1.0,
        0.13,
    ),
    ind(
        "Murata LQW18AN (wire-wound)",
        &["LQW18"],
        P0603,
        "wire-wound",
        4.1,
        0.05,
        60.0,
        1.0,
        0.16,
    ),
    // Generic by case, when the part number says nothing.
    cap("generic 0201 MLCC", &[], P0201, "C0G", 0.20, 0.18, 8e-4),
    cap("generic 0402 MLCC", &[], P0402, "C0G", 0.30, 0.13, 8e-4),
    cap("generic 0603 MLCC", &[], P0603, "C0G", 0.45, 0.10, 8e-4),
    cap("generic 0805 MLCC", &[], P0805, "C0G", 0.60, 0.08, 8e-4),
    ind(
        "generic 0201 RF inductor (thin-film)",
        &[],
        P0201,
        "thin-film",
        4.5,
        0.20,
        22.0,
        1.0,
        1.0,
    ),
    ind(
        "generic 0402 RF inductor (wire-wound)",
        &[],
        P0402,
        "wire-wound",
        4.1,
        0.07,
        40.0,
        1.0,
        0.25,
    ),
    ind(
        "generic 0603 RF inductor (wire-wound)",
        &[],
        P0603,
        "wire-wound",
        4.0,
        0.05,
        55.0,
        1.0,
        0.15,
    ),
    ind(
        "generic 0805 RF inductor (wire-wound)",
        &[],
        P0805,
        "wire-wound",
        3.6,
        0.05,
        60.0,
        1.0,
        0.12,
    ),
];

/// A fitted part: a family and a value.
#[derive(Clone, Debug)]
pub struct ChipModel {
    pub family: &'static Family,
    /// F for a capacitor, H for an inductor.
    pub value: f64,
    /// Class II dielectric (X7R/X5R): a large lossy capacitor.
    pub class2: bool,
}

/// Picks the family for a part from its part number, value and
/// footprint text, else the generic one for its kind and case.
pub fn family(kind: Kind, case: Package, text: &str) -> &'static Family {
    let up = text.to_ascii_uppercase();
    let named = FAMILIES
        .iter()
        .filter(|f| f.kind == kind && !f.keys.is_empty())
        .find(|f| f.keys.iter().any(|k| up.contains(k)));
    if let Some(f) = named {
        return f;
    }
    let case = match case {
        Package::P1206 | Package::Large => P0805,
        c => c,
    };
    FAMILIES
        .iter()
        .find(|f| f.kind == kind && f.keys.is_empty() && f.case == case)
        .expect("a generic family for every case")
}

/// The family named by `name` (any part of its name, or one of its keys).
pub fn family_named(kind: Kind, name: &str) -> Option<&'static Family> {
    let up = name.to_ascii_uppercase();
    FAMILIES.iter().find(|f| {
        f.kind == kind
            && (f
                .keys
                .iter()
                .any(|k| up.contains(k) || k.contains(up.as_str()))
                || f.name.to_ascii_uppercase().contains(&up))
    })
}

impl ChipModel {
    /// A capacitor of `farads` in `case`; `text` is its part number,
    /// value and footprint (for the family and dielectric).
    pub fn capacitor(farads: f64, case: Package, text: &str) -> ChipModel {
        let up = text.to_ascii_uppercase();
        let c0g = ["C0G", "COG", "NP0", "NPO", "CH"].iter().any(|k| {
            up.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|t| t == *k)
        });
        let x = ["X7R", "X5R", "X7S", "X6S", "X8R", "Y5V"]
            .iter()
            .any(|k| up.contains(k));
        ChipModel {
            family: family(Kind::Cap, case, text),
            value: farads,
            class2: x || (!c0g && farads > 1.0e-9),
        }
    }

    /// An inductor of `henries` in `case`.
    pub fn inductor(henries: f64, case: Package, text: &str) -> ChipModel {
        ChipModel {
            family: family(Kind::Ind, case, text),
            value: henries,
            class2: false,
        }
    }

    pub fn kind(&self) -> Kind {
        self.family.kind
    }

    /// The inductor's winding capacitance, F.
    pub fn cp(&self) -> f64 {
        let f = self.family;
        f.cp10 * (self.value / 10e-9).max(1e-3).powf(f.cp_exp)
    }

    /// The inductor's DC resistance, Ω.
    pub fn rdc(&self) -> f64 {
        let f = self.family;
        f.rdc10 * (self.value / 10e-9).max(1e-3).powf(f.rdc_exp)
    }

    /// The capacitor's series inductance, H.
    pub fn esl(&self) -> f64 {
        match self.kind() {
            Kind::Cap => {
                // Class II parts are thicker stacks: a little more.
                if self.class2 {
                    self.family.esl * 1.1
                } else {
                    self.family.esl
                }
            }
            Kind::Ind => 0.0,
        }
    }

    /// Series resistance at `f` GHz, Ω: the capacitor's ESR, or the
    /// inductor's winding loss (before its capacitance across).
    pub fn esr(&self, f: f64) -> f64 {
        let f = f.max(1e-6);
        let fam = self.family;
        match self.kind() {
            Kind::Cap => {
                let w = TAU * f * 1e9;
                // Large values have more electrodes in parallel.
                let plates = (self.value / 10e-12).max(1.0).powf(0.2);
                let (r0, tan_d) = if self.class2 {
                    (crate::pdn::CapModel::mlcc(self.value, fam.case).esr, 0.02)
                } else {
                    (0.0, fam.tan_d)
                };
                r0 + fam.r1 * f.sqrt() / plates + tan_d / (w * self.value)
            }
            Kind::Ind => {
                // Chosen so that Q at f_ref is q_ref (ignoring Cp).
                let w = TAU * fam.f_ref * 1e9;
                let r_ref = (w * self.value / fam.q_ref).max(self.rdc());
                let rac = (r_ref - self.rdc()) / fam.f_ref.sqrt();
                self.rdc() + rac * f.sqrt()
            }
        }
    }

    /// Its impedance at `f` GHz.
    pub fn z(&self, f: f64) -> Cx {
        let w = TAU * f.max(1e-6) * 1e9;
        match self.kind() {
            Kind::Cap => Cx::new(self.esr(f), w * self.esl() - 1.0 / (w * self.value)),
            Kind::Ind => {
                let zs = Cx::new(self.esr(f), w * self.value);
                let y = Cx::new(1.0, 0.0).div(zs).add(Cx::new(0.0, w * self.cp()));
                Cx::new(1.0, 0.0).div(y)
            }
        }
    }

    /// Self-resonant frequency, GHz.
    pub fn srf(&self) -> f64 {
        let lc = match self.kind() {
            Kind::Cap => self.esl() * self.value,
            Kind::Ind => self.value * self.cp(),
        };
        1.0 / (TAU * lc.sqrt()) / 1e9
    }

    /// Q at `f` GHz: |X| / R of its impedance.
    pub fn q(&self, f: f64) -> f64 {
        let z = self.z(f);
        if z.re <= 0.0 {
            return f64::INFINITY;
        }
        z.im.abs() / z.re
    }

    /// The inductance (or, for a capacitor, capacitance) it looks like
    /// at `f` GHz, from its reactance: positive H for an inductive
    /// part, negative F (as -C) for a capacitive one.
    pub fn effective(&self, f: f64) -> (Kind, f64) {
        let w = TAU * f.max(1e-6) * 1e9;
        let x = self.z(f).im;
        if x >= 0.0 {
            (Kind::Ind, x / w)
        } else {
            (Kind::Cap, -1.0 / (w * x))
        }
    }

    /// "Murata LQW15AN (wire-wound), SRF 4.4 GHz, Q 42 at 1 GHz".
    pub fn describe(&self) -> String {
        let v = match self.kind() {
            Kind::Cap => crate::pdn::farads_text(self.value),
            Kind::Ind => format!("{} nH", trim(self.value * 1e9)),
        };
        let srf = self.srf();
        // Q where the part is still itself: 1 GHz, or half its SRF.
        let q_at = if srf < 2.0 { srf * 0.5 } else { 1.0 };
        let mut s = format!(
            "{v} {}{}, SRF {} GHz, Q {:.0} at {} GHz",
            self.family.name,
            match self.kind() {
                Kind::Cap if self.class2 => " (X7R/X5R)",
                Kind::Cap => " (C0G)",
                Kind::Ind => "",
            },
            trim(srf),
            self.q(q_at),
            trim(q_at)
        );
        match self.kind() {
            Kind::Cap => s.push_str(&format!(
                ", ESL {:.2} nH, ESR {:.2} Ω at 1 GHz",
                self.esl() * 1e9,
                self.esr(1.0)
            )),
            Kind::Ind => s.push_str(&format!(
                ", {:.2} pF across, DCR {:.2} Ω",
                self.cp() * 1e12,
                self.rdc()
            )),
        }
        s
    }

    /// A table of the model at the given frequencies, GHz.
    pub fn table(&self, freqs: &[f64]) -> String {
        let mut out = format!("{}\n", self.describe());
        out.push_str("     GHz      |Z| Ω     R Ω     X Ω       Q   looks like\n");
        for &f in freqs {
            let z = self.z(f);
            let (k, v) = self.effective(f);
            let like = match k {
                Kind::Ind => format!("{} nH", trim(v * 1e9)),
                Kind::Cap => format!("{} pF", trim(v * 1e12)),
            };
            out.push_str(&format!(
                "{:>8.3} {:>10.3} {:>7.3} {:>8.2} {:>7.1}   {like}\n",
                f,
                z.abs(),
                z.re,
                z.im,
                self.q(f)
            ));
        }
        out
    }
}

fn trim(v: f64) -> String {
    let s = if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else if v >= 0.1 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A note when a part resonates inside `fmin..fmax` GHz (or, for an
/// inductor, below the band: then it is a capacitor there).
pub fn srf_note(des: &str, m: &ChipModel, fmin: f64, fmax: f64) -> Option<String> {
    let srf = m.srf();
    match m.kind() {
        Kind::Ind if srf < fmax => Some(format!(
            "{des} {}: its SRF {} GHz is {} the band ({}-{} GHz); above it the inductor acts as a capacitor (looks like {} at {} GHz)",
            m.describe().split(',').next().unwrap_or(""),
            trim(srf),
            if srf < fmin { "below" } else { "inside" },
            trim(fmin),
            trim(fmax),
            match m.effective(fmax) {
                (Kind::Cap, c) => format!("{} pF", trim(c * 1e12)),
                (Kind::Ind, l) => format!("{} nH", trim(l * 1e9)),
            },
            trim(fmax)
        )),
        Kind::Cap if srf > fmin && srf < fmax && m.value < 1e-9 => Some(format!(
            "{des} {}: its SRF {} GHz is inside the band; above it the capacitor acts as an inductor",
            m.describe().split(',').next().unwrap_or(""),
            trim(srf)
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inductor_srf_matches_family() {
        let m = ChipModel::inductor(10e-9, P0402, "LQW15AN10NJ00D");
        assert!(m.family.name.contains("LQW15"));
        assert!((m.srf() - 4.4).abs() < 0.05, "{}", m.srf());
        // Q about 42 at 1 GHz.
        let q = m.q(1.0);
        assert!(q > 35.0 && q < 55.0, "{q}");
        // Above the SRF it is capacitive.
        assert_eq!(m.effective(6.0).0, Kind::Cap);
        assert_eq!(m.effective(1.0).0, Kind::Ind);
    }

    #[test]
    fn larger_inductor_resonates_lower() {
        let a = ChipModel::inductor(10e-9, P0402, "");
        let b = ChipModel::inductor(47e-9, P0402, "");
        assert!(b.srf() < a.srf() / 1.8);
        assert!(b.srf() > 1.4 && b.srf() < 2.2, "{}", b.srf());
        // Near its SRF it looks larger than its value.
        let (_, l) = b.effective(1.2);
        assert!(l > 60e-9, "{l}");
    }

    #[test]
    fn cap_resonates_by_esl() {
        let m = ChipModel::capacitor(100e-12, P0402, "100pF");
        assert!(m.family.name.contains("generic 0402 MLCC"));
        assert!(!m.class2);
        assert!(m.srf() > 0.8 && m.srf() < 1.2, "{}", m.srf());
        let hq = ChipModel::capacitor(10e-12, P0402, "GJM1555C1H100");
        assert!(hq.q(1.0) > 100.0, "{}", hq.q(1.0));
        let x = ChipModel::capacitor(100e-9, P0402, "100nF X7R");
        assert!(x.class2);
        assert!(x.esr(1.0) > m.esr(1.0) * 0.5);
    }

    #[test]
    fn notes_flag_inband_srf() {
        let l = ChipModel::inductor(47e-9, P0402, "");
        assert!(srf_note("L1", &l, 0.5, 6.0).is_some());
        let small = ChipModel::inductor(1e-9, P0402, "");
        assert!(srf_note("L2", &small, 0.5, 6.0).is_none());
    }

    #[test]
    fn family_by_name() {
        assert!(family_named(Kind::Ind, "0402HP").is_some());
        assert!(family_named(Kind::Ind, "lqg15").is_some());
        assert!(family_named(Kind::Cap, "gjm15").is_some());
    }
}
