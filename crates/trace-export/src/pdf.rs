//! A small PDF writer: pages of lines, filled shapes and text in the
//! standard Helvetica fonts, enough for drawings and tables.

use flate2::{write::ZlibEncoder, Compression};
use std::fmt::Write as _;
use std::io::Write as _;

/// Helvetica advance widths (1/1000 em) for ASCII 32 to 126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// Width of `s` in Helvetica at size 1 (bold runs about 6 % wider).
pub fn text_width(s: &str, bold: bool) -> f64 {
    let w: f64 = encode(s)
        .iter()
        .map(|&b| match b {
            32..=126 => HELV[(b - 32) as usize] as f64,
            _ => 556.0,
        })
        .sum();
    w / 1000.0 * if bold { 1.06 } else { 1.0 }
}

/// Text as WinAnsi bytes: Latin-1 as it is, Ω as "Ohm", µ as µ, the
/// rest as '?'.
fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{3A9}' | '\u{2126}' => out.extend_from_slice(b"Ohm"),
            '–' => out.push(0x96),
            '—' => out.push(0x97),
            '·' => out.push(0xB7),
            c if (c as u32) < 0x20 => out.push(b' '),
            c if (c as u32) < 0x7F || (0xA0..=0xFF).contains(&(c as u32)) => out.push(c as u8),
            _ => out.push(b'?'),
        }
    }
    out
}

fn string(s: &str) -> String {
    let mut o = String::from("(");
    for b in encode(s) {
        match b {
            b'(' | b')' | b'\\' => {
                o.push('\\');
                o.push(b as char);
            }
            32..=126 => o.push(b as char),
            _ => {
                let _ = write!(o, "\\{b:03o}");
            }
        }
    }
    o.push(')');
    o
}

fn n(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

/// One page's drawing, in points from its bottom left corner.
pub struct Page {
    pub width: f64,
    pub height: f64,
    ops: String,
}

/// An RGB colour, 0 to 255.
pub type Rgb = [u8; 3];

fn rgb(c: Rgb) -> String {
    format!(
        "{} {} {}",
        n(c[0] as f64 / 255.0),
        n(c[1] as f64 / 255.0),
        n(c[2] as f64 / 255.0)
    )
}

impl Page {
    pub fn new(width: f64, height: f64) -> Page {
        Page {
            width,
            height,
            ops: String::from("1 J 1 j\n"),
        }
    }

    fn path(&mut self, pts: &[[f64; 2]], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            let _ = writeln!(
                self.ops,
                "{} {} {}",
                n(p[0]),
                n(p[1]),
                if i == 0 { "m" } else { "l" }
            );
        }
        if close {
            self.ops.push_str("h\n");
        }
    }

    /// An open line through `pts`.
    pub fn polyline(&mut self, pts: &[[f64; 2]], width: f64, color: Rgb) {
        if pts.len() < 2 {
            return;
        }
        let _ = writeln!(self.ops, "{} RG {} w", rgb(color), n(width));
        self.path(pts, false);
        self.ops.push_str("S\n");
    }

    /// A closed shape, filled and/or stroked.
    pub fn polygon(&mut self, pts: &[[f64; 2]], fill: Option<Rgb>, stroke: Option<(f64, Rgb)>) {
        if pts.len() < 3 || (fill.is_none() && stroke.is_none()) {
            return;
        }
        if let Some(f) = fill {
            let _ = writeln!(self.ops, "{} rg", rgb(f));
        }
        if let Some((w, c)) = stroke {
            let _ = writeln!(self.ops, "{} RG {} w", rgb(c), n(w));
        }
        self.path(pts, true);
        self.ops.push_str(match (fill, stroke) {
            (Some(_), Some(_)) => "B\n",
            (Some(_), None) => "f\n",
            _ => "S\n",
        });
    }

    /// A filled circle.
    pub fn dot(&mut self, c: [f64; 2], r: f64, color: Rgb) {
        let k = 0.5523 * r;
        let [x, y] = c;
        let _ = writeln!(
            self.ops,
            "{} rg {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f",
            rgb(color),
            n(x + r), n(y),
            n(x + r), n(y + k), n(x + k), n(y + r), n(x), n(y + r),
            n(x - k), n(y + r), n(x - r), n(y + k), n(x - r), n(y),
            n(x - r), n(y - k), n(x - k), n(y - r), n(x), n(y - r),
            n(x + k), n(y - r), n(x + r), n(y - k), n(x + r), n(y),
        );
    }

    /// Text with its baseline starting at `at`, turned `angle_deg`
    /// anticlockwise.
    pub fn text(
        &mut self,
        at: [f64; 2],
        size: f64,
        bold: bool,
        angle_deg: f64,
        color: Rgb,
        s: &str,
    ) {
        if s.is_empty() {
            return;
        }
        let (sn, cs) = angle_deg.to_radians().sin_cos();
        let _ = writeln!(
            self.ops,
            "BT {} rg /{} {} Tf {} {} {} {} {} {} Tm {} Tj ET",
            rgb(color),
            if bold { "F2" } else { "F1" },
            n(size),
            n(cs),
            n(sn),
            n(-sn),
            n(cs),
            n(at[0]),
            n(at[1]),
            string(s)
        );
    }

    /// Text centred on `c` (both ways), turned `angle_deg`.
    pub fn text_centered(
        &mut self,
        c: [f64; 2],
        size: f64,
        bold: bool,
        angle_deg: f64,
        color: Rgb,
        s: &str,
    ) {
        let w = text_width(s, bold) * size;
        let h = 0.72 * size;
        let (sn, cs) = angle_deg.to_radians().sin_cos();
        let (dx, dy) = (-w / 2.0, -h / 2.0);
        let at = [c[0] + dx * cs - dy * sn, c[1] + dx * sn + dy * cs];
        self.text(at, size, bold, angle_deg, color, s);
    }
}

