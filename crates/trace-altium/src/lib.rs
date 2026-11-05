//! Reads Altium Designer files (`.PcbDoc`, `.SchDoc`) into Trace's data model.
//!
//! Both formats are OLE compound files holding one stream per object type.
//! Format knowledge here was worked out from real files; KiCad's open-source
//! Altium importer was used as a behaviour reference.

mod pcb;
mod pcb_ascii;
pub mod records;
mod sch;

pub use pcb::read_pcbdoc;
pub use sch::read_schdoc;

use std::io::{Read, Seek};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot open {path}: {source}")]
    Open {
        path: String,
        source: std::io::Error,
    },
    #[error("not an Altium file: missing stream {0}")]
    MissingStream(String),
    #[error("read error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Container<F> {
    cfb: cfb::CompoundFile<F>,
}

impl Container<std::fs::File> {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let cfb = cfb::open(path).map_err(|source| Error::Open {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Container { cfb })
    }
}

impl<F: Read + Seek> Container<F> {
    /// Reads a whole stream; `None` when the file does not have it.
    pub(crate) fn stream(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        if !self.cfb.is_stream(name) {
            return Ok(None);
        }
        let mut buf = Vec::new();
        self.cfb.open_stream(name)?.read_to_end(&mut buf)?;
        Ok(Some(buf))
    }
}
