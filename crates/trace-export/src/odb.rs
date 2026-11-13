//! ODB++ output: one job (matrix, a `pcb` step with its profile, every
//! copper, mask, paste, silkscreen, drill and mechanical layer, both
//! component layers and the EDA data that ties features to nets and pins),
//! packed as the `.tgz` fabricators take.
//!
//! The layers are drawn by the same code that writes the Gerber files, so
//! the two outputs always agree. Units are millimetres; pad shapes that are
//! not a standard symbol become user symbols under `symbols/`.

use crate::fab::{self, Ap, FabFile, FabOptions, Owner, Plot};
use std::collections::HashMap;
use std::fmt::Write;
use trace_core::{Board, LayerKind, LayerRef, Length, Point, Side};

const STEP: &str = "pcb";

/// A coordinate or size in nm as millimetres, without trailing zeros.
fn mm(v: i64) -> String {
    trim(format!("{:.6}", v as f64 / 1e6))
}

/// A size in nm as microns, for symbol names.
fn um(v: i64) -> String {
    trim(format!("{:.3}", v as f64 / 1e3))
}

fn trim(s: String) -> String {
    let t = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if t == "-0" || t.is_empty() {
        "0".into()
    } else {
        t.into()
    }
}

fn deg(v: f64) -> String {
    let v = v.rem_euclid(360.0);
    let s = trim(format!("{v:.4}"));
    if s == "360" {
        "0".into()
    } else {
        s
    }
}

/// User symbols, shared by every layer of the job.
#[derive(Default)]
struct UserSymbols {
    names: HashMap<Ap, String>,
    files: Vec<(String, String)>,
}

impl UserSymbols {
    /// The symbol for an outline (relative to the flash point) grown by
    /// `grow` with round corners.
    fn outline(&mut self, ap: &Ap) -> String {
        if let Some(n) = self.names.get(ap) {
            return n.clone();
        }
        let name = format!("trace_pad_{}", self.files.len() + 1);
        let mut f = Feats::new();
        if let Ap::Outline(pts, grow) = ap {
            let ring: Vec<Point> = pts.iter().map(|p| pt(p.0, p.1)).collect();
            f.surface(&ring, &[]);
            if *grow > 0 {
                for (k, a) in ring.iter().enumerate() {
                    let b = ring[(k + 1) % ring.len()];
                    f.line(2 * grow, "", *a, b);
                }
            }
        }
        let text = f.finish();
        self.names.insert(ap.clone(), name.clone());
        self.files.push((name.clone(), text));
        name
    }
}

fn pt(x: i64, y: i64) -> Point {
    Point {
        x: Length(x),
        y: Length(y),
    }
}

/// One features file being written.
struct Feats<'a> {
    symbols: Vec<String>,
    index: HashMap<String, usize>,
    body: String,
    count: usize,
    owner: Option<Owner>,
    /// (feature index, owner) of each copper or drill feature.
    owners: Vec<(usize, Owner)>,
    /// Attribute of the features that follow (`;…`), if any.
    attr: &'static str,
    user: Option<&'a mut UserSymbols>,
}

/// Attribute names written at the head of every features file.
const ATTRS: [&str; 2] = [".smd", ".drill"];

