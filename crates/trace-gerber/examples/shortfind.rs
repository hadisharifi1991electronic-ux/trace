//! `shortfind <named> <test>`: traces `test` with the net names of `named`
//! (objects matched by position) and prints where two named nets meet.
use std::collections::HashMap;
use trace_core::{Pad, Polygon, Track, Via};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let src = trace_gerber::read_path(&a[0]).unwrap();
    let mut truth = src.board;
    if std::env::var_os("NETCHECK_RETRACE").is_some() {
        let g = &mut truth.geometry;
        g.tracks.iter_mut().for_each(|x| x.net = None);
        g.arcs.iter_mut().for_each(|x| x.net = None);
        g.pads.iter_mut().for_each(|x| x.net = None);
        g.vias.iter_mut().for_each(|x| x.net = None);
        g.polygons.iter_mut().for_each(|x| x.net = None);
        truth.nets.clear();
        trace_gerber::nets::trace(&mut truth, &src.trace_input);
    }
    let imp = trace_gerber::read_path(&a[1]).unwrap();
    let mut t = imp.board.clone();
    let r = |v: i64| (v as f64 / 10000.0).round() as i64;
    let tk = |x: &Track| {
        (
            format!("{:?}", x.layer),
            r(x.start.x.0),
            r(x.start.y.0),
            r(x.end.x.0),
            r(x.end.y.0),
        )
    };
    let vk = |v: &Via| (r(v.at.x.0), r(v.at.y.0), v.from_layer, v.to_layer);
    let pk = |p: &Pad| (format!("{:?}", p.layer), r(p.at.x.0), r(p.at.y.0));
    let qk = |q: &Polygon| {
        let xs = q.outline.iter().map(|p| p.x.0);
        let ys = q.outline.iter().map(|p| p.y.0);
        let (x0, x1) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
        let (y0, y1) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
        (
            format!("{:?}", q.layer),
            (x0 + x1) / 20000,
            (y0 + y1) / 20000,
            (x1 - x0) / 10000,
            (y1 - y0) / 10000,
        )
    };
    let g = &truth.geometry;
    let tn: HashMap<_, _> = g.tracks.iter().map(|x| (tk(x), x.net)).collect();
    let vn: HashMap<_, _> = g.vias.iter().map(|x| (vk(x), x.net)).collect();
    let pn: HashMap<_, _> = g.pads.iter().map(|x| (pk(x), x.net)).collect();
    let qn: HashMap<_, _> = g.polygons.iter().map(|x| (qk(x), x.net)).collect();
    t.nets = truth.nets.clone();
    let mut miss = [0; 4];
    for x in &mut t.geometry.tracks {
        x.net = tn.get(&tk(x)).copied().flatten();
        miss[0] += x.net.is_none() as usize;
    }
    for x in &mut t.geometry.vias {
        x.net = vn.get(&vk(x)).copied().flatten();
        miss[1] += x.net.is_none() as usize;
    }
    for x in &mut t.geometry.pads {
        x.net = pn.get(&pk(x)).copied().flatten();
        miss[2] += x.net.is_none() as usize;
    }
    for x in &mut t.geometry.polygons {
        x.net = qn.get(&qk(x)).copied().flatten();
        miss[3] += x.net.is_none() as usize;
    }
    for x in &mut t.geometry.arcs {
        x.net = None;
    }
    eprintln!(
        "unnamed: tracks {} vias {} pads {} pours {}",
        miss[0], miss[1], miss[2], miss[3]
    );
    std::env::set_var("TRACE_NET_DEBUG", "1");
    trace_gerber::nets::trace(&mut t, &imp.trace_input);
}
