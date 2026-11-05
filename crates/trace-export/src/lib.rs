//! Writes a Trace board in other tools' formats, so a board read from Gerber,
//! ODB++, Altium or Allegro data can be carried on in another tool.

pub mod kicad;

#[doc(hidden)]
pub use kicad::fracture;
pub use kicad::to_kicad;