impl<'a> Feats<'a> {
    fn new() -> Feats<'a> {
        Feats {
            symbols: Vec::new(),
            index: HashMap::new(),
            body: String::new(),
            count: 0,
            owner: None,
            owners: Vec::new(),
            attr: "",
            user: None,
        }
    }

    fn with(user: &'a mut UserSymbols) -> Feats<'a> {
        Feats {
            user: Some(user),
            ..Feats::new()
        }
    }

    fn sym(&mut self, name: String) -> usize {
        if let Some(&i) = self.index.get(&name) {
            return i;
        }
        self.symbols.push(name.clone());
        self.index.insert(name, self.symbols.len() - 1);
        self.symbols.len() - 1
    }

    fn round(&mut self, d: i64) -> usize {
        self.sym(format!("r{}", um(d.max(1))))
    }

    /// Ends a feature record: its attribute and owner.
    fn done(&mut self) {
        if !self.attr.is_empty() {
            self.body.push(';');
            self.body.push_str(self.attr);
        }
        self.body.push('\n');
        if let Some(o) = self.owner {
            self.owners.push((self.count, o));
        }
        self.count += 1;
    }

    fn pad(&mut self, sym: usize, at: Point, cw_deg: f64) {
        let _ = write!(self.body, "P {} {} {sym} P 0", mm(at.x.0), mm(at.y.0));
        let a = deg(cw_deg);
        if a == "0" {
            self.body.push_str(" 0");
        } else {
            let _ = write!(self.body, " 8 {a}");
        }
        self.done();
    }

    /// One closed surface: the outline as an island and its holes.
    fn surface(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        if outline.len() < 3 {
            return;
        }
        self.body.push_str("S P 0");
        self.done();
        // Islands clockwise, holes counter-clockwise.
        let contour = |body: &mut String, ring: &[Point], island: bool| {
            let mut v: Vec<Point> = ring.to_vec();
            if v.len() > 1 && v.first() == v.last() {
                v.pop();
            }
            if v.len() < 3 {
                return;
            }
            let area: f64 = (0..v.len())
                .map(|i| {
                    let (a, b) = (v[i], v[(i + 1) % v.len()]);
                    a.x.0 as f64 * b.y.0 as f64 - b.x.0 as f64 * a.y.0 as f64
                })
                .sum();
            if (area < 0.0) != island {
                v.reverse();
            }
            let _ = writeln!(
                body,
                "OB {} {} {}",
                mm(v[0].x.0),
                mm(v[0].y.0),
                if island { "I" } else { "H" }
            );
            for p in v.iter().skip(1).chain(v.first()) {
                let _ = writeln!(body, "OS {} {}", mm(p.x.0), mm(p.y.0));
            }
            body.push_str("OE\n");
        };
        contour(&mut self.body, outline, true);
        for h in holes {
            contour(&mut self.body, h, false);
        }
        self.body.push_str("SE\n");
    }

    fn finish(self) -> String {
        let mut o = String::from("#\n#Units\n#\nUNITS=MM\n#\n#Feature symbol names\n#\n");
        for (i, s) in self.symbols.iter().enumerate() {
            let _ = writeln!(o, "${i} {s}");
        }
        o.push_str("#\n#Feature attribute names\n#\n");
        for (i, a) in ATTRS.iter().enumerate() {
            let _ = writeln!(o, "@{i} {a}");
        }
        o.push_str("#\n#Layer features\n#\n");
        o.push_str(&self.body);
        o
    }
}

impl Plot for Feats<'_> {
    fn attrs(&mut self, _net: &str, _pin: &str) {}

    fn owner(&mut self, o: Owner) {
        self.owner = Some(o);
    }

    fn flash(&mut self, ap: Ap, function: &'static str, at: Point) {
        self.attr = if function.starts_with("SMDPad") {
            "0"
        } else {
            ""
        };
        match &ap {
            Ap::Circle(d) => {
                let s = self.round(*d);
                self.pad(s, at, 0.0);
            }
            Ap::Rect(w, h) => {
                let s = self.sym(format!("rect{}x{}", um(*w), um(*h)));
                self.pad(s, at, 0.0);
            }
            Ap::Obround(w, h) if w == h => {
                let s = self.round(*w);
                self.pad(s, at, 0.0);
            }
            Ap::Obround(w, h) => {
                let s = self.sym(format!("oval{}x{}", um(*w), um(*h)));
                self.pad(s, at, 0.0);
            }
            Ap::Bar(a, _, w) => {
                // A round-ended bar through the flash point: an oval turned
                // to its direction (ODB++ turns clockwise).
                let half = (a.0 as f64).hypot(a.1 as f64);
                let len = (2.0 * half).round() as i64 + w;
                let ccw = (a.1 as f64).atan2(a.0 as f64).to_degrees();
                let s = self.sym(format!("oval{}x{}", um(len), um(*w)));
                self.pad(s, at, -ccw);
            }
            Ap::Outline(..) => {
                let name = match self.user.as_deref_mut() {
                    Some(u) => u.outline(&ap),
                    None => return,
                };
                let s = self.sym(name);
                self.pad(s, at, 0.0);
            }
        }
        self.attr = "";
    }

    fn line(&mut self, d: i64, _function: &'static str, a: Point, b: Point) {
        let s = self.round(d);
        let _ = write!(
            self.body,
            "L {} {} {} {} {s} P 0",
            mm(a.x.0),
            mm(a.y.0),
            mm(b.x.0),
            mm(b.y.0)
        );
        self.done();
    }

    fn arc(&mut self, d: i64, _function: &'static str, a: &trace_core::Arc) {
        let sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
        let full = sweep < 1e-9;
        if full && (a.end_deg - a.start_deg).abs() < 1.0 {
            return;
        }
        let r = a.radius.0 as f64;
        let at = |deg: f64| {
            let (s, co) = deg.to_radians().sin_cos();
            (
                a.center.x.0 + (r * co).round() as i64,
                a.center.y.0 + (r * s).round() as i64,
            )
        };
        let s = at(a.start_deg);
        let e = if full { s } else { at(a.end_deg) };
        let sym = self.round(d);
        let _ = write!(
            self.body,
            "A {} {} {} {} {} {} {sym} P 0 N",
            mm(s.0),
            mm(s.1),
            mm(e.0),
            mm(e.1),
            mm(a.center.x.0),
            mm(a.center.y.0)
        );
        self.done();
    }

    fn polygon(&mut self, outline: &[Point], holes: &[Vec<Point>]) {
        self.surface(outline, holes);
    }
}

