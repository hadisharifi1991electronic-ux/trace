//! Interactive routing: click on copper to start, click to fix corners,
//! V for a via to the next layer, and the route ends on copper of its own
//! net or with Enter. Shift+Space cycles the corner style and Space flips
//! the bend, as in Altium. Starting on a net of a differential pair routes
//! both nets together along a centre line at the pair's width and gap.
//! Ctrl+click on several pads gathers a bus: its nets are routed side by
//! side, in the order of their pads, at the track width and the bus gap.
//! Copper of other nets keeps its clearance; a route that would break it is
//! drawn red and is not placed.

use trace_core::bus;
use trace_core::edit::{Edit, History};
use trace_core::route::{self, Corner, Hit, Obstacles, Piece, ViaKind, P};
use trace_core::walk::{self, Ask, Shove};
use trace_core::{Arc, Board, LayerRef, Length, Point, Track, Via};

/// One run of track on one layer; runs are joined by vias. Each click adds
/// a group of centre-line pieces, so Backspace takes back a click at a time.
#[derive(Clone)]
struct Run {
    layer: u8,
    start: P,
    clicks: Vec<Vec<Piece>>,
    /// Pairs and buses: where each lane's track starts (pads or vias), in
    /// lane order, and where they end when vias closed the run.
    from: Option<Vec<P>>,
    to: Option<Vec<P>>,
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

/// What the route does with copper of other nets in its way, cycled with
/// Shift+R as in Altium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Finds a way round it.
    Walkaround,
    /// Pushes tracks aside, and goes round what cannot be pushed.
    Push,
    /// Stops: the route turns red and is not placed.
    Stop,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Walkaround => Mode::Push,
            Mode::Push => Mode::Stop,
            Mode::Stop => Mode::Walkaround,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Walkaround => "Walkaround",
            Mode::Push => "Push and walkaround",
            Mode::Stop => "Stop at obstacle",
        }
    }
}

