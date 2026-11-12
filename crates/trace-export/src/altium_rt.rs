//! Saving a board back into the Altium `.PcbDoc` it was opened from.
//!
//! A fresh file (see [`crate::to_altium`]) carries only what Trace models.
//! A board opened from a PcbDoc carries much more: rules, polygon pour
//! settings, dimensions, 3D bodies, unions, library links, unique ids. This
//! writer starts from the original file and keeps every record as it is,
//! changing only what was edited in Trace:
//!
//! - records whose object is unchanged are copied byte for byte;
//! - parts that moved, turned or flipped take everything they own with
//!   them (pads, silk, texts, regions, 3D bodies), patched in place;
//! - net changes (from the schematic) patch the net of each record;
//! - removed tracks, arcs, vias, pads and areas are dropped, and new ones
//!   are added, copied from a like record of the same board when there is
//!   one (a via of the same style, a pad of the same shape) so they carry
//!   the board's own settings;
//! - new parts get a part record, a designator and a comment;
//! - Altium's side tables that point at records by position (unique ids,
//!   GUIDs, pad and via library links, part parameters, shape-based
//!   regions) are renumbered to match.
//!
//! The written file is checked by reading it back: the board read back
//! must match the board saved.

use crate::altium::{
    binary_record, delphi, put_f64, put_i32, put_u16, put_u32, trim, Region, Writer, TEXT,
};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use trace_altium::pcb_raw::{self, Layers};
use trace_core::place::{Move, Placement};
use trace_core::*;

/// What saving over the original file did.
#[derive(Clone, Debug, Default)]
pub struct SaveReport {
    /// Records copied as they were.
    pub kept: usize,
    /// Records kept but changed (moved with their part, another net...).
    pub changed: usize,
    /// Records written new.
    pub added: usize,
    /// Records of objects no longer on the board.
    pub removed: usize,
    /// Things that could not be carried over.
    pub notes: Vec<String>,
}

impl SaveReport {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} records kept, {} changed, {} added, {} removed",
            self.kept, self.changed, self.added, self.removed
        );
        for n in &self.notes {
            s.push_str("; ");
            s.push_str(n);
        }
        s
    }
}

type IoResult<T> = std::io::Result<T>;

fn bad(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// The board as the bytes of a `.PcbDoc`, built on the binary PcbDoc at
/// `original` that it was opened from. `name` is the file name Altium
/// shows. Boards from other sources (or an ASCII PcbDoc) are written
/// fresh, as [`crate::to_altium`] does.
pub fn to_altium_over(
    original: &Path,
    board: &Board,
    name: &str,
) -> IoResult<(Vec<u8>, SaveReport)> {
    let data = std::fs::read(original)?;
    if !data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        let mut r = SaveReport::default();
        r.notes
            .push("the original is not a binary PcbDoc, so the board was written fresh".into());
        return Ok((crate::to_altium(board, name)?, r));
    }
    let base = trace_altium::read_pcbdoc(original).map_err(bad)?;
    let file = Cfb::read(data)?;
    let mut m = Merge::new(file, &base, board, name);
    m.run()?;
    let report = m.report.clone();
    Ok((m.file.write()?, report))
}

// ---------------------------------------------------------------------------
// The compound file: every storage and stream, in order.

struct Cfb {
    storages: Vec<(String, [u8; 16])>,
    streams: Vec<(String, Vec<u8>)>,
    root_clsid: [u8; 16],
}

impl Cfb {
    fn read(data: Vec<u8>) -> IoResult<Cfb> {
        let mut cf = cfb::CompoundFile::open(Cursor::new(data))?;
        let root_clsid = *cf.root_entry().clsid().as_bytes();
        let entries: Vec<(String, bool, [u8; 16])> = cf
            .walk()
            .filter(|e| e.path() != Path::new("/"))
            .map(|e| {
                (
                    e.path().to_string_lossy().replace('\\', "/"),
                    e.is_stream(),
                    *e.clsid().as_bytes(),
                )
            })
            .collect();
        let mut storages = Vec::new();
        let mut streams = Vec::new();
        for (path, is_stream, clsid) in entries {
            if is_stream {
                let mut buf = Vec::new();
                cf.open_stream(&path)?.read_to_end(&mut buf)?;
                streams.push((path, buf));
            } else {
                storages.push((path, clsid));
            }
        }
        Ok(Cfb {
            storages,
            streams,
            root_clsid,
        })
    }

    fn get(&self, path: &str) -> Option<&Vec<u8>> {
        self.streams
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
            .map(|(_, d)| d)
    }

    fn set(&mut self, path: &str, data: Vec<u8>) {
        if let Some(s) = self
            .streams
            .iter_mut()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
        {
            s.1 = data;
        } else {
            let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            if !dir.is_empty() && !self.storages.iter().any(|(p, _)| p == dir) {
                self.storages.push((dir.to_string(), [0; 16]));
            }
            self.streams.push((path.to_string(), data));
        }
    }

    /// A storage's `Data` stream and its record count in `Header`.
    fn set_records(&mut self, storage: &str, count: usize, data: Vec<u8>) {
        self.set(&format!("/{storage}/Data"), data);
        self.set(
            &format!("/{storage}/Header"),
            (count as u32).to_le_bytes().to_vec(),
        );
    }

    fn write(&self) -> IoResult<Vec<u8>> {
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        cf.set_storage_clsid("/", uuid_of(self.root_clsid))?;
        let mut storages = self.storages.clone();
        // Parents before children.
        storages.sort_by_key(|(p, _)| p.matches('/').count());
        for (path, clsid) in &storages {
            cf.create_storage_all(path)?;
            if *clsid != [0; 16] {
                cf.set_storage_clsid(path, uuid_of(*clsid))?;
            }
        }
        for (path, data) in &self.streams {
            cf.create_stream(path)?.write_all(data)?;
        }
        cf.flush()?;
        Ok(cf.into_inner().into_inner())
    }
}

fn uuid_of(b: [u8; 16]) -> uuid::Uuid {
    uuid::Uuid::from_bytes(b)
}

// ---------------------------------------------------------------------------
// Records.

/// One binary record: its type byte and blocks.
#[derive(Clone)]
struct Rec {
    kind: u8,
    blocks: Vec<Vec<u8>>,
}

fn split(data: &[u8], blocks: usize) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < data.len() {
        let kind = data[i];
        i += 1;
        let mut bl = Vec::with_capacity(blocks);
        for _ in 0..blocks {
            let Some(n) = data.get(i..i + 4) else {
                break 'outer;
            };
            let n = u32::from_le_bytes(n.try_into().unwrap()) as usize;
            let Some(b) = data.get(i + 4..i + 4 + n) else {
                break 'outer;
            };
            bl.push(b.to_vec());
            i += 4 + n;
        }
        out.push(Rec { kind, blocks: bl });
    }
    out
}

fn join(recs: &[Rec]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in recs {
        let subs: Vec<&[u8]> = r.blocks.iter().map(|b| b.as_slice()).collect();
        binary_record(&mut out, r.kind, &subs);
    }
    out
}

/// A text record's properties in their order, as written.
#[derive(Clone)]
struct Text {
    fields: Vec<(String, String)>,
    /// A closing zero byte, as Altium writes.
    nul: bool,
}

impl Text {
    fn parse(b: &[u8]) -> Text {
        let nul = b.last() == Some(&0);
        let s: String = b
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as char)
            .collect();
        let fields = s
            .split('|')
            .filter(|f| !f.is_empty())
            .map(|f| match f.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (f.to_string(), String::new()),
            })
            .collect();
        Text { fields, nul }
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }

    fn set(&mut self, k: &str, v: impl Into<String>) {
        let v = v.into();
        // A Unicode copy of the old value would win over the new one.
        let utf = format!("%UTF8%{k}");
        self.fields.retain(|(a, _)| !a.eq_ignore_ascii_case(&utf));
        match self.fields.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(k)) {
            Some(f) => f.1 = v,
            None => self.fields.push((k.to_string(), v)),
        }
        if !v_is_latin1(self.get(k).unwrap_or("")) {
            let value = self.get(k).unwrap_or("").to_string();
            let bytes: String = value.bytes().map(|b| b as char).collect();
            self.fields.push((utf, bytes));
        }
    }

    fn remove_where(&mut self, f: impl Fn(&str) -> bool) {
        self.fields.retain(|(k, _)| !f(k));
    }

    fn bytes(&self) -> Vec<u8> {
        let mut s = String::new();
        for (k, v) in &self.fields {
            s.push('|');
            s.push_str(k);
            s.push('=');
            s.push_str(&v.replace('|', "/"));
        }
        let mut b: Vec<u8> = s
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        if self.nul {
            b.push(0);
        }
        b
    }
}

fn v_is_latin1(s: &str) -> bool {
    s.chars().all(|c| (c as u32) < 256)
}

/// A text stream: u32 length + text, repeated, with `prefix` bytes before
/// each length (two in `Rules6`).
fn split_text(data: &[u8], prefix: usize) -> Vec<(Vec<u8>, Text)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + prefix + 4 <= data.len() {
        let pre = data[i..i + prefix].to_vec();
        i += prefix;
        let n = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        let Some(b) = data.get(i..i + n) else { break };
        out.push((pre, Text::parse(b)));
        i += n;
    }
    out
}

