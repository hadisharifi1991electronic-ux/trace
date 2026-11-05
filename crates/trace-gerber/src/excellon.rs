//! Excellon NC drill reader (the dialects written by Altium, KiCad, Eagle,
//! Allegro, PADS and others), including routed slots and X2 comments.

use crate::geom::Pt;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Hole {
    pub at: Pt,
    /// Other end of a slot.
    pub end: Option<Pt>,
    pub diameter: f64,
    /// `Some(true)` plated, `Some(false)` not plated, `None` unknown.
    pub plated: Option<bool>,
    /// X2 `.AperFunction` of the tool, such as `ViaDrill` or `ComponentDrill`.
    pub function: Option<String>,
}

#[derive(Debug, Default)]
pub struct Drill {
    pub holes: Vec<Hole>,
    /// X2 `.FileFunction`, such as `Plated,1,4,PTH`.
    pub file_function: Option<String>,
    pub plated: Option<bool>,
    /// First and last copper layer of a blind or buried span (1-based).
    pub span: Option<(u32, u32)>,
}

#[derive(Clone, Debug)]
struct Tool {
    diameter: f64,
    plated: Option<bool>,
    function: Option<String>,
}

/// True when the data looks like an Excellon drill file.
pub fn sniff(data: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&data[..data.len().min(4096)]);
    let head = head.trim_start();
    if head.contains("%FS") || head.contains("%MO") {
        return false;
    }
    head.starts_with("M48")
        || head.lines().take(40).any(|l| {
            let l = l.trim();
            l == "M48"
                || (l.starts_with(";T") && l.contains("Holesize"))
                || l.starts_with("METRIC")
                || l.starts_with("INCH")
                || (l.starts_with('T')
                    && l.contains('C')
                    && l[1..2].chars().all(|c| c.is_ascii_digit()))
        })
}

/// An Allegro tool comment: number, diameter in mm, plated.
fn allegro_tool(c: &str) -> Option<(u32, f64, bool)> {
    let rest = c.strip_prefix('T')?;
    if !rest.contains("Holesize") {
        return None;
    }
    let n: u32 = rest.split_whitespace().next()?.parse().ok()?;
    let after = rest.split_once('=')?.1.trim();
    let v: f64 = after.split_whitespace().next()?.parse().ok()?;
    let up = c.to_ascii_uppercase();
    let words: Vec<&str> = up.split_whitespace().collect();
    let unit = if words.contains(&"MM") {
        1.0
    } else if words.contains(&"INCH") || words.contains(&"INCHES") {
        25.4
    } else {
        0.0254
    };
    let plated = !(words.contains(&"NON_PLATED")
        || words.contains(&"NON-PLATED")
        || words.contains(&"NONPLATED"));
    Some((n, v * unit, plated))
}