pub struct Active {
    pub net: Option<u32>,
    pub pair: Option<PairInfo>,
    /// Every net routed, in lane order across the route: one, the two of a
    /// pair, or a bus.
    pub lanes: Vec<Option<u32>>,
    /// A bus: the gap between neighbouring tracks.
    pub bus_gap: Option<f64>,
    runs: Vec<Run>,
    /// Vias placed so far, with their nets and kinds.
    vias: Vec<(P, Option<u32>, ViaKind)>,
    /// How many vias each layer change placed, so Backspace takes back
    /// a whole stack.
    via_groups: Vec<usize>,
    /// Centre-line pieces from the last fixed point to the cursor.
    pub preview: Vec<Piece>,
    /// The preview ends on copper of the route's own net (for a pair: on
    /// the pair's pads, the routed net's first).
    pub lands: bool,
    landing: Option<Vec<P>>,
    pub hit: Option<Hit>,
    /// Tracks the preview pushes aside, shown moved and placed with the
    /// next click.
    pub shove: Option<Shove>,
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
    /// Each via placed so far: centre, pad and drill diameter.
    pub fn via_shapes(&self) -> impl Iterator<Item = (P, f64, f64)> + '_ {
        self.vias.iter().map(|v| (v.0, v.2.diameter, v.2.hole))
    }
    fn fix_preview(&mut self) {
        let preview = std::mem::take(&mut self.preview);
        if !preview.is_empty() {
            self.runs.last_mut().unwrap().clicks.push(preview);
        }
    }
    fn size(&self, layer: u8, single: f64) -> (f64, f64) {
        match (&self.pair, self.bus_gap) {
            (Some(p), _) => p
                .sizes
                .get(layer as usize)
                .copied()
                .unwrap_or((single, single)),
            (None, Some(gap)) => (single, gap),
            (None, None) => (single, 0.0),
        }
    }

    /// A bus of more than two nets (whose lanes fan in at 45°).
    pub fn is_bus(&self) -> bool {
        self.lanes.len() > 2
    }

    /// Half the width of everything routed along the centre line.
    fn half_width(&self, layer: u8, single: f64) -> f64 {
        let (w, gap) = self.size(layer, single);
        (self.lanes.len().max(1) as f64 - 1.0) / 2.0 * (w + gap) + w / 2.0
    }

    /// Every piece of copper the route would place now, including the part
    /// that follows the cursor.
    pub fn layout(&self, single: f64) -> Vec<Placed> {
        let mut out = Vec::new();
        let last = self.runs.len() - 1;
        for (ri, run) in self.runs.iter().enumerate() {
            let live = ri == last;
            let mut centre: Vec<Piece> = run.clicks.iter().flatten().copied().collect();
            let mut to = run.to.clone();
            if live {
                centre.extend(self.preview.iter().copied());
                if to.is_none() && self.lands {
                    to = self.landing.clone();
                }
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
            match &run.from {
                Some(from) if self.lanes.len() >= 2 && from.len() == self.lanes.len() => {
                    if centre.is_empty() {
                        continue;
                    }
                    let n = self.lanes.len();
                    let side = bus::side(&centre, run.start, from);
                    let fan = self.is_bus();
                    for (i, &net) in self.lanes.iter().enumerate() {
                        let k = side * bus::slot(i, n) * (w + gap);
                        let t = to.as_ref().and_then(|t| t.get(i).copied());
                        for p in bus::lane(&centre, k, from[i], t, fan) {
                            push(net, p);
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

/// Length tuning settings and the meander following the cursor.
pub struct Tune {
    /// Length to reach, mil; `None` takes it from the Constraint Manager.
    pub target: Option<f64>,
    pub amp: f64,
    pub space: f64,
    pub flip: bool,
    pub preview: Option<TunePreview>,
}

pub struct TunePreview {
    pub track: usize,
    pub layer: u8,
    pub net: u32,
    pub width: f64,
    pub pieces: Vec<Piece>,
    pub added: f64,
    pub length: f64,
    pub target: f64,
    pub why: String,
    pub hit: Option<Hit>,
}

impl TunePreview {
    /// Removes the straight track and puts the meander in its place.
    pub fn edits(&self, board: &Board) -> Vec<Edit> {
        let Some(old) = board.geometry.tracks.get(self.track) else {
            return Vec::new();
        };
        let mut edits = vec![Edit::RemoveTrack {
            index: self.track,
            track: old.clone(),
        }];
        let start = board.geometry.tracks.len() - 1;
        for (k, p) in self.pieces.iter().enumerate() {
            edits.push(Edit::InsertTrack {
                index: start + k,
                track: Track {
                    component: None,
                    layer: old.layer.clone(),
                    net: old.net,
                    start: Point {
                        x: Length::from_mil(p.start()[0]),
                        y: Length::from_mil(p.start()[1]),
                    },
                    end: Point {
                        x: Length::from_mil(p.end()[0]),
                        y: Length::from_mil(p.end()[1]),
                    },
                    width: old.width,
                    text: false,
                },
            });
        }
        edits
    }
}

pub struct Router {
    pub tune: Tune,
    pub width: f64,
    /// Size of the through via.
    pub via_d: f64,
    pub via_hole: f64,
    /// Blind, buried and micro vias the router may use (not the through
    /// via), with their sizes.
    pub kinds: Vec<ViaKind>,
    /// The via to place at a layer change: `None` picks the narrowest
    /// via or stack that joins the two layers; `Some(i)` is `kinds[i]`,
    /// or the through via past the end of `kinds`.
    pub via_choice: Option<usize>,
    pub clearance: f64,
    pub corner: Corner,
    /// Space: which run of a corner comes first.
    pub flip: bool,
    /// Round corners as a fraction of the largest that fits (, and .).
    pub arc: f64,
    /// Route both nets of a differential pair together.
    pub pairs: bool,
    /// Gap between neighbouring tracks of a bus.
    pub bus_gap: f64,
    /// Bus starts gathered with Ctrl+click (pad or track end, net), before
    /// the bus is routed.
    pub bus: Vec<(P, u32)>,
    /// The nets of the bus placed last, for matching their lengths.
    pub last_bus: Vec<u32>,
    pub active: Option<Active>,
    pub history: History,
    pub message: Option<String>,
    obstacles: Option<Obstacles>,
    pub mode: Mode,
    /// Edits that push tracks aside, fixed by a click before the route is
    /// placed; the app applies them as their own step.
    pub shoved: Option<Vec<Edit>>,
    /// What the last update was worked out for; the same again (the
    /// cursor still) needs no new search.
    seen: Option<String>,
}

impl Router {
    pub fn new() -> Self {
        Router {
            tune: Tune {
                target: None,
                amp: 40.0,
                space: 0.0,
                flip: false,
                preview: None,
            },
            width: route::DEFAULT_WIDTH,
            via_d: 20.0,
            via_hole: 10.0,
            kinds: Vec::new(),
            via_choice: None,
            clearance: route::DEFAULT_CLEARANCE,
            corner: Corner::Line45,
            flip: false,
            arc: 1.0,
            pairs: true,
            bus_gap: route::DEFAULT_CLEARANCE,
            bus: Vec::new(),
            last_bus: Vec::new(),
            active: None,
            history: History::default(),
            message: None,
            obstacles: None,
            mode: Mode::Walkaround,
            shoved: None,
            seen: None,
        }
    }

    /// New board: sizes from its rules and vias, empty history. The corner
    /// style is kept.
    pub fn load(&mut self, board: &Board) {
        let (corner, arc, pairs, mode) = (self.corner, self.arc, self.pairs, self.mode);
        *self = Router::new();
        (self.corner, self.arc, self.pairs, self.mode) = (corner, arc, pairs, mode);
        self.width = route::board_width(board);
        self.clearance = route::board_clearance(board);
        self.bus_gap = self.clearance;
        let n = board.stackup.copper_layers().count().max(1) as u8;
        let kinds = route::via_kinds(board);
        match kinds.iter().find(|k| k.from == 0 && k.to >= n - 1) {
            Some(t) => (self.via_d, self.via_hole) = (t.diameter, t.hole),
            None => (self.via_d, self.via_hole) = route::common_via(board),
        }
        self.kinds = kinds
            .into_iter()
            .filter(|k| !(k.from == 0 && k.to >= n - 1))
            .collect();
    }

    /// The through via on a board of `n` copper layers, at its set size.
    pub fn through(&self, n: u8) -> ViaKind {
        ViaKind {
            from: 0,
            to: n.saturating_sub(1),
            diameter: self.via_d,
            hole: self.via_hole,
        }
    }

    /// The vias a change from layer `a` to `b` places at one spot, or why
    /// none can.
    pub fn via_stack(&self, n: u8, a: u8, b: u8) -> Result<Vec<ViaKind>, String> {
        let through = self.through(n);
        match self.via_choice {
            Some(i) => {
                let k = self.kinds.get(i).copied().unwrap_or(through);
                if k.spans(a) && k.spans(b) {
                    Ok(vec![k])
                } else {
                    Err(format!(
                        "The chosen via joins L{} to L{}, not L{} to L{}: pick another via or Auto",
                        k.from + 1,
                        k.to + 1,
                        a + 1,
                        b + 1
                    ))
                }
            }
            None => {
                let mut all = self.kinds.clone();
                all.push(through);
                let chain = route::via_chain(&all, a, b);
                if chain.is_empty() {
                    Err(format!("No via joins L{} to L{}", a + 1, b + 1))
                } else {
                    Ok(chain)
                }
            }
        }
    }

    /// The board changed; obstacles are rebuilt on next use.
    pub fn board_changed(&mut self) {
        self.obstacles = None;
        self.seen = None;
    }

    pub fn obstacles(&mut self, board: &Board) -> &Obstacles {
        self.obstacles.get_or_insert_with(|| Obstacles::new(board))
    }

    /// Starts a route at `at`. For a pair, `partner_pad` is where the
    /// partner's track starts.
    pub fn start(&mut self, at: P, net: Option<u32>, layer: u8, pair: Option<(PairInfo, P)>) {
        self.seen = None;
        self.message = None;
        self.bus.clear();
        let (pair, from, start) = match pair {
            Some((info, b)) => (
                Some(info),
                Some(vec![at, b]),
                [(at[0] + b[0]) / 2.0, (at[1] + b[1]) / 2.0],
            ),
            None => (None, None, at),
        };
        let lanes = match &pair {
            Some(p) => vec![net, p.partner],
            None => vec![net],
        };
        self.begin(net, pair, lanes, None, layer, start, from);
    }

    /// Adds a bus start (Ctrl+click), or takes it back when its net is
    /// already in the bus. Returns how many nets the bus has.
    pub fn pick_bus(&mut self, at: P, net: u32) -> usize {
        self.seen = None;
        match self.bus.iter().position(|b| b.1 == net) {
            Some(i) => {
                self.bus.remove(i);
            }
            None => self.bus.push((at, net)),
        }
        self.bus.len()
    }

    /// Starts routing the gathered bus on `layer`; false when fewer than
    /// two nets were gathered.
    pub fn start_bus(&mut self, layer: u8) -> bool {
        if self.bus.len() < 2 {
            return false;
        }
        self.seen = None;
        self.message = None;
        let mut starts = std::mem::take(&mut self.bus);
        bus::order(&mut starts);
        let n = starts.len() as f64;
        let start = starts
            .iter()
            .fold([0.0, 0.0], |s, (p, _)| [s[0] + p[0] / n, s[1] + p[1] / n]);
        let lanes: Vec<Option<u32>> = starts.iter().map(|s| Some(s.1)).collect();
        let from = starts.iter().map(|s| s.0).collect();
        self.begin(
            lanes[0],
            None,
            lanes,
            Some(self.bus_gap),
            layer,
            start,
            Some(from),
        );
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        net: Option<u32>,
        pair: Option<PairInfo>,
        lanes: Vec<Option<u32>>,
        bus_gap: Option<f64>,
        layer: u8,
        start: P,
        from: Option<Vec<P>>,
    ) {
        self.active = Some(Active {
            net,
            pair,
            lanes,
            bus_gap,
            runs: vec![Run {
                layer,
                start,
                clicks: Vec::new(),
                from,
                to: None,
            }],
            vias: Vec::new(),
            via_groups: Vec::new(),
            preview: Vec::new(),
            lands: false,
            landing: None,
            hit: None,
            shove: None,
        });
    }

    pub fn cancel(&mut self) {
        self.seen = None;
        self.active = None;
        self.bus.clear();
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
        let from = a.last_point();
        let lanes = a.lanes.clone();
        let clicks: usize = a.runs.iter().map(|r| r.clicks.len()).sum();
        let key = format!(
            "{cursor:?} {snap:?} {from:?} {} {} {clicks} {:?} {corner:?} {flip} {arc} {width} {clearance}",
            a.layer(),
            a.runs.len(),
            self.mode,
        );
        if self.seen.as_deref() == Some(key.as_str()) {
            return;
        }
        self.seen = Some(key);
        let mut landing = None;
        // Landing: on copper of a routed net, with every other lane's pad
        // next to it.
        let (target, lands) = match snap {
            Some((p, n)) if n.is_some() && lanes.contains(&n) => {
                if lanes.len() == 1 {
                    (p, true)
                } else {
                    let ends: Option<Vec<P>> = lanes
                        .iter()
                        .map(|&m| if m == n { Some(p) } else { partner_pad(p, m) })
                        .collect();
                    match ends {
                        Some(v) => {
                            let k = v.len() as f64;
                            let c = v
                                .iter()
                                .fold([0.0, 0.0], |s, q| [s[0] + q[0] / k, s[1] + q[1] / k]);
                            landing = Some(v);
                            (c, true)
                        }
                        None => (cursor, false),
                    }
                }
            }
            _ => (cursor, false),
        };
        let preview = route::corner_path(from, target, corner, flip, arc);
        let a = self.active.as_mut().unwrap();
        a.preview = preview;
        a.lands = lands && (target[0] - from[0]).hypot(target[1] - from[1]) > 0.01;
        a.landing = landing;
        let nets = lanes;
        let mode = self.mode;
        let mut hit = self.live_hit(board, &nets, clearance);
        let a = self.active.as_mut().unwrap();
        a.shove = None;
        if hit.is_none() || mode == Mode::Stop {
            self.active.as_mut().unwrap().hit = hit;
            return;
        }
        let layer = a.layer();
        let half = a.half_width(layer, width);
        // Push: move the tracks in the way, if nothing else is.
        if mode == Mode::Push {
            let caps: Vec<walk::Capsule> = a
                .layout(width)
                .into_iter()
                .filter(|p| p.live)
                .flat_map(|p| {
                    let r = p.width / 2.0;
                    p.piece.chords().into_iter().map(move |(x, y)| (x, y, r))
                })
                .collect();
            let obstacles = self.obstacles(board);
            if let Some(s) = walk::shove(board, obstacles, layer, &caps, &nets, clearance) {
                let skip = |t: usize| s.remove.contains(&t);
                let clear = caps
                    .iter()
                    .all(|&(x, y, r)| obstacles.clear(layer, x, y, r, &nets, clearance, &skip));
                if clear {
                    let a = self.active.as_mut().unwrap();
                    a.shove = Some(s);
                    a.hit = None;
                    return;
                }
            }
        }
        // Walkaround: a 45° path round what is in the way.
        let obstacles = self.obstacles(board);
        let ask = Ask {
            obstacles,
            layer,
            r: half,
            nets: &nets,
            clearance,
            extra: &[],
            skip: &[],
        };
        // When there is no way round, as far as it goes: the route stops
        // short of what is in the way, as in Altium.
        if let Some((path, reached)) = walk::walk_toward(&ask, from, target) {
            let pieces: Vec<Piece> = path.windows(2).map(|w| Piece::Line(w[0], w[1])).collect();
            let a = self.active.as_mut().unwrap();
            let keep = std::mem::replace(&mut a.preview, pieces);
            let (lands, landing) = (a.lands, a.landing.clone());
            if !reached {
                a.lands = false;
                a.landing = None;
            }
            let again = self.live_hit(board, &nets, clearance);
            let a = self.active.as_mut().unwrap();
            if again.is_none() {
                hit = None;
            } else {
                a.preview = keep;
                (a.lands, a.landing) = (lands, landing);
            }
        }
        self.active.as_mut().unwrap().hit = hit;
    }

    /// The worst clearance break of the part of the route that follows the
    /// cursor.
    /// A pair's tracks may come close to the partner's pads; a bus track
    /// keeps its clearance from the other lanes' copper.
    fn live_hit(&mut self, board: &Board, nets: &[Option<u32>], clearance: f64) -> Option<Hit> {
        let width = self.width;
        let a = self.active.as_ref()?;
        let own = a.is_bus();
        let placed: Vec<(u8, P, P, f64, Option<u32>)> = a
            .layout(width)
            .into_iter()
            .filter(|p| p.live)
            .flat_map(|p| {
                let (l, w, n) = (p.layer, p.width, p.net);
                p.piece
                    .chords()
                    .into_iter()
                    .map(move |(x, y)| (l, x, y, w, n))
            })
            .collect();
        let obstacles = self.obstacles(board);
        let mut hit: Option<Hit> = None;
        for (l, p, q, w, n) in placed {
            let skip = if own { std::slice::from_ref(&n) } else { nets };
            if let Some(h) = obstacles.check_pair_track(l, p, q, w, skip, clearance) {
                if hit.as_ref().is_none_or(|x| h.gap < x.gap) {
                    hit = Some(h);
                }
            }
        }
        hit
    }

    /// Fixes the corners up to the cursor. Returns the edits that place the
    /// route when it has reached copper of its own net.
    pub fn click(&mut self, board: &Board) -> Option<Vec<Edit>> {
        self.seen = None;
        let a = self.active.as_mut()?;
        if let Some(h) = &a.hit {
            self.message = Some(too_close(board, h, self.clearance));
            return None;
        }
        let shove = a.shove.take();
        let lands = a.lands;
        if lands {
            let landing = a.landing.clone();
            if let Some(l) = landing {
                a.runs.last_mut().unwrap().to = Some(l);
            }
        }
        a.fix_preview();
        self.message = None;
        if lands {
            let mut edits = self.finish(board)?;
            if let Some(s) = shove {
                // The pushed tracks go in after the route's own.
                let added = edits
                    .iter()
                    .filter(|e| matches!(e, Edit::InsertTrack { .. }))
                    .count();
                edits.extend(s.edits(board, board.geometry.tracks.len() + added));
            }
            return Some(edits);
        }
        if let Some(s) = shove {
            self.shoved = Some(s.edits(board, board.geometry.tracks.len()));
        }
        None
    }

    /// Places a via (a pair of vias for a pair) at the last fixed point,
    /// after fixing the preview, and carries on on `next` layer.
    pub fn via(&mut self, board: &Board, next: u8) {
        self.seen = None;
        let clearance = self.clearance;
        if self.active.as_ref().is_some_and(|a| a.layer() == next) {
            return;
        }
        let n = board.stackup.copper_layers().count().max(1) as u8;
        let cur = self.active.as_ref().map(|a| a.layer()).unwrap_or(0);
        let stack = match self.via_stack(n, cur, next) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        let d = stack.iter().map(|k| k.diameter).fold(0.0, f64::max);
        if let Some(a) = self.active.as_mut().filter(|a| a.hit.is_none()) {
            a.fix_preview();
        }
        let Some(a) = self.active.as_ref() else {
            return;
        };
        let at = a.last_point();
        let net = a.net;
        // Pairs and buses: one via per net in a row across the route, far
        // enough apart for the via pads and the clearance between them.
        let multi = a.lanes.len() >= 2;
        let mut spots: Vec<P> = vec![at];
        if multi {
            let run = a.runs.last().unwrap();
            let centre: Vec<Piece> = run.clicks.iter().flatten().copied().collect();
            let (Some(from), Some(last)) = (
                run.from.as_ref(),
                centre.last().and_then(|p| p.chords().last().copied()),
            ) else {
                self.message = Some("Route away from the pads before changing layer.".into());
                return;
            };
            let side = bus::side(&centre, run.start, from);
            let dir = [last.1[0] - last.0[0], last.1[1] - last.0[1]];
            let l = dir[0].hypot(dir[1]).max(1e-9);
            let n = [-dir[1] / l, dir[0] / l];
            let (w, gap) = a.size(run.layer, self.width);
            let step = (w + gap).max(d + clearance);
            let k = a.lanes.len();
            spots = (0..k)
                .map(|i| {
                    let o = side * bus::slot(i, k) * step;
                    [at[0] + n[0] * o, at[1] + n[1] * o]
                })
                .collect();
        }
        let nets = a.lanes.clone();
        for &v in &spots {
            for k in &stack {
                if let Some(h) = self
                    .obstacles(board)
                    .check_span_via(v, k.diameter, &nets, clearance, k.from, k.to)
                {
                    self.message = Some(format!(
                        "No room for a via here: {}",
                        too_close(board, &h, clearance)
                    ));
                    return;
                }
            }
        }
        let a = self.active.as_mut().unwrap();
        let (from, to) = if multi {
            for (&v, &lane) in spots.iter().zip(&nets) {
                for k in &stack {
                    a.vias.push((v, lane, *k));
                }
            }
            a.via_groups.push(stack.len() * spots.len());
            (Some(spots.clone()), Some(spots))
        } else {
            for k in &stack {
                a.vias.push((at, net, *k));
            }
            a.via_groups.push(stack.len());
            (None, None)
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
        self.seen = None;
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let run = a.runs.last_mut().unwrap();
        if run.clicks.pop().is_some() {
            return;
        }
        if a.runs.len() > 1 {
            let lanes = a.lanes.len();
            a.runs.pop();
            let n = a.via_groups.pop().unwrap_or(lanes);
            for _ in 0..n {
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
        if a.is_bus() {
            self.last_bus = a.lanes.iter().flatten().copied().collect();
        }
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
                            component: None,
                            layer,
                            net: placed.net,
                            start: point(p),
                            end: point(q),
                            width,
                            text: false,
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
                            component: None,
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
        for (vi, &(v, net, k)) in (board.geometry.vias.len()..).zip(a.vias.iter()) {
            edits.push(Edit::InsertVia {
                index: vi,
                via: Via {
                    net,
                    from_layer: k.from,
                    to_layer: k.to,
                    at: point(v),
                    diameter: Length::from_mil(k.diameter),
                    hole: Length::from_mil(k.hole),
                    rings: Vec::new(),
                    fab: Default::default(),
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
            fab: Default::default(),
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

    #[test]
    fn layer_changes_stack_microvias_and_buried_vias() {
        let mut b = Board::default();
        for i in 0..6 {
            b.stackup.layers.push(StackLayer {
                name: format!("L{i}"),
                altium_id: None,
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                material: None,
            });
        }
        b.nets.push(Net {
            name: "A".into(),
            ..Default::default()
        });
        b.geometry.pads = vec![pad(0, 0.0, 0.0)];
        let mut r = Router::new();
        let k = |from, to, d| ViaKind {
            from,
            to,
            diameter: d,
            hole: d / 2.0,
        };
        r.kinds = vec![k(0, 1, 10.0), k(1, 2, 10.0), k(2, 3, 16.0)];
        let no_pad = |_: P, _: Option<u32>| None;
        r.start([0.0, 0.0], Some(0), 0, None);
        r.update(&b, [200.0, 0.0], None, no_pad);
        assert!(r.click(&b).is_none());
        // Top to L3: microvias 0-1 and 1-2 stacked.
        r.via(&b, 2);
        assert_eq!(r.active.as_ref().unwrap().layer(), 2, "{:?}", r.message);
        r.update(&b, [200.0, 200.0], None, no_pad);
        assert!(r.click(&b).is_none());
        // L3 to L4: the buried 2-3.
        r.via(&b, 3);
        // L4 to the bottom: no blind via reaches it but the through via.
        r.via(&b, 5);
        assert_eq!(r.active.as_ref().unwrap().layer(), 5, "{:?}", r.message);
        // Backspace takes back the whole last stack.
        r.back();
        assert_eq!(r.active.as_ref().unwrap().layer(), 3);
        // A chosen via that does not reach says so.
        r.via_choice = Some(0);
        r.via(&b, 4);
        assert!(r.message.as_deref().unwrap_or("").contains("not L4 to L5"));
        r.via_choice = None;
        let edits = r.finish(&b).unwrap();
        let spans: Vec<(u8, u8, f64)> = edits
            .iter()
            .filter_map(|e| match e {
                Edit::InsertVia { via, .. } => {
                    Some((via.from_layer, via.to_layer, via.diameter.mil()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(spans, vec![(0, 1, 10.0), (1, 2, 10.0), (2, 3, 16.0)]);
    }

    #[test]
    fn pair_walks_round_a_pad_in_the_way() {
        let mut b = Board::default();
        b.stackup.layers.push(StackLayer {
            name: "L0".into(),
            altium_id: None,
            kind: LayerKind::Copper,
            thickness: Length::from_mil(1.4),
            dielectric_constant: None,
            material: None,
        });
        b.nets = ["D_P", "D_N", "X"]
            .iter()
            .map(|n| Net {
                name: (*n).into(),
                ..Default::default()
            })
            .collect();
        let mut block = pad(2, 150.0, 0.0);
        block.size_x = Length::from_mil(30.0);
        block.size_y = Length::from_mil(30.0);
        b.geometry.pads = vec![pad(0, 0.0, 10.0), pad(1, 0.0, -10.0), block];
        let mut r = Router::new();
        r.clearance = 5.0;
        let info = PairInfo {
            name: "D".into(),
            partner: Some(1),
            sizes: vec![(5.0, 5.0)],
        };
        r.start([0.0, 10.0], Some(0), 0, Some((info, [0.0, -10.0])));
        let no_pad = |_: P, _: Option<u32>| None;
        r.mode = Mode::Stop;
        r.update(&b, [300.0, 0.0], None, no_pad);
        assert!(
            r.active.as_ref().unwrap().hit.is_some(),
            "straight through the pad"
        );
        r.mode = Mode::Walkaround;
        r.update(&b, [300.0, 0.0], None, no_pad);
        let a = r.active.as_ref().unwrap();
        assert!(a.hit.is_none());
        assert!(a.preview.len() > 1, "goes round");
        // Both tracks keep clear of the pad.
        let obs = Obstacles::new(&b);
        for p in a.layout(r.width).iter().filter(|p| p.live) {
            for (x, y) in p.piece.chords() {
                let hit = obs.check_pair_track(0, x, y, p.width, &[Some(0), Some(1)], 5.0);
                assert!(hit.is_none(), "{x:?} {y:?}");
            }
        }
    }

    #[test]
    fn bus_routes_lanes_in_pad_order_and_lands_on_every_pad() {
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
        b.nets = (0..4)
            .map(|i| Net {
                name: format!("D{i}"),
                ..Default::default()
            })
            .collect();
        // Starts in a column at 25 mil pitch, ends in a column at 2000.
        for i in 0..4u32 {
            let y = 25.0 * i as f64 - 37.5;
            b.geometry.pads.push(pad(i, 0.0, y));
            let mut far = pad(i, 2000.0, y + 300.0);
            far.layer = LayerRef::AllCopper;
            b.geometry.pads.push(far);
        }
        let mut r = Router::new();
        r.width = 5.0;
        r.clearance = 5.0;
        r.bus_gap = 5.0;
        r.mode = Mode::Stop;
        // Gathered out of order; the bus sorts them.
        for i in [2u32, 0, 3, 1] {
            r.pick_bus([0.0, 25.0 * i as f64 - 37.5], i);
        }
        assert!(r.start_bus(0));
        let lanes = r.active.as_ref().unwrap().lanes.clone();
        assert!(
            lanes == [0, 1, 2, 3].map(Some) || lanes == [3, 2, 1, 0].map(Some),
            "{lanes:?}"
        );
        let no_pad = |_: P, _: Option<u32>| None;
        r.update(&b, [800.0, 0.0], None, no_pad);
        assert!(
            r.active.as_ref().unwrap().hit.is_none(),
            "{:?}",
            r.active.as_ref().unwrap().hit
        );
        assert!(r.click(&b).is_none());
        r.via(&b, 1);
        assert_eq!(
            r.active.as_ref().unwrap().via_points().count(),
            4,
            "{:?}",
            r.message
        );
        let ends = |_: P, n: Option<u32>| -> Option<P> {
            let i = n? as f64;
            Some([2000.0, 25.0 * i - 37.5 + 300.0])
        };
        // A corner first, so the bus comes in square to the far pads.
        r.update(&b, [1500.0, 300.0], None, no_pad);
        assert!(r.active.as_ref().unwrap().hit.is_none());
        assert!(r.click(&b).is_none());
        r.update(&b, [2000.0, 262.5], Some(([2000.0, 262.5], Some(0))), ends);
        let a = r.active.as_ref().unwrap();
        assert!(a.lands);
        assert!(a.hit.is_none(), "{:?}", a.hit);
        let edits = r.click(&b).expect("landed");
        let mut tracks: Vec<Track> = Vec::new();
        for e in &edits {
            if let Edit::InsertTrack { track, .. } = e {
                tracks.push(track.clone());
            }
        }
        for i in 0..4u32 {
            let y = 25.0 * i as f64 - 37.5;
            let at = |t: &Track, x: f64, y: f64| {
                let m = |p: Point| (p.x.mil() - x).abs() < 1e-3 && (p.y.mil() - y).abs() < 1e-3;
                m(t.start) || m(t.end)
            };
            assert!(
                tracks.iter().any(|t| t.net == Some(i) && at(t, 0.0, y)),
                "start {i}"
            );
            assert!(
                tracks
                    .iter()
                    .any(|t| t.net == Some(i) && at(t, 2000.0, y + 300.0)),
                "end {i}"
            );
        }
        // No two lanes on the same layer come closer than the gap.
        for (i, t) in tracks.iter().enumerate() {
            for u in &tracks[i + 1..] {
                if t.net == u.net || t.layer != u.layer {
                    continue;
                }
                let p = |q: Point| [q.x.mil(), q.y.mil()];
                let obs = Obstacles::new(&Board {
                    geometry: trace_core::Geometry {
                        tracks: vec![u.clone()],
                        ..Default::default()
                    },
                    stackup: b.stackup.clone(),
                    ..Default::default()
                });
                let layer = match t.layer {
                    LayerRef::Copper(l) => l,
                    _ => 0,
                };
                assert!(
                    obs.check_track(layer, p(t.start), p(t.end), 5.0, t.net, 4.9)
                        .is_none(),
                    "{t:?} {u:?}"
                );
            }
        }
    }
}
