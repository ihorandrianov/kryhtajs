//! Snapshot serialization: whole-runtime durable execution.
//!
//! Format: magic "KRHT", version u8, little-endian, length-prefixed.

use crate::error::{JSError, Result};

pub struct ByteWriter {
    buf: Vec<u8>,
}

pub struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool_(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn str_(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

fn truncated() -> JSError {
    JSError::Message("snapshot: truncated input".to_string())
}

impl<'a> ByteReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(truncated)?;
        if end > self.buf.len() {
            return Err(truncated());
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bool_(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn str_(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| JSError::Message("snapshot: invalid UTF-8".to_string()))
    }

    pub fn is_at_end(&self) -> bool {
        self.pos == self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut w = ByteWriter::new();
        w.u8(7);
        w.u16(65_000);
        w.u32(4_000_000_000);
        w.u64(u64::MAX);
        w.i32(-42);
        w.f64(-0.5);
        w.bool_(true);
        w.str_("крихта");
        let bytes = w.finish();

        let mut r = ByteReader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 65_000);
        assert_eq!(r.u32().unwrap(), 4_000_000_000);
        assert_eq!(r.u64().unwrap(), u64::MAX);
        assert_eq!(r.i32().unwrap(), -42);
        assert_eq!(r.f64().unwrap(), -0.5);
        assert!(r.bool_().unwrap());
        assert_eq!(r.str_().unwrap(), "крихта");
        assert!(r.is_at_end());
    }

    #[test]
    fn truncated_input_errors_not_panics() {
        let mut w = ByteWriter::new();
        w.u32(1234);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes[..2]);
        assert!(r.u32().is_err());
    }

    #[test]
    fn bogus_string_length_errors() {
        let mut w = ByteWriter::new();
        w.u32(u32::MAX); // claims a 4GB string
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert!(r.str_().is_err());
    }
}
