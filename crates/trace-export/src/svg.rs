//! Draws a schematic sheet as SVG, for viewing in a browser or printing.

use std::fmt::Write;
use trace_core::{Rgb, SchGeom, Sheet};

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The sheet as an SVG document, 1 user unit = 1 mil.
pub fn sheet_svg(sheet: &Sheet) -> String {
    let d = &sheet.drawing;
    let [w, h] = d.size;
    let y = |v: f64| h - v;
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{}" height="{}" font-family="Times New Roman, serif">"#,
        w / 10.0,
        h / 10.0
    );
    let _ = writeln!(
        s,
        r#"<rect width="{w}" height="{h}" fill="{}"/>"#,
        hex(d.paper)
    );
    for im in &d.images {
        let mime = match im.data.get(..2) {
            Some(b"BM") => "image/bmp",
            Some([0x89, b'P']) => "image/png",
            _ => "image/jpeg",
        };
        let _ = writeln!(
            s,
            r#"<image x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" preserveAspectRatio="{}" href="data:{mime};base64,{}"/>"#,
            im.min[0],
            y(im.max[1]),
            im.max[0] - im.min[0],
            im.max[1] - im.min[1],
            if im.keep_aspect {
                "xMidYMid meet"
            } else {
                "none"
            },
            base64(&im.data)
        );
    }
    for sh in &d.shapes {
        let stroke = sh
            .stroke
            .map(|c| {
                format!(
                    r#" stroke="{}" stroke-width="{}"{}"#,
                    hex(c),
                    sh.width.max(4.0),
                    if sh.dashed {
                        r#" stroke-dasharray="40 25""#
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        let fill = sh.fill.map(hex).unwrap_or_else(|| "none".into());
        let pts = |v: &[[f64; 2]]| {
            v.iter()
                .map(|p| format!("{:.1},{:.1}", p[0], y(p[1])))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let _ = match &sh.geom {
            SchGeom::Line(v) => writeln!(
                s,
                r#"<polyline points="{}" fill="none"{stroke} stroke-linecap="round" stroke-linejoin="round"/>"#,
                pts(v)
            ),
            SchGeom::Poly(v) => {
                writeln!(s, r#"<polygon points="{}" fill="{fill}"{stroke}/>"#, pts(v))
            }
            SchGeom::Rect { min, max, r } => writeln!(
                s,
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" ry="{:.1}" fill="{fill}"{stroke}/>"#,
                min[0],
                y(max[1]),
                max[0] - min[0],
                max[1] - min[1],
                r[0],
                r[1]
            ),
            SchGeom::Arc { c, r, start, end } => {
                if end - start >= 359.99 {
                    writeln!(
                        s,
                        r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}" fill="{fill}"{stroke}/>"#,
                        c[0],
                        y(c[1]),
                        r[0],
                        r[1]
                    )
                } else {
                    let p = |a: f64| {
                        let a = a.to_radians();
                        (c[0] + r[0] * a.cos(), y(c[1] + r[1] * a.sin()))
                    };
                    let (a, b) = (p(*start), p(*end));
                    let large = if end - start > 180.0 { 1 } else { 0 };
                    writeln!(
                        s,
                        r#"<path d="M{:.1},{:.1} A{:.1},{:.1} 0 {large} 0 {:.1},{:.1}" fill="{fill}"{stroke}/>"#,
                        a.0, a.1, r[0], r[1], b.0, b.1
                    )
                }
            }
        };
    }
    for t in &d.texts {
        let anchor = ["start", "middle", "end"][(t.just % 3) as usize];
        let base = ["text-after-edge", "central", "text-before-edge"][(t.just / 3).min(2) as usize];
        let (x, yy) = (t.at[0], y(t.at[1]));
        let rot = if t.rot == 0 {
            String::new()
        } else {
            format!(
                r#" transform="rotate({} {x:.1} {yy:.1})""#,
                -90 * t.rot as i32
            )
        };
        let lines: Vec<&str> = t.text.split('\n').collect();
        let _ = write!(
            s,
            r#"<text x="{x:.1}" y="{yy:.1}" font-size="{:.0}" fill="{}" text-anchor="{anchor}" dominant-baseline="{base}"{}{}{rot}>"#,
            t.size,
            hex(t.color),
            if t.bold { r#" font-weight="bold""# } else { "" },
            if t.italic {
                r#" font-style="italic""#
            } else {
                ""
            },
        );
        if lines.len() == 1 {
            s.push_str(&esc(lines[0]));
        } else {
            for (i, l) in lines.iter().enumerate() {
                let _ = write!(
                    s,
                    r#"<tspan x="{x:.1}" dy="{}">{}</tspan>"#,
                    if i == 0 { 0.0 } else { t.size * 1.15 },
                    esc(l)
                );
            }
        }
        s.push_str("</text>\n");
    }
    s.push_str("</svg>\n");
    s
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
