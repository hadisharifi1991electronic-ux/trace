//! Saves a PcbDoc over itself, with or without some edits, reads the result
//! back and reports any difference. `roundtrip <in.PcbDoc> <out.PcbDoc> [edit]`
use std::time::Instant;
use trace_core::place::{place, Placement};
use trace_core::*;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (src, dst) = (&a[1], &a[2]);
    let edit = a.get(3).is_some_and(|s| s == "edit");
    let repour = a.get(3).is_some_and(|s| s == "repour");
    let t = Instant::now();
    let mut b = trace_altium::read_pcbdoc(src).unwrap();
    println!("read in {:?}", t.elapsed());
    if edit {
        edits(&mut b);
    }
    if repour {
        // Pour the first few pours again, as Trace pours them.
        let n = b.geometry.pours.len().min(5);
        for i in 0..n {
            if b.geometry.pours[i].outline.is_empty() {
                continue;
            }
            let fresh = trace_core::pour::pour(&b, i);
            b.geometry.polygons.retain(|p| p.pour != Some(i as u32));
            println!("pour {i}: {} areas", fresh.len());
            b.geometry.polygons.extend(fresh);
        }
    }
    let t = Instant::now();
    let (bytes, rep) =
        trace_export::to_altium_over(std::path::Path::new(src), &b, "out.PcbDoc").unwrap();
    println!("wrote in {:?}: {}", t.elapsed(), rep.summary());
    std::fs::write(dst, &bytes).unwrap();
    let back = trace_altium::read_pcbdoc(dst).unwrap();
    let d = trace_export::altium_rt::compare(&b, &back);
    if d.is_empty() {
        println!("read back: same board");
    }
    for l in d {
        println!("DIFF {l}");
    }
}

fn edits(b: &mut Board) {
    let n = b.components.len();
    // Move and turn one part, flip another (pick parts that own pads).
    let with_pads = |b: &Board, k: usize| {
        b.geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(k as u32))
            .count()
    };
    let parts: Vec<usize> = (0..n).filter(|&k| with_pads(b, k) >= 2).collect();
    let (pm, pf) = (parts[parts.len() / 3], parts[parts.len() / 2]);
    let from = Placement::of(b, pm).unwrap();
    let to = Placement {
        x: Length(from.x.0 + Length::from_mil(50.0).0),
        y: Length(from.y.0 - Length::from_mil(25.0).0),
        rotation_deg: from.rotation_deg + 90.0,
        ..from
    };
    place(b, pm, from, to);
    let from = Placement::of(b, pf).unwrap();
    let to = Placement {
        side: if from.side == Side::Top {
            Side::Bottom
        } else {
            Side::Top
        },
        ..from
    };
    place(b, pf, from, to);
    println!(
        "moved {} ({} pads), flipped {} ({} pads)",
        b.components[pm].designator,
        with_pads(b, pm),
        b.components[pf].designator,
        with_pads(b, pf)
    );
    // Remove some routing, add some.
    let g = &mut b.geometry;
    let free: Vec<usize> = (0..g.tracks.len())
        .filter(|&i| g.tracks[i].component.is_none() && !g.tracks[i].text)
        .collect();
    for &i in free.iter().rev().take(10) {
        g.tracks.remove(i);
    }
    let mut t = g.tracks[free[0]].clone();
    t.start.x.0 += 100_000;
    t.end.x.0 += 100_000;
    g.tracks.push(t);
    if !g.vias.is_empty() {
        let mut v = g.vias[0].clone();
        v.at.x.0 += 200_000;
        g.vias.push(v);
        g.vias.remove(1);
    }
    if !g.arcs.is_empty() {
        g.arcs.remove(0);
    }
    // A new net on one pad, and a pad moved to another net.
    b.nets.push(Net {
        name: "TRACE_NEW_NET".into(),
        ..Default::default()
    });
    let nn = b.nets.len() as u32 - 1;
    let pads: Vec<usize> = (0..b.geometry.pads.len())
        .filter(|&i| b.geometry.pads[i].net.is_some())
        .collect();
    b.geometry.pads[pads[0]].net = Some(nn);
    let other = b.geometry.pads[pads[pads.len() - 1]].net;
    b.geometry.pads[pads[1]].net = other;
    // A new part: a copy of the moved part, 500 mil to the right.
    let mut c = b.components[pm].clone();
    c.designator = "TRACE1".into();
    c.x.0 += Length::from_mil(500.0).0;
    b.components.push(c);
    let k = b.components.len() as u32 - 1;
    let d = Length::from_mil(500.0).0;
    let new_pads: Vec<Pad> = b
        .geometry
        .pads
        .iter()
        .filter(|p| p.component == Some(pm as u32))
        .map(|p| {
            let mut p = p.clone();
            p.component = Some(k);
            p.net = None;
            p.at.x.0 += d;
            for q in &mut p.outline {
                q.x.0 += d;
            }
            p
        })
        .collect();
    b.geometry.pads.extend(new_pads);
}
