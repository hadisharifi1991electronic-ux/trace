//! Drawing and editing shapes: pours, fixed copper regions, keepouts, pour
//! cutouts and the board outline. A shape is picked by its edge or by the
//! point inside it, its corners moved, added or taken away, the whole of it
//! moved or deleted, and a pour's net and connection style changed.
//!
//! Edits swap the board's shapes as a whole (`Edit::Shapes`); a pour whose
//! outline or settings change loses its copper, which pouring again (after
//! the edit) puts back.

use crate::edit::{Edit, Shapes};
use crate::{Board, LayerRef, Length, Point, Polygon, Pour};

type P = [f64; 2];

fn mil(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn pt(p: P) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

/// What kind of shape to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A pour: an outline filled with copper of its net, kept clear of
    /// other nets, poured again after edits.
    Pour,
    /// Solid copper exactly as drawn (Altium's region, Allegro's static
    /// shape).
    Region,
    /// No copper, tracks or vias inside, on every layer.
    Keepout,
    /// A hole in the pours of one layer.
    Cutout,
    /// The board's edge.
    Outline,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Pour,
        Kind::Region,
        Kind::Keepout,
        Kind::Cutout,
        Kind::Outline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Pour => "Pour",
            Kind::Region => "Copper region",
            Kind::Keepout => "Keepout",
            Kind::Cutout => "Pour cutout",
            Kind::Outline => "Board outline",
        }
    }
}

/// A shape on the board, by where it is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeRef {
    Pour(usize),
    /// A copper area that no pour, part or teardrop made: a region or a
    /// keepout.
    Area(usize),
    Cutout(usize),
    Outline,
}

/// Whether a copper area is one drawn on its own (not poured copper, not
/// part of a footprint, not a teardrop).
fn drawn(p: &Polygon) -> bool {
    p.pour.is_none() && p.component.is_none() && !p.teardrop
}

/// A ring without the repeated closing point some sources keep.
fn open(mut r: Vec<P>) -> Vec<P> {
    while r.len() > 3 {
        let (a, b) = (r[0], r[r.len() - 1]);
        if (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6 {
            r.pop();
        } else {
            break;
        }
    }
    r
}

fn bbox(r: &[P]) -> [f64; 4] {
    r.iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        })
}

fn signed_area(r: &[P]) -> f64 {
    let n = r.len();
    (0..n)
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

fn contains(r: &[P], p: P) -> bool {
    let mut inside = false;
    let n = r.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The point of segment a-b nearest to p, and how far it is.
fn nearest_on(a: P, b: P, p: P) -> (P, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + t * dx, a[1] + t * dy];
    (q, (q[0] - p[0]).hypot(q[1] - p[1]))
}

/// The corner of `ring` within `tol` of `p`, nearest first.
pub fn corner_at(ring: &[P], p: P, tol: f64) -> Option<usize> {
    ring.iter()
        .enumerate()
        .map(|(i, q)| (i, (q[0] - p[0]).hypot(q[1] - p[1])))
        .filter(|(_, d)| *d <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The edge of `ring` within `tol` of `p`: its index (from corner i to
/// i + 1) and the point on it.
pub fn edge_at(ring: &[P], p: P, tol: f64) -> Option<(usize, P)> {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (q, d) = nearest_on(ring[i], ring[(i + 1) % n], p);
            (i, q, d)
        })
        .filter(|t| t.2 <= tol)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, q, _)| (i, q))
}

impl ShapeRef {
    /// The shape's outline (mil), without a repeated closing point.
    pub fn ring(&self, board: &Board) -> Option<Vec<P>> {
        let g = &board.geometry;
        let pts: &[Point] = match *self {
            ShapeRef::Pour(i) => &g.pours.get(i)?.outline,
            ShapeRef::Area(i) => &g.polygons.get(i)?.outline,
            ShapeRef::Cutout(i) => &g.cutouts.get(i)?.outline,
            ShapeRef::Outline => &g.outline,
        };
        (pts.len() >= 3).then(|| open(pts.iter().map(|&q| mil(q)).collect()))
    }

