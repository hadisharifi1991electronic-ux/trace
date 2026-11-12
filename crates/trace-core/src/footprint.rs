//! Footprints: a part's pads and drawing in its own frame (origin at the
//! part's reference point, unturned, seen from the top), kept in libraries,
//! taken from placed parts and put back on boards.

use crate::edit::{Edit, Prims};
use crate::font::{text_tracks, Placing};
use crate::place::{place, Placement};
use crate::{Arc, Board, Component, LayerRef, Length, Pad, Point, Polygon, Side, Track};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Text a footprint carries. Special strings such as `.Designator` and
/// `.Comment` become the part's own text when it is placed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FpText {
    pub layer: LayerRef,
    /// Start of the baseline.
    pub at: Point,
    pub height: Length,
    /// Stroke width.
    pub width: Length,
    pub rotation_deg: f64,
    #[serde(default)]
    pub mirror: bool,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Footprint {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Body height above the board, when the library gives it.
    #[serde(default)]
    pub height: Option<Length>,
    /// Copper layers of the frame the pads are given in: copper layer 0 is
    /// the top and `copper - 1` the bottom.
    pub copper: u8,
    #[serde(default)]
    pub pads: Vec<Pad>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub arcs: Vec<Arc>,
    /// Filled shapes: copper regions and mask, paste, silk or mechanical
    /// areas.
    #[serde(default)]
    pub regions: Vec<Polygon>,
    #[serde(default)]
    pub texts: Vec<FpText>,
}

impl Default for Footprint {
    fn default() -> Self {
        Footprint {
            name: String::new(),
            description: String::new(),
            height: None,
            copper: 2,
            pads: Vec::new(),
            tracks: Vec::new(),
            arcs: Vec::new(),
            regions: Vec::new(),
            texts: Vec::new(),
        }
    }
}

/// A footprint library as Trace saves it (`.tracelib`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    /// Trace version that wrote it.
    #[serde(default)]
    pub version: String,
    pub footprints: Vec<Footprint>,
}

impl Library {
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let f = std::io::BufWriter::new(std::fs::File::create(path)?);
        serde_json::to_writer(f, self).map_err(std::io::Error::other)
    }

    pub fn load(path: &std::path::Path) -> std::io::Result<Library> {
        let f = std::io::BufReader::new(std::fs::File::open(path)?);
        serde_json::from_reader(f).map_err(std::io::Error::other)
    }
}

const ORIGIN: Placement = Placement {
    x: Length(0),
    y: Length(0),
    rotation_deg: 0.0,
    side: Side::Top,
};

fn copper_count(board: &Board) -> u8 {
    board.stackup.copper_layers().count().max(1) as u8
}

/// A board holding only part `index`'s own items (and the part list), for
/// moving them with `place`.
fn part_board(board: &Board, index: usize, prims: Prims) -> Board {
    let mut b = Board {
        stackup: board.stackup.clone(),
        components: board.components.clone(),
        ..Default::default()
    };
    let _ = index;
    let g = &mut b.geometry;
    g.pads = prims.pads;
    g.tracks = prims.tracks;
    g.arcs = prims.arcs;
    g.polygons = prims.polygons;
    g.areas = prims.areas;
    b
}

fn mil(p: Point) -> [f64; 2] {
    [p.x.mil(), p.y.mil()]
}

