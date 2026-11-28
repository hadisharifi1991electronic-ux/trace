//! Reads a set of fabrication files back and compares them with the board
//! they were written from: every file recognised, the copper layers in
//! order, every hole by size, the nets and parts the files name, and the
//! board outline. What a fab house's CAM sees, checked before it goes out.

use crate::{read_files, FileReport, SourceFile};
use std::collections::{BTreeMap, BTreeSet};
use trace_core::{Board, LayerRef, Point};

/// One compared item.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckLine {
    pub item: String,
    /// What the board has.
    pub board: String,
    /// What the files read back as.
    pub files: String,
    pub ok: bool,
    /// Details of a difference (missing names, sizes), or empty.
    pub note: String,
}

/// The result of [`check`].
#[derive(Clone, Debug, Default)]
pub struct FabCheck {
    pub lines: Vec<CheckLine>,
    /// How each fab file was taken.
    pub files: Vec<FileReport>,
    /// Reader warnings.
    pub warnings: Vec<String>,
}

impl FabCheck {
    /// Items that differ.
    pub fn problems(&self) -> usize {
        self.lines.iter().filter(|l| !l.ok).count()
    }

    /// Plain text, one item per line, for the command line.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for l in &self.lines {
            s.push_str(&format!(
                "  {} {:<22} board {:<28} files {}\n",
                if l.ok { "ok  " } else { "DIFF" },
                l.item,
                l.board,
                l.files
            ));
            if !l.note.is_empty() {
                s.push_str(&format!("       {}\n", l.note));
            }
        }
        s
    }
}

/// Holes within this many nm count as the same size (inch files round
/// to 0.25 µm).
const SIZE_TOL: i64 = 2_000;
/// Outline corners within this many nm agree.
const OUTLINE_TOL: i64 = 10_000;

