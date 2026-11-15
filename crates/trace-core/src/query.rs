//! Board queries in the manner of Altium's PCB Filter: a small expression
//! language that picks tracks, arcs, vias and parts.
//!
//! ```text
//! IsTrack And InNet('GND') And Width < 10mil
//! IsVia And HoleSize >= 0.2mm
//! IsComponent And Footprint Like 'SOT23*' And OnBottom
//! InNetClass('DDR') Or InDifferentialPair('PCIE_TX0')
//! ```
//!
//! Words are not case-sensitive. Strings take single or double quotes.
//! Lengths take mil (the default), mm, um or in. `Like` matches `*` and
//! `?`. `And`, `Or`, `Not` (or `&&`, `||`, `!`) and brackets combine.

use crate::select::Sel;
use crate::{Board, LayerRef, Side};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Num(f64),
    Op(&'static str),
    Open,
    Close,
    Comma,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '(' {
            out.push(Tok::Open);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::Close);
            i += 1;
        } else if ch == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else if ch == '\'' || ch == '"' {
            let q = ch;
            let mut j = i + 1;
            while j < c.len() && c[j] != q {
                j += 1;
            }
            if j >= c.len() {
                return Err("a string has no closing quote".into());
            }
            out.push(Tok::Str(c[i + 1..j].iter().collect()));
            i = j + 1;
        } else if ch.is_ascii_digit()
            || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let mut j = i;
            while j < c.len() && (c[j].is_ascii_digit() || c[j] == '.') {
                j += 1;
            }
            let n: f64 = c[i..j]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| "a number is not readable")?;
            // A unit right after the number.
            let mut k = j;
            while k < c.len() && c[k] == ' ' {
                k += 1;
            }
            let mut e = k;
            while e < c.len() && c[e].is_ascii_alphabetic() {
                e += 1;
            }
            let unit: String = c[k..e].iter().collect::<String>().to_ascii_lowercase();
            let (v, used) = match unit.as_str() {
                "mil" | "mils" => (n, true),
                "mm" => (n / 0.0254, true),
                "um" => (n / 25.4, true),
                "in" | "inch" => (n * 1000.0, true),
                _ => (n, false),
            };
            out.push(Tok::Num(v));
            i = if used { e } else { j };
        } else if ch.is_alphabetic() || ch == '_' {
            let mut j = i;
            while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
                j += 1;
            }
            out.push(Tok::Word(c[i..j].iter().collect()));
            i = j;
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op: &'static str = match two.as_str() {
                "<=" => "<=",
                ">=" => ">=",
                "<>" => "<>",
                "!=" => "<>",
                "==" => "=",
                "&&" => "and",
                "||" => "or",
                _ => match ch {
                    '<' => "<",
                    '>' => ">",
                    '=' => "=",
                    '!' => "not",
                    _ => return Err(format!("'{ch}' is not part of the language")),
                },
            };
            i += if matches!(two.as_str(), "<=" | ">=" | "<>" | "!=" | "==" | "&&" | "||") {
                2
            } else {
                1
            };
            out.push(Tok::Op(op));
        }
    }
    Ok(out)
}

