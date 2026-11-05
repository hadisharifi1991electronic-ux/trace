//! Interactive routing: click on copper to start, click to fix corners,
//! V for a via to the next layer, and the route ends on copper of its own
//! net or with Enter. Shift+Space cycles the corner style and Space flips
//! the bend, as in Altium. Starting on a net of a differential pair routes
//! both nets together along a centre line at the pair's width and gap.
//! Copper of other nets keeps its clearance; a route that would break it is
//! drawn red and is not placed.

use trace_core::edit::{Edit, History};
use trace_core::route::{self, Corner, Hit, Obstacles, Piece, P};
use trace_core::{Arc, Board, LayerRef, Length, Point, Track, Via};

/// One run of track on one layer; runs are joined by vias. Each click adds
/// a group of centre-line pieces, so Backspace takes back a click at a time.
#[derive(Clone)]
struct Run {
    layer: u8,
    start: P,
    clicks: Vec<Vec<Piece>>,
    /// Pairs: where the routed net's track and its partner's start (pads
    /// or vias), and where they end when vias closed the run.
    from: Option<(P, P)>,
    to: Option<(P, P)>,
}

impl Run {
    fn end(&self) -> P {
        self.clicks
            .iter()
            .rev()
            .find_map(|c| c.last())
            .map(|p| p.end())
            .unwrap_or(self.start)
    }
}

/// Width and gap of a pair on each copper layer, and the partner net.
#[derive(Clone)]
pub struct PairInfo {
    pub name: String,
    pub partner: Option<u32>,
    /// (width, gap) per copper layer.
    pub sizes: Vec<(f64, f64)>,
}

pub struct Active {
    pub net: Option<u32>,
    pub pair: Option<PairInfo>,
    runs: Vec<Run>,
    /// Vias placed so far, with their nets.
    vias: Vec<(P, Option<u32>)>,
    /// Centre-line pieces from the last fixed point to the cursor.
    pub preview: Vec<Piece>,
    /// The preview ends on copper of the route's own net (for a pair: on
    /// the pair's pads, the routed net's first).
    pub lands: bool,
    landing: Option<(P, P)>,
    pub hit: Option<Hit>,
}

/// A piece of copper the route will place.
pub struct Placed {
    pub layer: u8,
    pub net: Option<u32>,
    pub piece: Piece,
    pub width: f64,
    /// Part of the run still following the cursor.
    pub live: bool,
}

impl Active {
    pub fn layer(&self) -> u8 {
        self.runs.last().map(|r| r.layer).unwrap_or(0)
    }
    pub fn last_point(&self) -> P {
        self.runs.last().map(|r| r.end()).unwrap_or([0.0, 0.0])
    }
    pub fn via_points(&self) -> impl Iterator<Item = P> + '_ {
        self.vias.iter().map(|v| v.0)
    }
    fn fix_preview(&mut self) {
        let preview = std::mem::take(&mut self.preview);
        if !preview.is_empty() {
            self.runs.last_mut().unwrap().clicks.push(preview);
        }
    }
    fn size(&self, layer: u8, single: f64) -> (f64, f64) {
        match &self.pair {
            Some(p) => p
                .sizes
                .get(layer as usize)
                .copied()
                .unwrap_or((single, single)),
            None => (single, 0.0),
        }
    }

    /// Every piece of copper the route would place now, including the part
    /// that follows the cursor.
    pub fn layout(&self, single: f64) -> Vec<Placed> {
        let mut out = Vec::new();
        let last = self.runs.len() - 1;
        for (ri, run) in self.runs.iter().enumerate() {
            let live = ri == last;
            let mut centre: Vec<Piece> = run.clicks.iter().flatten().copied().collect();
            let mut to = run.to;
            if live {
                centre.extend(self.preview.iter().copied());
                to = to.or(self.landing.filter(|_| self.lands));
            }
            let (w, gap) = self.size(run.layer, single);
            let mut push = |net: Option<u32>, piece: Piece| {
                if piece.length() > 1e-6 {
                    out.push(Placed {
                        layer: run.layer,
                        net,
                        piece,
                        width: w,
                        live,
                    });
                }
            };
            match (&self.pair, run.from) {
                (Some(pair), Some((fa, fb))) => {
                    let Some(dir) = centre.first().and_then(|p| p.chords().first().copied()) else {
                        continue;
                    };
                    let d = [dir.1[0] - dir.0[0], dir.1[1] - dir.0[1]];
                    let side =
                        if d[0] * (fa[1] - run.start[1]) - d[1] * (fa[0] - run.start[0]) >= 0.0 {
                            1.0
                        } else {
                            -1.0
                        };
                    let half = (w + gap) / 2.0;
                    for (net, from, to, k) in [
                        (self.net, fa, to.map(|t| t.0), side),
                        (pair.partner, fb, to.map(|t| t.1), -side),
                    ] {
                        let off = route::offset(&centre, k * half);
                        let (Some(first), Some(lastp)) = (off.first(), off.last()) else {
                            continue;
                        };
                        push(net, Piece::Line(from, first.start()));
                        let end = lastp.end();
                        for p in off {
                            push(net, p);
                        }
                        if let Some(t) = to {
                            push(net, Piece::Line(end, t));
                        }
                    }
                }
                _ => {
                    for p in centre {
                        push(self.net, p);
                    }
                }
            }
        }
        out
    }
}