/// Reads `files` (Gerber, drill and IPC-356; other files are passed over)
/// and compares them with `board`. Backdrill files are left out: their
/// holes are drilled in holes already counted.
pub fn check(board: &Board, files: Vec<SourceFile>) -> FabCheck {
    let mut out = FabCheck::default();
    let n = board.stackup.copper_layers().count().max(1);
    let fab: Vec<SourceFile> = files
        .into_iter()
        .filter(|f| crate::is_fab_file(&f.name))
        .filter(|f| !f.name.to_ascii_uppercase().contains("BACKDRILL"))
        .collect();
    let names_expected = fab.iter().any(|f| {
        f.name.to_ascii_lowercase().ends_with(".ipc") || f.data.windows(5).any(|w| w == b"%TO.N")
    });
    let parts_expected = fab.iter().any(|f| f.data.windows(5).any(|w| w == b"%TO.C"));
    let file_names: Vec<String> = fab.iter().map(|f| f.name.clone()).collect();
    let imp = match read_files(fab) {
        Ok(i) => i,
        Err(e) => {
            out.lines.push(CheckLine {
                item: "Files read back".into(),
                board: format!("{} files", file_names.len()),
                files: "not readable".into(),
                ok: false,
                note: e.to_string(),
            });
            return out;
        }
    };
    let back = &imp.board;
    out.warnings = imp.warnings.clone();

    // Every file taken as something.
    let unknown: Vec<&FileReport> = imp
        .files
        .iter()
        .filter(|f| f.role == "Unknown" || f.role.starts_with("Not used"))
        .collect();
    out.lines.push(CheckLine {
        item: "Files recognised".into(),
        board: format!("{} files", imp.files.len()),
        files: format!("{} recognised", imp.files.len() - unknown.len()),
        ok: unknown.is_empty(),
        note: names(unknown.iter().map(|f| f.file.as_str())),
    });

    // Copper layers, and each copper file at the index its name gives.
    let nb = back.stackup.copper_layers().count();
    out.lines.push(CheckLine {
        item: "Copper layers".into(),
        board: n.to_string(),
        files: nb.to_string(),
        ok: nb == n,
        note: String::new(),
    });
    let misplaced: Vec<String> = imp
        .files
        .iter()
        .filter_map(|f| {
            let got = f.copper_index?;
            let want = expected_index(&f.file, n)?;
            (got as usize != want)
                .then(|| format!("{} as L{} (want L{})", f.file, got + 1, want + 1))
        })
        .collect();
    out.lines.push(CheckLine {
        item: "Copper layer order".into(),
        board: format!("L1-L{n}"),
        files: if misplaced.is_empty() {
            "same order".into()
        } else {
            format!("{} out of place", misplaced.len())
        },
        ok: misplaced.is_empty(),
        note: misplaced.join(", "),
    });

    // Holes by size.
    let ours = holes(board);
    let theirs = holes(back);
    let diffs = hole_diffs(&ours, &theirs);
    out.lines.push(CheckLine {
        item: "Holes".into(),
        board: format!("{} in {} sizes", total(&ours), ours.len()),
        files: format!("{} in {} sizes", total(&theirs), theirs.len()),
        ok: diffs.is_empty(),
        note: diffs.join(", "),
    });

    // Nets and parts by name, when the files carry names.
    let our_nets = copper_nets(board);
    let their_nets: BTreeSet<&str> = back.nets.iter().map(|n| n.name.as_str()).collect();
    if names_expected {
        let missing: Vec<&str> = our_nets
            .iter()
            .copied()
            .filter(|n| !their_nets.contains(n))
            .collect();
        out.lines.push(CheckLine {
            item: "Nets".into(),
            board: format!("{} with copper", our_nets.len()),
            files: format!("{} named", their_nets.len()),
            ok: missing.is_empty(),
            note: if missing.is_empty() {
                String::new()
            } else {
                format!("missing: {}", names(missing.iter().copied()))
            },
        });
        let our_parts = copper_parts(board);
        let their_parts: BTreeSet<&str> = back
            .components
            .iter()
            .map(|c| c.designator.as_str())
            .collect();
        if parts_expected {
            let missing: Vec<&str> = our_parts
                .iter()
                .copied()
                .filter(|p| !their_parts.contains(p))
                .collect();
            out.lines.push(CheckLine {
                item: "Parts on copper".into(),
                board: our_parts.len().to_string(),
                files: their_parts.len().to_string(),
                ok: missing.is_empty(),
                note: if missing.is_empty() {
                    String::new()
                } else {
                    format!("missing: {}", names(missing.iter().copied()))
                },
            });
        }
    } else {
        out.lines.push(CheckLine {
            item: "Nets".into(),
            board: format!("{} with copper", our_nets.len()),
            files: format!("{} traced", back.nets.len()),
            ok: true,
            note: "the files carry no net names (X1 without IPC-356), so the fab house traces them from the copper".into(),
        });
    }

    // Outline extents.
    if board.geometry.outline.len() >= 3 {
        let a = extents(&board.geometry.outline);
        let b = extents(&back.geometry.outline);
        let ok = match (a, b) {
            (Some(a), Some(b)) => a
                .iter()
                .zip(b.iter())
                .all(|(x, y)| (x - y).abs() <= OUTLINE_TOL),
            _ => false,
        };
        let size = |e: Option<[i64; 4]>| match e {
            Some(e) => format!(
                "{:.3} x {:.3} mm",
                (e[2] - e[0]) as f64 / 1e6,
                (e[3] - e[1]) as f64 / 1e6
            ),
            None => "none".into(),
        };
        out.lines.push(CheckLine {
            item: "Board outline".into(),
            board: size(a),
            files: size(b),
            ok,
            note: String::new(),
        });
    }
    out.files = imp.files;
    out
}

/// The copper index a file's Altium-style name gives: GTL top, GBL bottom,
/// G1.. inner.
fn expected_index(file: &str, n: usize) -> Option<usize> {
    let ext = file.rsplit('.').next()?.to_ascii_uppercase();
    match ext.as_str() {
        "GTL" => Some(0),
        "GBL" => Some(n - 1),
        _ => ext
            .strip_prefix('G')
            .and_then(|k| k.parse::<usize>().ok())
            .filter(|&k| k >= 1 && k + 1 < n),
    }
}

/// Hole count by diameter (nm).
fn holes(b: &Board) -> BTreeMap<i64, usize> {
    let mut m = BTreeMap::new();
    let pads = b.geometry.pads.iter().map(|p| p.hole.0);
    let vias = b.geometry.vias.iter().map(|v| v.hole.0);
    for d in pads.chain(vias).filter(|&d| d > 0) {
        *m.entry(d).or_insert(0) += 1;
    }
    m
}

fn total(m: &BTreeMap<i64, usize>) -> usize {
    m.values().sum()
}