/// A layer of the matrix.
struct MatrixLayer {
    name: String,
    context: &'static str,
    kind: &'static str,
    span: Option<(String, String)>,
}

/// Lower-case ODB++ entity name: letters, digits and `_+-.` only.
fn entity(s: &str) -> String {
    let n: String = s
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || "_+-.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if n.is_empty() {
        "layer".into()
    } else {
        n
    }
}

/// A name without spaces, for component, package and pin fields.
fn word(s: &str) -> String {
    let w: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == ';' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if w.is_empty() {
        "-".into()
    } else {
        w
    }
}

/// Every file of the ODB++ job, under `<job>/`.
pub fn odb_files(board: &Board, job: &str, opt: &FabOptions) -> Vec<FabFile> {
    let job = entity(job);
    let n = fab::copper_count(board).max(1) as u8;
    let mut user = UserSymbols::default();
    let mut out: Vec<FabFile> = Vec::new();
    let mut matrix: Vec<MatrixLayer> = Vec::new();
    // (layer name, owners) of the layers whose features carry nets.
    let mut netted: Vec<(String, Vec<(usize, Owner)>)> = Vec::new();
    let mut taken: Vec<String> = [
        "comp_+_top",
        "comp_+_bot",
        "sst",
        "ssb",
        "spt",
        "spb",
        "smt",
        "smb",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let unique = |want: String, taken: &mut Vec<String>| {
        let mut name = want.clone();
        let mut k = 2;
        while taken.contains(&name) {
            name = format!("{want}_{k}");
            k += 1;
        }
        taken.push(name.clone());
        name
    };
    let layer_file = |name: &str, text: String| FabFile {
        name: format!("{job}/steps/{STEP}/layers/{name}/features"),
        data: text.into_bytes(),
    };

    let side_layers = |board: &Board, side: Side, user: &mut UserSymbols| {
        let top = side == Side::Top;
        let mut silk = Feats::with(user);
        fab::plot_other(
            &mut silk,
            board,
            &if top {
                LayerRef::TopSilk
            } else {
                LayerRef::BottomSilk
            },
        );
        let silk = silk.finish();
        let mut paste = Feats::with(user);
        fab::plot_paste(&mut paste, board, side, n, opt);
        let paste = paste.finish();
        let mut mask = Feats::with(user);
        fab::plot_mask(&mut mask, board, side, n, opt);
        let mask = mask.finish();
        let s = if top { 't' } else { 'b' };
        [
            (format!("ss{s}"), "SILK_SCREEN", silk),
            (format!("sp{s}"), "SOLDER_PASTE", paste),
            (format!("sm{s}"), "SOLDER_MASK", mask),
        ]
    };

    // Top side, top to bottom.
    matrix.push(MatrixLayer {
        name: "comp_+_top".into(),
        context: "BOARD",
        kind: "COMPONENT",
        span: None,
    });
    for (name, kind, text) in side_layers(board, Side::Top, &mut user) {
        out.push(layer_file(&name, text));
        matrix.push(MatrixLayer {
            name,
            context: "BOARD",
            kind,
            span: None,
        });
    }
    // Copper and dielectric layers in stack-up order, each with its
    // thickness (and permittivity and material) in its attribute list.
    let mut cu_names = Vec::new();
    let mut stack: Vec<Option<&trace_core::StackLayer>> = Vec::new();
    for s in &board.stackup.layers {
        match s.kind {
            LayerKind::Copper if stack.iter().filter(|s| is_cu(s)).count() < n as usize => {
                stack.push(Some(s))
            }
            LayerKind::Dielectric => stack.push(Some(s)),
            _ => {}
        }
    }
    while stack.iter().filter(|s| is_cu(s)).count() < n as usize {
        stack.push(None);
    }
    let mut dielectrics = 0;
    for s in stack {
        if !is_cu(&s) {
            let Some(s) = s else { continue };
            dielectrics += 1;
            let want = match entity(&s.name) {
                w if w == "layer" => format!("dielectric_{dielectrics}"),
                w => w,
            };
            let name = unique(want, &mut taken);
            out.push(layer_file(&name, Feats::new().finish()));
            out.push(attr_file(&job, &name, s, false));
            matrix.push(MatrixLayer {
                name,
                context: "BOARD",
                kind: "DIELECTRIC",
                span: None,
            });
            continue;
        }
        let l = cu_names.len() as u8;
        let want = s
            .map(|s| entity(&s.name))
            .unwrap_or_else(|| format!("l{}", l + 1));
        let name = unique(want, &mut taken);
        let mut f = Feats::with(&mut user);
        fab::plot_copper(&mut f, board, l, n);
        netted.push((name.clone(), std::mem::take(&mut f.owners)));
        out.push(layer_file(&name, f.finish()));
        if let Some(s) = s {
            out.push(attr_file(&job, &name, s, true));
        }
        matrix.push(MatrixLayer {
            name: name.clone(),
            context: "BOARD",
            kind: "SIGNAL",
            span: None,
        });
        cu_names.push(name);
    }
    let mut bottom = side_layers(board, Side::Bottom, &mut user);
    bottom.reverse();
    for (name, kind, text) in bottom {
        out.push(layer_file(&name, text));
        matrix.push(MatrixLayer {
            name,
            context: "BOARD",
            kind,
            span: None,
        });
    }
    matrix.push(MatrixLayer {
        name: "comp_+_bot".into(),
        context: "BOARD",
        kind: "COMPONENT",
        span: None,
    });

    for ((a, b, plated), f) in drill_layers(board, n) {
        let want = if !plated {
            "drill_npth".to_string()
        } else if a == 0 && b == n - 1 {
            "drill".to_string()
        } else {
            format!("drill_{}-{}", a + 1, b + 1)
        };
        let name = unique(want, &mut taken);
        netted.push((name.clone(), f.owners.clone()));
        out.push(layer_file(&name, f.finish()));
        matrix.push(MatrixLayer {
            name,
            context: "BOARD",
            kind: "DRILL",
            span: Some((cu_names[a as usize].clone(), cu_names[b as usize].clone())),
        });
    }
    // Backdrills: a non-plated drill layer for each side and layer kept,
    // spanning the copper layers drilled through.
    let mut backs: std::collections::BTreeMap<(bool, u8), Feats<'static>> = Default::default();
    for v in &board.geometry.vias {
        for (top, b) in [(true, v.fab.back_top), (false, v.fab.back_bottom)] {
            let Some(b) = b else { continue };
            let f = backs.entry((top, b.keep)).or_insert_with(Feats::new);
            f.attr = "1=1";
            let s = f.round(b.diameter.0);
            f.pad(s, v.at, 0.0);
        }
    }
    for ((top, keep), f) in backs {
        let (a, b) = if top {
            (0, keep.saturating_sub(1))
        } else {
            (keep + 1, n - 1)
        };
        if (top && keep == 0) || a > b || b >= n {
            continue;
        }
        let side = if top { "top" } else { "bot" };
        let name = unique(format!("backdrill_{side}_{}-{}", a + 1, b + 1), &mut taken);
        out.push(layer_file(&name, f.finish()));
        matrix.push(MatrixLayer {
            name,
            context: "BOARD",
            kind: "DRILL",
            span: Some((cu_names[a as usize].clone(), cu_names[b as usize].clone())),
        });
    }
    for m in fab::mechanical_layers(board) {
        let name = unique(format!("mech{m}"), &mut taken);
        let mut f = Feats::with(&mut user);
        fab::plot_other(&mut f, board, &LayerRef::Mechanical(m));
        out.push(layer_file(&name, f.finish()));
        matrix.push(MatrixLayer {
            name,
            context: "MISC",
            kind: "DOCUMENT",
            span: None,
        });
    }

    // The profile: the board outline as one surface.
    let mut profile = Feats::new();
    profile.surface(&board.geometry.outline, &[]);
    out.push(FabFile {
        name: format!("{job}/steps/{STEP}/profile"),
        data: profile.finish().into_bytes(),
    });

    let (eda, comps) = eda_data(board, &netted);
    out.push(FabFile {
        name: format!("{job}/steps/{STEP}/eda/data"),
        data: eda.into_bytes(),
    });
    for (layer, text) in comps {
        out.push(FabFile {
            name: format!("{job}/steps/{STEP}/layers/{layer}/components"),
            data: text.into_bytes(),
        });
    }
    for (name, text) in std::mem::take(&mut user.files) {
        out.push(FabFile {
            name: format!("{job}/symbols/{name}/features"),
            data: text.into_bytes(),
        });
    }

    let mut m = String::new();
    let _ = writeln!(
        m,
        "STEP {{\n   COL=1\n   NAME={}\n}}\n",
        STEP.to_uppercase()
    );
    for (row, l) in matrix.iter().enumerate() {
        let (start, end) = l.span.clone().unwrap_or_default();
        let _ = writeln!(
            m,
            "LAYER {{\n   ROW={}\n   CONTEXT={}\n   TYPE={}\n   ID={}\n   NAME={}\n   POLARITY=POSITIVE\n   START_NAME={start}\n   END_NAME={end}\n   OLD_NAME=\n   COLOR=0\n}}\n",
            row + 1,
            l.context,
            l.kind,
            row + 1,
            l.name.to_uppercase()
        );
    }
    out.push(FabFile {
        name: format!("{job}/matrix/matrix"),
        data: m.into_bytes(),
    });
    let now = stamp();
    let version = env!("CARGO_PKG_VERSION");
    out.push(FabFile {
        name: format!("{job}/misc/info"),
        data: format!(
            "JOB_NAME={job}\nODB_VERSION_MAJOR=8\nODB_VERSION_MINOR=1\nODB_SOURCE=Trace {version}\nCREATION_DATE={now}\nSAVE_DATE={now}\nSAVE_APP=Trace {version}\nSAVE_USER=\nUNITS=MM\nMAX_UID=0\n"
        )
        .into_bytes(),
    });
    out.push(FabFile {
        name: format!("{job}/steps/{STEP}/stephdr"),
        data: b"UNITS=MM\nX_DATUM=0\nY_DATUM=0\nX_ORIGIN=0\nY_ORIGIN=0\nTOP_ACTIVE=0\nBOTTOM_ACTIVE=0\nRIGHT_ACTIVE=0\nLEFT_ACTIVE=0\nAFFECTING_BOM=\nAFFECTING_BOM_CHANGED=0\n".to_vec(),
    });
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn is_cu(s: &Option<&trace_core::StackLayer>) -> bool {
    s.is_none_or(|s| s.kind == LayerKind::Copper)
}

/// A stack-up layer's attribute list: its name in the design, thickness
/// (copper as weight, 1 oz = 35 um), permittivity and material.
fn attr_file(job: &str, layer: &str, s: &trace_core::StackLayer, copper: bool) -> FabFile {
    let mut t = String::from("UNITS=MM\n");
    let _ = writeln!(t, ".eda_layers = \"{}\"", s.name.replace('"', "'"));
    if copper {
        if s.thickness.0 > 0 {
            let _ = writeln!(t, ".copper_weight = {:.6}", s.thickness.0 as f64 / 35_000.0);
        }
    } else {
        if s.thickness.0 > 0 {
            let _ = writeln!(t, ".layer_dielectric = {}", mm(s.thickness.0));
        }
        if let Some(e) = s.dielectric_constant {
            let _ = writeln!(t, ".dielectric_constant = {e}");
        }
        if let Some(m) = s.material.as_deref().filter(|m| !m.is_empty()) {
            let _ = writeln!(t, ".comment = {m}");
        }
    }
    FabFile {
        name: format!("{job}/steps/{STEP}/layers/{layer}/attrlist"),
        data: t.into_bytes(),
    }
}

/// The drill layers: plated through holes, non-plated holes and one per
/// blind or buried span, keyed (first copper, last copper, plated).
fn drill_layers(board: &Board, n: u8) -> Vec<((u8, u8, bool), Feats<'static>)> {
    let mut sets: Vec<((u8, u8, bool), Feats<'static>)> = Vec::new();
    let get = |key: (u8, u8, bool), sets: &mut Vec<((u8, u8, bool), Feats<'static>)>| match sets
        .iter()
        .position(|s| s.0 == key)
    {
        Some(i) => i,
        None => {
            sets.push((key, Feats::new()));
            sets.len() - 1
        }
    };
    for (i, p) in board.geometry.pads.iter().enumerate() {
        if p.hole.0 <= 0 {
            continue;
        }
        let pl = fab::plated(p);
        let k = get((0, n - 1, pl), &mut sets);
        let f = &mut sets[k].1;
        f.owner = Some(Owner::Pad(i));
        f.attr = if pl { "1=0" } else { "1=1" };
        if p.fab.slot.0 > p.hole.0 {
            let half = (p.fab.slot.0 - p.hole.0) as f64 / 2.0;
            let (s, c) = p.fab.slot_deg.to_radians().sin_cos();
            let d = ((half * c).round() as i64, (half * s).round() as i64);
            let a = pt(p.at.x.0 - d.0, p.at.y.0 - d.1);
            let b = pt(p.at.x.0 + d.0, p.at.y.0 + d.1);
            f.line(p.hole.0, "", a, b);
        } else {
            let s = f.round(p.hole.0);
            f.pad(s, p.at, 0.0);
        }
    }
    for (i, v) in board.geometry.vias.iter().enumerate() {
        if v.hole.0 <= 0 {
            continue;
        }
        let (a, b) = (v.from_layer.min(n - 1), v.to_layer.min(n - 1));
        let pl = !v.fab.unplated;
        let k = get((a.min(b), a.max(b), pl), &mut sets);
        let f = &mut sets[k].1;
        f.owner = Some(Owner::Via(i));
        f.attr = if pl { "1=2" } else { "1=1" };
        let s = f.round(v.hole.0);
        f.pad(s, v.at, 0.0);
    }
    sets.sort_by_key(|s| (!s.0 .2, s.0 .0, s.0 .1));
    sets
}

/// The EDA data (nets, their subnets and features, packages) and each
/// side's components file, as (layer, text).
fn eda_data(
    board: &Board,
    netted: &[(String, Vec<(usize, Owner)>)],
) -> (String, Vec<(String, String)>) {
    let geo = &board.geometry;
    // Features of each owner, as (layer index, feature index).
    let mut fids: HashMap<Owner, Vec<(usize, usize)>> = HashMap::new();
    for (li, (_, owners)) in netted.iter().enumerate() {
        for &(fi, o) in owners {
            fids.entry(o).or_default().push((li, fi));
        }
    }
    // Pads of each part, in board order; their toeprint is their place.
    let mut pins: Vec<Vec<usize>> = vec![Vec::new(); board.components.len()];
    for (i, p) in geo.pads.iter().enumerate() {
        if let Some(c) = p.component.filter(|&c| (c as usize) < pins.len()) {
            if matches!(p.layer, LayerRef::Copper(_) | LayerRef::AllCopper) {
                pins[c as usize].push(i);
            }
        }
    }
    // Each part's number within its side's components file.
    let mut side_index = vec![0usize; board.components.len()];
    let mut counts = [0usize; 2];
    for (i, c) in board.components.iter().enumerate() {
        let s = (c.side == Side::Bottom) as usize;
        side_index[i] = counts[s];
        counts[s] += 1;
    }
    let pad_part: HashMap<usize, (usize, usize)> = pins
        .iter()
        .enumerate()
        .flat_map(|(c, ps)| ps.iter().enumerate().map(move |(t, &p)| (p, (c, t))))
        .collect();

    let none = board.nets.len();
    let net_of = |n: Option<u32>| n.map(|n| n as usize).filter(|&n| n < none).unwrap_or(none);
    // Subnets of each net, in order.
    enum Sub {
        Pin(usize),
        Via(usize),
        Trace,
        Plane,
    }
    let mut subs: Vec<Vec<Sub>> = (0..=none).map(|_| Vec::new()).collect();
    // Subnet number of each part pad, for the components files.
    let mut pad_sub: HashMap<usize, (usize, usize)> = HashMap::new();
    for (i, p) in geo.pads.iter().enumerate() {
        if pad_part.contains_key(&i) {
            let n = net_of(p.net);
            pad_sub.insert(i, (n, subs[n].len()));
            subs[n].push(Sub::Pin(i));
        }
    }
    for (i, v) in geo.vias.iter().enumerate() {
        subs[net_of(v.net)].push(Sub::Via(i));
    }
    // Tracks and free pads go in one trace subnet per net.
    let mut traces: Vec<Vec<(usize, usize)>> = vec![Vec::new(); none + 1];
    let mut planes: Vec<Vec<(usize, usize)>> = vec![Vec::new(); none + 1];
    for (o, list) in &fids {
        match *o {
            Owner::Trace(n) => traces[net_of(n)].extend(list),
            Owner::Pad(i) if !pad_part.contains_key(&i) => {
                traces[net_of(geo.pads[i].net)].extend(list)
            }
            Owner::Plane(n) => planes[net_of(n)].extend(list),
            _ => {}
        }
    }
    for n in 0..=none {
        traces[n].sort_unstable();
        planes[n].sort_unstable();
        if !traces[n].is_empty() {
            subs[n].push(Sub::Trace);
        }
        if !planes[n].is_empty() {
            subs[n].push(Sub::Plane);
        }
    }

    let mut e = String::new();
    let _ = writeln!(e, "HDR Trace {}", env!("CARGO_PKG_VERSION"));
    e.push_str("UNITS=MM\n");
    e.push_str("LYR");
    for (name, _) in netted {
        let _ = write!(e, " {name}");
    }
    e.push_str("\n#\n");
    let fid = |e: &mut String, list: Option<&Vec<(usize, usize)>>| {
        let mut l: Vec<(usize, usize)> = list.cloned().unwrap_or_default();
        l.sort_unstable();
        for (li, fi) in l {
            let _ = writeln!(e, "FID C {li} {fi}");
        }
    };
    for (n, list) in subs.iter().enumerate() {
        let name = if n == none {
            "$NONE$".to_string()
        } else {
            word(&board.nets[n].name)
        };
        let _ = writeln!(e, "# NET {n}\nNET {name}");
        for s in list {
            match s {
                Sub::Pin(i) => {
                    let (c, t) = pad_part[i];
                    let bottom = board.components[c].side == Side::Bottom;
                    let _ = writeln!(
                        e,
                        "SNT TOP {} {} {t}",
                        if bottom { 'B' } else { 'T' },
                        side_index[c]
                    );
                    fid(&mut e, fids.get(&Owner::Pad(*i)));
                }
                Sub::Via(i) => {
                    e.push_str("SNT VIA\n");
                    fid(&mut e, fids.get(&Owner::Via(*i)));
                }
                Sub::Trace => {
                    e.push_str("SNT TRC\n");
                    fid(&mut e, Some(&traces[n]));
                }
                Sub::Plane => {
                    e.push_str("SNT PLN S C 0\n");
                    fid(&mut e, Some(&planes[n]));
                }
            }
        }
    }

    // One package per footprint, from its first part.
    let mut pkg_of: Vec<usize> = Vec::with_capacity(board.components.len());
    let mut pkgs: Vec<(String, usize)> = Vec::new();
    for (i, c) in board.components.iter().enumerate() {
        let key = c.footprint.clone();
        let k = match pkgs
            .iter()
            .position(|p| p.0 == key && pins[p.1].len() == pins[i].len())
        {
            Some(k) => k,
            None => {
                pkgs.push((key, i));
                pkgs.len() - 1
            }
        };
        pkg_of.push(k);
    }
    // Where a pad sits in its part's own frame (unturned, seen from the top).
    let local = |c: &trace_core::Component, at: Point| -> (i64, i64) {
        let (dx, dy) = ((at.x.0 - c.x.0) as f64, (at.y.0 - c.y.0) as f64);
        let (s, co) = (-c.rotation_deg).to_radians().sin_cos();
        let (x, y) = (dx * co - dy * s, dx * s + dy * co);
        let x = if c.side == Side::Bottom { -x } else { x };
        (x.round() as i64, y.round() as i64)
    };
    e.push_str("#\n#Packages\n#\n");
    let mut names: Vec<String> = Vec::new();
    for (k, (fp, first)) in pkgs.iter().enumerate() {
        let c = &board.components[*first];
        let mut name = word(fp);
        if names.contains(&name) {
            name = format!("{name}__{k}");
        }
        names.push(name.clone());
        let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        let mut rows = String::new();
        let mut centres = Vec::new();
        for &pi in &pins[*first] {
            let p = &geo.pads[pi];
            let (x, y) = local(c, p.at);
            centres.push((x, y));
            let (hx, hy) = (p.size_x.0 / 2, p.size_y.0 / 2);
            let h = hx.max(hy);
            b = [
                b[0].min(x - h),
                b[1].min(y - h),
                b[2].max(x + h),
                b[3].max(y + h),
            ];
            let kind = if p.hole.0 > 0 { 'T' } else { 'S' };
            let _ = writeln!(
                rows,
                "PIN {} {kind} {} {} 0 E {}",
                word(&p.name),
                mm(x),
                mm(y),
                if p.hole.0 > 0 { 'T' } else { 'S' }
            );
            let _ = writeln!(
                rows,
                "RC {} {} {} {}",
                mm(x - hx),
                mm(y - hy),
                mm(2 * hx),
                mm(2 * hy)
            );
        }
        if b[0] > b[2] {
            b = [0, 0, 0, 0];
        }
        let mut pitch = i64::MAX;
        for (i, a) in centres.iter().enumerate() {
            for q in &centres[i + 1..] {
                let d = ((a.0 - q.0) as f64).hypot((a.1 - q.1) as f64) as i64;
                if d > 0 {
                    pitch = pitch.min(d);
                }
            }
        }
        if pitch == i64::MAX {
            pitch = 0;
        }
        let _ = writeln!(
            e,
            "# PKG {k}\nPKG {name} {} {} {} {} {}",
            mm(pitch),
            mm(b[0]),
            mm(b[1]),
            mm(b[2]),
            mm(b[3])
        );
        let _ = writeln!(
            e,
            "RC {} {} {} {}",
            mm(b[0]),
            mm(b[1]),
            mm(b[2] - b[0]),
            mm(b[3] - b[1])
        );
        e.push_str(&rows);
    }

    let mut files = Vec::new();
    for side in [Side::Top, Side::Bottom] {
        let bottom = side == Side::Bottom;
        let mirror = if bottom { 'M' } else { 'N' };
        let mut t = String::from("#\n#Units\n#\nUNITS=MM\n#\n#Components\n#\n");
        for (i, c) in board.components.iter().enumerate() {
            if c.side != side {
                continue;
            }
            let _ = writeln!(
                t,
                "# CMP {}\nCMP {} {} {} {} {mirror} {} {}",
                side_index[i],
                pkg_of[i],
                mm(c.x.0),
                mm(c.y.0),
                deg(-c.rotation_deg),
                word(&c.designator),
                word(&c.footprint)
            );
            for (k, v) in &c.params {
                if !v.is_empty() && !k.is_empty() {
                    let _ = writeln!(t, "PRP {} '{}'", word(k), v.replace('\'', "`"));
                }
            }
            for (ti, &pi) in pins[i].iter().enumerate() {
                let p = &geo.pads[pi];
                let (net, sub) = pad_sub.get(&pi).copied().unwrap_or((none, 0));
                let _ = writeln!(
                    t,
                    "TOP {ti} {} {} {} {mirror} {net} {sub} {}",
                    mm(p.at.x.0),
                    mm(p.at.y.0),
                    deg(-p.rotation_deg),
                    word(&p.name)
                );
            }
            t.push_str("#\n");
        }
        files.push((
            if bottom { "comp_+_bot" } else { "comp_+_top" }.to_string(),
            t,
        ));
    }
    (e, files)
}

/// The time now as ODB++ writes it, `yyyymmdd.hhmmss` (UTC).
fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's method).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}{m:02}{d:02}.{:02}{:02}{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// The job packed as a gzip-compressed tar archive (`.tgz`).
pub fn odb_tgz(files: &[FabFile]) -> std::io::Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for f in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(f.data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(mtime);
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, &f.name, f.data.as_slice())?;
    }
    tar.into_inner()?.finish()
}

/// The board as an ODB++ job named `job`, packed as `.tgz`.
pub fn to_odb(board: &Board, job: &str, opt: &FabOptions) -> std::io::Result<Vec<u8>> {
    odb_tgz(&odb_files(board, job, opt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fab::tests::{board, pt};
    use trace_core::*;

    #[test]
    fn job_reads_back_with_its_nets_parts_and_holes() {
        let mut b = board();
        // A copper area with a hole, and a slot.
        b.geometry.polygons.push(Polygon {
            layer: LayerRef::Copper(1),
            net: Some(0),
            outline: vec![pt(1.0, 1.0), pt(39.0, 1.0), pt(39.0, 11.0), pt(1.0, 11.0)],
            holes: vec![vec![pt(5.0, 5.0), pt(6.0, 5.0), pt(6.0, 6.0)]],
            pour: None,
            component: None,
        });
        // A core between the two copper layers.
        b.stackup.layers[0].thickness = Length::from_mm(0.035);
        b.stackup.layers.insert(
            1,
            StackLayer {
                name: "Core".into(),
                altium_id: None,
                kind: LayerKind::Dielectric,
                thickness: Length::from_mm(1.5),
                dielectric_constant: Some(4.2),
                material: Some("FR-4".into()),
            },
        );
        let files = odb_files(&b, "Test Job", &FabOptions::default());
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        for want in [
            "test_job/matrix/matrix",
            "test_job/misc/info",
            "test_job/steps/pcb/profile",
            "test_job/steps/pcb/eda/data",
            "test_job/steps/pcb/layers/top/features",
            "test_job/steps/pcb/layers/bottom/features",
            "test_job/steps/pcb/layers/smt/features",
            "test_job/steps/pcb/layers/drill/features",
            "test_job/steps/pcb/layers/comp_+_top/components",
        ] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        let text = |n: &str| {
            String::from_utf8(
                files
                    .iter()
                    .find(|f| f.name.ends_with(n))
                    .unwrap()
                    .data
                    .clone(),
            )
            .unwrap()
        };
        let top = text("layers/top/features");
        // 90° rect swaps its sides; 45° becomes a user symbol.
        assert!(top.contains("rect900x800"), "{top}");
        assert!(top.contains("trace_pad_1"), "{top}");
        // The area keeps its hole as a hole contour.
        assert!(text("layers/bottom/features").contains(" H\n"));

        let tgz = to_odb(&b, "Test Job", &FabOptions::default()).unwrap();
        let dir = std::env::temp_dir().join(format!("trace-odb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.tgz");
        std::fs::write(&path, tgz).unwrap();
        let back = trace_gerber::read_path(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let g = &back.board.geometry;
        assert_eq!(back.board.stackup.copper_layers().count(), 2);
        // The stack-up comes back with its names, thicknesses and core.
        let st = &back.board.stackup.layers;
        assert_eq!(st.len(), 3);
        assert_eq!(st[0].name, "Top");
        assert!((st[0].thickness.mm() - 0.035).abs() < 1e-6);
        assert_eq!(st[1].kind, LayerKind::Dielectric);
        assert!((st[1].thickness.mm() - 1.5).abs() < 1e-6);
        assert_eq!(st[1].dielectric_constant, Some(4.2));
        assert_eq!(st[1].material.as_deref(), Some("FR-4"));
        assert_eq!(g.vias.len(), 1, "{:?}", back.warnings);
        assert_eq!(
            g.tracks
                .iter()
                .filter(|t| matches!(t.layer, LayerRef::Copper(_)))
                .count(),
            1
        );
        assert_eq!(g.pads.iter().filter(|p| p.hole.0 > 0).count(), 1);
        let names: Vec<&str> = back.board.nets.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"SIG_WITH_A_VERY_LONG_NAME"), "{names:?}");
        assert!(names.contains(&"GND"));
        let r10 = back
            .board
            .components
            .iter()
            .find(|c| c.designator == "R10")
            .expect("R10");
        assert!(
            (r10.rotation_deg - 90.0).abs() < 1e-6,
            "{}",
            r10.rotation_deg
        );
        assert!(back.board.components.iter().any(|c| c.designator == "R2"));
        // Pads keep their part and pin.
        let p = g
            .pads
            .iter()
            .find(|p| p.name == "2" && p.component.is_some())
            .expect("pin 2");
        assert_eq!(
            back.board.nets[p.net.unwrap() as usize].name,
            "SIG_WITH_A_VERY_LONG_NAME"
        );
        assert_eq!(back.board.geometry.outline.len(), 4);
    }

    #[test]
    fn dates_are_civil() {
        let s = stamp();
        assert_eq!(s.len(), 15);
        assert!(s.starts_with("20"));
    }
}
