use crate::records::{split_sch, Props, SchRecord};
use crate::{Container, Error, Result};
use std::path::Path;
use trace_core::{Sheet, Symbol};

// Record ids in the schematic `FileHeader` stream.
const COMPONENT: i64 = 1;
const PIN: i64 = 2;
const POWER_PORT: i64 = 17;
const PORT: i64 = 18;
const NET_LABEL: i64 = 25;
const WIRE: i64 = 27;
const JUNCTION: i64 = 29;
const SHEET_FILE_NAME: i64 = 33;
const DESIGNATOR: i64 = 34;
const PARAMETER: i64 = 41;

/// Reads one `.SchDoc` sheet: symbols with designators and values, net labels,
/// power ports, ports and links to child sheets.
pub fn read_schdoc(path: impl AsRef<Path>) -> Result<Sheet> {
    let path = path.as_ref();
    let mut c = Container::open(path)?;
    let data = c
        .stream("/FileHeader")?
        .ok_or_else(|| Error::MissingStream("FileHeader".into()))?;
    let records = split_sch(&data);

    let mut sheet = Sheet {
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ..Default::default()
    };

    // OwnerIndex counts records after the header, so record k has index k - 1.
    let mut symbol_at = vec![None::<usize>; records.len()];
    let props = |r: &SchRecord| match r {
        SchRecord::Text(p) => Some(p.clone()),
        SchRecord::Binary(_) => None,
    };
    let owner = |p: &Props| p.int("OWNERINDEX").map(|i| i as usize + 1);

    for (k, rec) in records.iter().enumerate() {
        let Some(p) = props(rec) else {
            // Binary records are pins in recent file versions.
            if let Some(s) = symbol_for_last(&symbol_at, k) {
                sheet.symbols[s].pins += 1;
            }
            continue;
        };
        match p.int("RECORD") {
            Some(COMPONENT) => {
                symbol_at[k] = Some(sheet.symbols.len());
                sheet.symbols.push(Symbol {
                    library_ref: p.str("LIBREFERENCE"),
                    description: p.str("COMPONENTDESCRIPTION"),
                    ..Default::default()
                });
            }
            Some(PIN) => {
                if let Some(s) = owner(&p).and_then(|o| symbol_at.get(o).copied().flatten()) {
                    sheet.symbols[s].pins += 1;
                }
            }
            Some(DESIGNATOR) => {
                if let Some(s) = owner(&p).and_then(|o| symbol_at.get(o).copied().flatten()) {
                    sheet.symbols[s].designator = p.str("TEXT");
                }
            }
            Some(PARAMETER) if p.str("NAME").eq_ignore_ascii_case("Value") => {
                if let Some(s) = owner(&p).and_then(|o| symbol_at.get(o).copied().flatten()) {
                    sheet.symbols[s].value = p.get("TEXT").map(str::to_string);
                }
            }
            Some(NET_LABEL) => sheet.net_labels.push(p.str("TEXT")),
            Some(POWER_PORT) => sheet.power_ports.push(p.str("TEXT")),
            Some(PORT) => sheet.ports.push(p.str("NAME")),
            Some(SHEET_FILE_NAME) => sheet.sheet_refs.push(p.str("TEXT")),
            Some(WIRE) => sheet.wires += 1,
            Some(JUNCTION) => sheet.junctions += 1,
            _ => {}
        }
    }
    sheet.net_labels.sort();
    sheet.net_labels.dedup();
    sheet.power_ports.sort();
    sheet.power_ports.dedup();
    Ok(sheet)
}

/// Binary pin records do not carry an owner in a form we parse yet; they
/// follow their component, so attribute them to the most recent one.
fn symbol_for_last(symbol_at: &[Option<usize>], k: usize) -> Option<usize> {
    symbol_at[..k].iter().rev().find_map(|s| *s)
}
