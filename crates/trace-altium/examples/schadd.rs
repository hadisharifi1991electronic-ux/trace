//! `schadd <file.SchDoc> [stream]`: records of a schematic side stream.
use std::io::Read;
use trace_altium::records::{split_sch, SchRecord};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut f = cfb::open(&a[0]).unwrap();
    let mut buf = Vec::new();
    let s = a.get(1).map(String::as_str).unwrap_or("/Additional");
    f.open_stream(s).unwrap().read_to_end(&mut buf).unwrap();
    for (k, r) in split_sch(&buf).iter().enumerate() {
        match r {
            SchRecord::Text(p) => {
                let kv: Vec<String> = p.keys().map(|(k, v)| format!("{k}={v}")).collect();
                println!("#{k}: {}", kv.join(" "));
            }
            SchRecord::Binary(b) => println!("#{k}: bin {}", b.len()),
        }
    }
}
