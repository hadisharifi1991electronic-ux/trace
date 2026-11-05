//! Works out what each fabrication file is: which copper layer, mask, silk,
//! paste, outline or drill. Evidence is used in this order:
//!
//! 1. X2 file attributes (`%TF.FileFunction`), which name the layer exactly;
//! 2. CAD-specific comments, such as Altium's `Layer_Physical_Order`;
//! 3. file extensions (Protel/Altium, Eagle, OrCAD and others);
//! 4. words in the file name (KiCad, Allegro, PADS, EasyEDA, DipTrace...).
//!
//! Files that stay unknown are reported and left out.

use crate::excellon::Drill;
use crate::gerber::Gerber;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CuPos {
    Top,
    Inner,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Copper {
        pos: CuPos,
        /// Layer number from the name or attributes (L2, In1, G1, ly3...).
        number: Option<u32>,
        /// Physical position in the stack when the file states it.
        order: Option<u32>,
        plane: bool,
    },
    Mask(Side),
    Paste(Side),
    Silk(Side),
    Outline,
    Keepout,
    /// Mechanical or documentation layers (assembly, fab, courtyard...).
    Mechanical(u32),
    Drill {
        plated: Option<bool>,
        span: Option<(u32, u32)>,
    },
    DrillDrawing,
    /// Not board data: reports, aperture lists, pad masters.
    Ignore,
    Unknown,
}

impl Role {
    pub fn label(&self) -> String {
        let side = |s: &Side| if *s == Side::Top { "top" } else { "bottom" };
        match self {
            Role::Copper {
                pos, number, plane, ..
            } => {
                let kind = if *plane { "plane" } else { "copper" };
                match pos {
                    CuPos::Top => format!("Top {kind}"),
                    CuPos::Bottom => format!("Bottom {kind}"),
                    CuPos::Inner => match number {
                        Some(n) => format!("Inner {kind} {n}"),
                        None => format!("Inner {kind}"),
                    },
                }
            }
            Role::Mask(s) => format!("Solder mask, {}", side(s)),
            Role::Paste(s) => format!("Paste, {}", side(s)),
            Role::Silk(s) => format!("Silkscreen, {}", side(s)),
            Role::Outline => "Board outline".into(),
            Role::Keepout => "Keep-out".into(),
            Role::Mechanical(n) => format!("Mechanical {n}"),
            Role::Drill { plated, span } => {
                let p = match plated {
                    Some(true) => "Plated drill",
                    Some(false) => "Non-plated drill",
                    None => "Drill",
                };
                match span {
                    Some((a, b)) => format!("{p}, layers {a}-{b}"),
                    None => p.into(),
                }
            }
            Role::DrillDrawing => "Drill drawing".into(),
            Role::Ignore => "Not used (report or helper file)".into(),
            Role::Unknown => "Unknown".into(),
        }
    }

    pub fn is_copper(&self) -> bool {
        matches!(self, Role::Copper { .. })
    }
}

