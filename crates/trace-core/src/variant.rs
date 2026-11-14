//! Assembly variants, as Altium's project variants and Allegro's variant
//! editor keep them: one board, built several ways. Each variant names
//! the parts left off (not fitted) and the parameters changed on others
//! (an alternate value or part number). Outputs for a variant leave the
//! parts off the BOM, the pick and place file and the 3D model, and mark
//! them on the assembly drawing.

use crate::{Board, Component};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Designators of the parts left off.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub not_fitted: BTreeSet<String>,
    /// Parameters changed, by designator: name -> value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, BTreeMap<String, String>>,
}

impl Variant {
    pub fn new(name: &str) -> Variant {
        Variant {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn fitted(&self, designator: &str) -> bool {
        !self.not_fitted.contains(designator)
    }

    /// The part's parameters in this variant. A value written `=Name`
    /// takes the parameter Name's value, as Altium's do.
    pub fn params_of(&self, c: &Component) -> BTreeMap<String, String> {
        let mut p = c.params.clone();
        if let Some(ch) = self.params.get(&c.designator) {
            for (k, v) in ch {
                // Change the existing key whatever its case.
                let key = p
                    .keys()
                    .find(|x| x.eq_ignore_ascii_case(k))
                    .cloned()
                    .unwrap_or_else(|| k.clone());
                p.insert(key, v.clone());
            }
        }
        let refs: Vec<(String, String)> = p
            .iter()
            .filter_map(|(k, v)| {
                let name = v.strip_prefix('=')?;
                let val = p
                    .iter()
                    .find(|(x, _)| x.eq_ignore_ascii_case(name))
                    .map(|(_, y)| y.clone())?;
                (!val.starts_with('=')).then(|| (k.clone(), val))
            })
            .collect();
        p.extend(refs);
        p
    }
}

/// The variant outputs are made for, if one is chosen.
pub fn active(board: &Board) -> Option<&Variant> {
    board.variant.and_then(|i| board.variants.get(i))
}

/// Picks the variant for outputs by name (case ignored); `None` or an
/// empty name is the board as designed.
pub fn choose(board: &mut Board, name: Option<&str>) -> Result<(), String> {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => {
            board.variant = None;
            Ok(())
        }
        Some(n) => {
            let i = board
                .variants
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(n))
                .ok_or_else(|| {
                    let names: Vec<&str> = board.variants.iter().map(|v| v.name.as_str()).collect();
                    if names.is_empty() {
                        format!("the board has no variant {n} (it has no variants)")
                    } else {
                        format!("the board has no variant {n} (it has {})", names.join(", "))
                    }
                })?;
            board.variant = Some(i);
            Ok(())
        }
    }
}

/// Whether part `index` is fitted in the active variant.
pub fn fitted(board: &Board, index: usize) -> bool {
    match (active(board), board.components.get(index)) {
        (Some(v), Some(c)) => v.fitted(&c.designator),
        _ => true,
    }
}

/// The parts left off in the active variant.
pub fn not_fitted(board: &Board) -> HashSet<usize> {
    let Some(v) = active(board) else {
        return HashSet::new();
    };
    board
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| !v.fitted(&c.designator))
        .map(|(i, _)| i)
        .collect()
}

/// The board's parts with the active variant's parameters.
pub fn parts(board: &Board) -> Vec<Component> {
    match active(board) {
        None => board.components.clone(),
        Some(v) => board
            .components
            .iter()
            .map(|c| Component {
                params: v.params_of(c),
                ..c.clone()
            })
            .collect(),
    }
}

/// The active variant's name, for titles: "" for the board as designed.
pub fn name(board: &Board) -> &str {
    active(board).map_or("", |v| v.name.as_str())
}

/// Designators renamed (annotation, Update PCB): variants follow.
pub fn rename(board: &mut Board, renames: &BTreeMap<String, String>) {
    for v in &mut board.variants {
        v.not_fitted = v
            .not_fitted
            .iter()
            .map(|d| renames.get(d).cloned().unwrap_or_else(|| d.clone()))
            .collect();
        v.params = std::mem::take(&mut v.params)
            .into_iter()
            .map(|(d, p)| (renames.get(&d).cloned().unwrap_or(d), p))
            .collect();
    }
}

