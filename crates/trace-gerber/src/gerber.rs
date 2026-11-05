//! RS-274X (extended Gerber) reader, following the Ucamco Gerber Layer
//! Format Specification, including the X2 attributes and the deprecated
//! commands older CAD tools still write.
//!
//! Everything is converted to millimetres with y pointing up.

use crate::geom::{self, Pt};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum Shape {
    Circle(f64),
    Rect(f64, f64),
    Obround(f64, f64),
    Polygon {
        d: f64,
        n: u32,
        rot: f64,
    },
    Macro(Vec<Prim>),
    /// Aperture block (`%AB`): a group of objects flashed as one.
    Block(Vec<Item>),
}

/// An evaluated macro primitive, in millimetres around the flash point.
#[derive(Clone, Debug)]
pub struct Prim {
    pub on: bool,
    pub kind: PrimKind,
}

#[derive(Clone, Debug)]
pub enum PrimKind {
    Circle {
        c: Pt,
        d: f64,
    },
    /// Closed outline (first point not repeated).
    Outline(Vec<Pt>),
}

#[derive(Clone, Debug)]
pub struct Aperture {
    pub dcode: u32,
    pub shape: Shape,
    /// Round hole in a standard aperture (rarely used).
    pub hole: f64,
    /// X2 `.AperFunction`, such as `ViaPad`, `SMDPad,CuDef` or `Conductor`.
    pub function: Option<String>,
}

/// Load transformation in effect when an aperture was flashed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub rot: f64,
    pub scale: f64,
}

impl Default for Xform {
    fn default() -> Self {
        Xform {
            mirror_x: false,
            mirror_y: false,
            rot: 0.0,
            scale: 1.0,
        }
    }
}

impl Xform {
    pub fn is_identity(&self) -> bool {
        *self == Xform::default()
    }
    pub fn apply(&self, p: Pt) -> Pt {
        let (mut x, mut y) = (p[0] * self.scale, p[1] * self.scale);
        if self.mirror_x {
            x = -x;
        }
        if self.mirror_y {
            y = -y;
        }
        geom::rotate([x, y], self.rot)
    }
}

#[derive(Clone, Debug)]
pub enum Obj {
    Flash {
        ap: u32,
        at: Pt,
        xf: Xform,
    },
    Line {
        ap: u32,
        a: Pt,
        b: Pt,
    },
    /// Circular arc from `a` to `b` around `c`; `a == b` is a full circle.
    Arc {
        ap: u32,
        a: Pt,
        b: Pt,
        c: Pt,
        cw: bool,
    },
    /// Filled region; each contour is closed and the contours are unioned.
    Region(Vec<Vec<Pt>>),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub obj: Obj,
    pub dark: bool,
    /// Index into [`Gerber::attrs`]; 0 means no object attributes.
    pub attrs: u32,
}

/// X2 object attributes (`%TO`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ObjAttrs {
    pub net: Option<String>,
    pub component: Option<String>,
    pub pin: Option<String>,
}

#[derive(Debug, Default)]
pub struct Gerber {
    pub apertures: Vec<Aperture>,
    pub items: Vec<Item>,
    pub attrs: Vec<ObjAttrs>,
    /// `%TF.FileFunction` value, such as `Copper,L2,Inr,Plane`.
    pub file_function: Option<String>,
    pub file_polarity_negative: bool,
    pub image_negative: bool,
    pub software: Option<String>,
    /// The first comments, used to recognise the layer.
    pub comments: Vec<String>,
    pub warnings: Vec<String>,
}

