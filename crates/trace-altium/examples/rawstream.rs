//! `rawstream <file> <stream>`: hex dump of a whole stream.
use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut f = cfb::open(&a[0]).unwrap();
    let mut buf = Vec::new();
    f.open_stream(&a[1]).unwrap().read_to_end(&mut buf).unwrap();
    let max: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(buf.len());
    for (k, c) in buf[..max.min(buf.len())].chunks(32).enumerate() {
        let hex: Vec<String> = c.iter().map(|b| format!("{b:02x}")).collect();
        let txt: String = c
            .iter()
            .map(|&b| {
                if (32..127).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        println!("{:5} {} {}", k * 32, hex.join(" "), txt);
    }
}