fn join_text(recs: &[(Vec<u8>, Text)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (pre, t) in recs {
        out.extend_from_slice(pre);
        let b = t.bytes();
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&b);
    }
    out
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Altium's 32-bit layer id of a layer byte.
fn v7_of(id: u8) -> Option<u32> {
    Some(match id {
        1..=31 => 0x0100_0000 + id as u32,
        32 => 0x0100_FFFF,
        33 => 0x0103_0006,
        34 => 0x0103_0007,
        35..=38 => 0x0103_0008 + (id - 35) as u32,
        56 => 0x0103_000D,
        57..=72 => 0x0102_0000 + (id - 56) as u32,
        74 => 0x0103_000F,
        _ => return None,
    })
}

/// The name `V7_LAYER` gives a layer byte.
fn v7_name(id: u8) -> Option<String> {
    Some(match id {
        1 => "TOP".into(),
        2..=31 => format!("MID{}", id - 1),
        32 => "BOTTOM".into(),
        33 => "TOPOVERLAY".into(),
        34 => "BOTTOMOVERLAY".into(),
        35 => "TOPPASTE".into(),
        36 => "BOTTOMPASTE".into(),
        37 => "TOPSOLDER".into(),
        38 => "BOTTOMSOLDER".into(),
        56 => "KEEPOUT".into(),
        57..=72 => format!("MECHANICAL{}", id - 56),
        74 => "MULTILAYER".into(),
        _ => return None,
    })
}

const NONE16: u16 = u16::MAX;

/// Positions within this much (nm) are the same place: moves done in
/// floating point leave a few nanometres of noise.
const TOL: i64 = 100;

fn near(a: Point, b: Point) -> bool {
    (a.x.0 - b.x.0).abs() <= TOL && (a.y.0 - b.y.0).abs() <= TOL
}
fn near_len(a: Length, b: Length) -> bool {
    (a.0 - b.0).abs() <= TOL
}
fn near_deg(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) < 0.01
}
fn near_ring(a: &[Point], b: &[Point]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| near(*p, *q))
}

fn cell(p: Point) -> (i64, i64) {
    (p.x.0.div_euclid(10_000), p.y.0.div_euclid(10_000))
}

/// Pairs each of `exp` with an unused item of `ed` that is the same,
/// looking near its anchor point. Returns the partner of each of `exp`.
fn pair<T>(
    exp: &[T],
    ed: &[T],
    anchor: impl Fn(&T) -> Point,
    same: impl Fn(&T, &T) -> bool,
    used: &mut [bool],
) -> Vec<Option<usize>> {
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in ed.iter().enumerate() {
        if !used[i] {
            grid.entry(cell(anchor(t))).or_default().push(i);
        }
    }
    exp.iter()
        .map(|e| {
            let (cx, cy) = cell(anchor(e));
            let mut best: Option<usize> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &j in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                        if !used[j] && best.is_none_or(|b| j < b) && same(e, &ed[j]) {
                            best = Some(j);
                        }
                    }
                }
            }
            if let Some(j) = best {
                used[j] = true;
            }
            best
        })
        .collect()
}

fn same_track(a: &Track, b: &Track) -> bool {
    a.layer == b.layer
        && a.component == b.component
        && near_len(a.width, b.width)
        && ((near(a.start, b.start) && near(a.end, b.end))
            || (near(a.start, b.end) && near(a.end, b.start)))
}

fn same_arc(a: &Arc, b: &Arc) -> bool {
    a.layer == b.layer
        && a.component == b.component
        && near_len(a.width, b.width)
        && near_len(a.radius, b.radius)
        && near(a.center, b.center)
        && near_deg(a.start_deg, b.start_deg)
        && near_deg(a.end_deg, b.end_deg)
}

fn same_via(a: &Via, b: &Via) -> bool {
    a.from_layer.min(a.to_layer) == b.from_layer.min(b.to_layer)
        && a.from_layer.max(a.to_layer) == b.from_layer.max(b.to_layer)
        && near(a.at, b.at)
        && near_len(a.diameter, b.diameter)
        && near_len(a.hole, b.hole)
        && a.fab == b.fab
        && a.rings.len() == b.rings.len()
        && a
            .rings
            .iter()
            .zip(&b.rings)
            .all(|(x, y)| x.0 == y.0 && near_len(x.1, y.1))
}

fn same_pad(a: &Pad, b: &Pad) -> bool {
    a.layer == b.layer
        && a.component == b.component
        && a.name == b.name
        && near(a.at, b.at)
        && near_len(a.size_x, b.size_x)
        && near_len(a.size_y, b.size_y)
        && near_len(a.hole, b.hole)
        && a.shape == b.shape
        && near_deg(a.rotation_deg, b.rotation_deg)
        && a.stack.len() == b.stack.len()
        && a.stack.iter().zip(&b.stack).all(|(x, y)| {
            x.layer == y.layer
                && x.shape == y.shape
                && near_len(x.size_x, y.size_x)
                && near_len(x.size_y, y.size_y)
        })
}

fn same_area(a: &Polygon, b: &Polygon) -> bool {
    a.layer == b.layer
        && a.component == b.component
        && a.pour == b.pour
        && a.holes.len() == b.holes.len()
        && near_ring(&a.outline, &b.outline)
        && a.holes.iter().zip(&b.holes).all(|(x, y)| near_ring(x, y))
}

fn first(p: &[Point]) -> Point {
    p.first().copied().unwrap_or_default()
}

/// Model-side moves, as `trace_core::place::place` does them.
fn move_track(t: &mut Track, m: &Move) {
    t.start = m.point(t.start);
    t.end = m.point(t.end);
    t.layer = m.layer(&t.layer);
}
fn move_arc(a: &mut Arc, m: &Move) {
    let (s, e) = (m.angle(a.start_deg), m.angle(a.end_deg));
    (a.start_deg, a.end_deg) = if m.flips() { (e, s) } else { (s, e) };
    a.center = m.point(a.center);
    a.layer = m.layer(&a.layer);
}
fn move_pad(p: &mut Pad, m: &Move) {
    p.at = m.point(p.at);
    p.rotation_deg = m.angle(p.rotation_deg);
    p.layer = m.layer(&p.layer);
    for q in &mut p.outline {
        *q = m.point(*q);
    }
    for l in &mut p.stack {
        l.layer = m.copper(l.layer);
        l.rotation_deg = m.angle(l.rotation_deg);
        for q in &mut l.outline {
            *q = m.point(*q);
        }
    }
    if m.flips() {
        p.stack.sort_by_key(|l| l.layer);
    }
}
fn move_area(f: &mut Polygon, m: &Move) {
    for q in f.outline.iter_mut().chain(f.holes.iter_mut().flatten()) {
        *q = m.point(*q);
    }
    f.layer = m.layer(&f.layer);
}

// ---------------------------------------------------------------------------
// The merge.

struct Merge<'a> {
    file: Cfb,
    base: &'a Board,
    ed: &'a Board,
    layers: Layers,
    /// Old part index to new.
    comp_map: Vec<Option<u32>>,
    /// How each old part moved.
    moves: Vec<Option<Move>>,
    /// Old net index to new.
    net_map: Vec<Option<u32>>,
    /// Old record index to new, per stream, for the side tables.
    renumber: HashMap<&'static str, Vec<Option<u32>>>,
    /// Records written new, per stream (new indices from the old count).
    w: Writer<'a>,
    report: SaveReport,
    #[allow(dead_code)]
    name: String,
    /// Copper regions new pads need for their exact outline.
    pad_regions: Vec<Region>,
}

impl<'a> Merge<'a> {
    fn new(file: Cfb, base: &'a Board, ed: &'a Board, name: &str) -> Self {
        let layers = Layers::new(&base.stackup);
        let ncopper = base.stackup.copper_layers().count() as u8;
        let copper_ids: Vec<u8> = (0..ncopper).map_while(|i| layers.id_of(i)).collect();
        let mut w = Writer::in_place(ed, copper_ids);
        // Unique ids unlike the ones Altium gave.
        let seed = ed.geometry.tracks.len() as u64 * 7919
            + ed.geometry.vias.len() as u64 * 104_729
            + ed.components.len() as u64;
        w.seed(seed ^ 0x5DEE_CE66_D1CE);

        let comp_map: Vec<Option<u32>> = (0..base.components.len())
            .map(|i| (i < ed.components.len()).then_some(i as u32))
            .collect();
        let moves = (0..base.components.len())
            .map(|i| {
                let j = comp_map[i]? as usize;
                let from = Placement::of(base, i)?;
                let to = Placement::of(ed, j)?;
                let moved = from.x != to.x
                    || from.y != to.y
                    || from.side != to.side
                    || !near_deg(from.rotation_deg, to.rotation_deg)
                    || (from.rotation_deg - to.rotation_deg).abs() > 1e-9;
                moved.then(|| Move::new(base, from, to))
            })
            .collect();
        // Nets by name (the n-th net of a name to the n-th of that name).
        let mut by_name: HashMap<&str, Vec<u32>> = HashMap::new();
        for (j, n) in ed.nets.iter().enumerate().rev() {
            by_name.entry(n.name.as_str()).or_default().push(j as u32);
        }
        let net_map = base
            .nets
            .iter()
            .map(|n| by_name.get_mut(n.name.as_str()).and_then(|v| v.pop()))
            .collect();
        Merge {
            file,
            base,
            ed,
            layers,
            comp_map,
            moves,
            net_map,
            renumber: HashMap::new(),
            w,
            report: SaveReport::default(),
            name: name.to_string(),
            pad_regions: Vec::new(),
        }
    }

    fn old_comp(&self, c: Option<u32>) -> Result<Option<u32>, ()> {
        match c {
            None => Ok(None),
            Some(c) => match self.comp_map.get(c as usize) {
                Some(Some(n)) => Ok(Some(*n)),
                Some(None) => Err(()),
                None => Ok(None),
            },
        }
    }

    fn mv(&self, c: Option<u32>) -> Option<&Move> {
        self.moves.get(c? as usize)?.as_ref()
    }

    fn new_net(&self, n: Option<u32>) -> Option<u32> {
        self.net_map.get(n? as usize).copied().flatten()
    }

    fn run(&mut self) -> IoResult<()> {
        self.nets();
        self.components();
        self.tracks();
        self.arcs();
        self.vias();
        self.pads();
        self.areas();
        self.texts();
        self.bodies();
        self.polygons();
        self.params();
        self.side_tables();
        self.classes();
        self.diff_pairs();
        self.rules();
        self.board6();
        Ok(())
    }

    // --- records shared by every primitive --------------------------------

    /// Points a kept record at the new net and part numbers. `None` when
    /// its part is gone (the record goes with it).
    fn relink(&self, b: &mut [u8]) -> Option<Option<u32>> {
        let comp = u16_at(b, 7).filter(|&c| c != NONE16).map(u32::from);
        let nc = self.old_comp(comp).ok()?;
        if b.len() >= 9 {
            put_u16(b, 7, nc.map_or(NONE16, |c| c as u16));
        }
        if let Some(n) = u16_at(b, 3).filter(|&n| n != NONE16) {
            if (n as usize) < self.net_map.len() {
                let nn = self.new_net(Some(n as u32));
                put_u16(b, 3, nn.map_or(NONE16, |n| n as u16));
            }
        }
        Some(comp)
    }

