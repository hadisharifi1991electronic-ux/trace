//! The raw `.brd` database: file header, string table and object blocks.
//!
//! Layouts follow the reverse-engineered description in KiCad 10's Allegro
//! importer (GPL-3.0-or-later, `pcbnew/pcb_io/allegro`). Every block type
//! must be read in full to find the next one, so all of them are parsed even
//! though only some are kept.

use crate::stream::Stream;
use crate::{Error, Result};
use std::collections::HashMap;

/// File format generation, in release order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Ver {
    Pre16,
    V160,
    V161,
    V162,
    V164,
    V165,
    V166,
    V172,
    V174,
    V175,
    V180,
    V181,
    V190,
}

impl Ver {
    fn from_magic(magic: u32) -> Result<Ver> {
        Ok(match magic & 0xFFFF_FF00 {
            0x0013_0000 => Ver::V160,
            0x0013_0200 => Ver::V161,
            0x0013_0400 | 0x0013_0500 => Ver::V162,
            0x0013_0C00 => Ver::V164,
            0x0013_1000 => Ver::V165,
            0x0013_1500 => Ver::V166,
            0x0014_0400 | 0x0014_0500 | 0x0014_0600 | 0x0014_0700 => Ver::V172,
            0x0014_0900 | 0x0014_0E00 => Ver::V174,
            0x0014_1500 => Ver::V175,
            0x0015_0000 => Ver::V180,
            0x0015_0200 => Ver::V181,
            0x0016_0100 => Ver::V190,
            _ if (magic >> 16) & 0xFFFF <= 0x12 => Ver::Pre16,
            _ => {
                return Err(Error::Format(format!(
                    "unknown Allegro file version {magic:#010x}"
                )))
            }
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Ver::Pre16 => "before 16.0",
            Ver::V160 => "16.0",
            Ver::V161 => "16.1",
            Ver::V162 => "16.2",
            Ver::V164 => "16.4",
            Ver::V165 => "16.5",
            Ver::V166 => "16.6",
            Ver::V172 => "17.2",
            Ver::V174 => "17.4",
            Ver::V175 => "17.5",
            Ver::V180 => "18.0",
            Ver::V181 => "18.1",
            Ver::V190 => "19.0",
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Ll {
    pub head: u32,
    pub tail: u32,
}

pub struct Header {
    pub ver: Ver,
    pub program: String,
    pub units: u8,
    pub divisor: u32,
    pub string_count: u32,
    pub x27_end: u32,
    pub ll_nets: Ll,
    pub ll_shapes: Ll,
    pub ll_graphics: Ll,
    pub ll_rects_shapes: Ll,
    pub ll_fp_defs: Ll,
    pub tails: Vec<u32>,
    /// Per layer class: key of its 0x2A layer list (0 when none).
    pub layer_lists: [u32; 25],
}

fn ll(s: &mut Stream, ver: Ver) -> Result<Ll> {
    let (a, b) = (s.u32()?, s.u32()?);
    Ok(if ver >= Ver::V180 {
        Ll { head: a, tail: b }
    } else {
        Ll { head: b, tail: a }
    })
}

fn parse_header(s: &mut Stream) -> Result<Header> {
    let magic = s.u32()?;
    let ver = Ver::from_magic(magic)?;
    s.skip_u32(7)?;
    let mut tails = Vec::new();
    let mut x27_end = 0;
    let mut string_count = 0;
    if ver < Ver::V180 {
        s.skip_u32(7)?;
    } else {
        s.skip_u32(2)?;
        x27_end = s.u32()?;
        s.skip_u32(2)?;
        string_count = s.u32()?;
        s.skip_u32(1)?;
        for _ in 0..5 {
            tails.push(ll(s, ver)?.tail);
        }
    }
    let mut lists = Vec::with_capacity(18);
    for _ in 0..18 {
        lists.push(ll(s, ver)?);
    }
    if ver < Ver::V180 {
        s.skip_u32(2)?;
    } else {
        tails.push(ll(s, ver)?.tail);
    }
    tails.push(ll(s, ver)?.tail); // 0x36
    if ver < Ver::V180 {
        tails.push(ll(s, ver)?.tail);
    }
    tails.push(ll(s, ver)?.tail);
    tails.push(ll(s, ver)?.tail);
    if ver < Ver::V180 {
        s.skip_u32(1)?;
    } else {
        tails.push(ll(s, ver)?.tail);
        s.skip_u32(1)?;
        if ver >= Ver::V181 {
            s.skip_u32(8)?;
        }
        s.skip_u32(1)?;
    }
    tails.extend(lists.iter().map(|l| l.tail));
    let program = s.fixed_str(60, false)?;
    s.skip_u32(2)?;
    s.skip_u32(if ver < Ver::V180 { 17 } else { 9 })?;
    let units = s.u8()?;
    if !(1..=5).contains(&units) {
        return Err(Error::Format(format!("unknown board units {units}")));
    }
    s.skip(3)?;
    s.skip_u32(1)?;
    if ver < Ver::V180 {
        s.skip_u32(1)?;
        x27_end = s.u32()?;
    }
    s.skip_u32(1)?;
    if ver < Ver::V180 {
        string_count = s.u32()?;
    }
    s.skip_u32(50 + 3)?;
    let divisor = s.u32()?;
    s.skip_u32(if ver >= Ver::V181 { 102 } else { 110 })?;
    let mut layer_lists = [0u32; 25];
    for l in layer_lists.iter_mut() {
        s.skip_u32(1)?;
        *l = s.u32()?;
    }
    Ok(Header {
        ver,
        program: program.trim().to_string(),
        units,
        divisor,
        string_count,
        x27_end,
        ll_nets: lists[5],
        ll_shapes: lists[3],
        ll_graphics: lists[4],
        ll_rects_shapes: lists[7],
        ll_fp_defs: lists[9],
        tails,
        layer_lists,
    })
}

/// Allegro's layer address: a class (ETCH, silkscreen, ...) and a subclass.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LayerInfo {
    pub class: u8,
    pub sub: u8,
}

pub mod class {
    pub const BOARD_GEOMETRY: u8 = 0x01;
    pub const DRAWING_FORMAT: u8 = 0x04;
    pub const ETCH: u8 = 0x06;
    pub const PACKAGE_GEOMETRY: u8 = 0x09;
    pub const REF_DES: u8 = 0x0D;
    pub const ANTI_ETCH: u8 = 0x14;
    pub const BOUNDARY: u8 = 0x15;
}

#[derive(Clone, Copy, Debug)]
pub struct PadComp {
    pub kind: u8,
    pub w: i32,
    pub h: i32,
    pub z1: i32,
    pub off_x: i32,
    pub off_y: i32,
}

#[derive(Clone, Debug)]
pub struct Padstack {
    pub name: u32,
    pub start_layer: u8,
    pub layer_count: u16,
    pub drill: u32,
    pub slot: (u32, u32),
    pub plated: bool,
    pub fixed: usize,
    pub per_layer: usize,
    pub comps: Vec<PadComp>,
}

impl Padstack {
    /// The copper pad shape on the n-th layer of this padstack.
    pub fn pad(&self, layer: usize) -> Option<&PadComp> {
        self.comps
            .get(self.fixed + layer * self.per_layer + 2)
            .filter(|c| c.kind != 0)
    }
}

#[derive(Clone, Debug)]
pub enum LayerName {
    Text(String),
    Id(u32),
}

#[derive(Clone, Debug)]
pub enum Data {
    Arc {
        cw: bool,
        width: u32,
        start: (i32, i32),
        end: (i32, i32),
        center: (f64, f64),
        radius: f64,
        bbox: [i32; 4],
    },
    NetAssign {
        net: u32,
        conn: u32,
    },
    Track {
        layer: LayerInfo,
        first_seg: u32,
    },
    CompInst {
        refdes: u32,
    },
    Pad {
        name: u32,
        x: i32,
        y: i32,
        padstack: u32,
        rotation: u32,
    },
    Graphic {
        layer: LayerInfo,
        first_seg: u32,
    },
    Seg {
        width: u32,
        start: (i32, i32),
        end: (i32, i32),
    },
    Net {
        name: u32,
        assignment: u32,
    },
    Padstack(Padstack),
    Rect {
        layer: LayerInfo,
        coords: [i32; 4],
        rotation: u32,
    },
    Shape {
        layer: LayerInfo,
        first_keepout: u32,
        first_seg: u32,
        flags: u32,
    },
    LayerList(Vec<(LayerName, u32)>),
    FpDef {
        name: u32,
        first_inst: u32,
    },
    FpInst {
        bottom: bool,
        rotation: u32,
        x: i32,
        y: i32,
        inst_ref: u32,
        graphics: u32,
        first_pad: u32,
    },
    PlacedPad {
        net: u32,
        next_in_fp: u32,
        pad: u32,
        bbox: [i32; 4],
    },
    Via {
        net: u32,
        x: i32,
        y: i32,
        padstack: u32,
    },
    Keepout {
        first_seg: u32,
    },
    /// A cross-section record (0x21 blob, subtype 0x0518): one layer of
    /// the stack-up, surfaces and dielectrics included.
    Stack(StackRecord),
    Other,
}

pub struct Block {
    pub kind: u8,
    pub key: u32,
    pub next: u32,
    pub data: Data,
}

fn layer(s: &mut Stream) -> Result<LayerInfo> {
    Ok(LayerInfo {
        class: s.u8()?,
        sub: s.u8()?,
    })
}

fn coords4(s: &mut Stream) -> Result<[i32; 4]> {
    Ok([s.i32()?, s.i32()?, s.i32()?, s.i32()?])
}

/// Reads `n` u32 words when `cond` holds.
fn opt(s: &mut Stream, cond: bool, n: usize) -> Result<()> {
    if cond {
        s.skip_u32(n)?;
    }
    Ok(())
}

/// Parses the block starting after its type byte. `None` for an unknown type.
fn parse_block(s: &mut Stream, kind: u8, v: Ver, x27_end: usize) -> Result<Option<Block>> {
    let ge = |x: Ver| v >= x;
    let lt = |x: Ver| v < x;
    let mut next = 0;
    let key;
    let data = match kind {
        0x01 => {
            s.skip(1)?;
            s.u8()?;
            let sub = s.u8()?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            let width = s.u32()?;
            let start = (s.i32()?, s.i32()?);
            let end = (s.i32()?, s.i32()?);
            let center = (s.f64()?, s.f64()?);
            let radius = s.f64()?;
            Data::Arc {
                cw: sub & 0x40 != 0,
                width,
                start,
                end,
                center,
                radius,
                bbox: coords4(s)?,
            }
        }
        0x03 => {
            s.skip(1)?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            let sub = s.u8()?;
            s.u8()?;
            let size = s.u16()? as usize;
            opt(s, ge(Ver::V172), 1)?;
            match sub {
                0x65 => {}
                0x64 | 0x66 | 0x67 | 0x6A => s.skip_u32(1)?,
                0x69 => s.skip_u32(2)?,
                0x68 | 0x6B | 0x6D | 0x6E | 0x6F | 0x71 | 0x73 | 0x78 => {
                    s.fixed_str(size, true)?;
                }
                0x6C => {
                    let n = s.u32()? as usize;
                    if n > 1_000_000 {
                        return Err(Error::Format("field list too long".into()));
                    }
                    s.skip_u32(n)?;
                }
                0x70 | 0x74 => {
                    let x0 = s.u16()? as usize;
                    let x1 = s.u16()? as usize;
                    s.skip(x1 + 4 * x0)?;
                }
                0xF6 => s.skip_u32(20)?,
                _ if size == 4 => s.skip_u32(1)?,
                _ if size == 8 => s.skip_u32(2)?,
                _ => {
                    return Err(Error::Format(format!(
                        "unknown field type {sub:#04x} of size {size} at {:#x}",
                        s.pos()
                    )))
                }
            }
            Data::Other
        }
        0x04 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            let net = s.u32()?;
            let conn = s.u32()?;
            opt(s, ge(Ver::V174), 1)?;
            Data::NetAssign { net, conn }
        }
        0x05 => {
            s.skip(1)?;
            let layer = layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(9)?;
            opt(s, ge(Ver::V172), 2)?;
            let first_seg = s.u32()?;
            s.skip_u32(2)?;
            Data::Track { layer, first_seg }
        }
        0x06 => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(6)?;
            opt(s, ge(Ver::V172), 1)?;
            Data::Other
        }
        0x07 => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 3)?;
            s.skip_u32(1)?;
            opt(s, lt(Ver::V172), 1)?;
            let refdes = s.u32()?;
            s.skip_u32(4)?;
            Data::CompInst { refdes }
        }
        0x08 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            opt(s, lt(Ver::V172), 1)?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            Data::Other
        }
        0x09 => {
            s.skip(3)?;
            key = s.u32()?;
            s.skip_u32(4)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(5)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x0A => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(4 + 4 + 5)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x0C => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            if lt(Ver::V172) {
                s.skip(4)?;
            } else {
                s.skip_u32(3)?;
            }
            s.skip_u32(1)?;
            opt(s, ge(Ver::V180), 1)?;
            s.skip_u32(2 + 2 + 3)?;
            opt(s, ge(Ver::V174) && lt(Ver::V180), 1)?;
            Data::Other
        }
        0x0D => {
            s.skip(3)?;
            key = s.u32()?;
            let name = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V174), 1)?;
            let x = s.i32()?;
            let y = s.i32()?;
            let padstack = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            let rotation = s.u32()?;
            Data::Pad {
                name,
                x,
                y,
                padstack,
                rotation,
            }
        }
        0x0E => {
            s.u8()?;
            let layer = layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(4)?;
            opt(s, ge(Ver::V172), 2)?;
            let coords = coords4(s)?;
            s.skip_u32(3)?;
            let rotation = s.u32()?;
            Data::Rect {
                layer,
                coords,
                rotation,
            }
        }
        0x0F => {
            s.skip(3)?;
            key = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V174), 1)?;
            if lt(Ver::V190) {
                s.skip(32)?;
            }
            opt(s, ge(Ver::V190), 1)?;
            if ge(Ver::V172) {
                next = s.u32()?;
            }
            s.skip_u32(3)?;
            Data::Other
        }
        0x10 => {
            s.skip(3)?;
            key = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V174), 1)?;
            s.skip_u32(5)?;
            Data::Other
        }
        0x11 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(1)?;
            next = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x12 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(4)?;
            opt(s, ge(Ver::V165), 1)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x14 => {
            s.u8()?;
            let layer = layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            let first_seg = s.u32()?;
            s.skip_u32(2)?;
            Data::Graphic { layer, first_seg }
        }
        0x15..=0x17 => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            let width = s.u32()?;
            let start = (s.i32()?, s.i32()?);
            let end = (s.i32()?, s.i32()?);
            Data::Seg { width, start, end }
        }
        0x1A => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(20)?;
            Data::Other
        }
        0x1B => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            let name = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            let assignment = s.u32()?;
            s.skip_u32(7)?;
            Data::Net { name, assignment }
        }
        0x1C => {
            s.skip(1)?;
            let n = s.u8()? as usize;
            let start_layer = s.u8()?;
            key = s.u32()?;
            next = s.u32()?;
            let name = s.u32()?;
            let (drill, slot, plated, layer_count);
            if lt(Ver::V172) {
                drill = s.u32()?;
                s.skip_u32(5)?;
                s.u8()?;
                let flags = s.u8()?;
                s.skip(2)?;
                s.u16()?;
                s.u16()?;
                s.u16()?;
                layer_count = s.u16()?;
                s.skip_u32(5)?;
                slot = (s.u32()?, s.u32()?);
                s.skip_u32(1)?;
                opt(s, ge(Ver::V165), 1)?;
                plated = flags & 0x01 != 0;
            } else {
                s.skip_u32(3)?;
                s.u8()?;
                s.u8()?;
                let flags = s.u8()?;
                s.u8()?;
                s.skip_u32(2)?;
                s.u16()?;
                s.u16()?;
                layer_count = s.u16()?;
                s.u16()?;
                s.skip_u32(4)?;
                drill = s.u32()?;
                s.skip_u32(2)?;
                slot = (s.u32()?, s.u32()?);
                s.skip_u32(6)?; // travel tolerances, drill mark, drill chars
                s.skip_u32(21)?;
                opt(s, ge(Ver::V180), 8)?;
                plated = flags & 0x20 != 0;
            }
            if layer_count > 256 {
                return Err(Error::Format("padstack has too many layers".into()));
            }
            let fixed = if lt(Ver::V165) {
                10
            } else if lt(Ver::V172) {
                11
            } else {
                21
            };
            let per_layer = if lt(Ver::V172) { 3 } else { 4 };
            let count = fixed + layer_count as usize * per_layer;
            let mut comps = Vec::with_capacity(count);
            for i in 0..count {
                let kind = s.u8()?;
                s.skip(3)?;
                opt(s, ge(Ver::V172), 1)?;
                let w = s.i32()?;
                let h = s.i32()?;
                let z1 = if ge(Ver::V172) { s.i32()? } else { 0 };
                let off_x = s.i32()?;
                let off_y = s.i32()?;
                if ge(Ver::V172) {
                    s.skip_u32(2)?;
                } else {
                    s.skip_u32(1)?;
                    if i + 1 < count {
                        s.skip_u32(1)?;
                    }
                }
                comps.push(PadComp {
                    kind,
                    w,
                    h,
                    z1,
                    off_x,
                    off_y,
                });
            }
            s.skip_u32(n * if lt(Ver::V172) { 8 } else { 10 })?;
            Data::Padstack(Padstack {
                name,
                start_layer,
                layer_count,
                drill,
                slot,
                plated,
                fixed,
                per_layer,
                comps,
            })
        }
        0x1D => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            let a = s.u16()? as usize;
            let b = s.u16()? as usize;
            s.skip(b * if v == Ver::V161 { 48 } else { 56 })?;
            s.skip(a * 256)?;
            opt(s, ge(Ver::V172), 1)?;
            Data::Other
        }
        0x1E => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            if ge(Ver::V161) {
                s.skip(4)?;
            }
            s.skip_u32(1)?;
            let size = s.u32()? as usize;
            s.fixed_str(size, true)?;
            opt(s, ge(Ver::V172), 1)?;
            Data::Other
        }
        0x1F => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(3)?;
            s.u16()?;
            let size = s.u16()? as usize;
            s.skip(if ge(Ver::V175) {
                size * 384 + 8
            } else if ge(Ver::V172) {
                size * 280 + 8
            } else if ge(Ver::V162) {
                size * 280 + 4
            } else {
                size * 240 + 4
            })?;
            Data::Other
        }
        0x20 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(7)?;
            opt(s, ge(Ver::V174), 10)?;
            Data::Other
        }
        0x21 => {
            s.u8()?;
            let sub = s.u16()?;
            let size = s.u32()? as usize;
            if size < 12 {
                return Err(Error::Format("blob block too small".into()));
            }
            key = s.u32()?;
            let body = s.bytes(size - 12)?;
            if std::env::var("ALLEGRO_DEBUG").is_ok() {
                eprintln!("0x21 sub {sub:#06x} size {size}");
            }
            match stack_record(sub, body) {
                Some(r) => Data::Stack(r),
                None => Data::Other,
            }
        }
        0x22 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(8)?;
            Data::Other
        }
        0x23 => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2 + 3 + 5 + 4)?;
            opt(s, ge(Ver::V164), 4)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x24 => {
            s.u8()?;
            let layer = layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            let coords = coords4(s)?;
            s.skip_u32(3)?;
            let rotation = s.u32()?;
            Data::Rect {
                layer,
                coords,
                rotation,
            }
        }
        0x26 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x27 => {
            key = 0;
            if x27_end <= s.pos() {
                return Err(Error::Format("constraint table runs past its end".into()));
            }
            s.seek(x27_end - 1)?;
            Data::Other
        }
        0x28 => {
            s.u8()?;
            let layer = layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(2)?;
            let flags = if ge(Ver::V172) {
                let f = s.u32()?;
                s.skip_u32(1)?;
                f
            } else {
                0
            };
            s.skip_u32(2)?;
            let first_keepout = s.u32()?;
            let first_seg = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            opt(s, lt(Ver::V172), 1)?;
            s.skip_u32(4)?;
            Data::Shape {
                layer,
                first_keepout,
                first_seg,
                flags,
            }
        }
        0x29 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(13)?;
            Data::Other
        }
        0x2A => {
            s.skip(1)?;
            let n = s.u16()? as usize;
            opt(s, ge(Ver::V174), 1)?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                if lt(Ver::V165) {
                    let name = s.fixed_str(32, true)?;
                    list.push((LayerName::Text(name), s.u32()?));
                } else {
                    let id = s.u32()?;
                    let props = s.u32()?;
                    s.skip_u32(1)?;
                    list.push((LayerName::Id(id), props));
                }
            }
            key = s.u32()?;
            Data::LayerList(list)
        }
        0x2B => {
            s.skip(3)?;
            key = s.u32()?;
            let name = s.u32()?;
            s.skip_u32(5)?;
            next = s.u32()?;
            let first_inst = s.u32()?;
            s.skip_u32(7)?;
            opt(s, ge(Ver::V164), 1)?;
            opt(s, ge(Ver::V172), 1)?;
            Data::FpDef { name, first_inst }
        }
        0x2C => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 3)?;
            s.skip_u32(1)?;
            opt(s, lt(Ver::V172), 1)?;
            s.skip_u32(4)?;
            Data::Other
        }
        0x2D => {
            s.u8()?;
            let bottom = s.u8()? != 0;
            s.u8()?;
            key = s.u32()?;
            next = s.u32()?;
            let mut inst_ref = 0;
            if ge(Ver::V172) {
                s.skip_u32(1)?;
            } else {
                inst_ref = s.u32()?;
            }
            s.skip(4)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            let rotation = s.u32()?;
            let x = s.i32()?;
            let y = s.i32()?;
            if ge(Ver::V172) {
                inst_ref = s.u32()?;
            }
            let graphics = s.u32()?;
            let first_pad = s.u32()?;
            s.skip_u32(5)?;
            Data::FpInst {
                bottom,
                rotation,
                x,
                y,
                inst_ref,
                graphics,
                first_pad,
            }
        }
        0x2E => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(6)?;
            opt(s, ge(Ver::V172), 1)?;
            Data::Other
        }
        0x2F => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(6)?;
            Data::Other
        }
        0x30 => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 4)?; // two words, font properties, pointer
            opt(s, ge(Ver::V174), 1)?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            opt(s, lt(Ver::V172), 2)?; // word and font properties
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(4)?;
            opt(s, lt(Ver::V172), 1)?;
            Data::Other
        }
        0x31 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(3)?;
            s.u16()?;
            let len = s.u16()? as usize;
            opt(s, ge(Ver::V174), 1)?;
            s.fixed_str(len, true)?;
            Data::Other
        }
        0x32 => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            let net = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            let next_in_fp = s.u32()?;
            s.skip_u32(2)?;
            let pad = s.u32()?;
            s.skip_u32(4)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(2)?;
            let bbox = coords4(s)?;
            Data::PlacedPad {
                net,
                next_in_fp,
                pad,
                bbox,
            }
        }
        0x33 => {
            s.skip(1)?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            let net = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            let x = s.i32()?;
            let y = s.i32()?;
            s.skip_u32(1)?;
            let padstack = s.u32()?;
            s.skip_u32(4 + 4)?;
            Data::Via {
                net,
                x,
                y,
                padstack,
            }
        }
        0x34 => {
            s.u8()?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V172), 1)?;
            s.skip_u32(1)?;
            let first_seg = s.u32()?;
            s.skip_u32(2)?;
            Data::Keepout { first_seg }
        }
        0x35 => {
            key = 0;
            s.skip(3 + 120)?;
            Data::Other
        }
        0x36 => {
            s.skip(1)?;
            let code = s.u16()?;
            key = s.u32()?;
            next = s.u32()?;
            opt(s, ge(Ver::V172), 1)?;
            let items = s.u32()?;
            let count = s.u32()?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V174), 1)?;
            if items > 1_000_000 || count > items {
                return Err(Error::Format("definition table has a bad size".into()));
            }
            for _ in 0..items {
                match code {
                    0x02 => {
                        s.fixed_str(32, true)?;
                        s.skip_u32(14)?;
                        opt(s, ge(Ver::V164), 3)?;
                        opt(s, ge(Ver::V172), 2)?;
                    }
                    0x03 => {
                        s.fixed_str(if ge(Ver::V172) { 64 } else { 32 }, true)?;
                        opt(s, ge(Ver::V174), 1)?;
                    }
                    0x05 => {
                        s.skip(28)?;
                        opt(s, ge(Ver::V175), 1)?;
                    }
                    0x06 => {
                        s.skip(4)?;
                        s.skip_u32(1)?;
                        opt(s, lt(Ver::V172), 50)?;
                    }
                    0x08 => {
                        s.skip_u32(4)?;
                        opt(s, ge(Ver::V174) && lt(Ver::V190), 1)?;
                        s.skip_u32(4)?;
                        opt(s, ge(Ver::V172), 8)?;
                    }
                    0x0B => s.skip(1016)?,
                    0x0C => s.skip(232)?,
                    0x0D => s.skip(200)?,
                    0x0F => s.skip_u32(5)?,
                    0x10 => {
                        s.skip(108)?;
                        opt(s, ge(Ver::V180), 1)?;
                    }
                    0x12 => s.skip(1052)?,
                    _ => {
                        return Err(Error::Format(format!(
                            "unknown definition table type {code:#04x}"
                        )))
                    }
                }
            }
            Data::Other
        }
        0x37 => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            s.skip_u32(1)?;
            next = s.u32()?;
            s.skip_u32(3)?;
            opt(s, ge(Ver::V174), 1)?;
            s.skip_u32(100)?;
            Data::Other
        }
        0x38 => {
            s.skip(3)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(1)?;
            if lt(Ver::V166) {
                s.fixed_str(20, true)?;
            }
            opt(s, ge(Ver::V166), 2)?;
            s.skip_u32(7)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x39 => {
            s.skip(3)?;
            key = s.u32()?;
            s.skip_u32(2)?;
            s.skip(44)?;
            Data::Other
        }
        0x3A => {
            s.skip(1)?;
            layer(s)?;
            key = s.u32()?;
            next = s.u32()?;
            s.skip_u32(1)?;
            opt(s, ge(Ver::V174), 1)?;
            Data::Other
        }
        0x3B => {
            key = 0;
            s.u8()?;
            s.u16()?;
            let len = s.u32()? as usize;
            s.fixed_str(128, true)?;
            s.fixed_str(32, true)?;
            s.skip_u32(2)?;
            opt(s, ge(Ver::V172), 1)?;
            s.fixed_str(len, true)?;
            Data::Other
        }
        0x3C => {
            s.u8()?;
            s.u16()?;
            key = s.u32()?;
            opt(s, ge(Ver::V174), 1)?;
            let n = s.u32()? as usize;
            if n > 1_000_000 {
                return Err(Error::Format("key list too long".into()));
            }
            s.skip_u32(n)?;
            Data::Other
        }
        0x3E => {
            s.skip(3)?;
            key = s.u32()?;
            s.skip_u32(9)?;
            Data::Other
        }
        _ => return Ok(None),
    };
    Ok(Some(Block {
        kind,
        key,
        next,
        data,
    }))
}

