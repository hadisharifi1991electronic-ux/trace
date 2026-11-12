//! Pouring copper again after edits: a pour fills its outline, keeps its
//! clearance from copper of other nets on its layer, and drops islands that
//! touch none of its own net's copper.
//!
//! Gaps, thermal reliefs and the board-edge gap come from the board's
//! clearance, polygon connect and board outline clearance rules.

use crate::edit::Edit;
use crate::{Board, LayerRef, Length, PadShape, Point, Polygon};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::outline::offset::OutlineOffset;
use i_overlay::mesh::style::{LineJoin, OutlineStyle};

type P = [f64; 2];

fn mil(p: Point) -> P {
    [p.x.mil(), p.y.mil()]
}

fn point(p: P) -> Point {
    Point {
        x: Length::from_mil(p[0]),
        y: Length::from_mil(p[1]),
    }
}

fn area(r: &[P]) -> f64 {
    let n = r.len();
    (0..n)
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
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

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Polygon around the segment `a`-`b` at distance `r`, circumscribing the
/// true round ends so the cut is never short of the clearance.
fn capsule(a: P, b: P, r: f64) -> Vec<P> {
    let n = 8;
    let r = r / (std::f64::consts::PI / (2.0 * n as f64)).cos();
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let base = dy.atan2(dx);
    let mut out = Vec::with_capacity(2 * n + 2);
    for (c, from) in [
        (b, base - std::f64::consts::FRAC_PI_2),
        (a, base + std::f64::consts::FRAC_PI_2),
    ] {
        for k in 0..=n {
            let t = from + std::f64::consts::PI * k as f64 / n as f64;
            out.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
        }
    }
    out
}

/// A rectangle `sx` by `sy` around `c`, turned by `deg`.
fn rect(c: P, sx: f64, sy: f64, deg: f64) -> Vec<P> {
    let (s, co) = deg.to_radians().sin_cos();
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|&(u, v)| {
            let (x, y) = (u * sx / 2.0, v * sy / 2.0);
            [c[0] + x * co - y * s, c[1] + x * s + y * co]
        })
        .collect()
}

/// Land of a pad grown by `e`: rectangles keep square corners (a little
/// more clearance than needed at the corners), round and oval lands stay
/// round.
fn land(c: P, sx: f64, sy: f64, deg: f64, shape: PadShape, e: f64) -> Vec<P> {
    match shape {
        PadShape::Round => {
            let along = (sx - sy).abs() / 2.0;
            let (s, co) = deg.to_radians().sin_cos();
            let (ux, uy) = if sx >= sy { (co, s) } else { (-s, co) };
            capsule(
                [c[0] - ux * along, c[1] - uy * along],
                [c[0] + ux * along + 1e-6, c[1] + uy * along],
                sx.min(sy) / 2.0 + e,
            )
        }
        _ => rect(c, sx + 2.0 * e, sy + 2.0 * e, deg),
    }
}

/// Does a rule scope apply to a net, as far as its `InNet('...')` terms
/// say? `not InNet('X')` leaves X out; a plain `InNet` names the only nets.
fn scope_has_net(scope: &str, net: &str) -> bool {
    let low = scope.to_ascii_lowercase();
    let mut named = Vec::new();
    let mut excluded = Vec::new();
    let mut rest = low.as_str();
    while let Some(i) = rest.find("innet('") {
        let before = rest[..i].trim_end();
        let tail = &rest[i + 7..];
        let Some(j) = tail.find('\'') else { break };
        let name = &tail[..j];
        if before.ends_with("not") || before.ends_with("not (") || before.ends_with('!') {
            excluded.push(name.to_string());
        } else {
            named.push(name.to_string());
        }
        rest = &tail[j..];
    }
    let net = net.to_ascii_lowercase();
    !excluded.contains(&net) && (named.is_empty() || named.contains(&net))
}

