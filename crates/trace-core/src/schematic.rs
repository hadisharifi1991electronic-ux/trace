use serde::{Deserialize, Serialize};

/// One schematic sheet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    pub symbols: Vec<Symbol>,
    pub net_labels: Vec<String>,
    pub power_ports: Vec<String>,
    pub ports: Vec<String>,
    /// Child sheets referenced by sheet symbols.
    pub sheet_refs: Vec<String>,
    pub wires: usize,
    pub junctions: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Symbol {
    pub designator: String,
    pub library_ref: String,
    pub description: String,
    pub value: Option<String>,
    pub pins: usize,
}