pub struct Router {
    pub width: f64,
    pub via_d: f64,
    pub via_hole: f64,
    pub clearance: f64,
    pub corner: Corner,
    /// Space: which run of a corner comes first.
    pub flip: bool,
    /// Round corners as a fraction of the largest that fits (, and .).
    pub arc: f64,
    /// Route both nets of a differential pair together.
    pub pairs: bool,
    pub active: Option<Active>,
    pub history: History,
    pub message: Option<String>,
    obstacles: Option<Obstacles>,
}

impl Router {
    pub fn new() -> Self {
        Router {
            width: route::DEFAULT_WIDTH,
            via_d: 20.0,
            via_hole: 10.0,
            clearance: route::DEFAULT_CLEARANCE,
            corner: Corner::Line45,
            flip: false,
            arc: 1.0,
            pairs: true,
            active: None,
            history: History::default(),
            message: None,
            obstacles: None,
        }
    }

    /// New board: sizes from its rules and vias, empty history. The corner
    /// style is kept.
    pub fn load(&mut self, board: &Board) {
        let (corner, arc, pairs) = (self.corner, self.arc, self.pairs);
        *self = Router::new();
        (self.corner, self.arc, self.pairs) = (corner, arc, pairs);
        self.width = route::board_width(board);
        self.clearance = route::board_clearance(board);
        (self.via_d, self.via_hole) = route::common_via(board);
    }

    /// The board changed; obstacles are rebuilt on next use.
    pub fn board_changed(&mut self) {
        self.obstacles = None;
    }

    fn obstacles(&mut self, board: &Board) -> &Obstacles {
        self.obstacles.get_or_insert_with(|| Obstacles::new(board))
    }

    /// Starts a route at `at`. For a pair, `partner_pad` is where the
    /// partner's track starts.
    pub fn start(&mut self, at: P, net: Option<u32>, layer: u8, pair: Option<(PairInfo, P)>) {
        self.message = None;
        let (pair, from, start) = match pair {
            Some((info, b)) => (
                Some(info),
                Some((at, b)),
                [(at[0] + b[0]) / 2.0, (at[1] + b[1]) / 2.0],
            ),
            None => (None, None, at),
        };
        self.active = Some(Active {
            net,
            pair,
            runs: vec![Run {
                layer,
                start,
                clicks: Vec::new(),
                from,
                to: None,
            }],
            vias: Vec::new(),
            preview: Vec::new(),
            lands: false,
            landing: None,
            hit: None,
        });
    }

    pub fn cancel(&mut self) {
        self.active = None;
        self.message = None;
    }

    /// The width tracks are drawn and placed at on the active layer.
    pub fn current_width(&self) -> f64 {
        match &self.active {
            Some(a) => a.size(a.layer(), self.width).0,
            None => self.width,
        }
    }