/// Gaps a pour keeps from each kind of copper of other nets, in mil.
#[derive(Clone, Copy, Debug)]
pub struct Gaps {
    pub track: f64,
    pub arc: f64,
    pub smd: f64,
    pub th: f64,
    pub via: f64,
    pub area: f64,
    pub hole: f64,
}

impl Gaps {
    fn max(&self) -> f64 {
        [
            self.track, self.arc, self.smd, self.th, self.via, self.area, self.hole,
        ]
        .into_iter()
        .fold(0.0, f64::max)
    }
}

/// The gaps for a pour of `net`: from the highest-priority clearance rule
/// scoped to polygons that covers the net, else the board's catch-all
/// clearance rule (the lowest priority). A rule's per-object polygon gaps
/// (`OBJECTCLEARANCES`, 1/10,000 mil) override its plain gap.
pub fn pour_gaps(board: &Board, net: Option<u32>) -> Gaps {
    let name = net
        .and_then(|n| board.nets.get(n as usize))
        .map(|n| n.name.as_str())
        .unwrap_or("");
    let mut rules: Vec<_> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("Clearance"))
        .collect();
    rules.sort_by_key(|r| r.priority);
    let poly = rules.iter().find(|r| {
        r.scope
            .iter()
            .any(|s| s.to_ascii_lowercase().contains("polygon"))
            && r.scope.iter().all(|s| scope_has_net(s, name))
    });
    let rule = poly.or(rules.last());
    let base = crate::route::board_clearance(board);
    let Some(r) = rule else {
        return Gaps {
            track: base,
            arc: base,
            smd: base,
            th: base,
            via: base,
            area: base,
            hole: base,
        };
    };
    let param = |k: &str| {
        r.params
            .iter()
            .find(|(pk, _)| pk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    };
    let gap = param("GAP")
        .and_then(Length::parse)
        .map(Length::mil)
        .unwrap_or(base);
    let objects = param("OBJECTCLEARANCES").unwrap_or("");
    let with = |kind: &str| {
        objects
            .split(';')
            .filter_map(|item| {
                let (pair, value) = item.split_once(':')?;
                let mut parts = pair
                    .split('-')
                    .map(|o| o.trim_start_matches("ClearanceObj_"));
                let (a, b) = (parts.next()?, parts.next()?);
                let hit = (a == "Poly" && b == kind) || (b == "Poly" && a == kind);
                hit.then(|| value.trim().parse::<f64>().ok().map(|v| v / 10_000.0))?
            })
            .next()
            .unwrap_or(gap)
    };
    Gaps {
        track: with("Track"),
        arc: with("Arc"),
        smd: with("SMDPad"),
        th: with("THPad"),
        via: with("Via"),
        area: with("Fill").max(with("Region")).max(with("Poly")),
        hole: with("Hole"),
    }
}

/// How a pour joins a pad or via of its own net.
#[derive(Clone, Copy, Debug)]
enum Style {
    Direct,
    Relief {
        gap: f64,
        width: f64,
        entries: u32,
        diagonal: bool,
    },
}

/// Polygon connect styles for through pads, surface pads and vias, from
/// the board's `PolygonConnect` rule that applies to everything (else the
/// first by priority).
struct Connect {
    th: Style,
    smd: Style,
    via: Style,
}

