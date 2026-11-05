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