    fn set_net(b: &mut [u8], net: Option<u32>) {
        put_u16(b, 3, net.map_or(NONE16, |n| n.min(NONE16 as u32 - 1) as u16));
    }

    /// The layer byte a flip sends `id` to.
    fn flip_id(&self, id: u8, m: &Move) -> u8 {
        if !m.flips() {
            return id;
        }
        match m.layer(&self.layers.get(id)) {
            LayerRef::Copper(i) => self.layers.id_of(i).unwrap_or(id),
            LayerRef::TopSilk => 33,
            LayerRef::BottomSilk => 34,
            LayerRef::Other(x) => x,
            _ => id,
        }
    }

    /// Moves a point stored as two i32 (internal units) at `at`.
    fn xy(b: &mut [u8], at: usize, m: &Move) {
        let (Some(x), Some(y)) = (i32_at(b, at), i32_at(b, at + 4)) else {
            return;
        };
        let [u, v] = m.xy([x as f64 / 10_000.0, y as f64 / 10_000.0]);
        put_i32(b, at, (u * 10_000.0).round() as i32);
        put_i32(b, at + 4, (v * 10_000.0).round() as i32);
    }

    fn angle(b: &mut [u8], at: usize, m: &Move) {
        if let Some(a) = f64_at(b, at).filter(|a| a.is_finite()) {
            put_f64(b, at, m.angle(a));
        }
    }

    /// Changes the layer byte, and the 32-bit layer id wherever the record
    /// carries it, when a flip moves the record to another layer.
    fn relayer(&self, b: &mut [u8], m: &Move) -> u8 {
        let end = b.len();
        self.relayer_upto(b, m, end)
    }

