//! Writes a sheet as an Altium `.SchDoc`.
//!
//! A Trace sheet keeps what it draws (shapes, texts, pins, wires, net tags)
//! rather than Altium's records, so the writer works the records back out:
//! pins, wires, junctions, net labels, power ports, ports, sheet symbols and
//! the sheet border are recognised by drawing each candidate record with
//! the reader's own code and finding what it draws on the sheet. What is
//! left becomes plain graphics and text, owned by its part where it has
//! one. Reading the written file gives the same drawing back.
//!
//! When the sheet was read from a `.SchDoc`, that file's records fill in
//! what a drawing does not hold: each part's hidden parameters, library
//! links and footprint models, the sheet's parameters and settings.

use super::*;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use trace_core::{SchShape, SchText};

/// The shapes and texts a record draws, by index into the sheet's lists.
type Found = (Vec<usize>, Vec<usize>);

/// One record being written, its fields in order.
#[derive(Clone, Default)]
struct Rec(Vec<(String, String)>);

impl Rec {
    fn new(kind: i64) -> Rec {
        let mut r = Rec::default();
        r.set("RECORD", kind.to_string());
        r
    }
    fn set(&mut self, k: &str, v: impl Into<String>) -> &mut Self {
        let k = k.to_ascii_uppercase();
        let v = v.into();
        match self.0.iter_mut().find(|(a, _)| *a == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k, v)),
        }
        self
    }
    fn remove(&mut self, k: &str) {
        let k = k.to_ascii_uppercase();
        self.0.retain(|(a, _)| *a != k);
    }
    fn get(&self, k: &str) -> Option<&str> {
        let k = k.to_ascii_uppercase();
        self.0
            .iter()
            .find(|(a, _)| *a == k)
            .map(|(_, v)| v.as_str())
    }
    fn flag(&mut self, k: &str, on: bool) -> &mut Self {
        if on {
            self.set(k, "T");
        } else {
            self.remove(k);
        }
        self
    }
    /// A length in mil, as Altium's 10-mil units and a fraction.
    fn len(&mut self, k: &str, mil: f64) -> &mut Self {
        let units = (mil / 10.0).floor();
        let mut frac = ((mil - units * 10.0) * 10_000.0).round() as i64;
        let mut units = units as i64;
        if frac >= 100_000 {
            units += 1;
            frac -= 100_000;
        }
        self.set(k, units.to_string());
        let fk = format!("{k}_FRAC");
        if frac != 0 {
            self.set(&fk, frac.to_string());
        } else {
            self.remove(&fk);
        }
        self
    }
    fn at(&mut self, p: Pt) -> &mut Self {
        self.len("LOCATION.X", p[0]).len("LOCATION.Y", p[1])
    }
    fn corner(&mut self, p: Pt) -> &mut Self {
        self.len("CORNER.X", p[0]).len("CORNER.Y", p[1])
    }
    fn points(&mut self, pts: &[Pt]) -> &mut Self {
        self.set("LOCATIONCOUNT", pts.len().to_string());
        for (i, p) in pts.iter().enumerate() {
            self.len(&format!("X{}", i + 1), p[0]);
            self.len(&format!("Y{}", i + 1), p[1]);
        }
        self
    }
    fn color(&mut self, k: &str, c: Rgb) -> &mut Self {
        let v = c[0] as i64 | (c[1] as i64) << 8 | (c[2] as i64) << 16;
        self.set(k, v.to_string())
    }
    fn owner(&mut self, idx: usize) -> &mut Self {
        self.set("OWNERINDEX", idx.to_string())
    }
    fn from_props(p: &Props) -> Rec {
        let mut r = Rec::new(p.int("RECORD").unwrap_or(0));
        let mut keys: Vec<(&String, &String)> = p.keys().filter(|(k, _)| *k != "RECORD").collect();
        keys.sort();
        for (k, v) in keys {
            r.set(k, v.clone());
        }
        r
    }
    fn props(&self) -> Props {
        let mut p = Props::default();
        for (k, v) in &self.0 {
            p.set(k, v.clone());
        }
        p
    }
    /// The record's bytes: `|KEY=VALUE…` in Latin-1, and before a value
    /// that is not plain ASCII its UTF-8 copy, as Altium writes them. A
    /// bar, which would end the value, becomes a broken bar (`¦`), which
    /// Altium leaves out of the Latin-1 copy.
    fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        for (k, v) in &self.0 {
            if v.chars().all(|c| c.is_ascii() && c != '|' && c != '\0') {
                b.push(b'|');
                b.extend_from_slice(k.as_bytes());
                b.push(b'=');
                b.extend_from_slice(v.as_bytes());
                continue;
            }
            let clean = v.replace('|', "\u{a6}").replace('\0', " ");
            b.extend_from_slice(format!("|%UTF8%{k}=").as_bytes());
            b.extend_from_slice(clean.as_bytes());
            b.push(b'|');
            b.extend_from_slice(k.as_bytes());
            b.push(b'=');
            b.extend(clean.chars().filter(|&c| c != '\u{a6}').map(|c| {
                if (c as u32) < 256 {
                    c as u32 as u8
                } else {
                    b'?'
                }
            }));
        }
        b
    }
}

/// A record as stored: length (with its closing zero) and type 0.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() as u32 + 1) & 0x00ff_ffff)
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(body);
    v.push(0);
    v
}

/// Altium's line width steps.
fn width_code(w: f64) -> i64 {
    if w <= 0.0 {
        0
    } else if w <= 15.0 {
        1
    } else if w <= 40.0 {
        2
    } else {
        3
    }
}

fn close(a: Pt, b: Pt) -> bool {
    (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
}

fn same_pts(a: &[Pt], b: &[Pt]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| close(*p, *q))
}

fn same_geom(a: &SchGeom, b: &SchGeom) -> bool {
    match (a, b) {
        (SchGeom::Line(x), SchGeom::Line(y)) | (SchGeom::Poly(x), SchGeom::Poly(y)) => {
            same_pts(x, y)
        }
        (
            SchGeom::Rect { min, max, r },
            SchGeom::Rect {
                min: m2,
                max: x2,
                r: r2,
            },
        ) => close(*min, *m2) && close(*max, *x2) && close(*r, *r2),
        (
            SchGeom::Arc { c, r, start, end },
            SchGeom::Arc {
                c: c2,
                r: r2,
                start: s2,
                end: e2,
            },
        ) => {
            close(*c, *c2) && close(*r, *r2) && (start - s2).abs() < 1e-3 && (end - e2).abs() < 1e-3
        }
        _ => false,
    }
}

fn key(p: Pt) -> (i64, i64) {
    (p[0].round() as i64, p[1].round() as i64)
}

fn anchor(g: &SchGeom) -> Pt {
    match g {
        SchGeom::Line(v) | SchGeom::Poly(v) => v.first().copied().unwrap_or([0.0, 0.0]),
        SchGeom::Rect { min, .. } => *min,
        SchGeom::Arc { c, .. } => *c,
    }
}

/// The sheet's shapes and texts, and which are already spoken for.
struct Pool<'a> {
    d: &'a Drawing,
    shape_used: Vec<bool>,
    text_used: Vec<bool>,
    shapes_at: HashMap<(i64, i64), Vec<usize>>,
    texts_at: HashMap<(i64, i64), Vec<usize>>,
}

