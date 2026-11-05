//! Interactive routing: click on copper to start, click to fix corners,
//! V for a via to the next layer, and the route ends on copper of its own
//! net or with Enter. Copper of other nets keeps its clearance; a route
//! that would break it is drawn red and is not placed.

use trace_core::edit::{Edit, History};
use trace_core::route::{self, Hit, Obstacles, P};
use trace_core::{Board, LayerRef, Length, Point, Track, Via};

/// One run of track on one layer; runs are joined by vias.
#[derive(Clone)]
struct Run {
    layer: u8,
    pts: Vec<P>,
}

pub struct Active {
    pub net: Option<u32>,
    runs: Vec<Run>,
    /// Via centres between consecutive runs.
    vias: Vec<P>,
    /// Corners from the last fixed point to the cursor.
    pub preview: Vec<P>,
    /// The preview ends on copper of the route's own net.
    pub lands: bool,
    pub hit: Option<Hit>,
}

impl Active {
    pub fn layer(&self) -> u8 {
        self.runs.last().map(|r| r.layer).unwrap_or(0)
    }
    pub fn last_point(&self) -> P {
        *self.runs.last().and_then(|r| r.pts.last()).unwrap()
    }
    /// Fixed segments, by layer, for drawing.
    pub fn fixed(&self) -> impl Iterator<Item = (u8, P, P)> + '_ {
        self.runs
            .iter()
            .flat_map(|r| r.pts.windows(2).map(move |w| (r.layer, w[0], w[1])))
    }
    pub fn via_points(&self) -> &[P] {
        &self.vias
    }
}

pub struct Router {
    pub width: f64,
    pub via_d: f64,
    pub via_hole: f64,
    pub clearance: f64,
    pub diagonal_first: bool,
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
            diagonal_first: false,
            active: None,
            history: History::default(),
            message: None,
            obstacles: None,
        }
    }

    /// New board: sizes from its rules and vias, empty history.
    pub fn load(&mut self, board: &Board) {
        *self = Router::new();
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

    pub fn start(&mut self, at: P, net: Option<u32>, layer: u8) {
        self.message = None;
        self.active = Some(Active {
            net,
            runs: vec![Run {
                layer,
                pts: vec![at],
            }],
            vias: Vec::new(),
            preview: Vec::new(),
            lands: false,
            hit: None,
        });
    }

    pub fn cancel(&mut self) {
        self.active = None;
        self.message = None;
    }

    /// Follows the cursor. `snap` is copper under the cursor on the
    /// route's layer: its centre or track end, and its net.
    pub fn update(&mut self, board: &Board, cursor: P, snap: Option<(P, Option<u32>)>) {
        let (width, clearance, diag) = (self.width, self.clearance, self.diagonal_first);
        let Some(a) = &self.active else { return };
        let (net, layer, from) = (a.net, a.layer(), a.last_point());
        let (target, lands) = match snap {
            Some((p, n)) if n.is_some() && n == net => (p, true),
            _ => (cursor, false),
        };
        let preview = route::path45(from, target, diag);
        let obstacles = self.obstacles(board);
        let mut hit: Option<Hit> = None;
        let mut prev = from;
        for &p in &preview {
            if let Some(h) = obstacles.check_track(layer, prev, p, width, net, clearance) {
                if hit.as_ref().is_none_or(|w| h.gap < w.gap) {
                    hit = Some(h);
                }
            }
            prev = p;
        }
        let a = self.active.as_mut().unwrap();
        a.preview = preview;
        a.lands = lands && (target[0] - from[0]).hypot(target[1] - from[1]) > 0.01;
        a.hit = hit;
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
        let preview = std::mem::take(&mut a.preview);
        a.runs.last_mut().unwrap().pts.extend(preview);
        self.message = None;
        if lands {
            return self.finish(board);
        }
        None
    }

    /// Places a via at the last fixed point (after fixing the preview) and
    /// carries on on `next` layer.
    pub fn via(&mut self, board: &Board, next: u8) {
        let (d, clearance) = (self.via_d, self.clearance);
        if self.active.as_ref().is_some_and(|a| a.layer() == next) {
            return;
        }
        if self.active.as_ref().is_some_and(|a| a.hit.is_none()) {
            let a = self.active.as_mut().unwrap();
            let preview = std::mem::take(&mut a.preview);
            a.runs.last_mut().unwrap().pts.extend(preview);
        }
        let Some((at, net)) = self.active.as_ref().map(|a| (a.last_point(), a.net)) else {
            return;
        };
        if let Some(h) = self.obstacles(board).check_via(at, d, net, clearance) {
            self.message = Some(format!(
                "No room for a via here: {}",
                too_close(board, &h, clearance)
            ));
            return;
        }
        let a = self.active.as_mut().unwrap();
        a.vias.push(at);
        a.runs.push(Run {
            layer: next,
            pts: vec![at],
        });
        a.preview.clear();
        a.hit = None;
        self.message = None;
    }

    /// Takes back the last corner, or the last via.
    pub fn back(&mut self) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let run = a.runs.last_mut().unwrap();
        if run.pts.len() > 1 {
            run.pts.pop();
        } else if a.runs.len() > 1 {
            a.runs.pop();
            a.vias.pop();
        } else {
            self.active = None;
        }
    }

    /// Ends the route where it is and returns the edits that place it, or
    /// `None` when nothing was drawn.
    pub fn finish(&mut self, board: &Board) -> Option<Vec<Edit>> {
        let a = self.active.take()?;
        let point = |p: P| Point {
            x: Length::from_mil(p[0]),
            y: Length::from_mil(p[1]),
        };
        let mut edits = Vec::new();
        let mut ti = board.geometry.tracks.len();
        for r in &a.runs {
            for w in r.pts.windows(2) {
                if (w[0][0] - w[1][0]).hypot(w[0][1] - w[1][1]) < 0.01 {
                    continue;
                }
                edits.push(Edit::InsertTrack {
                    index: ti,
                    track: Track {
                        layer: LayerRef::Copper(r.layer),
                        net: a.net,
                        start: point(w[0]),
                        end: point(w[1]),
                        width: Length::from_mil(self.width),
                    },
                });
                ti += 1;
            }
        }
        let copper = board.stackup.copper_layers().count().max(1) as u8;
        for (vi, &v) in (board.geometry.vias.len()..).zip(a.vias.iter()) {
            edits.push(Edit::InsertVia {
                index: vi,
                via: Via {
                    net: a.net,
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
