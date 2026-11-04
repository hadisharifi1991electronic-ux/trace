//! Design data model for Trace.
//!
//! Everything here is format-neutral: importers (Altium, KiCad, ...) build these
//! types, and the editor, router and checkers only ever read these types.

pub mod board;
pub mod schematic;
pub mod units;

pub use board::*;
pub use schematic::*;
pub use units::Length;
