use serde::{Deserialize, Serialize};

/// One schematic sheet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    pub symbols: Vec<Symbol>,
    pub net_labels: Vec<String>,
    pub power_ports: Vec<String>,
    pub ports: Vec<String>,
    /// Child sheets referenced by sheet symbols.
    pub sheet_refs: Vec<String>,
    pub wires: usize,
    pub junctions: usize,
    /// What the sheet looks like, for the schematic viewer.
    #[serde(default, skip_serializing_if = "Drawing::is_empty")]
    pub drawing: Drawing,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Symbol {
    pub designator: String,
    pub library_ref: String,
    pub description: String,
    pub value: Option<String>,
    pub pins: usize,
}

/// A point on a sheet in mil, y up.
pub type Pt = [f64; 2];

/// An RGB colour.
pub type Rgb = [u8; 3];

/// The graphics of one sheet, already placed: every coordinate is in sheet
/// mil with y up, and parts are drawn in their placed position and rotation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Drawing {
    /// Sheet size in mil.
    pub size: [f64; 2],
    pub paper: Rgb,
    pub shapes: Vec<SchShape>,
    pub texts: Vec<SchText>,
    pub pins: Vec<SchPin>,
    /// Wires as polylines; each is one object for connectivity.
    pub wires: Vec<Vec<Pt>>,
    pub junctions: Vec<Pt>,
    /// Net labels, power ports, ports and sheet entries: things that name a net.
    pub tags: Vec<NetTag>,
    /// Placed parts; `symbol` indexes `Sheet::symbols`.
    pub parts: Vec<SchPart>,
    /// Schematic nets found from the wiring, filled by `Drawing::connect`.
    pub nets: Vec<SchNet>,
    /// Pictures (logos, block diagrams) stored in the sheet.
    #[serde(default)]
    pub images: Vec<SchImage>,
    /// Sheet symbols: the box and the file of the sheet they stand for.
    #[serde(default)]
    pub links: Vec<SheetLink>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SheetLink {
    pub min: Pt,
    pub max: Pt,
    pub file: String,
}

/// A picture placed on the sheet: the file's own bytes (BMP, PNG or JPEG).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchImage {
    pub min: Pt,
    pub max: Pt,
    pub keep_aspect: bool,
    #[serde(skip)]
    pub data: std::sync::Arc<Vec<u8>>,
}

