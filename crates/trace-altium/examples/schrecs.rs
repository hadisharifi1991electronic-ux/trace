//! `schrecs <file.SchDoc> [RECORD [n]]`: record histogram of a schematic, or
//! the first n records of one type.
use std::collections::BTreeMap;
use std::io::Read;
use trace_altium::records::{split_sch, SchRecord};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut f = cfb::open(&a[0]).unwrap();
    for e in f.walk() {
        if e.is_stream() {
            println!("stream {} {}", e.path().display(), e.len());
        }
    }
    let mut buf = Vec::new();
    f.open_stream("/FileHeader")
        .unwrap()
        .read_to_end(&mut buf)
        .unwrap();
    let recs = split_sch(&buf);
    let want: Option<i64> = a.get(1).map(|s| s.parse().unwrap());
    let n: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(3);
    let mut hist = BTreeMap::new();
    let mut shown = 0;
    for (k, r) in recs.iter().enumerate() {
        let id = match r {
            SchRecord::Text(p) => p.int("RECORD").unwrap_or(-1),
            SchRecord::Binary(b) => 1000 + b[0] as i64,
        };
        *hist.entry(id).or_insert(0) += 1;
        if Some(id) == want && shown < n {
            shown += 1;
            match r {
                SchRecord::Text(p) => {
                    let kv: Vec<String> = p.keys().map(|(k, v)| format!("{k}={v}")).collect();
                    println!("#{k}: {}", kv.join(" "));
                }
                SchRecord::Binary(b) => println!("#{k}: {:02x?}", &b[..b.len().min(80)]),
            }
        }
    }
    if want.is_none() {
        println!("{hist:?}");
    }
}