impl Gerber {
    pub fn aperture(&self, index: u32) -> &Aperture {
        &self.apertures[index as usize]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Omit {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, PartialEq)]
enum Interp {
    Linear,
    Cw,
    Ccw,
}

struct Frame {
    items: Vec<Item>,
    kind: FrameKind,
}

enum FrameKind {
    Root,
    Block(u32),
    Repeat { nx: u32, ny: u32, dx: f64, dy: f64 },
}

struct Parser {
    g: Gerber,
    unit: f64,
    int_digits: usize,
    dec_digits: usize,
    omit: Omit,
    incremental: bool,
    interp: Interp,
    multi_quadrant: bool,
    pos: Pt,
    ap: Option<u32>,
    dmap: HashMap<u32, u32>,
    macros: HashMap<String, Vec<String>>,
    dark: bool,
    xf: Xform,
    region: Option<Vec<Vec<Pt>>>,
    contour: Vec<Pt>,
    last_op: u32,
    aper_function: Option<String>,
    obj_attrs: ObjAttrs,
    attr_index: HashMap<ObjAttrs, u32>,
    cur_attr: u32,
    stack: Vec<Frame>,
    offset: Pt,
    done: bool,
}

/// Reads a Gerber file. Errors only when the data is clearly not Gerber;
/// small problems are recorded in `warnings`.
pub fn parse(data: &[u8]) -> Result<Gerber, String> {
    let mut p = Parser {
        g: Gerber::default(),
        unit: 25.4,
        int_digits: 2,
        dec_digits: 4,
        omit: Omit::Leading,
        incremental: false,
        interp: Interp::Linear,
        multi_quadrant: false,
        pos: [0.0, 0.0],
        ap: None,
        dmap: HashMap::new(),
        macros: HashMap::new(),
        dark: true,
        xf: Xform::default(),
        region: None,
        contour: Vec::new(),
        last_op: 2,
        aper_function: None,
        obj_attrs: ObjAttrs::default(),
        attr_index: HashMap::new(),
        cur_attr: 0,
        stack: vec![Frame {
            items: Vec::new(),
            kind: FrameKind::Root,
        }],
        offset: [0.0, 0.0],
        done: false,
    };
    p.g.attrs.push(ObjAttrs::default());
    p.attr_index.insert(ObjAttrs::default(), 0);

    let text = String::from_utf8_lossy(data);
    let mut chars = text.chars().peekable();
    let mut word = String::new();
    while let Some(c) = chars.next() {
        if p.done {
            break;
        }
        match c {
            '%' => {
                let mut ext = String::new();
                for c in chars.by_ref() {
                    if c == '%' {
                        break;
                    }
                    if c != '\r' && c != '\n' {
                        ext.push(c);
                    }
                }
                p.extended(&ext);
            }
            '*' => {
                let w = std::mem::take(&mut word);
                p.word(w.trim());
            }
            '\r' | '\n' => {}
            _ => word.push(c),
        }
    }
    if !word.trim().is_empty() {
        p.word(word.trim());
    }
    if p.region.is_some() {
        p.end_region();
    }
    while p.stack.len() > 1 {
        p.close_frame();
    }
    let root = p.stack.pop().unwrap();
    p.g.items = root.items;
    if p.g.items.is_empty() && p.g.apertures.is_empty() && p.g.file_function.is_none() {
        return Err("no Gerber data found".into());
    }
    Ok(p.g)
}

impl Parser {
    fn warn(&mut self, w: String) {
        if self.g.warnings.len() < 50 {
            self.g.warnings.push(w);
        }
    }

    fn emit(&mut self, obj: Obj) {
        let item = Item {
            obj,
            dark: self.dark,
            attrs: self.cur_attr,
        };
        self.stack.last_mut().unwrap().items.push(item);
    }

    fn close_frame(&mut self) {
        let frame = self.stack.pop().unwrap();
        match frame.kind {
            FrameKind::Root => self.stack.push(frame),
            FrameKind::Block(dcode) => {
                let index = self.g.apertures.len() as u32;
                self.g.apertures.push(Aperture {
                    dcode,
                    shape: Shape::Block(frame.items),
                    hole: 0.0,
                    function: self.aper_function.clone(),
                });
                self.dmap.insert(dcode, index);
            }
            FrameKind::Repeat { nx, ny, dx, dy } => {
                let parent = &mut self.stack.last_mut().unwrap().items;
                for j in 0..ny {
                    for i in 0..nx {
                        let d = [i as f64 * dx, j as f64 * dy];
                        for it in &frame.items {
                            parent.push(translate_item(it, d));
                        }
                    }
                }
            }
        }
    }