impl<'a> Pool<'a> {
    fn new(d: &'a Drawing) -> Pool<'a> {
        let mut shapes_at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, s) in d.shapes.iter().enumerate() {
            shapes_at.entry(key(anchor(&s.geom))).or_default().push(i);
        }
        let mut texts_at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, t) in d.texts.iter().enumerate() {
            texts_at.entry(key(t.at)).or_default().push(i);
        }
        Pool {
            d,
            shape_used: vec![false; d.shapes.len()],
            text_used: vec![false; d.texts.len()],
            shapes_at,
            texts_at,
        }
    }

    fn near<'b>(
        map: &'b HashMap<(i64, i64), Vec<usize>>,
        p: Pt,
    ) -> impl Iterator<Item = usize> + 'b {
        let (x, y) = key(p);
        (-1..=1).flat_map(move |dx| {
            (-1..=1).flat_map(move |dy| map.get(&(x + dx, y + dy)).into_iter().flatten().copied())
        })
    }

    fn find_shape(&self, g: &SchShape, taken: &[usize]) -> Option<usize> {
        Self::near(&self.shapes_at, anchor(&g.geom)).find(|&i| {
            let s = &self.d.shapes[i];
            !self.shape_used[i]
                && !taken.contains(&i)
                && s.part == g.part
                && (s.width - g.width).abs() < 0.01
                && s.fill.is_some() == g.fill.is_some()
                && same_geom(&s.geom, &g.geom)
        })
    }

    fn find_text(&self, g: &SchText, taken: &[usize]) -> Option<usize> {
        Self::near(&self.texts_at, g.at).find(|&i| {
            let t = &self.d.texts[i];
            !self.text_used[i]
                && !taken.contains(&i)
                && t.part == g.part
                && t.text == g.text
                && t.rot == g.rot
                && t.just == g.just
                && close(t.at, g.at)
        })
    }

    /// The sheet's shapes and texts that `g` draws, if it draws nothing
    /// that is not there.
    fn find(&self, g: &Drawing) -> Option<Found> {
        let mut s = Vec::new();
        for sh in &g.shapes {
            s.push(self.find_shape(sh, &s)?);
        }
        let mut t = Vec::new();
        for tx in &g.texts {
            t.push(self.find_text(tx, &t)?);
        }
        Some((s, t))
    }

    fn take(&mut self, found: &Found) {
        for &i in &found.0 {
            self.shape_used[i] = true;
        }
        for &i in &found.1 {
            self.text_used[i] = true;
        }
    }
}

/// Font table: Altium font ids from 1.
struct Fonts {
    list: Vec<(i64, bool, bool, String)>,
}

impl Fonts {
    fn id(&mut self, t: &SchText) -> usize {
        let size = (t.size / FONT_SCALE).round().max(1.0) as i64;
        if let Some(i) = self
            .list
            .iter()
            .position(|f| f.0 == size && f.1 == t.bold && f.2 == t.italic)
        {
            return i + 1;
        }
        self.list
            .push((size, t.bold, t.italic, "Times New Roman".into()));
        self.list.len()
    }
}

/// Draws one record with the reader's own code.
fn draw(part: Option<u32>, f: impl FnOnce(&mut Builder, usize)) -> Drawing {
    let recs: Vec<Props> = Vec::new();
    let mut b = Builder {
        recs: &recs,
        d: Drawing::default(),
        fonts: Vec::new(),
        part_of: vec![part],
        shown: HashMap::new(),
        doc_params: HashMap::new(),
        comp_params: HashMap::new(),
        file_name: String::new(),
    };
    f(&mut b, 0);
    b.d
}

/// What an existing `.SchDoc` adds: its records, by kind.
struct Original {
    recs: Vec<Props>,
    /// (designator, part id) -> component record index.
    parts: HashMap<(String, i64), usize>,
    children: HashMap<usize, Vec<usize>>,
    /// Records of the `Additional` stream (blankets).
    additional: Vec<Props>,
    file_name: String,
    /// The records every drawing of a single record starts from: the
    /// file header, the sheet (without its border) and its parameters.
    base: Vec<SchRecord>,
}

impl Original {
    fn read(path: &Path) -> Result<Original> {
        let mut c = Container::open(path)?;
        let data = c
            .stream("/FileHeader")?
            .ok_or_else(|| Error::MissingStream("FileHeader".into()))?;
        let recs = to_props(&split_sch(&data));
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for (k, p) in recs.iter().enumerate() {
            if let Some(o) = p.int("OWNERINDEX") {
                let o = (o + 1) as usize;
                if o < recs.len() && o != k {
                    children.entry(o).or_default().push(k);
                }
            }
        }
        let mut parts = HashMap::new();
        for (k, p) in recs.iter().enumerate() {
            if p.int("RECORD") != Some(COMPONENT) {
                continue;
            }
            let des = children
                .get(&k)
                .into_iter()
                .flatten()
                .find(|&&c| recs[c].int("RECORD") == Some(DESIGNATOR))
                .map(|&c| recs[c].str("TEXT"))
                .unwrap_or_default();
            parts.insert(
                (
                    des.to_ascii_uppercase(),
                    p.int("CURRENTPARTID").unwrap_or(1),
                ),
                k,
            );
        }
        let additional = match c.stream("/Additional")? {
            Some(a) => to_props(&split_sch(&a)),
            None => Vec::new(),
        };
        let mut o = Original {
            recs,
            parts,
            children,
            additional,
            file_name: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            base: Vec::new(),
        };
        let mut base = vec![SchRecord::Text(Props::parse(
            "|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0",
        ))];
        let mut head = o
            .sheet()
            .map(Rec::from_props)
            .unwrap_or_else(|| Rec::new(SHEET));
        head.set("BORDERON", "F");
        base.push(SchRecord::Text(head.props()));
        base.extend(
            o.sheet_params()
                .into_iter()
                .map(|p| SchRecord::Text(p.clone())),
        );
        o.base = base;
        Ok(o)
    }

    /// The record that owns `k`, if any.
    fn owner_of(&self, k: usize) -> Option<usize> {
        let o = (self.recs[k].int("OWNERINDEX")? + 1) as usize;
        (o < self.recs.len() && o != k).then_some(o)
    }

    /// The sheet's own parameters (title block values).
    fn sheet_params(&self) -> Vec<&Props> {
        (0..self.recs.len())
            .filter(|&k| {
                self.recs[k].int("RECORD") == Some(PARAMETER) && self.owner_of(k).is_none()
            })
            .map(|k| &self.recs[k])
            .collect()
    }

    /// Record `k` and everything under it, moved by `shift`, numbered as
    /// if `k` were written at body index `base`.
    fn tree(&self, k: usize, shift: Pt, base: usize) -> Vec<Rec> {
        let mut ks = vec![k];
        self.subtree(k, &mut ks);
        let pos: HashMap<usize, usize> =
            ks.iter().enumerate().map(|(i, &r)| (r, base + i)).collect();
        ks.iter()
            .map(|&r| {
                let mut rec = moved(&self.recs[r], shift);
                if r == k {
                    rec.remove("OWNERINDEX");
                } else if let Some(&n) = self.owner_of(r).and_then(|o| pos.get(&o)) {
                    rec.owner(n);
                }
                rec
            })
            .collect()
    }

    /// What `tree` draws when read on its own, on this file's sheet.
    fn draw(&self, tree: &[Rec]) -> Sheet {
        let mut records = self.base.clone();
        for r in tree {
            records.push(SchRecord::Text(r.props()));
        }
        read_records(&records, String::new(), &self.file_name)
    }

    fn sheet(&self) -> Option<&Props> {
        self.recs.iter().find(|p| p.int("RECORD") == Some(SHEET))
    }

    /// Every record under `k`, depth first.
    fn subtree(&self, k: usize, out: &mut Vec<usize>) {
        for &c in self.children.get(&k).into_iter().flatten() {
            out.push(c);
            self.subtree(c, out);
        }
    }
}

/// A short id in Altium's style, the same for the same seed.
fn unique_id(seed: u64) -> String {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    (0..8)
        .map(|_| {
            x ^= x >> 33;
            x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            (b'A' + (x % 26) as u8) as char
        })
        .collect()
}

