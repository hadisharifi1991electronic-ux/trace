//! Altium integrated libraries (`.IntLib`): a compound file that holds the
//! compiled schematic and footprint libraries (`SchLib/<n>.schlib`,
//! `PCBLib/<n>.pcblib`), each a whole `.SchLib` or `.PcbLib` compressed
//! with zlib behind a one-byte tag.

use crate::{pcblib, sch, Container, Error, Result};
use std::io::Read;
use std::path::Path;
use trace_core::footprint::Library;
use trace_core::SymbolLib;

/// What an integrated library holds.
#[derive(Clone, Debug)]
pub struct IntLib {
    pub name: String,
    /// The symbols of every schematic library inside.
    pub symbols: SymbolLib,
    /// The footprints of every footprint library inside.
    pub footprints: Library,
}

/// One embedded library's bytes: a tag byte (2), then zlib data.
fn inflate(d: &[u8]) -> Option<Vec<u8>> {
    let body = match d.first() {
        Some(2) => &d[1..],
        _ => d,
    };
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(body)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Reads an Altium integrated library: its symbols and footprints.
pub fn read_intlib(path: impl AsRef<Path>) -> Result<IntLib> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut c = Container::open(path)?;
    let mut entries: Vec<(String, String)> = Vec::new();
    for dir in ["SchLib", "PCBLib"] {
        let path = format!("/{dir}");
        if !c.cfb.is_storage(&path) {
            continue;
        }
        let names: Vec<String> = c
            .cfb
            .read_storage(&path)?
            .filter(|e| e.is_stream())
            .map(|e| e.name().to_string())
            .collect();
        entries.extend(names.into_iter().map(|n| (dir.to_string(), n)));
    }
    if entries.is_empty() {
        return Err(Error::MissingStream("SchLib or PCBLib".into()));
    }
    // Numbered libraries in order (0, 1, 2 … 10).
    entries.sort_by_key(|(d, n)| {
        let k: u32 = n
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (d.clone(), k)
    });
    let mut lib = IntLib {
        name: name.clone(),
        symbols: SymbolLib {
            name: name.clone(),
            symbols: Vec::new(),
        },
        footprints: Library::default(),
    };
    for (dir, file) in entries {
        let Some(raw) = c.stream(&format!("/{dir}/{file}"))? else {
            continue;
        };
        let Some(data) = inflate(&raw) else { continue };
        let Ok(mut inner) = Container::from_bytes(data) else {
            continue;
        };
        if dir == "SchLib" {
            if let Ok(s) = sch::schlib_from(&mut inner, name.clone()) {
                lib.symbols.symbols.extend(s.symbols);
            }
        } else if let Ok(f) = pcblib::pcblib_from(&mut inner) {
            lib.footprints.footprints.extend(f.footprints);
        }
    }
    lib.symbols.symbols.sort_by(|a, b| {
        (a.name.to_ascii_lowercase(), a.part).cmp(&(b.name.to_ascii_lowercase(), b.part))
    });
    lib.footprints
        .footprints
        .sort_by_key(|f| f.name.to_ascii_lowercase());
    Ok(lib)
}