pub enum Content<'a> {
    Gerber(&'a Gerber),
    Drill(&'a Drill),
}

/// Role of one file and the evidence it came from.
pub fn detect(name: &str, content: Content) -> (Role, String) {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], base[i + 1..].to_ascii_lowercase()),
        _ => (base, String::new()),
    };
    let words = tokens(stem);

    match content {
        Content::Drill(d) => {
            let mut plated = d.plated;
            if plated.is_none() {
                let all_known: Vec<Option<bool>> = d.holes.iter().map(|h| h.plated).collect();
                if !all_known.is_empty() && all_known.iter().all(|p| *p == all_known[0]) {
                    plated = all_known[0];
                }
            }
            let lower = stem.to_ascii_lowercase();
            if plated.is_none() || d.file_function.is_none() {
                if has(&words, &["npth", "nonplated", "unplated", "np"])
                    || lower.contains("non_plated")
                    || lower.contains("non-plated")
                    || lower.contains("nonplated")
                {
                    plated = Some(false);
                } else if has(&words, &["pth", "plated"])
                    && d.file_function.is_none()
                    && plated.is_none()
                {
                    plated = Some(true);
                }
            }
            let span = d.span.or_else(|| layer_pair(stem));
            let why = if d.file_function.is_some() {
                "X2 attribute in the drill file"
            } else {
                "drill file content"
            };
            (Role::Drill { plated, span }, why.into())
        }
        Content::Gerber(g) => {
            if let Some(f) = &g.file_function {
                if let Some(r) = from_file_function(f) {
                    return (r, format!("X2 attribute {f}"));
                }
            }
            let order = g.comments.iter().find_map(|c| {
                c.strip_prefix("Layer_Physical_Order=")
                    .and_then(|v| v.trim().parse::<u32>().ok())
            });
            if let Some(r) = from_extension(&ext) {
                let r = match r {
                    Role::Copper {
                        pos, number, plane, ..
                    } => Role::Copper {
                        pos,
                        number,
                        order,
                        plane,
                    },
                    r => r,
                };
                return (r, format!("file extension .{ext}"));
            }
            // KiCad and others write "<project>-<layer>"; the last part is
            // the most reliable, so try it on its own first.
            let tail = stem
                .rsplit_once('-')
                .map(|(_, t)| tokens(t))
                .filter(|t| !t.is_empty());
            if let Some(r) = tail
                .as_deref()
                .and_then(from_words)
                .or_else(|| from_words(&words))
            {
                let r = match r {
                    Role::Copper {
                        pos, number, plane, ..
                    } => Role::Copper {
                        pos,
                        number,
                        order,
                        plane,
                    },
                    r => r,
                };
                return (r, "words in the file name".into());
            }
            // Comments such as "G04 Layer: BottomSilkLayer*" (EasyEDA) or
            // Allegro's "LAYER ... ETCH/TOP".
            for c in &g.comments {
                let lc = c.to_ascii_lowercase();
                if lc.contains("layer") || lc.contains("etch") || lc.contains("film") {
                    if let Some(r) = from_words(&tokens(c)) {
                        return (r, format!("file comment \"{c}\""));
                    }
                }
            }
            if let Some(r) = from_aperture_functions(g) {
                return (r, "X2 aperture attributes".into());
            }
            (Role::Unknown, "no recognisable name or attributes".into())
        }
    }
}

/// Role for files that are clearly not layer data, decided before parsing.
pub fn skip_by_name(name: &str) -> bool {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    let ext = base.rsplit('.').next().unwrap_or("");
    matches!(
        ext,
        "rep"
            | "extrep"
            | "rul"
            | "apr"
            | "apr_lib"
            | "drr"
            | "ldp"
            | "pdf"
            | "html"
            | "htm"
            | "xls"
            | "xlsx"
            | "csv"
            | "step"
            | "stp"
            | "png"
            | "jpg"
            | "jpeg"
            | "bmp"
            | "zip"
            | "md"
            | "json"
            | "gbrjob"
            | "pos"
            | "ipc"
            | "d356"
            | "xml"
            | "log"
            | "dri"
            | "gpi"
            | "pro"
            | "cam"
            | "brd"
            | "sch"
            | "kicad_pcb"
            | "pcbdoc"
            | "schdoc"
            | "doc"
            | "docx"
            | "rtf"
            | "gvp"
            | "net"
            | "bom"
            | "pdb"
            | "dxf"
            | "svg"
    ) || base.starts_with("readme")
        || base.contains("pick place")
        || base.contains("pick_place")
        || base.contains("status report")
        || base.contains("stackup compatibility")
        || base == "art_param.txt"
        || base == "nc_param.txt"
        || base == "nc_tools.txt"
}

fn layer_pair(stem: &str) -> Option<(u32, u32)> {
    // "ncdrill-1-4", "board-RoundHoles-1-2", "drill_1_6"
    let parts: Vec<&str> = stem.split(['-', '_', ' ']).collect();
    if parts.len() >= 3 {
        let (a, b) = (parts[parts.len() - 2], parts[parts.len() - 1]);
        if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
            if a < b && b <= 64 {
                return Some((a, b));
            }
        }
    }
    None
}

