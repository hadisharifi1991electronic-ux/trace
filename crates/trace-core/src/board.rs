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
    /// Component classes: their names and the designators in each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_classes: Vec<NetClass>,
    pub rules: Vec<Rule>,
    pub stats: BoardStats,
    pub geometry: crate::Geometry,
    /// Set on a panel Trace made: copies of one board, numbered copy by
    /// copy, so rules written for the board apply to every copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelOf>,
    /// Assembly variants: parts left off or changed in each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<crate::variant::Variant>,
    /// The variant outputs (BOM, pick and place, assembly drawing, STEP)
    /// are made for; `None` is the board as designed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<usize>,
    /// Parts marked as net ties (designators), besides those named as one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub net_ties: Vec<String>,
}

/// How a panel's nets and parts follow from its board: `nets` nets and
/// `parts` parts per copy, copies after the first named with `_2`, `_3`...
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelOf {
    pub copies: u32,
    pub nets: u32,
    pub parts: u32,
}

impl PanelOf {
    /// The board's own name for net or part `i` named `name` in the panel.
    pub fn base<'a>(&self, name: &'a str, i: usize, per_copy: u32) -> &'a str {
        let k = if per_copy == 0 {
            0
        } else {
            i / per_copy as usize
        };
        if k == 0 {
            return name;
        }
        name.strip_suffix(&format!("_{}", k + 1)).unwrap_or(name)
    }
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
    /// Loss tangent (Df), for dielectric and mask layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_tangent: Option<f64>,
    /// Surface roughness of copper layers, RMS, micrometres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roughness_um: Option<f64>,
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
    /// Vertical length through via barrels, between the layers actually used.
    pub via_length: Length,
    pub vias: usize,
    pub pads: usize,
    /// Separate copper islands joining this net's pads; 1 means fully routed.
    pub pieces: usize,
    /// Net is connected partly through copper pours, which are not imported
    /// yet, so `pieces` overstates how open it is.
    pub has_pour: bool,
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

impl Net {
    /// Track, arc and via barrel length together.
    pub fn total_length(&self) -> Length {
        Length(self.routed_length.0 + self.via_length.0)
    }
    /// True when every pad of the net is joined by copper.
    pub fn is_complete(&self) -> bool {
        self.pads < 2 || self.pieces <= 1
    }
}

impl Stackup {
    /// Depth of the centre of each copper layer below the top surface, top first.
    pub fn copper_depths(&self) -> Vec<Length> {
        let mut depth = 0i64;
        let mut out = Vec::new();
        for l in &self.layers {
            if l.kind == LayerKind::Copper {
                out.push(Length(depth + l.thickness.0 / 2));
            }
            if matches!(l.kind, LayerKind::Copper | LayerKind::Dielectric) {
                depth += l.thickness.0;
            }
        }
        out
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
    /// The part's parameters (value, part number, manufacturer,
    /// description...), as the source names them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Component {
    /// The first parameter present of `names`, compared without case.
    pub fn param(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|n| {
            self.params
                .iter()
                .find(|(k, v)| k.eq_ignore_ascii_case(n) && !v.trim().is_empty())
                .map(|(_, v)| v.as_str())
        })
    }
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