    pub fn kind(&self, board: &Board) -> Kind {
        match *self {
            ShapeRef::Pour(_) => Kind::Pour,
            ShapeRef::Area(i) => match board.geometry.polygons.get(i).map(|p| &p.layer) {
                Some(LayerRef::Keepout) => Kind::Keepout,
                _ => Kind::Region,
            },
            ShapeRef::Cutout(_) => Kind::Cutout,
            ShapeRef::Outline => Kind::Outline,
        }
    }

    pub fn layer(&self, board: &Board) -> Option<LayerRef> {
        let g = &board.geometry;
        match *self {
            ShapeRef::Pour(i) => g.pours.get(i).map(|p| p.layer.clone()),
            ShapeRef::Area(i) => g.polygons.get(i).map(|p| p.layer.clone()),
            ShapeRef::Cutout(i) => g.cutouts.get(i).map(|p| p.layer.clone()),
            ShapeRef::Outline => None,
        }
    }

    pub fn net(&self, board: &Board) -> Option<u32> {
        let g = &board.geometry;
        match *self {
            ShapeRef::Pour(i) => g.pours.get(i).and_then(|p| p.net),
            ShapeRef::Area(i) => g.polygons.get(i).and_then(|p| p.net),
            _ => None,
        }
    }

    /// "Pour GND on TOP", for the status line.
    pub fn describe(&self, board: &Board) -> String {
        let kind = self.kind(board).label();
        let net = self
            .net(board)
            .and_then(|n| board.nets.get(n as usize))
            .map(|n| format!(" {}", n.name))
            .unwrap_or_default();
        let layer = match self.layer(board) {
            Some(LayerRef::Copper(l)) => board
                .stackup
                .copper_layers()
                .nth(l as usize)
                .map(|s| format!(" on {}", s.name))
                .unwrap_or_default(),
            Some(LayerRef::AllCopper) => " on every layer".into(),
            _ => String::new(),
        };
        format!("{kind}{net}{layer}")
    }

    /// Whether this is still a shape of the board (after an undo).
    pub fn exists(&self, board: &Board) -> bool {
        let g = &board.geometry;
        match *self {
            ShapeRef::Pour(i) => i < g.pours.len(),
            ShapeRef::Area(i) => g.polygons.get(i).is_some_and(drawn),
            ShapeRef::Cutout(i) => i < g.cutouts.len(),
            ShapeRef::Outline => g.outline.len() >= 3,
        }
    }
}

/// Every shape that can be edited on copper layer `layer` (or any layer
/// when `None`): pours and regions of that layer, keepouts, cutouts and the
/// board outline.
pub fn shapes(board: &Board, layer: Option<u8>) -> Vec<ShapeRef> {
    let g = &board.geometry;
    let on = |l: &LayerRef| match (l, layer) {
        (_, None) => true,
        (LayerRef::Copper(a), Some(b)) => *a == b,
        (LayerRef::AllCopper | LayerRef::Keepout, _) => true,
        _ => false,
    };
    let mut out: Vec<ShapeRef> = Vec::new();
    out.extend(
        (0..g.pours.len())
            .filter(|&i| on(&g.pours[i].layer) && g.pours[i].outline.len() >= 3)
            .map(ShapeRef::Pour),
    );
    out.extend(
        (0..g.polygons.len())
            .filter(|&i| drawn(&g.polygons[i]) && on(&g.polygons[i].layer))
            .map(ShapeRef::Area),
    );
    out.extend(
        (0..g.cutouts.len())
            .filter(|&i| on(&g.cutouts[i].layer))
            .map(ShapeRef::Cutout),
    );
    if g.outline.len() >= 3 {
        out.push(ShapeRef::Outline);
    }
    out
}