pub struct Db {
    pub header: Header,
    pub strings: HashMap<u32, String>,
    pub blocks: Vec<Block>,
    by_key: HashMap<u32, usize>,
    sentinels: std::collections::HashSet<u32>,
}

impl Db {
    pub fn parse(data: &[u8]) -> Result<Db> {
        let mut s = Stream::new(data);
        let header = parse_header(&mut s)?;
        if header.ver == Ver::Pre16 {
            return Err(Error::Format(format!(
                "this board was saved by {}, older than Allegro 16.0; open and save it in Allegro 16 or newer first",
                header.program
            )));
        }
        if header.divisor == 0 {
            return Err(Error::Format("board units divisor is zero".into()));
        }
        s.seek(0x1200)?;
        let mut strings = HashMap::with_capacity(header.string_count as usize);
        for _ in 0..header.string_count {
            let id = s.u32()?;
            strings.insert(id, s.cstr(true)?);
        }

        let mut blocks = Vec::new();
        let ver = header.ver;
        let x27_end = header.x27_end as usize;
        loop {
            let offset = s.pos();
            let Some(kind) = s.peek_u8() else { break };
            if kind == 0 {
                // End marker, but 18.x files can have zero gaps between groups.
                if ver >= Ver::V180 {
                    let mut p = offset;
                    while data.get(p) == Some(&0) {
                        p += 1;
                    }
                    let start = p - p % 4;
                    if matches!(data.get(start), Some(&b) if (1..=0x3E).contains(&b)) {
                        s.seek(start)?;
                        continue;
                    }
                }
                break;
            }
            s.skip(1)?;
            match parse_block(&mut s, kind, ver, x27_end)? {
                Some(b) => blocks.push(b),
                None => {
                    return Err(Error::Format(format!(
                        "unknown object type {kind:#04x} at offset {offset:#x} (Allegro {})",
                        ver.label()
                    )))
                }
            }
        }
        let by_key = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.key != 0)
            .map(|(i, b)| (b.key, i))
            .collect();
        let sentinels = if ver >= Ver::V180 {
            header.tails.iter().copied().filter(|&t| t != 0).collect()
        } else {
            Default::default()
        };
        Ok(Db {
            header,
            strings,
            blocks,
            by_key,
            sentinels,
        })
    }

    pub fn get(&self, key: u32) -> Option<&Block> {
        self.by_key.get(&key).map(|&i| &self.blocks[i])
    }

    pub fn string(&self, id: u32) -> &str {
        self.strings.get(&id).map(String::as_str).unwrap_or("")
    }

    /// Follows a linked list from `head` until it reaches `tail`, a list end
    /// marker, an unknown key or a key already seen.
    pub fn walk(&self, head: u32, tail: u32) -> Vec<&Block> {
        self.walk_with(head, tail, |b| b.next)
    }

    pub fn walk_with(&self, head: u32, tail: u32, next: impl Fn(&Block) -> u32) -> Vec<&Block> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = head;
        while let Some(b) = self.get(cur) {
            if !seen.insert(cur) {
                break;
            }
            out.push(b);
            if cur == tail {
                break;
            }
            cur = next(b);
            if cur == tail || self.sentinels.contains(&cur) {
                break;
            }
        }
        out
    }
}