    /// Follows the cursor. `snap` is copper under the cursor on the
    /// route's layer: its centre or track end, and its net. `partner_pad`
    /// finds the partner's pad next to a pad of the pair.
    pub fn update(
        &mut self,
        board: &Board,
        cursor: P,
        snap: Option<(P, Option<u32>)>,
        partner_pad: impl Fn(P, Option<u32>) -> Option<P>,
    ) {
        let (width, clearance) = (self.width, self.clearance);
        let (corner, flip, arc) = (self.corner, self.flip, self.arc);
        let Some(a) = &self.active else { return };
        let (net, from) = (a.net, a.last_point());
        let partner = a.pair.as_ref().map(|p| p.partner);
        let mut landing = None;
        let (target, lands) = match (snap, partner) {
            (Some((p, n)), None) if n.is_some() && n == net => (p, true),
            (Some((p, n)), Some(pn)) if n.is_some() && (n == net || n == pn) => {
                let other = if n == net { pn } else { net };
                match partner_pad(p, other) {
                    Some(q) => {
                        landing = Some(if n == net { (p, q) } else { (q, p) });
                        ([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0], true)
                    }
                    None => (cursor, false),
                }
            }
            _ => (cursor, false),
        };
        let preview = route::corner_path(from, target, corner, flip, arc);
        let a = self.active.as_mut().unwrap();
        a.preview = preview;
        a.lands = lands && (target[0] - from[0]).hypot(target[1] - from[1]) > 0.01;
        a.landing = landing;
        let nets = [net, partner.flatten()];
        let placed: Vec<(u8, P, P, f64)> = a
            .layout(width)
            .into_iter()
            .filter(|p| p.live)
            .flat_map(|p| {
                let (l, w) = (p.layer, p.width);
                p.piece.chords().into_iter().map(move |(x, y)| (l, x, y, w))
            })
            .collect();
        let obstacles = self.obstacles(board);
        let mut hit: Option<Hit> = None;
        for (l, p, q, w) in placed {
            if let Some(h) = obstacles.check_pair_track(l, p, q, w, nets, clearance) {
                if hit.as_ref().is_none_or(|x| h.gap < x.gap) {
                    hit = Some(h);
                }
            }
        }
        self.active.as_mut().unwrap().hit = hit;
    }

    /// Fixes the corners up to the cursor. Returns the edits that place the
    /// route when it has reached copper of its own net.
    pub fn click(&mut self, board: &Board) -> Option<Vec<Edit>> {
        let a = self.active.as_mut()?;
        if let Some(h) = &a.hit {
            self.message = Some(too_close(board, h, self.clearance));
            return None;
        }
        let lands = a.lands;
        if lands {
            let landing = a.landing;
            if let Some(l) = landing {
                a.runs.last_mut().unwrap().to = Some(l);
            }
        }
        a.fix_preview();
        self.message = None;
        if lands {
            return self.finish(board);
        }
        None
    }

    /// Places a via (a pair of vias for a pair) at the last fixed point,
    /// after fixing the preview, and carries on on `next` layer.
    pub fn via(&mut self, board: &Board, next: u8) {
        let (d, clearance) = (self.via_d, self.clearance);
        if self.active.as_ref().is_some_and(|a| a.layer() == next) {
            return;
        }
        if let Some(a) = self.active.as_mut().filter(|a| a.hit.is_none()) {
            a.fix_preview();
        }
        let Some(a) = self.active.as_ref() else {
            return;
        };
        let at = a.last_point();
        let net = a.net;
        // Pairs: one via per net, either side of the centre line, far
        // enough apart for both pads and the clearance between them.
        let pair_vias = a.pair.as_ref().and_then(|pair| {
            let run = a.runs.last().unwrap();
            let lay = a.layout(self.width);
            let mine: Vec<&Placed> = lay.iter().filter(|p| p.live && p.net == net).collect();
            let (fa, _) = run.from?;
            let a_end = mine.last().map(|p| p.piece.end()).unwrap_or(fa);
            let dir = run
                .clicks
                .iter()
                .flatten()
                .last()
                .and_then(|p| p.chords().last().copied())
                .map(|(x, y)| [y[0] - x[0], y[1] - x[1]])
                .unwrap_or([a_end[0] - at[0], a_end[1] - at[1]]);
            let l = dir[0].hypot(dir[1]).max(1e-9);
            let n = [-dir[1] / l, dir[0] / l];
            let side = if n[0] * (a_end[0] - at[0]) + n[1] * (a_end[1] - at[1]) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            let (w, gap) = a.size(run.layer, self.width);
            let half = (w + gap).max(d + clearance) / 2.0 * side;
            Some((
                [at[0] + n[0] * half, at[1] + n[1] * half],
                [at[0] - n[0] * half, at[1] - n[1] * half],
                pair.partner,
            ))
        });
        let nets = [net, a.pair.as_ref().and_then(|p| p.partner)];
        let spots: Vec<P> = match pair_vias {
            Some((va, vb, _)) => vec![va, vb],
            None => vec![at],
        };
        for &v in &spots {
            if let Some(h) = self.obstacles(board).check_pair_via(v, d, nets, clearance) {
                self.message = Some(format!(
                    "No room for a via here: {}",
                    too_close(board, &h, clearance)
                ));
                return;
            }
        }
        let a = self.active.as_mut().unwrap();
        let (from, to) = match pair_vias {
            Some((va, vb, partner)) => {
                a.vias.push((va, net));
                a.vias.push((vb, partner));
                (Some((va, vb)), Some((va, vb)))
            }
            None => {
                a.vias.push((at, net));
                (None, None)
            }
        };
        a.runs.last_mut().unwrap().to = to;
        a.runs.push(Run {
            layer: next,
            start: at,
            clicks: Vec::new(),
            from,
            to: None,
        });
        a.preview.clear();
        a.hit = None;
        self.message = None;
    }

