//! Bill of materials with part data: the board's parts grouped into
//! lines, joined with each part number's maker data (lifecycle, stock,
//! price breaks, datasheet) from a parts file, a cache or a distributor,
//! and checked for what stops a build: no part number, obsolete or
//! not-for-new-design parts, too little stock, one part number on two
//! footprints, and lines that could be one.

use crate::{Board, Component};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

/// Where a part is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Lifecycle {
    #[default]
    Unknown,
    Active,
    New,
    /// Not recommended for new designs.
    Nrnd,
    /// End of life announced: last time buy.
    LastTimeBuy,
    Obsolete,
}

impl Lifecycle {
    pub fn word(self) -> &'static str {
        match self {
            Lifecycle::Unknown => "",
            Lifecycle::Active => "Active",
            Lifecycle::New => "New",
            Lifecycle::Nrnd => "NRND",
            Lifecycle::LastTimeBuy => "End of life",
            Lifecycle::Obsolete => "Obsolete",
        }
    }

    /// Reads a lifecycle as distributors and part lists write it.
    pub fn parse(s: &str) -> Lifecycle {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() {
            Lifecycle::Unknown
        } else if t.contains("obsolete") || t.contains("discontinued") {
            Lifecycle::Obsolete
        } else if t.contains("not recommended") || t.contains("nrnd") {
            Lifecycle::Nrnd
        } else if t.contains("end of life")
            || t == "eol"
            || t.contains("last time")
            || t.contains("ltb")
        {
            Lifecycle::LastTimeBuy
        } else if t.contains("new") || t.contains("preview") {
            Lifecycle::New
        } else if t.contains("active") || t.contains("production") || t == "ok" {
            Lifecycle::Active
        } else {
            Lifecycle::Unknown
        }
    }
}

/// What is known about one maker's part number.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartData {
    pub mpn: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Units in stock at the source.
    #[serde(default)]
    pub stock: Option<u64>,
    /// Price breaks: (from quantity, unit price).
    #[serde(default)]
    pub prices: Vec<(u64, f64)>,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub datasheet: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub lead_time: String,
    /// The maker's suggested replacement.
    #[serde(default)]
    pub replacement: String,
    /// Where it came from: "parts file", "Mouser" ...
    #[serde(default)]
    pub source: String,
    /// When it was looked up (YYYY-MM-DD), for the cache.
    #[serde(default)]
    pub fetched: String,
}

impl PartData {
    /// Unit price when buying `qty`.
    pub fn unit_price(&self, qty: u64) -> Option<f64> {
        let q = qty.max(1);
        self.prices
            .iter()
            .filter(|p| p.0 <= q)
            .max_by_key(|p| p.0)
            .or_else(|| self.prices.iter().min_by_key(|p| p.0))
            .map(|p| p.1)
    }
}

/// Kinds of BOM problems.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flag {
    NoPartNumber,
    Obsolete,
    LastTimeBuy,
    Nrnd,
    OutOfStock,
    LowStock,
    FootprintConflict,
    NoData,
    NoPrice,
    Generic,
    OneSource,
    Consolidate,
    Alternate,
}

/// How much a flag matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Flag {
    pub fn word(self) -> &'static str {
        match self {
            Flag::NoPartNumber => "no part number",
            Flag::Obsolete => "obsolete",
            Flag::LastTimeBuy => "end of life",
            Flag::Nrnd => "not for new designs",
            Flag::OutOfStock => "not enough stock",
            Flag::LowStock => "low stock",
            Flag::FootprintConflict => "part number on two footprints",
            Flag::NoData => "no part data",
            Flag::NoPrice => "no price",
            Flag::Generic => "generic part",
            Flag::OneSource => "one source",
            Flag::Consolidate => "could share a part number",
            Flag::Alternate => "alternative footprint left off",
        }
    }

    pub fn level(self) -> Level {
        match self {
            Flag::NoPartNumber | Flag::Obsolete | Flag::OutOfStock => Level::Error,
            Flag::LastTimeBuy
            | Flag::Nrnd
            | Flag::LowStock
            | Flag::NoData
            | Flag::NoPrice
            | Flag::FootprintConflict => Level::Warning,
            Flag::Generic | Flag::OneSource | Flag::Consolidate | Flag::Alternate => Level::Info,
        }
    }
}

/// A problem on a BOM line.
#[derive(Clone, Debug)]
pub struct Issue {
    pub flag: Flag,
    pub text: String,
}

/// One BOM line: parts alike in part number, value and footprint.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub designators: Vec<String>,
    /// Indices of the parts in board.components.
    pub parts: Vec<usize>,
    pub value: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    /// The footprints its parts use, joined.
    pub footprint: String,
    pub footprints: Vec<String>,
    /// The part number is only the library's code for the part (Allegro
    /// device type, library reference), not a maker's number.
    pub code: bool,
    /// Other makers' numbers given for the part (Manufacturer 2 ...).
    pub alternates: Vec<(String, String)>,
    pub data: Option<PartData>,
    pub issues: Vec<Issue>,
}

impl Line {
    pub fn qty(&self) -> usize {
        self.designators.len()
    }

    /// Designator letters: R, C, U ...
    pub fn prefix(&self) -> String {
        prefix(self.designators.first().map_or("", |s| s.as_str()))
    }

    pub fn worst(&self) -> Option<Level> {
        self.issues.iter().map(|i| i.flag.level()).max()
    }

    /// Not a maker's part number to look up: a library code or a
    /// generic part.
    pub fn generic(&self) -> bool {
        self.code || generic(&self.manufacturer, &self.mpn)
    }

    pub fn has(&self, f: Flag) -> bool {
        self.issues.iter().any(|i| i.flag == f)
    }
}

