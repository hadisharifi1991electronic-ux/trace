//! `recs <file> <stream> [n]`: hex of the first n binary records of a stream
//! (type byte, then length-prefixed subrecords).
use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(1);
    let subs: usize = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(1);
    let mut f = cfb::open(&a[0]).unwrap();
    let mut buf = Vec::new();
    f.open_stream(&a[1]).unwrap().read_to_end(&mut buf).unwrap();
    let mut i = 0;
    for r in 0..n {
        if i >= buf.len() {
            break;
        }
        let t = buf[i];
        i += 1;
        println!("record {r} type {t}");
        for s in 0..subs {
            let len = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            i += 4;
            let d = &buf[i..i + len];
            println!("  sub {s} len {len}");
            for (k, c) in d.chunks(32).enumerate() {
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
                println!("    {:4} {} {}", k * 32, hex.join(" "), txt);
            }
            i += len;
        }
    }
}