pub fn parse(data: &[u8]) -> Result<Drill, String> {
    let text = String::from_utf8_lossy(data);
    let mut d = Drill::default();
    let mut tools: HashMap<u32, Tool> = HashMap::new();
    let mut unit = 25.4;
    let mut metric = false;
    // LZ: leading zeros present (trailing ones dropped); TZ the reverse.
    let mut keep_leading = false;
    let mut digits: Option<(usize, usize)> = None;
    let mut plated_section: Option<bool> = None;
    let mut pending_function: Option<String> = None;
    let mut tool: Option<u32> = None;
    let mut pos: Pt = [0.0, 0.0];
    let mut incremental = false;
    let mut route: Option<Vec<Pt>> = None;
    let mut tool_down = false;
    let mut in_header = false;

    let number = |s: &str,
                  metric: bool,
                  keep_leading: bool,
                  digits: Option<(usize, usize)>,
                  unit: f64|
     -> f64 {
        if s.contains('.') {
            return s.parse::<f64>().unwrap_or(0.0) * unit;
        }
        let (neg, s) = match s.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        let (int, dec) = digits.unwrap_or(if metric { (3, 3) } else { (2, 4) });
        let v = if keep_leading {
            let mut t = s.to_string();
            while t.len() < int + dec {
                t.push('0');
            }
            t.parse::<f64>().unwrap_or(0.0)
        } else {
            s.parse::<f64>().unwrap_or(0.0)
        } / 10f64.powi(dec as i32);
        if neg {
            -v * unit
        } else {
            v * unit
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix(';') {
            let c = c.trim();
            if let Some(attr) = c.strip_prefix("#@!") {
                let attr = attr.trim();
                if let Some(v) = attr.strip_prefix("TF.FileFunction,") {
                    d.file_function = Some(v.to_string());
                    let f: Vec<&str> = v.split(',').collect();
                    match f.first() {
                        Some(&"Plated") => d.plated = Some(true),
                        Some(&"NonPlated") => d.plated = Some(false),
                        _ => {}
                    }
                    if let (Some(a), Some(b)) = (
                        f.get(1).and_then(|s| s.parse().ok()),
                        f.get(2).and_then(|s| s.parse().ok()),
                    ) {
                        d.span = Some((a, b));
                    }
                } else if let Some(v) = attr.strip_prefix("TA.AperFunction,") {
                    pending_function = Some(v.to_string());
                    if v.starts_with("NonPlated") {
                        plated_section = Some(false);
                    } else if v.starts_with("Plated") {
                        plated_section = Some(true);
                    }
                }
            } else if let Some((n, dia, plated)) = allegro_tool(c) {
                // Allegro: ";T01 Holesize 1. = 8.000000 Tolerance = ... PLATED MILS"
                tools.insert(
                    n,
                    Tool {
                        diameter: dia,
                        plated: Some(plated),
                        function: None,
                    },
                );
            } else if let Some(t) = c.strip_prefix("TYPE=") {
                plated_section = Some(!t.starts_with("NON"));
            } else if let Some(f) = c.strip_prefix("FILE_FORMAT=") {
                let p: Vec<usize> = f.split(':').filter_map(|s| s.trim().parse().ok()).collect();
                if p.len() == 2 {
                    digits = Some((p[0], p[1]));
                }
            } else if c.contains("Holes") && c.contains("NPTH")
                || c.contains("NON_PLATED")
                || c.contains("Non-Plated")
            {
                plated_section = Some(false);
            }
            continue;
        }
        if line == "M48" {
            in_header = true;
            continue;
        }
        if in_header && (line == "%" || line == "M95") {
            in_header = false;
            continue;
        }
        if line.starts_with("METRIC") || line.starts_with("INCH") || line == "M71" || line == "M72"
        {
            metric = line.starts_with("METRIC") || line == "M71";
            unit = if metric { 1.0 } else { 25.4 };
            if line.contains(",LZ") {
                keep_leading = true;
            } else if line.contains(",TZ") {
                keep_leading = false;
            }
            // e.g. METRIC,LZ,000.000
            if let Some(fmt) = line
                .split(',')
                .find(|p| p.contains('.') && p.chars().all(|c| c == '0' || c == '.'))
            {
                let mut s = fmt.split('.');
                digits = Some((s.next().unwrap_or("").len(), s.next().unwrap_or("").len()));
            }
            continue;
        }
        if line == "M30" || line == "M00" {
            break;
        }
        match line {
            "G90" => {
                incremental = false;
                continue;
            }
            "G91" | "ICI,ON" => {
                incremental = true;
                continue;
            }
            "M15" => {
                tool_down = true;
                continue;
            }
            "M16" | "M17" => {
                tool_down = false;
                if let (Some(path), Some(t)) = (route.take(), tool.and_then(|t| tools.get(&t))) {
                    for s in path.windows(2) {
                        d.holes.push(Hole {
                            at: s[0],
                            end: Some(s[1]),
                            diameter: t.diameter,
                            plated: t.plated,
                            function: t.function.clone(),
                        });
                    }
                }
                continue;
            }
            _ => {}
        }
        if let Some(rest) = line.strip_prefix('T') {
            let n_end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let Ok(n) = rest[..n_end].parse::<u32>() else {
                continue;
            };
            let def = &rest[n_end..];
            if let Some(ci) = def.find('C') {
                let v: String = def[ci + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                let dia = if v.contains('.') {
                    v.parse::<f64>().unwrap_or(0.0) * unit
                } else {
                    number(&v, metric, keep_leading, digits, unit)
                };
                tools.insert(
                    n,
                    Tool {
                        diameter: dia,
                        plated: plated_section,
                        function: pending_function.take(),
                    },
                );
            }
            if !in_header {
                tool = Some(n);
            }
            continue;
        }
        if in_header {
            continue;
        }
        // "R03X000500": three more holes, each one step further on.
        if let Some(rest) = line.strip_prefix('R') {
            let n_end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if let (Ok(n), Some(t)) = (
                rest[..n_end].parse::<u32>(),
                tool.and_then(|t| tools.get(&t)).cloned(),
            ) {
                let mut step = [0.0, 0.0];
                let b = &rest.as_bytes()[n_end..];
                let mut i = 0;
                while i < b.len() {
                    let k = match b[i] {
                        b'X' => 0,
                        b'Y' => 1,
                        _ => {
                            i += 1;
                            continue;
                        }
                    };
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len()
                        && (b[j].is_ascii_digit() || b[j] == b'.' || b[j] == b'-' || b[j] == b'+')
                    {
                        j += 1;
                    }
                    step[k] = number(
                        &rest[n_end + start..n_end + j],
                        metric,
                        keep_leading,
                        digits,
                        unit,
                    );
                    i = j;
                }
                for _ in 0..n.min(100_000) {
                    pos = [pos[0] + step[0], pos[1] + step[1]];
                    d.holes.push(Hole {
                        at: pos,
                        end: None,
                        diameter: t.diameter,
                        plated: t.plated,
                        function: t.function.clone(),
                    });
                }
                continue;
            }
        }
        // Coordinates, possibly with G00/G01/G85.
        let mut slot_split = line.splitn(2, "G85");
        let first = slot_split.next().unwrap_or("");
        let second = slot_split.next();
        let parse_xy = |s: &str, pos: &mut Pt| -> bool {
            let mut found = false;
            let mut chars = s.char_indices().peekable();
            while let Some((i, c)) = chars.next() {
                if c == 'X' || c == 'Y' {
                    let start = i + 1;
                    let mut end = start;
                    while let Some(&(j, cc)) = chars.peek() {
                        if cc.is_ascii_digit() || cc == '.' || cc == '-' || cc == '+' {
                            end = j + 1;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let v = number(&s[start..end], metric, keep_leading, digits, unit);
                    let k = if c == 'X' { 0 } else { 1 };
                    pos[k] = if incremental { pos[k] + v } else { v };
                    found = true;
                }
            }
            found
        };
        let g = first.trim_start();
        let is_move = g.starts_with("G00");
        let is_draw = g.starts_with("G01") || g.starts_with("G02") || g.starts_with("G03");
        if !parse_xy(first, &mut pos) {
            continue;
        }
        let t = tool.and_then(|t| tools.get(&t)).cloned();
        let Some(t) = t else { continue };
        if is_move {
            route = Some(vec![pos]);
            continue;
        }
        if is_draw || (tool_down && route.is_some()) {
            if let Some(r) = &mut route {
                r.push(pos);
            }
            continue;
        }
        if let Some(sec) = second {
            let start = pos;
            parse_xy(sec, &mut pos);
            d.holes.push(Hole {
                at: start,
                end: Some(pos),
                diameter: t.diameter,
                plated: t.plated,
                function: t.function.clone(),
            });
            continue;
        }
        d.holes.push(Hole {
            at: pos,
            end: None,
            diameter: t.diameter,
            plated: t.plated,
            function: t.function.clone(),
        });
    }
    if d.plated.is_some() {
        for h in &mut d.holes {
            if h.plated.is_none() {
                h.plated = d.plated;
            }
        }
    }
    if d.holes.is_empty() && tools.is_empty() {
        return Err("no drill data found".into());
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_codes() {
        let f = b";T01 Holesize 1. = 8.000000 Tolerance = +0.000000/-0.000000 PLATED MILS Quantity = 3\n%\nG90\nT01\nX010000Y-020000\nR02X000500\nM30\n";
        let d = parse(f).unwrap();
        assert_eq!(d.holes.len(), 3);
        assert!(
            (d.holes[2].at[0] - 1.1 * 25.4).abs() < 1e-9
                && (d.holes[2].at[1] + 2.0 * 25.4).abs() < 1e-9
        );
        assert!((d.holes[0].diameter - 0.2032).abs() < 1e-9);
    }

    #[test]
    fn altium_style() {
        let f = b"M48\n;FILE_FORMAT=4:4\nMETRIC,LZ\n;TYPE=PLATED\nT1F00S00C0.2000\n;TYPE=NON_PLATED\nT5F00S00C0.8000\n%\nT01\nX0006625Y0003175\nX00061\nT05\nX0001Y0001\nM30\n";
        let d = parse(f).unwrap();
        assert_eq!(d.holes.len(), 3);
        assert!((d.holes[0].at[0] - 6.625).abs() < 1e-9);
        assert!((d.holes[1].at[0] - 6.1).abs() < 1e-9 && (d.holes[1].at[1] - 3.175).abs() < 1e-9);
        assert_eq!(d.holes[0].plated, Some(true));
        assert_eq!(d.holes[2].plated, Some(false));
        assert!((d.holes[2].diameter - 0.8).abs() < 1e-9);
    }

    #[test]
    fn kicad_style_with_slot() {
        let f = b"M48\n; #@! TF.FileFunction,Plated,1,2,PTH\nFMAT,2\nMETRIC\nT1C0.300\n%\nG90\nG05\nT1\nX10.0Y-5.0\nX1.0Y1.0G85X2.0Y1.0\nM30\n";
        let d = parse(f).unwrap();
        assert_eq!(d.holes.len(), 2);
        assert_eq!(d.holes[0].at, [10.0, -5.0]);
        assert_eq!(d.holes[1].end, Some([2.0, 1.0]));
        assert_eq!(d.holes[0].plated, Some(true));
    }
}