fn from_file_function(f: &str) -> Option<Role> {
    let p: Vec<&str> = f.split(',').map(|s| s.trim()).collect();
    let side = |s: Option<&&str>| match s {
        Some(&"Bot") => Side::Bottom,
        _ => Side::Top,
    };
    Some(match p.first().copied()? {
        "Copper" => {
            let number = p
                .get(1)
                .and_then(|l| l.trim_start_matches('L').parse().ok());
            let pos = match p.get(2).copied() {
                Some("Top") => CuPos::Top,
                Some("Bot") => CuPos::Bottom,
                _ => CuPos::Inner,
            };
            Role::Copper {
                pos,
                number,
                order: number,
                plane: p.get(3).is_some_and(|t| *t == "Plane"),
            }
        }
        "Soldermask" => Role::Mask(side(p.get(1))),
        "Paste" => Role::Paste(side(p.get(1))),
        "Legend" => Role::Silk(side(p.get(1))),
        "Profile" => Role::Outline,
        "Keepout" => Role::Keepout,
        "Drillmap" | "FabricationDrawing" => Role::DrillDrawing,
        "AssemblyDrawing" | "Component" | "Other" | "ArrayDrawing" | "Vcut" | "Depthrout"
        | "Pads" | "Glue" | "Carbonmask" | "Goldmask" | "Heatsinkmask" | "Peelablemask"
        | "Silvermask" | "Tinmask" | "Viafill" => Role::Mechanical(0),
        "Plated" | "NonPlated" => Role::Ignore,
        _ => return None,
    })
}

fn copper(pos: CuPos, number: Option<u32>, plane: bool) -> Role {
    Role::Copper {
        pos,
        number,
        order: None,
        plane,
    }
}

fn from_extension(ext: &str) -> Option<Role> {
    use CuPos::*;
    let num = |prefix: &str| ext.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok());
    Some(match ext {
        // Protel / Altium, also used by EasyEDA, Eagle CAM jobs and KiCad.
        "gtl" => copper(Top, None, false),
        "gbl" => copper(Bottom, None, false),
        "gto" => Role::Silk(Side::Top),
        "gbo" => Role::Silk(Side::Bottom),
        "gts" => Role::Mask(Side::Top),
        "gbs" => Role::Mask(Side::Bottom),
        "gtp" => Role::Paste(Side::Top),
        "gbp" => Role::Paste(Side::Bottom),
        "gko" => Role::Outline,
        "gml" | "gm" => Role::Mechanical(1),
        "gpt" | "gpb" => Role::Ignore,
        "gta" | "gba" => Role::Mechanical(0),
        // Eagle
        "cmp" => copper(Top, None, false),
        "sol" => copper(Bottom, None, false),
        "stc" => Role::Mask(Side::Top),
        "sts" => Role::Mask(Side::Bottom),
        "plc" => Role::Silk(Side::Top),
        "pls" => Role::Silk(Side::Bottom),
        "crc" => Role::Paste(Side::Top),
        "crs" => Role::Paste(Side::Bottom),
        "dim" | "mil" | "oln" | "out" | "bor" => Role::Outline,
        // OrCAD Layout and others
        "top" => copper(Top, None, false),
        "bot" => copper(Bottom, None, false),
        "sst" | "slk" => Role::Silk(Side::Top),
        "ssb" => Role::Silk(Side::Bottom),
        "smt" => Role::Mask(Side::Top),
        "smb" => Role::Mask(Side::Bottom),
        "spt" => Role::Paste(Side::Top),
        "spb" => Role::Paste(Side::Bottom),
        "fab" => Role::DrillDrawing,
        _ => {
            if let Some(n) = num("gp") {
                copper(Inner, Some(n), true)
            } else if let Some(n) = num("gm") {
                Role::Mechanical(n)
            } else if num("gd").or_else(|| num("gg")).is_some() {
                Role::DrillDrawing
            } else {
                let n = num("gl").or_else(|| num("g")).or_else(|| num("ly")).or_else(|| num("l")).or_else(|| num("in"))?;
                copper(Inner, Some(n), false)
            }
        }
    })
}

/// Lower-case words of a name, split at punctuation, digits and camel case.
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(&prev) = cur.chars().last().as_ref() {
            let camel = prev.is_ascii_lowercase() && c.is_ascii_uppercase();
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let acronym_end = prev.is_ascii_uppercase() && c.is_ascii_uppercase() && next_lower;
            if camel || acronym_end {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|w| w.to_ascii_lowercase()).collect()
}