/// The whole BOM.
#[derive(Clone, Debug, Default)]
pub struct Bom {
    pub lines: Vec<Line>,
    /// Parts left off (variant or marked not fitted).
    pub not_fitted: Vec<String>,
    /// Parts not bought: no pads, no designator, or marked not for BOM.
    pub left_out: Vec<String>,
    /// Footprints stacked on the same pads as alternatives: (the one
    /// counted, the one left off).
    pub stacked: Vec<(String, String)>,
    /// Boards the quantities and stock are for.
    pub boards: u64,
    /// Variant the BOM is for ("" = as designed).
    pub variant: String,
}

fn prefix(d: &str) -> String {
    d.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Passive and simple parts many makers build alike.
fn commodity(prefix: &str) -> bool {
    matches!(
        prefix,
        "R" | "C" | "L" | "FB" | "E" | "RN" | "TP" | "B" | "H" | "MH" | "FID" | "J" | "P"
    )
}

const MPN_KEYS: [&str; 8] = [
    "Manufacturer Part Number",
    "Manufacturer Part Number 1",
    "Manufacturer_Part_Number",
    "MPN",
    "MFR_PN",
    "Mfr Part Number",
    "PartNumber",
    "Part Number",
];
const MAKER_KEYS: [&str; 6] = [
    "Manufacturer",
    "Manufacturer 1",
    "Manufacturer Name",
    "Manufacturer_Name",
    "MFR",
    "Mfr",
];

fn truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "yes" | "true" | "y" | "x" | "dnp" | "dni"
    )
}

/// Marked in the design as not fitted.
fn marked_not_fitted(c: &Component) -> bool {
    if c.param(&["DNP", "DNI", "Not Fitted", "NotFitted", "No Pop", "NoPop"])
        .is_some_and(truthy)
    {
        return true;
    }
    if c.param(&["Fitted", "Populate", "Populated", "Assembly"])
        .is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "no" | "false" | "0" | "dnp" | "not fitted"
            )
        })
    {
        return true;
    }
    let v = c
        .param(&["Value", "Comment"])
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    matches!(v.as_str(), "NOT FITTED" | "DNF") || !v.is_empty() && crate::pathloss::placeholder(&v)
}

/// A value that is a maker's part number rather than "10uF" or "4.7K":
/// letters and digits, no spaces, and not a number with a unit.
fn part_number_like(v: &str) -> bool {
    let v = v.trim();
    if v.len() < 6 || v.contains(char::is_whitespace) {
        return false;
    }
    let digits = v.chars().filter(char::is_ascii_digit).count();
    let letters = v.chars().filter(char::is_ascii_alphabetic).count();
    if letters == 0 {
        return digits >= 8 && digits == v.len();
    }
    if digits < 2 || letters < 2 {
        return false;
    }
    if v.starts_with(|c: char| c.is_ascii_digit()) {
        let rest = v.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        if !rest.chars().any(|c| c.is_ascii_digit()) || rest.contains([',', '/', '%']) {
            return false;
        }
    }
    true
}

/// A library code without its footprint tail: Allegro device types end
/// in the footprint ("CAP,0.1UF,E011567,C0201-C0201" and
/// "...,E011567,C0201-(C0201-2),C0201" are one part).
fn strip_footprint(code: &str, footprint: &str) -> String {
    let fp = footprint.trim().to_ascii_uppercase();
    let mut fields: Vec<&str> = code.split(',').collect();
    while fields.len() > 1 {
        let last = fields[fields.len() - 1]
            .trim()
            .to_ascii_uppercase()
            .replace(['(', ')'], "");
        let head = last.split('-').next().unwrap_or("").to_string();
        let fits = !fp.is_empty()
            && (last.contains(&fp)
                || head.len() >= 3 && fp.starts_with(&head)
                || fp.contains(&last));
        if !fits {
            break;
        }
        fields.pop();
    }
    fields.join(",").trim().to_string()
}

/// Pairs of parts stacked on the same pads as alternatives: two or more
/// pads of one sit on pads of the other (same layer, centres closer
/// than half the larger pad).
fn stacked_pairs(board: &Board) -> Vec<(usize, usize)> {
    let g = &board.geometry;
    let cell = 40.0;
    let cell_of = |p: &crate::Point| {
        (
            (p.x.mil() / cell).floor() as i64,
            (p.y.mil() / cell).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in g.pads.iter().enumerate() {
        if p.component.is_some() {
            grid.entry(cell_of(&p.at)).or_default().push(i);
        }
    }
    let mut count: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, p) in g.pads.iter().enumerate() {
        let Some(ca) = p.component else { continue };
        let k = cell_of(&p.at);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid.get(&(k.0 + dx, k.1 + dy)).into_iter().flatten() {
                    let q = &g.pads[j];
                    let Some(cb) = q.component else { continue };
                    if j <= i || ca == cb || p.layer != q.layer {
                        continue;
                    }
                    let r = 0.5
                        * p.size_x
                            .mil()
                            .max(p.size_y.mil())
                            .max(q.size_x.mil())
                            .max(q.size_y.mil());
                    let d = (p.at.x.mil() - q.at.x.mil()).hypot(p.at.y.mil() - q.at.y.mil());
                    if d < r {
                        *count
                            .entry((ca.min(cb) as usize, ca.max(cb) as usize))
                            .or_default() += 1;
                    }
                }
            }
        }
    }
    count
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(k, _)| k)
        .collect()
}

/// A part number field holding no part number.
fn placeholder(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_uppercase().as_str(),
        "" | "TBD" | "TBA" | "N/A" | "NA" | "-" | "?" | "NONE" | "X"
    )
}

