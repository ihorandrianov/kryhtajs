//! Fixed-capacity string pool with interning

use core::str;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct StrId(pub u16);

impl StrId {
    pub const EMPTY: StrId = StrId(0);
    pub const NONE: StrId = StrId(u16::MAX);
    pub fn is_none(self) -> bool { self.0 == u16::MAX }
    pub fn is_some(self) -> bool { self.0 != u16::MAX }
}

#[derive(Clone, Copy, Default)]
struct StringEntry { offset: u16, len: u16 }

pub struct FixedStringPool<const BUFFER_SIZE: usize, const MAX_STRINGS: usize> {
    buffer: [u8; BUFFER_SIZE],
    entries: [StringEntry; MAX_STRINGS],
    count: u16,
    used: u16,
    marks: [bool; MAX_STRINGS],
}

impl<const BUFFER_SIZE: usize, const MAX_STRINGS: usize>
    FixedStringPool<BUFFER_SIZE, MAX_STRINGS>
{
    pub const fn new() -> Self {
        Self {
            buffer: [0; BUFFER_SIZE],
            entries: [StringEntry { offset: 0, len: 0 }; MAX_STRINGS],
            count: 0,
            used: 0,
            marks: [false; MAX_STRINGS],
        }
    }

    pub fn init(&mut self) {
        if self.count == 0 {
            self.entries[0] = StringEntry { offset: 0, len: 0 };
            self.count = 1;
        }
    }

    pub fn intern(&mut self, s: &str) -> Option<StrId> {
        for i in 0..self.count as usize {
            if self.get_str(i as u16) == Some(s) {
                return Some(StrId(i as u16));
            }
        }
        let bytes = s.as_bytes();
        let len = bytes.len();
        if self.count as usize >= MAX_STRINGS || self.used as usize + len > BUFFER_SIZE {
            return None;
        }
        let offset = self.used;
        self.buffer[offset as usize..offset as usize + len].copy_from_slice(bytes);
        let id = self.count;
        self.entries[id as usize] = StringEntry { offset, len: len as u16 };
        self.count += 1;
        self.used += len as u16;
        Some(StrId(id))
    }

    pub fn get(&self, id: StrId) -> Option<&str> { self.get_str(id.0) }

    fn get_str(&self, idx: u16) -> Option<&str> {
        if idx as usize >= self.count as usize { return None; }
        let entry = &self.entries[idx as usize];
        let start = entry.offset as usize;
        let end = start + entry.len as usize;
        Some(unsafe { str::from_utf8_unchecked(&self.buffer[start..end]) })
    }

    pub fn len(&self) -> usize { self.count as usize }
    pub fn is_empty(&self) -> bool { self.count == 0 }
    pub fn bytes_used(&self) -> usize { self.used as usize }

    pub fn clear_marks(&mut self) {
        for i in 0..self.count as usize { self.marks[i] = false; }
        if self.count > 0 { self.marks[0] = true; }
    }

    #[inline(always)]
    pub fn mark(&mut self, id: StrId) {
        if (id.0 as usize) < self.count as usize { self.marks[id.0 as usize] = true; }
    }

    #[inline(always)]
    pub fn is_marked(&self, id: StrId) -> bool {
        if (id.0 as usize) < self.count as usize { self.marks[id.0 as usize] } else { false }
    }

    pub fn reset(&mut self) {
        self.count = 1;
        self.used = 0;
        self.marks = [false; MAX_STRINGS];
        self.marks[0] = true;
    }
}

impl<const B: usize, const S: usize> Default for FixedStringPool<B, S> {
    fn default() -> Self {
        let mut pool = Self::new();
        pool.init();
        pool
    }
}