impl Connect {
    fn of(board: &Board, net: Option<u32>) -> Connect {
        let name = net
            .and_then(|n| board.nets.get(n as usize))
            .map(|n| n.name.as_str())
            .unwrap_or("");
        // Rules for particular parts, layers or rooms are left to Altium;
        // the rest apply by priority where their nets fit.
        const PARTIAL: &[&str] = &[
            "incomponent",
            "hasfootprint",
            "onlayer",
            "holesize",
            "withinroom",
            "touchesroom",
            "ispad",
            "isvia",
            "innetclass",
        ];
        let mut rules: Vec<_> = board
            .rules
            .iter()
            .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("PolygonConnect"))
            .filter(|r| {
                r.scope.iter().all(|s| {
                    let low = s.to_ascii_lowercase();
                    !PARTIAL.iter().any(|w| low.contains(w)) && scope_has_net(s, name)
                })
            })
            .collect();
        rules.sort_by_key(|r| r.priority);
        let Some(r) = rules.first() else {
            return Connect {
                th: Style::Direct,
                smd: Style::Direct,
                via: Style::Direct,
            };
        };
        let get = |prefix: &str, key: &str| {
            r.params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{prefix}.{key}")))
                .or_else(|| r.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
                .map(|(_, v)| v.as_str())
        };
        let style = |prefix: &str| {
            if !get(prefix, "CONNECTSTYLE").is_some_and(|s| s.eq_ignore_ascii_case("Relief")) {
                return Style::Direct;
            }
            let len = |k: &str, d: f64| {
                get(prefix, k)
                    .and_then(Length::parse)
                    .map(Length::mil)
                    .unwrap_or(d)
            };
            Style::Relief {
                gap: len("AIRGAPWIDTH", 10.0),
                width: len("RELIEFCONDUCTORWIDTH", 10.0),
                entries: get(prefix, "RELIEFENTRIES")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(4),
                diagonal: get(prefix, "POLYGONRELIEFANGLE").is_some_and(|v| v.contains("45")),
            }
        };
        Connect {
            th: style("THPAD"),
            smd: style("SMDPAD"),
            via: style("VIA"),
        }
    }
}

/// Gap between pours and the board edge, from the board outline
/// clearance rule; 0 when there is none.
fn outline_clearance(board: &Board, net: Option<u32>) -> f64 {
    let name = net
        .and_then(|n| board.nets.get(n as usize))
        .map(|n| n.name.as_str())
        .unwrap_or("");
    let mut rules: Vec<_> = board
        .rules
        .iter()
        .filter(|r| r.enabled && r.kind.eq_ignore_ascii_case("BoardOutlineClearance"))
        .filter(|r| r.scope.iter().all(|s| scope_has_net(s, name)))
        .collect();
    rules.sort_by_key(|r| r.priority);
    rules
        .iter()
        .find_map(|r| {
            r.params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("GAP"))
                .and_then(|(_, v)| Length::parse(v))
        })
        .map(Length::mil)
        .unwrap_or(0.0)
}

/// Pours the pour at `index` again and returns its new copper.
pub fn pour(board: &Board, index: usize) -> Vec<Polygon> {
    pour_among(board, &board.geometry.polygons, index)
}

