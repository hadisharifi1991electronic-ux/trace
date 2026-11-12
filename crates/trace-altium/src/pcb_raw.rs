//! Record-level access to a binary `.PcbDoc`, for writers that keep a
//! board's original records and change only what was edited: each
//! primitive record decoded the way [`crate::read_pcbdoc`] decodes it.

use crate::pcb::{self, LayerMap};
use trace_core::{Arc, LayerRef, Pad, Polygon, Stackup, Track, Via};

pub use crate::records::{split_binary, split_pcb, Props};

/// Altium layer numbers of a board's stack-up.
pub struct Layers(LayerMap);

impl Layers {
    pub fn new(stackup: &Stackup) -> Self {
        Layers(LayerMap::new(stackup))
    }
    /// The Trace layer of an Altium layer byte.
    pub fn get(&self, id: u8) -> LayerRef {
        self.0.get(id)
    }
    /// The Altium layer byte of copper layer `i` (0 is the top).
    pub fn id_of(&self, i: u8) -> Option<u8> {
        self.0.id_of(i)
    }
}

pub fn track(b: &[u8], l: &Layers) -> Option<Track> {
    pcb::parse_track(b, &l.0)
}
pub fn arc(b: &[u8], l: &Layers) -> Option<Arc> {
    pcb::parse_arc(b, &l.0)
}
pub fn via(b: &[u8], l: &Layers) -> Option<Via> {
    pcb::parse_via(b, &l.0)
}
/// A pad from all six blocks of its record.
pub fn pad(blocks: &[&[u8]], l: &Layers) -> Option<Pad> {
    pcb::parse_pad(blocks, &l.0)
}
pub fn fill(b: &[u8], l: &Layers) -> Option<Polygon> {
    pcb::parse_fill(b, &l.0)
}
pub fn region(b: &[u8], l: &Layers) -> Option<Polygon> {
    pcb::parse_region(b, &l.0)
}