/// A parsed query.
#[derive(Clone, Debug)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp(Box<Expr>, &'static str, Box<Expr>),
    Call(String, Vec<Expr>),
    Word(String),
    Str(String),
    Num(f64),
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut a = self.and()?;
        while self.is_word("or") || self.peek() == Some(&Tok::Op("or")) {
            self.i += 1;
            a = Expr::Or(Box::new(a), Box::new(self.and()?));
        }
        Ok(a)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut a = self.not()?;
        while self.is_word("and") || self.peek() == Some(&Tok::Op("and")) {
            self.i += 1;
            a = Expr::And(Box::new(a), Box::new(self.not()?));
        }
        Ok(a)
    }
    fn not(&mut self) -> Result<Expr, String> {
        if self.is_word("not") || self.peek() == Some(&Tok::Op("not")) {
            self.i += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }
    fn cmp(&mut self) -> Result<Expr, String> {
        let a = self.primary()?;
        let op = match self.peek() {
            Some(Tok::Op(o)) if !matches!(*o, "and" | "or" | "not") => *o,
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("like") => "like",
            _ => return Ok(a),
        };
        self.i += 1;
        let b = self.primary()?;
        Ok(Expr::Cmp(Box::new(a), op, Box::new(b)))
    }
    fn primary(&mut self) -> Result<Expr, String> {
        let t = self.peek().cloned().ok_or("the query ends too soon")?;
        self.i += 1;
        match t {
            Tok::Open => {
                let e = self.or()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err("a bracket is not closed".into());
                }
                self.i += 1;
                Ok(e)
            }
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Word(w) => {
                if self.peek() == Some(&Tok::Open) {
                    self.i += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::Close) {
                        loop {
                            args.push(self.or()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.i += 1,
                                Some(Tok::Close) => break,
                                _ => return Err(format!("{w}( needs a closing bracket")),
                            }
                        }
                    }
                    self.i += 1;
                    Ok(Expr::Call(w.to_ascii_lowercase(), args))
                } else {
                    Ok(Expr::Word(w.to_ascii_lowercase()))
                }
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

pub fn parse(s: &str) -> Result<Expr, String> {
    let t = lex(s)?;
    if t.is_empty() {
        return Err("the query is empty".into());
    }
    let mut p = Parser { t, i: 0 };
    let e = p.or()?;
    if p.i < p.t.len() {
        return Err(format!("unexpected {:?} after the end", p.t[p.i]));
    }
    check(&e)?;
    Ok(e)
}

const WORDS: &[&str] = &[
    "istrack",
    "isarc",
    "isvia",
    "iscomponent",
    "ontop",
    "onbottom",
    "net",
    "layer",
    "width",
    "holesize",
    "diameter",
    "designator",
    "name",
    "footprint",
    "comment",
    "value",
    "rotation",
    "side",
    "length",
    "x",
    "y",
    "all",
    "true",
    "false",
    "isfree",
    "hasnet",
];
const CALLS: &[&str] = &[
    "innet",
    "innetclass",
    "onlayer",
    "incomponent",
    "indifferentialpair",
    "incomponentclass",
    "hasfootprint",
    "isdesignator",
];

fn check(e: &Expr) -> Result<(), String> {
    match e {
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Cmp(a, _, b) => {
            check(a)?;
            check(b)
        }
        Expr::Not(a) => check(a),
        Expr::Call(f, args) => {
            if !CALLS.contains(&f.as_str()) {
                return Err(format!("{f}() is not known; try {}", CALLS.join(", ")));
            }
            args.iter().try_for_each(check)
        }
        Expr::Word(w) => {
            if WORDS.contains(&w.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "'{w}' is not known; words are {}",
                    WORDS.join(", ")
                ))
            }
        }
        _ => Ok(()),
    }
}

/// One object a query looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Obj {
    Track(usize),
    Arc(usize),
    Via(usize),
    Part(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    B(bool),
    N(f64),
    S(String),
    None,
}

fn like(s: &str, pat: &str) -> bool {
    let s: Vec<char> = s.to_lowercase().chars().collect();
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    // Wildcard match with backtracking over the last star.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while i < s.len() {
        if j < p.len() && (p[j] == '?' || p[j] == s[i]) {
            i += 1;
            j += 1;
        } else if j < p.len() && p[j] == '*' {
            star = Some(j);
            mark = i;
            j += 1;
        } else if let Some(st) = star {
            j = st + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while j < p.len() && p[j] == '*' {
        j += 1;
    }
    j == p.len()
}

struct Ctx<'a> {
    b: &'a Board,
    copper: Vec<String>,
}