/// Marked as not part of the bought BOM (solder joints, logos ...).
fn not_bought(c: &Component) -> bool {
    let n = c.param(&MPN_KEYS).unwrap_or("").trim().to_ascii_lowercase();
    n == "nobom"
        || n == "no bom"
        || c.param(&["Exclude From BOM", "ExcludeFromBOM", "BOM", "In BOM"])
            .is_some_and(|v| {
                let v = v.trim().to_ascii_lowercase();
                v == "exclude" || v == "no" || v == "false" || v == "0"
            })
}

/// The part numbers given for other sources: Manufacturer 2 / Manufacturer
/// Part Number 2 and up.
fn alternates(c: &Component) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in 2..=6 {
        let mpn = c.param(&[
            &format!("Manufacturer Part Number {n}"),
            &format!("MPN{n}"),
            &format!("MPN {n}"),
            &format!("Alt MPN {}", n - 1),
        ]);
        if let Some(m) = mpn {
            let maker = c
                .param(&[&format!("Manufacturer {n}"), &format!("Manufacturer{n}")])
                .unwrap_or("");
            out.push((maker.to_string(), m.to_string()));
        }
    }
    out
}

/// Part numbers that are really the library's own codes for a generic part.
fn generic(maker: &str, mpn: &str) -> bool {
    let m = maker.trim().to_ascii_lowercase();
    m == "generic"
        || m == "any"
        || m == "various"
        || mpn.contains('%')
        || m.is_empty() && mpn.contains('_')
}

/// Normal form of a part number for lookups.
pub fn key(mpn: &str) -> String {
    mpn.trim().to_ascii_uppercase()
}

/// Groups the board's fitted parts into BOM lines.
pub fn build(board: &Board) -> Bom {
    let off = crate::variant::not_fitted(board);
    let parts = crate::variant::parts(board);
    let mut has_pads = vec![false; parts.len()];
    for p in &board.geometry.pads {
        if let Some(c) = p.component {
            if let Some(h) = has_pads.get_mut(c as usize) {
                *h = true;
            }
        }
    }
    let mut bom = Bom {
        boards: 1,
        variant: crate::variant::name(board).to_string(),
        ..Bom::default()
    };
    // Footprints stacked as alternatives: count one of each group (a
    // capacitor before an inductor before a resistor, else the first
    // designator), the parts not fitted aside.
    let fitted = |i: usize| !off.contains(&i) && !marked_not_fitted(&parts[i]);
    let mut dropped: HashMap<usize, usize> = HashMap::new();
    {
        let rank = |i: usize| {
            let d = parts[i].designator.to_ascii_uppercase();
            let kind = match prefix(&d).as_str() {
                "C" => 0,
                "L" => 1,
                "R" => 2,
                _ => 3,
            };
            (kind, d)
        };
        let mut pairs = stacked_pairs(board);
        pairs.retain(|&(a, b)| fitted(a) && fitted(b));
        pairs.sort_by(|x, y| {
            let kx = rank(x.0).min(rank(x.1));
            let ky = rank(y.0).min(rank(y.1));
            kx.0.cmp(&ky.0)
                .then_with(|| crate::variant::natural(&kx.1, &ky.1))
        });
        for (a, b) in pairs {
            if dropped.contains_key(&a) || dropped.contains_key(&b) {
                continue;
            }
            let (ra, rb) = (rank(a), rank(b));
            let a_first =
                ra.0 < rb.0 || ra.0 == rb.0 && crate::variant::natural(&ra.1, &rb.1).is_le();
            let (keep, drop) = if a_first { (a, b) } else { (b, a) };
            dropped.insert(drop, keep);
        }
    }
    type Key = (String, String, String, String);
    let mut groups: BTreeMap<Key, Line> = BTreeMap::new();
    for (i, c) in parts.iter().enumerate() {
        let d = c.designator.trim();
        let get = |k: &[&str]| c.param(k).unwrap_or("").trim().to_string();
        let mut mpn = get(&MPN_KEYS);
        if placeholder(&mpn) {
            mpn.clear();
        }
        // Parts without pads are bought only when they have a maker's
        // part number (screws, cables, boxes); drawings and logos are not.
        let fiducial = prefix(d) == "FID"
            || c.footprint.to_ascii_lowercase().contains("fiducial")
            || get(&["Value", "Comment"])
                .to_ascii_lowercase()
                .contains("fiducial");
        if d.is_empty() || !has_pads[i] && mpn.is_empty() || fiducial || not_bought(c) {
            if !d.is_empty() {
                bom.left_out.push(d.to_string());
            }
            continue;
        }
        if !fitted(i) {
            bom.not_fitted.push(d.to_string());
            continue;
        }
        if let Some(&k) = dropped.get(&i) {
            bom.stacked
                .push((parts[k].designator.clone(), d.to_string()));
            continue;
        }
        let mut code = false;
        if mpn.is_empty() {
            mpn = c
                .param(&[
                    "Device Type",
                    "DEVICE_TYPE",
                    "Part Code",
                    "Internal Part Number",
                ])
                .map(str::to_string)
                .or_else(|| c.library_ref.clone())
                .unwrap_or_default()
                .trim()
                .to_string();
            if placeholder(&mpn) {
                mpn.clear();
            }
            mpn = strip_footprint(&mpn, &c.footprint);
            code = !mpn.is_empty();
        }
        let value = get(&["Value", "Comment"]);
        // A value that is itself a maker's number (ADRV9025BBCZ) stands
        // in for a missing one.
        if (mpn.is_empty() || code) && part_number_like(&value) {
            mpn = value.clone();
            code = false;
        }
        let maker = get(&MAKER_KEYS);
        let k = (
            key(&mpn),
            maker.to_ascii_uppercase(),
            value.clone(),
            if mpn.is_empty() {
                c.footprint.clone()
            } else {
                String::new()
            },
        );
        let line = groups.entry(k).or_insert_with(|| Line {
            value,
            mpn,
            manufacturer: maker,
            description: get(&["Description"]),
            code,
            alternates: alternates(c),
            ..Line::default()
        });
        line.designators.push(d.to_string());
        line.parts.push(i);
        if !line.footprints.contains(&c.footprint) {
            line.footprints.push(c.footprint.clone());
            line.footprint = line.footprints.join(", ");
        }
    }
    let mut lines: Vec<Line> = groups.into_values().collect();
    for l in &mut lines {
        let mut idx: Vec<(String, usize)> = l
            .designators
            .iter()
            .cloned()
            .zip(l.parts.iter().copied())
            .collect();
        idx.sort_by(|a, b| crate::variant::natural(&a.0, &b.0));
        l.designators = idx.iter().map(|x| x.0.clone()).collect();
        l.parts = idx.iter().map(|x| x.1).collect();
    }
    lines.sort_by(|a, b| crate::variant::natural(&a.designators[0], &b.designators[0]));
    let natural = crate::variant::natural;
    bom.not_fitted.sort_by(|a, b| natural(a, b));
    bom.left_out.sort_by(|a, b| natural(a, b));
    bom.lines = lines;
    bom
}

