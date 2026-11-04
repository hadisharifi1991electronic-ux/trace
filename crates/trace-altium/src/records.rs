//! Altium's property records: `|KEY=VALUE|KEY=VALUE...` text blocks, each
//! prefixed by a little-endian length, stored inside OLE compound-file streams.

use std::collections::HashMap;

/// One record's properties. Keys are upper-cased; Altium is case-insensitive.
#[derive(Clone, Debug, Default)]
pub struct Props {
    map: HashMap<String, String>,
}

impl Props {
    pub fn parse(text: &str) -> Self {
        let mut map = HashMap::new();
        for field in text.split('|') {
            if let Some((k, v)) = field.split_once('=') {
                let key = k.trim().to_ascii_uppercase();
                // `%UTF8%KEY` carries a Unicode copy of KEY; prefer it.
                if let Some(base) = key.strip_prefix("%UTF8%") {
                    map.insert(base.to_string(), v.to_string());
                } else {
                    map.entry(key).or_insert_with(|| v.to_string());
                }
            }
        }
        Props { map }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(&key.to_ascii_uppercase()).map(String::as_str)
    }
    pub fn str(&self, key: &str) -> String {
        self.get(key).unwrap_or_default().to_string()
    }
    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }
    pub fn float(&self, key: &str) -> Option<f64> {
        self.get(key)?.trim().parse().ok()
    }
    pub fn bool(&self, key: &str) -> bool {
        matches!(self.get(key), Some(v) if v.eq_ignore_ascii_case("TRUE") || v == "T")
    }
    pub fn keys(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }
}

/// Altium stores 8-bit text as Windows-1252; map bytes to chars directly
/// (exact for ASCII and Latin-1, which covers designators and net names).
fn decode(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect()
}

/// A record from a schematic stream: text records parse into properties,
/// binary records (used for some pins) are kept raw.
#[derive(Clone, Debug)]
pub enum SchRecord {
    Text(Props),
    Binary(Vec<u8>),
}

/// Splits a schematic `FileHeader` stream. Each record has a 4-byte header:
/// low 24 bits = length, high byte = type (0 text, 1 binary).
pub fn split_sch(data: &[u8]) -> Vec<SchRecord> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= data.len() {
        let hdr = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let len = (hdr & 0x00FF_FFFF) as usize;
        let kind = hdr >> 24;
        i += 4;
        if i + len > data.len() {
            break;
        }
        let body = &data[i..i + len];
        out.push(if kind == 0 {
            SchRecord::Text(Props::parse(&decode(body)))
        } else {
            SchRecord::Binary(body.to_vec())
        });
        i += len;
    }
    out
}

/// Splits a PCB `*6/Data` text stream: u32 length + text, repeated.
/// `prefix` skips a per-record type tag (2 bytes in `Rules6`).
pub fn split_pcb(data: &[u8], prefix: usize) -> Vec<Props> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + prefix + 4 <= data.len() {
        i += prefix;
        let len = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        i += 4;
        if i + len > data.len() {
            break;
        }
        out.push(Props::parse(&decode(&data[i..i + len])));
        i += len;
    }
    out
}

/// Splits a binary primitive stream (`Tracks6`, `Vias6`, `Arcs6`, `Pads6`):
/// each record is a type byte followed by `subrecords` blocks of
/// `[len u32][bytes]`. Returns every block of each record.
pub fn split_binary(data: &[u8], subrecords: usize) -> Vec<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut i = 0;
    'records: while i < data.len() {
        i += 1; // record type
        let mut blocks = Vec::with_capacity(subrecords);
        for _ in 0..subrecords {
            if i + 4 > data.len() {
                break 'records;
            }
            let len = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            i += 4;
            if i + len > data.len() {
                break 'records;
            }
            blocks.push(&data[i..i + len]);
            i += len;
        }
        out.push(blocks);
    }
    out
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
pub(crate) fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
pub(crate) fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(text: &str) -> Vec<u8> {
        let mut v = (text.len() as u32 + 1).to_le_bytes().to_vec();
        v.extend_from_slice(text.as_bytes());
        v.push(0);
        v
    }

    #[test]
    fn parses_props() {
        let p = Props::parse("|RECORD=1|LibReference=R|%UTF8%Name=Ω|Name=?|ENABLED=TRUE");
        assert_eq!(p.int("record"), Some(1));
        assert_eq!(p.get("LIBREFERENCE"), Some("R"));
        assert_eq!(p.get("name"), Some("Ω"));
        assert!(p.bool("enabled"));
    }

    #[test]
    fn splits_pcb_and_sch_streams() {
        let mut data = rec("|NAME=GND");
        data.extend(rec("|NAME=VCC"));
        let recs = split_pcb(&data, 0);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].get("NAME"), Some("VCC"));

        let mut sch = rec("|HEADER=x");
        sch.extend_from_slice(&(0x0100_0002u32).to_le_bytes());
        sch.extend_from_slice(&[1, 2]);
        let recs = split_sch(&sch);
        assert!(matches!(recs[0], SchRecord::Text(_)));
        assert!(matches!(recs[1], SchRecord::Binary(ref b) if b == &[1, 2]));
    }
}