impl Drawing {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.texts.is_empty() && self.wires.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SchGeom {
    /// An open polyline.
    Line(Vec<Pt>),
    /// A closed polygon.
    Poly(Vec<Pt>),
    /// An axis-aligned rectangle with optional corner radii.
    Rect { min: Pt, max: Pt, r: [f64; 2] },
    /// An elliptical arc, angles in degrees counter-clockwise from +x. A full
    /// ellipse has start 0 and end 360.
    Arc {
        c: Pt,
        r: [f64; 2],
        start: f64,
        end: f64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchShape {
    pub geom: SchGeom,
    /// Stroke width in mil; 0 draws the thinnest visible line.
    pub width: f64,
    pub stroke: Option<Rgb>,
    pub fill: Option<Rgb>,
    pub dashed: bool,
    pub part: Option<u32>,
    /// Which wire or bus this is, for highlighting a net.
    pub wire: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchText {
    pub at: Pt,
    pub text: String,
    /// Font height (em) in mil.
    pub size: f64,
    pub color: Rgb,
    /// Quarter turns counter-clockwise.
    pub rot: u8,
    /// 0..8: bottom/centre/top rows of left/centre/right, as Altium numbers them
    /// (0 bottom-left, 4 centre, 8 top-right).
    pub just: u8,
    pub bold: bool,
    pub italic: bool,
    /// Wrap inside this width (text frames), in mil.
    pub wrap: Option<f64>,
    pub part: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchPin {
    /// The connection end.
    pub at: Pt,
    /// The end on the part body.
    pub base: Pt,
    pub part: u32,
    pub name: String,
    /// Pin number, matching the footprint's pad name.
    pub number: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagKind {
    Label,
    Power,
    Port,
    SheetEntry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetTag {
    /// Where it connects; a port connects at either end.
    pub at: Vec<Pt>,
    pub text: String,
    pub kind: TagKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchPart {
    pub designator: String,
    pub symbol: usize,
    pub min: Pt,
    pub max: Pt,
}

/// A net within one sheet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchNet {
    /// Name from a power port, net label or port, if it has one.
    pub name: Option<String>,
    pub wires: Vec<u32>,
    pub pins: Vec<u32>,
    pub tags: Vec<u32>,
}

fn on_segment(p: Pt, a: Pt, b: Pt, tol: f64) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy <= tol * tol
}

fn same(a: Pt, b: Pt, tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

impl Drawing {
    /// Works out the sheet's nets: wires join where an end or a junction
    /// lies on another wire, pins and tags join the wire they touch, and
    /// tags with the same text join each other.
    pub fn connect(&mut self) {
        const TOL: f64 = 0.6;
        let (nw, np, nt) = (self.wires.len(), self.pins.len(), self.tags.len());
        let mut d = Dsu((0..nw + np + nt).collect());
        let pin = |i: usize| nw + i;
        let tag = |i: usize| nw + np + i;
        let on_wire = |p: Pt, w: &[Pt]| w.windows(2).any(|s| on_segment(p, s[0], s[1], TOL));
        // Points that join whatever wire they touch.
        let mut points: Vec<(Pt, usize)> = Vec::new();
        for (i, w) in self.wires.iter().enumerate() {
            if let (Some(&a), Some(&b)) = (w.first(), w.last()) {
                points.push((a, i));
                points.push((b, i));
            }
        }
        for (i, p) in self.pins.iter().enumerate() {
            points.push((p.at, pin(i)));
        }
        for (i, t) in self.tags.iter().enumerate() {
            for &p in &t.at {
                points.push((p, tag(i)));
            }
        }
        let junction_wires: Vec<Vec<usize>> = self
            .junctions
            .iter()
            .map(|&j| {
                (0..nw)
                    .filter(|&w| on_wire(j, &self.wires[w]))
                    .collect::<Vec<_>>()
            })
            .collect();
        for ws in junction_wires {
            for w in ws.windows(2) {
                d.join(w[0], w[1]);
            }
        }
        // Wire bounding boxes keep the search cheap.
        let boxes: Vec<[f64; 4]> = self
            .wires
            .iter()
            .map(|w| {
                w.iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                        [
                            b[0].min(p[0]),
                            b[1].min(p[1]),
                            b[2].max(p[0]),
                            b[3].max(p[1]),
                        ]
                    })
            })
            .collect();
        for &(p, owner) in &points {
            for (w, b) in boxes.iter().enumerate() {
                if p[0] < b[0] - TOL || p[0] > b[2] + TOL || p[1] < b[1] - TOL || p[1] > b[3] + TOL
                {
                    continue;
                }
                if owner != w && on_wire(p, &self.wires[w]) {
                    d.join(owner, w);
                }
            }
        }
        // Pins and tags that touch directly.
        let mut sorted: Vec<(Pt, usize)> = points.iter().filter(|p| p.1 >= nw).copied().collect();
        sorted.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
        for i in 0..sorted.len() {
            for j in i + 1..sorted.len() {
                if sorted[j].0[0] - sorted[i].0[0] > TOL {
                    break;
                }
                if same(sorted[i].0, sorted[j].0, TOL) {
                    d.join(sorted[i].1, sorted[j].1);
                }
            }
        }
        // Same text, same net (labels and power ports; ports and entries too
        // within one sheet).
        let mut by_name: std::collections::HashMap<&str, usize> = Default::default();
        for (i, t) in self.tags.iter().enumerate() {
            if t.text.is_empty() {
                continue;
            }
            match by_name.get(t.text.as_str()) {
                Some(&o) => d.join(tag(i), o),
                None => {
                    by_name.insert(&t.text, tag(i));
                }
            }
        }
        let mut index: std::collections::HashMap<usize, usize> = Default::default();
        let mut nets: Vec<SchNet> = Vec::new();
        let mut net_of = |d: &mut Dsu, i: usize, nets: &mut Vec<SchNet>| {
            let r = d.find(i);
            *index.entry(r).or_insert_with(|| {
                nets.push(SchNet::default());
                nets.len() - 1
            })
        };
        for w in 0..nw {
            let n = net_of(&mut d, w, &mut nets);
            nets[n].wires.push(w as u32);
        }
        for p in 0..np {
            let n = net_of(&mut d, pin(p), &mut nets);
            nets[n].pins.push(p as u32);
        }
        for t in 0..nt {
            let n = net_of(&mut d, tag(t), &mut nets);
            nets[n].tags.push(t as u32);
        }
        for n in &mut nets {
            let rank = |k: TagKind| match k {
                TagKind::Power => 0,
                TagKind::Label => 1,
                TagKind::Port => 2,
                TagKind::SheetEntry => 3,
            };
            n.name = n
                .tags
                .iter()
                .map(|&t| &self.tags[t as usize])
                .filter(|t| !t.text.is_empty())
                .min_by_key(|t| rank(t.kind))
                .map(|t| t.text.clone());
        }
        self.nets = nets;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_tees_and_labels() {
        let mut d = Drawing {
            wires: vec![
                vec![[0.0, 0.0], [100.0, 0.0]],
                vec![[50.0, 0.0], [50.0, 50.0]],  // tee onto wire 0
                vec![[0.0, 20.0], [100.0, 20.0]], // crosses wire 1, no junction
                vec![[200.0, 0.0], [300.0, 0.0]],
            ],
            pins: vec![SchPin {
                at: [300.0, 0.0],
                base: [310.0, 0.0],
                part: 0,
                name: String::new(),
                number: "1".into(),
            }],
            tags: vec![
                NetTag {
                    at: vec![[10.0, 0.0]],
                    text: "A".into(),
                    kind: TagKind::Label,
                },
                NetTag {
                    at: vec![[250.0, 0.0]],
                    text: "A".into(),
                    kind: TagKind::Label,
                },
            ],
            ..Default::default()
        };
        d.connect();
        let net = |w: u32| d.nets.iter().position(|n| n.wires.contains(&w)).unwrap();
        assert_eq!(net(0), net(1));
        assert_ne!(net(0), net(2));
        assert_eq!(net(0), net(3));
        assert_eq!(d.nets[net(3)].pins, vec![0]);
        assert_eq!(d.nets[net(0)].name.as_deref(), Some("A"));
    }
}
