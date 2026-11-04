//! A loaded board turned into drawable primitives (in mil, y up), with an
//! index per net and a grid for picking.

use std::collections::HashMap;
use std::path::PathBuf;
use trace_core::{Board, LayerRef, PadShape};

/// Where a primitive is drawn. Copper layers are 0 (top) to n-1 (bottom).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    Copper(u8),
    TopSilk,
    BottomSilk,
    Vias,
    Other,
}

#[derive(Clone, Copy)]
pub enum Geom {
    /// Segment with full width.
    Seg {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        w: f32,
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
    },
    /// Rectangle centred on (x, y), rotated by `rot` degrees.
    Rect {
        x: f32,
        y: f32,
        sx: f32,
        sy: f32,
        rot: f32,
    },
}

impl Geom {
    pub fn bbox(&self) -> [f32; 4] {
        match *self {
            Geom::Seg { x1, y1, x2, y2, w } => [
                x1.min(x2) - w / 2.0,
                y1.min(y2) - w / 2.0,
                x1.max(x2) + w / 2.0,
                y1.max(y2) + w / 2.0,
            ],
            Geom::Circle { x, y, r } => [x - r, y - r, x + r, y + r],
            Geom::Rect { x, y, sx, sy, .. } => {
                let r = sx.hypot(sy) / 2.0;
                [x - r, y - r, x + r, y + r]
            }
        }
    }