/// `body` sorted into the original file's order: each record goes where
/// the run it belongs to (`marks`: from body index, original index) was in
/// the file, records owned by another go with their owner, and new ones
/// go last in the order they were made. Owner indices follow.
fn in_order(body: Vec<Rec>, marks: &[(usize, Option<usize>)]) -> Vec<Rec> {
    let n = body.len();
    let mut key = vec![(0usize, 0usize); n];
    let mut m = 0;
    for i in 0..n {
        while m + 1 < marks.len() && marks[m + 1].0 <= i {
            m += 1;
        }
        let own = match marks[m].1 {
            Some(k) => (k, 0),
            None => (usize::MAX, i),
        };
        let owner = body[i]
            .get("OWNERINDEX")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&o| o < i);
        key[i] = owner.map_or(own, |o| key[o]);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (key[i], i));
    let mut at = vec![0; n];
    for (new, &old) in order.iter().enumerate() {
        at[old] = new;
    }
    let mut slots: Vec<Option<Rec>> = body.into_iter().map(Some).collect();
    order
        .iter()
        .map(|&old| {
            let mut r = slots[old].take().unwrap_or_default();
            if let Some(o) = r.get("OWNERINDEX").and_then(|v| v.parse::<usize>().ok()) {
                if o < n {
                    r.owner(at[o]);
                }
            }
            r
        })
        .collect()
}

/// A copy of an original record, moved by `shift`.
fn moved(p: &Props, shift: Pt) -> Rec {
    let mut r = Rec::from_props(p);
    if shift == [0.0, 0.0] {
        return r;
    }
    let mut keys: Vec<(String, f64)> = vec![
        ("LOCATION.X".into(), shift[0]),
        ("LOCATION.Y".into(), shift[1]),
        ("CORNER.X".into(), shift[0]),
        ("CORNER.Y".into(), shift[1]),
    ];
    for i in 1..=p.int("LOCATIONCOUNT").unwrap_or(0).max(0) {
        keys.push((format!("X{i}"), shift[0]));
        keys.push((format!("Y{i}"), shift[1]));
    }
    for (k, dv) in keys {
        if p.get(&k).is_some() {
            r.len(&k, len(p, &k) + dv);
        }
    }
    r
}

/// Which of the sheet's wires, junctions, net tags and sheet links a
/// record kept from the original file stands for.
struct Kept {
    wires: Vec<bool>,
    junctions: Vec<bool>,
    tags: Vec<bool>,
    links: Vec<bool>,
}

impl Kept {
    fn new(d: &Drawing) -> Kept {
        Kept {
            wires: vec![false; d.wires.len()],
            junctions: vec![false; d.junctions.len()],
            tags: vec![false; d.tags.len()],
            links: vec![false; d.links.len()],
        }
    }

    /// Marks what `g` holds as kept, if the sheet has all of it free.
    fn claim(&mut self, d: &Drawing, g: &Drawing) -> bool {
        fn pick<T>(
            done: &[bool],
            new: &mut Vec<usize>,
            all: &[T],
            want: impl Fn(&T) -> bool,
        ) -> bool {
            match (0..all.len()).find(|&i| !done[i] && !new.contains(&i) && want(&all[i])) {
                Some(i) => {
                    new.push(i);
                    true
                }
                None => false,
            }
        }
        let same_pts =
            |a: &[Pt], b: &[Pt]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| close(*p, *q));
        let (mut w, mut j, mut t, mut l) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for gw in &g.wires {
            if !pick(&self.wires, &mut w, &d.wires, |x| same_pts(x, gw)) {
                return false;
            }
        }
        for gj in &g.junctions {
            if !pick(&self.junctions, &mut j, &d.junctions, |x| close(*x, *gj)) {
                return false;
            }
        }
        for gt in &g.tags {
            if !pick(&self.tags, &mut t, &d.tags, |x| {
                x.kind == gt.kind && x.text == gt.text && same_pts(&x.at, &gt.at)
            }) {
                return false;
            }
        }
        for gl in &g.links {
            if !pick(&self.links, &mut l, &d.links, |x| {
                x.file == gl.file && close(x.min, gl.min) && close(x.max, gl.max)
            }) {
                return false;
            }
        }
        for i in w {
            self.wires[i] = true;
        }
        for i in j {
            self.junctions[i] = true;
        }
        for i in t {
            self.tags[i] = true;
        }
        for i in l {
            self.links[i] = true;
        }
        true
    }
}

fn image_name(data: &[u8], n: usize) -> String {
    let ext = if data.starts_with(b"\x89PNG") {
        "png"
    } else if data.starts_with(b"\xff\xd8") {
        "jpg"
    } else {
        "bmp"
    };
    format!("image{n}.{ext}")
}