impl Ctx<'_> {
    fn layer_name(&self, l: &LayerRef) -> String {
        match l {
            LayerRef::Copper(i) => self
                .copper
                .get(*i as usize)
                .cloned()
                .unwrap_or_else(|| format!("Copper {}", i + 1)),
            LayerRef::TopSilk => "Top Overlay".into(),
            LayerRef::BottomSilk => "Bottom Overlay".into(),
            LayerRef::AllCopper => "Multi-Layer".into(),
            LayerRef::Keepout => "Keep-Out".into(),
            LayerRef::Mechanical(i) => format!("Mechanical {i}"),
            LayerRef::Other(i) => format!("Other {i}"),
        }
    }

    fn net_of(&self, o: Obj) -> Option<&str> {
        let g = &self.b.geometry;
        let n = match o {
            Obj::Track(i) => g.tracks[i].net,
            Obj::Arc(i) => g.arcs[i].net,
            Obj::Via(i) => g.vias[i].net,
            Obj::Part(_) => None,
        }?;
        self.b.nets.get(n as usize).map(|n| n.name.as_str())
    }

    /// Copper layers an object is on (0 = top).
    fn layers(&self, o: Obj) -> Vec<String> {
        let g = &self.b.geometry;
        match o {
            Obj::Track(i) => vec![self.layer_name(&g.tracks[i].layer)],
            Obj::Arc(i) => vec![self.layer_name(&g.arcs[i].layer)],
            Obj::Via(i) => {
                let v = &g.vias[i];
                (v.from_layer..=v.to_layer)
                    .map(|l| self.layer_name(&LayerRef::Copper(l)))
                    .collect()
            }
            Obj::Part(i) => {
                let last = self.copper.len().saturating_sub(1) as u8;
                vec![self.layer_name(&LayerRef::Copper(
                    if self.b.components[i].side == Side::Top {
                        0
                    } else {
                        last
                    },
                ))]
            }
        }
    }

    fn word(&self, w: &str, o: Obj) -> Val {
        let g = &self.b.geometry;
        let s = |x: &str| Val::S(x.to_string());
        let part = match o {
            Obj::Part(i) => Some(&self.b.components[i]),
            _ => None,
        };
        let last = self.copper.len().saturating_sub(1) as u8;
        match w {
            "all" | "true" => Val::B(true),
            "false" => Val::B(false),
            "istrack" => Val::B(matches!(o, Obj::Track(_))),
            "isarc" => Val::B(matches!(o, Obj::Arc(_))),
            "isvia" => Val::B(matches!(o, Obj::Via(_))),
            "iscomponent" => Val::B(matches!(o, Obj::Part(_))),
            "hasnet" => Val::B(self.net_of(o).is_some()),
            "isfree" => Val::B(!matches!(o, Obj::Part(_)) && self.net_of(o).is_none()),
            "ontop" | "onbottom" => {
                let top = w == "ontop";
                Val::B(match o {
                    Obj::Track(i) => {
                        g.tracks[i].layer == LayerRef::Copper(if top { 0 } else { last })
                    }
                    Obj::Arc(i) => g.arcs[i].layer == LayerRef::Copper(if top { 0 } else { last }),
                    Obj::Via(i) => {
                        let v = &g.vias[i];
                        if top {
                            v.from_layer == 0
                        } else {
                            v.to_layer == last
                        }
                    }
                    Obj::Part(i) => (self.b.components[i].side == Side::Top) == top,
                })
            }
            "net" => self.net_of(o).map(s).unwrap_or(Val::None),
            "layer" => match o {
                Obj::Track(_) | Obj::Arc(_) | Obj::Part(_) => Val::S(self.layers(o).remove(0)),
                Obj::Via(_) => Val::None,
            },
            "width" => match o {
                Obj::Track(i) => Val::N(g.tracks[i].width.mil()),
                Obj::Arc(i) => Val::N(g.arcs[i].width.mil()),
                _ => Val::None,
            },
            "holesize" => match o {
                Obj::Via(i) => Val::N(g.vias[i].hole.mil()),
                _ => Val::None,
            },
            "diameter" => match o {
                Obj::Via(i) => Val::N(g.vias[i].diameter.mil()),
                _ => Val::None,
            },
            "length" => match o {
                Obj::Track(i) => {
                    let t = &g.tracks[i];
                    Val::N((t.end.x.mil() - t.start.x.mil()).hypot(t.end.y.mil() - t.start.y.mil()))
                }
                Obj::Arc(i) => {
                    let a = &g.arcs[i];
                    let mut sweep = (a.end_deg - a.start_deg).rem_euclid(360.0);
                    if sweep == 0.0 {
                        sweep = 360.0;
                    }
                    Val::N(a.radius.mil() * sweep.to_radians())
                }
                _ => Val::None,
            },
            "x" | "y" => {
                let (x, y) = match o {
                    Obj::Track(i) => {
                        let t = &g.tracks[i];
                        (
                            (t.start.x.mil() + t.end.x.mil()) / 2.0,
                            (t.start.y.mil() + t.end.y.mil()) / 2.0,
                        )
                    }
                    Obj::Arc(i) => (g.arcs[i].center.x.mil(), g.arcs[i].center.y.mil()),
                    Obj::Via(i) => (g.vias[i].at.x.mil(), g.vias[i].at.y.mil()),
                    Obj::Part(i) => (self.b.components[i].x.mil(), self.b.components[i].y.mil()),
                };
                Val::N(if w == "x" { x } else { y })
            }
            "designator" | "name" => part.map(|c| s(&c.designator)).unwrap_or(Val::None),
            "footprint" => part.map(|c| s(&c.footprint)).unwrap_or(Val::None),
            "comment" | "value" => part
                .and_then(|c| c.param(&["Value", "Comment"]))
                .map(s)
                .unwrap_or(Val::None),
            "rotation" => part.map(|c| Val::N(c.rotation_deg)).unwrap_or(Val::None),
            "side" => part
                .map(|c| s(if c.side == Side::Top { "Top" } else { "Bottom" }))
                .unwrap_or(Val::None),
            _ => Val::None,
        }
    }

    fn eval(&self, e: &Expr, o: Obj) -> Val {
        match e {
            Expr::And(a, b) => Val::B(self.truth(a, o) && self.truth(b, o)),
            Expr::Or(a, b) => Val::B(self.truth(a, o) || self.truth(b, o)),
            Expr::Not(a) => Val::B(!self.truth(a, o)),
            Expr::Num(n) => Val::N(*n),
            Expr::Str(s) => Val::S(s.clone()),
            Expr::Word(w) => self.word(w, o),
            Expr::Cmp(a, op, b) => {
                let (x, y) = (self.eval(a, o), self.eval(b, o));
                Val::B(match (x, y) {
                    (Val::N(x), Val::N(y)) => {
                        let eps = 1e-6;
                        match *op {
                            "=" => (x - y).abs() < eps,
                            "<>" => (x - y).abs() >= eps,
                            "<" => x < y - eps,
                            ">" => x > y + eps,
                            "<=" => x <= y + eps,
                            ">=" => x >= y - eps,
                            _ => false,
                        }
                    }
                    (Val::S(x), Val::S(y)) => match *op {
                        "=" => x.eq_ignore_ascii_case(&y),
                        "<>" => !x.eq_ignore_ascii_case(&y),
                        "like" => like(&x, &y),
                        _ => false,
                    },
                    (Val::B(x), Val::B(y)) => match *op {
                        "=" => x == y,
                        "<>" => x != y,
                        _ => false,
                    },
                    _ => false,
                })
            }
            Expr::Call(f, args) => {
                let strs: Vec<String> = args
                    .iter()
                    .filter_map(|a| match self.eval(a, o) {
                        Val::S(s) => Some(s),
                        Val::N(n) => Some(format!("{n}")),
                        _ => None,
                    })
                    .collect();
                let any = |v: &str| strs.iter().any(|s| like(v, s));
                Val::B(match f.as_str() {
                    "innet" => self.net_of(o).is_some_and(any),
                    "onlayer" => self.layers(o).iter().any(|l| any(l)),
                    "innetclass" => self.net_of(o).is_some_and(|n| {
                        self.b
                            .net_classes
                            .iter()
                            .filter(|c| any(&c.name))
                            .any(|c| c.members.iter().any(|m| m == n))
                    }),
                    "indifferentialpair" => self.net_of(o).is_some_and(|n| {
                        self.b
                            .diff_pairs
                            .iter()
                            .filter(|p| any(&p.name))
                            .any(|p| p.positive == n || p.negative == n)
                    }),
                    "incomponent" | "isdesignator" => match o {
                        Obj::Part(i) => any(&self.b.components[i].designator),
                        _ => false,
                    },
                    "hasfootprint" => match o {
                        Obj::Part(i) => any(&self.b.components[i].footprint),
                        _ => false,
                    },
                    "incomponentclass" => match o {
                        Obj::Part(i) => {
                            let d = &self.b.components[i].designator;
                            self.b
                                .component_classes
                                .iter()
                                .filter(|c| any(&c.name))
                                .any(|c| c.members.iter().any(|m| m == d))
                        }
                        _ => false,
                    },
                    _ => false,
                })
            }
        }
    }

    fn truth(&self, e: &Expr, o: Obj) -> bool {
        matches!(self.eval(e, o), Val::B(true))
    }
}

