use crate::Length;
use serde::{Deserialize, Serialize};

/// A point in board coordinates (y grows upward, as in fabrication data).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: Length,
    pub y: Length,
}

/// Which layer a primitive sits on. Copper layers are numbered from the top
/// (0) down; everything else is named.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LayerRef {
    Copper(u8),
    TopSilk,
    BottomSilk,
    /// Through-hole items present on every copper layer.
    AllCopper,
    Keepout,
    Mechanical(u8),
    Other(u8),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub layer: LayerRef,
    pub net: Option<u32>,
    pub start: Point,
    pub end: Point,
    pub width: Length,
    /// The part it belongs to (silk outline, part copper), which moves with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arc {
    pub layer: LayerRef,
    pub net: Option<u32>,
    pub center: Point,
    pub radius: Length,
    pub start_deg: f64,
    pub end_deg: f64,
    pub width: Length,
    /// The part it belongs to, as for tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Via {
    pub net: Option<u32>,
    /// First and last copper layer the via spans (0 = top).
    pub from_layer: u8,
    pub to_layer: u8,
    pub at: Point,
    pub diameter: Length,
    pub hole: Length,
    /// Pad diameter on each copper layer of the span when it differs from
    /// layer to layer; a diameter equal to the hole means no pad there.
    /// Empty when the via has `diameter` on every layer it spans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rings: Vec<(u8, Length)>,
    /// Solder mask and tenting, where the via sets its own.
    #[serde(default, skip_serializing_if = "Fab::is_default")]
    pub fab: Fab,
}

/// What a pad or via says for manufacture where it does not leave it to
/// the board's rules: its own mask and paste expansion, tenting, a
/// non-plated hole, a slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fab {
    /// Solder mask expansion of its own; `None` takes the board's rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Length>,
    /// Paste expansion of its own (negative shrinks); `None` takes the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste: Option<Length>,
    /// Covered by solder mask on the top or bottom side: no opening there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tent_top: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tent_bottom: bool,
    /// No paste at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_paste: bool,
    /// The hole is not plated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unplated: bool,
    /// Slot length, end to end, for a slotted hole; zero for a round hole.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slot: Length,
    /// Direction of the slot, degrees counter-clockwise.
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub slot_deg: f64,
}

fn is_zero(l: &Length) -> bool {
    l.0 == 0
}

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}

impl Fab {
    pub fn is_default(&self) -> bool {
        *self == Fab::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadShape {
    Round,
    Rect,
    Octagon,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pad {
    pub layer: LayerRef,
    pub net: Option<u32>,
    pub component: Option<u32>,
    pub name: String,
    pub at: Point,
    pub size_x: Length,
    pub size_y: Length,
    pub hole: Length,
    pub shape: PadShape,
    pub rotation_deg: f64,
    /// Exact copper outline (board coordinates) when the pad is not a plain
    /// circle, oval or rectangle, such as a solder-jumper half or a custom
    /// pad. Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<Point>,
    /// Through-hole pads whose copper differs from layer to layer: the land
    /// on each copper layer that has one. A copper layer missing from the
    /// list has only the plated hole there. Empty when the pad has the same
    /// shape on every layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<Land>,
    /// Mask, paste and hole details of its own.
    #[serde(default, skip_serializing_if = "Fab::is_default")]
    pub fab: Fab,
}

/// The copper of a through-hole pad on one layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Land {
    pub layer: u8,
    pub shape: PadShape,
    pub size_x: Length,
    pub size_y: Length,
    pub rotation_deg: f64,
    /// Exact outline in board coordinates for irregular lands; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<Point>,
}

/// A filled copper area (pour, plane or fill) with its cut-outs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Polygon {
    pub layer: LayerRef,
    pub net: Option<u32>,
    pub outline: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
    /// The pour (`Geometry::pours`) this area was poured from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pour: Option<u32>,
}

/// A copper pour as drawn: the area it may fill, its net and layer. Its
/// poured copper is in `Geometry::polygons`, tagged with the pour's index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pour {
    pub layer: LayerRef,
    pub net: Option<u32>,
    pub outline: Vec<Point>,
    /// Islands not touching copper of the pour's net are left out.
    #[serde(default)]
    pub remove_dead: bool,
    /// Pours are filled in this order; a later pour keeps clear of earlier ones.
    #[serde(default)]
    pub order: i32,
}

