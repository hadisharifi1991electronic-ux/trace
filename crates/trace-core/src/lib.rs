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
pub mod file;
pub mod font;
pub mod footprint;
pub mod geometry;
pub mod impedance;
pub mod panel;
pub mod place;
pub mod pour;
pub mod route;
pub mod sch_edit;
pub mod schematic;
pub mod units;
pub mod walk;

pub use board::*;
pub use geometry::*;
pub use schematic::*;
pub use units::Length;