/// Variants from an Altium project file (`[ProjectVariantN]` sections).
pub fn from_prjpcb(text: &str) -> Vec<Variant> {
    let mut out: Vec<Variant> = Vec::new();
    let mut cur: Option<Variant> = None;
    // ParamVariationN and ParamDesignatorN come in pairs.
    let mut pv: BTreeMap<u32, (String, String)> = BTreeMap::new();
    let mut pd: BTreeMap<u32, String> = BTreeMap::new();
    let flush = |cur: &mut Option<Variant>,
                 pv: &mut BTreeMap<u32, (String, String)>,
                 pd: &mut BTreeMap<u32, String>,
                 out: &mut Vec<Variant>| {
        if let Some(mut v) = cur.take() {
            for (k, (name, value)) in std::mem::take(pv) {
                if let Some(d) = pd.get(&k) {
                    v.params.entry(d.clone()).or_default().insert(name, value);
                }
            }
            pd.clear();
            out.push(v);
        }
    };
    let fields = |s: &str| -> BTreeMap<String, String> {
        s.split('|')
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                Some((k.trim().to_string(), v.to_string()))
            })
            .collect()
    };
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') {
            flush(&mut cur, &mut pv, &mut pd, &mut out);
            if line.to_ascii_lowercase().starts_with("[projectvariant") {
                cur = Some(Variant::default());
            }
            continue;
        }
        let Some(v) = cur.as_mut() else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let num =
            |prefix: &str| -> Option<u32> { key.strip_prefix(prefix).and_then(|n| n.parse().ok()) };
        if key == "Description" {
            v.name = value.trim().to_string();
        } else if let Some(_n) = num("Variation") {
            let f = fields(value);
            let d = f.get("Designator").cloned().unwrap_or_default();
            // Kind 1: not fitted; 2: an alternate part (its parameters
            // follow as ParamVariation lines).
            if !d.is_empty() && f.get("Kind").map(String::as_str) == Some("1") {
                v.not_fitted.insert(d);
            }
        } else if let Some(n) = num("ParamVariation") {
            let f = fields(value);
            if let (Some(name), Some(val)) = (f.get("ParameterName"), f.get("VariantValue")) {
                pv.insert(n, (name.clone(), val.clone()));
            }
        } else if let Some(n) = num("ParamDesignator") {
            pd.insert(n, value.trim().to_string());
        }
    }
    flush(&mut cur, &mut pv, &mut pd, &mut out);
    for (i, v) in out.iter_mut().enumerate() {
        if v.name.is_empty() {
            v.name = format!("Variant {}", i + 1);
        }
    }
    out
}

/// Variants as a short report: name, parts left off, parts changed.
pub fn summary(board: &Board) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    if board.variants.is_empty() {
        s.push_str("No assembly variants.\n");
        return s;
    }
    for (i, v) in board.variants.iter().enumerate() {
        let _ = writeln!(
            s,
            "{}{}: {} parts not fitted, {} parts changed",
            v.name,
            if board.variant == Some(i) {
                " (active)"
            } else {
                ""
            },
            v.not_fitted.len(),
            v.params.len()
        );
        if !v.not_fitted.is_empty() {
            let mut d: Vec<&String> = v.not_fitted.iter().collect();
            d.sort_by(|a, b| natural(a, b));
            let list: Vec<&str> = d.iter().map(|s| s.as_str()).collect();
            let _ = writeln!(s, "  not fitted: {}", list.join(" "));
        }
        for (d, p) in &v.params {
            let value = p
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("Value"))
                .or_else(|| p.iter().find(|(k, _)| k.eq_ignore_ascii_case("Comment")))
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            let _ = writeln!(
                s,
                "  {d}: {} parameters changed{}",
                p.len(),
                if value.is_empty() {
                    String::new()
                } else {
                    format!(", value {value}")
                }
            );
        }
    }
    s
}

/// Orders designators as people read them: R2 before R10.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let mut out: Vec<(String, u64)> = Vec::new();
        let (mut text, mut digits) = (String::new(), String::new());
        for ch in s.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                if !digits.is_empty() {
                    out.push((std::mem::take(&mut text), digits.parse().unwrap_or(0)));
                    digits.clear();
                }
                text.push(ch);
            }
        }
        out.push((text, digits.parse().unwrap_or(0)));
        out
    };
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Length, Side};

    const PRJ: &str = "[Document1]\r\nDocumentPath=a.PcbDoc\r\n\r\n[ProjectVariant1]\r\nUniqueId=X\r\nDescription=Std\r\nVariationCount=0\r\n\r\n[ProjectVariant2]\r\nDescription=Low cost\r\nVariationCount=2\r\nVariation1=Designator=C5|UniqueId=\\AB|Kind=1|AlternatePart=\r\nVariation2=Designator=R22|UniqueId=\\CD|Kind=2|AlternatePart==Value\r\nParamVariation1=ParameterName=Comment|VariantValue==Value\r\nParamDesignator1=R22\r\nParamVariation2=ParameterName=Value|VariantValue=120\r\nParamDesignator2=R22\r\n\r\n[Configuration1]\r\nName=x\r\n";

    fn part(d: &str) -> Component {
        Component {
            designator: d.into(),
            footprint: "0402".into(),
            library_ref: None,
            x: Length(0),
            y: Length(0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: [
                ("Value".to_string(), "100".to_string()),
                ("Comment".to_string(), "100".to_string()),
            ]
            .into(),
        }
    }

    #[test]
    fn reads_altium_variants_and_applies_them() {
        let v = from_prjpcb(PRJ);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Std");
        assert!(v[0].not_fitted.is_empty());
        assert_eq!(v[1].name, "Low cost");
        assert!(v[1].not_fitted.contains("C5"));
        assert_eq!(v[1].params["R22"]["Value"], "120");
        let mut b = Board {
            components: vec![part("R22"), part("C5"), part("R1")],
            variants: v,
            ..Default::default()
        };
        assert!(fitted(&b, 1));
        choose(&mut b, Some("low COST")).unwrap();
        assert!(!fitted(&b, 1));
        assert_eq!(not_fitted(&b), [1].into());
        let p = parts(&b);
        assert_eq!(p[0].params["Value"], "120");
        assert_eq!(p[0].params["Comment"], "120");
        assert_eq!(p[2].params["Value"], "100");
        assert!(choose(&mut b, Some("nope")).is_err());
        let mut r = BTreeMap::new();
        r.insert("C5".to_string(), "C7".to_string());
        rename(&mut b, &r);
        assert!(b.variants[1].not_fitted.contains("C7"));
        assert!(summary(&b).contains("Low cost (active): 1 parts not fitted, 1 parts changed"));
    }
}