/// Part data by part number.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Db {
    pub parts: BTreeMap<String, PartData>,
}

impl Db {
    pub fn get(&self, mpn: &str) -> Option<&PartData> {
        self.parts.get(&key(mpn))
    }

    /// Adds or replaces; returns how many.
    pub fn add(&mut self, items: Vec<PartData>) -> usize {
        let n = items.len();
        for p in items {
            if !p.mpn.trim().is_empty() {
                self.parts.insert(key(&p.mpn), p);
            }
        }
        n
    }

    pub fn from_json(text: &str) -> Result<Db, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Splits CSV text into rows of fields (quotes, doubled quotes, commas,
/// semicolons or tabs as the header uses).
pub fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let sep = [',', ';', '\t']
        .into_iter()
        .max_by_key(|s| first.matches(*s).count())
        .unwrap_or(',');
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut f = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    f.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                f.push(ch);
            }
        } else if ch == '"' {
            quoted = true;
        } else if ch == sep {
            row.push(std::mem::take(&mut f));
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut f));
            if row.iter().any(|x| !x.trim().is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            f.push(ch);
        }
    }
    row.push(f);
    if row.iter().any(|x| !x.trim().is_empty()) {
        rows.push(row);
    }
    rows
}

/// A number from text such as "$1,234.50", "0,12 €" or "12 345 In Stock".
pub fn number(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    if t.is_empty() {
        return None;
    }
    let t = match (t.rfind('.'), t.rfind(',')) {
        // Both: the later one is the decimal mark.
        (Some(d), Some(c)) if c > d => t.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => t.replace(',', ""),
        // Comma alone: decimal when 1-2 digits or more than 3 follow it
        // once, else thousands.
        (None, Some(c)) => {
            let after = t.len() - c - 1;
            if t.matches(',').count() == 1 && after != 3 {
                t.replace(',', ".")
            } else {
                t.replace(',', "")
            }
        }
        _ => t,
    };
    t.parse().ok()
}

/// Reads a parts file: a CSV with a part number column and any of
/// manufacturer, description, lifecycle, stock, price (one, or one
/// column per break: "Price@100", "100", "Price 1000"), currency,
/// datasheet, URL, lead time and replacement.
pub fn parts_csv(text: &str, source: &str) -> Result<Vec<PartData>, String> {
    let rows = csv_rows(text);
    let Some(head) = rows.first() else {
        return Err("the file is empty".into());
    };
    let h: Vec<String> = head.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let find = |names: &[&str]| names.iter().find_map(|n| h.iter().position(|x| x == n));
    let mpn = find(&[
        "mpn",
        "manufacturer part number",
        "mfr part number",
        "mfr part #",
        "manufacturer part number 1",
        "part number",
        "partnumber",
        "mfr_pn",
        "part",
    ])
    .ok_or("no part number column (MPN, Manufacturer Part Number or Part Number)")?;
    let maker = find(&["manufacturer", "mfr", "maker", "manufacturer 1", "brand"]);
    let desc = find(&["description", "desc"]);
    let life = find(&[
        "lifecycle",
        "lifecycle status",
        "status",
        "part status",
        "life cycle",
    ]);
    let stock = find(&[
        "stock",
        "availability",
        "quantity available",
        "qty available",
        "in stock",
        "available",
    ]);
    let cur = find(&["currency"]);
    let ds = find(&["datasheet", "datasheet url", "data sheet"]);
    let url = find(&["url", "link", "product url", "product page"]);
    let lead = find(&["lead time", "leadtime", "factory lead time"]);
    let repl = find(&["replacement", "suggested replacement", "alternate"]);
    let price = find(&["price", "unit price", "cost", "unit cost"]);
    // Break columns: "price@100", "price 100", "100", "qty 100".
    let mut breaks: Vec<(usize, u64)> = Vec::new();
    for (i, x) in h.iter().enumerate() {
        let t = x
            .trim_start_matches("price")
            .trim_start_matches("unit price")
            .trim_start_matches(['@', ' ', '_', '-'])
            .trim_end_matches(['+', ' ']);
        if t.len() < x.len() || x.chars().all(|c| c.is_ascii_digit() || c == '+') {
            if let Ok(q) = t.parse::<u64>() {
                breaks.push((i, q));
            }
        }
    }
    let mut out = Vec::new();
    for r in &rows[1..] {
        let at = |c: Option<usize>| c.and_then(|c| r.get(c)).map_or("", |s| s.trim());
        let m = at(Some(mpn));
        if m.is_empty() {
            continue;
        }
        let mut prices: Vec<(u64, f64)> = breaks
            .iter()
            .filter_map(|&(c, q)| number(at(Some(c))).map(|p| (q, p)))
            .collect();
        if prices.is_empty() {
            if let Some(p) = number(at(price)) {
                prices.push((1, p));
            }
        }
        prices.sort_by_key(|p| p.0);
        let mut currency = at(cur).to_string();
        if currency.is_empty() {
            let p = at(price);
            currency = if p.contains('$') {
                "USD".into()
            } else if p.contains('€') {
                "EUR".into()
            } else if p.contains('£') {
                "GBP".into()
            } else {
                String::new()
            };
        }
        out.push(PartData {
            mpn: m.to_string(),
            manufacturer: at(maker).to_string(),
            description: at(desc).to_string(),
            lifecycle: Lifecycle::parse(at(life)),
            stock: number(at(stock)).map(|x| x as u64),
            prices,
            currency,
            datasheet: at(ds).to_string(),
            url: at(url).to_string(),
            lead_time: at(lead).to_string(),
            replacement: at(repl).to_string(),
            source: source.to_string(),
            fetched: String::new(),
        });
    }
    Ok(out)
}