/// Everything the query picks: the board's own tracks, arcs and vias, and
/// parts. A part's own drawing goes with the part.
pub fn run(board: &Board, e: &Expr) -> Sel {
    let cx = Ctx {
        b: board,
        copper: board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect(),
    };
    let g = &board.geometry;
    let mut sel = Sel::default();
    for (i, t) in g.tracks.iter().enumerate() {
        if t.component.is_none() && cx.truth(e, Obj::Track(i)) {
            sel.tracks.insert(i);
        }
    }
    for (i, a) in g.arcs.iter().enumerate() {
        if a.component.is_none() && cx.truth(e, Obj::Arc(i)) {
            sel.arcs.insert(i);
        }
    }
    for i in 0..g.vias.len() {
        if cx.truth(e, Obj::Via(i)) {
            sel.vias.insert(i);
        }
    }
    for i in 0..board.components.len() {
        if cx.truth(e, Obj::Part(i)) {
            sel.parts.insert(i);
        }
    }
    sel
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', ""))
}

/// The attributes of an object that Find Similar Objects can match on:
/// (name, value shown, query term).
pub fn attributes(board: &Board, o: Obj) -> Vec<(&'static str, String, String)> {
    let cx = Ctx {
        b: board,
        copper: board
            .stackup
            .copper_layers()
            .map(|l| l.name.clone())
            .collect(),
    };
    let g = &board.geometry;
    let mut out = Vec::new();
    let kind = match o {
        Obj::Track(_) => ("Kind", "Track", "IsTrack"),
        Obj::Arc(_) => ("Kind", "Arc", "IsArc"),
        Obj::Via(_) => ("Kind", "Via", "IsVia"),
        Obj::Part(_) => ("Kind", "Component", "IsComponent"),
    };
    out.push((kind.0, kind.1.to_string(), kind.2.to_string()));
    if let Some(n) = cx.net_of(o) {
        out.push(("Net", n.to_string(), format!("InNet({})", quote(n))));
    }
    let mil = |v: f64| format!("{:.4}mil", v);
    match o {
        Obj::Track(i) => {
            let t = &g.tracks[i];
            let l = cx.layer_name(&t.layer);
            out.push(("Layer", l.clone(), format!("OnLayer({})", quote(&l))));
            out.push((
                "Width",
                format!("{:.2} mil", t.width.mil()),
                format!("Width = {}", mil(t.width.mil())),
            ));
        }
        Obj::Arc(i) => {
            let a = &g.arcs[i];
            let l = cx.layer_name(&a.layer);
            out.push(("Layer", l.clone(), format!("OnLayer({})", quote(&l))));
            out.push((
                "Width",
                format!("{:.2} mil", a.width.mil()),
                format!("Width = {}", mil(a.width.mil())),
            ));
        }
        Obj::Via(i) => {
            let v = &g.vias[i];
            out.push((
                "Hole",
                format!("{:.2} mil", v.hole.mil()),
                format!("HoleSize = {}", mil(v.hole.mil())),
            ));
            out.push((
                "Diameter",
                format!("{:.2} mil", v.diameter.mil()),
                format!("Diameter = {}", mil(v.diameter.mil())),
            ));
            let span = format!(
                "OnLayer({}) And OnLayer({})",
                quote(&cx.layer_name(&LayerRef::Copper(v.from_layer))),
                quote(&cx.layer_name(&LayerRef::Copper(v.to_layer)))
            );
            out.push((
                "Span",
                format!("{} to {}", v.from_layer + 1, v.to_layer + 1),
                span,
            ));
        }
        Obj::Part(i) => {
            let c = &board.components[i];
            out.push((
                "Footprint",
                c.footprint.clone(),
                format!("Footprint = {}", quote(&c.footprint)),
            ));
            if let Some(v) = c.param(&["Value", "Comment"]) {
                out.push(("Value", v.to_string(), format!("Value = {}", quote(v))));
            }
            let side = if c.side == Side::Top { "Top" } else { "Bottom" };
            out.push(("Side", side.to_string(), format!("Side = '{side}'")));
            out.push((
                "Rotation",
                format!("{:.1}°", c.rotation_deg),
                format!("Rotation = {}", c.rotation_deg),
            ));
            let prefix: String = c
                .designator
                .chars()
                .take_while(|ch| ch.is_alphabetic())
                .collect();
            if !prefix.is_empty() {
                out.push((
                    "Designator prefix",
                    format!("{prefix}…"),
                    format!("Designator Like '{prefix}*'"),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(like("DDR_DQ12", "ddr*"));
        assert!(like("U12", "U??"));
        assert!(!like("U123", "U??"));
        assert!(like("ABC", "*"));
        assert!(like("A_B_C", "*_C"));
    }

    #[test]
    fn parses() {
        assert!(parse("IsTrack And InNet('GND') And Width < 10mil").is_ok());
        assert!(parse("(IsVia or IsTrack) && !HasNet").is_ok());
        assert!(parse("IsVia And HoleSize >= 0.2mm").is_ok());
        assert!(parse("IsTrack And Bogus").is_err());
        assert!(parse("InNet('GND'").is_err());
        assert!(parse("Width < ").is_err());
    }

    #[test]
    fn units() {
        let t = lex("0.254mm 10 mil 1in 7").unwrap();
        let n: Vec<f64> = t
            .iter()
            .map(|t| if let Tok::Num(n) = t { *n } else { f64::NAN })
            .collect();
        for (a, b) in n.iter().zip([10.0, 10.0, 1000.0, 7.0]) {
            assert!((a - b).abs() < 1e-9, "{a} {b}");
        }
    }
}