    // ---- extended (%...%) commands -------------------------------------

    fn extended(&mut self, ext: &str) {
        let ext = ext.trim();
        if let Some(rest) = ext.strip_prefix("AM") {
            let mut blocks = rest.split('*');
            let name = blocks.next().unwrap_or("").trim().to_string();
            let body: Vec<String> = blocks
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect();
            self.macros.insert(name, body);
            return;
        }
        for block in ext.split('*') {
            let block = block.trim();
            if !block.is_empty() {
                self.ext_block(block);
            }
        }
    }

    fn ext_block(&mut self, b: &str) {
        // Byte 2 can fall inside a non-ASCII character in a garbled block.
        let cut = (0..=b.len().min(2))
            .rev()
            .find(|&i| b.is_char_boundary(i))
            .unwrap_or(0);
        let (cmd, arg) = b.split_at(cut);
        match cmd {
            "FS" => self.format(arg),
            "MO" => {
                self.unit = if arg.starts_with("IN") { 25.4 } else { 1.0 };
            }
            "AD" => self.aperture_def(arg),
            "LP" => self.dark = !arg.starts_with('C'),
            "LM" => {
                self.xf.mirror_x = arg.contains('X');
                self.xf.mirror_y = arg.contains('Y');
            }
            "LR" => self.xf.rot = arg.parse().unwrap_or(0.0),
            "LS" => self.xf.scale = arg.parse().unwrap_or(1.0),
            "IP" => self.g.image_negative = arg.starts_with("NEG"),
            "OF" => {
                let a = parse_letters(arg);
                let get = |k| {
                    a.iter()
                        .find(|(c, _)| *c == k)
                        .map(|(_, v)| v.parse().unwrap_or(0.0))
                };
                self.offset = [
                    get('A').unwrap_or(0.0) * self.unit,
                    get('B').unwrap_or(0.0) * self.unit,
                ];
            }
            "SR" => {
                // Ends the previous step and repeat, then opens a new one if
                // counts are given.
                if matches!(self.stack.last().unwrap().kind, FrameKind::Repeat { .. }) {
                    self.close_frame();
                }
                if !arg.is_empty() {
                    let a = parse_letters(arg);
                    let get = |k| {
                        a.iter()
                            .find(|(c, _)| *c == k)
                            .map(|(_, v)| v.parse::<f64>().unwrap_or(0.0))
                    };
                    let nx = get('X').unwrap_or(1.0).max(1.0) as u32;
                    let ny = get('Y').unwrap_or(1.0).max(1.0) as u32;
                    if nx > 1 || ny > 1 {
                        self.stack.push(Frame {
                            items: Vec::new(),
                            kind: FrameKind::Repeat {
                                nx,
                                ny,
                                dx: get('I').unwrap_or(0.0) * self.unit,
                                dy: get('J').unwrap_or(0.0) * self.unit,
                            },
                        });
                    }
                }
            }
            "AB" => {
                if let Some(d) = arg.strip_prefix('D') {
                    let dcode = d.parse().unwrap_or(0);
                    self.stack.push(Frame {
                        items: Vec::new(),
                        kind: FrameKind::Block(dcode),
                    });
                } else if matches!(self.stack.last().unwrap().kind, FrameKind::Block(_)) {
                    self.close_frame();
                }
            }
            "TF" => self.file_attr(arg),
            "TA" => {
                let (name, value) = split_attr(arg);
                if name == ".AperFunction" {
                    self.aper_function = Some(value.to_string());
                }
            }
            "TO" => {
                let (name, value) = split_attr(arg);
                let mut fields = value.split(',');
                match name {
                    ".N" => self.obj_attrs.net = fields.next().map(unescape),
                    ".C" => self.obj_attrs.component = fields.next().map(unescape),
                    ".P" => {
                        self.obj_attrs.component = fields.next().map(unescape);
                        self.obj_attrs.pin = fields.next().map(unescape);
                    }
                    _ => {}
                }
                self.update_attrs();
            }
            "TD" => {
                match arg {
                    "" => {
                        self.obj_attrs = ObjAttrs::default();
                        self.aper_function = None;
                    }
                    ".N" => self.obj_attrs.net = None,
                    ".C" => self.obj_attrs.component = None,
                    ".P" => self.obj_attrs.pin = None,
                    ".AperFunction" => self.aper_function = None,
                    _ => {}
                }
                self.update_attrs();
            }
            // Deprecated image parameters that do not change geometry we use.
            "IN" | "LN" | "IR" | "AS" | "MI" | "SF" | "IF" | "IJ" | "IO" | "KO" | "PF" => {}
            _ => {
                let w = format!("unknown command %{b}%");
                self.warn(w)
            }
        }
    }

