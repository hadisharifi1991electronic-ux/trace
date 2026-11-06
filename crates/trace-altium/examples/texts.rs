//! `texts <file.PcbDoc>`: the text records of a binary board, raw.
use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut f = cfb::open(&a[0]).unwrap();
    let mut buf = Vec::new();
    f.open_stream("/Texts6/Data")
        .unwrap()
        .read_to_end(&mut buf)
        .unwrap();
    let recs = trace_altium::records::split_binary(&buf, 2);
    let mut layers = std::collections::BTreeMap::new();
    for r in &recs {
        *layers.entry(r[0][0]).or_insert(0) += 1;
    }
    println!("{} texts, by layer id: {layers:?}", recs.len());
    for r in recs
        .iter()
        .take(a.get(1).and_then(|n| n.parse().ok()).unwrap_or(8))
    {
        let b = r[0];
        let s = r
            .get(1)
            .map(|s| String::from_utf8_lossy(&s[1..]).into_owned())
            .unwrap_or_default();
        let i32_at = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        println!(
            "len {} layer {} comp {} at {:.1},{:.1} h {:.1} w {:.1} rot {:.0} mir {} comment {} desig {} kind {} wide {} text {:?}",
            b.len(), b[0], u16::from_le_bytes([b[7], b[8]]) as i32,
            i32_at(13) as f64 / 1e4, i32_at(17) as f64 / 1e4, i32_at(21) as f64 / 1e4, i32_at(36) as f64 / 1e4,
            f64::from_le_bytes(b[27..35].try_into().unwrap()), b[35], b.get(40).copied().unwrap_or(9), b.get(41).copied().unwrap_or(9), b.get(43).copied().unwrap_or(9),
            if b.len() >= 119 { u32::from_le_bytes(b[115..119].try_into().unwrap()) } else { 0 }, s
        );
    }
}
