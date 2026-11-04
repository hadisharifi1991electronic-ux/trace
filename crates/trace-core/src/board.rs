use crate::Length;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Board {
    pub stackup: Stackup,
    pub nets: Vec<Net>,
    pub net_classes: Vec<NetClass>,
    pub diff_pair_classes: Vec<NetClass>,
    pub diff_pairs: Vec<DiffPair>,
    pub components: Vec<Component>,
    pub rules: Vec<Rule>,
    pub stats: BoardStats,
    pub geometry: crate::Geometry,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BoardStats {
    pub tracks: usize,
    pub vias: usize,
    pub pads: usize,
    pub polygons: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stackup {
    /// Top to bottom.
    pub layers: Vec<StackLayer>,
}

impl Stackup {
    pub fn copper_layers(&self) -> impl Iterator<Item = &StackLayer> {
        self.layers
            .iter()
            .filter(|l| matches!(l.kind, LayerKind::Copper))
    }
    pub fn total_thickness(&self) -> Length {
        Length(self.layers.iter().map(|l| l.thickness.0).sum())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StackLayer {
    pub name: String,
    /// Layer number in the source file, kept so primitives can be matched up.
    #[serde(skip)]
    pub altium_id: Option<u8>,
    pub kind: LayerKind,
    pub thickness: Length,
    /// Relative permittivity, for dielectric and mask layers.
    pub dielectric_constant: Option<f64>,
    pub material: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    Copper,
    Dielectric,
    SolderMask,
    Silkscreen,
    Paste,
    Other,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Net {
    pub name: String,
    /// Total length of copper tracks and arcs on this net.
    pub routed_length: Length,
    pub vias: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffPair {
    pub name: String,
    pub positive: String,
    pub negative: String,
}

impl Board {
    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetClass {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Component {
    pub designator: String,
    pub footprint: String,
    pub library_ref: Option<String>,
    pub x: Length,
    pub y: Length,
    pub rotation_deg: f64,
    pub side: Side,
    /// Schematic sheet the part came from.
    pub sheet: Option<String>,
}

/// A design rule, kept close to its source so nothing is lost on import.
/// The constraint manager maps known kinds onto typed constraints.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i64,
    pub scope: Vec<String>,
    pub params: BTreeMap<String, String>,
}