/// The shape at `p` (mil): one whose edge passes within `tol`, the nearest
/// edge first; otherwise the smallest one around it (the board outline only
/// by its edge).
pub fn pick(board: &Board, p: P, tol: f64, layer: Option<u8>) -> Option<ShapeRef> {
    let all = shapes(board, layer);
    let rings: Vec<(ShapeRef, Vec<P>)> = all
        .into_iter()
        .filter_map(|s| s.ring(board).map(|r| (s, r)))
        .collect();
    let by_edge = rings
        .iter()
        .filter_map(|(s, r)| {
            let n = r.len();
            (0..n)
                .map(|i| nearest_on(r[i], r[(i + 1) % n], p).1)
                .min_by(f64::total_cmp)
                .filter(|d| *d <= tol)
                .map(|d| (*s, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((s, _)) = by_edge {
        return Some(s);
    }
    rings
        .iter()
        .filter(|(s, r)| *s != ShapeRef::Outline && contains(r, p))
        .min_by(|a, b| signed_area(&a.1).abs().total_cmp(&signed_area(&b.1).abs()))
        .map(|(s, _)| *s)
}

fn grown(b: [f64; 4], by: f64) -> [f64; 4] {
    [b[0] - by, b[1] - by, b[2] + by, b[3] + by]
}

fn join(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// One swap of the board's shapes: `f` changes a copy of them, and `area`
/// is where copper may change.
fn swap(board: &Board, area: Option<[f64; 4]>, f: impl FnOnce(&mut Shapes)) -> Vec<Edit> {
    let before = Shapes::of(board);
    let mut after = before.clone();
    f(&mut after);
    vec![Edit::Shapes {
        before: Box::new(before),
        after: Box::new(after),
        area: area.map(|b| grown(b, 50.0)),
    }]
}

/// Takes pour `i`'s copper away, so pouring again makes it anew.
fn unpour(s: &mut Shapes, i: usize) {
    s.polygons.retain(|p| p.pour != Some(i as u32));
}

/// Gives a shape a new outline (mil).
pub fn set_ring(board: &Board, r: ShapeRef, ring: &[P]) -> Vec<Edit> {
    let ring = open(ring.to_vec());
    if ring.len() < 3 || signed_area(&ring).abs() < 1e-6 {
        return Vec::new();
    }
    let Some(old) = r.ring(board) else {
        return Vec::new();
    };
    let area = join(bbox(&old), bbox(&ring));
    let pts: Vec<Point> = ring.iter().map(|&q| pt(q)).collect();
    swap(board, Some(area), |s| match r {
        ShapeRef::Pour(i) => {
            s.pours[i].outline = pts;
            unpour(s, i);
        }
        ShapeRef::Area(i) => s.polygons[i].outline = pts,
        ShapeRef::Cutout(i) => s.cutouts[i].outline = pts,
        ShapeRef::Outline => s.outline = pts,
    })
}

/// Moves a shape by (dx, dy) mil.
pub fn move_by(board: &Board, r: ShapeRef, dx: f64, dy: f64) -> Vec<Edit> {
    let Some(ring) = r.ring(board) else {
        return Vec::new();
    };
    let moved: Vec<P> = ring.iter().map(|q| [q[0] + dx, q[1] + dy]).collect();
    let mut e = set_ring(board, r, &moved);
    // A region's holes go with it.
    if let (ShapeRef::Area(i), Some(Edit::Shapes { after, .. })) = (r, e.first_mut()) {
        for h in after.polygons[i].holes.iter_mut().flatten() {
            *h = pt([h.x.mil() + dx, h.y.mil() + dy]);
        }
    }
    e
}

/// Deletes a shape (not the board outline).
pub fn delete(board: &Board, r: ShapeRef) -> Vec<Edit> {
    if r == ShapeRef::Outline || !r.exists(board) {
        return Vec::new();
    }
    let area = r.ring(board).map(|q| bbox(&q));
    swap(board, area, |s| match r {
        ShapeRef::Pour(i) => {
            s.pours.remove(i);
            let k = i as u32;
            s.polygons.retain(|p| p.pour != Some(k));
            for p in &mut s.polygons {
                if let Some(j) = p.pour {
                    if j > k {
                        p.pour = Some(j - 1);
                    }
                }
            }
        }
        ShapeRef::Area(i) => {
            s.polygons.remove(i);
        }
        ShapeRef::Cutout(i) => {
            s.cutouts.remove(i);
        }
        ShapeRef::Outline => {}
    })
}

/// Adds a shape drawn as `ring` (mil). `layer` is the copper layer of a
/// pour, region or cutout; `net` that of a pour or region. Returns the
/// edits and where the new shape will be.
pub fn add(
    board: &Board,
    kind: Kind,
    ring: &[P],
    layer: u8,
    net: Option<u32>,
) -> Option<(Vec<Edit>, ShapeRef)> {
    let mut ring = open(ring.to_vec());
    if ring.len() < 3 || signed_area(&ring).abs() < 1.0 {
        return None;
    }
    // Counter-clockwise, as pours and areas are kept.
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    let pts: Vec<Point> = ring.iter().map(|&q| pt(q)).collect();
    let g = &board.geometry;
    let area = Some(bbox(&ring));
    let polygon = |layer: LayerRef, net: Option<u32>| Polygon {
        layer,
        net,
        outline: pts.clone(),
        holes: Vec::new(),
        pour: None,
        component: None,
        teardrop: false,
    };
    Some(match kind {
        Kind::Pour => {
            let order = g.pours.iter().map(|p| p.order).max().map_or(0, |o| o + 1);
            let e = swap(board, area, |s| {
                s.pours.push(Pour {
                    layer: LayerRef::Copper(layer),
                    net,
                    outline: pts.clone(),
                    remove_dead: true,
                    order,
                    min_island: 0.0,
                    connect: None,
                })
            });
            (e, ShapeRef::Pour(g.pours.len()))
        }
        Kind::Region | Kind::Keepout => {
            let p = if kind == Kind::Region {
                polygon(LayerRef::Copper(layer), net)
            } else {
                polygon(LayerRef::Keepout, None)
            };
            let e = swap(board, area, |s| s.polygons.push(p));
            (e, ShapeRef::Area(g.polygons.len()))
        }
        Kind::Cutout => {
            let p = polygon(LayerRef::Copper(layer), None);
            let e = swap(board, area, |s| s.cutouts.push(p));
            (e, ShapeRef::Cutout(g.cutouts.len()))
        }
        Kind::Outline => {
            let old = (g.outline.len() >= 3)
                .then(|| bbox(&open(g.outline.iter().map(|&q| mil(q)).collect())));
            let a = match old {
                Some(o) => join(o, bbox(&ring)),
                None => bbox(&ring),
            };
            let e = swap(board, Some(a), |s| s.outline = pts.clone());
            (e, ShapeRef::Outline)
        }
    })
}

/// Changes a pour's settings (net, layer, connection style, dead copper);
/// its copper goes, to be poured again.
pub fn set_pour(board: &Board, i: usize, f: impl FnOnce(&mut Pour)) -> Vec<Edit> {
    let Some(old) = board.geometry.pours.get(i) else {
        return Vec::new();
    };
    let mut new = old.clone();
    f(&mut new);
    if format!("{new:?}") == format!("{old:?}") {
        return Vec::new();
    }
    let area = ShapeRef::Pour(i).ring(board).map(|r| bbox(&r));
    swap(board, area, |s| {
        s.pours[i] = new;
        unpour(s, i);
    })
}

/// Drops pour `i`'s copper so it is poured afresh.
pub fn pour_again(board: &Board, i: usize) -> Vec<Edit> {
    if i >= board.geometry.pours.len() {
        return Vec::new();
    }
    let area = ShapeRef::Pour(i).ring(board).map(|r| bbox(&r));
    swap(board, area, |s| unpour(s, i))
}

/// Puts a copper region on another net.
pub fn set_area_net(board: &Board, i: usize, net: Option<u32>) -> Vec<Edit> {
    if board.geometry.polygons.get(i).is_none_or(|p| p.net == net) {
        return Vec::new();
    }
    let area = ShapeRef::Area(i).ring(board).map(|r| bbox(&r));
    swap(board, area, |s| s.polygons[i].net = net)
}

/// The ring with corner `k` moved to `to`.
pub fn with_corner(ring: &[P], k: usize, to: P) -> Vec<P> {
    let mut r = ring.to_vec();
    if let Some(c) = r.get_mut(k) {
        *c = to;
    }
    r
}

/// The ring with a corner added after corner `k` (on edge k), at `at`.
pub fn with_new_corner(ring: &[P], k: usize, at: P) -> Vec<P> {
    let mut r = ring.to_vec();
    r.insert((k + 1).min(r.len()), at);
    r
}

/// The ring without corner `k`; `None` if three or fewer are left.
pub fn without_corner(ring: &[P], k: usize) -> Option<Vec<P>> {
    (ring.len() > 3 && k < ring.len()).then(|| {
        let mut r = ring.to_vec();
        r.remove(k);
        r
    })
}

/// A rectangle as a ring, from two opposite corners.
pub fn rect_ring(a: P, b: P) -> Vec<P> {
    vec![a, [b[0], a[1]], b, [a[0], b[1]]]
}

/// Points drawn by hand kept to 45°: `p` moved so the edge from `last`
/// runs along the nearest multiple of 45°.
pub fn snap45(last: P, p: P) -> P {
    let (dx, dy) = (p[0] - last[0], p[1] - last[1]);
    let len = dx.hypot(dy);
    if len < 1e-9 {
        return p;
    }
    let a = (dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    // Keep the reach along the chosen direction.
    let along = dx * a.cos() + dy * a.sin();
    [last[0] + along * a.cos(), last[1] + along * a.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn board() -> Board {
        let mut b = Board::default();
        for n in ["Top", "Bottom"] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length(0),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.nets.push(Net {
            name: "GND".into(),
            ..Default::default()
        });
        // A ground via, so a ground pour round it is not dead copper.
        b.geometry.vias.push(Via {
            net: Some(0),
            from_layer: 0,
            to_layer: 1,
            at: pt([350.0, 250.0]),
            diameter: Length::from_mil(20.0),
            hole: Length::from_mil(10.0),
            rings: Vec::new(),
            fab: Default::default(),
        });
        b.geometry.outline = rect_ring([0.0, 0.0], [1000.0, 1000.0])
            .into_iter()
            .map(pt)
            .collect();
        b
    }

    fn run(b: &Board, e: &[Edit]) -> Board {
        let mut c = b.clone();
        for x in e {
            x.apply(&mut c);
        }
        c
    }

    #[test]
    fn draw_edit_and_delete_a_pour() {
        let b = board();
        let ring = rect_ring([100.0, 100.0], [600.0, 400.0]);
        let (e, r) = add(&b, Kind::Pour, &ring, 0, Some(0)).unwrap();
        let mut b1 = run(&b, &e);
        assert_eq!(r, ShapeRef::Pour(0));
        assert_eq!(b1.geometry.pours.len(), 1);
        // Pouring after the edit fills it.
        if let Some(p) = pour::repour_after(&b1, &e) {
            p.apply(&mut b1);
        }
        assert!(b1.geometry.polygons.iter().any(|p| p.pour == Some(0)));
        // Picked by its edge and from inside.
        assert_eq!(pick(&b1, [350.0, 101.0], 3.0, Some(0)), Some(r));
        assert_eq!(pick(&b1, [350.0, 250.0], 3.0, Some(0)), Some(r));
        assert_eq!(pick(&b1, [350.0, 250.0], 3.0, Some(1)), None);
        assert_eq!(
            pick(&b1, [0.5, 500.0], 3.0, Some(0)),
            Some(ShapeRef::Outline)
        );
        // A corner dragged: the copper goes and comes back poured again.
        let ring = r.ring(&b1).unwrap();
        let k = corner_at(&ring, [600.0, 400.0], 5.0).unwrap();
        let moved = with_corner(&ring, k, [800.0, 700.0]);
        let e2 = set_ring(&b1, r, &moved);
        let mut b2 = run(&b1, &e2);
        assert!(!b2.geometry.polygons.iter().any(|p| p.pour == Some(0)));
        if let Some(p) = pour::repour_after(&b2, &e2) {
            p.apply(&mut b2);
        }
        let poured = b2
            .geometry
            .polygons
            .iter()
            .find(|p| p.pour == Some(0))
            .unwrap();
        assert!(poured.outline.iter().any(|q| q.x.mil() > 700.0));
        // Thermal style on the pour itself.
        let e3 = set_pour(&b2, 0, |p| p.connect = Some(PourConnect::USUAL));
        let b3 = run(&b2, &e3);
        assert_eq!(b3.geometry.pours[0].connect, Some(PourConnect::USUAL));
        // Deleting takes its copper too; undo puts both back.
        let e4 = delete(&b2, r);
        let b4 = run(&b2, &e4);
        assert!(b4.geometry.pours.is_empty());
        assert!(b4.geometry.polygons.is_empty());
        let mut u = b4.clone();
        for x in e4.iter().rev() {
            x.inverse().apply(&mut u);
        }
        assert_eq!(u.geometry.pours.len(), 1);
        assert_eq!(u.geometry.polygons.len(), b2.geometry.polygons.len());
    }

    #[test]
    fn connection_styles() {
        let mut b = board();
        b.geometry.pads.push(Pad {
            layer: LayerRef::AllCopper,
            net: Some(0),
            component: None,
            name: "1".into(),
            at: pt([200.0, 250.0]),
            size_x: Length::from_mil(60.0),
            size_y: Length::from_mil(60.0),
            hole: Length::from_mil(35.0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        let ring = rect_ring([100.0, 100.0], [600.0, 400.0]);
        let (e, _) = add(&b, Kind::Pour, &ring, 0, Some(0)).unwrap();
        let b0 = run(&b, &e);
        let copper = |j: Join| {
            let e = set_pour(&b0, 0, |p| {
                p.connect = Some(PourConnect {
                    th_pads: j,
                    smd_pads: Join::Direct,
                    vias: Join::Direct,
                })
            });
            let mut x = run(&b0, &e);
            pour::repour_after(&x, &e).unwrap().apply(&mut x);
            // Copper within 50 mil of the pad: area of the pour less holes.
            x.geometry
                .polygons
                .iter()
                .filter(|p| p.pour == Some(0))
                .map(|p| {
                    let o: Vec<P> = p.outline.iter().map(|&q| mil(q)).collect();
                    let h: f64 = p
                        .holes
                        .iter()
                        .map(|h| signed_area(&h.iter().map(|&q| mil(q)).collect::<Vec<_>>()).abs())
                        .sum();
                    signed_area(&o).abs() - h
                })
                .sum::<f64>()
        };
        let direct = copper(Join::Direct);
        let relief = copper(Join::RELIEF);
        let none = copper(Join::None { gap: 10.0 });
        assert!(direct > relief + 100.0, "{direct} {relief}");
        assert!(relief > none + 100.0, "{relief} {none}");
    }

    #[test]
    fn regions_keepouts_cutouts_and_corners() {
        let b = board();
        let tri = vec![[100.0, 100.0], [300.0, 100.0], [200.0, 300.0]];
        let (e, r) = add(&b, Kind::Region, &tri, 1, Some(0)).unwrap();
        let b1 = run(&b, &e);
        assert_eq!(r, ShapeRef::Area(0));
        assert_eq!(b1.geometry.polygons[0].layer, LayerRef::Copper(1));
        let (e, k) = add(&b1, Kind::Keepout, &tri, 0, None).unwrap();
        let b2 = run(&b1, &e);
        assert_eq!(k.kind(&b2), Kind::Keepout);
        let (e, c) = add(&b2, Kind::Cutout, &tri, 0, None).unwrap();
        let b3 = run(&b2, &e);
        assert_eq!(c, ShapeRef::Cutout(0));
        let m = run(&b3, &move_by(&b3, r, 10.0, 0.0));
        assert!((m.geometry.polygons[0].outline[0].x.mil() - 110.0).abs() < 1e-6);
        // Corners added and taken away.
        let ring = r.ring(&b3).unwrap();
        let (i, at) = edge_at(&ring, [200.0, 101.0], 3.0).unwrap();
        assert_eq!(i, 0);
        let four = with_new_corner(&ring, i, at);
        assert_eq!(four.len(), 4);
        assert_eq!(without_corner(&four, 1).unwrap().len(), 3);
        assert!(without_corner(&ring, 0).is_none());
        let s = snap45([0.0, 0.0], [100.0, 90.0]);
        assert!((s[0] - s[1]).abs() < 1e-9);
        // A new outline.
        let o = rect_ring([0.0, 0.0], [2000.0, 500.0]);
        let (e, _) = add(&b3, Kind::Outline, &o, 0, None).unwrap();
        let b4 = run(&b3, &e);
        assert_eq!(b4.geometry.outline.len(), 4);
        assert!(delete(&b4, ShapeRef::Outline).is_empty());
    }
}