/// Physical content of a board.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Geometry {
    /// Board outline as a closed polygon.
    pub outline: Vec<Point>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub vias: Vec<Via>,
    pub pads: Vec<Pad>,
    #[serde(default)]
    pub polygons: Vec<Polygon>,
    /// Filled shapes on layers other than copper: solder mask and paste
    /// openings, silkscreen and mechanical areas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<Polygon>,
    /// Pour outlines, for pouring again after edits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pours: Vec<Pour>,
    /// True when pads and vias carry their copper layer by layer (`stack`,
    /// `rings`), as boards rebuilt from fabrication data do: a pad or via
    /// without that list then has the same copper on every layer it spans.
    /// False when the source did not say, and unused inner pads may be gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub per_layer_copper: bool,
    /// 3D models of parts as the source stores them (STEP text). Not saved
    /// in Trace boards.
    #[serde(skip)]
    pub models: Vec<Model3d>,
    /// Where each part's 3D model sits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bodies: Vec<Body3d>,
}

/// A part's 3D model file.
#[derive(Clone, Debug)]
pub struct Model3d {
    pub name: String,
    /// STEP file contents.
    pub step: std::sync::Arc<Vec<u8>>,
}

/// One placement of a 3D model on the board.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Body3d {
    pub component: Option<u32>,
    /// Index into `Geometry::models`.
    pub model: u32,
    /// Model origin on the board.
    pub at: Point,
    /// Rotation about the board normal, degrees counter-clockwise.
    pub rotation_deg: f64,
    /// The model's own rotation about its x, y and z axes, degrees, applied
    /// in that order before it is placed.
    pub model_rotation_deg: [f64; 3],
    /// Height of the model origin above the board surface.
    pub standoff: Length,
    /// On the bottom side, seen through the board.
    pub bottom: bool,
    pub opacity: f32,
}

fn ring_area(v: &[[f64; 2]]) -> f64 {
    let n = v.len();
    (0..n)
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Points in mm, closing point dropped, turned counter-clockwise.
fn ccw(pts: &[Point]) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = pts.iter().map(|p| [p.x.mm(), p.y.mm()]).collect();
    if v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    if ring_area(&v) < 0.0 {
        v.reverse();
    }
    v
}

/// One outline with its holes joined in through zero-width cuts, the form
/// KiCad keeps zone fills in.
pub fn fracture(outline: &[Point], holes: &[Vec<Point>]) -> Vec<[f64; 2]> {
    let mut out = ccw(outline);
    let mut hs: Vec<Vec<[f64; 2]>> = holes
        .iter()
        .filter(|h| h.len() >= 3)
        .map(|h| {
            let mut v = ccw(h);
            v.reverse(); // holes run clockwise
            v
        })
        .collect();
    // Leftmost holes first, so a cut drawn to the left never crosses a hole
    // that has not been joined yet.
    let left = |h: &Vec<[f64; 2]>| h.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
    hs.sort_by(|a, b| left(a).total_cmp(&left(b)));
    for h in hs {
        let (hi, p) = h
            .iter()
            .enumerate()
            .min_by(|a, b| a.1[0].total_cmp(&b.1[0]))
            .map(|(i, p)| (i, *p))
            .unwrap();
        // Nearest edge of the current outline straight to the left of p.
        let mut best: Option<(f64, usize, [f64; 2])> = None;
        for i in 0..out.len() {
            let (a, b) = (out[i], out[(i + 1) % out.len()]);
            if (a[1] > p[1]) == (b[1] > p[1]) {
                continue;
            }
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            let x = a[0] + t * (b[0] - a[0]);
            if x <= p[0] && best.is_none_or(|bb| p[0] - x < bb.0) {
                best = Some((p[0] - x, i, [x, p[1]]));
            }
        }
        let Some((_, i, q)) = best else { continue };
        let mut joined = Vec::with_capacity(out.len() + h.len() + 3);
        joined.extend_from_slice(&out[..=i]);
        joined.push(q);
        for k in 0..=h.len() {
            joined.push(h[(hi + k) % h.len()]);
        }
        joined.push(q);
        joined.extend_from_slice(&out[i + 1..]);
        out = joined;
    }
    out
}