    fn relayer_upto(&self, b: &mut [u8], m: &Move, end: usize) -> u8 {
        let Some(&old) = b.first() else { return 0 };
        let new = self.flip_id(old, m);
        if new != old {
            b[0] = new;
            if let (Some(a), Some(z)) = (v7_of(old), v7_of(new)) {
                let mut i = 13;
                while i + 4 <= end.min(b.len()) {
                    if u32_at(b, i) == Some(a) {
                        put_u32(b, i, z);
                        i += 4;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        new
    }

    /// Moves the double vertices of a region-like record (regions, body
    /// outlines): 18-byte header, properties, then rings.
    fn move_rings(b: &mut [u8], m: &Move) {
        let holes = u16_at(b, 14).unwrap_or(0) as usize;
        let Some(plen) = u32_at(b, 18) else { return };
        let mut i = 22 + plen as usize;
        for _ in 0..=holes {
            let Some(n) = u32_at(b, i) else { return };
            i += 4;
            for _ in 0..n {
                let (Some(x), Some(y)) = (f64_at(b, i), f64_at(b, i + 8)) else {
                    return;
                };
                let [u, v] = m.xy([x / 10_000.0, y / 10_000.0]);
                put_f64(b, i, u * 10_000.0);
                put_f64(b, i + 8, v * 10_000.0);
                i += 16;
            }
        }
    }

    /// The same for a shape-based region: the outline as 37-byte vertices
    /// (round flag, point, centre, radius, two angles), holes as doubles.
    fn move_shape_rings(b: &mut [u8], m: &Move) {
        let holes = u16_at(b, 14).unwrap_or(0) as usize;
        let Some(plen) = u32_at(b, 18) else { return };
        let mut i = 22 + plen as usize;
        let Some(n) = u32_at(b, i) else { return };
        i += 4;
        for _ in 0..=n {
            if i + 37 > b.len() {
                return;
            }
            let round = b[i] != 0;
            Self::xy(b, i + 1, m);
            if round {
                Self::xy(b, i + 9, m);
                let (s, e) = (f64_at(b, i + 21).unwrap_or(0.0), f64_at(b, i + 29).unwrap_or(0.0));
                let (s, e) = (m.angle(s), m.angle(e));
                let (s, e) = if m.flips() { (e, s) } else { (s, e) };
                put_f64(b, i + 21, s);
                put_f64(b, i + 29, e);
            }
            i += 37;
        }
        for _ in 0..holes {
            let Some(n) = u32_at(b, i) else { return };
            i += 4;
            for _ in 0..n {
                let (Some(x), Some(y)) = (f64_at(b, i), f64_at(b, i + 8)) else {
                    return;
                };
                let [u, v] = m.xy([x / 10_000.0, y / 10_000.0]);
                put_f64(b, i, u * 10_000.0);
                put_f64(b, i + 8, v * 10_000.0);
                i += 16;
            }
        }
    }

    /// Rewrites the property block of a region-like record.
    fn edit_props(b: &mut Vec<u8>, f: impl FnOnce(&mut Text)) {
        let Some(plen) = u32_at(b, 18) else { return };
        let end = 22 + plen as usize;
        if end > b.len() {
            return;
        }
        let mut t = Text::parse(&b[22..end]);
        f(&mut t);
        let nb = t.bytes();
        let mut out = b[..18].to_vec();
        out.extend_from_slice(&(nb.len() as u32).to_le_bytes());
        out.extend_from_slice(&nb);
        out.extend_from_slice(&b[end..]);
        *b = out;
    }

    fn count(&mut self, changed: bool) {
        if changed {
            self.report.changed += 1;
        } else {
            self.report.kept += 1;
        }
    }

    // --- nets and parts ----------------------------------------------------

    fn nets(&mut self) {
        let Some(data) = self.file.get("/Nets6/Data") else {
            return;
        };
        let old = split_text(data, 0);
        // The record of each new net, in the board's order.
        let mut of_new: Vec<Option<usize>> = vec![None; self.ed.nets.len()];
        for (i, n) in self.net_map.iter().enumerate() {
            if let Some(n) = n {
                of_new[*n as usize] = Some(i);
            }
        }
        let mut out = Vec::new();
        for (j, n) in self.ed.nets.iter().enumerate() {
            match of_new[j].and_then(|i| old.get(i)) {
                Some(r) => {
                    out.push(r.clone());
                    self.report.kept += 1;
                }
                None => {
                    let rec = self.w.net_record(n);
                    out.extend(split_text(&rec, 0));
                    self.report.added += 1;
                }
            }
        }
        self.report.removed += self.net_map.iter().filter(|n| n.is_none()).count();
        let renum = self.net_map.clone();
        self.renumber.insert("Nets6", renum);
        self.file.set_records("Nets6", out.len(), join_text(&out));
    }

    fn components(&mut self) {
        let Some(data) = self.file.get("/Components6/Data") else {
            return;
        };
        let old = split_text(data, 0);
        let mut out = Vec::new();
        for (j, c) in self.ed.components.iter().enumerate() {
            let i = j; // parts keep their place in the list
            match (old.get(i), self.base.components.get(i)) {
                (Some((pre, t)), Some(b)) => {
                    let mut t = t.clone();
                    let mut changed = false;
                    if b.x != c.x || b.y != c.y {
                        t.set("X", format!("{}mil", trim(c.x.mil(), 4)));
                        t.set("Y", format!("{}mil", trim(c.y.mil(), 4)));
                        changed = true;
                    }
                    if (b.rotation_deg - c.rotation_deg).abs() > 1e-9 {
                        t.set("ROTATION", delphi(c.rotation_deg.rem_euclid(360.0)));
                        changed = true;
                    }
                    if b.side != c.side {
                        t.set(
                            "LAYER",
                            if c.side == Side::Bottom { "BOTTOM" } else { "TOP" },
                        );
                        changed = true;
                    }
                    if b.designator != c.designator {
                        t.set("SOURCEDESIGNATOR", c.designator.clone());
                        changed = true;
                    }
                    if b.footprint != c.footprint && !c.footprint.is_empty() {
                        t.set("PATTERN", c.footprint.clone());
                        changed = true;
                    }
                    if b.library_ref != c.library_ref {
                        if let Some(l) = &c.library_ref {
                            t.set("SOURCELIBREFERENCE", l.clone());
                            changed = true;
                        }
                    }
                    out.push((pre.clone(), t));
                    self.count(changed);
                }
                _ => {
                    let id = self.w.unique_id();
                    let rec = self.w.component_record(c, &id);
                    out.extend(split_text(&rec, 0));
                    self.report.added += 1;
                }
            }
        }
        self.report.removed += self.comp_map.iter().filter(|c| c.is_none()).count();
        self.renumber.insert("Components6", self.comp_map.clone());
        self.file.set_records("Components6", out.len(), join_text(&out));
    }

    // --- primitives ----------------------------------------------------------

    /// Keeps, patches, drops and adds the records of one primitive stream.
    /// `parse` decodes a record into the model; `exp` turns an old model
    /// item into what it should be now (part renumbered and moved), or
    /// none when its part is gone; `patch` moves a kept record with its
    /// part. Returns the kept records, the partner of each kept record,
    /// and which edited items were matched.
    #[allow(clippy::too_many_arguments)]
    fn merge<T: Clone>(
        &mut self,
        stream: &'static str,
        recs: Vec<Rec>,
        main: usize,
        parse: impl Fn(&Rec) -> Option<T>,
        base_items: &[T],
        edited: &[T],
        exp: impl Fn(&Self, &T) -> Option<T>,
        anchor: impl Fn(&T) -> Point + Copy,
        same: impl Fn(&T, &T) -> bool + Copy,
        net_of: impl Fn(&T) -> Option<u32>,
        patch: impl Fn(&Self, &mut Rec, &Move),
    ) -> (Vec<Rec>, Vec<bool>) {
        let parsed: Vec<Option<T>> = recs.iter().map(&parse).collect();
        // Objects the reader makes beyond the records (text drawn as
        // tracks, say): they are kept by their own records elsewhere, so an
        // edited object that is one of them is not written here.
        let from_recs: Vec<T> = parsed.iter().flatten().cloned().collect();
        let mut used_b = vec![false; base_items.len()];
        let back = pair(&from_recs, base_items, anchor, same, &mut used_b);
        let derived: Vec<T> = base_items
            .iter()
            .zip(&used_b)
            .filter(|(_, u)| !**u)
            .map(|(t, _)| t.clone())
            .collect();
        // The reader may fill in a net the record leaves out (poured copper
        // takes its pour's): compare nets with what the reader made.
        let mut eff_net: Vec<Option<u32>> = Vec::with_capacity(recs.len());
        {
            let mut k = 0;
            for p in &parsed {
                eff_net.push(p.as_ref().and_then(|t| {
                    let n = back[k].map(|b| net_of(&base_items[b])).unwrap_or(net_of(t));
                    k += 1;
                    n
                }));
            }
        }

        let mut used = vec![false; edited.len()];
        let exp_derived: Vec<T> = derived.iter().filter_map(|t| exp(self, t)).collect();
        pair(&exp_derived, edited, anchor, same, &mut used);
        let exp_recs: Vec<Option<T>> = parsed
            .iter()
            .map(|p| p.as_ref().and_then(|t| exp(self, t)))
            .collect();
        let flat: Vec<T> = exp_recs.iter().flatten().cloned().collect();
        let partner = pair(&flat, edited, anchor, same, &mut used);

        let mut out = Vec::new();
        let mut renum = vec![None; recs.len()];
        let mut k = 0;
        for (i, mut r) in recs.into_iter().enumerate() {
            let matched = if exp_recs[i].is_some() {
                let m = partner[k];
                k += 1;
                Some(m)
            } else {
                None
            };
            if parsed[i].is_some() && matches!(matched, None | Some(None)) {
                // Its object was removed, or its part is gone.
                self.report.removed += 1;
                continue;
            }
            let Some(b) = r.blocks.get_mut(main) else {
                out.push(r);
                continue;
            };
            let Some(comp) = self.relink(b) else {
                self.report.removed += 1;
                continue;
            };
            let mut changed = false;
            if let Some(Some(j)) = matched {
                let want = net_of(&edited[j]);
                let had = self.new_net(eff_net[i]);
                if want != had {
                    Self::set_net(&mut r.blocks[main], want);
                    changed = true;
                }
            }
            if let Some(m) = self.mv(comp) {
                patch(self, &mut r, m);
                changed = true;
            }
            self.count(changed);
            renum[i] = Some(out.len() as u32);
            out.push(r);
        }
        self.renumber.insert(stream, renum);
        (out, used)
    }

    fn exp_common<T: Clone>(
        &self,
        t: &T,
        comp: Option<u32>,
        set: impl Fn(&mut T, Option<u32>),
        mv: impl Fn(&mut T, &Move),
    ) -> Option<T> {
        let nc = self.old_comp(comp).ok()?;
        let mut t = t.clone();
        set(&mut t, nc);
        if let Some(m) = self.mv(comp) {
            mv(&mut t, m);
        }
        Some(t)
    }

    fn tracks(&mut self) {
        let Some(data) = self.file.get("/Tracks6/Data") else {
            return;
        };
        let recs = split(data, 1);
        let layers = Layers::new(&self.base.stackup);
        let ed = &self.ed.geometry.tracks;
        let (mut out, used) = self.merge(
            "Tracks6",
            recs,
            0,
            |r| pcb_raw::track(&r.blocks[0], &layers),
            &self.base.geometry.tracks,
            ed,
            |s, t| s.exp_common(t, t.component, |t, c| t.component = c, move_track),
            |t| t.start,
            same_track,
            |t| t.net,
            |s, r, m| {
                let b = &mut r.blocks[0];
                Self::xy(b, 13, m);
                Self::xy(b, 21, m);
                s.relayer(b, m);
            },
        );
        let template = out
            .iter()
            .find(|r| {
                let b = &r.blocks[0];
                b.len() >= 45
                    && u16_at(b, 7) == Some(NONE16)
                    && u16_at(b, 5) == Some(NONE16)
                    && u16_at(b, 3) != Some(NONE16)
            })
            .map(|r| r.blocks[0].clone());
        let mut skipped_text = 0;
        for (t, u) in ed.iter().zip(&used) {
            if *u {
                continue;
            }
            if t.text {
                skipped_text += 1;
                continue;
            }
            let Some(rec) = self.w.track_record(t) else {
                continue;
            };
            let mut r = split(&rec, 1).remove(0);
            if let Some(tpl) = &template {
                // The board's own track record, with this track's values.
                let mut b = tpl.clone();
                let n = &r.blocks[0];
                b[0] = n[0];
                b[1] = tpl[1] & !0x01; // not locked
                b[3..13].copy_from_slice(&n[3..13]);
                b[13..33].copy_from_slice(&n[13..33]);
                put_u32(&mut b, 41, u32_at(n, 41).unwrap_or(0));
                r.blocks[0] = b;
            }
            out.push(r);
            self.report.added += 1;
        }
        if skipped_text > 0 {
            self.report.notes.push(format!(
                "{skipped_text} strokes of new text left out (new parts get designator texts)"
            ));
        }
        self.file
            .set_records("Tracks6", out.len(), join(&out));
    }

    fn arcs(&mut self) {
        let Some(data) = self.file.get("/Arcs6/Data") else {
            return;
        };
        let recs = split(data, 1);
        let layers = Layers::new(&self.base.stackup);
        let ed = &self.ed.geometry.arcs;
        let (mut out, used) = self.merge(
            "Arcs6",
            recs,
            0,
            |r| pcb_raw::arc(&r.blocks[0], &layers),
            &self.base.geometry.arcs,
            ed,
            |s, t| s.exp_common(t, t.component, |t, c| t.component = c, move_arc),
            |a| a.center,
            same_arc,
            |a| a.net,
            |s, r, m| {
                let b = &mut r.blocks[0];
                Self::xy(b, 13, m);
                let (Some(sa), Some(ea)) = (f64_at(b, 25), f64_at(b, 33)) else {
                    return;
                };
                let full = (ea - sa).abs() >= 359.999;
                let (mut a, mut e) = (m.angle(sa), m.angle(ea));
                if m.flips() {
                    std::mem::swap(&mut a, &mut e);
                }
                if full {
                    e = a + 360.0;
                }
                put_f64(b, 25, a);
                put_f64(b, 33, e);
                s.relayer(b, m);
            },
        );
        for (a, u) in ed.iter().zip(&used) {
            if *u {
                continue;
            }
            if let Some(rec) = self.w.arc_record_of(a) {
                out.push(split(&rec, 1).remove(0));
                self.report.added += 1;
            }
        }
        self.file.set_records("Arcs6", out.len(), join(&out));
    }

    fn vias(&mut self) {
        let Some(data) = self.file.get("/Vias6/Data") else {
            return;
        };
        let recs = split(data, 1);
        let layers = Layers::new(&self.base.stackup);
        let ed = &self.ed.geometry.vias;
        // Vias have no part of their own in the model.
        let (mut out, used) = self.merge(
            "Vias6",
            recs.clone(),
            0,
            |r| pcb_raw::via(&r.blocks[0], &layers),
            &self.base.geometry.vias,
            ed,
            |_, t| Some(t.clone()),
            |v| v.at,
            same_via,
            |v| v.net,
            |_, r, m| Self::xy(&mut r.blocks[0], 13, m),
        );
        // New vias: a copy of a via of the same style when the board has one.
        let mut styles: Vec<(Via, usize)> = Vec::new();
        for (i, r) in recs.iter().enumerate() {
            if let Some(v) = pcb_raw::via(&r.blocks[0], &layers) {
                if !styles.iter().any(|(s, _)| same_style(s, &v)) {
                    styles.push((v, i));
                }
            }
        }
        for (v, u) in ed.iter().zip(&used) {
            if *u {
                continue;
            }
            let r = match styles.iter().find(|(s, _)| same_style(s, v)) {
                Some((_, i)) => {
                    let mut r = recs[*i].clone();
                    let b = &mut r.blocks[0];
                    b[1] &= !0x01;
                    Self::set_net(b, v.net.filter(|&n| (n as usize) < self.ed.nets.len()));
                    put_u16(b, 5, NONE16);
                    put_u16(b, 7, NONE16);
                    put_i32(b, 13, (v.at.x.0 as f64 / 2.54).round() as i32);
                    put_i32(b, 17, (v.at.y.0 as f64 / 2.54).round() as i32);
                    r
                }
                None => split(&self.w.via_record(v), 1).remove(0),
            };
            out.push(r);
            self.report.added += 1;
        }
        self.file.set_records("Vias6", out.len(), join(&out));
    }

    fn pads(&mut self) {
        let Some(data) = self.file.get("/Pads6/Data") else {
            return;
        };
        let recs = split(data, 6);
        let layers = Layers::new(&self.base.stackup);
        let ed = &self.ed.geometry.pads;
        let (mut out, used) = self.merge(
            "Pads6",
            recs.clone(),
            4,
            |r| {
                let bl: Vec<&[u8]> = r.blocks.iter().map(|b| b.as_slice()).collect();
                pcb_raw::pad(&bl, &layers)
            },
            &self.base.geometry.pads,
            ed,
            |s, t| s.exp_common(t, t.component, |t, c| t.component = c, move_pad),
            |p| p.at,
            same_pad,
            |p| p.net,
            |s, r, m| {
                let b = &mut r.blocks[4];
                Self::xy(b, 13, m);
                Self::angle(b, 52, m);
                let old = b[0];
                let new = s.relayer(b, m);
                if m.flips() {
                    // Tenting and the top and bottom lands change sides.
                    let f = b[1];
                    b[1] = (f & !0x60) | ((f & 0x20) << 1) | ((f & 0x40) >> 1);
                    if b.len() > 51 && b[62] == 2 {
                        for k in 0..8 {
                            b.swap(21 + k, 37 + k);
                        }
                        b.swap(49, 51);
                    }
                    if new != old && (1..=32).contains(&old) && (1..=32).contains(&new) {
                        if let Some(x) = r.blocks.get_mut(5).filter(|x| x.len() >= 596) {
                            let (o, n) = (old as usize - 1, new as usize - 1);
                            x[532 + n] = x[532 + o];
                            x[564 + n] = x[564 + o];
                        }
                    }
                }
            },
        );
        // New pads: a copy of a pad of the same shape when there is one.
        let parsed: Vec<Option<Pad>> = recs
            .iter()
            .map(|r| {
                let bl: Vec<&[u8]> = r.blocks.iter().map(|b| b.as_slice()).collect();
                pcb_raw::pad(&bl, &layers)
            })
            .collect();
        let mut extra_regions: Vec<Region> = Vec::new();
        for (p, u) in ed.iter().zip(&used) {
            if *u {
                continue;
            }
            let tpl = parsed
                .iter()
                .position(|q| q.as_ref().is_some_and(|q| same_pad_shape(q, p)));
            let r = match tpl {
                Some(i) => {
                    let mut r = recs[i].clone();
                    let mut name = vec![p.name.len().min(255) as u8];
                    name.extend(p.name.bytes().take(255));
                    r.blocks[0] = name;
                    let b = &mut r.blocks[4];
                    b[1] &= !0x01;
                    Self::set_net(b, p.net.filter(|&n| (n as usize) < self.ed.nets.len()));
                    put_u16(b, 5, NONE16);
                    put_u16(
                        b,
                        7,
                        p.component
                            .filter(|&c| (c as usize) < self.ed.components.len())
                            .map_or(NONE16, |c| c as u16),
                    );
                    put_i32(b, 13, (p.at.x.0 as f64 / 2.54).round() as i32);
                    put_i32(b, 17, (p.at.y.0 as f64 / 2.54).round() as i32);
                    put_f64(b, 52, p.rotation_deg.rem_euclid(360.0));
                    r
                }
                None => {
                    let Some((rec, region)) = self.w.pad_record_of(p) else {
                        continue;
                    };
                    extra_regions.extend(region);
                    split(&rec, 6).remove(0)
                }
            };
            out.push(r);
            self.report.added += 1;
        }
        self.pad_regions = extra_regions;
        self.file.set_records("Pads6", out.len(), join(&out));
    }
}

fn same_style(a: &Via, b: &Via) -> bool {
    a.from_layer == b.from_layer
        && a.to_layer == b.to_layer
        && a.diameter == b.diameter
        && a.hole == b.hole
        && a.fab == b.fab
        && a.rings.len() == b.rings.len()
        && a.rings.iter().zip(&b.rings).all(|(x, y)| x == y)
}

fn same_pad_shape(a: &Pad, b: &Pad) -> bool {
    a.layer == b.layer
        && a.shape == b.shape
        && a.size_x == b.size_x
        && a.size_y == b.size_y
        && a.hole == b.hole
        && a.fab == b.fab
        && a.outline.is_empty() == b.outline.is_empty()
        && a.stack.len() == b.stack.len()
        && a.stack.iter().zip(&b.stack).all(|(x, y)| {
            x.layer == y.layer && x.shape == y.shape && x.size_x == y.size_x && x.size_y == y.size_y
        })
}

impl<'a> Merge<'a> {
    // --- copper and other areas ------------------------------------------------

    fn areas(&mut self) {
        let layers = Layers::new(&self.base.stackup);
        let base: Vec<Polygon> = self
            .base
            .geometry
            .polygons
            .iter()
            .chain(&self.base.geometry.areas)
            .cloned()
            .collect();
        let ed: Vec<Polygon> = self
            .ed
            .geometry
            .polygons
            .iter()
            .chain(&self.ed.geometry.areas)
            .cloned()
            .collect();
        let pour_net = |s: &Self, q: &Polygon| {
            q.net.or_else(|| {
                q.pour
                    .and_then(|i| s.base.geometry.pours.get(i as usize))
                    .and_then(|p| p.net)
            })
        };
        let exp = |s: &Self, q: &Polygon| {
            let mut e = s.exp_common(q, q.component, |q, c| q.component = c, move_area)?;
            e.net = pour_net(s, q);
            Some(e)
        };
        let mut used = vec![false; ed.len()];
        let mut regions_out = None;
        if let Some(data) = self.file.get("/Regions6/Data") {
            let recs = split(data, 1);
            let (out, u) = self.merge(
                "Regions6",
                recs,
                0,
                |r| pcb_raw::region(&r.blocks[0], &layers),
                &base,
                &ed,
                exp,
                |q| first(&q.outline),
                same_area,
                |q| q.net,
                |s, r, m| {
                    let b = &mut r.blocks[0];
                    Self::move_rings(b, m);
                    let (old, new) = (b[0], s.relayer_upto(b, m, 18));
                    if old != new {
                        if let Some(n) = v7_name(new) {
                            Self::edit_props(b, |t| t.set("V7_LAYER", n));
                        }
                    }
                },
            );
            used.iter_mut().zip(u).for_each(|(a, b)| *a |= b);
            regions_out = Some(out);
        }
        let mut fills_out = None;
        if let Some(data) = self.file.get("/Fills6/Data") {
            let recs = split(data, 1);
            let (out, u) = self.merge(
                "Fills6",
                recs,
                0,
                |r| pcb_raw::fill(&r.blocks[0], &layers),
                &base,
                &ed,
                exp,
                |q| first(&q.outline),
                same_area,
                |q| q.net,
                |s, r, m| {
                    let b = &mut r.blocks[0];
                    let (Some(ax), Some(ay), Some(bx), Some(by)) =
                        (i32_at(b, 13), i32_at(b, 17), i32_at(b, 21), i32_at(b, 25))
                    else {
                        return;
                    };
                    let c = [
                        (ax as f64 + bx as f64) / 20_000.0,
                        (ay as f64 + by as f64) / 20_000.0,
                    ];
                    let (hx, hy) = ((bx - ax) as f64 / 2.0, (by - ay) as f64 / 2.0);
                    let [u, v] = m.xy(c);
                    let (u, v) = (u * 10_000.0, v * 10_000.0);
                    put_i32(b, 13, (u - hx).round() as i32);
                    put_i32(b, 17, (v - hy).round() as i32);
                    put_i32(b, 21, (u + hx).round() as i32);
                    put_i32(b, 25, (v + hy).round() as i32);
                    Self::angle(b, 29, m);
                    s.relayer(b, m);
                },
            );
            used.iter_mut().zip(u).for_each(|(a, b)| *a |= b);
            fills_out = Some(out);
        }
        if let Some(out) = fills_out {
            self.file.set_records("Fills6", out.len(), join(&out));
        }

        // Shape-based regions: one for each region when the file keeps
        // them that way, which then change with their region.
        let regions_old = self.file.get("/Regions6/Data").map(|d| split(d, 1));
        let shapes_old = self.file.get("/ShapeBasedRegions6/Data").map(|d| split(d, 1));
        let twins = match (&regions_old, &shapes_old) {
            (Some(r), Some(s)) => r.len() == s.len() && !r.is_empty(),
            _ => false,
        };
        let Some(mut out) = regions_out else {
            return;
        };
        let mut shapes_out: Vec<Rec> = Vec::new();
        if twins {
            let renum = self.renumber.get("Regions6").cloned().unwrap_or_default();
            let olds = shapes_old.unwrap();
            let rold = regions_old.unwrap();
            let mut placed: Vec<Option<Rec>> = vec![None; out.len()];
            for (i, mut r) in olds.into_iter().enumerate() {
                let Some(Some(n)) = renum.get(i) else { continue };
                let b = &mut r.blocks[0];
                let comp = u16_at(&rold[i].blocks[0], 7)
                    .filter(|&c| c != NONE16)
                    .map(u32::from);
                // Net and part as the region now has them.
                let now = &out[*n as usize].blocks[0];
                if b.len() >= 9 && now.len() >= 9 {
                    b[3..5].copy_from_slice(&now[3..5]);
                    b[7..9].copy_from_slice(&now[7..9]);
                }
                if let Some(m) = self.mv(comp) {
                    Self::move_shape_rings(b, m);
                    let (old, new) = (b[0], self.relayer_upto(b, m, 18));
                    if old != new {
                        if let Some(nm) = v7_name(new) {
                            Self::edit_props(b, |t| t.set("V7_LAYER", nm));
                        }
                    }
                }
                placed[*n as usize] = Some(r);
            }
            shapes_out = placed.into_iter().flatten().collect();
            if shapes_out.len() != out.len() {
                // Could not keep them in step; Altium rebuilds them.
                shapes_out.clear();
            }
        }
        let mut new_regions: Vec<(Region, Option<u16>)> = Vec::new();
        for (q, u) in ed.iter().zip(&used) {
            if *u {
                continue;
            }
            if let Some(r) = self.w.area_region(q) {
                new_regions.push((r, q.pour.map(|p| p as u16)));
            }
        }
        for r in std::mem::take(&mut self.pad_regions) {
            new_regions.push((r, None));
        }
        for (r, pour) in &new_regions {
            out.push(split(&self.w.region_record(r, *pour), 1).remove(0));
            if twins && !shapes_out.is_empty() {
                shapes_out.push(split(&self.w.shape_region_record(r, *pour), 1).remove(0));
            }
            self.report.added += 1;
        }
        if twins {
            if shapes_out.len() == out.len() {
                self.file
                    .set_records("ShapeBasedRegions6", shapes_out.len(), join(&shapes_out));
            } else {
                self.file.set_records("ShapeBasedRegions6", 0, Vec::new());
                self.renumber.insert("ShapeBasedRegions6", Vec::new());
            }
            if !self.renumber.contains_key("ShapeBasedRegions6") {
                let r = self.renumber.get("Regions6").cloned().unwrap_or_default();
                self.renumber.insert("ShapeBasedRegions6", r);
            }
        }
        self.file.set_records("Regions6", out.len(), join(&out));
    }

    // --- texts ---------------------------------------------------------------

    fn texts(&mut self) {
        let Some(data) = self.file.get("/Texts6/Data") else {
            return;
        };
        let recs = split(data, 2);
        let mut wide = read_wide(self.file.get("/WideStrings6/Data").map(|v| v.as_slice()));
        let wide_before = wide.len();
        let mut wide_edits = 0;
        let mut out = Vec::new();
        let mut renum = vec![None; recs.len()];
        let mut designator_tpl: Option<Rec> = None;
        let mut comment_tpl: Option<Rec> = None;
        for (i, mut r) in recs.into_iter().enumerate() {
            let b = &mut r.blocks[0];
            let long = b.len() >= 123;
            if long && b[41] != 0 && designator_tpl.is_none() {
                designator_tpl = Some(r.clone());
            }
            if long && r.blocks[0][40] != 0 && comment_tpl.is_none() {
                comment_tpl = Some(r.clone());
            }
            let b = &mut r.blocks[0];
            let Some(comp) = self.relink(b) else {
                self.report.removed += 1;
                continue;
            };
            let mut changed = false;
            if let Some(m) = self.mv(comp) {
                Self::xy(b, 13, m);
                Self::angle(b, 27, m);
                self.relayer(b, m);
                // A flipped part's text reads mirrored, whatever its layer.
                if m.flips() && b.len() > 35 {
                    b[35] = u8::from(b[35] == 0);
                }
                changed = true;
            }
            // A renamed part's designator.
            if let Some(c) = comp.map(|c| c as usize) {
                let (Some(was), Some(now)) = (
                    self.base.components.get(c),
                    self.comp_map[c].and_then(|n| self.ed.components.get(n as usize)),
                ) else {
                    continue;
                };
                let b = &r.blocks[0];
                if b.len() >= 123
                    && b[41] != 0
                    && was.designator != now.designator
                    && !now.designator.is_empty()
                {
                    let old_text = pascal_str(&r.blocks[1]);
                    if old_text == was.designator {
                        r.blocks[1] = pascal(&now.designator);
                        if let Some(k) = u32_at(&r.blocks[0], 115) {
                            set_wide(&mut wide, k, &now.designator);
                            wide_edits += 1;
                        }
                        changed = true;
                    }
                }
            }
            self.count(changed);
            renum[i] = Some(out.len() as u32);
            out.push(r);
        }
        self.renumber.insert("Texts6", renum);

        // Designators and comments of new parts.
        let first_new = self.comp_map.iter().flatten().count();
        for j in first_new..self.ed.components.len() {
            let c = &self.ed.components[j];
            let comment = c
                .param(&["Comment", "Value"])
                .map(str::to_string)
                .unwrap_or_else(|| c.footprint.clone());
            for (tpl, text, flag) in [
                (&designator_tpl, c.designator.clone(), 41usize),
                (&comment_tpl, comment, 40),
            ] {
                let mut r = match tpl {
                    Some(t) => t.clone(),
                    None => {
                        let mut b = TEXT.to_vec();
                        b[40] = 0;
                        b[41] = 0;
                        b[flag] = 1;
                        Rec {
                            kind: 5,
                            blocks: vec![b, Vec::new()],
                        }
                    }
                };
                let b = &mut r.blocks[0];
                let layer = if c.side == Side::Bottom { 34 } else { 33 };
                if let (Some(a), Some(z)) = (v7_of(b[0]), v7_of(layer)) {
                    for k in 13..b.len().saturating_sub(3) {
                        if u32_at(b, k) == Some(a) {
                            put_u32(b, k, z);
                        }
                    }
                }
                b[0] = layer;
                b[1] &= !0x01;
                put_u16(b, 3, NONE16);
                put_u16(b, 5, NONE16);
                put_u16(b, 7, j as u16);
                put_i32(b, 13, (c.x.0 as f64 / 2.54).round() as i32);
                put_i32(b, 17, (c.y.0 as f64 / 2.54).round() as i32);
                put_f64(b, 27, c.rotation_deg.rem_euclid(360.0));
                b[35] = u8::from(c.side == Side::Bottom);
                if b.len() >= 123 {
                    let k = wide.iter().map(|w| w.0 + 1).max().unwrap_or(0);
                    put_u32(b, 115, k);
                    set_wide(&mut wide, k, &text);
                }
                r.blocks[1] = pascal(&text);
                out.push(r);
                self.report.added += 1;
            }
        }
        self.file.set_records("Texts6", out.len(), join(&out));
        if wide.len() != wide_before || wide_edits > 0 {
            let n = wide.len();
            self.file.set_records("WideStrings6", n, write_wide(&wide));
        }
    }

    // --- 3D bodies -------------------------------------------------------------

    fn bodies(&mut self) {
        for stream in ["ComponentBodies6", "ShapeBasedComponentBodies6"] {
            let Some(data) = self.file.get(&format!("/{stream}/Data")) else {
                continue;
            };
            let recs = split(data, 1);
            if recs.is_empty() {
                continue;
            }
            let shape = stream.starts_with("Shape");
            let mut out = Vec::new();
            for mut r in recs {
                let b = &mut r.blocks[0];
                let Some(comp) = self.relink(b) else {
                    self.report.removed += 1;
                    continue;
                };
                let mut changed = false;
                if let Some(m) = self.mv(comp) {
                    if shape {
                        Self::move_shape_rings(b, m);
                    } else {
                        Self::move_rings(b, m);
                    }
                    Self::edit_props(b, |t| {
                        let len = |k: &str| {
                            t.get(k).and_then(Length::parse).map(|l| l.mil())
                        };
                        if let (Some(x), Some(y)) = (len("MODEL.2D.X"), len("MODEL.2D.Y")) {
                            let [u, v] = m.xy([x, y]);
                            t.set("MODEL.2D.X", format!("{}mil", trim(u, 4)));
                            t.set("MODEL.2D.Y", format!("{}mil", trim(v, 4)));
                        }
                        let rot = t
                            .get("MODEL.2D.ROTATION")
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .unwrap_or(0.0);
                        t.set("MODEL.2D.ROTATION", format!("{:.3}", m.angle(rot)));
                        if m.flips() {
                            let bottom = t.get("BODYPROJECTION") == Some("1");
                            t.set("BODYPROJECTION", if bottom { "0" } else { "1" });
                        }
                    });
                    changed = true;
                }
                self.count(changed);
                out.push(r);
            }
            let first_new = self.comp_map.iter().flatten().count();
            if self.ed.components.len() > first_new
                && self
                    .ed
                    .geometry
                    .bodies
                    .iter()
                    .any(|b| b.component.is_some_and(|c| c as usize >= first_new))
                && !shape
            {
                self.report
                    .notes
                    .push("3D models of new parts are not carried over".into());
            }
            self.file.set_records(stream, out.len(), join(&out));
        }
    }

    // --- pours -----------------------------------------------------------------

    fn polygons(&mut self) {
        let Some(data) = self.file.get("/Polygons6/Data") else {
            return;
        };
        let mut recs = split_text(data, 0);
        for (_, t) in &mut recs {
            let digits: String = t
                .get("NET")
                .unwrap_or("")
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u32>() {
                if (n as usize) < self.net_map.len() {
                    let nn = self.new_net(Some(n)).unwrap_or(NONE16 as u32);
                    if nn != n {
                        t.set("NET", nn.to_string());
                    }
                }
            }
        }
        if self.ed.geometry.pours.len() > recs.len() {
            self.report.notes.push(format!(
                "{} new pours kept as copper areas only",
                self.ed.geometry.pours.len() - recs.len()
            ));
        }
        let n = recs.len();
        self.file.set_records("Polygons6", n, join_text(&recs));
    }

    // --- part parameters -----------------------------------------------------

    fn params(&mut self) {
        let Some(data) = self.file.get("/PrimitiveParameters/Data") else {
            return;
        };
        let recs = split_text(data, 0);
        // Groups: a record naming its owner, then its parameters.
        let mut groups: Vec<Vec<(Vec<u8>, Text)>> = Vec::new();
        for r in recs {
            if r.1.get("ID").is_some() || groups.is_empty() {
                groups.push(vec![r]);
            } else {
                groups.last_mut().unwrap().push(r);
            }
        }
        let mut out = Vec::new();
        for mut g in groups {
            let id = g[0].1.get("ID").unwrap_or("").to_string();
            if let Some(n) = id.strip_prefix("Component#").and_then(|n| n.parse::<usize>().ok()) {
                let Some(Some(nn)) = self.comp_map.get(n) else {
                    continue;
                };
                g[0].1.set("ID", format!("Component#{nn}"));
                let plain = g[0].1.get("VARIANTGUID").unwrap_or("").is_empty()
                    && g[0].1.get("APPURTENANCE").is_none();
                if plain {
                    let (was, now) = (&self.base.components[n].params, &self.ed.components[*nn as usize].params);
                    if was != now {
                        let head = g[0].clone();
                        g = vec![head];
                        for (k, v) in now {
                            g.push((
                                Vec::new(),
                                Text {
                                    fields: vec![("NAME".into(), k.clone()), ("VALUE".into(), v.clone())],
                                    nul: true,
                                },
                            ));
                        }
                        let count = g.len() - 1;
                        g[0].1.set("COUNT", count.to_string());
                    }
                }
            }
            out.extend(g);
        }
        // The header counts owners, not records.
        let n = out.iter().filter(|(_, t)| t.get("ID").is_some()).count();
        self.file.set_records("PrimitiveParameters", n, join_text(&out));
    }

    // --- tables that point at records by position -----------------------------

    fn side_tables(&mut self) {
        let changed = self.report.changed + self.report.added + self.report.removed > 0;
        // Pad unique ids.
        if let Some(data) = self.file.get("/UniqueIDPrimitiveInformation/Data") {
            let mut recs = split_text(data, 0);
            recs.retain_mut(|(_, t)| {
                let stream = match t.get("PRIMITIVEOBJECTID") {
                    Some("Pad") => "Pads6",
                    Some("Via") => "Vias6",
                    Some("Track") => "Tracks6",
                    Some("Arc") => "Arcs6",
                    Some("Text") => "Texts6",
                    Some("Region") => "Regions6",
                    Some("Fill") => "Fills6",
                    _ => return true,
                };
                let Some(map) = self.renumber.get(stream) else {
                    return true;
                };
                let Some(i) = t.get("PRIMITIVEINDEX").and_then(|v| v.parse::<usize>().ok()) else {
                    return true;
                };
                match map.get(i) {
                    Some(Some(n)) => {
                        t.set("PRIMITIVEINDEX", n.to_string());
                        true
                    }
                    _ => false,
                }
            });
            let n = recs.len();
            self.file.set_records("UniqueIDPrimitiveInformation", n, join_text(&recs));
        }
        // GUIDs: (kind, index, 16-byte guid).
        let guid_streams: [(u32, &str); 11] = [
            (0x0d01, "Arcs6"),
            (0x0e02, "Pads6"),
            (0x0f03, "Vias6"),
            (0x1004, "Tracks6"),
            (0x1105, "Texts6"),
            (0x1206, "Fills6"),
            (0x0208, "Nets6"),
            (0x0309, "Components6"),
            (0x1359, "Regions6"),
            (0x1459, "ShapeBasedRegions6"),
            (0x0807, "Connections6"),
        ];
        if changed {
            self.file.set_records("Connections6", 0, Vec::new());
            self.renumber.insert("Connections6", Vec::new());
        }
        if let Some(data) = self.file.get("/PrimitiveGuids/Data").cloned() {
            let mut out = Vec::new();
            for e in data.chunks_exact(24) {
                let kind = u32_at(e, 0).unwrap();
                let idx = u32_at(e, 4).unwrap() as usize;
                let stream = guid_streams.iter().find(|(k, _)| *k == kind).map(|s| s.1);
                let new = match stream.and_then(|s| self.renumber.get(s)) {
                    Some(map) => match map.get(idx) {
                        Some(Some(n)) => *n,
                        _ => continue,
                    },
                    None => idx as u32,
                };
                out.extend_from_slice(&e[..4]);
                out.extend_from_slice(&new.to_le_bytes());
                out.extend_from_slice(&e[8..]);
            }
            let n = out.len() / 24;
            self.file.set_records("PrimitiveGuids", n, out);
        }
        // Pad and via library links: (0, index, kind 2 pad / 3 via, entry).
        if let Some(data) = self.file.get("/PadViaCacheLibraryLinksSection/Data").cloned() {
            let mut out = Vec::new();
            for e in data.chunks_exact(10) {
                let idx = u32_at(e, 1).unwrap() as usize;
                let stream = match e[5] {
                    2 => "Pads6",
                    3 => "Vias6",
                    _ => "",
                };
                let new = match self.renumber.get(stream) {
                    Some(map) => match map.get(idx) {
                        Some(Some(n)) => *n,
                        _ => continue,
                    },
                    None => idx as u32,
                };
                out.push(e[0]);
                out.extend_from_slice(&new.to_le_bytes());
                out.extend_from_slice(&e[5..]);
            }
            let n = out.len() / 10;
            self.file.set_records("PadViaCacheLibraryLinksSection", n, out);
        }
    }

    // --- classes, pairs, rules -------------------------------------------------

    fn classes(&mut self) {
        let Some(data) = self.file.get("/Classes6/Data") else {
            return;
        };
        let mut recs = split_text(data, 0);
        let lists: [(i64, &Vec<NetClass>, &Vec<NetClass>); 3] = [
            (0, &self.base.net_classes, &self.ed.net_classes),
            (1, &self.base.component_classes, &self.ed.component_classes),
            (6, &self.base.diff_pair_classes, &self.ed.diff_pair_classes),
        ];
        let mut keep = vec![true; recs.len()];
        let mut extra = Vec::new();
        for (kind, was, now) in lists {
            for (i, (_, t)) in recs.iter_mut().enumerate() {
                if t.get("KIND").and_then(|k| k.parse::<i64>().ok()) != Some(kind) {
                    continue;
                }
                let name = t.get("NAME").unwrap_or("").to_string();
                let before = was.iter().find(|c| c.name == name);
                match now.iter().find(|c| c.name == name) {
                    Some(c) => {
                        if before.map(|b| &b.members) != Some(&c.members) {
                            t.remove_where(|k| {
                                k.len() > 1
                                    && k.starts_with(['M', 'm'])
                                    && k[1..].chars().all(|c| c.is_ascii_digit())
                            });
                            for (k, m) in c.members.iter().enumerate() {
                                t.set(&format!("M{k}"), m.clone());
                            }
                            self.report.changed += 1;
                        }
                    }
                    None if before.is_some() => {
                        keep[i] = false;
                        self.report.removed += 1;
                    }
                    None => {}
                }
            }
            let template = recs.iter().find(|(_, t)| {
                t.get("KIND").and_then(|k| k.parse::<i64>().ok()) == Some(kind)
                    && t.get("SUPERCLASS") != Some("TRUE")
            });
            for c in now.iter().filter(|c| !was.iter().any(|b| b.name == c.name)) {
                let mut t = match template {
                    Some((_, t)) => t.clone(),
                    None => Text {
                        fields: vec![
                            ("SELECTION".into(), "FALSE".into()),
                            ("LAYER".into(), "TOP".into()),
                            ("LOCKED".into(), "FALSE".into()),
                            ("KIND".into(), kind.to_string()),
                            ("SUPERCLASS".into(), "FALSE".into()),
                        ],
                        nul: true,
                    },
                };
                t.remove_where(|k| {
                    k.len() > 1 && k.starts_with(['M', 'm']) && k[1..].chars().all(|c| c.is_ascii_digit())
                });
                t.set("NAME", c.name.clone());
                for (k, m) in c.members.iter().enumerate() {
                    t.set(&format!("M{k}"), m.clone());
                }
                let id = self.w.unique_id();
                t.set("UNIQUEID", id);
                extra.push((Vec::new(), t));
                self.report.added += 1;
            }
        }
        let mut out: Vec<(Vec<u8>, Text)> = recs
            .into_iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(r, _)| r)
            .collect();
        out.extend(extra);
        let n = out.len();
        self.file.set_records("Classes6", n, join_text(&out));
    }

    fn diff_pairs(&mut self) {
        let data = self
            .file
            .get("/DifferentialPairs6/Data")
            .cloned()
            .unwrap_or_default();
        let recs = split_text(&data, 0);
        if recs.is_empty() && self.ed.diff_pairs.is_empty() {
            return;
        }
        let mut out = Vec::new();
        for (pre, mut t) in recs.iter().cloned() {
            let name = t.get("NAME").unwrap_or("").to_string();
            match self.ed.diff_pairs.iter().find(|d| d.name == name) {
                Some(d) => {
                    let mut changed = false;
                    if t.get("POSITIVENETNAME") != Some(d.positive.as_str()) {
                        t.set("POSITIVENETNAME", d.positive.clone());
                        changed = true;
                    }
                    if t.get("NEGATIVENETNAME") != Some(d.negative.as_str()) {
                        t.set("NEGATIVENETNAME", d.negative.clone());
                        changed = true;
                    }
                    self.count(changed);
                    out.push((pre, t));
                }
                None if self.base.diff_pairs.iter().any(|d| d.name == name) => {
                    self.report.removed += 1;
                }
                None => out.push((pre, t)),
            }
        }
        for d in &self.ed.diff_pairs {
            if recs.iter().any(|(_, t)| t.get("NAME") == Some(d.name.as_str())) {
                continue;
            }
            let id = self.w.unique_id();
            out.push((
                Vec::new(),
                Text {
                    fields: vec![
                        ("SELECTION".into(), "FALSE".into()),
                        ("LAYER".into(), "TOP".into()),
                        ("LOCKED".into(), "FALSE".into()),
                        ("POLYGONOUTLINE".into(), "FALSE".into()),
                        ("USERROUTED".into(), "TRUE".into()),
                        ("KEEPOUT".into(), "FALSE".into()),
                        ("UNIONINDEX".into(), "0".into()),
                        ("NAME".into(), d.name.clone()),
                        ("UNIQUEID".into(), id),
                        ("POSITIVENETNAME".into(), d.positive.clone()),
                        ("NEGATIVENETNAME".into(), d.negative.clone()),
                        ("GATHERCONTROL".into(), "FALSE".into()),
                    ],
                    nul: true,
                },
            ));
            self.report.added += 1;
        }
        let n = out.len();
        self.file.set_records("DifferentialPairs6", n, join_text(&out));
    }

    fn rules(&mut self) {
        let Some(data) = self.file.get("/Rules6/Data") else {
            return;
        };
        let recs = split_text(data, 2);
        let mut out = Vec::new();
        let rule_key = |r: &Rule| (r.kind.to_ascii_uppercase(), r.name.clone());
        for (pre, mut t) in recs.iter().cloned() {
            let kind = t.get("RULEKIND").unwrap_or("").to_ascii_uppercase();
            let name = t.get("NAME").unwrap_or("").to_string();
            let find = |rs: &'a [Rule]| rs.iter().find(|r| rule_key(r) == (kind.clone(), name.clone()));
            let (was, now) = (find(&self.base.rules), find(&self.ed.rules));
            match (was, now) {
                (Some(w), Some(n)) => {
                    let mut changed = false;
                    if w.enabled != n.enabled {
                        t.set("ENABLED", if n.enabled { "TRUE" } else { "FALSE" });
                        changed = true;
                    }
                    if w.priority != n.priority {
                        t.set("PRIORITY", n.priority.to_string());
                        changed = true;
                    }
                    if w.scope != n.scope {
                        for (k, key) in ["SCOPE1EXPRESSION", "SCOPE2EXPRESSION"].iter().enumerate() {
                            if let Some(s) = n.scope.get(k) {
                                t.set(key, s.clone());
                            }
                        }
                        changed = true;
                    }
                    for (k, v) in &n.params {
                        if w.params.get(k) != Some(v) {
                            t.set(k, v.clone());
                            changed = true;
                        }
                    }
                    self.count(changed);
                    out.push((pre, t));
                }
                (Some(_), None) => self.report.removed += 1,
                _ => out.push((pre, t)),
            }
        }
        // New rules, from a rule of the same kind.
        for r in &self.ed.rules {
            if self.base.rules.iter().any(|b| rule_key(b) == rule_key(r)) {
                continue;
            }
            let Some((pre, tpl)) = recs.iter().find(|(_, t)| {
                t.get("RULEKIND")
                    .is_some_and(|k| k.eq_ignore_ascii_case(&r.kind))
            }) else {
                self.report
                    .notes
                    .push(format!("rule {} ({}) has no like rule to copy, left out", r.name, r.kind));
                continue;
            };
            let mut t = tpl.clone();
            t.set("NAME", r.name.clone());
            t.set("ENABLED", if r.enabled { "TRUE" } else { "FALSE" });
            t.set("PRIORITY", r.priority.to_string());
            for (k, key) in ["SCOPE1EXPRESSION", "SCOPE2EXPRESSION"].iter().enumerate() {
                t.set(key, r.scope.get(k).cloned().unwrap_or_else(|| "All".into()));
            }
            for (k, v) in &r.params {
                t.set(k, v.clone());
            }
            let id = self.w.unique_id();
            t.set("UNIQUEID", id);
            out.push((pre.clone(), t));
            self.report.added += 1;
        }
        let n = out.len();
        self.file.set_records("Rules6", n, join_text(&out));
    }

    // --- the board record -------------------------------------------------------

    fn board6(&mut self) {
        let Some(data) = self.file.get("/Board6/Data") else {
            return;
        };
        let mut recs = split_text(data, 0);
        let Some((_, t)) = recs.first_mut() else { return };
        let (was, now) = (&self.base.stackup.layers, &self.ed.stackup.layers);
        let mut changed = false;
        if was.len() == now.len() {
            for (i, (a, b)) in was.iter().zip(now).enumerate() {
                let key = |f: &str| format!("V9_STACK_LAYER{i}_{f}");
                if a.name != b.name {
                    t.set(&key("NAME"), b.name.clone());
                    changed = true;
                }
                if a.thickness != b.thickness {
                    let k = if b.kind == LayerKind::Copper {
                        "COPTHICK"
                    } else {
                        "DIELHEIGHT"
                    };
                    t.set(&key(k), format!("{}mil", trim(b.thickness.mil(), 4)));
                    changed = true;
                }
                if a.dielectric_constant != b.dielectric_constant {
                    if let Some(e) = b.dielectric_constant {
                        t.set(&key("DIELCONST"), trim(e, 3));
                        changed = true;
                    }
                }
                if a.material != b.material {
                    if let Some(m) = &b.material {
                        t.set(&key("DIELMATERIAL"), m.clone());
                        changed = true;
                    }
                }
            }
        } else {
            self.report
                .notes
                .push("layers added or removed in the stack-up are not written".into());
        }
        if self.base.geometry.outline.len() != self.ed.geometry.outline.len()
            || !near_ring(&self.base.geometry.outline, &self.ed.geometry.outline)
        {
            self.report
                .notes
                .push("the board shape is kept as it was".into());
        }
        self.count(changed);
        self.file.set_records("Board6", 1, join_text(&recs[..1]));
    }
}

fn pascal(s: &str) -> Vec<u8> {
    let b: Vec<u8> = s
        .chars()
        .take(255)
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect();
    let mut out = vec![b.len() as u8];
    out.extend(b);
    out
}

fn pascal_str(b: &[u8]) -> String {
    let n = b.first().copied().unwrap_or(0) as usize;
    b.get(1..1 + n)
        .unwrap_or(&[])
        .iter()
        .map(|&c| c as char)
        .collect()
}

/// `WideStrings6`: index u32, byte length u32, UTF-16 text with a closing
/// zero, repeated.
/// Entries kept as stored (index and raw UTF-16 bytes), so the ones not
/// changed are written back exactly.
fn read_wide(d: Option<&[u8]>) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let Some(d) = d else { return out };
    let mut i = 0;
    while i + 8 <= d.len() {
        let (k, n) = (u32_at(d, i).unwrap(), u32_at(d, i + 4).unwrap() as usize);
        let Some(bytes) = d.get(i + 8..i + 8 + n) else {
            break;
        };
        out.push((k, bytes.to_vec()));
        i += 8 + n;
    }
    out
}

fn set_wide(w: &mut Vec<(u32, Vec<u8>)>, k: u32, s: &str) {
    let mut units: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    units.extend_from_slice(&[0, 0]);
    match w.iter_mut().find(|e| e.0 == k) {
        Some(e) => e.1 = units,
        None => w.push((k, units)),
    }
}

fn write_wide(w: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, units) in w {
        out.extend_from_slice(&k.to_le_bytes());
        out.extend_from_slice(&(units.len() as u32).to_le_bytes());
        out.extend_from_slice(units);
    }
    out
}


/// Differences between a board that was saved and the board read back
/// from the file: objects missing on either side, nets that differ. Text
/// drawn as strokes is left out (it is kept as text). Empty when the file
/// holds the board as saved.
pub fn compare(saved: &Board, back: &Board) -> Vec<String> {
    let mut out = Vec::new();
    let name = |b: &Board, n: Option<u32>| {
        n.and_then(|n| b.nets.get(n as usize)).map(|n| n.name.clone())
    };
    fn check<T>(
        out: &mut Vec<String>,
        what: &str,
        a: &[T],
        b: &[T],
        anchor: impl Fn(&T) -> Point + Copy,
        same: impl Fn(&T, &T) -> bool + Copy,
        net: impl Fn(&T, bool) -> Option<String>,
    ) {
        let mut used = vec![false; b.len()];
        let m = pair(a, b, anchor, same, &mut used);
        let missing = m.iter().filter(|m| m.is_none()).count();
        let extra = used.iter().filter(|u| !**u).count();
        let nets = m
            .iter()
            .enumerate()
            .filter(|(i, j)| j.is_some_and(|j| net(&a[*i], true) != net(&b[j], false)))
            .count();
        if missing + extra + nets > 0 {
            out.push(format!(
                "{what}: {missing} missing, {extra} extra, {nets} on another net"
            ));
        }
    }
    let (g, h) = (&saved.geometry, &back.geometry);
    let real = |v: &[Track]| v.iter().filter(|t| !t.text).cloned().collect::<Vec<_>>();
    check(&mut out, "tracks", &real(&g.tracks), &real(&h.tracks), |t| t.start, same_track, |t, s| {
        name(if s { saved } else { back }, t.net)
    });
    check(&mut out, "arcs", &g.arcs, &h.arcs, |a| a.center, same_arc, |t, s| {
        name(if s { saved } else { back }, t.net)
    });
    check(&mut out, "vias", &g.vias, &h.vias, |v| v.at, same_via, |t, s| {
        name(if s { saved } else { back }, t.net)
    });
    check(&mut out, "pads", &g.pads, &h.pads, |p| p.at, same_pad, |t, s| {
        name(if s { saved } else { back }, t.net)
    });
    let areas = |g: &Geometry| g.polygons.iter().chain(&g.areas).cloned().collect::<Vec<_>>();
    check(
        &mut out,
        "areas",
        &areas(g),
        &areas(h),
        |q| first(&q.outline),
        same_area,
        |_, _| None,
    );
    if saved.components.len() != back.components.len() {
        out.push(format!(
            "parts: {} saved, {} read back",
            saved.components.len(),
            back.components.len()
        ));
    } else {
        let moved = saved
            .components
            .iter()
            .zip(&back.components)
            .filter(|(a, b)| {
                (a.x.0 - b.x.0).abs() > TOL
                    || (a.y.0 - b.y.0).abs() > TOL
                    || a.side != b.side
                    || !near_deg(a.rotation_deg, b.rotation_deg)
                    || a.designator != b.designator
            })
            .count();
        if moved > 0 {
            out.push(format!("parts: {moved} placed or named differently"));
        }
    }
    let names = |b: &Board| b.nets.iter().map(|n| n.name.clone()).collect::<Vec<_>>();
    if names(saved) != names(back) {
        out.push("nets: the net list differs".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::mil(x, y)
    }

    /// A small board, saved fresh, then edited and saved over itself.
    #[test]
    fn edits_are_saved_over_the_original() {
        let mut b = Board::default();
        for n in ["GND", "SIG"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        b.components.push(Component {
            designator: "R1".into(),
            footprint: "R0603".into(),
            library_ref: None,
            x: Length::from_mil(1000.0),
            y: Length::from_mil(1000.0),
            rotation_deg: 0.0,
            side: Side::Top,
            sheet: None,
            params: Default::default(),
        });
        let pad = |name: &str, x: f64, net| Pad {
            layer: LayerRef::Copper(0),
            net: Some(net),
            component: Some(0),
            name: name.into(),
            at: pt(x, 1000.0),
            size_x: Length::from_mil(30.0),
            size_y: Length::from_mil(30.0),
            hole: Length(0),
            shape: PadShape::Rect,
            rotation_deg: 0.0,
            outline: Vec::new(),
            stack: Vec::new(),
            fab: Fab::default(),
        };
        let g = &mut b.geometry;
        g.outline = vec![pt(0.0, 0.0), pt(2000.0, 0.0), pt(2000.0, 2000.0), pt(0.0, 2000.0)];
        g.pads.push(pad("1", 970.0, 0));
        g.pads.push(pad("2", 1030.0, 1));
        for k in 0..3 {
            g.tracks.push(Track {
                layer: LayerRef::Copper(0),
                net: Some(1),
                start: pt(1030.0, 1000.0 + k as f64 * 100.0),
                end: pt(1500.0, 1000.0 + k as f64 * 100.0),
                width: Length::from_mil(8.0),
                component: None,
                text: false,
            });
        }
        let dir = tempfile::tempdir().unwrap();
        let orig = dir.path().join("a.PcbDoc");
        std::fs::write(&orig, crate::to_altium(&b, "a.PcbDoc").unwrap()).unwrap();
        let mut b = trace_altium::read_pcbdoc(&orig).unwrap();

        // Unchanged: every stream as it was.
        let (same, r) = to_altium_over(&orig, &b, "a.PcbDoc").unwrap();
        assert_eq!((r.changed, r.added, r.removed), (0, 0, 0));
        let (a, c) = (Cfb::read(std::fs::read(&orig).unwrap()).unwrap(), Cfb::read(same).unwrap());
        assert_eq!(a.streams, c.streams);

        // Turn the part, drop a track, add one, move a pad to GND.
        let from = Placement::of(&b, 0).unwrap();
        trace_core::place::place(
            &mut b,
            0,
            from,
            Placement {
                rotation_deg: 90.0,
                ..from
            },
        );
        b.geometry.tracks.remove(1);
        let mut t = b.geometry.tracks[0].clone();
        t.start.y.0 += 1_000_000;
        t.end.y.0 += 1_000_000;
        b.geometry.tracks.push(t);
        b.geometry.pads[1].net = Some(0);
        let (bytes, r) = to_altium_over(&orig, &b, "a.PcbDoc").unwrap();
        assert!(r.added >= 1 && r.removed >= 1 && r.changed >= 2, "{r:?}");
        let out = dir.path().join("b.PcbDoc");
        std::fs::write(&out, bytes).unwrap();
        let back = trace_altium::read_pcbdoc(&out).unwrap();
        assert_eq!(compare(&b, &back), Vec::<String>::new());
        assert!((back.components[0].rotation_deg - 90.0).abs() < 1e-9);
    }
}
