use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut c = cfb::open(&a[1]).unwrap();
    let mut s = c.open_stream(&a[2]).unwrap();
    let mut b = Vec::new();
    s.read_to_end(&mut b).unwrap();
    std::fs::write(&a[3], &b).unwrap();
}
