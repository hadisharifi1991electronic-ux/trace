//! Little-endian reader over the whole file, with bounds checks.

use crate::{Error, Result};

pub struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Stream { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(Error::Format(format!("seek past end of file to {pos:#x}")));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| {
                Error::Format(format!(
                    "unexpected end of file reading {n} bytes at {:#x}",
                    self.pos
                ))
            })?;
        let b = &self.data[self.pos..end];
        self.pos = end;
        Ok(b)
    }

    /// Next byte, or `None` at the end of the file.
    pub fn peek_u8(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn skip_u32(&mut self, n: usize) -> Result<()> {
        self.skip(4 * n)
    }

    /// Allegro stores doubles as two little-endian words, high word first.
    pub fn f64(&mut self) -> Result<f64> {
        let hi = self.u32()? as u64;
        let lo = self.u32()? as u64;
        Ok(f64::from_bits((hi << 32) | lo))
    }

    fn align4(&mut self) -> Result<()> {
        let pad = (4 - self.pos % 4) % 4;
        self.skip(pad)
    }

    /// A NUL-terminated string, optionally padded to the next 4-byte boundary.
    pub fn cstr(&mut self, align: bool) -> Result<String> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let n = rest
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| Error::Format(format!("unterminated string at {:#x}", self.pos)))?;
        let s = latin1(&rest[..n]);
        self.pos += n + 1;
        if align {
            self.align4()?;
        }
        Ok(s)
    }

    /// A string in a fixed-size field, optionally padded to 4 bytes after.
    pub fn fixed_str(&mut self, len: usize, align: bool) -> Result<String> {
        let b = self.bytes(len)?;
        let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        let s = latin1(&b[..n]);
        if align {
            self.align4()?;
        }
        Ok(s)
    }
}

/// Allegro text is 8-bit; keep valid UTF-8 as is, otherwise treat as Latin-1.
fn latin1(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}