fn has(words: &[String], any: &[&str]) -> bool {
    words.iter().any(|w| any.contains(&w.as_str()))
}

/// Splits a word such as `in2`, `l12`, `layer3` or `gnd2` into its prefix and number.
fn split_num(w: &str) -> Option<(&str, u32)> {
    let i = w.find(|c: char| c.is_ascii_digit())?;
    let n = w[i..].parse().ok()?;
    Some((&w[..i], n))
}

fn side_of(words: &[String], copper_context: bool) -> Option<Side> {
    // Layer words come last in most names; project names come first.
    for w in words.iter().rev() {
        match w.as_str() {
            "top" | "front" | "f" | "t" | "upper" | "cmp" | "component" | "comp" | "primary" => {
                return Some(Side::Top)
            }
            "bottom" | "bot" | "back" | "b" | "lower" | "secondary" => return Some(Side::Bottom),
            "sol" | "solder" if copper_context => return Some(Side::Bottom),
            "smt" | "sst" | "spt" | "plc" | "stc" | "crc" | "tsm" | "tsp" => {
                return Some(Side::Top)
            }
            "smb" | "ssb" | "spb" | "pls" | "sts" | "crs" | "bsm" | "bsp" => {
                return Some(Side::Bottom)
            }
            _ => {}
        }
    }
    None
}

fn from_words(words: &[String]) -> Option<Role> {
    use CuPos::*;
    if words.is_empty() {
        return None;
    }
    let joined = words.concat();
    let side = side_of(words, false);
    if has(
        words,
        &[
            "drill", "drl", "ncdrill", "drillmap", "holes", "nc", "thruhole",
        ],
    ) || joined.contains("drilldrawing")
        || joined.contains("drillmap")
        || joined.contains("fabdrawing")
    {
        return Some(Role::DrillDrawing);
    }
    if has(
        words,
        &[
            "paste",
            "cream",
            "stencil",
            "pastemask",
            "sp",
            "spt",
            "spb",
            "crc",
            "crs",
            "tsp",
            "bsp",
        ],
    ) {
        return Some(Role::Paste(side.unwrap_or(Side::Top)));
    }
    if has(
        words,
        &[
            "mask",
            "soldermask",
            "sm",
            "smt",
            "smb",
            "stop",
            "resist",
            "stc",
            "sts",
            "tsm",
            "bsm",
        ],
    ) || joined.contains("soldermask")
    {
        return Some(Role::Mask(side.unwrap_or(Side::Top)));
    }
    if has(
        words,
        &[
            "silk",
            "silks",
            "silkscreen",
            "legend",
            "overlay",
            "ss",
            "sst",
            "ssb",
            "plc",
            "pls",
            "slk",
            "screen",
        ],
    ) || joined.contains("silkscreen")
    {
        return Some(Role::Silk(side.unwrap_or(Side::Top)));
    }
    if has(
        words,
        &[
            "outline",
            "edge",
            "cuts",
            "profile",
            "dimension",
            "border",
            "contour",
            "mill",
            "milling",
            "rout",
            "boundary",
            "gko",
            "bo",
        ],
    ) || joined.contains("boardoutline")
        || joined.contains("edgecuts")
    {
        return Some(Role::Outline);
    }
    if has(words, &["keepout", "ko"]) || joined.contains("keepout") {
        return Some(Role::Keepout);
    }
    if has(
        words,
        &[
            "fab",
            "assembly",
            "assy",
            "courtyard",
            "crtyd",
            "fabrication",
            "drawing",
            "dwgs",
            "comments",
            "cmts",
            "eco1",
            "eco2",
            "margin",
            "user",
            "notes",
            "mechanical",
            "mech",
            "dimensions",
            "doc",
        ],
    ) {
        return Some(Role::Mechanical(0));
    }
    // Copper: numbered inner layers first.
    for w in words.iter().rev() {
        if let Some((prefix, n)) = split_num(w) {
            match prefix {
                "in" | "inner" | "mid" | "l" | "layer" | "lyr" | "ly" | "sig" | "signal"
                | "art" | "g" | "gl" | "int" | "internal" | "inr" => {
                    return Some(copper(Inner, Some(n), false));
                }
                "plane" | "gnd" | "pwr" | "power" | "vcc" | "gp" | "pln" => {
                    return Some(copper(Inner, Some(n), true))
                }
                _ => {}
            }
        }
    }
    // KiCad: "In1_Cu" splits into "in1", "cu"; caught above.
    let copperish = has(
        words,
        &[
            "cu",
            "copper",
            "signal",
            "sig",
            "etch",
            "layer",
            "art",
            "plane",
            "gnd",
            "ground",
            "pwr",
            "power",
            "vcc",
            "inner",
            "internal",
            "cmp",
            "sol",
            "conductor",
            "routing",
            "trace",
        ],
    );
    if let Some(s) = side_of(words, true) {
        // A bare "top" or "bottom" file is almost always copper.
        let pos = if s == Side::Top { Top } else { Bottom };
        return Some(copper(pos, None, false));
    }
    if copperish {
        let plane = has(words, &["plane", "gnd", "ground", "pwr", "power", "vcc"]);
        return Some(copper(Inner, None, plane));
    }
    None
}