/// Sizes whose counts differ, sizes within [`SIZE_TOL`] taken as one.
fn hole_diffs(ours: &BTreeMap<i64, usize>, theirs: &BTreeMap<i64, usize>) -> Vec<String> {
    let group = |m: &BTreeMap<i64, usize>| {
        let mut g: Vec<(i64, usize)> = Vec::new();
        for (&d, &c) in m {
            match g.last_mut() {
                Some(last) if d - last.0 <= SIZE_TOL => last.1 += c,
                _ => g.push((d, c)),
            }
        }
        g
    };
    let (a, b) = (group(ours), group(theirs));
    let find = |g: &[(i64, usize)], d: i64| {
        g.iter()
            .find(|x| (x.0 - d).abs() <= SIZE_TOL)
            .map_or(0, |x| x.1)
    };
    let mut out = Vec::new();
    for &(d, c) in &a {
        let t = find(&b, d);
        if t != c {
            out.push(format!(
                "{:.3} mm: {c} on the board, {t} in the files",
                d as f64 / 1e6
            ));
        }
    }
    for &(d, c) in &b {
        if find(&a, d) == 0 {
            out.push(format!(
                "{:.3} mm: none on the board, {c} in the files",
                d as f64 / 1e6
            ));
        }
    }
    out
}

fn on_copper(l: &LayerRef) -> bool {
    matches!(l, LayerRef::Copper(_) | LayerRef::AllCopper)
}

/// Names of nets that have copper on the board.
fn copper_nets(b: &Board) -> BTreeSet<&str> {
    let g = &b.geometry;
    let ids = g
        .pads
        .iter()
        .filter(|p| on_copper(&p.layer))
        .map(|p| p.net)
        .chain(
            g.tracks
                .iter()
                .filter(|t| on_copper(&t.layer) && !t.text)
                .map(|t| t.net),
        )
        .chain(g.arcs.iter().filter(|a| on_copper(&a.layer)).map(|a| a.net))
        .chain(g.vias.iter().map(|v| v.net))
        .chain(
            g.polygons
                .iter()
                .filter(|p| on_copper(&p.layer))
                .map(|p| p.net),
        );
    ids.flatten()
        .filter_map(|i| b.nets.get(i as usize))
        .map(|n| n.name.as_str())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Designators of parts with copper pads.
fn copper_parts(b: &Board) -> BTreeSet<&str> {
    b.geometry
        .pads
        .iter()
        .filter(|p| on_copper(&p.layer))
        .filter_map(|p| p.component)
        .filter_map(|c| b.components.get(c as usize))
        .map(|c| c.designator.as_str())
        .filter(|d| !d.is_empty())
        .collect()
}

/// [min x, min y, max x, max y] in nm.
fn extents(pts: &[Point]) -> Option<[i64; 4]> {
    let mut it = pts.iter();
    let p = it.next()?;
    let mut e = [p.x.0, p.y.0, p.x.0, p.y.0];
    for p in it {
        e[0] = e[0].min(p.x.0);
        e[1] = e[1].min(p.y.0);
        e[2] = e[2].max(p.x.0);
        e[3] = e[3].max(p.y.0);
    }
    Some(e)
}

/// Up to eight names, then a count of the rest.
fn names<'a>(it: impl Iterator<Item = &'a str>) -> String {
    let v: Vec<&str> = it.collect();
    if v.len() <= 8 {
        v.join(", ")
    } else {
        format!("{} and {} more", v[..8].join(", "), v.len() - 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copper_file_names_give_layer_indexes() {
        assert_eq!(expected_index("b.GTL", 6), Some(0));
        assert_eq!(expected_index("b.GBL", 6), Some(5));
        assert_eq!(expected_index("b.G3", 6), Some(3));
        assert_eq!(expected_index("b.G5", 6), None);
        assert_eq!(expected_index("b.GTO", 6), None);
        assert_eq!(expected_index("b.GM13", 6), None);
    }

    #[test]
    fn hole_sizes_within_tolerance_agree() {
        let a: BTreeMap<i64, usize> = [(300_000, 4), (1_000_000, 2)].into();
        let b: BTreeMap<i64, usize> = [(300_001, 4), (999_999, 2)].into();
        assert!(hole_diffs(&a, &b).is_empty());
        let c: BTreeMap<i64, usize> = [(300_000, 3), (1_000_000, 2), (500_000, 1)].into();
        let d = hole_diffs(&a, &c);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(d[0].starts_with("0.300 mm: 4 on the board, 3"));
        assert!(d[1].starts_with("0.500 mm: none on the board, 1"));
    }
}