    /// Distance from a point to the shape's edge (negative inside).
    pub fn distance(&self, px: f32, py: f32) -> f32 {
        match *self {
            Geom::Seg { x1, y1, x2, y2, w } => {
                let (dx, dy) = (x2 - x1, y2 - y1);
                let l = dx * dx + dy * dy;
                let t = if l > 0.0 {
                    (((px - x1) * dx + (py - y1) * dy) / l).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (px - (x1 + t * dx)).hypot(py - (y1 + t * dy)) - w / 2.0
            }
            Geom::Circle { x, y, r } => (px - x).hypot(py - y) - r,
            Geom::Rect { x, y, sx, sy, rot } => {
                let (s, c) = (-rot.to_radians()).sin_cos();
                let (u, v) = ((px - x) * c - (py - y) * s, (px - x) * s + (py - y) * c);
                (u.abs() - sx / 2.0).max(v.abs() - sy / 2.0)
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Prim {
    pub slot: Slot,
    pub net: Option<u32>,
    pub geom: Geom,
    /// Through-hole pads appear on every copper layer.
    pub all_layers: bool,
    pub component: Option<u32>,
}

pub struct Layer {
    pub slot: Slot,
    pub name: String,
    pub color: [u8; 3],
    pub visible: bool,
    pub count: usize,
}

pub struct Scene {
    pub path: PathBuf,
    pub board: Board,
    pub prims: Vec<Prim>,
    pub layers: Vec<Layer>,
    pub copper_count: u8,
    pub outline: Vec<[f32; 2]>,
    pub bounds: [f32; 4],
    pub by_net: HashMap<u32, Vec<usize>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    pub net_by_name: HashMap<String, u32>,
    pub pair_of: HashMap<String, (String, String)>, // net -> (pair name, partner)
}

const CELL: f32 = 150.0;

const PALETTE: [[u8; 3]; 16] = [
    [70, 167, 88],
    [245, 165, 36],
    [168, 110, 233],
    [18, 165, 148],
    [233, 61, 130],
    [142, 140, 153],
    [255, 180, 84],
    [95, 184, 240],
    [127, 209, 127],
    [214, 64, 159],
    [158, 123, 240],
    [46, 189, 229],
    [247, 107, 21],
    [194, 166, 121],
    [110, 159, 255],
    [255, 139, 139],
];

fn mil(l: trace_core::Length) -> f32 {
    l.mil() as f32
}

impl Scene {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let board = trace_altium::read_pcbdoc(&path).map_err(|e| e.to_string())?;
        Ok(Self::from_board(path, board))
    }

    pub fn from_board(path: PathBuf, board: Board) -> Self {
        let copper: Vec<String> = board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect();
        let n = copper.len() as u8;
        let slot = |l: &LayerRef| match l {
            LayerRef::Copper(i) => Slot::Copper(*i),
            LayerRef::AllCopper => Slot::Copper(0),
            LayerRef::TopSilk => Slot::TopSilk,
            LayerRef::BottomSilk => Slot::BottomSilk,
            _ => Slot::Other,
        };
        let g = &board.geometry;
        let mut prims =
            Vec::with_capacity(g.tracks.len() + g.arcs.len() * 4 + g.vias.len() + g.pads.len());
        for t in &g.tracks {
            prims.push(Prim {
                slot: slot(&t.layer),
                net: t.net,
                geom: Geom::Seg {
                    x1: mil(t.start.x),
                    y1: mil(t.start.y),
                    x2: mil(t.end.x),
                    y2: mil(t.end.y),
                    w: mil(t.width),
                },
                all_layers: false,
                component: None,
            });
        }
        for a in &g.arcs {
            let (cx, cy, r) = (a.center.x.mil(), a.center.y.mil(), a.radius.mil());
            let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
            if sweep == 0.0 {
                sweep = 360.0;
            }
            let steps = (sweep / 10.0).ceil().max(1.0) as usize;
            let p = |i: usize| {
                let t = (a.start_deg + sweep * i as f64 / steps as f64).to_radians();
                ((cx + r * t.cos()) as f32, (cy + r * t.sin()) as f32)
            };
            for i in 0..steps {
                let ((x1, y1), (x2, y2)) = (p(i), p(i + 1));
                prims.push(Prim {
                    slot: slot(&a.layer),
                    net: a.net,
                    geom: Geom::Seg {
                        x1,
                        y1,
                        x2,
                        y2,
                        w: mil(a.width),
                    },
                    all_layers: false,
                    component: None,
                });
            }
        }
        for p in &g.pads {
            let (x, y, sx, sy) = (mil(p.at.x), mil(p.at.y), mil(p.size_x), mil(p.size_y));
            let geom = if p.shape == PadShape::Round && (sx - sy).abs() < 1e-3 {
                Geom::Circle { x, y, r: sx / 2.0 }
            } else if p.shape == PadShape::Round {
                // Oval: a segment with round ends along the long axis.
                let (long, short) = (sx.max(sy), sx.min(sy));
                let ang = (p.rotation_deg as f32 + if sx >= sy { 0.0 } else { 90.0 }).to_radians();
                let h = (long - short) / 2.0;
                Geom::Seg {
                    x1: x - h * ang.cos(),
                    y1: y - h * ang.sin(),
                    x2: x + h * ang.cos(),
                    y2: y + h * ang.sin(),
                    w: short,
                }
            } else {
                Geom::Rect {
                    x,
                    y,
                    sx,
                    sy,
                    rot: p.rotation_deg as f32,
                }
            };
            prims.push(Prim {
                slot: slot(&p.layer),
                net: p.net,
                geom,
                all_layers: p.layer == LayerRef::AllCopper,
                component: p.component,
            });
        }
        for v in &g.vias {
            prims.push(Prim {
                slot: Slot::Vias,
                net: v.net,
                geom: Geom::Circle {
                    x: mil(v.at.x),
                    y: mil(v.at.y),
                    r: mil(v.diameter) / 2.0,
                },
                all_layers: true,
                component: None,
            });
        }

        let mut layers: Vec<Layer> = copper
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let color = if i == 0 {
                    [229, 72, 77]
                } else if i as u8 == n - 1 {
                    [62, 156, 240]
                } else {
                    PALETTE[(i - 1) % PALETTE.len()]
                };
                Layer {
                    slot: Slot::Copper(i as u8),
                    name: name.clone(),
                    color,
                    visible: true,
                    count: 0,
                }
            })
            .collect();
        layers.push(Layer {
            slot: Slot::Vias,
            name: "Vias".into(),
            color: [201, 180, 88],
            visible: true,
            count: 0,
        });
        layers.push(Layer {
            slot: Slot::TopSilk,
            name: "Top silk".into(),
            color: [230, 230, 230],
            visible: true,
            count: 0,
        });
        layers.push(Layer {
            slot: Slot::BottomSilk,
            name: "Bottom silk".into(),
            color: [180, 140, 255],
            visible: false,
            count: 0,
        });
        for p in &prims {
            if let Some(l) = layers.iter_mut().find(|l| l.slot == p.slot) {
                l.count += 1;
            }
        }

        let outline: Vec<[f32; 2]> = g.outline.iter().map(|p| [mil(p.x), mil(p.y)]).collect();
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let pts: Vec<[f32; 2]> = if outline.len() >= 3 {
            outline.clone()
        } else {
            prims
                .iter()
                .flat_map(|p| {
                    let b = p.geom.bbox();
                    [[b[0], b[1]], [b[2], b[3]]]
                })
                .collect()
        };
        for [x, y] in pts {
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }

        let mut by_net: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, p) in prims.iter().enumerate() {
            if let Some(n) = p.net {
                by_net.entry(n).or_default().push(i);
            }
            let b = p.geom.bbox();
            for gx in (b[0] / CELL).floor() as i32..=(b[2] / CELL).floor() as i32 {
                for gy in (b[1] / CELL).floor() as i32..=(b[3] / CELL).floor() as i32 {
                    grid.entry((gx, gy)).or_default().push(i);
                }
            }
        }
        let net_by_name = board
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i as u32))
            .collect();
        let mut pair_of = HashMap::new();
        for p in &board.diff_pairs {
            pair_of.insert(p.positive.clone(), (p.name.clone(), p.negative.clone()));
            pair_of.insert(p.negative.clone(), (p.name.clone(), p.positive.clone()));
        }