    fn update_attrs(&mut self) {
        let a = self.obj_attrs.clone();
        let next = self.g.attrs.len() as u32;
        self.cur_attr = *self.attr_index.entry(a.clone()).or_insert_with(|| {
            self.g.attrs.push(a);
            next
        });
    }

    fn file_attr(&mut self, arg: &str) {
        let (name, value) = split_attr(arg);
        match name {
            ".FileFunction" => self.g.file_function = Some(value.to_string()),
            ".FilePolarity" => self.g.file_polarity_negative = value.starts_with("Neg"),
            ".GenerationSoftware" => self.g.software = Some(value.replace(',', " ")),
            _ => {}
        }
    }

    fn format(&mut self, arg: &str) {
        let mut chars = arg.chars().peekable();
        match chars.next() {
            Some('T') => self.omit = Omit::Trailing,
            Some('D') | Some('L') => self.omit = Omit::Leading,
            _ => {}
        }
        if chars.next() == Some('I') {
            self.incremental = true;
        }
        let rest: String = chars.collect();
        for (c, v) in parse_letters(&rest) {
            if c == 'X' && v.len() >= 2 {
                let d: Vec<u32> = v.chars().filter_map(|c| c.to_digit(10)).collect();
                if d.len() >= 2 {
                    self.int_digits = d[0] as usize;
                    self.dec_digits = d[1] as usize;
                }
            }
        }
    }

    fn aperture_def(&mut self, arg: &str) {
        let Some(rest) = arg.strip_prefix('D') else {
            return;
        };
        let n = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let Ok(dcode) = rest[..n].parse::<u32>() else {
            return;
        };
        let def = &rest[n..];
        let (name, params) = match def.find(',') {
            Some(i) => (&def[..i], &def[i + 1..]),
            None => (def, ""),
        };
        let vals: Vec<f64> = params
            .split(['X', 'x'])
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse().unwrap_or(0.0))
            .collect();
        let u = self.unit;
        let v = |i: usize| vals.get(i).copied().unwrap_or(0.0);
        let (shape, hole) = match name {
            "C" => (Shape::Circle(v(0) * u), v(1) * u),
            "R" => (Shape::Rect(v(0) * u, v(1) * u), v(2) * u),
            "O" => (Shape::Obround(v(0) * u, v(1) * u), v(2) * u),
            "P" => (
                Shape::Polygon {
                    d: v(0) * u,
                    n: v(1).max(3.0) as u32,
                    rot: v(2),
                },
                v(3) * u,
            ),
            _ => match self.macros.get(name) {
                Some(body) => (Shape::Macro(eval_macro(body, &vals, u)), 0.0),
                None => {
                    self.warn(format!("aperture D{dcode} uses unknown macro {name}"));
                    (Shape::Circle(0.0), 0.0)
                }
            },
        };
        let index = self.g.apertures.len() as u32;
        self.g.apertures.push(Aperture {
            dcode,
            shape,
            hole,
            function: self.aper_function.clone(),
        });
        self.dmap.insert(dcode, index);
    }

    // ---- word commands ------------------------------------------------

