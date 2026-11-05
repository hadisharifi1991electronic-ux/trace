//! `ruleparams <file> <kind>...`: every parameter of rules of these kinds.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let b = trace_altium::read_pcbdoc(&a[0]).unwrap();
    for r in &b.rules {
        if a[1..].iter().any(|k| k == &r.kind) {
            println!("{} {} {:?} enabled={}", r.kind, r.name, r.scope, r.enabled);
            for (k, v) in &r.params {
                println!("    {k}={v}");
            }
        }
    }
    println!(
        "net classes: {:?}",
        b.net_classes
            .iter()
            .map(|c| (&c.name, c.members.len()))
            .collect::<Vec<_>>()
    );
    println!(
        "pair classes: {:?}",
        b.diff_pair_classes
            .iter()
            .map(|c| (&c.name, c.members.len()))
            .collect::<Vec<_>>()
    );
}