impl Footprint {
    /// Part `index` of `board` as a footprint: its pads, drawing and
    /// copper, brought back to its own frame. Text strokes on the board
    /// (its designator) become a `.Designator` text where they were.
    pub fn from_part(board: &Board, index: usize) -> Option<Footprint> {
        let c = board.components.get(index)?;
        let me = Some(index as u32);
        let g = &board.geometry;
        let prims = Prims {
            pads: g
                .pads
                .iter()
                .filter(|p| p.component == me)
                .cloned()
                .collect(),
            tracks: g
                .tracks
                .iter()
                .filter(|t| t.component == me)
                .cloned()
                .collect(),
            arcs: g
                .arcs
                .iter()
                .filter(|a| a.component == me)
                .cloned()
                .collect(),
            polygons: g
                .polygons
                .iter()
                .filter(|p| p.component == me && p.pour.is_none())
                .cloned()
                .collect(),
            areas: g
                .areas
                .iter()
                .filter(|p| p.component == me)
                .cloned()
                .collect(),
            components: Vec::new(),
        };
        let mut b = part_board(board, index, prims);
        let at = Placement::of(board, index)?;
        place(&mut b, index, at, ORIGIN);
        let g = b.geometry;
        let mut fp = Footprint {
            name: c.footprint.clone(),
            description: c.param(&["Description"]).unwrap_or_default().to_string(),
            copper: copper_count(board),
            ..Default::default()
        };
        let clean = |net: &mut Option<u32>, comp: &mut Option<u32>| {
            *net = None;
            *comp = None;
        };
        for mut p in g.pads {
            clean(&mut p.net, &mut p.component);
            fp.pads.push(p);
        }
        let (text, lines): (Vec<Track>, Vec<Track>) = g.tracks.into_iter().partition(|t| t.text);
        for mut t in lines {
            clean(&mut t.net, &mut t.component);
            fp.tracks.push(t);
        }
        for mut a in g.arcs {
            clean(&mut a.net, &mut a.component);
            fp.arcs.push(a);
        }
        for mut p in g.polygons.into_iter().chain(g.areas) {
            clean(&mut p.net, &mut p.component);
            fp.regions.push(p);
        }
        // The designator's strokes: one text where they were.
        let mut by_layer: HashMap<LayerRef, ([f64; 4], Length)> = HashMap::new();
        for t in &text {
            let e = by_layer
                .entry(t.layer.clone())
                .or_insert(([f64::MAX, f64::MAX, f64::MIN, f64::MIN], t.width));
            for q in [mil(t.start), mil(t.end)] {
                e.0 = [
                    e.0[0].min(q[0]),
                    e.0[1].min(q[1]),
                    e.0[2].max(q[0]),
                    e.0[3].max(q[1]),
                ];
            }
        }
        let mut layers: Vec<_> = by_layer.into_iter().collect();
        layers.sort_by_key(|(l, _)| format!("{l:?}"));
        for (layer, (b, width)) in layers.into_iter().take(1) {
            // Text is usually read along its long side.
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            let upright = h > w * 1.5;
            fp.texts.push(FpText {
                layer,
                at: if upright {
                    Point::mil(b[2], b[1])
                } else {
                    Point::mil(b[0], b[1])
                },
                height: Length::from_mil(if upright { w } else { h }.max(20.0)),
                width,
                rotation_deg: if upright { 90.0 } else { 0.0 },
                mirror: false,
                text: ".Designator".into(),
            });
        }
        Some(fp)
    }

    /// Extent in mil (min x, min y, max x, max y) of pads and drawing.
    pub fn bounds(&self) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut add = |q: [f64; 2], r: f64| {
            b = [
                b[0].min(q[0] - r),
                b[1].min(q[1] - r),
                b[2].max(q[0] + r),
                b[3].max(q[1] + r),
            ];
        };
        for p in &self.pads {
            add(mil(p.at), p.size_x.mil().max(p.size_y.mil()) / 2.0);
        }
        for t in &self.tracks {
            add(mil(t.start), t.width.mil() / 2.0);
            add(mil(t.end), t.width.mil() / 2.0);
        }
        for a in &self.arcs {
            add(mil(a.center), a.radius.mil() + a.width.mil() / 2.0);
        }
        for r in &self.regions {
            for &q in &r.outline {
                add(mil(q), 0.0);
            }
        }
        if b[0] > b[2] {
            [-50.0, -50.0, 50.0, 50.0]
        } else {
            b
        }
    }

    /// The footprint's copper layer `l` on a board with `n` copper layers:
    /// the bottom stays the bottom.
    fn layer_on(&self, l: u8, n: u8) -> u8 {
        if l > 0 && l + 1 >= self.copper {
            n - 1
        } else {
            l.min(n - 1)
        }
    }

    /// The text strokes of the footprint's texts for `part`, in the
    /// footprint frame.
    pub fn text_strokes(&self, part: Option<&Component>) -> Vec<Track> {
        let mut out = Vec::new();
        for t in &self.texts {
            let text = match t.text.strip_prefix('.') {
                Some(k) if k.eq_ignore_ascii_case("Designator") => match part {
                    Some(c) => c.designator.clone(),
                    None => "REF**".into(),
                },
                Some(k) => match part {
                    Some(c) => match c.param(&[
                        k,
                        if k.eq_ignore_ascii_case("Comment") {
                            "Value"
                        } else {
                            k
                        },
                    ]) {
                        Some(v) => v.to_string(),
                        None => continue,
                    },
                    None => t.text.clone(),
                },
                None => t.text.clone(),
            };
            let p = Placing {
                at: t.at,
                height: t.height,
                width: t.width,
                rotation_deg: t.rotation_deg,
                mirror: t.mirror,
                length: None,
            };
            out.extend(text_tracks(&text, &p, t.layer.clone(), None));
        }
        out
    }

    /// Everything the footprint puts on `board` for part `index`, where the
    /// part sits. Pads carry no net; texts are drawn when `texts` is set.
    pub fn instance(&self, board: &Board, index: usize, texts: bool) -> Prims {
        let n = copper_count(board);
        let me = Some(index as u32);
        let copper = |l: &LayerRef| match l {
            LayerRef::Copper(i) => LayerRef::Copper(self.layer_on(*i, n)),
            other => other.clone(),
        };
        let mut prims = Prims::default();
        for p in &self.pads {
            let mut p = p.clone();
            p.layer = copper(&p.layer);
            for l in &mut p.stack {
                l.layer = self.layer_on(l.layer, n);
            }
            p.stack.retain(|l| l.layer < n);
            p.component = me;
            p.net = None;
            prims.pads.push(p);
        }
        let strokes = if texts {
            self.text_strokes(board.components.get(index))
        } else {
            Vec::new()
        };
        for t in self.tracks.iter().chain(&strokes) {
            let mut t = t.clone();
            t.layer = copper(&t.layer);
            t.component = me;
            t.net = None;
            prims.tracks.push(t);
        }
        for a in &self.arcs {
            let mut a = a.clone();
            a.layer = copper(&a.layer);
            a.component = me;
            a.net = None;
            prims.arcs.push(a);
        }
        for r in &self.regions {
            let mut r = r.clone();
            r.layer = copper(&r.layer);
            r.component = me;
            r.net = None;
            r.pour = None;
            if matches!(r.layer, LayerRef::Copper(_)) {
                prims.polygons.push(r);
            } else {
                prims.areas.push(r);
            }
        }
        let Some(to) = Placement::of(board, index) else {
            return prims;
        };
        let mut b = part_board(board, index, prims);
        place(&mut b, index, ORIGIN, to);
        let g = b.geometry;
        Prims {
            pads: g.pads,
            tracks: g.tracks,
            arcs: g.arcs,
            polygons: g.polygons,
            areas: g.areas,
            components: Vec::new(),
        }
    }
}

