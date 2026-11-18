use super::*;
use crate::*;

fn p(x: f64, y: f64) -> Point {
    Point {
        x: Length::from_mil(x),
        y: Length::from_mil(y),
    }
}

fn track(net: u32, a: (f64, f64), b: (f64, f64), w: f64) -> Track {
    Track {
        component: None,
        layer: LayerRef::Copper(0),
        net: Some(net),
        start: p(a.0, a.1),
        end: p(b.0, b.1),
        width: Length::from_mil(w),
        text: false,
    }
}

fn pad(x: f64, y: f64, net: u32, size: f64) -> Pad {
    Pad {
        layer: LayerRef::Copper(0),
        net: Some(net),
        component: None,
        name: String::new(),
        at: p(x, y),
        size_x: Length::from_mil(size),
        size_y: Length::from_mil(size),
        hole: Length(0),
        shape: PadShape::Round,
        rotation_deg: 0.0,
        outline: Vec::new(),
        stack: Vec::new(),
        fab: Default::default(),
    }
}

fn via(net: u32, x: f64, y: f64) -> Via {
    Via {
        net: Some(net),
        from_layer: 0,
        to_layer: 1,
        at: p(x, y),
        diameter: Length::from_mil(20.0),
        hole: Length::from_mil(10.0),
        rings: Vec::new(),
        fab: Default::default(),
    }
}

fn board(tracks: Vec<Track>, pads: Vec<Pad>, vias: Vec<Via>, nets: usize) -> Board {
    let mut b = Board::default();
    for name in ["Top", "Bottom"] {
        b.stackup.layers.push(StackLayer {
            name: name.into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length(0),
            dielectric_constant: None,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        });
    }
    for i in 0..nets {
        b.nets.push(Net {
            name: format!("N{i}"),
            ..Default::default()
        });
    }
    b.geometry.tracks = tracks;
    b.geometry.pads = pads;
    b.geometry.vias = vias;
    b
}

/// The board after the push, with the route's own tracks put in.
fn after(b: &Board, s: &Pushed, route: &[Track]) -> Board {
    let mut nb = b.clone();
    nb.geometry.tracks.extend_from_slice(route);
    for e in s.edits(b, nb.geometry.tracks.len()) {
        e.apply(&mut nb);
    }
    nb
}

/// The smallest gap between copper of different nets (tracks, vias and
/// round pads), all layers taken as one.
fn worst_gap(b: &Board) -> f64 {
    let mut caps: Vec<(P, P, f64, Option<u32>)> = b
        .geometry
        .tracks
        .iter()
        .map(|t| (pt(t.start), pt(t.end), t.width.mil() / 2.0, t.net))
        .collect();
    for v in &b.geometry.vias {
        caps.push((pt(v.at), pt(v.at), v.diameter.mil() / 2.0, v.net));
    }
    for q in &b.geometry.pads {
        caps.push((pt(q.at), pt(q.at), q.size_x.mil() / 2.0, q.net));
    }
    let mut worst = f64::MAX;
    for i in 0..caps.len() {
        for j in i + 1..caps.len() {
            let (a, c) = (&caps[i], &caps[j]);
            if a.3 == c.3 {
                continue;
            }
            worst = worst.min(seg_seg(a.0, a.1, c.0, c.1) - a.2 - c.2);
        }
    }
    worst
}

/// Track ends of `net` still meet: every end is shared with another
/// track or sits on a via or pad, apart from the `free` ends.
fn joined(b: &Board, net: u32, free: &[P]) {
    let g = &b.geometry;
    let ends: Vec<P> = g
        .tracks
        .iter()
        .filter(|t| t.net == Some(net))
        .flat_map(|t| [pt(t.start), pt(t.end)])
        .collect();
    for &e in &ends {
        let n = ends.iter().filter(|&&f| dist(e, f) < 1e-3).count();
        let on_via = g
            .vias
            .iter()
            .any(|v| v.net == Some(net) && dist(pt(v.at), e) < 1e-3);
        let on_pad = g
            .pads
            .iter()
            .any(|q| q.net == Some(net) && dist(pt(q.at), e) < 1e-3);
        let is_free = free.iter().any(|&f| dist(f, e) < 1e-3);
        assert!(
            n >= 2 || on_via || on_pad || is_free,
            "net {net}: loose end {e:?}"
        );
    }
}

const CL: f64 = 5.0;

