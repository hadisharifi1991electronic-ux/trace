//! `streams <file>`: lists the streams of an OLE compound file with sizes
//! and the first bytes of each.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let mut f = cfb::open(&p).unwrap();
    let entries: Vec<(String, bool, u64)> = f
        .walk()
        .map(|e| (e.path().display().to_string(), e.is_stream(), e.len()))
        .collect();
    for (path, stream, len) in entries {
        if !stream {
            println!("{path}/");
            continue;
        }
        let mut s = f.open_stream(&path).unwrap();
        let mut buf = Vec::new();
        use std::io::Read;
        s.read_to_end(&mut buf).unwrap();
        let head: String = buf
            .iter()
            .take(48)
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    *b as char
                } else {
                    '.'
                }
            })
            .collect();
        let hex: String = buf
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{path:40} {len:9}  {hex}  {head}");
    }
}