    /// Takes back the last click, or the last via.
    pub fn back(&mut self) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let run = a.runs.last_mut().unwrap();
        if run.clicks.pop().is_some() {
            return;
        }
        if a.runs.len() > 1 {
            let pair = a.runs.last().unwrap().from.is_some();
            a.runs.pop();
            a.vias.pop();
            if pair {
                a.vias.pop();
            }
            a.runs.last_mut().unwrap().to = None;
        } else {
            self.active = None;
        }
    }

    /// Ends the route where it is and returns the edits that place it, or
    /// `None` when nothing was drawn.
    pub fn finish(&mut self, board: &Board) -> Option<Vec<Edit>> {
        let mut a = self.active.take()?;
        a.preview.clear();
        let point = |p: P| Point {
            x: Length::from_mil(p[0]),
            y: Length::from_mil(p[1]),
        };
        let mut edits = Vec::new();
        let mut ti = board.geometry.tracks.len();
        let mut ai = board.geometry.arcs.len();
        for placed in a.layout(self.width) {
            let layer = LayerRef::Copper(placed.layer);
            let width = Length::from_mil(placed.width);
            match placed.piece {
                Piece::Line(p, q) => {
                    edits.push(Edit::InsertTrack {
                        index: ti,
                        track: Track {
                            layer,
                            net: placed.net,
                            start: point(p),
                            end: point(q),
                            width,
                        },
                    });
                    ti += 1;
                }
                Piece::Arc {
                    c,
                    r,
                    start_deg,
                    end_deg,
                    ..
                } => {
                    edits.push(Edit::InsertArc {
                        index: ai,
                        arc: Arc {
                            layer,
                            net: placed.net,
                            center: point(c),
                            radius: Length::from_mil(r),
                            start_deg,
                            end_deg,
                            width,
                        },
                    });
                    ai += 1;
                }
            }
        }
        let copper = board.stackup.copper_layers().count().max(1) as u8;
        for (vi, &(v, net)) in (board.geometry.vias.len()..).zip(a.vias.iter()) {
            edits.push(Edit::InsertVia {
                index: vi,
                via: Via {
                    net,
                    from_layer: 0,
                    to_layer: copper - 1,
                    at: point(v),
                    diameter: Length::from_mil(self.via_d),
                    hole: Length::from_mil(self.via_hole),
                    rings: Vec::new(),
                },
            });
        }
        self.message = None;
        (!edits.is_empty()).then_some(edits)
    }
}

fn too_close(board: &Board, h: &Hit, clearance: f64) -> String {
    let name = h
        .net
        .and_then(|n| board.nets.get(n as usize))
        .map(|n| n.name.as_str())
        .unwrap_or("unconnected copper");
    if h.gap < 0.0 {
        format!("would short to {name}")
    } else {
        format!(
            "{:.1} mil from {name}, clearance is {:.1} mil",
            h.gap, clearance
        )
    }
}

