//! Writes a Trace board in other tools' formats, so a board read from Gerber,
//! ODB++, Altium or Allegro data can be carried on in another tool.

pub mod altium;
pub mod altium_rt;
pub mod assembly;
pub mod dxf;
pub mod fab;
pub mod fabdraw;
pub mod ipc2581;
pub mod kicad;
pub mod odb;
pub mod pdf;
pub mod step;
pub mod svg;

pub use altium::{to_altium, to_pcblib};
pub use altium_rt::{to_altium_over, SaveReport};
pub use assembly::{assembly_pdf, Paper};
pub use dxf::to_dxf;
pub use fab::{fab_files, fab_zip, FabFile, FabOptions};
pub use fabdraw::fab_drawing_pdf;
pub use ipc2581::to_ipc2581;
#[doc(hidden)]
pub use kicad::fracture;
pub use kicad::to_kicad;
pub use odb::{odb_files, odb_tgz, to_odb};
pub use step::{to_step, StepOptions, StepReport};
pub use svg::sheet_svg;