#[test]
fn pushes_a_chain_of_tracks() {
    // Three tracks, 6 wide, 11 apart centre to centre (exactly 5 mil
    // clearance), and a route along y = 0 too close to the first.
    let tracks = vec![
        track(1, (0.0, 10.0), (400.0, 10.0), 6.0),
        track(2, (0.0, 21.0), (400.0, 21.0), 6.0),
        track(3, (0.0, 32.0), (400.0, 32.0), 6.0),
    ];
    let b = board(tracks, Vec::new(), Vec::new(), 4);
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [150.0, 0.0],
        [250.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    let s = push(&b, &obs, CL, &route, &[]).expect("pushed");
    assert_eq!(s.remove, vec![0, 1, 2], "all three move");
    let nb = after(&b, &s, &[track(0, (150.0, 0.0), (250.0, 0.0), 6.0)]);
    let g = worst_gap(&nb);
    assert!(g >= CL - 1e-3, "{g}");
    for (n, y) in [(1, 10.0), (2, 21.0), (3, 32.0)] {
        joined(&nb, n, &[[0.0, y], [400.0, y]]);
    }
    // Hugging: the first track moves out no further than it has to, give
    // or take a fine grid step.
    let lift = nb
        .geometry
        .tracks
        .iter()
        .filter(|t| t.net == Some(1))
        .map(|t| t.start.y.mil().max(t.end.y.mil()))
        .fold(0.0, f64::max);
    assert!(lift <= 3.0 + 5.0 + 3.0 + 4.0, "{lift}");
    // Far from the route, tracks are where they were.
    for t in nb.geometry.tracks.iter().filter(|t| t.net == Some(3)) {
        if t.start.x.mil() < 40.0 {
            assert!((t.start.y.mil() - 32.0).abs() < 1e-6);
        }
    }
}

#[test]
fn pushes_a_via_and_its_tracks_follow() {
    // A via of net 1 just above the route, with a track from it to a pad.
    let tracks = vec![track(1, (200.0, 15.0), (200.0, 200.0), 6.0)];
    let pads = vec![pad(200.0, 200.0, 1, 30.0)];
    let b = board(tracks, pads, vec![via(1, 200.0, 15.0)], 2);
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [100.0, 0.0],
        [300.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    let s = push(&b, &obs, CL, &route, &[]).expect("pushed");
    assert_eq!(s.vias.len(), 1);
    let (_, c) = s.vias[0];
    assert!(c[1] >= 3.0 + 10.0 + CL - 1e-3, "{c:?}");
    let nb = after(&b, &s, &[track(0, (100.0, 0.0), (300.0, 0.0), 6.0)]);
    assert!(worst_gap(&nb) >= CL - 1e-3, "{}", worst_gap(&nb));
    joined(&nb, 1, &[]);
    // The via's track still starts on the via.
    assert!(nb
        .geometry
        .tracks
        .iter()
        .filter(|t| t.net == Some(1))
        .any(|t| dist(pt(t.start), c) < 1e-3 || dist(pt(t.end), c) < 1e-3));
}

#[test]
fn a_via_pushes_tracks_on_both_layers() {
    let mut bottom = track(2, (0.0, 12.0), (300.0, 12.0), 6.0);
    bottom.layer = LayerRef::Copper(1);
    let tracks = vec![track(1, (0.0, -12.0), (300.0, -12.0), 6.0), bottom];
    let b = board(tracks, Vec::new(), Vec::new(), 3);
    let obs = Obstacles::new(&b);
    // A new through via of net 0 at (150, 0), between them.
    let v = [Pusher::via([150.0, 0.0], 20.0, 0, 1, &[Some(0)])];
    let s = push(&b, &obs, CL, &v, &[]).expect("pushed");
    assert_eq!(s.remove, vec![0, 1]);
    let mut nb = after(&b, &s, &[]);
    nb.geometry.vias.push(via(0, 150.0, 0.0));
    for l in 0..2u8 {
        let mut one = nb.clone();
        one.geometry
            .tracks
            .retain(|t| t.layer == LayerRef::Copper(l));
        assert!(
            worst_gap(&one) >= CL - 1e-3,
            "layer {l}: {}",
            worst_gap(&one)
        );
    }
}

#[test]
fn a_pad_in_the_way_cannot_be_pushed() {
    let b = board(Vec::new(), vec![pad(200.0, 12.0, 1, 20.0)], Vec::new(), 2);
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [100.0, 0.0],
        [300.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    assert!(push(&b, &obs, CL, &route, &[]).is_none());
}

#[test]
fn a_track_is_never_pushed_into_a_pad() {
    // Track of net 1 at y = 10, a pad of net 2 just above it: the track
    // has to go round the pad, or the push fails; never through it.
    let tracks = vec![track(1, (0.0, 10.0), (400.0, 10.0), 6.0)];
    let pads = vec![pad(200.0, 30.0, 2, 20.0)];
    let b = board(tracks, pads, Vec::new(), 3);
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [150.0, 0.0],
        [250.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    if let Some(s) = push(&b, &obs, CL, &route, &[]) {
        let nb = after(&b, &s, &[track(0, (150.0, 0.0), (250.0, 0.0), 6.0)]);
        assert!(worst_gap(&nb) >= CL - 1e-3, "{}", worst_gap(&nb));
    }
}

#[test]
fn a_pair_moves_together_and_keeps_its_gap() {
    // Pair N1/N2: 6 wide, 12 centre to centre.
    let tracks = vec![
        track(1, (0.0, 10.0), (400.0, 10.0), 6.0),
        track(2, (0.0, 22.0), (400.0, 22.0), 6.0),
    ];
    let mut b = board(tracks, Vec::new(), Vec::new(), 3);
    b.diff_pairs.push(DiffPair {
        name: "P".into(),
        positive: "N1".into(),
        negative: "N2".into(),
    });
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [150.0, 0.0],
        [250.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    let s = push(&b, &obs, CL, &route, &[]).expect("pushed");
    assert_eq!(s.remove, vec![0, 1]);
    let nb = after(&b, &s, &[track(0, (150.0, 0.0), (250.0, 0.0), 6.0)]);
    let rt = nb
        .geometry
        .tracks
        .iter()
        .find(|t| t.net == Some(0))
        .unwrap()
        .clone();
    for t in nb.geometry.tracks.iter().filter(|t| t.net != Some(0)) {
        let g = seg_seg(pt(t.start), pt(t.end), pt(rt.start), pt(rt.end)) - 6.0;
        assert!(g >= CL - 1e-3, "{g}");
    }
    // Every N1 segment stays 12 from the nearest N2 track: the gap is
    // kept, not squeezed or opened.
    let n2: Vec<&Track> = nb
        .geometry
        .tracks
        .iter()
        .filter(|t| t.net == Some(2))
        .collect();
    for t in nb.geometry.tracks.iter().filter(|t| t.net == Some(1)) {
        let d = n2
            .iter()
            .map(|u| seg_seg(pt(t.start), pt(t.end), pt(u.start), pt(u.end)))
            .fold(f64::MAX, f64::min);
        assert!((d - 12.0).abs() < 1.5, "{d}");
    }
    joined(&nb, 1, &[[0.0, 10.0], [400.0, 10.0]]);
    joined(&nb, 2, &[[0.0, 22.0], [400.0, 22.0]]);
}

#[test]
fn a_long_line_moves_only_near_the_route() {
    // A line of three segments; the route is beside the middle one only.
    let tracks = vec![
        track(1, (0.0, 10.0), (300.0, 10.0), 6.0),
        track(1, (300.0, 10.0), (600.0, 10.0), 6.0),
        track(1, (600.0, 10.0), (900.0, 10.0), 6.0),
    ];
    let b = board(tracks, Vec::new(), Vec::new(), 2);
    let obs = Obstacles::new(&b);
    let route = [Pusher::track(
        0,
        [400.0, 0.0],
        [500.0, 0.0],
        3.0,
        &[Some(0)],
    )];
    let s = push(&b, &obs, CL, &route, &[]).expect("pushed");
    let nb = after(&b, &s, &[track(0, (400.0, 0.0), (500.0, 0.0), 6.0)]);
    assert!(worst_gap(&nb) >= CL - 1e-3);
    joined(&nb, 1, &[[0.0, 10.0], [900.0, 10.0]]);
    for t in nb.geometry.tracks.iter().filter(|t| t.net == Some(1)) {
        for q in [t.start, t.end] {
            if q.x.mil() < 250.0 || q.x.mil() > 650.0 {
                assert!((q.y.mil() - 10.0).abs() < 1e-6, "{q:?}");
            }
        }
    }
}

#[test]
fn copper_already_tight_on_the_board_does_not_block_a_push() {
    // Both tracks pass the route's start pad closer than the clearance,
    // as drawn on the board; the route hugs the pad below and pushes them.
    let tracks = vec![
        track(1, (-100.0, 10.0), (400.0, 10.0), 6.0),
        track(2, (-100.0, 21.0), (400.0, 21.0), 6.0),
    ];
    let pads = vec![pad(0.0, 0.0, 0, 8.0), pad(150.0, -14.0, 3, 16.0)];
    let b = board(tracks, pads, Vec::new(), 4);
    let obs = Obstacles::new(&b);
    let ask = crate::walk::Ask {
        obstacles: &obs,
        layer: 0,
        r: 3.0,
        nets: &[Some(0)],
        clearance: CL,
        extra: &[],
        skip: &[],
        see_through: true,
        deadline: None,
    };
    let path = crate::walk::walkaround(&ask, [0.0, 0.0], [300.0, 0.0]).expect("hugs the pad");
    let route: Vec<Pusher> = path
        .windows(2)
        .map(|w| Pusher::track(0, w[0], w[1], 3.0, &[Some(0)]))
        .collect();
    let s = push(&b, &obs, CL, &route, &[]).expect("pushed");
    assert_eq!(s.remove, vec![0, 1]);
}