/// One layer of the cross-section, as Allegro stores it: fixed 20-byte
/// text fields (a value with a unit, or a bare number in SI units).
#[derive(Clone, Debug, Default)]
pub struct StackRecord {
    /// Copper layer index (0 = top), None for a dielectric or surface.
    pub copper: Option<u8>,
    pub plane: bool,
    /// Position in the stack, from the top surface (0).
    pub position: u32,
    pub material: String,
    pub thickness: String,
    pub conductivity: String,
    pub er: String,
    /// CONDUCTOR, PLANE, DIELECTRIC or SURFACE.
    pub kind: String,
    pub loss_tangent: String,
}

fn stack_record(sub: u16, b: &[u8]) -> Option<StackRecord> {
    // After the key: 4 flag bytes, the position, then text fields.
    if sub != 0x0518 || b.len() < 8 + 20 * 7 {
        return None;
    }
    let text = |i: usize| -> String {
        let f = &b[8 + 20 * i..8 + 20 * (i + 1)];
        let end = f.iter().position(|&c| c == 0).unwrap_or(f.len());
        String::from_utf8_lossy(&f[..end]).trim().to_string()
    };
    let kind = text(5).to_ascii_uppercase();
    if !matches!(
        kind.as_str(),
        "CONDUCTOR" | "PLANE" | "DIELECTRIC" | "SURFACE"
    ) {
        return None;
    }
    Some(StackRecord {
        copper: (b[0] != 0xfd && matches!(kind.as_str(), "CONDUCTOR" | "PLANE")).then_some(b[0]),
        plane: kind == "PLANE",
        position: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        material: text(0),
        thickness: text(1),
        conductivity: text(3),
        er: text(4),
        kind,
        loss_tangent: text(6),
    })
}