    fn word(&mut self, w: &str) {
        if w.is_empty() {
            return;
        }
        if let Some(c) = w.strip_prefix("G04").or_else(|| w.strip_prefix("G4 ")) {
            self.comment(c.trim());
            return;
        }
        let mut x = None;
        let mut y = None;
        let mut i = None;
        let mut j = None;
        let mut d = None;
        for (c, v) in parse_letters(w) {
            match c {
                'G' => self.gcode(v.parse().unwrap_or(-1)),
                'D' => d = v.parse::<u32>().ok(),
                'M' => {
                    if matches!(v.parse::<u32>(), Ok(0) | Ok(2) | Ok(30)) {
                        self.done = true;
                    }
                }
                'X' => x = Some(v),
                'Y' => y = Some(v),
                'I' => i = Some(v),
                'J' => j = Some(v),
                'N' => {}
                _ => {}
            }
        }
        let has_coord = x.is_some() || y.is_some() || i.is_some() || j.is_some();
        let op = match d {
            Some(n) if n >= 10 => {
                match self.dmap.get(&n) {
                    Some(&a) => self.ap = Some(a),
                    None => {
                        self.warn(format!("D{n} selected but not defined"));
                        self.ap = None;
                    }
                }
                None
            }
            Some(n @ 1..=3) => Some(n),
            _ if has_coord => Some(self.last_op),
            _ => None,
        };
        let Some(op) = op else { return };
        self.last_op = op;
        let mut to = self.pos;
        if let Some(v) = x {
            let n = self.number(v);
            to[0] = if self.incremental {
                to[0] + n
            } else {
                n + self.offset[0]
            };
        }
        if let Some(v) = y {
            let n = self.number(v);
            to[1] = if self.incremental {
                to[1] + n
            } else {
                n + self.offset[1]
            };
        }
        let ij = [
            i.map(|v| self.number(v)).unwrap_or(0.0),
            j.map(|v| self.number(v)).unwrap_or(0.0),
        ];
        match op {
            1 => self.draw(to, ij),
            2 => {
                if self.region.is_some() {
                    self.close_contour();
                }
                self.pos = to;
            }
            _ => {
                self.pos = to;
                match self.ap {
                    Some(ap) if self.region.is_none() => self.emit(Obj::Flash {
                        ap,
                        at: to,
                        xf: self.xf,
                    }),
                    _ => {}
                }
            }
        }
    }

    fn comment(&mut self, c: &str) {
        // KiCad and others put X2 attributes in comments for old readers.
        if let Some(attr) = c.strip_prefix("#@!") {
            let attr = attr.trim();
            if let Some(a) = attr.strip_prefix("TF") {
                self.file_attr(a);
            } else if attr.starts_with("TA") || attr.starts_with("TO") || attr.starts_with("TD") {
                self.ext_block(attr);
            }
            return;
        }
        if self.g.comments.len() < 40 {
            self.g.comments.push(c.to_string());
        }
    }

    fn gcode(&mut self, n: i32) {
        match n {
            1 => self.interp = Interp::Linear,
            2 => self.interp = Interp::Cw,
            3 => self.interp = Interp::Ccw,
            36 => {
                self.region = Some(Vec::new());
                self.contour.clear();
            }
            37 => self.end_region(),
            74 => self.multi_quadrant = false,
            75 => self.multi_quadrant = true,
            70 => self.unit = 25.4,
            71 => self.unit = 1.0,
            90 => self.incremental = false,
            91 => self.incremental = true,
            _ => {}
        }
    }

