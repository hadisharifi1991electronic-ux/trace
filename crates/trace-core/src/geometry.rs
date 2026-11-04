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
}
