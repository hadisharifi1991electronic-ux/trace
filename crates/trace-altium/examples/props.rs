//! `props <file> <stream>`: prints the |KEY=VALUE| records of a text stream,
//! one property per line, prefixed with the record number.
use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut f = cfb::open(&a[0]).unwrap();
    let mut buf = Vec::new();
    f.open_stream(&a[1]).unwrap().read_to_end(&mut buf).unwrap();
    let mut i = 0;
    let mut n = 0;
    while i + 4 <= buf.len() {
        let len = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
        let rec = &buf[i + 4..(i + 4 + len).min(buf.len())];
        let s = String::from_utf8_lossy(rec);
        for kv in s
            .trim_end_matches('\0')
            .split('|')
            .filter(|x| !x.is_empty())
        {
            println!("{n}\t{kv}");
        }
        i += 4 + len;
        n += 1;
    }
}