/// Joins part data to the lines and checks them for `boards` boards.
pub fn check(bom: &mut Bom, db: &Db, boards: u64) {
    bom.boards = boards.max(1);
    let n = bom.boards;
    let stacked = bom.stacked.clone();
    // Lines alike in value and footprint but with other part numbers.
    let mut alike: HashMap<(String, String), usize> = HashMap::new();
    for l in &bom.lines {
        if !l.value.is_empty() {
            *alike
                .entry((l.value.to_ascii_lowercase(), l.footprint.clone()))
                .or_default() += 1;
        }
    }
    for l in &mut bom.lines {
        l.issues.clear();
        l.data = if l.mpn.is_empty() {
            None
        } else {
            db.get(&l.mpn).cloned()
        };
        let need = l.qty() as u64 * n;
        let p = l.prefix();
        let generic_line = l.generic();
        let mut add = |flag: Flag, text: String| l.issues.push(Issue { flag, text });
        if l.mpn.is_empty() {
            add(Flag::NoPartNumber, "no part number to buy".into());
        } else if generic_line {
            add(
                Flag::Generic,
                format!(
                    "{} is a library code, not a maker's part number: the buyer picks one",
                    l.mpn
                ),
            );
        }
        if !l.code && l.footprints.len() > 1 {
            add(
                Flag::FootprintConflict,
                format!(
                    "{} is used on {} footprints: {}",
                    l.mpn,
                    l.footprints.len(),
                    l.footprint
                ),
            );
        }
        let alts: Vec<String> = stacked
            .iter()
            .filter(|(k, _)| l.designators.contains(k))
            .map(|(k, d)| format!("{d} (on {k}'s pads)"))
            .collect();
        if !alts.is_empty() {
            add(
                Flag::Alternate,
                format!("alternative footprints left off: {}", alts.join(", ")),
            );
        }
        if alike
            .get(&(l.value.to_ascii_lowercase(), l.footprint.clone()))
            .is_some_and(|&c| c > 1)
        {
            add(
                Flag::Consolidate,
                format!(
                    "other lines have {} on {} under another part number",
                    l.value, l.footprint
                ),
            );
        }
        match &l.data {
            None if !l.mpn.is_empty() && !generic_line => {
                add(Flag::NoData, "not in the parts data".into());
            }
            None => {}
            Some(d) => {
                match d.lifecycle {
                    Lifecycle::Obsolete => add(
                        Flag::Obsolete,
                        if d.replacement.is_empty() {
                            "obsolete".into()
                        } else {
                            format!("obsolete; the maker suggests {}", d.replacement)
                        },
                    ),
                    Lifecycle::LastTimeBuy => {
                        add(Flag::LastTimeBuy, "end of life: last time buy".into())
                    }
                    Lifecycle::Nrnd => add(Flag::Nrnd, "not recommended for new designs".into()),
                    _ => {}
                }
                if let Some(s) = d.stock {
                    if s < need {
                        add(Flag::OutOfStock, format!("{s} in stock, {need} needed"));
                    } else if s < need * 5 {
                        add(Flag::LowStock, format!("{s} in stock, {need} needed"));
                    }
                }
                if d.prices.is_empty() {
                    add(Flag::NoPrice, "no price".into());
                }
            }
        }
        if !l.mpn.is_empty() && !commodity(&p) && l.alternates.is_empty() && !generic_line {
            add(
                Flag::OneSource,
                "one maker's part number only, no second source given".into(),
            );
        }
    }
}

/// Cost of the BOM for its boards.
#[derive(Clone, Debug, Default)]
pub struct Cost {
    pub per_board: f64,
    pub total: f64,
    /// Lines with no price.
    pub unpriced: usize,
    pub currency: String,
}

pub fn line_cost(l: &Line, boards: u64) -> Option<(f64, f64)> {
    let need = l.qty() as u64 * boards.max(1);
    let unit = l.data.as_ref()?.unit_price(need)?;
    Some((unit, unit * need as f64))
}

pub fn cost(bom: &Bom) -> Cost {
    let mut c = Cost::default();
    let mut currencies: Vec<String> = Vec::new();
    for l in &bom.lines {
        match line_cost(l, bom.boards) {
            Some((_, t)) => {
                c.total += t;
                if let Some(d) = &l.data {
                    if !d.currency.is_empty() && !currencies.contains(&d.currency) {
                        currencies.push(d.currency.clone());
                    }
                }
            }
            None => c.unpriced += 1,
        }
    }
    c.per_board = c.total / bom.boards.max(1) as f64;
    c.currency = currencies.join("/");
    c
}