    fn number(&self, s: &str) -> f64 {
        let v = if s.contains('.') {
            s.parse::<f64>().unwrap_or(0.0)
        } else {
            let (neg, digits) = match s.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, s.strip_prefix('+').unwrap_or(s)),
            };
            let n: f64 = match self.omit {
                Omit::Leading => digits.parse::<f64>().unwrap_or(0.0),
                Omit::Trailing => {
                    let total = self.int_digits + self.dec_digits;
                    let mut d = digits.to_string();
                    while d.len() < total {
                        d.push('0');
                    }
                    d.parse::<f64>().unwrap_or(0.0)
                }
            };
            let n = n / 10f64.powi(self.dec_digits as i32);
            if neg {
                -n
            } else {
                n
            }
        };
        v * self.unit
    }

    fn arc_center(&self, a: Pt, b: Pt, ij: Pt, cw: bool) -> Pt {
        if self.multi_quadrant {
            return [a[0] + ij[0], a[1] + ij[1]];
        }
        // Single quadrant: the signs of I and J are implied; pick the centre
        // that gives an arc of at most 90 degrees in the right direction.
        let mut best = [a[0] + ij[0], a[1] + ij[1]];
        let mut best_err = f64::MAX;
        for sx in [1.0, -1.0] {
            for sy in [1.0, -1.0] {
                let c = [a[0] + sx * ij[0].abs(), a[1] + sy * ij[1].abs()];
                let ra = geom::dist(c, a);
                let rb = geom::dist(c, b);
                let sweep = geom::sweep(c, a, b, cw);
                let mut err = (ra - rb).abs();
                if sweep > std::f64::consts::FRAC_PI_2 + 1e-3 {
                    err += 1e3;
                }
                if err < best_err {
                    best_err = err;
                    best = c;
                }
            }
        }
        best
    }

    fn draw(&mut self, to: Pt, ij: Pt) {
        let from = self.pos;
        self.pos = to;
        let arc = match self.interp {
            Interp::Linear => None,
            Interp::Cw => Some(true),
            Interp::Ccw => Some(false),
        };
        if let Some(contours) = &self.region {
            let _ = contours;
            if self.contour.is_empty() {
                self.contour.push(from);
            }
            match arc {
                None => self.contour.push(to),
                Some(cw) => {
                    let c = self.arc_center(from, to, ij, cw);
                    let full = self.multi_quadrant && geom::dist(from, to) < 1e-9;
                    let pts = geom::arc_points(c, from, to, cw, full);
                    self.contour.extend(pts.into_iter().skip(1));
                }
            }
            return;
        }
        let Some(ap) = self.ap else {
            self.warn("draw without an aperture".into());
            return;
        };
        match arc {
            None => self.emit(Obj::Line { ap, a: from, b: to }),
            Some(cw) => {
                let c = self.arc_center(from, to, ij, cw);
                let (a, b) = if !self.multi_quadrant && geom::dist(from, to) < 1e-9 {
                    // A single-quadrant arc cannot be a full circle.
                    return;
                } else {
                    (from, to)
                };
                self.emit(Obj::Arc { ap, a, b, c, cw })
            }
        }
    }

    fn close_contour(&mut self) {
        let c = std::mem::take(&mut self.contour);
        if c.len() >= 3 {
            if let Some(r) = &mut self.region {
                let mut c = c;
                if geom::dist(c[0], *c.last().unwrap()) < 1e-9 {
                    c.pop();
                }
                r.push(c);
            }
        }
    }

    fn end_region(&mut self) {
        self.close_contour();
        if let Some(r) = self.region.take() {
            if !r.is_empty() {
                self.emit(Obj::Region(r));
            }
        }
    }
}

fn translate_item(it: &Item, d: Pt) -> Item {
    let t = |p: Pt| [p[0] + d[0], p[1] + d[1]];
    let obj = match &it.obj {
        Obj::Flash { ap, at, xf } => Obj::Flash {
            ap: *ap,
            at: t(*at),
            xf: *xf,
        },
        Obj::Line { ap, a, b } => Obj::Line {
            ap: *ap,
            a: t(*a),
            b: t(*b),
        },
        Obj::Arc { ap, a, b, c, cw } => Obj::Arc {
            ap: *ap,
            a: t(*a),
            b: t(*b),
            c: t(*c),
            cw: *cw,
        },
        Obj::Region(cs) => Obj::Region(
            cs.iter()
                .map(|c| c.iter().map(|&p| t(p)).collect())
                .collect(),
        ),
    };
    Item {
        obj,
        dark: it.dark,
        attrs: it.attrs,
    }
}

/// Splits `XabcY-12I3` into letter/value pairs.
fn parse_letters(s: &str) -> Vec<(char, &str)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            let start = i + 1;
            let mut k = start;
            while k < b.len()
                && (b[k].is_ascii_digit() || b[k] == b'.' || b[k] == b'-' || b[k] == b'+')
            {
                k += 1;
            }
            out.push((c.to_ascii_uppercase(), &s[start..k]));
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