/// The pages as a PDF document.
pub fn document(pages: &[Page], title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", offsets.len());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    // 1 catalog, 2 pages, 3 and 4 fonts, 5 info, then a page and its
    // content per page.
    let first = 6;
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", first + 2 * i))
        .collect();
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(
        &mut out,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    for f in ["Helvetica", "Helvetica-Bold"] {
        obj(
            &mut out,
            format!("<< /Type /Font /Subtype /Type1 /BaseFont /{f} /Encoding /WinAnsiEncoding >>")
                .as_bytes(),
        );
    }
    obj(
        &mut out,
        format!(
            "<< /Title {} /Producer (Trace {}) >>",
            string(title),
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    );
    for (i, p) in pages.iter().enumerate() {
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                n(p.width),
                n(p.height),
                first + 2 * i + 1
            )
            .as_bytes(),
        );
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        let _ = z.write_all(p.ops.as_bytes());
        let data = z.finish().unwrap_or_default();
        let mut body = format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            data.len()
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in &offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len() + 1
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_escaped_and_measured() {
        assert_eq!(string("a(b)\\"), "(a\\(b\\)\\\\)");
        assert_eq!(string("10kΩ µ"), "(10kOhm \\265)");
        assert!((text_width("0", false) - 0.556).abs() < 1e-9);
    }

    #[test]
    fn document_has_its_pages_and_a_good_xref() {
        let mut p = Page::new(595.0, 842.0);
        p.text([50.0, 50.0], 12.0, false, 0.0, [0, 0, 0], "Hello");
        let pdf = document(&[p, Page::new(595.0, 842.0)], "T");
        let find = |from: usize, pat: &[u8]| {
            pdf[from..]
                .windows(pat.len())
                .position(|w| w == pat)
                .map(|i| i + from)
        };
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(find(0, b"/Count 2").is_some());
        let sx = find(0, b"startxref\n").unwrap() + 10;
        let at: usize = String::from_utf8_lossy(&pdf[sx..])
            .lines()
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap();
        assert!(pdf[at..].starts_with(b"xref"));
        // Every object offset points at its object.
        let table = String::from_utf8_lossy(&pdf[at..]).into_owned();
        for (i, line) in table.lines().skip(3).take(9).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(
                pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()),
                "{i}"
            );
        }
    }
}