fn esc(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl Bom {
    pub fn parts(&self) -> usize {
        self.lines.iter().map(Line::qty).sum()
    }

    /// Lines with each flag.
    pub fn counts(&self) -> Vec<(Flag, usize)> {
        let mut m: BTreeMap<Flag, usize> = BTreeMap::new();
        for l in &self.lines {
            let mut seen = HashSet::new();
            for i in &l.issues {
                if seen.insert(i.flag) {
                    *m.entry(i.flag).or_default() += 1;
                }
            }
        }
        let mut v: Vec<(Flag, usize)> = m.into_iter().collect();
        v.sort_by_key(|(f, _)| (std::cmp::Reverse(f.level()), *f));
        v
    }

    pub fn csv(&self) -> String {
        let mut o = String::from(
            "Item,Quantity,Designators,Value,Manufacturer,Part Number,Description,Footprint,Lifecycle,Stock,Unit Price,Line Price,Currency,Datasheet,Source,Issues\n",
        );
        for (i, l) in self.lines.iter().enumerate() {
            let d = l.data.as_ref();
            let (unit, total) = line_cost(l, self.boards)
                .map_or((String::new(), String::new()), |(u, t)| {
                    (format!("{u:.4}"), format!("{t:.2}"))
                });
            let _ = writeln!(
                o,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                i + 1,
                l.qty() as u64 * self.boards,
                esc(&l.designators.join(" ")),
                esc(&l.value),
                esc(&l.manufacturer),
                esc(&l.mpn),
                esc(&l.description),
                esc(&l.footprint),
                d.map_or("", |d| d.lifecycle.word()),
                d.and_then(|d| d.stock)
                    .map_or(String::new(), |s| s.to_string()),
                unit,
                total,
                d.map_or("", |d| d.currency.as_str()),
                esc(d.map_or("", |d| d.datasheet.as_str())),
                d.map_or("", |d| d.source.as_str()),
                esc(&l
                    .issues
                    .iter()
                    .map(|i| i.flag.word())
                    .collect::<Vec<_>>()
                    .join("; ")),
            );
        }
        o
    }

    /// A parts file to fill in by hand (or from a distributor's export)
    /// and read back: one row per part number, with what is known.
    pub fn parts_template(&self) -> String {
        let mut o = String::from(
            "MPN,Manufacturer,Description,Lifecycle,Stock,Price@1,Price@10,Price@100,Price@1000,Currency,Datasheet,URL,Replacement\n",
        );
        let mut seen = HashSet::new();
        for l in &self.lines {
            if l.mpn.is_empty() || !seen.insert(key(&l.mpn)) {
                continue;
            }
            let d = l.data.clone().unwrap_or_default();
            let at = |q: u64| {
                d.prices
                    .iter()
                    .find(|p| p.0 == q)
                    .map_or(String::new(), |p| format!("{}", p.1))
            };
            let _ = writeln!(
                o,
                "{},{},{},{},{},{},{},{},{},{},{},{},{}",
                esc(&l.mpn),
                esc(if d.manufacturer.is_empty() {
                    &l.manufacturer
                } else {
                    &d.manufacturer
                }),
                esc(if d.description.is_empty() {
                    &l.description
                } else {
                    &d.description
                }),
                d.lifecycle.word(),
                d.stock.map_or(String::new(), |s| s.to_string()),
                at(1),
                at(10),
                at(100),
                at(1000),
                d.currency,
                esc(&d.datasheet),
                esc(&d.url),
                esc(&d.replacement)
            );
        }
        o
    }

    pub fn report(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "BOM{}: {} lines, {} parts per board, {} boards",
            if self.variant.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.variant)
            },
            self.lines.len(),
            self.parts(),
            self.boards
        );
        if !self.not_fitted.is_empty() {
            let _ = writeln!(
                o,
                "Not fitted ({}): {}",
                self.not_fitted.len(),
                self.not_fitted.join(" ")
            );
        }
        if !self.left_out.is_empty() {
            let _ = writeln!(
                o,
                "Not bought ({}: fiducials, drawings, marked no BOM): {}",
                self.left_out.len(),
                self.left_out.join(" ")
            );
        }
        if !self.stacked.is_empty() {
            let _ = writeln!(
                o,
                "Alternative footprints on the same pads, left off ({}): {}",
                self.stacked.len(),
                self.stacked
                    .iter()
                    .map(|(k, d)| format!("{d} (for {k})"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        let with = self.lines.iter().filter(|l| l.data.is_some()).count();
        let _ = writeln!(o, "Part data for {with} of {} lines", self.lines.len());
        let c = cost(self);
        if c.total > 0.0 {
            let _ = writeln!(
                o,
                "Cost: {:.2} {} per board, {:.2} for {} boards{}",
                c.per_board,
                c.currency,
                c.total,
                self.boards,
                if c.unpriced > 0 {
                    format!(" ({} lines without a price)", c.unpriced)
                } else {
                    String::new()
                }
            );
        }
        let counts = self.counts();
        if !counts.is_empty() {
            let _ = writeln!(o, "\nIssues:");
            for (f, n) in &counts {
                let _ = writeln!(
                    o,
                    "  {:<8} {:<32} {n} line(s)",
                    match f.level() {
                        Level::Error => "ERROR",
                        Level::Warning => "warning",
                        Level::Info => "note",
                    },
                    f.word()
                );
            }
        }
        let _ = writeln!(o);
        for (i, l) in self.lines.iter().enumerate() {
            let price = line_cost(l, self.boards)
                .map_or(String::new(), |(u, t)| format!("  {u:.4} ea, {t:.2}"));
            let life = l.data.as_ref().map_or(String::new(), |d| {
                let mut s = d.lifecycle.word().to_string();
                if let Some(st) = d.stock {
                    s = format!("{s} {st} in stock").trim().to_string();
                }
                s
            });
            let _ = writeln!(
                o,
                "{:>3} {:>4}  {:<24} {:<10} {:<14} {}  {}{}",
                i + 1,
                l.qty(),
                trim_list(&l.designators, 24),
                l.value,
                l.manufacturer,
                l.mpn,
                life,
                price
            );
            for is in &l.issues {
                if is.flag.level() > Level::Info {
                    let _ = writeln!(o, "           - {}", is.text);
                }
            }
        }
        o
    }
}

/// "R1 R2 R3 ..." cut to about `n` characters.
pub fn trim_list(d: &[String], n: usize) -> String {
    let all = d.join(" ");
    if all.len() <= n {
        return all;
    }
    let mut s = String::new();
    for x in d {
        if s.len() + x.len() + 4 > n {
            break;
        }
        s.push_str(x);
        s.push(' ');
    }
    format!("{s}+{}", d.len() - s.split_whitespace().count())
}

/// The Mouser Search API request body for up to ten part numbers.
pub fn mouser_body(mpns: &[String]) -> String {
    serde_json::json!({
        "SearchByPartRequest": {
            "mouserPartNumber": mpns.join("|"),
            "partSearchOptions": "Exact"
        }
    })
    .to_string()
}

/// Reads the Mouser Search API answer.
pub fn mouser_parse(json: &str, today: &str) -> Result<Vec<PartData>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Mouser answer: {e}"))?;
    if let Some(errs) = v["Errors"].as_array() {
        if let Some(e) = errs.first() {
            return Err(format!(
                "Mouser: {}",
                e["Message"].as_str().unwrap_or("error")
            ));
        }
    }
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").trim().to_string();
    let mut out = Vec::new();
    for p in v["SearchResults"]["Parts"].as_array().into_iter().flatten() {
        let mut prices: Vec<(u64, f64)> = p["PriceBreaks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| Some((b["Quantity"].as_u64()?, number(b["Price"].as_str()?)?)))
            .collect();
        prices.sort_by_key(|x| x.0);
        let currency = p["PriceBreaks"][0]["Currency"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let stock = number(&s(&p["AvailabilityInStock"]))
            .or_else(|| {
                let a = s(&p["Availability"]);
                a.to_ascii_lowercase()
                    .contains("in stock")
                    .then(|| number(&a))
                    .flatten()
            })
            .map(|x| x as u64)
            .or(Some(0));
        let life = s(&p["LifecycleStatus"]);
        out.push(PartData {
            mpn: s(&p["ManufacturerPartNumber"]),
            manufacturer: s(&p["Manufacturer"]),
            description: s(&p["Description"]),
            lifecycle: if life.is_empty() {
                Lifecycle::Active
            } else {
                Lifecycle::parse(&life)
            },
            stock,
            prices,
            currency,
            datasheet: s(&p["DataSheetUrl"]),
            url: s(&p["ProductDetailUrl"]),
            lead_time: s(&p["LeadTime"]),
            replacement: s(&p["SuggestedReplacement"]),
            source: "Mouser".into(),
            fetched: today.to_string(),
        });
    }
    Ok(out)
}

/// Looks part numbers up with the Mouser Search API (a free key from
/// mouser.com/api-hub), ten at a time, through the system's curl.
/// `progress` is told how many are done.
pub fn mouser_fetch(
    api_key: &str,
    mpns: &[String],
    today: &str,
    mut progress: impl FnMut(usize, usize),
) -> Result<(Vec<PartData>, Vec<String>), String> {
    let mut found = Vec::new();
    let mut notes = Vec::new();
    let chunks: Vec<&[String]> = mpns.chunks(10).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        if k > 0 {
            // 30 calls a minute at most.
            std::thread::sleep(std::time::Duration::from_millis(2100));
        }
        let url = format!(
            "https://api.mouser.com/api/v1/search/partnumber?apiKey={}",
            api_key.trim()
        );
        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-m",
            "30",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
        ])
        .args([
            "-H",
            "Accept: application/json",
            "--data-binary",
            "@-",
            &url,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not run curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            let _ = stdin.write_all(mouser_body(chunk).as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "Mouser lookup failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let got = mouser_parse(&String::from_utf8_lossy(&out.stdout), today)?;
        // Exact search can return other packagings; keep the asked ones.
        let asked: HashSet<String> = chunk.iter().map(|m| key(m)).collect();
        for m in chunk.iter() {
            if !got.iter().any(|g| key(&g.mpn) == key(m)) {
                notes.push(format!("{m}: not found at Mouser"));
            }
        }
        found.extend(got.into_iter().filter(|g| asked.contains(&key(&g.mpn))));
        progress(((k + 1) * 10).min(mpns.len()), mpns.len());
    }
    // One answer per part number: the one with most stock.
    let mut best: BTreeMap<String, PartData> = BTreeMap::new();
    for p in found {
        let k = key(&p.mpn);
        match best.get(&k) {
            Some(b) if b.stock >= p.stock => {}
            _ => {
                best.insert(k, p);
            }
        }
    }
    Ok((best.into_values().collect(), notes))
}

/// The part numbers worth looking up: real makers' numbers not in `db`
/// (or all of them with `again`).
pub fn to_look_up(bom: &Bom, db: &Db, again: bool) -> Vec<String> {
    let mut seen = HashSet::new();
    bom.lines
        .iter()
        .filter(|l| !l.mpn.is_empty() && !l.generic())
        .filter(|l| again || db.get(&l.mpn).is_none())
        .filter(|l| seen.insert(key(&l.mpn)))
        .map(|l| l.mpn.clone())
        .collect()
}

/// Today's date as YYYY-MM-DD (UTC), for the cache.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// Where looked-up part data is kept between runs.
pub fn cache_path() -> std::path::PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
                })
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("Trace").join("parts-cache.json")
}

