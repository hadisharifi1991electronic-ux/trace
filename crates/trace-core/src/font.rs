//! A stroke font for silkscreen and copper text, so text from a design can
//! be drawn and plotted as lines.

use crate::{LayerRef, Length, Point, Track};

#[path = "font_data.rs"]
mod data;

/// Glyph strokes in font units (cap height 21, y down, baseline at 9).
fn glyph(ch: char) -> &'static str {
    let c = ch as u32;
    let i = if (0x20..0x100).contains(&c) {
        c - 0x20
    } else {
        b'?' as u32 - 0x20
    };
    data::GLYPHS[i as usize]
}

/// The strokes of a line of text, in units of the cap height: baseline at
/// y = 0, y up, starting at x = 0. Returns the polylines and the advance.
pub fn strokes(text: &str) -> (Vec<Vec<[f64; 2]>>, f64) {
    let mut out = Vec::new();
    let mut x = 0.0;
    for ch in text.chars() {
        let g = glyph(ch).as_bytes();
        if g.len() < 2 {
            continue;
        }
        let v = |b: u8| b as f64 - b'R' as f64;
        let (l, r) = (v(g[0]), v(g[1]));
        let mut line: Vec<[f64; 2]> = Vec::new();
        for pair in g[2..].chunks(2) {
            if pair.len() < 2 {
                break;
            }
            if pair == b" R" {
                if line.len() > 1 {
                    out.push(std::mem::take(&mut line));
                }
                line.clear();
                continue;
            }
            line.push([x + (v(pair[0]) - l) / 21.0, (9.0 - v(pair[1])) / 21.0]);
        }
        if line.len() > 1 {
            out.push(line);
        }
        x += (r - l) / 21.0;
    }
    (out, x)
}

/// How a piece of text sits on the board.
#[derive(Clone, Debug)]
pub struct Placing {
    /// Start of the baseline, before rotation.
    pub at: Point,
    /// Cap height of the stroke centre lines.
    pub height: Length,
    /// Stroke width.
    pub width: Length,
    pub rotation_deg: f64,
    /// Read from the other side: mirrored left to right.
    pub mirror: bool,
    /// Stretch or squeeze the text to this length (stroke centres), so it
    /// takes the room the source tool gave it.
    pub length: Option<Length>,
}

/// Text as tracks on `layer`.
pub fn text_tracks(text: &str, p: &Placing, layer: LayerRef, component: Option<u32>) -> Vec<Track> {
    let (lines, _) = strokes(text);
    let (lo, hi) = lines
        .iter()
        .flatten()
        .fold((f64::MAX, f64::MIN), |a, q| (a.0.min(q[0]), a.1.max(q[0])));
    if lo > hi {
        return Vec::new();
    }
    let h = p.height.0 as f64;
    let sx = match p.length {
        Some(len) if hi - lo > 1e-9 && len.0 > 0 => {
            (len.0 as f64 / ((hi - lo) * h)).clamp(0.4, 2.5)
        }
        _ => 1.0,
    };
    let (s, c) = p.rotation_deg.to_radians().sin_cos();
    let place = |q: [f64; 2]| {
        let mut x = (q[0] - lo) * h * sx;
        let y = q[1] * h;
        if p.mirror {
            x = -x;
        }
        Point {
            x: Length(p.at.x.0 + (x * c - y * s).round() as i64),
            y: Length(p.at.y.0 + (x * s + y * c).round() as i64),
        }
    };
    let mut out = Vec::new();
    for l in &lines {
        for w in l.windows(2) {
            let (a, b) = (place(w[0]), place(w[1]));
            if a == b {
                continue;
            }
            out.push(Track {
                layer: layer.clone(),
                net: None,
                start: a,
                end: b,
                width: p.width,
                component,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_have_strokes_and_advance() {
        let (l, adv) = strokes("A");
        // Two legs and the bar.
        assert_eq!(l.len(), 2);
        assert!(adv > 0.5 && adv < 1.0);
        let top = l.iter().flatten().map(|q| q[1]).fold(f64::MIN, f64::max);
        assert!((top - 1.0).abs() < 1e-9);
        let (_, adv2) = strokes("AA");
        assert!((adv2 - 2.0 * adv).abs() < 1e-9);
    }

    #[test]
    fn text_is_turned_mirrored_and_fitted() {
        let p = Placing {
            at: Point::default(),
            height: Length::from_mil(25.0),
            width: Length::from_mil(5.0),
            rotation_deg: 90.0,
            mirror: false,
            length: Some(Length::from_mil(60.0)),
        };
        let t = text_tracks("J13", &p, LayerRef::TopSilk, Some(3));
        assert!(!t.is_empty() && t.iter().all(|t| t.component == Some(3)));
        let ys = t.iter().flat_map(|t| [t.start.y.mil(), t.end.y.mil()]);
        let (lo, hi) = ys.fold((f64::MAX, f64::MIN), |a, y| (a.0.min(y), a.1.max(y)));
        // Turned 90°: the text runs up the y axis, 60 mil long.
        assert!(lo.abs() < 0.01 && (hi - 60.0).abs() < 0.01, "{lo} {hi}");
        let xs = t.iter().flat_map(|t| [t.start.x.mil(), t.end.x.mil()]);
        let (xl, xh) = xs.fold((f64::MAX, f64::MIN), |a, x| (a.0.min(x), a.1.max(x)));
        assert!(xh <= 0.01 && xl > -25.01, "{xl} {xh}");
        let m = text_tracks(
            "J13",
            &Placing {
                mirror: true,
                rotation_deg: 0.0,
                ..p
            },
            LayerRef::BottomSilk,
            None,
        );
        assert!(m.iter().all(|t| t.start.x.0 <= 0 && t.end.x.0 <= 0));
    }
}
