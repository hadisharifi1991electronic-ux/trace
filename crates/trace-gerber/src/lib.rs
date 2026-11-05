//! Turns fabrication data back into a board. Give it a folder, a `.zip` of
//! Gerber and drill files, or the files themselves: each file's layer is
//! recognised automatically, then copper, holes, silkscreen and the outline
//! are rebuilt and nets are traced through the copper (or read from X2
//! attributes when the files carry them).

#![allow(clippy::type_complexity)]

mod build;
pub mod detect;
pub mod excellon;
mod geom;
pub mod gerber;
#[doc(hidden)]
pub mod nets;
pub mod odb;

pub use build::{FileReport, Import};

use build::{Layer, LayerData};
use detect::{Content, Role};
use std::io::Read;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Open {
        path: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// One input file.
pub struct SourceFile {
    pub name: String,
    pub data: Vec<u8>,
}

const MAX_FILE: u64 = 512 * 1024 * 1024;

/// File extensions that are Gerber or drill data on their own (or ODB++
/// archives).
pub fn is_fab_file(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let fixed = [
        "tgz", "tar", "gbr", "ger", "grb", "gbx", "pho", "art", "gtl", "gbl", "gto", "gbo", "gts",
        "gbs", "gtp", "gbp", "gko", "gml", "gm", "cmp", "sol", "stc", "sts", "plc", "pls", "crc",
        "crs", "dim", "drl", "xln", "exc", "drd", "ncd", "tap", "nc",
    ];
    if fixed.contains(&ext.as_str()) {
        return true;
    }
    let num = |p: &str| {
        ext.strip_prefix(p)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    num("g") || num("gp") || num("gm") || num("gl")
}

/// Reads a folder, an archive (`.zip`) or a single Gerber or drill file.
pub fn read_path(path: impl AsRef<Path>) -> Result<Import> {
    let path = path.as_ref();
    let open = |source| Error::Open {
        path: path.display().to_string(),
        source,
    };
    if path.is_dir() {
        if let Some(root) = find_odb_root(path, 0) {
            let mut entries = Vec::new();
            walk_all(&root, &root, 0, &mut entries).map_err(open)?;
            let job = odb::Job::from_entries(entries)
                .ok_or_else(|| Error::Format("incomplete ODB++ job".into()))?;
            return odb::read(&job);
        }
        let mut files = Vec::new();
        walk(path, path, 0, &mut files).map_err(open)?;
        return read_files(files);
    }
    let data = std::fs::read(path).map_err(open)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let entries = if data.starts_with(b"PK") {
        Some(unzip(&data)?)
    } else if data.starts_with(&[0x1f, 0x8b]) || is_tar(&data) {
        Some(untar(&data)?)
    } else {
        None
    };
    if let Some(entries) = entries {
        let raw: Vec<(String, Vec<u8>)> = entries.into_iter().map(|f| (f.name, f.data)).collect();
        if raw.iter().any(|(n, _)| {
            let n = n.to_ascii_lowercase();
            n.ends_with("matrix/matrix") || n.ends_with("matrix/matrix.z")
        }) {
            let job = odb::Job::from_entries(raw)
                .ok_or_else(|| Error::Format("incomplete ODB++ job".into()))?;
            return odb::read(&job);
        }
        let files = raw
            .into_iter()
            .filter(|(n, _)| {
                let base = n.rsplit('/').next().unwrap_or(n);
                !n.contains("__MACOSX")
                    && !n.contains("__Previews")
                    && !base.starts_with('.')
                    && !detect::skip_by_name(base)
            })
            .map(|(name, data)| SourceFile { name, data })
            .collect();
        return read_files(files);
    }
    read_files(vec![SourceFile { name, data }])
}

/// The folder holding `matrix/matrix`, if `dir` is (or contains) an ODB++ job.
fn find_odb_root(dir: &Path, depth: u32) -> Option<std::path::PathBuf> {
    if dir.join("matrix").join("matrix").is_file()
        || dir.join("matrix").join("matrix.Z").is_file()
        || dir.join("matrix").join("matrix.z").is_file()
    {
        return Some(dir.to_path_buf());
    }
    if depth >= 2 {
        return None;
    }
    let mut subs: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    subs.sort_by_key(|e| e.file_name());
    subs.into_iter()
        .find_map(|e| find_odb_root(&e.path(), depth + 1))
}

fn walk_all(
    root: &Path,
    dir: &Path,
    depth: u32,
    out: &mut Vec<(String, Vec<u8>)>,
) -> std::io::Result<()> {
    if depth > 8 {
        return Ok(());
    }
    for e in std::fs::read_dir(dir)?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            walk_all(root, &p, depth + 1, out)?;
        } else if e.metadata().map(|m| m.len() <= MAX_FILE).unwrap_or(false) {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&p)?));
        }
    }
    Ok(())
}

