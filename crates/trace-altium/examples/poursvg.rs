//! SVG of one pour: the file's copper (blue) and Trace's (red outline), with other copper grey.
use trace_core::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let b = trace_altium::read_pcbdoc(&a[0]).unwrap();
    let i: usize = a[1].parse().unwrap();
    let def = &b.geometry.pours[i];
    let LayerRef::Copper(l) = def.layer else {
        return;
    };
    let xs: Vec<f64> = def.outline.iter().map(|p| p.x.mil()).collect();
    let ys: Vec<f64> = def.outline.iter().map(|p| p.y.mil()).collect();
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::MAX, f64::min) - 50.0,
        xs.iter().cloned().fold(f64::MIN, f64::max) + 50.0,
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f64::MAX, f64::min) - 50.0,
        ys.iter().cloned().fold(f64::MIN, f64::max) + 50.0,
    );
    let path = |r: &[Point]| {
        let mut s = String::new();
        for (k, p) in r.iter().enumerate() {
            s += &format!(
                "{}{:.2},{:.2} ",
                if k == 0 { "M" } else { "L" },
                p.x.mil(),
                -p.y.mil()
            );
        }
        s + "Z "
    };
    let poly = |f: &Polygon| {
        let mut s = path(&f.outline);
        for h in &f.holes {
            s += &path(h);
        }
        s
    };
    println!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='{} {} {} {}' width='1600' style='background:#111'>", x0, -y1, x1 - x0, y1 - y0);
    for t in b
        .geometry
        .tracks
        .iter()
        .filter(|t| t.layer == LayerRef::Copper(l))
    {
        println!("<line x1='{}' y1='{}' x2='{}' y2='{}' stroke='#777' stroke-width='{}' stroke-linecap='round'/>", t.start.x.mil(), -t.start.y.mil(), t.end.x.mil(), -t.end.y.mil(), t.width.mil());
    }
    for p in b
        .geometry
        .pads
        .iter()
        .filter(|p| p.layer == LayerRef::Copper(l) || p.layer == LayerRef::AllCopper)
    {
        println!(
            "<circle cx='{}' cy='{}' r='{}' fill='#777'/>",
            p.at.x.mil(),
            -p.at.y.mil(),
            p.size_x.mil().min(p.size_y.mil()) / 2.0
        );
    }
    for f in b
        .geometry
        .polygons
        .iter()
        .filter(|f| f.pour == Some(i as u32))
    {
        println!(
            "<path d='{}' fill='#2a6fdb' fill-opacity='0.6' fill-rule='evenodd'/>",
            poly(f)
        );
    }
    for f in trace_core::pour::pour(&b, i) {
        println!(
            "<path d='{}' fill='none' stroke='#ff4040' stroke-width='1.5' fill-rule='evenodd'/>",
            poly(&f)
        );
    }
    println!("</svg>");
}
