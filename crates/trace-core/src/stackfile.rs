//! Stack-up files: a board's layers as text, for boards whose file carries
//! no cross-section (or a wrong one).
//!
//! One layer per line, top first, fields split by commas, tabs or `;`:
//!
//! ```text
//! # name, kind, thickness, Er, Df
//! L1,      copper,     2.15mil
//! Rogers,  dielectric, 8mil,  3.55, 0.0027
//! L2,      copper,     0.6mil
//! core,    dielectric, 4mil,  3.6,  0.0035
//! ```
//!
//! `kind` is copper (also conductor, plane, signal) or dielectric (also
//! core, prepreg). Thickness takes mil, mm, um, in or oz (copper); a bare
//! number is mil. Er and Df may be left out; copper may end with a
//! material name. Blank lines and lines from `#` are skipped.

use crate::{LayerKind, Length, StackLayer, Stackup};

pub fn parse(text: &str) -> Result<Stackup, String> {
    let mut st = Stackup::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line
            .split([',', '\t', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let err = |m: &str| format!("line {}: {m}: {line}", n + 1);
        if f.len() < 3 {
            return Err(err("needs name, kind and thickness"));
        }
        let kind = match f[1].to_ascii_lowercase().as_str() {
            "copper" | "conductor" | "plane" | "signal" | "cu" => LayerKind::Copper,
            "dielectric" | "core" | "prepreg" | "laminate" | "mask" => LayerKind::Dielectric,
            k => return Err(err(&format!("unknown kind {k}"))),
        };
        let thickness = thickness(f[2]).ok_or_else(|| err("not a thickness"))?;
        let num = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok());
        let (er, df) = match kind {
            LayerKind::Dielectric => (num(3), num(4)),
            _ => (None, None),
        };
        if let Some(e) = er {
            if !(1.0..=20.0).contains(&e) {
                return Err(err("Er out of range"));
            }
        }
        if let Some(d) = df {
            if !(0.0..1.0).contains(&d) {
                return Err(err("Df out of range"));
            }
        }
        let material = match kind {
            LayerKind::Dielectric => f.get(5),
            _ => f.get(3),
        }
        .map(|s| s.to_string());
        st.layers.push(StackLayer {
            name: f[0].to_string(),
            altium_id: None,
            kind,
            thickness,
            dielectric_constant: er,
            loss_tangent: df,
            roughness_um: None,
            material,
        });
    }
    if !st.layers.iter().any(|l| l.kind == LayerKind::Copper) {
        return Err("no copper layers".into());
    }
    Ok(st)
}

fn thickness(v: &str) -> Option<Length> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(oz) = v.strip_suffix("oz") {
        return oz
            .trim()
            .parse::<f64>()
            .ok()
            .map(|x| Length::from_mil(x * 1.37));
    }
    Length::parse(&v.replace(' ', ""))
}

/// Puts `file`'s stack-up on `board`. With as many copper layers as the
/// board, the board keeps its own copper names; otherwise it is refused.
pub fn apply(board: &mut crate::Board, file: &Stackup) -> Result<(), String> {
    apply_to(&mut board.stackup, file)
}

/// As `apply`, on a stack-up.
pub fn apply_to(current: &mut Stackup, file: &Stackup) -> Result<(), String> {
    let have: Vec<String> = current
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Copper)
        .map(|l| l.name.clone())
        .collect();
    let n = file
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Copper)
        .count();
    if !have.is_empty() && have.len() != n {
        return Err(format!(
            "the stack-up has {n} copper layers, the board {}",
            have.len()
        ));
    }
    let mut st = file.clone();
    let mut names = have.into_iter();
    for l in st.layers.iter_mut() {
        if l.kind == LayerKind::Copper {
            if let Some(name) = names.next() {
                l.name = name;
            }
        }
    }
    *current = st;
    Ok(())
}

/// The board's stack-up as a stack-up file.
pub fn write(st: &Stackup) -> String {
    let mut o = String::from("# name, kind, thickness, Er, Df, material\n");
    for l in &st.layers {
        match l.kind {
            LayerKind::Copper => {
                o += &format!("{}, copper, {:.3}mil", l.name, l.thickness.mil());
                if let Some(m) = &l.material {
                    o += &format!(", {m}");
                }
            }
            LayerKind::Dielectric => {
                o += &format!(
                    "{}, dielectric, {:.3}mil, {}, {}",
                    l.name,
                    l.thickness.mil(),
                    l.dielectric_constant
                        .map_or(String::new(), |e| e.to_string()),
                    l.loss_tangent.map_or(String::new(), |d| d.to_string())
                );
                if let Some(m) = &l.material {
                    o += &format!(", {m}");
                }
            }
            _ => continue,
        }
        o.push('\n');
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_writes_and_applies() {
        let st = parse(
            "# test\nTop, copper, 1oz\nRogers; dielectric; 8mil; 3.55; 0.0027\n\
             L2\tplane\t0.6\ncore, core, 0.1mm, 3.6\nBot, copper, 35um\n",
        )
        .unwrap();
        assert_eq!(st.layers.len(), 5);
        assert!((st.layers[0].thickness.mil() - 1.37).abs() < 1e-6);
        assert_eq!(st.layers[1].dielectric_constant, Some(3.55));
        assert_eq!(st.layers[1].loss_tangent, Some(0.0027));
        assert!((st.layers[3].thickness.mil() - 3.937).abs() < 1e-3);
        assert!((st.layers[4].thickness.mil() - 1.378).abs() < 1e-3);
        let again = parse(&write(&st)).unwrap();
        assert_eq!(again.layers.len(), 5);
        assert_eq!(again.layers[1].dielectric_constant, Some(3.55));
        let mut b = crate::Board::default();
        for n in ["TOP", "L2", "BOTTOM"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        apply(&mut b, &st).unwrap();
        assert_eq!(b.stackup.layers[2].name, "L2");
        assert_eq!(b.stackup.layers[4].name, "BOTTOM");
        assert!(parse("a, copper, x").is_err());
        let mut two = crate::Board::default();
        two.stackup.layers = b.stackup.layers[..1].to_vec();
        assert!(apply(&mut two, &st).is_err());
    }
}