/// Writes `sheet` as an Altium `.SchDoc`. With `original` (the `.SchDoc`
/// it was read from) parts keep their hidden parameters, library links and
/// models, and the sheet its parameters and settings.
pub fn write_schdoc(sheet: &Sheet, original: Option<&Path>) -> Result<Vec<u8>> {
    let orig = match original {
        Some(p) => Some(Original::read(p)?),
        None => None,
    };
    let d = &sheet.drawing;
    let mut pool = Pool::new(d);
    let mut body: Vec<Rec> = Vec::new();
    let mut fonts = Fonts { list: Vec::new() };
    if let Some(s) = orig.as_ref().and_then(|o| o.sheet()) {
        for i in 1..=s.int("FONTIDCOUNT").unwrap_or(0) {
            fonts.list.push((
                s.int(&format!("SIZE{i}")).unwrap_or(10),
                s.bool(&format!("BOLD{i}")),
                s.bool(&format!("ITALIC{i}")),
                s.get(&format!("FONTNAME{i}"))
                    .unwrap_or("Times New Roman")
                    .to_string(),
            ));
        }
    }
    if fonts.list.is_empty() {
        fonts
            .list
            .push((10, false, false, "Times New Roman".into()));
    }

    // ---- the sheet record (fonts are filled in at the end) ---------------
    let mut head = match orig.as_ref().and_then(|o| o.sheet()) {
        Some(p) => Rec::from_props(p),
        None => {
            let mut r = Rec::new(SHEET);
            r.set("USEMBCS", "T")
                .set("ISBOC", "T")
                .set("HOTSPOTGRIDON", "T")
                .set("HOTSPOTGRIDSIZE", "4")
                .set("SNAPGRIDON", "T")
                .set("SNAPGRIDSIZE", "10")
                .set("VISIBLEGRIDON", "T")
                .set("VISIBLEGRIDSIZE", "10")
                .set("SYSTEMFONT", "1");
            r
        }
    };
    {
        let p = head.props();
        let mut size = if p.bool("USECUSTOMSHEET") {
            [len(&p, "CUSTOMX"), len(&p, "CUSTOMY")]
        } else {
            sheet_size(p.int("SHEETSTYLE").unwrap_or(0))
        };
        if p.int("WORKSPACEORIENTATION") == Some(1) {
            size = [size[1], size[0]];
        }
        if !close(size, d.size) {
            head.set("USECUSTOMSHEET", "T")
                .remove("WORKSPACEORIENTATION");
            head.len("CUSTOMX", d.size[0]).len("CUSTOMY", d.size[1]);
        }
        head.color("AREACOLOR", d.paper);
        // The border, if the sheet shows it.
        let mut with = head.clone();
        with.set("BORDERON", "T");
        let p = with.props();
        let mut g = Drawing {
            size: d.size,
            ..Default::default()
        };
        border(&mut g, &p);
        match pool.find(&g) {
            Some(found) => {
                if let Some(&i) = found.0.first() {
                    if let Some(c) = d.shapes[i].stroke {
                        if with.get("COLOR").is_some() || c != [0, 0, 0] {
                            with.color("COLOR", c);
                        }
                    }
                }
                pool.take(&found);
                head = with;
            }
            None => {
                head.remove("BORDERON");
                head.set("BORDERON", "F");
            }
        }
    }
    body.push(head);
    // Where each run of records came from in the original file, so they
    // can be written back in its order (which is the order they draw in).
    let mut marks: Vec<(usize, Option<usize>)> = vec![(0, Some(0))];
    // Sheet parameters (title block values), as they were.
    let mut kept = Kept::new(d);
    if let Some(o) = &orig {
        let np = o.base.len() - 2;
        for k in 0..o.recs.len() {
            if o.recs[k].int("RECORD") == Some(PARAMETER) && o.owner_of(k).is_none() {
                marks.push((body.len(), Some(k)));
                body.push(Rec::from_props(&o.recs[k]));
            }
        }
        // Every other record of the file that still draws what it drew is
        // kept as it was, with all it holds that a drawing does not.
        for k in 0..o.recs.len() {
            if o.owner_of(k).is_some() {
                continue;
            }
            match o.recs[k].int("RECORD") {
                None | Some(SHEET) | Some(COMPONENT) | Some(PARAMETER) | Some(IMAGE) => continue,
                _ => {}
            }
            let g = o.draw(&o.tree(k, [0.0, 0.0], 1 + np)).drawing;
            if g.shapes.is_empty() && g.texts.is_empty() {
                continue;
            }
            let Some(found) = pool.find(&g) else { continue };
            if !kept.claim(d, &g) {
                continue;
            }
            pool.take(&found);
            let base = body.len();
            marks.push((base, Some(k)));
            body.extend(o.tree(k, [0.0, 0.0], base));
        }
    }
    let np = orig.as_ref().map_or(0, |o| o.base.len() - 2);

    // ---- parts ------------------------------------------------------------
    let mut pins_of: Vec<Vec<usize>> = vec![Vec::new(); d.parts.len()];
    for (i, p) in d.pins.iter().enumerate() {
        if let Some(v) = pins_of.get_mut(p.part as usize) {
            v.push(i);
        }
    }
    for (pi, part) in d.parts.iter().enumerate() {
        let sym = sheet.symbols.get(part.symbol).cloned().unwrap_or_default();
        let des = if sym.designator.is_empty() {
            part.designator.clone()
        } else {
            sym.designator.clone()
        };
        let pid = Some(pi as u32);
        // The designator text, and the unit letter it shows.
        let mut unit = 1i64;
        let mut des_text = None;
        for (ti, t) in d.texts.iter().enumerate() {
            if t.part != pid || pool.text_used[ti] {
                continue;
            }
            if t.text == des {
                des_text = Some(ti);
                break;
            }
            if let Some(rest) = t.text.strip_prefix(des.as_str()) {
                let mut cs = rest.chars();
                if let (Some(c @ 'A'..='Z'), None) = (cs.next(), cs.next()) {
                    unit = (c as u8 - b'A' + 1) as i64;
                    des_text = Some(ti);
                    break;
                }
            }
        }
        let o_comp = orig.as_ref().and_then(|o| {
            o.parts
                .get(&(des.to_ascii_uppercase(), unit))
                .map(|&k| (o, k))
        });
        // Where the part moved to since the file was read: by its first pin.
        let mut shift = [0.0, 0.0];
        if let Some((o, k)) = o_comp {
            let first_old = o
                .children
                .get(&k)
                .into_iter()
                .flatten()
                .map(|&c| &o.recs[c])
                .find(|c| {
                    let u = c.int("OWNERPARTID").unwrap_or(-1);
                    c.int("RECORD") == Some(PIN)
                        && c.int("PINCONGLOMERATE").unwrap_or(0) & 4 == 0
                        && (u <= 0 || u == unit)
                        && c.int("OWNERPARTDISPLAYMODE").unwrap_or(0)
                            == o.recs[k].int("DISPLAYMODE").unwrap_or(0)
                });
            if let (Some(op), Some(&first)) = (first_old, pins_of[pi].first()) {
                let num = op.str("DESIGNATOR");
                let new = pins_of[pi]
                    .iter()
                    .map(|&i| &d.pins[i])
                    .find(|p| p.number == num)
                    .unwrap_or(&d.pins[first]);
                let ob = at(op);
                shift = [new.base[0] - ob[0], new.base[1] - ob[1]];
            }
            // A part that still draws as it did is kept whole, with its
            // other units, display modes and everything else it holds.
            let old = o.draw(&o.tree(k, shift, 1 + np));
            let mut g = old.drawing;
            for s in &mut g.shapes {
                s.part = s.part.map(|_| pi as u32);
            }
            for t in &mut g.texts {
                t.part = t.part.map(|_| pi as u32);
            }
            let same = old.symbols.first().is_some_and(|o| {
                o.designator == sym.designator
                    && o.value == sym.value
                    && o.footprint == sym.footprint
                    && o.library_ref == sym.library_ref
                    && o.description == sym.description
            });
            if same && g.pins.len() == pins_of[pi].len() {
                if let Some(found) = pool.find(&g) {
                    pool.take(&found);
                    let base = body.len();
                    marks.push((base, Some(k)));
                    body.extend(o.tree(k, shift, base));
                    continue;
                }
            }
        }
        let ci = body.len();
        marks.push((ci, o_comp.map(|(_, k)| k)));
        let mut comp = match o_comp {
            Some((o, k)) => Rec::from_props(&o.recs[k]),
            None => {
                let mut r = Rec::new(COMPONENT);
                r.set("LIBREFERENCE", sym.library_ref.clone())
                    .set("DESIGNITEMID", sym.library_ref.clone())
                    .set("DISPLAYMODECOUNT", "1")
                    .set("LIBRARYPATH", "*")
                    .set("SOURCELIBRARYNAME", "*")
                    .set("TARGETFILENAME", "*")
                    .set("AREACOLOR", "11599871")
                    .set("COLOR", "128")
                    .set("PARTIDLOCKED", "F")
                    .set("UNIQUEID", unique_id(pi as u64 + 1));
                r
            }
        };
        if !sym.description.is_empty() {
            comp.set("COMPONENTDESCRIPTION", sym.description.clone());
        }
        comp.set("OWNERPARTID", "-1")
            .set("CURRENTPARTID", unit.to_string())
            .set("DISPLAYMODE", "0");
        let count = comp.props().int("PARTCOUNT").unwrap_or(2).max(
            if des_text.is_some_and(|t| d.texts[t].text != des) {
                (unit + 1).max(3)
            } else {
                2
            },
        );
        comp.set("PARTCOUNT", count.to_string());
        comp.set("ALLPINCOUNT", pins_of[pi].len().to_string());
        if let Some((o, k)) = o_comp {
            let oa = at(&o.recs[k]);
            comp.at([oa[0] + shift[0], oa[1] + shift[1]]);
        } else {
            let a = pins_of[pi]
                .first()
                .map(|&i| d.pins[i].at)
                .unwrap_or(part.min);
            comp.at(a);
        }
        body.push(comp);

        // Designator.
        let mut drec = Rec::new(DESIGNATOR);
        drec.owner(ci)
            .set("OWNERPARTID", "-1")
            .set("NAME", "Designator")
            .set("TEXT", des.clone())
            .set("READONLYSTATE", "1");
        match des_text {
            Some(ti) => {
                let t = &d.texts[ti];
                text_fields(&mut drec, t, &mut fonts);
                pool.text_used[ti] = true;
            }
            None => {
                drec.at(part.min).set("ISHIDDEN", "T");
            }
        }
        body.push(drec);

        // Parameters: visible ones from the part's texts, hidden ones from
        // the file it came from.
        let mut named: Vec<String> = Vec::new();
        let old_visible: Vec<&Props> = match o_comp {
            Some((o, k)) => o
                .children
                .get(&k)
                .into_iter()
                .flatten()
                .map(|&c| &o.recs[c])
                .filter(|c| c.int("RECORD") == Some(PARAMETER) && !c.bool("ISHIDDEN"))
                .collect(),
            None => Vec::new(),
        };
        for ti in 0..d.texts.len() {
            let t = &d.texts[ti];
            if t.part != pid || pool.text_used[ti] {
                continue;
            }
            let from_old = old_visible.iter().find(|c| {
                let v = c.str("TEXT");
                !named.contains(&c.str("NAME").to_ascii_uppercase())
                    && (v == t.text
                        || (c.bool("SHOWNAME") && t.text == format!("{}: {v}", c.str("NAME"))))
            });
            let (name, show_name, value) = match from_old {
                Some(c) => (c.str("NAME"), c.bool("SHOWNAME"), c.str("TEXT")),
                None if Some(&t.text) == sym.value.as_ref()
                    && !named.contains(&"COMMENT".to_string()) =>
                {
                    ("Comment".to_string(), false, t.text.clone())
                }
                None => continue,
            };
            let mut r = Rec::new(PARAMETER);
            r.owner(ci)
                .set("OWNERPARTID", "-1")
                .set("NAME", name.clone())
                .set("TEXT", value)
                .flag("SHOWNAME", show_name);
            text_fields(&mut r, t, &mut fonts);
            named.push(name.to_ascii_uppercase());
            pool.text_used[ti] = true;
            body.push(r);
        }
        let mut models = false;
        if let Some((o, k)) = o_comp {
            // Hidden parameters and the model list, moved with the part.
            let mut sub = Vec::new();
            for &c in o.children.get(&k).into_iter().flatten() {
                let p = &o.recs[c];
                let kind = p.int("RECORD");
                let hidden_param = kind == Some(PARAMETER)
                    && (p.bool("ISHIDDEN") || p.get("LOCATION.X").is_none())
                    && !named.contains(&p.str("NAME").to_ascii_uppercase());
                let unit_of = p.int("OWNERPARTID").unwrap_or(-1);
                let mode = p.int("OWNERPARTDISPLAYMODE").unwrap_or(0);
                let other = (unit_of > 0 && unit_of != unit)
                    || mode != o.recs[k].int("DISPLAYMODE").unwrap_or(0);
                if hidden_param || kind == Some(44) || other {
                    sub.push(c);
                    if kind == Some(44) {
                        models = true;
                        o.subtree(c, &mut sub);
                    }
                }
            }
            let mut map: HashMap<usize, usize> = HashMap::new();
            map.insert(k, ci);
            for c in sub {
                let p = &o.recs[c];
                let mut r = moved(p, shift);
                // What the sheet changed since: the value and the footprint.
                if p.int("RECORD") == Some(PARAMETER) && p.str("NAME").eq_ignore_ascii_case("VALUE")
                {
                    if let Some(v) = &sym.value {
                        r.set("TEXT", v.clone());
                    }
                }
                if p.int("RECORD") == Some(IMPLEMENTATION)
                    && p.str("MODELTYPE").eq_ignore_ascii_case("PCBLIB")
                    && p.bool("ISCURRENT")
                    && !sym.footprint.is_empty()
                    && p.str("MODELNAME") != sym.footprint
                {
                    r.set("MODELNAME", sym.footprint.clone());
                    if p.get("MODELDATAFILEENTITY0").is_some() {
                        r.set("MODELDATAFILEENTITY0", sym.footprint.clone());
                    }
                }
                let owner = (p.int("OWNERINDEX").unwrap_or(-1) + 1) as usize;
                match map.get(&owner) {
                    Some(&n) => {
                        r.owner(n);
                    }
                    None => continue,
                }
                if p.int("RECORD") == Some(PARAMETER) {
                    named.push(p.str("NAME").to_ascii_uppercase());
                }
                map.insert(c, body.len());
                body.push(r);
            }
        }
        for (name, v) in [
            (
                "Comment",
                sym.value.clone().unwrap_or_else(|| sym.library_ref.clone()),
            ),
            ("Value", sym.value.clone().unwrap_or_default()),
        ] {
            if v.is_empty() || named.contains(&name.to_ascii_uppercase()) {
                continue;
            }
            let mut r = Rec::new(PARAMETER);
            r.owner(ci)
                .set("OWNERPARTID", "-1")
                .at(part.min)
                .set("ISHIDDEN", "T")
                .set("NAME", name)
                .set("TEXT", v);
            body.push(r);
        }
        if !models && !sym.footprint.is_empty() {
            let li = body.len();
            let mut l = Rec::new(44);
            l.owner(ci);
            body.push(l);
            let mi = body.len();
            let mut m = Rec::new(IMPLEMENTATION);
            m.owner(li)
                .set("USECOMPONENTLIBRARY", "T")
                .set("MODELNAME", sym.footprint.clone())
                .set("MODELTYPE", "PCBLIB")
                .set("DATAFILECOUNT", "1")
                .set("MODELDATAFILEENTITY0", sym.footprint.clone())
                .set("MODELDATAFILEKIND0", "PCBLib")
                .set("ISCURRENT", "T")
                .set("UNIQUEID", unique_id(1000 + pi as u64));
            body.push(m);
            for kind in [46, 48] {
                let mut r = Rec::new(kind);
                r.owner(mi);
                body.push(r);
            }
        }

        // Pins: the record that draws each pin's line, name, number and edge
        // marks as found on the sheet.
        for &i in &pins_of[pi] {
            let p = &d.pins[i];
            let v = [p.at[0] - p.base[0], p.at[1] - p.base[1]];
            let length = v[0].hypot(v[1]);
            let orient = if v[0].abs() >= v[1].abs() {
                if v[0] >= 0.0 {
                    0
                } else {
                    2
                }
            } else if v[1] >= 0.0 {
                1
            } else {
                3
            };
            let mut best: Option<(Rec, Found)> = None;
            for combo in 0..16u8 {
                let mut r = Rec::new(PIN);
                r.owner(ci)
                    .set("OWNERPARTID", unit.to_string())
                    .at(p.base)
                    .len("PINLENGTH", length)
                    .set("NAME", p.name.clone())
                    .set("DESIGNATOR", p.number.clone())
                    .set("ELECTRICAL", "4");
                let mut flags = orient;
                if combo & 1 != 0 {
                    flags |= 0x08;
                }
                if combo & 2 != 0 {
                    flags |= 0x10;
                }
                r.set("PINCONGLOMERATE", flags.to_string());
                if combo & 4 != 0 {
                    r.set("SYMBOL_OUTEREDGE", "1");
                }
                if combo & 8 != 0 {
                    r.set("SYMBOL_INNEREDGE", "3");
                }
                let props = r.props();
                let g = draw(pid, |b, k| pin(b, k, &props));
                if let Some(found) = pool.find(&g) {
                    let n = found.0.len() + found.1.len();
                    if best
                        .as_ref()
                        .is_none_or(|b| b.1 .0.len() + b.1 .1.len() < n)
                    {
                        best = Some((r, found));
                    }
                }
            }
            let rec = match best {
                Some((mut r, found)) => {
                    if let Some(c) = found.0.first().and_then(|&s| d.shapes[s].stroke) {
                        r.color("COLOR", c);
                    }
                    pool.take(&found);
                    r
                }
                None => {
                    let mut r = Rec::new(PIN);
                    r.owner(ci)
                        .set("OWNERPARTID", unit.to_string())
                        .at(p.base)
                        .len("PINLENGTH", length)
                        .set("NAME", p.name.clone())
                        .set("DESIGNATOR", p.number.clone())
                        .set("ELECTRICAL", "4")
                        .set("PINCONGLOMERATE", (orient | 0x18).to_string());
                    r
                }
            };
            body.push(rec);
        }
        // The part's own graphics are written further down, once every
        // net tag and junction has taken what it draws.
        let _ = ci;
    }
    let comp_index: Vec<usize> = {
        let mut v = Vec::new();
        for (k, r) in body.iter().enumerate() {
            if r.get("RECORD") == Some("1") {
                v.push(k);
            }
        }
        v
    };

    marks.push((body.len(), None));

    // ---- wiring -----------------------------------------------------------
    for (wi, w) in d.wires.iter().enumerate() {
        if kept.wires[wi] {
            continue;
        }
        let mut r = Rec::new(WIRE);
        r.set("OWNERPARTID", "-1").points(w);
        let found = d
            .shapes
            .iter()
            .enumerate()
            .find(|(i, s)| s.wire == Some(wi as u32) && !pool.shape_used[*i]);
        match found {
            Some((i, s)) => {
                r.set("LINEWIDTH", width_code(s.width).max(1).to_string());
                if let Some(c) = s.stroke {
                    r.color("COLOR", c);
                }
                pool.shape_used[i] = true;
            }
            None => {
                r.set("LINEWIDTH", "1").set("COLOR", "8388608");
            }
        }
        body.push(r);
    }
    for (ji, &j) in d.junctions.iter().enumerate() {
        if kept.junctions[ji] {
            continue;
        }
        let mut r = Rec::new(JUNCTION);
        r.set("OWNERPARTID", "-1").at(j);
        let g = draw(None, |b, _| {
            b.d.shapes.push(SchShape {
                geom: SchGeom::Arc {
                    c: j,
                    r: [20.0, 20.0],
                    start: 0.0,
                    end: 360.0,
                },
                width: 0.0,
                stroke: None,
                fill: Some([0, 0, 0]),
                dashed: false,
                part: None,
                wire: None,
            })
        });
        if let Some(found) = pool.find(&g) {
            if let Some(c) = d.shapes[found.0[0]].fill {
                r.color("COLOR", c);
            }
            pool.take(&found);
        }
        body.push(r);
    }

    // ---- net tags ---------------------------------------------------------
    let mut sheet_boxes: Vec<(usize, Pt, Pt)> = Vec::new();
    for (li, link) in d.links.iter().enumerate() {
        if kept.links[li] {
            continue;
        }
        let mut r = Rec::new(SHEET_SYMBOL);
        r.set("OWNERPARTID", "-1")
            .at([link.min[0], link.max[1]])
            .len("XSIZE", link.max[0] - link.min[0])
            .len("YSIZE", link.max[1] - link.min[1])
            .set("ISSOLID", "T")
            .set("UNIQUEID", unique_id(5000 + body.len() as u64));
        let props = r.props();
        let (mn, mx) = (link.min, link.max);
        let g = draw(None, |b, k| {
            b.shape(k, rect(mn, mx, [0.0, 0.0]), &props, true)
        });
        if let Some(found) = pool.find(&g) {
            let s = &d.shapes[found.0[0]];
            if let Some(c) = s.stroke {
                r.color("COLOR", c);
            }
            match s.fill {
                Some(c) => {
                    r.color("AREACOLOR", c);
                }
                None => {
                    r.remove("ISSOLID");
                }
            }
            r.set("LINEWIDTH", width_code(s.width).to_string());
            pool.take(&found);
        }
        let si = body.len();
        body.push(r);
        sheet_boxes.push((si, link.min, link.max));
        // Its file name, and its name: the free texts above the box.
        let mut fname = Rec::new(SHEET_FILE_NAME);
        fname
            .owner(si)
            .set("OWNERPARTID", "-1")
            .set("TEXT", link.file.clone());
        let ft = (0..d.texts.len()).find(|&i| {
            !pool.text_used[i] && d.texts[i].part.is_none() && d.texts[i].text == link.file
        });
        match ft {
            Some(i) => {
                text_fields(&mut fname, &d.texts[i], &mut fonts);
                pool.text_used[i] = true;
            }
            None => {
                fname
                    .at([link.min[0], link.min[1] - 100.0])
                    .set("ISHIDDEN", "T");
            }
        }
        let nt = (0..d.texts.len()).find(|&i| {
            let t = &d.texts[i];
            !pool.text_used[i]
                && t.part.is_none()
                && t.at[0] >= link.min[0] - 1.0
                && t.at[0] <= link.max[0]
                && t.at[1] >= link.max[1]
                && t.at[1] <= link.max[1] + 200.0
        });
        let mut sname = Rec::new(SHEET_NAME);
        sname.owner(si).set("OWNERPARTID", "-1");
        match nt {
            Some(i) => {
                sname.set("TEXT", d.texts[i].text.clone());
                text_fields(&mut sname, &d.texts[i], &mut fonts);
                pool.text_used[i] = true;
            }
            None => {
                let stem = link
                    .file
                    .rsplit_once('.')
                    .map(|s| s.0)
                    .unwrap_or(&link.file);
                sname
                    .set("TEXT", stem.to_string())
                    .at([link.min[0], link.max[1] + 50.0])
                    .set("ISHIDDEN", "T");
            }
        }
        body.push(sname);
        body.push(fname);
    }
    for (ti, tag) in d.tags.iter().enumerate() {
        if kept.tags[ti] {
            continue;
        }
        let Some(&loc) = tag.at.first() else { continue };
        match tag.kind {
            TagKind::Label => {
                let mut r = Rec::new(NET_LABEL);
                r.set("OWNERPARTID", "-1")
                    .at(loc)
                    .set("TEXT", tag.text.clone());
                match tag
                    .graphic
                    .map(|g| g as usize)
                    .filter(|&g| g < d.texts.len())
                {
                    Some(g) => {
                        let t = &d.texts[g];
                        r.set("ORIENTATION", t.rot.to_string())
                            .set("JUSTIFICATION", t.just.to_string());
                        let f = fonts.id(t);
                        r.set("FONTID", f.to_string()).color("COLOR", t.color);
                        pool.text_used[g] = true;
                    }
                    None => {
                        r.set("COLOR", "128");
                    }
                }
                body.push(r);
            }
            TagKind::Power => body.push(power_rec(&mut pool, &mut fonts, tag, loc)),
            TagKind::Port => body.push(port_rec(&mut pool, &mut fonts, tag)),
            TagKind::SheetEntry => {
                let near = |a: f64, b: f64| (a - b).abs() < 0.01;
                let within = |v: f64, lo: f64, hi: f64| v >= lo - 0.01 && v <= hi + 0.01;
                let Some(&(si, min, max)) = sheet_boxes.iter().find(|(_, mn, mx)| {
                    ((near(loc[0], mn[0]) || near(loc[0], mx[0])) && within(loc[1], mn[1], mx[1]))
                        || ((near(loc[1], mn[1]) || near(loc[1], mx[1]))
                            && within(loc[0], mn[0], mx[0]))
                }) else {
                    continue;
                };
                let (side, dist) = if (loc[0] - min[0]).abs() < 0.01 {
                    (0, max[1] - loc[1])
                } else if (loc[0] - max[0]).abs() < 0.01 {
                    (1, max[1] - loc[1])
                } else if (loc[1] - max[1]).abs() < 0.01 {
                    (2, loc[0] - min[0])
                } else {
                    (3, loc[0] - min[0])
                };
                let units = (dist / 100.0).floor();
                let frac = ((dist - units * 100.0) * 10_000.0).round() as i64;
                let mut r = Rec::new(SHEET_ENTRY);
                r.owner(si)
                    .set("OWNERPARTID", "-1")
                    .set("SIDE", side.to_string())
                    .set("DISTANCEFROMTOP", (units as i64).to_string())
                    .set("NAME", tag.text.clone())
                    .set("IOTYPE", "0")
                    .set("STYLE", "0")
                    .set("ARROWKIND", "Block & Triangle");
                if frac != 0 {
                    r.set("DISTANCEFROMTOP_FRAC1", frac.to_string());
                }
                // Draw it as the reader would, to find its glyph and text.
                for id in candidate_fonts(&pool, &tag.text, &mut fonts) {
                    let mut c = r.clone();
                    c.set("TEXTFONTID", id.to_string());
                    let props = c.props();
                    let g = draw(None, |b, k| sheet_entry(b, k, &props, min, max));
                    if let Some(found) = pool.find(&g) {
                        if let Some(s) = found.0.first().map(|&i| &d.shapes[i]) {
                            if let Some(col) = s.stroke {
                                c.color("COLOR", col);
                            }
                            if let Some(col) = s.fill {
                                c.color("AREACOLOR", col);
                            }
                        }
                        if let Some(&t) = found.1.first() {
                            c.color("TEXTCOLOR", d.texts[t].color);
                        }
                        pool.take(&found);
                        r = c;
                        break;
                    }
                }
                body.push(r);
            }
        }
    }

    // Blankets, kept in the side stream, where they still are.
    let mut extra: Vec<Rec> = Vec::new();
    if let Some(o) = &orig {
        for p in &o.additional {
            if p.int("RECORD") != Some(BLANKET) {
                continue;
            }
            let g = Drawing {
                shapes: vec![blanket(p)],
                ..Default::default()
            };
            if let Some(found) = pool.find(&g) {
                pool.take(&found);
                extra.push(Rec::from_props(p));
            }
        }
    }

    // ---- what is left: graphics and text ----------------------------------
    // Text frames first, so each claims the box it is drawn in.
    let mut frames: Vec<(usize, Rec)> = Vec::new();
    for (i, t) in d.texts.iter().enumerate() {
        if !pool.text_used[i] && (t.wrap.is_some() || t.text.contains('\n')) {
            frames.push((i, frame_rec(&mut pool, t, &mut fonts)));
            pool.text_used[i] = true;
        }
    }
    // Part graphics go under their component.
    let mut owned: Vec<Rec> = Vec::new();
    for (i, s) in d.shapes.iter().enumerate() {
        if pool.shape_used[i] {
            continue;
        }
        let (owner, unit) = match s.part.and_then(|p| comp_index.get(p as usize)) {
            Some(&c) => (
                Some(c),
                body[c].get("CURRENTPARTID").unwrap_or("1").to_string(),
            ),
            None => (None, "-1".to_string()),
        };
        let mut r = shape_rec(s);
        r.set("OWNERPARTID", unit);
        if let Some(c) = owner {
            r.owner(c).set("ISNOTACCESIBLE", "T");
        }
        owned.push(r);
    }
    let mut frames = frames.into_iter().peekable();
    for (i, t) in d.texts.iter().enumerate() {
        let framed = frames.next_if(|(f, _)| *f == i).map(|(_, r)| r);
        if pool.text_used[i] && framed.is_none() {
            continue;
        }
        let (owner, unit) = match t.part.and_then(|p| comp_index.get(p as usize)) {
            Some(&c) => (
                Some(c),
                body[c].get("CURRENTPARTID").unwrap_or("1").to_string(),
            ),
            None => (None, "-1".to_string()),
        };
        let mut r = if let Some(r) = framed {
            r
        } else {
            let mut r = Rec::new(LABEL);
            r.set("TEXT", t.text.clone());
            text_fields(&mut r, t, &mut fonts);
            r
        };
        r.set("OWNERPARTID", unit);
        if let Some(c) = owner {
            r.owner(c);
        }
        owned.push(r);
    }
    // Pictures, kept in the Storage stream.
    let mut store: Vec<(String, &[u8])> = Vec::new();
    for (n, im) in d.images.iter().enumerate() {
        // A picture kept from the original keeps its name.
        let kept_as = body.iter().find_map(|r| {
            if r.get("RECORD") != Some("30") {
                return None;
            }
            let p = r.props();
            let (a, c) = (at(&p), corner(&p));
            let (mn, mx) = (
                [a[0].min(c[0]), a[1].min(c[1])],
                [a[0].max(c[0]), a[1].max(c[1])],
            );
            let name = p.str("FILENAME");
            (close(mn, im.min) && close(mx, im.max) && !store.iter().any(|(s, _)| *s == name))
                .then_some(name)
        });
        if let Some(name) = kept_as {
            store.push((name, im.data.as_slice()));
            continue;
        }
        let name = image_name(&im.data, n + 1);
        let mut r = Rec::new(IMAGE);
        r.set("OWNERPARTID", "-1")
            .at(im.min)
            .corner(im.max)
            .set("EMBEDIMAGE", "T")
            .flag("KEEPASPECT", im.keep_aspect)
            .set("FILENAME", name.clone());
        owned.push(r);
        store.push((name, im.data.as_slice()));
    }
    body.extend(owned);

    // ---- the original order, then fonts and the file -------------------------
    let mut body = in_order(body, &marks);

    let head = &mut body[0];
    let old: Vec<String> = head
        .0
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| {
            // Rotation, underline and strike-out of the file's own fonts stay.
            ["SIZE", "BOLD", "ITALIC", "FONTNAME"]
                .iter()
                .any(|f| k.strip_prefix(f).is_some_and(|n| n.parse::<u32>().is_ok()))
        })
        .collect();
    for k in old {
        head.remove(&k);
    }
    head.set("FONTIDCOUNT", fonts.list.len().to_string());
    for (i, f) in fonts.list.iter().enumerate() {
        let n = i + 1;
        head.set(&format!("SIZE{n}"), f.0.to_string());
        if f.1 {
            head.set(&format!("BOLD{n}"), "T");
        }
        if f.2 {
            head.set(&format!("ITALIC{n}"), "T");
        }
        head.set(&format!("FONTNAME{n}"), f.3.clone());
    }

    let mut stream = framed(
        format!(
            "|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0|WEIGHT={}|MINORVERSION=10|UNIQUEID={}",
            body.len(),
            unique_id(sheet.name.len() as u64 + body.len() as u64)
        )
        .as_bytes(),
    );
    for r in &body {
        stream.extend(framed(&r.bytes()));
    }
    let mut storage = framed(format!("|HEADER=Icon storage|WEIGHT={}", store.len()).as_bytes());
    for (name, data) in &store {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(data)?;
        let z = z.finish()?;
        let mut e = vec![0xD0u8, name.len().min(255) as u8];
        e.extend(name.bytes().take(255));
        e.extend((z.len() as u32).to_le_bytes());
        e.extend(&z);
        storage.extend(((e.len() as u32) | 0x0100_0000).to_le_bytes());
        storage.extend(e);
    }
    let mut additional =
        framed(b"|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0");
    for r in &extra {
        additional.extend(framed(&r.bytes()));
    }

    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
    cf.create_stream("/FileHeader")?.write_all(&stream)?;
    cf.create_stream("/Storage")?.write_all(&storage)?;
    cf.create_stream("/Additional")?.write_all(&additional)?;
    cf.flush()?;
    Ok(cf.into_inner().into_inner())
}

