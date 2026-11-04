//! Trace's own board file: open, readable JSON, so designs diff in Git.

use crate::Board;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const EXTENSION: &str = "tracepcb";
const FORMAT: &str = "trace-board";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FileV1 {
    format: String,
    version: u32,
    board: Board,
}

pub fn save(board: &Board, path: &Path) -> Result<(), String> {
    let file = FileV1 {
        format: FORMAT.into(),
        version: VERSION,
        board: board.clone(),
    };
    let text = serde_json::to_string(&file).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Board, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file: FileV1 = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a Trace board: {e}", path.display()))?;
    if file.format != FORMAT || file.version > VERSION {
        return Err(format!(
            "{} was made by a newer Trace (format {} v{})",
            path.display(),
            file.format,
            file.version
        ));
    }
    Ok(file.board)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips() {
        let mut b = crate::Board::default();
        b.nets.push(crate::Net {
            name: "GND".into(),
            ..Default::default()
        });
        let dir = std::env::temp_dir().join("trace-file-test.tracepcb");
        super::save(&b, &dir).unwrap();
        let back = super::load(&dir).unwrap();
        assert_eq!(back.nets[0].name, "GND");
        let _ = std::fs::remove_file(dir);
    }
}