fn split_attr(a: &str) -> (&str, &str) {
    match a.find(',') {
        Some(i) => (&a[..i], &a[i + 1..]),
        None => (a, ""),
    }
}

/// Undoes the `\uXXXX` escapes X2 uses for special characters.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&'u') {
            it.next();
            let hex: String = it.by_ref().take(4).collect();
            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(ch);
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ---- aperture macros --------------------------------------------------

fn eval_macro(body: &[String], params: &[f64], unit: f64) -> Vec<Prim> {
    let mut vars: HashMap<u32, f64> = params
        .iter()
        .enumerate()
        .map(|(i, v)| (i as u32 + 1, *v))
        .collect();
    let mut prims = Vec::new();
    for block in body {
        let block = block.trim();
        // Primitive 0 is a comment.
        if block.is_empty()
            || block.starts_with('0')
                && !block[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.')
        {
            continue;
        }
        if let Some(def) = block.strip_prefix('$') {
            if let Some(eq) = def.find('=') {
                if let Ok(n) = def[..eq].trim().parse::<u32>() {
                    let v = expr(&def[eq + 1..], &vars);
                    vars.insert(n, v);
                }
            }
            continue;
        }
        let fields: Vec<f64> = block.split(',').map(|f| expr(f, &vars)).collect();
        let code = fields.first().copied().unwrap_or(-1.0) as i32;
        let f = |i: usize| fields.get(i).copied().unwrap_or(0.0);
        let on = f(1) != 0.0;
        let u = unit;
        match code {
            1 => {
                let rot = f(5);
                let c = geom::rotate([f(3) * u, f(4) * u], rot);
                prims.push(Prim {
                    on,
                    kind: PrimKind::Circle { c, d: f(2) * u },
                });
            }
            2 | 20 => {
                let (w, a, b, rot) = (f(2) * u, [f(3) * u, f(4) * u], [f(5) * u, f(6) * u], f(7));
                let pts = geom::thick_line(a, b, w);
                prims.push(outline(on, pts, rot));
            }
            21 => {
                let (w, h, c, rot) = (f(2) * u, f(3) * u, [f(4) * u, f(5) * u], f(6));
                prims.push(outline(on, geom::rect(c, w, h), rot));
            }
            22 => {
                let (w, h, ll, rot) = (f(2) * u, f(3) * u, [f(4) * u, f(5) * u], f(6));
                prims.push(outline(
                    on,
                    geom::rect([ll[0] + w / 2.0, ll[1] + h / 2.0], w, h),
                    rot,
                ));
            }
            4 => {
                let n = f(2).max(0.0) as usize;
                let pts: Vec<Pt> = (0..=n)
                    .map(|k| [f(3 + 2 * k) * u, f(4 + 2 * k) * u])
                    .collect();
                let rot = f(5 + 2 * n);
                let mut pts = pts;
                if pts.len() > 1 && geom::dist(pts[0], *pts.last().unwrap()) < 1e-9 {
                    pts.pop();
                }
                prims.push(outline(on, pts, rot));
            }
            5 => {
                let (n, c, d, rot) = (f(2).max(3.0) as u32, [f(3) * u, f(4) * u], f(5) * u, f(6));
                prims.push(outline(on, geom::regular_polygon(c, d, n, 0.0), rot));
            }
            6 => {
                // Moiré: keep the outer ring as a disc, good enough to view.
                let c = geom::rotate([f(1) * u, f(2) * u], f(9));
                prims.push(Prim {
                    on: true,
                    kind: PrimKind::Circle { c, d: f(3) * u },
                });
            }
            7 => {
                // Thermal: a ring cut by two crossing gaps, as four quarters.
                let (c, outer, inner, gap, rot) =
                    ([f(1) * u, f(2) * u], f(3) * u, f(4) * u, f(5) * u, f(6));
                for q in geom::thermal_quarters(c, outer, inner, gap) {
                    prims.push(outline(true, q, rot));
                }
            }
            _ => {}
        }
    }
    prims
}

fn outline(on: bool, pts: Vec<Pt>, rot: f64) -> Prim {
    Prim {
        on,
        kind: PrimKind::Outline(pts.into_iter().map(|p| geom::rotate(p, rot)).collect()),
    }
}

/// Evaluates a macro arithmetic expression: `+ - x X / ( )`, numbers and `$n`.
fn expr(s: &str, vars: &HashMap<u32, f64>) -> f64 {
    let toks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    let mut pos = 0;
    let v = sum(&toks, &mut pos, vars);
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

fn sum(t: &[char], p: &mut usize, vars: &HashMap<u32, f64>) -> f64 {
    let mut v = product(t, p, vars);
    while *p < t.len() {
        match t[*p] {
            '+' => {
                *p += 1;
                v += product(t, p, vars);
            }
            '-' => {
                *p += 1;
                v -= product(t, p, vars);
            }
            _ => break,
        }
    }
    v
}

fn product(t: &[char], p: &mut usize, vars: &HashMap<u32, f64>) -> f64 {
    let mut v = factor(t, p, vars);
    while *p < t.len() {
        match t[*p] {
            'x' | 'X' => {
                *p += 1;
                v *= factor(t, p, vars);
            }
            '/' => {
                *p += 1;
                v /= factor(t, p, vars);
            }
            _ => break,
        }
    }
    v
}

fn factor(t: &[char], p: &mut usize, vars: &HashMap<u32, f64>) -> f64 {
    if *p >= t.len() {
        return 0.0;
    }
    match t[*p] {
        '-' => {
            *p += 1;
            -factor(t, p, vars)
        }
        '+' => {
            *p += 1;
            factor(t, p, vars)
        }
        '(' => {
            *p += 1;
            let v = sum(t, p, vars);
            if *p < t.len() && t[*p] == ')' {
                *p += 1;
            }
            v
        }
        '$' => {
            *p += 1;
            let start = *p;
            while *p < t.len() && t[*p].is_ascii_digit() {
                *p += 1;
            }
            let n: u32 = t[start..*p].iter().collect::<String>().parse().unwrap_or(0);
            vars.get(&n).copied().unwrap_or(0.0)
        }
        _ => {
            let start = *p;
            while *p < t.len() && (t[*p].is_ascii_digit() || t[*p] == '.') {
                *p += 1;
            }
            if start == *p {
                *p += 1;
                return 0.0;
            }
            t[start..*p]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions() {
        let vars = HashMap::from([(1, 2.0), (2, 0.5)]);
        assert_eq!(expr("$1x3+1", &vars), 7.0);
        assert_eq!(expr("($1+$2)/2", &vars), 1.25);
        assert_eq!(expr("-$1X-2", &vars), 4.0);
    }

    #[test]
    fn simple_file() {
        let g = parse(
            b"%FSLAX26Y26*%\n%MOMM*%\n%ADD10C,0.1*%\n%ADD11R,1X2*%\nD10*\nX0Y0D02*\nX1000000Y0D01*\nD11*\nX2000000Y1000000D03*\nG36*\nX0Y0D02*\nX1000000Y0D01*\nY1000000D01*\nX0D01*\nG37*\nM02*\n",
        )
        .unwrap();
        assert_eq!(g.items.len(), 3);
        match &g.items[0].obj {
            Obj::Line { a, b, .. } => {
                assert_eq!(*a, [0.0, 0.0]);
                assert_eq!(*b, [1.0, 0.0]);
            }
            o => panic!("{o:?}"),
        }
        match &g.items[2].obj {
            Obj::Region(c) => assert_eq!(c[0].len(), 4),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn trailing_zero_format() {
        let g = parse(b"%FSTAX24Y24*%%MOIN*%%ADD10C,0.01*%D10*X01Y005D03*M02*").unwrap();
        match &g.items[0].obj {
            Obj::Flash { at, .. } => {
                assert!((at[0] - 25.4).abs() < 1e-9);
                assert!((at[1] - 12.7).abs() < 1e-9);
            }
            o => panic!("{o:?}"),
        }
    }
}
