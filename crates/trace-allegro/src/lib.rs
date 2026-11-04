//! Reads Cadence Allegro `.brd` boards (Allegro 16.0 and newer) into Trace's
//! data model.
//!
//! The binary layout is proprietary. Trace follows the reverse-engineered
//! description in KiCad 10's Allegro importer, which is GPL-3.0-or-later like
//! Trace itself.

pub mod blocks;
mod build;
mod stream;

pub use build::read_brd_bytes;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot open {path}: {source}")]
    Open {
        path: String,
        source: std::io::Error,
    },
    #[error("not a readable Allegro board: {0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Reads an Allegro `.brd` board.
pub fn read_brd(path: impl AsRef<Path>) -> Result<trace_core::Board> {
    let path = path.as_ref();
    let data = std::fs::read(path).map_err(|source| Error::Open {
        path: path.display().to_string(),
        source,
    })?;
    read_brd_bytes(&data)
}
