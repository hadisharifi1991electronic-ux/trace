//! IPC-D-356 netlists, which most CAD tools write next to the Gerbers for
//! bare-board electrical test. Each record is a test point: a pad or via
//! with its net, part and pin, so the netlist names the traced nets and
//! tells which pads belong to which part.

use crate::geom::Pt;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct TestPoint {
    pub net: String,
    /// Part designator, or empty for vias and tooling holes.
    pub refdes: String,
    pub pin: String,
    pub at: Pt,
    /// Hole diameter (mm) for drilled points.
    pub hole: Option<f64>,
    /// 0 = reachable from both sides (through hole), 1 = top, n = layer n.
    pub access: u32,
    pub via: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Netlist {
    pub points: Vec<TestPoint>,
    /// Number of copper layers when the file says it.
    pub layers: Option<u32>,
}

/// True when the data looks like an IPC-D-356 netlist.
pub fn sniff(data: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&data[..data.len().min(8192)]);
    let mut p = 0;
    let mut rec = 0;
    for l in head.lines().take(200) {
        if l.starts_with("P  JOB") || l.starts_with("P  UNITS") || l.starts_with("P  CODE") {
            p += 1;
        }
        if (l.starts_with("317") || l.starts_with("327") || l.starts_with("367")) && l.len() > 40 {
            rec += 1;
        }
    }
    p >= 1 || rec >= 3
}

/// Parses an IPC-D-356 or 356A file.
pub fn parse(data: &[u8]) -> Netlist {
    let text = String::from_utf8_lossy(data);
    // CUST 0 and 2: inches in 0.0001; CUST 1 and SI: mm in 0.001.
    let mut scale = 0.00254;
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut out = Netlist::default();
    let mut raw: Vec<TestPoint> = Vec::new();
    for line in text.lines() {
        if line.len() < 3 || !line.is_ascii() {
            continue;
        }
        let code = &line[..3];
        if code == "P  " || code == "C  " {
            let rest = line[3..].trim();
            if let Some(u) = rest.strip_prefix("UNITS") {
                let u = u.trim().to_ascii_uppercase();
                scale = if u.starts_with("CUST 1") || u.starts_with("SI") {
                    0.001
                } else {
                    0.00254
                };
            } else if let Some(n) = rest.strip_prefix("NNAME") {
                // "NNAME12 FULL_NET_NAME": long names behind a short alias.
                let mut it = n.splitn(2, char::is_whitespace);
                let alias = it.next().unwrap_or("").trim();
                let full = it.next().unwrap_or("").trim();
                if !full.is_empty() {
                    aliases.insert(format!("NNAME{alias}"), full.to_string());
                    aliases.insert(alias.to_string(), full.to_string());
                }
            } else if let Some(n) = rest.strip_prefix("Number of etch Layers:") {
                out.layers = n.trim().parse().ok();
            }
            continue;
        }
        if !matches!(code, "317" | "327" | "367") || line.len() < 50 {
            continue;
        }
        let field = |a: usize, b: usize| {
            line.get(a..b.min(line.len()))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let net = field(3, 17);
        let refdes = field(20, 26);
        let pin = field(27, 31);
        // The rest is a run of tagged numbers: D0080 P A00 X+008000 Y-056500 X0160 Y0160 R090 S3.
        let tail = line.get(31..).unwrap_or("");
        let mut hole = None;
        let mut access = 0;
        let mut xy: Vec<f64> = Vec::new();
        let b = tail.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i] as char;
            let start = i + 1;
            let mut j = start;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            } else if matches!(c, 'X' | 'Y')
                && xy.len() < 2
                && j + 1 < b.len()
                && b[j] == b' '
                && b[j + 1].is_ascii_digit()
            {
                // Altium writes a space for the plus sign: "X 075373Y 046151".
                j += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let num = tail[start..j].trim_start();
            match c {
                'D' if !num.is_empty() && hole.is_none() && xy.is_empty() => {
                    hole = num.parse::<f64>().ok()
                }
                'A' if !num.is_empty() && xy.is_empty() => access = num.parse().unwrap_or(0),
                'X' | 'Y' if !num.is_empty() && xy.len() < 2 => xy.push(num.parse().unwrap_or(0.0)),
                _ => {}
            }
            i = if j > start { j } else { i + 1 };
        }
        if xy.len() < 2 {
            continue;
        }
        let net = aliases.get(&net).cloned().unwrap_or(net);
        raw.push(TestPoint {
            via: refdes.eq_ignore_ascii_case("VIA") || code == "317" && refdes.is_empty(),
            net,
            refdes,
            pin,
            at: [xy[0] * scale, xy[1] * scale],
            hole: hole.map(|h| h * scale),
            access,
        });
    }
    out.points = raw;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_allegro_records() {
        let f = b"P  UNITS CUST 0\n\
C  NNAMEm0001 UNNAMED_7_CAP_I328_N2\n\
327m0001            R903  -2          A01X-007060Y+023000X0220Y0240     S1      \n\
317GND              VIA   -    MD0080PA00X+008000Y-056500X0160Y0160R090 S3      \n\
999\n";
        let n = parse(f);
        assert_eq!(n.points.len(), 2);
        let r = &n.points[0];
        assert_eq!(
            (r.net.as_str(), r.refdes.as_str(), r.pin.as_str(), r.access),
            ("UNNAMED_7_CAP_I328_N2", "R903", "2", 1)
        );
        assert!((r.at[0] + 17.9324).abs() < 1e-6 && (r.at[1] - 58.42).abs() < 1e-6);
        let v = &n.points[1];
        assert!(v.via && v.net == "GND" && (v.hole.unwrap() - 0.2032).abs() < 1e-9);
        assert!((v.at[0] - 20.32).abs() < 1e-9 && (v.at[1] + 143.51).abs() < 1e-9);
    }

    #[test]
    fn reads_altium_records_with_a_space_for_plus() {
        let f = b"P  UNITS CUST 1\n\
327N/C              T5    -4         UA01X 075373Y 046151X0890Y0790R180 S0\n\
999\n";
        let n = parse(f);
        assert_eq!(n.points.len(), 1);
        let p = &n.points[0];
        assert!((p.at[0] - 75.373).abs() < 1e-9 && (p.at[1] - 46.151).abs() < 1e-9);
    }
}
