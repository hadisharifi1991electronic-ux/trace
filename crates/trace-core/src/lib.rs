//! Design data model for Trace.
//!
//! Everything here is format-neutral: importers (Altium, KiCad, ...) build these
//! types, and the editor, router and checkers only ever read these types.

pub mod backdrill;
pub mod board;
pub mod bus;
pub mod connectivity;
pub mod constraints;
pub mod drc;
pub mod eco;
pub mod edit;
pub mod fdtd;
pub mod field;
pub mod file;
pub mod font;
pub mod footprint;
pub mod geometry;
pub mod gloss;
pub mod ibis;
pub mod impedance;
pub mod pairopt;
pub mod panel;
pub mod pi;
pub mod place;
pub mod pour;
pub mod route;
pub mod sch_edit;
pub mod schematic;
pub mod select;
pub mod sim;
pub mod stackfile;
pub mod stitch;
pub mod teardrop;
pub mod touchstone;
pub mod units;
pub mod validate;
pub mod viaopt;
pub mod walk;

pub use board::*;
pub use geometry::*;
pub use schematic::*;
pub use units::Length;
