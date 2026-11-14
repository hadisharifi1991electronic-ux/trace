//! IPC-2581 (revision C) export: one XML file with the stack-up, the board
//! profile, every layer's copper, mask, paste and silkscreen with nets,
//! drill layers, the parts and their packages, the net list and the bill
//! of materials, as Altium's and Allegro's IPC-2581 writers produce for
//! fabrication and assembly.

use crate::fab::{
    mechanical_layers, plated, plot_copper, plot_mask, plot_other, plot_paste, Ap, FabOptions,
    Owner, Plot,
};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use trace_core::place::Placement;
use trace_core::{Board, LayerKind, LayerRef, PadShape, Point, Side};

/// A layer written: name, function, side, features, and the span of a
/// drill layer.
type LayerOut = (
    String,
    &'static str,
    &'static str,
    String,
    Option<(String, String)>,
);

/// One BOM reference: designator, populated ("true" or "false") and layer.
type BomRef = (String, &'static str, String);

fn mm(v: i64) -> String {
    let s = format!("{:.4}", v as f64 / 1e6);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

fn mmf(v: f64) -> String {
    mm((v * 1e6).round() as i64)
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

/// Shapes and line widths used, each written once in the dictionaries.
#[derive(Default)]
struct Dict {
    standard: Vec<(String, String)>,
    user: Vec<(String, String)>,
    lines: Vec<(String, String)>,
    index: HashMap<String, String>,
}

impl Dict {
    fn standard(&mut self, key: String, xml: String) -> String {
        if let Some(id) = self.index.get(&key) {
            return id.clone();
        }
        let id = format!("SP{}", self.standard.len());
        self.standard.push((id.clone(), xml));
        self.index.insert(key, id.clone());
        id
    }
    fn user(&mut self, key: String, xml: String) -> String {
        if let Some(id) = self.index.get(&key) {
            return id.clone();
        }
        let id = format!("US{}", self.user.len());
        self.user.push((id.clone(), xml));
        self.index.insert(key, id.clone());
        id
    }
    fn line(&mut self, width: i64) -> String {
        let key = format!("L{width}");
        if let Some(id) = self.index.get(&key) {
            return id.clone();
        }
        let id = format!("LD{}", self.lines.len());
        self.lines.push((
            id.clone(),
            format!("<LineDesc lineEnd=\"ROUND\" lineWidth=\"{}\"/>", mm(width)),
        ));
        self.index.insert(key, id.clone());
        id
    }

    /// The primitive for an aperture: (kind, id).
    fn aperture(&mut self, ap: &Ap) -> (bool, String) {
        match ap {
            Ap::Circle(d) => (
                true,
                self.standard(
                    format!("C{d}"),
                    format!("<Circle diameter=\"{}\"/>", mm(*d)),
                ),
            ),
            Ap::Rect(w, h) => (
                true,
                self.standard(
                    format!("R{w}x{h}"),
                    format!("<RectCenter width=\"{}\" height=\"{}\"/>", mm(*w), mm(*h)),
                ),
            ),
            Ap::Obround(w, h) => (
                true,
                self.standard(
                    format!("O{w}x{h}"),
                    format!("<Oval width=\"{}\" height=\"{}\"/>", mm(*w), mm(*h)),
                ),
            ),
            Ap::Outline(pts, _) => {
                let key = format!("U{pts:?}");
                let xml = format!(
                    "<UserSpecial><Contour>{}</Contour></UserSpecial>",
                    polygon(pts.iter().map(|&(x, y)| (x, y)))
                );
                (false, self.user(key, xml))
            }
            Ap::Bar(a, b, d) => {
                // A round-ended bar as an outline: two half circles.
                let key = format!("B{a:?}{b:?}{d}");
                let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
                let len = dx.hypot(dy).max(1.0);
                let (nx, ny) = (-dy / len * *d as f64 / 2.0, dx / len * *d as f64 / 2.0);
                let p = |x: f64, y: f64| (x.round() as i64, y.round() as i64);
                let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
                let xml = format!(
                    "<UserSpecial><Contour><Polygon><PolyBegin x=\"{}\" y=\"{}\"/><PolyStepSegment x=\"{}\" y=\"{}\"/><PolyStepCurve x=\"{}\" y=\"{}\" centerX=\"{}\" centerY=\"{}\" clockwise=\"false\"/><PolyStepSegment x=\"{}\" y=\"{}\"/><PolyStepCurve x=\"{}\" y=\"{}\" centerX=\"{}\" centerY=\"{}\" clockwise=\"false\"/></Polygon></Contour></UserSpecial>",
                    mm(p(ax + nx, ay + ny).0),
                    mm(p(ax + nx, ay + ny).1),
                    mm(p(bx + nx, by + ny).0),
                    mm(p(bx + nx, by + ny).1),
                    mm(p(bx - nx, by - ny).0),
                    mm(p(bx - nx, by - ny).1),
                    mm(b.0),
                    mm(b.1),
                    mm(p(ax - nx, ay - ny).0),
                    mm(p(ax - nx, ay - ny).1),
                    mm(p(ax + nx, ay + ny).0),
                    mm(p(ax + nx, ay + ny).1),
                    mm(a.0),
                    mm(a.1),
                );
                (false, self.user(key, xml))
            }
        }
    }
}

/// A closed `<Polygon>` through points (nm).
fn polygon(pts: impl Iterator<Item = (i64, i64)>) -> String {
    let v: Vec<(i64, i64)> = pts.collect();
    let mut s = String::from("<Polygon>");
    if let Some(&(x, y)) = v.first() {
        let _ = write!(s, "<PolyBegin x=\"{}\" y=\"{}\"/>", mm(x), mm(y));
        for &(x, y) in v.iter().skip(1).chain(v.first()) {
            let _ = write!(s, "<PolyStepSegment x=\"{}\" y=\"{}\"/>", mm(x), mm(y));
        }
    }
    s.push_str("</Polygon>");
    s
}

fn ring(pts: &[Point]) -> String {
    polygon(pts.iter().map(|q| (q.x.0, q.y.0)))
}

/// One layer's features, as `<Set>` elements.
struct Layer<'a> {
    board: &'a Board,
    dict: &'a mut Dict,
    out: String,
    net: String,
    owner: Option<Owner>,
    count: usize,
}

impl Layer<'_> {
    fn set(&mut self, body: &str) {
        if self.net.is_empty() {
            let _ = write!(self.out, "<Set>{body}</Set>");
        } else {
            let _ = write!(self.out, "<Set net=\"{}\">{body}</Set>", esc(&self.net));
        }
        self.count += 1;
    }
}

impl Plot for Layer<'_> {
    fn attrs(&mut self, net: &str, _pin: &str) {
        self.net = net.to_string();
    }
    fn owner(&mut self, o: Owner) {
        self.owner = Some(o);
    }
    fn flash(&mut self, ap: Ap, _function: &'static str, at: Point) {
        let (standard, id) = self.dict.aperture(&ap);
        let prim = if standard {
            format!("<StandardPrimitiveRef id=\"{id}\"/>")
        } else {
            format!("<UserPrimitiveRef id=\"{id}\"/>")
        };
        let pin = match self.owner.take() {
            Some(Owner::Pad(i)) => {
                let p = &self.board.geometry.pads[i];
                p.component
                    .and_then(|c| self.board.components.get(c as usize))
                    .filter(|_| !p.name.is_empty())
                    .map(|c| {
                        format!(
                            "<PinRef componentRef=\"{}\" pin=\"{}\"/>",
                            esc(&c.designator),
                            esc(&p.name)
                        )
                    })
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        let body = format!(
            "<Pad><Location x=\"{}\" y=\"{}\"/>{prim}{pin}</Pad>",
            mm(at.x.0),
            mm(at.y.0)
        );
        self.set(&body);
    }
    fn line(&mut self, d: i64, _function: &'static str, a: Point, b: Point) {
        let id = self.dict.line(d);
        let body = format!(
            "<Features><Location x=\"0\" y=\"0\"/><Line startX=\"{}\" startY=\"{}\" endX=\"{}\" endY=\"{}\"><LineDescRef id=\"{id}\"/></Line></Features>",
            mm(a.x.0),
            mm(a.y.0),
            mm(b.x.0),
            mm(b.y.0)
        );
        self.set(&body);
    }
    fn arc(&mut self, d: i64, _function: &'static str, a: &trace_core::Arc) {
        let id = self.dict.line(d);
        let (cx, cy, r) = (a.center.x.mm(), a.center.y.mm(), a.radius.mm());
        let at = |deg: f64| {
            let t = deg.to_radians();
            (mmf(cx + r * t.cos()), mmf(cy + r * t.sin()))
        };
        let (s, e) = (at(a.start_deg), at(a.end_deg));
        let body = format!(
            "<Features><Location x=\"0\" y=\"0\"/><Arc startX=\"{}\" startY=\"{}\" endX=\"{}\" endY=\"{}\" centerX=\"{}\" centerY=\"{}\" clockwise=\"false\"><LineDescRef id=\"{id}\"/></Arc></Features>",
            s.0,
            s.1,
            e.0,
            e.1,
            mmf(cx),
            mmf(cy)
        );
        self.set(&body);
    }
    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        if outline.len() < 3 {
            return;
        }
        let mut body = String::from("<Features><Location x=\"0\" y=\"0\"/><Contour>");
        body.push_str(&ring(outline));
        for h in holes.iter().filter(|h| h.len() >= 3) {
            let _ = write!(body, "<Cutout>{}</Cutout>", &ring(h)[9..ring(h).len() - 10]);
        }
        body.push_str("</Contour></Features>");
        self.set(&body);
    }
}

fn name_ok(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() {
        "LAYER".into()
    } else {
        t
    }
}

/// The board as an IPC-2581 revision C document, with how many features
/// it holds.
pub fn to_ipc2581(board: &Board, name: &str) -> (String, usize) {
    let n = board.stackup.copper_layers().count().max(1) as u8;
    let opt = FabOptions::from_board(board);
    let step = name_ok(name);
    let mut dict = Dict::default();
    let copper_names: Vec<String> = (0..n)
        .map(|l| {
            board
                .stackup
                .copper_layers()
                .nth(l as usize)
                .map(|s| name_ok(&s.name))
                .unwrap_or_else(|| format!("L{}", l + 1))
        })
        .collect();
    // (name, function, side, features)
    let mut layers: Vec<LayerOut> = Vec::new();
    let mut total = 0;
    let mut run = |dict: &mut Dict,
                   lname: String,
                   func: &'static str,
                   side: &'static str,
                   f: &dyn Fn(&mut Layer)| {
        let mut l = Layer {
            board,
            dict,
            out: String::new(),
            net: String::new(),
            owner: None,
            count: 0,
        };
        f(&mut l);
        total += l.count;
        layers.push((lname, func, side, l.out, None));
    };
    for l in 0..n {
        let side = if l == 0 {
            "TOP"
        } else if l == n - 1 {
            "BOTTOM"
        } else {
            "INTERNAL"
        };
        run(
            &mut dict,
            copper_names[l as usize].clone(),
            "SIGNAL",
            side,
            &|g| plot_copper(g, board, l, n),
        );
    }
    for (s, t, side) in [
        (Side::Top, "TOP", "TOP"),
        (Side::Bottom, "BOTTOM", "BOTTOM"),
    ] {
        let silk = if s == Side::Top {
            LayerRef::TopSilk
        } else {
            LayerRef::BottomSilk
        };
        run(
            &mut dict,
            format!("{t}_SOLDERMASK"),
            "SOLDERMASK",
            side,
            &|g| plot_mask(g, board, s, n, &opt),
        );
        run(&mut dict, format!("{t}_PASTE"), "SOLDERPASTE", side, &|g| {
            plot_paste(g, board, s, n, &opt)
        });
        run(
            &mut dict,
            format!("{t}_SILKSCREEN"),
            "SILKSCREEN",
            side,
            &|g| plot_other(g, board, &silk),
        );
    }
    for m in mechanical_layers(board) {
        run(
            &mut dict,
            format!("MECHANICAL_{m}"),
            "DOCUMENT",
            "NONE",
            &|g| plot_other(g, board, &LayerRef::Mechanical(m)),
        );
    }
    // Drill layers, one per span and plating.
    let mut drills: BTreeMap<(u8, u8, bool), String> = BTreeMap::new();
    let mut hole_no = 0usize;
    let net = |n: Option<u32>| {
        n.and_then(|n| board.nets.get(n as usize))
            .map(|n| format!(" net=\"{}\"", esc(&n.name)))
            .unwrap_or_default()
    };
    for p in board.geometry.pads.iter().filter(|p| p.hole.0 > 0) {
        hole_no += 1;
        let pl = plated(p);
        let _ = write!(
            drills.entry((0, n - 1, pl)).or_default(),
            "<Set{}><Hole name=\"H{hole_no}\" diameter=\"{}\" platingStatus=\"{}\" plusTol=\"0\" minusTol=\"0\" x=\"{}\" y=\"{}\"/></Set>",
            net(p.net),
            mm(p.hole.0),
            if pl { "PLATED" } else { "NONPLATED" },
            mm(p.at.x.0),
            mm(p.at.y.0)
        );
    }
    for v in board.geometry.vias.iter().filter(|v| v.hole.0 > 0) {
        hole_no += 1;
        let (a, b) = (v.from_layer.min(n - 1), v.to_layer.min(n - 1));
        let _ = write!(
            drills.entry((a, b, !v.fab.unplated)).or_default(),
            "<Set{}><Hole name=\"H{hole_no}\" diameter=\"{}\" platingStatus=\"VIA\" plusTol=\"0\" minusTol=\"0\" x=\"{}\" y=\"{}\"/></Set>",
            net(v.net),
            mm(v.hole.0),
            mm(v.at.x.0),
            mm(v.at.y.0)
        );
    }
    total += hole_no;
    for ((a, b, pl), body) in drills {
        let lname = format!("DRILL_{}_{}{}", a + 1, b + 1, if pl { "" } else { "_NPTH" });
        layers.push((
            lname,
            "DRILL",
            "ALL",
            body,
            Some((
                copper_names[a as usize].clone(),
                copper_names[b as usize].clone(),
            )),
        ));
    }

    // Packages: each footprint once, pins in the part's own frame.
    let mut packages: BTreeMap<String, String> = BTreeMap::new();
    let mut comps = String::new();
    let mut bom: BTreeMap<(String, String), Vec<BomRef>> = BTreeMap::new();
    let vparts = trace_core::variant::parts(board);
    for (ci, c) in vparts.iter().enumerate() {
        let pkg = name_ok(if c.footprint.is_empty() {
            &c.designator
        } else {
            &c.footprint
        });
        let place = Placement::of(board, ci);
        let pads: Vec<&trace_core::Pad> = board
            .geometry
            .pads
            .iter()
            .filter(|p| p.component == Some(ci as u32))
            .collect();
        let smt = pads.iter().all(|p| p.hole.0 == 0);
        if !packages.contains_key(&pkg) {
            let mut x = format!(
                "<Package name=\"{}\" type=\"OTHER\" pinOne=\"1\" pinOneOrientation=\"OTHER\">",
                esc(&pkg)
            );
            let mut seen = std::collections::HashSet::new();
            for p in &pads {
                if p.name.is_empty() || !seen.insert(p.name.clone()) {
                    continue;
                }
                let Some(pl) = &place else { continue };
                let q = pl.to_local([p.at.x.mil(), p.at.y.mil()]);
                let (w, h) = (p.size_x.0, p.size_y.0);
                let id = match p.shape {
                    PadShape::Round if w == h => {
                        dict.standard(format!("C{w}"), format!("<Circle diameter=\"{}\"/>", mm(w)))
                    }
                    PadShape::Round => dict.standard(
                        format!("O{w}x{h}"),
                        format!("<Oval width=\"{}\" height=\"{}\"/>", mm(w), mm(h)),
                    ),
                    _ => dict.standard(
                        format!("R{w}x{h}"),
                        format!("<RectCenter width=\"{}\" height=\"{}\"/>", mm(w), mm(h)),
                    ),
                };
                let rot = (p.rotation_deg - c.rotation_deg).rem_euclid(360.0);
                let xf = if rot.abs() > 1e-6 && (rot - 360.0).abs() > 1e-6 {
                    format!("<Xform rotation=\"{rot:.3}\"/>")
                } else {
                    String::new()
                };
                let _ = write!(
                    x,
                    "<Pin number=\"{}\" type=\"{}\" electricalType=\"ELECTRICAL\">{xf}<Location x=\"{}\" y=\"{}\"/><StandardPrimitiveRef id=\"{id}\"/></Pin>",
                    esc(&p.name),
                    if p.hole.0 > 0 { "THRU" } else { "SURFACE" },
                    mmf(q[0] * 0.0254),
                    mmf(q[1] * 0.0254),
                );
            }
            x.push_str("</Package>");
            packages.insert(pkg.clone(), x);
        }
        let layer = if c.side == Side::Bottom {
            copper_names[(n - 1) as usize].clone()
        } else {
            copper_names[0].clone()
        };
        let part = c
            .param(&["Value", "Comment", "Part Number", "PartNumber"])
            .map(str::to_string)
            .or_else(|| c.library_ref.clone())
            .unwrap_or_else(|| pkg.clone());
        let mirror = if c.side == Side::Bottom {
            " mirror=\"true\""
        } else {
            ""
        };
        let _ = write!(
            comps,
            "<Component refDes=\"{}\" packageRef=\"{}\" layerRef=\"{}\" part=\"{}\" mountType=\"{}\"><Xform rotation=\"{:.3}\"{mirror}/><Location x=\"{}\" y=\"{}\"/></Component>",
            esc(&c.designator),
            esc(&pkg),
            esc(&layer),
            esc(&part),
            if smt { "SMT" } else { "THMT" },
            c.rotation_deg.rem_euclid(360.0),
            mm(c.x.0),
            mm(c.y.0)
        );
        bom.entry((part, pkg.clone())).or_default().push((
            c.designator.clone(),
            if trace_core::variant::fitted(board, ci) {
                "true"
            } else {
                "false"
            },
            layer,
        ));
    }

    // Nets.
    let mut nets: BTreeMap<u32, Vec<(String, String)>> = BTreeMap::new();
    for p in &board.geometry.pads {
        if let (Some(n), Some(c)) = (p.net, p.component) {
            if let Some(c) = board.components.get(c as usize) {
                if !p.name.is_empty() {
                    nets.entry(n)
                        .or_default()
                        .push((c.designator.clone(), p.name.clone()));
                }
            }
        }
    }

    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<IPC-2581 revision=\"C\" xmlns=\"http://webstds.ipc.org/2581\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n");
    // Content.
    s.push_str("<Content roleRef=\"Owner\">\n<FunctionMode mode=\"FABRICATION\" level=\"3\"/>\n");
    let _ = writeln!(s, "<StepRef name=\"{}\"/>", esc(&step));
    for (lname, ..) in &layers {
        let _ = writeln!(s, "<LayerRef name=\"{}\"/>", esc(lname));
    }
    let dielectrics = board
        .stackup
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Dielectric)
        .count();
    for k in 1..=dielectrics {
        let _ = writeln!(s, "<LayerRef name=\"DIELECTRIC_{k}\"/>");
    }
    let _ = writeln!(s, "<BomRef name=\"{}_BOM\"/>", esc(&step));
    s.push_str("<DictionaryStandard units=\"MILLIMETER\">\n");
    for (id, x) in &dict.standard {
        let _ = writeln!(s, "<EntryStandard id=\"{id}\">{x}</EntryStandard>");
    }
    s.push_str("</DictionaryStandard>\n<DictionaryUser units=\"MILLIMETER\">\n");
    for (id, x) in &dict.user {
        let _ = writeln!(s, "<EntryUser id=\"{id}\">{x}</EntryUser>");
    }
    s.push_str("</DictionaryUser>\n<DictionaryLineDesc units=\"MILLIMETER\">\n");
    for (id, x) in &dict.lines {
        let _ = writeln!(s, "<EntryLineDesc id=\"{id}\">{x}</EntryLineDesc>");
    }
    s.push_str("</DictionaryLineDesc>\n</Content>\n");
    // Logistics and history.
    let date = crate::assembly::today();
    s.push_str("<LogisticHeader><Role id=\"Owner\" roleFunction=\"SENDER\"/><Enterprise id=\"Trace\" code=\"NONE\"/><Person name=\"Trace\" enterpriseRef=\"Trace\" roleRef=\"Owner\"/></LogisticHeader>\n");
    let _ = writeln!(
        s,
        "<HistoryRecord number=\"1\" origination=\"{date}T00:00:00\" software=\"Trace\" lastChange=\"{date}T00:00:00\"><FileRevision fileRevisionId=\"1\" comment=\"\"><SoftwarePackage name=\"Trace\" revision=\"{}\" vendor=\"Trace\"/></FileRevision></HistoryRecord>",
        env!("CARGO_PKG_VERSION")
    );
    // BOM.
    let _ = writeln!(
        s,
        "<Bom name=\"{0}_BOM\" revision=\"1.0\"><BomHeader assembly=\"{0}\" revision=\"1.0\"><StepRef name=\"{0}\"/></BomHeader>",
        esc(&step)
    );
    for ((part, pkg), refs) in &bom {
        let _ = write!(
            s,
            "<BomItem OEMDesignNumberRef=\"{}\" quantity=\"{}\" category=\"ELECTRICAL\">",
            esc(part),
            refs.iter().filter(|r| r.1 == "true").count()
        );
        for (d, populate, layer) in refs {
            let _ = write!(
                s,
                "<RefDes name=\"{}\" packageRef=\"{}\" populate=\"{populate}\" layerRef=\"{}\"/>",
                esc(d),
                esc(pkg),
                esc(layer)
            );
        }
        s.push_str("<Characteristics category=\"ELECTRICAL\"/></BomItem>\n");
    }
    s.push_str("</Bom>\n");
    // ECAD.
    let _ = writeln!(s, "<Ecad name=\"{}\">", esc(&step));
    s.push_str("<CadHeader units=\"MILLIMETER\"/>\n<CadData>\n");
    for (lname, func, side, _, span) in &layers {
        match span {
            Some((a, b)) => {
                let _ = writeln!(
                    s,
                    "<Layer name=\"{}\" layerFunction=\"{func}\" side=\"{side}\" polarity=\"POSITIVE\"><Span fromLayer=\"{}\" toLayer=\"{}\"/></Layer>",
                    esc(lname),
                    esc(a),
                    esc(b)
                );
            }
            None => {
                let _ = writeln!(
                    s,
                    "<Layer name=\"{}\" layerFunction=\"{func}\" side=\"{side}\" polarity=\"POSITIVE\"/>",
                    esc(lname)
                );
            }
        }
    }
    // Dielectric layers and the stack-up.
    let mut stack = String::new();
    let mut seq = 0;
    let mut overall = 0i64;
    let mut ci = 0usize;
    let mut di = 0usize;
    let mut extra_layers = String::new();
    for l in &board.stackup.layers {
        let lname = match l.kind {
            LayerKind::Copper => {
                ci += 1;
                copper_names.get(ci - 1).cloned().unwrap_or_default()
            }
            LayerKind::Dielectric => {
                di += 1;
                let nm = format!("DIELECTRIC_{di}");
                let _ = writeln!(
                    extra_layers,
                    "<Layer name=\"{nm}\" layerFunction=\"DIELPREG\" side=\"INTERNAL\" polarity=\"POSITIVE\"/>"
                );
                nm
            }
            _ => continue,
        };
        seq += 1;
        overall += l.thickness.0;
        let mut spec = String::new();
        if let Some(er) = l.dielectric_constant {
            let _ = write!(spec, " dielectricConstant=\"{er}\"");
        }
        if let Some(df) = l.loss_tangent {
            let _ = write!(spec, " lossTangent=\"{df}\"");
        }
        let _ = write!(
            stack,
            "<StackupLayer layerOrGroupRef=\"{}\" thickness=\"{}\" tolPlus=\"0\" tolMinus=\"0\" sequence=\"{seq}\"/>",
            esc(&lname),
            mm(l.thickness.0)
        );
        let _ = spec;
    }
    s.push_str(&extra_layers);
    if seq > 0 {
        let _ = writeln!(
            s,
            "<Stackup name=\"STACKUP\" overallThickness=\"{0}\" tolPlus=\"0\" tolMinus=\"0\" whereMeasured=\"METAL\"><StackupGroup name=\"GROUP\" thickness=\"{0}\" tolPlus=\"0\" tolMinus=\"0\">{stack}</StackupGroup></Stackup>",
            mm(overall)
        );
    }
    let _ = writeln!(s, "<Step name=\"{}\">", esc(&step));
    s.push_str("<Datum x=\"0\" y=\"0\"/>\n");
    let g = &board.geometry;
    if g.outline.len() >= 3 {
        s.push_str("<Profile>");
        s.push_str(&ring(&g.outline));
        for slot in g.slots.iter().filter(|o| o.len() >= 3) {
            let r = ring(slot);
            let _ = write!(s, "<Cutout>{}</Cutout>", &r[9..r.len() - 10]);
        }
        s.push_str("</Profile>\n");
    }
    for x in packages.values() {
        s.push_str(x);
        s.push('\n');
    }
    s.push_str(&comps);
    s.push('\n');
    for (n, pins) in &nets {
        let Some(net) = board.nets.get(*n as usize) else {
            continue;
        };
        let _ = write!(s, "<LogicalNet name=\"{}\">", esc(&net.name));
        for (d, p) in pins {
            let _ = write!(
                s,
                "<PinRef componentRef=\"{}\" pin=\"{}\"/>",
                esc(d),
                esc(p)
            );
        }
        s.push_str("</LogicalNet>\n");
    }
    for (lname, _, _, body, _) in &layers {
        if body.is_empty() {
            continue;
        }
        let _ = writeln!(
            s,
            "<LayerFeature layerRef=\"{}\">{body}</LayerFeature>",
            esc(lname)
        );
    }
    s.push_str("</Step>\n</CadData>\n</Ecad>\n</IPC-2581>\n");
    (s, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_core::*;

    #[test]
    fn writes_a_well_formed_document() {
        let mut b = Board::default();
        for (n, k) in [
            ("TOP", LayerKind::Copper),
            ("CORE", LayerKind::Dielectric),
            ("BOT", LayerKind::Copper),
        ] {
            b.stackup.layers.push(StackLayer {
                name: n.into(),
                altium_id: None,
                kind: k,
                thickness: Length::from_mm(0.5),
                dielectric_constant: Some(4.2),
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        b.nets.push(Net {
            name: "A&B".into(),
            pieces: 1,
            ..Default::default()
        });
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "0603".into(),
            library_ref: None,
            x: Length::from_mm(5.0),
            y: Length::from_mm(5.0),
            rotation_deg: 90.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        for (k, x) in [(1, 4.2), (2, 5.8)] {
            b.geometry.pads.push(Pad {
                layer: LayerRef::Copper(0),
                net: Some(0),
                component: Some(0),
                name: k.to_string(),
                at: Point {
                    x: Length::from_mm(5.0),
                    y: Length::from_mm(x),
                },
                size_x: Length::from_mm(0.8),
                size_y: Length::from_mm(0.9),
                hole: Length(0),
                shape: PadShape::Rect,
                rotation_deg: 90.0,
                outline: Vec::new(),
                stack: Vec::new(),
                fab: Default::default(),
            });
        }
        b.geometry.outline = vec![
            Point::mil(0.0, 0.0),
            Point::mil(1000.0, 0.0),
            Point::mil(1000.0, 1000.0),
        ];
        let (x, n) = to_ipc2581(&b, "demo");
        assert!(n >= 2);
        assert!(x.contains("<LogicalNet name=\"A&amp;B\"><PinRef componentRef=\"R1\" pin=\"1\"/>"));
        assert!(x.contains("<Package name=\"0603\""));
        assert!(x.contains("<StackupLayer layerOrGroupRef=\"DIELECTRIC_1\" thickness=\"0.5\""));
        assert!(x.contains("<Pad><Location x=\"5\" y=\"4.2\"/>"));
        // Every element opened is closed.
        let opens = x.matches('<').count();
        assert_eq!(opens, x.matches('>').count());
    }
}