pub fn load_cache() -> Db {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| Db::from_json(&t).ok())
        .unwrap_or_default()
}

pub fn save_cache(db: &Db) -> Result<(), String> {
    let p = cache_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, db.to_json()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number("$1,234.50"), Some(1234.5));
        assert_eq!(number("0,12 €"), Some(0.12));
        assert_eq!(number("1.234,5"), Some(1234.5));
        assert_eq!(number("12,345 In Stock"), Some(12345.0));
        assert_eq!(number("£0.036"), Some(0.036));
        assert_eq!(number(""), None);
    }

    #[test]
    fn lifecycle_words() {
        assert_eq!(Lifecycle::parse("Obsolete"), Lifecycle::Obsolete);
        assert_eq!(
            Lifecycle::parse("Not Recommended for New Designs"),
            Lifecycle::Nrnd
        );
        assert_eq!(Lifecycle::parse("End of Life"), Lifecycle::LastTimeBuy);
        assert_eq!(Lifecycle::parse("New Product"), Lifecycle::New);
        assert_eq!(Lifecycle::parse("Active"), Lifecycle::Active);
    }

    #[test]
    fn price_breaks() {
        let d = PartData {
            prices: vec![(1, 0.10), (10, 0.05), (100, 0.02)],
            ..PartData::default()
        };
        assert_eq!(d.unit_price(1), Some(0.10));
        assert_eq!(d.unit_price(9), Some(0.10));
        assert_eq!(d.unit_price(50), Some(0.05));
        assert_eq!(d.unit_price(5000), Some(0.02));
        let e = PartData {
            prices: vec![(10, 0.5)],
            ..PartData::default()
        };
        assert_eq!(e.unit_price(1), Some(0.5));
    }

    #[test]
    fn parts_file() {
        let t = "MPN;Manufacturer;Lifecycle;Stock;Price@1;Price@100\n\
                 \"GRM155R71C104KA88D\";Murata;Active;150000;0,10;0,02\n\
                 LM317;TI;Obsolete;0;1,20;0,80\n";
        let p = parts_csv(t, "file").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].prices, vec![(1, 0.10), (100, 0.02)]);
        assert_eq!(p[1].lifecycle, Lifecycle::Obsolete);
        assert_eq!(p[0].stock, Some(150000));
    }

    #[test]
    fn mouser_answer() {
        let j = r#"{"Errors":[],"SearchResults":{"NumberOfResult":1,"Parts":[
          {"Availability":"152874 In Stock","AvailabilityInStock":"152874",
           "DataSheetUrl":"https://x/ds.pdf","Description":"MLCC 0.1uF",
           "LeadTime":"84 Days","LifecycleStatus":null,"Manufacturer":"Murata Electronics",
           "ManufacturerPartNumber":"GRM155R71C104KA88D","MouserPartNumber":"81-GRM155R71C104KA88",
           "PriceBreaks":[{"Quantity":1,"Price":"$0.10","Currency":"USD"},{"Quantity":10,"Price":"$0.036","Currency":"USD"}],
           "ProductDetailUrl":"https://x/p","SuggestedReplacement":""},
          {"Availability":"","AvailabilityInStock":null,"LifecycleStatus":"Obsolete",
           "Manufacturer":"TI","ManufacturerPartNumber":"OLD1","PriceBreaks":[],"SuggestedReplacement":"NEW1"}]}}"#;
        let p = mouser_parse(j, "2026-11-17").unwrap();
        assert_eq!(p[0].stock, Some(152874));
        assert_eq!(p[0].prices[1], (10, 0.036));
        assert_eq!(p[0].lifecycle, Lifecycle::Active);
        assert_eq!(p[0].currency, "USD");
        assert_eq!(p[1].lifecycle, Lifecycle::Obsolete);
        assert_eq!(p[1].replacement, "NEW1");
        assert!(mouser_parse(
            r#"{"Errors":[{"Message":"Invalid unique identifier."}]}"#,
            ""
        )
        .is_err());
        assert!(mouser_body(&["A".into(), "B".into()]).contains("A|B"));
    }

    #[test]
    fn values_and_codes() {
        assert!(part_number_like("ADRV9025BBCZ"));
        assert!(part_number_like("2450BL15B200E"));
        assert!(part_number_like("0451350403"));
        assert!(part_number_like("U.FL-R-SMT-1(01)"));
        assert!(!part_number_like("10uF"));
        assert!(!part_number_like("2.2uH,1.3A"));
        assert!(!part_number_like("47OHM"));
        assert!(!part_number_like("100nF/50V"));
        assert_eq!(
            strip_footprint("CAP_SM-0.1UF,6.3V,10,E011567,C0201-C0201", "C0201"),
            "CAP_SM-0.1UF,6.3V,10,E011567"
        );
        assert_eq!(
            strip_footprint("CAP_SM-10UF,E007143,C0603-(C0603L),C0603", "C0603L"),
            "CAP_SM-10UF,E007143"
        );
        assert_eq!(
            strip_footprint("RES_100K_1/16W_1%_0402_402_100K", "402"),
            "RES_100K_1/16W_1%_0402_402_100K"
        );
    }

    #[test]
    fn date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