/// Location, orientation, justification, font and colour of a text.
fn text_fields(r: &mut Rec, t: &SchText, fonts: &mut Fonts) {
    r.at(t.at)
        .set("ORIENTATION", t.rot.to_string())
        .set("JUSTIFICATION", t.just.to_string())
        .set("FONTID", fonts.id(t).to_string())
        .color("COLOR", t.color);
}

/// Font ids that may draw a text: those of unused texts with these words.
fn candidate_fonts(pool: &Pool, text: &str, fonts: &mut Fonts) -> Vec<usize> {
    let mut ids: Vec<usize> = Vec::new();
    for (i, t) in pool.d.texts.iter().enumerate() {
        if !pool.text_used[i] && t.text == text {
            let id = fonts.id(t);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if ids.is_empty() {
        ids.push(1);
    }
    ids
}

fn power_rec(pool: &mut Pool, fonts: &mut Fonts, tag: &trace_core::NetTag, loc: Pt) -> Rec {
    let d = pool.d;
    let mut base = Rec::new(POWER_PORT);
    base.set("OWNERPARTID", "-1")
        .at(loc)
        .set("TEXT", tag.text.clone())
        .set(
            "UNIQUEID",
            unique_id(key(loc).0 as u64 * 31 + key(loc).1 as u64),
        );
    let ids = candidate_fonts(pool, &tag.text, fonts);
    let mut best: Option<(Rec, Found)> = None;
    for show in [true, false] {
        for style in 0..=10 {
            for orient in 0..4 {
                for &id in if show { &ids[..] } else { &ids[..1] } {
                    let mut r = base.clone();
                    r.set("STYLE", style.to_string())
                        .set("ORIENTATION", orient.to_string())
                        .set("SHOWNETNAME", if show { "T" } else { "F" })
                        .set("FONTID", id.to_string());
                    let props = r.props();
                    let g = draw(None, |b, k| power_port(b, k, &props));
                    if let Some(found) = pool.find(&g) {
                        let n = found.0.len() + found.1.len();
                        if best
                            .as_ref()
                            .is_none_or(|b| b.1 .0.len() + b.1 .1.len() < n)
                        {
                            best = Some((r, found));
                        }
                    }
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    match best {
        Some((mut r, found)) => {
            if let Some(c) = found.0.first().and_then(|&s| d.shapes[s].stroke) {
                r.color("COLOR", c);
            }
            pool.take(&found);
            r
        }
        None => {
            base.set("STYLE", "2")
                .set("SHOWNETNAME", "T")
                .set("COLOR", "128");
            base
        }
    }
}

fn port_rec(pool: &mut Pool, fonts: &mut Fonts, tag: &trace_core::NetTag) -> Rec {
    let d = pool.d;
    let (a, e) = (tag.at[0], *tag.at.get(1).unwrap_or(&tag.at[0]));
    let vertical = (a[0] - e[0]).abs() < 0.01 && (a[1] - e[1]).abs() > 0.01;
    let w = if vertical { e[1] - a[1] } else { e[0] - a[0] };
    let mut base = Rec::new(PORT);
    base.set("OWNERPARTID", "-1")
        .at(a)
        .set("NAME", tag.text.clone())
        .len("WIDTH", w)
        .set("IOTYPE", "0")
        .set(
            "UNIQUEID",
            unique_id(key(a).0 as u64 * 37 + key(a).1 as u64),
        );
    // Heights: 100 mil, or what an outline across the port says.
    let mut heights = vec![100.0];
    for s in &d.shapes {
        if let SchGeom::Poly(v) = &s.geom {
            let (lo, hi) = v.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                let t = if vertical { p[0] - a[0] } else { p[1] - a[1] };
                (lo.min(t), hi.max(t))
            });
            let along = v.iter().all(|p| {
                let s = if vertical { p[1] - a[1] } else { p[0] - a[0] };
                s > -0.01 && s < w + 0.01
            });
            if along && lo < 0.0 && hi > 0.0 && (hi + lo).abs() < 0.01 {
                let h = hi * 2.0;
                if !heights.iter().any(|x| (x - h).abs() < 0.01) {
                    heights.push(h);
                }
            }
        }
    }
    let ids = candidate_fonts(pool, &tag.text, fonts);
    for &h in &heights {
        for style in 0..4 {
            for align in 0..3 {
                for &id in &ids {
                    let mut r = base.clone();
                    r.len("HEIGHT", h)
                        .set("STYLE", (style + if vertical { 4 } else { 0 }).to_string())
                        .set("ALIGNMENT", align.to_string())
                        .set("TEXTFONTID", id.to_string());
                    let props = r.props();
                    let g = draw(None, |b, k| port(b, k, &props));
                    if let Some(found) = pool.find(&g) {
                        if let Some(s) = found.0.first().map(|&i| &d.shapes[i]) {
                            if let Some(c) = s.stroke {
                                r.color("COLOR", c);
                            }
                            if let Some(c) = s.fill {
                                r.color("AREACOLOR", c);
                            }
                        }
                        if let Some(&t) = found.1.first() {
                            r.color("TEXTCOLOR", d.texts[t].color);
                        }
                        pool.take(&found);
                        return r;
                    }
                }
            }
        }
    }
    base.set("STYLE", if vertical { "4" } else { "0" });
    base
}

/// A text frame for a wrapped or multi-line text, with the rectangle drawn
/// around it if there is one.
fn frame_rec(pool: &mut Pool, t: &SchText, fonts: &mut Fonts) -> Rec {
    let d = pool.d;
    let mut r = Rec::new(TEXT_FRAME);
    let align = match t.just {
        7 => 2,
        8 => 3,
        _ => 1,
    };
    // A rectangle the text sits in the top of, the same margin all round.
    let mut found = None;
    for (i, s) in d.shapes.iter().enumerate() {
        if pool.shape_used[i] || s.part != t.part {
            continue;
        }
        let SchGeom::Rect { min, max, r: rr } = &s.geom else {
            continue;
        };
        if rr[0] != 0.0 || rr[1] != 0.0 {
            continue;
        }
        let m = max[1] - t.at[1];
        if m < -0.01 {
            continue;
        }
        let x = match align {
            2 => (min[0] + max[0]) / 2.0,
            3 => max[0] - m,
            _ => min[0] + m,
        };
        let wrap_ok = match t.wrap {
            Some(w) => (w - (max[0] - min[0] - 2.0 * m).max(10.0)).abs() < 0.01,
            None => true,
        };
        if (x - t.at[0]).abs() < 0.01 && wrap_ok && t.at[1] >= min[1] {
            found = Some((i, *min, *max, m));
            break;
        }
    }
    let lines = t.text.lines().count().max(1) as f64;
    let (min, max, margin) = match found {
        Some((i, min, max, m)) => {
            let s = &d.shapes[i];
            match s.fill {
                Some(c) => {
                    r.set("ISSOLID", "T").color("AREACOLOR", c);
                }
                None => {
                    r.set("ISSOLID", "F");
                }
            }
            if let Some(c) = s.stroke {
                r.set("SHOWBORDER", "T").color("COLOR", c);
            }
            r.set("LINEWIDTH", width_code(s.width).to_string());
            pool.shape_used[i] = true;
            (min, max, m)
        }
        None => {
            let w = t.wrap.unwrap_or(
                t.size * 0.6 * t.text.lines().map(|l| l.len()).max().unwrap_or(1) as f64,
            );
            let h = t.size * 1.3 * lines;
            let x0 = match align {
                2 => t.at[0] - w / 2.0,
                3 => t.at[0] - w,
                _ => t.at[0],
            };
            r.set("ISSOLID", "F");
            ([x0, t.at[1] - h], [x0 + w, t.at[1]], 0.0)
        }
    };
    r.at(min)
        .corner(max)
        .len("TEXTMARGIN", margin)
        .set("ALIGNMENT", align.to_string())
        .flag("WORDWRAP", t.wrap.is_some())
        .set("CLIPTORECT", "T")
        .set("FONTID", fonts.id(t).to_string())
        .color("TEXTCOLOR", t.color)
        .set("TEXT", t.text.replace('\n', "~1"));
    r
}

/// The record for a graphic.
fn shape_rec(s: &SchShape) -> Rec {
    let mut r = match &s.geom {
        SchGeom::Line(v) if v.len() == 2 => {
            let mut r = Rec::new(LINE);
            r.at(v[0]).corner(v[1]);
            r
        }
        SchGeom::Line(v) => {
            let mut r = Rec::new(POLYLINE);
            r.points(v);
            r
        }
        SchGeom::Poly(v) => {
            let mut r = Rec::new(POLYGON);
            r.points(v);
            r
        }
        SchGeom::Rect { min, max, r: rr } if rr[0] == 0.0 && rr[1] == 0.0 => {
            let mut r = Rec::new(RECTANGLE);
            r.at(*min).corner(*max);
            r
        }
        SchGeom::Rect { min, max, r: rr } => {
            let mut r = Rec::new(ROUND_RECT);
            r.at(*min)
                .corner(*max)
                .len("CORNERXRADIUS", rr[0])
                .len("CORNERYRADIUS", rr[1]);
            r
        }
        SchGeom::Arc {
            c,
            r: rr,
            start,
            end,
        } => {
            let full = (end - start - 360.0).abs() < 1e-6 && start.abs() < 1e-6;
            let kind = if full {
                ELLIPSE
            } else if s.fill.is_some() {
                PIECHART
            } else if (rr[0] - rr[1]).abs() < 0.01 {
                ARC
            } else {
                ELLIPTICAL_ARC
            };
            let mut r = Rec::new(kind);
            r.at(*c).len("RADIUS", rr[0]);
            if kind == ELLIPSE || kind == ELLIPTICAL_ARC {
                r.len("SECONDARYRADIUS", rr[1]);
            }
            if !full {
                r.set("STARTANGLE", format!("{:.3}", start.rem_euclid(360.0)))
                    .set("ENDANGLE", format!("{:.3}", end.rem_euclid(360.0)));
            }
            r
        }
    };
    r.set("LINEWIDTH", width_code(s.width).to_string());
    match s.stroke.or(s.fill) {
        Some(c) => {
            r.color("COLOR", c);
        }
        None => {
            r.set("COLOR", "0");
        }
    }
    if let Some(c) = s.fill {
        r.set("ISSOLID", "T").color("AREACOLOR", c);
    }
    if s.dashed {
        r.set("LINESTYLE", "1");
    }
    r
}