fn is_tar(d: &[u8]) -> bool {
    d.len() > 262 && &d[257..262] == b"ustar"
}

fn untar(data: &[u8]) -> Result<Vec<SourceFile>> {
    let bad = |e: std::io::Error| Error::Format(format!("bad tar archive: {e}"));
    let reader: Box<dyn Read> = if data.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(data))
    } else {
        Box::new(data)
    };
    let mut ar = tar::Archive::new(reader);
    let mut out = Vec::new();
    for e in ar.entries().map_err(bad)? {
        let mut e = e.map_err(bad)?;
        if !e.header().entry_type().is_file() || e.size() > MAX_FILE {
            continue;
        }
        let name = e.path().map_err(bad)?.to_string_lossy().into_owned();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).map_err(bad)?;
        out.push(SourceFile { name, data: buf });
    }
    Ok(out)
}

fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<SourceFile>) -> std::io::Result<()> {
    if depth > 4 {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("__") {
            continue;
        }
        if p.is_dir() {
            walk(root, &p, depth + 1, out)?;
        } else if e.metadata().map(|m| m.len() <= MAX_FILE).unwrap_or(false)
            && !detect::skip_by_name(&name)
        {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(SourceFile {
                name: rel,
                data: std::fs::read(&p)?,
            });
        }
    }
    Ok(())
}

fn unzip(data: &[u8]) -> Result<Vec<SourceFile>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data))
        .map_err(|e| Error::Format(format!("bad zip archive: {e}")))?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z
            .by_index(i)
            .map_err(|e| Error::Format(format!("bad zip entry: {e}")))?;
        let name = f.name().to_string();
        let base = name.rsplit('/').next().unwrap_or(&name).to_string();
        let _ = &base;
        if f.is_dir() || f.size() > MAX_FILE {
            continue;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)
            .map_err(|e| Error::Format(format!("cannot unpack {name}: {e}")))?;
        out.push(SourceFile { name, data: buf });
    }
    Ok(out)
}

/// Recognises and converts a set of Gerber and drill files.
pub fn read_files(files: Vec<SourceFile>) -> Result<Import> {
    let mut layers = Vec::new();
    for f in files {
        let base = f
            .name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&f.name)
            .to_string();
        if detect::skip_by_name(&base) {
            continue;
        }
        let (role, reason, data) = if excellon::sniff(&f.data) {
            match excellon::parse(&f.data) {
                Ok(d) => {
                    let (r, why) = detect::detect(&base, Content::Drill(&d));
                    (r, why, LayerData::Drill(d))
                }
                Err(e) => (Role::Ignore, e, LayerData::None),
            }
        } else {
            match gerber::parse(&f.data) {
                Ok(g) => {
                    let (r, why) = detect::detect(&base, Content::Gerber(&g));
                    (r, why, LayerData::Gerber(g))
                }
                Err(_) => (
                    Role::Ignore,
                    "not Gerber or drill data".into(),
                    LayerData::None,
                ),
            }
        };
        layers.push(Layer {
            file: f.name,
            role,
            reason,
            data,
        });
    }
    // Nothing recognised as copper: show unknown Gerber files as copper so a
    // single file can still be viewed.
    if !layers.iter().any(|l| l.role.is_copper()) {
        for l in &mut layers {
            if l.role == Role::Unknown && matches!(l.data, LayerData::Gerber(_)) {
                l.role = Role::Copper {
                    pos: detect::CuPos::Inner,
                    number: None,
                    order: None,
                    plane: false,
                };
                l.reason = "not recognised, shown as copper".into();
            }
        }
    }
    if !layers.iter().any(|l| l.role.is_copper()) {
        let found: Vec<String> = layers
            .iter()
            .map(|l| format!("{} ({})", l.file, l.role.label()))
            .collect();
        return Err(Error::Format(if found.is_empty() {
            "no Gerber or drill files found".into()
        } else {
            format!("no copper layer recognised among: {}", found.join(", "))
        }));
    }
    Ok(build::build(
        layers,
        "X2 net attributes in the Gerber files",
    ))
}