fn from_aperture_functions(g: &Gerber) -> Option<Role> {
    let f: Vec<&str> = g
        .apertures
        .iter()
        .filter_map(|a| a.function.as_deref())
        .collect();
    if f.is_empty() {
        return None;
    }
    if f.iter()
        .any(|s| s.starts_with("Conductor") || s.starts_with("ViaPad") || s.contains("CuDef"))
    {
        return Some(copper(CuPos::Inner, None, false));
    }
    if f.iter().all(|s| s.starts_with("Profile")) {
        return Some(Role::Outline);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_name(name: &str) -> Role {
        let g = Gerber::default();
        detect(name, Content::Gerber(&g)).0
    }

    #[test]
    fn tokens_split() {
        assert_eq!(
            tokens("Gerber_TopSilkscreenLayer"),
            ["gerber", "top", "silkscreen", "layer"]
        );
        assert_eq!(tokens("board-In2_Cu"), ["board", "in2", "cu"]);
        assert_eq!(tokens("SOLDERMASK_TOP"), ["soldermask", "top"]);
    }

    #[test]
    fn common_names() {
        assert!(matches!(
            by_name("board.GTL"),
            Role::Copper {
                pos: CuPos::Top,
                ..
            }
        ));
        assert!(matches!(
            by_name("board.G3"),
            Role::Copper {
                pos: CuPos::Inner,
                number: Some(3),
                ..
            }
        ));
        assert!(matches!(
            by_name("board.GP1"),
            Role::Copper { plane: true, .. }
        ));
        assert_eq!(by_name("board.GM1"), Role::Mechanical(1));
        assert!(matches!(
            by_name("proj-F_Cu.gbr"),
            Role::Copper {
                pos: CuPos::Top,
                ..
            }
        ));
        assert!(matches!(
            by_name("proj-B_Cu.gbr"),
            Role::Copper {
                pos: CuPos::Bottom,
                ..
            }
        ));
        assert!(matches!(
            by_name("proj-In2_Cu.gbr"),
            Role::Copper {
                number: Some(2),
                ..
            }
        ));
        assert_eq!(by_name("proj-F_Mask.gbr"), Role::Mask(Side::Top));
        assert_eq!(by_name("proj-B_Silkscreen.gbr"), Role::Silk(Side::Bottom));
        assert_eq!(by_name("proj-Edge_Cuts.gbr"), Role::Outline);
        assert_eq!(by_name("SOLDERMASK_BOTTOM.art"), Role::Mask(Side::Bottom));
        assert!(matches!(
            by_name("TOP.art"),
            Role::Copper {
                pos: CuPos::Top,
                ..
            }
        ));
        assert!(matches!(
            by_name("L3_GND.art"),
            Role::Copper {
                number: Some(3),
                ..
            }
        ));
        assert_eq!(by_name("Gerber_BoardOutlineLayer.GKO"), Role::Outline);
        assert_eq!(by_name("board.plc"), Role::Silk(Side::Top));
        assert_eq!(by_name("PASTEMASK_TOP.art"), Role::Paste(Side::Top));
    }
}