/// A rounded rectangle centred on `at`, as an outline in board coordinates
/// with each corner flattened to short chords (for pads Altium draws with
/// rounded corners).
pub fn round_rect(
    at: Point,
    sx: Length,
    sy: Length,
    rotation_deg: f64,
    radius: Length,
) -> Vec<Point> {
    let (hx, hy) = (sx.0 as f64 / 2.0, sy.0 as f64 / 2.0);
    let r = (radius.0 as f64).clamp(0.0, hx.min(hy));
    let (s, c) = rotation_deg.to_radians().sin_cos();
    let mut out = Vec::with_capacity(28);
    // Corner centres counter-clockwise from the top right.
    let corners = [
        (hx - r, hy - r, 0.0),
        (-(hx - r), hy - r, 90.0),
        (-(hx - r), -(hy - r), 180.0),
        (hx - r, -(hy - r), 270.0),
    ];
    for (cx, cy, a0) in corners {
        for k in 0..=6 {
            let a = (a0 + 15.0 * k as f64).to_radians();
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            out.push(Point {
                x: Length(at.x.0 + (x * c - y * s).round() as i64),
                y: Length(at.y.0 + (x * s + y * c).round() as i64),
            });
        }
    }
    out.dedup();
    out
}

/// An area grown (positive `d`) or shrunk (negative) by `d`, with sharp
/// corners: what a mask or paste opening of an area is. May split into
/// several areas, or vanish.
pub fn offset_area(
    outline: &[Point],
    holes: &[Vec<Point>],
    d: Length,
) -> Vec<(Vec<Point>, Vec<Vec<Point>>)> {
    use i_overlay::mesh::outline::offset::OutlineOffset;
    use i_overlay::mesh::style::{LineJoin, OutlineStyle};
    let ring = |pts: &[Point], ccw: bool| -> Vec<[f64; 2]> {
        let mut v: Vec<[f64; 2]> = pts
            .iter()
            .map(|p| [p.x.0 as f64 / 1000.0, p.y.0 as f64 / 1000.0])
            .collect();
        if v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        if (ring_area(&v) > 0.0) != ccw {
            v.reverse();
        }
        v
    };
    let back = |v: Vec<[f64; 2]>| -> Vec<Point> {
        v.into_iter()
            .map(|p| Point {
                x: Length((p[0] * 1000.0).round() as i64),
                y: Length((p[1] * 1000.0).round() as i64),
            })
            .collect()
    };
    if outline.len() < 3 {
        return Vec::new();
    }
    if d.0 == 0 {
        return vec![(outline.to_vec(), holes.to_vec())];
    }
    let mut shape = vec![ring(outline, true)];
    shape.extend(
        holes
            .iter()
            .filter(|h| h.len() >= 3)
            .map(|h| ring(h, false)),
    );
    let style = OutlineStyle::new(d.0 as f64 / 1000.0).line_join(LineJoin::Miter(0.1));
    shape
        .outline(&style)
        .into_iter()
        .filter(|s| !s.is_empty() && s[0].len() >= 3)
        .map(|mut s| {
            let o = back(s.remove(0));
            (o, s.into_iter().map(back).collect())
        })
        .collect()
}

#[cfg(test)]
mod offset_tests {
    use super::*;

    #[test]
    fn grows_and_shrinks_a_square() {
        let p = |x: f64, y: f64| Point {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
        };
        let sq = vec![p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)];
        let area = |v: &[Point]| {
            let r: Vec<[f64; 2]> = v.iter().map(|q| [q.x.mm(), q.y.mm()]).collect();
            ring_area(&r).abs()
        };
        let g = offset_area(&sq, &[], Length::from_mm(0.5));
        assert_eq!(g.len(), 1);
        assert!((area(&g[0].0) - 9.0).abs() < 1e-3, "{}", area(&g[0].0));
        let s = offset_area(&sq, &[], Length::from_mm(-0.5));
        assert!((area(&s[0].0) - 1.0).abs() < 1e-3);
        assert!(offset_area(&sq, &[], Length::from_mm(-1.5)).is_empty());
    }
}