/// Edits that remove every track, arc and via of a net.
pub fn unroute(board: &Board, net: u32) -> Vec<Edit> {
    let g = &board.geometry;
    let mut edits = Vec::new();
    for (i, t) in g.tracks.iter().enumerate().rev() {
        if t.net == Some(net) && matches!(t.layer, LayerRef::Copper(_)) {
            edits.push(Edit::RemoveTrack {
                index: i,
                track: t.clone(),
            });
        }
    }
    for (i, a) in g.arcs.iter().enumerate().rev() {
        if a.net == Some(net) && matches!(a.layer, LayerRef::Copper(_)) {
            edits.push(Edit::RemoveArc {
                index: i,
                arc: a.clone(),
            });
        }
    }
    for (i, v) in g.vias.iter().enumerate().rev() {
        if v.net == Some(net) {
            edits.push(Edit::RemoveVia {
                index: i,
                via: v.clone(),
            });
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::{LayerKind, Net, Pad, PadShape, StackLayer};

    fn pad(net: u32, x: f64, y: f64) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: None,
            name: String::new(),
            at: Point {
                x: Length::from_mil(x),
                y: Length::from_mil(y),
            },
            size_x: Length::from_mil(8.0),
            size_y: Length::from_mil(8.0),
            hole: Length(0),
            shape: PadShape::Round,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
        }
    }

    #[test]
    fn pair_route_with_vias_lands_on_both_pads() {
        let mut b = Board::default();
        for i in 0..2 {
            b.stackup.layers.push(StackLayer {
                name: format!("L{i}"),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                material: None,
            });
        }
        b.nets = vec![
            Net {
                name: "D_P".into(),
                ..Default::default()
            },
            Net {
                name: "D_N".into(),
                ..Default::default()
            },
        ];
        b.geometry.pads = vec![pad(0, 0.0, 10.0), pad(1, 0.0, -10.0)];
        let mut far = vec![pad(0, 600.0, 10.0), pad(1, 600.0, -10.0)];
        for p in &mut far {
            p.layer = LayerRef::AllCopper;
        }
        b.geometry.pads.extend(far);
        let mut r = Router::new();
        let info = PairInfo {
            name: "D".into(),
            partner: Some(1),
            sizes: vec![(5.0, 5.0), (4.0, 6.0)],
        };
        r.start([0.0, 10.0], Some(0), 0, Some((info, [0.0, -10.0])));
        let no_pad = |_: P, _: Option<u32>| None;
        r.update(&b, [300.0, 0.0], None, no_pad);
        assert!(r.active.as_ref().unwrap().hit.is_none());
        assert!(r.click(&b).is_none());
        r.via(&b, 1);
        assert_eq!(r.active.as_ref().unwrap().via_points().count(), 2);
        let partner =
            |p: P, n: Option<u32>| -> Option<P> { (n == Some(1)).then_some([p[0], -10.0]) };
        r.update(&b, [600.0, 10.0], Some(([600.0, 10.0], Some(0))), partner);
        assert!(r.active.as_ref().unwrap().lands);
        let edits = r.click(&b).expect("landed");
        let mut ends = Vec::new();
        let mut vias = 0;
        for e in &edits {
            match e {
                Edit::InsertTrack { track, .. } => {
                    ends.push((track.net, track.end, &track.layer, track.width))
                }
                Edit::InsertVia { via, .. } => {
                    vias += 1;
                    assert!(via.net.is_some());
                }
                _ => {}
            }
        }
        assert_eq!(vias, 2);
        let reach = |net: u32, y: f64| {
            ends.iter().any(|(n, e, _, _)| {
                *n == Some(net) && (e.x.mil() - 600.0).abs() < 1e-3 && (e.y.mil() - y).abs() < 1e-3
            })
        };
        assert!(reach(0, 10.0) && reach(1, -10.0), "{ends:?}");
        // Second layer uses that layer's width.
        assert!(ends
            .iter()
            .any(|(_, _, l, w)| **l == LayerRef::Copper(1) && (w.mil() - 4.0).abs() < 1e-3));
    }
}