/// The board's parts that use footprint `name`.
pub fn parts_using(board: &Board, name: &str) -> Vec<usize> {
    board
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.footprint.eq_ignore_ascii_case(name))
        .map(|(i, _)| i)
        .collect()
}

/// An edit that gives each of `parts` the footprint `fp` in place of what
/// it has: pads take the net their old pad of the same name had, and the
/// part keeps its designator text when it has one. `None` when nothing
/// would change.
pub fn replace(board: &Board, fp: &Footprint, parts: &[usize]) -> Option<Edit> {
    if parts.is_empty() {
        return None;
    }
    let before = Prims::of(board);
    let mut b = board.clone();
    let mut area = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut grow = |p: &Pad| {
        let q = mil(p.at);
        let r = p.size_x.mil().max(p.size_y.mil());
        area = [
            area[0].min(q[0] - r),
            area[1].min(q[1] - r),
            area[2].max(q[0] + r),
            area[3].max(q[1] + r),
        ];
    };
    for &i in parts {
        let me = Some(i as u32);
        let g = &mut b.geometry;
        let nets: HashMap<String, Option<u32>> = g
            .pads
            .iter()
            .filter(|p| p.component == me)
            .map(|p| (p.name.to_ascii_uppercase(), p.net))
            .collect();
        for p in g.pads.iter().filter(|p| p.component == me) {
            grow(p);
        }
        let has_text = g.tracks.iter().any(|t| t.component == me && t.text);
        g.pads.retain(|p| p.component != me);
        g.tracks.retain(|t| t.component != me || t.text);
        g.arcs.retain(|a| a.component != me);
        g.polygons.retain(|p| p.component != me || p.pour.is_some());
        g.areas.retain(|p| p.component != me);
        let mut new = fp.instance(&b, i, !has_text);
        for p in &mut new.pads {
            p.net = nets.get(&p.name.to_ascii_uppercase()).copied().flatten();
            grow(p);
        }
        let g = &mut b.geometry;
        g.pads.extend(new.pads);
        g.tracks.extend(new.tracks);
        g.arcs.extend(new.arcs);
        g.polygons.extend(new.polygons);
        g.areas.extend(new.areas);
        if let Some(c) = b.components.get_mut(i) {
            c.footprint = fp.name.clone();
        }
    }
    Some(Edit::Parts {
        before: Box::new(before),
        after: Box::new(Prims::of(&b)),
        area: (area[0] <= area[2]).then_some(area),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PadShape, StackLayer, Stackup};

    fn pad(name: &str, x: f64, y: f64, comp: u32, net: u32) -> Pad {
        Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(comp),
            name: name.into(),
            at: Point::mil(x, y),
            size_x: Length::from_mil(20.0),
            size_y: Length::from_mil(30.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Default::default(),
        }
    }

    fn board() -> Board {
        let mut b = Board {
            stackup: Stackup {
                layers: (0..4)
                    .map(|i| StackLayer {
                        name: format!("L{i}"),
                        altium_id: None,
                        kind: crate::LayerKind::Copper,
                        thickness: Length(0),
                        dielectric_constant: None,
                        material: None,
                    })
                    .collect(),
            },
            ..Default::default()
        };
        for (d, x, side, rot) in [
            ("R1", 1000.0, Side::Top, 90.0),
            ("R2", 2000.0, Side::Bottom, 0.0),
        ] {
            b.components.push(Component {
                designator: d.into(),
                footprint: "R0402".into(),
                library_ref: None,
                x: Length::from_mil(x),
                y: Length::from_mil(500.0),
                rotation_deg: rot,
                side,
                sheet: None,
                params: Default::default(),
            });
        }
        // R1 turned a quarter: its pads lie along y.
        b.geometry.pads.push(pad("1", 1000.0, 480.0, 0, 0));
        b.geometry.pads.push(pad("2", 1000.0, 520.0, 0, 1));
        // R2 on the bottom, unturned: mirrored top to bottom, so pad 1
        // stays on the left.
        let mut p = pad("1", 1980.0, 500.0, 1, 1);
        p.layer = LayerRef::Copper(3);
        b.geometry.pads.push(p);
        let mut p = pad("2", 2020.0, 500.0, 1, 0);
        p.layer = LayerRef::Copper(3);
        b.geometry.pads.push(p);
        b.geometry.tracks.push(Track {
            layer: LayerRef::TopSilk,
            net: None,
            start: Point::mil(990.0, 450.0),
            end: Point::mil(990.0, 550.0),
            width: Length::from_mil(5.0),
            component: Some(0),
            text: false,
        });
        b
    }

    #[test]
    fn takes_a_part_back_to_its_own_frame() {
        let b = board();
        let fp = Footprint::from_part(&b, 0).unwrap();
        assert_eq!(fp.pads.len(), 2);
        let p1 = fp.pads.iter().find(|p| p.name == "1").unwrap();
        // Turned back: pad 1 at (-20, 0).
        assert!(
            (p1.at.x.mil() + 20.0).abs() < 0.01 && p1.at.y.mil().abs() < 0.01,
            "{:?}",
            p1.at
        );
        assert_eq!(p1.net, None);
        assert_eq!(fp.tracks.len(), 1);
        // The bottom part comes back on top too.
        let fp2 = Footprint::from_part(&b, 1).unwrap();
        assert!(fp2.pads.iter().all(|p| p.layer == LayerRef::Copper(0)));
        let q1 = fp2.pads.iter().find(|p| p.name == "1").unwrap();
        assert!((q1.at.x.mil() + 20.0).abs() < 0.01, "{:?}", q1.at);
    }

    #[test]
    fn replacing_keeps_nets_by_pad_name() {
        let mut b = board();
        let mut fp = Footprint::from_part(&b, 0).unwrap();
        // Wider pads, further apart, and a third pad.
        for p in &mut fp.pads {
            p.at.x = Length(p.at.x.0 * 2);
            p.size_x = Length::from_mil(25.0);
        }
        let mut extra = fp.pads[0].clone();
        extra.name = "3".into();
        extra.at = Point::mil(0.0, 40.0);
        fp.pads.push(extra);
        let parts = parts_using(&b, "R0402");
        assert_eq!(parts, vec![0, 1]);
        let e = replace(&b, &fp, &parts).unwrap();
        e.apply(&mut b);
        assert_eq!(b.geometry.pads.len(), 6);
        let r1: Vec<&Pad> = b
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(0))
            .collect();
        let p1 = r1.iter().find(|p| p.name == "1").unwrap();
        assert_eq!(p1.net, Some(0));
        // Turned a quarter: (-40, 0) lands at (1000, 460).
        assert!((p1.at.x.mil() - 1000.0).abs() < 0.01 && (p1.at.y.mil() - 460.0).abs() < 0.01);
        assert!(r1.iter().any(|p| p.name == "3" && p.net.is_none()));
        // The bottom part's pads stay on the bottom.
        assert!(b
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(1))
            .all(|p| p.layer == LayerRef::Copper(3)));
        // Undo puts it all back.
        e.inverse().apply(&mut b);
        assert_eq!(b.geometry.pads.len(), 4);
        assert_eq!(b.geometry.pads[0].at, Point::mil(1000.0, 480.0));
    }
}