        Scene {
            path,
            board,
            prims,
            layers,
            copper_count: n,
            outline,
            bounds,
            by_net,
            grid,
            net_by_name,
            pair_of,
        }
    }

    pub fn layer_visible(&self, slot: Slot) -> bool {
        self.layers
            .iter()
            .find(|l| l.slot == slot)
            .is_none_or(|l| l.visible)
    }

    /// Whether a primitive shows with the current layer settings.
    pub fn shows(&self, p: &Prim) -> bool {
        if p.slot == Slot::Other {
            return false;
        }
        if p.all_layers && p.slot != Slot::Vias {
            return self
                .layers
                .iter()
                .any(|l| matches!(l.slot, Slot::Copper(_)) && l.visible);
        }
        self.layer_visible(p.slot)
    }

    /// The primitive under a board point, preferring the active layer.
    pub fn pick(&self, x: f32, y: f32, tolerance: f32, active: Slot) -> Option<usize> {
        let cell = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        let mut best: Option<(f32, usize)> = None;
        for &i in self.grid.get(&cell)? {
            let p = &self.prims[i];
            if !self.shows(p) {
                continue;
            }
            let d = p.geom.distance(x, y);
            if d > tolerance {
                continue;
            }
            let score = d
                - if p.slot == active { tolerance } else { 0.0 }
                - if p.component.is_some() {
                    tolerance * 0.5
                } else {
                    0.0
                };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// Bounding box of a net's copper.
    pub fn net_bounds(&self, net: u32) -> Option<[f32; 4]> {
        let list = self.by_net.get(&net)?;
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &i in list {
            let g = self.prims[i].geom.bbox();
            b = [
                b[0].min(g[0]),
                b[1].min(g[1]),
                b[2].max(g[2]),
                b[3].max(g[3]),
            ];
        }
        Some(b)
    }
}