/// As `pour`, with `areas` standing for the board's copper areas.
fn pour_among(board: &Board, areas: &[Polygon], index: usize) -> Vec<Polygon> {
    let g = &board.geometry;
    let Some(def) = g.pours.get(index) else {
        return Vec::new();
    };
    let gaps = pour_gaps(board, def.net);
    let clearance = gaps.max();
    let LayerRef::Copper(layer) = def.layer else {
        return Vec::new();
    };
    if def.outline.len() < 3 {
        return Vec::new();
    }
    let outline: Vec<P> = def.outline.iter().map(|&p| mil(p)).collect();
    let b = bbox(&outline);
    let reach = [
        b[0] - clearance - 500.0,
        b[1] - clearance - 500.0,
        b[2] + clearance + 500.0,
        b[3] + clearance + 500.0,
    ];
    let net = def.net;
    let mut cuts: Vec<Vec<P>> = Vec::new();
    // Copper of the pour's own net inside the pour: what live islands touch.
    let mut anchors: Vec<P> = Vec::new();
    let add = |mut ring: Vec<P>, cuts: &mut Vec<Vec<P>>| {
        if ring.len() >= 3 && overlaps(bbox(&ring), reach) {
            if area(&ring) < 0.0 {
                ring.reverse();
            }
            cuts.push(ring);
        }
    };
    let connect = Connect::of(board, def.net);
    let mut reliefs: Vec<Vec<P>> = Vec::new();
    let mut spokes: Vec<Vec<P>> = Vec::new();
    let mut relieve = |c: P, sx: f64, sy: f64, deg: f64, shape: PadShape, style: &Style| {
        if let Style::Relief {
            gap,
            width,
            entries,
            diagonal,
        } = *style
        {
            reliefs.push(land(c, sx, sy, deg, shape, gap));
            let reach = sx.max(sy) / 2.0 + gap + 2.0;
            for k in 0..entries {
                let a = deg + if diagonal { 45.0 } else { 0.0 } + 360.0 / entries as f64 * k as f64;
                let (s, co) = a.to_radians().sin_cos();
                spokes.push(rect(
                    [c[0] + co * reach / 2.0, c[1] + s * reach / 2.0],
                    reach,
                    width,
                    a,
                ));
            }
        }
    };
    let on = |l: &LayerRef| *l == LayerRef::Copper(layer);

    for t in g.tracks.iter().filter(|t| on(&t.layer)) {
        let (a, z) = (mil(t.start), mil(t.end));
        if t.net.is_some() && t.net == net {
            anchors.extend([a, z, [(a[0] + z[0]) / 2.0, (a[1] + z[1]) / 2.0]]);
        } else {
            add(capsule(a, z, t.width.mil() / 2.0 + gaps.track), &mut cuts);
        }
    }
    for a in g.arcs.iter().filter(|a| on(&a.layer)) {
        let c = mil(a.center);
        let r = a.radius.mil();
        let mut sweep = a.end_deg - a.start_deg;
        if sweep <= 0.0 {
            sweep += 360.0;
        }
        let n = ((sweep / 10.0).ceil() as usize).max(1);
        let pts: Vec<P> = (0..=n)
            .map(|k| {
                let t = (a.start_deg + sweep * k as f64 / n as f64).to_radians();
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect();
        if a.net.is_some() && a.net == net {
            anchors.extend(pts);
        } else {
            for w in pts.windows(2) {
                add(
                    capsule(w[0], w[1], a.width.mil() / 2.0 + gaps.arc),
                    &mut cuts,
                );
            }
        }
    }
    for v in &g.vias {
        let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
        if layer < lo || layer > hi {
            continue;
        }
        let c = mil(v.at);
        if v.net.is_some() && v.net == net {
            anchors.push(c);
            let d = v
                .rings
                .iter()
                .find(|(l, _)| *l == layer)
                .map(|(_, d)| *d)
                .unwrap_or(v.diameter)
                .mil();
            relieve(c, d, d, 0.0, PadShape::Round, &connect.via);
            continue;
        }
        let d = v
            .rings
            .iter()
            .find(|(l, _)| *l == layer)
            .map(|(_, d)| *d)
            .unwrap_or(if v.rings.is_empty() {
                v.diameter
            } else {
                v.hole
            });
        let d = d.mil().max(v.hole.mil());
        add(
            capsule(c, [c[0] + 1e-6, c[1]], d / 2.0 + gaps.via),
            &mut cuts,
        );
    }
    for p in &g.pads {
        let through = p.layer == LayerRef::AllCopper;
        if !through && !on(&p.layer) {
            continue;
        }
        let c = mil(p.at);
        let ours = p.net.is_some() && p.net == net;
        // The land on this layer, or only the hole where a through pad
        // has no land here.
        let shape = if !p.stack.is_empty() {
            p.stack
                .iter()
                .find(|l| l.layer == layer)
                .map(|l| (l.size_x.mil(), l.size_y.mil(), l.rotation_deg, l.shape))
        } else {
            Some((p.size_x.mil(), p.size_y.mil(), p.rotation_deg, p.shape))
        };
        if ours {
            if let Some((sx, sy, deg, sh)) = shape {
                anchors.push(c);
                let style = if p.hole.0 > 0 {
                    &connect.th
                } else {
                    &connect.smd
                };
                relieve(c, sx, sy, deg, sh, style);
            }
            continue;
        }
        if !p.outline.is_empty() && !through {
            let ring: Vec<P> = p.outline.iter().map(|&q| mil(q)).collect();
            let bb = bbox(&ring);
            add(
                rect(
                    [(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0],
                    bb[2] - bb[0] + 2.0 * gaps.smd,
                    bb[3] - bb[1] + 2.0 * gaps.smd,
                    0.0,
                ),
                &mut cuts,
            );
            continue;
        }
        match shape {
            Some((sx, sy, deg, sh)) => {
                let gap = if p.hole.0 > 0 { gaps.th } else { gaps.smd };
                add(land(c, sx, sy, deg, sh, gap), &mut cuts)
            }
            None if p.hole.0 > 0 => add(
                capsule(c, [c[0] + 1e-6, c[1]], p.hole.mil() / 2.0 + gaps.hole),
                &mut cuts,
            ),
            None => {}
        }
    }
    // Other copper areas on the layer: other pours' copper and fixed
    // regions, kept clear by growing their outlines.
    for f in areas
        .iter()
        .filter(|f| on(&f.layer) && f.pour != Some(index as u32))
    {
        let ring: Vec<P> = f.outline.iter().map(|&q| mil(q)).collect();
        if ring.len() < 3 || !overlaps(bbox(&ring), reach) {
            continue;
        }
        if f.net.is_some() && f.net == net {
            continue;
        }
        // The area grown by the clearance; holes wound the other way, so
        // what is left of them stays open.
        let mut shape = vec![ring];
        if area(&shape[0]) < 0.0 {
            shape[0].reverse();
        }
        for h in &f.holes {
            let mut h: Vec<P> = h.iter().map(|&q| mil(q)).collect();
            if h.len() >= 3 {
                if area(&h) > 0.0 {
                    h.reverse();
                }
                shape.push(h);
            }
        }
        let style = OutlineStyle::new(gaps.area).line_join(LineJoin::Round(0.25));
        for grown in shape.outline(&style) {
            cuts.extend(grown);
        }
    }
    // Keep back from the board edge.
    let edge = outline_clearance(board, def.net);
    if edge > 0.0 {
        let ring: Vec<P> = g.outline.iter().map(|&q| mil(q)).collect();
        let n = ring.len();
        for i in 0..n {
            add(capsule(ring[i], ring[(i + 1) % n], edge), &mut cuts);
        }
    }

    let subject = vec![outline.clone()];
    let mut clip = cuts.clone();
    clip.extend(reliefs);
    let mut shapes = subject.overlay(&clip, OverlayRule::Difference, FillRule::NonZero);
    if !spokes.is_empty() {
        // Spokes join own-net pads across their air gaps, where they stay
        // clear of everything else.
        let spokes = spokes.overlay(&cuts, OverlayRule::Difference, FillRule::NonZero);
        let spokes: Vec<Vec<P>> = spokes.into_iter().flatten().collect();
        let spokes = spokes.overlay(&subject, OverlayRule::Intersect, FillRule::NonZero);
        let base: Vec<Vec<P>> = shapes.into_iter().flatten().collect();
        let add: Vec<Vec<P>> = spokes.into_iter().flatten().collect();
        shapes = base.overlay(&add, OverlayRule::Union, FillRule::NonZero);
    }

    let inside = |p: P, r: &[P]| {
        let mut odd = false;
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + n - 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                odd = !odd;
            }
        }
        odd
    };
    shapes
        .into_iter()
        .filter(|s| !s.is_empty() && area(&s[0]).abs() > 4.0)
        .filter(|s| {
            !def.remove_dead
                || net.is_none()
                || anchors
                    .iter()
                    .any(|&a| inside(a, &s[0]) && !s[1..].iter().any(|h| inside(a, h)))
        })
        .map(|s| {
            let mut rings = s.into_iter();
            let outer = rings.next().unwrap_or_default();
            Polygon {
                layer: def.layer.clone(),
                net,
                outline: outer.into_iter().map(point).collect(),
                holes: rings
                    .filter(|h| h.len() >= 3)
                    .map(|h| h.into_iter().map(point).collect())
                    .collect(),
                pour: Some(index as u32),
                component: None,
            }
        })
        .collect()
}

/// Board area an edit changes, in mil, and the copper layers it is on.
fn touched(board: &Board, e: &Edit) -> Option<([f64; 4], Vec<u8>)> {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let one = |l: &LayerRef| match l {
        LayerRef::Copper(i) => vec![*i],
        LayerRef::AllCopper => (0..n).collect(),
        _ => Vec::new(),
    };
    match e {
        Edit::InsertTrack { track: t, .. } | Edit::RemoveTrack { track: t, .. } => {
            let w = t.width.mil();
            let b = bbox(&[mil(t.start), mil(t.end)]);
            Some(([b[0] - w, b[1] - w, b[2] + w, b[3] + w], one(&t.layer)))
        }
        Edit::InsertArc { arc: a, .. } | Edit::RemoveArc { arc: a, .. } => {
            let c = mil(a.center);
            let r = a.radius.mil() + a.width.mil();
            Some(([c[0] - r, c[1] - r, c[0] + r, c[1] + r], one(&a.layer)))
        }
        Edit::InsertVia { via: v, .. } | Edit::RemoveVia { via: v, .. } => {
            let c = mil(v.at);
            let r = v.diameter.mil();
            let (lo, hi) = (v.from_layer.min(v.to_layer), v.from_layer.max(v.to_layer));
            Some((
                [c[0] - r, c[1] - r, c[0] + r, c[1] + r],
                (lo..=hi).collect(),
            ))
        }
        Edit::PlaceComponent { index, from, to } => {
            let b = crate::place::area(board, *index, *from, *to)?;
            Some((b, (0..n).collect()))
        }
        Edit::Netlist { pads, .. } => {
            // Pads that change net: pours about them connect differently.
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for p in pads
                .iter()
                .filter_map(|&(i, ..)| board.geometry.pads.get(i))
            {
                let c = mil(p.at);
                let r = p.size_x.mil().max(p.size_y.mil());
                b = [
                    b[0].min(c[0] - r),
                    b[1].min(c[1] - r),
                    b[2].max(c[0] + r),
                    b[3].max(c[1] + r),
                ];
            }
            (b[0] <= b[2]).then(|| (b, (0..n).collect()))
        }
        Edit::MoveComponent { .. } | Edit::SetPolygons { .. } => None,
    }
}

/// After `edits` have been applied: pours the pours they touch again, as
/// one edit that replaces the board's copper areas (so undo restores the
/// old ones). `None` when no pour is touched.
pub fn repour_after(board: &Board, edits: &[Edit]) -> Option<Edit> {
    let g = &board.geometry;
    let clearance = 50.0;
    let areas: Vec<([f64; 4], Vec<u8>)> = edits.iter().filter_map(|e| touched(board, e)).collect();
    let mut hit: Vec<usize> = g
        .pours
        .iter()
        .enumerate()
        .filter(|(_, p)| p.outline.len() >= 3)
        .filter(|(_, p)| {
            let LayerRef::Copper(l) = p.layer else {
                return false;
            };
            let ring: Vec<P> = p.outline.iter().map(|&q| mil(q)).collect();
            let b = bbox(&ring);
            let b = [
                b[0] - clearance,
                b[1] - clearance,
                b[2] + clearance,
                b[3] + clearance,
            ];
            areas
                .iter()
                .any(|(a, ls)| ls.contains(&l) && overlaps(*a, b))
        })
        .map(|(i, _)| i)
        .collect();
    if hit.is_empty() {
        return None;
    }
    // Earlier pours first, so later ones keep clear of their new copper.
    hit.sort_by_key(|&i| g.pours[i].order);
    let mut after = g.polygons.clone();
    for i in hit {
        let new = pour_among(board, &after, i);
        after.retain(|f| f.pour != Some(i as u32));
        after.extend(new);
    }
    Some(Edit::SetPolygons {
        before: g.polygons.clone(),
        after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn p(x: f64, y: f64) -> Point {
        Point {
            x: Length::from_mil(x),
            y: Length::from_mil(y),
        }
    }

    fn board() -> Board {
        let mut b = Board {
            nets: vec![
                Net {
                    name: "GND".into(),
                    ..Default::default()
                },
                Net {
                    name: "SIG".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        b.geometry.pours.push(Pour {
            layer: LayerRef::Copper(0),
            net: Some(0),
            outline: vec![
                p(0.0, 0.0),
                p(1000.0, 0.0),
                p(1000.0, 1000.0),
                p(0.0, 1000.0),
            ],
            remove_dead: true,
            order: 0,
        });
        // A GND pad so the pour is live.
        b.geometry.pads.push(Pad {
            layer: LayerRef::Copper(0),
            net: Some(0),
            component: None,
            name: "1".into(),
            at: p(100.0, 100.0),
            size_x: Length::from_mil(40.0),
            size_y: Length::from_mil(40.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        });
        b
    }

    fn copper(polys: &[Polygon]) -> f64 {
        polys
            .iter()
            .map(|f| {
                let o: Vec<P> = f.outline.iter().map(|&q| mil(q)).collect();
                area(&o).abs()
                    - f.holes
                        .iter()
                        .map(|h| area(&h.iter().map(|&q| mil(q)).collect::<Vec<_>>()).abs())
                        .sum::<f64>()
            })
            .sum()
    }

    #[test]
    fn a_track_of_another_net_is_cleared_and_given_back() {
        let mut b = board();
        let full = pour(&b, 0);
        assert_eq!(full.len(), 1);
        assert!((copper(&full) - 1_000_000.0).abs() < 1.0);
        b.geometry.polygons = full;
        let track = Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: p(500.0, 200.0),
            end: p(500.0, 800.0),
            width: Length::from_mil(10.0),
        };
        let add = Edit::InsertTrack { index: 0, track };
        add.apply(&mut b);
        let e = repour_after(&b, std::slice::from_ref(&add)).expect("pour touched");
        e.apply(&mut b);
        let cut = copper(&b.geometry.polygons);
        // 600 long, 10 wide plus the clearance each side, round ends.
        let w = 10.0 + 2.0 * pour_gaps(&b, Some(0)).track;
        let gone = 1_000_000.0 - cut;
        assert!(gone > w * 600.0 && gone < w * 1.05 * (600.0 + w), "{gone}");
        // Taking the track away fills the gap again.
        let del = add.inverse();
        del.apply(&mut b);
        let e = repour_after(&b, std::slice::from_ref(&del)).unwrap();
        e.apply(&mut b);
        assert!((copper(&b.geometry.polygons) - 1_000_000.0).abs() < 1.0);
    }

    #[test]
    fn islands_without_own_copper_are_dropped() {
        let mut b = board();
        // A wall of another net cuts the pour in two; only the half with
        // the GND pad stays.
        b.geometry.tracks.push(Track {
            component: None,
            layer: LayerRef::Copper(0),
            net: Some(1),
            start: p(500.0, -100.0),
            end: p(500.0, 1100.0),
            width: Length::from_mil(10.0),
        });
        let f = pour(&b, 0);
        assert_eq!(f.len(), 1);
        let c = copper(&f);
        assert!(c < 500_000.0 && c > 480_000.0, "{c}");
    }
}
