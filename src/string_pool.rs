//! Interned string pool with O(1) lookup

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct StrId(pub u32);

impl StrId {
    pub const EMPTY: StrId = StrId(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

pub struct StringPool {
    strings: Vec<String>,
    intern_map: HashMap<String, StrId>,
    marks: Vec<bool>,
}

impl StringPool {
    pub fn new() -> Self {
        let mut pool = Self {
            strings: Vec::new(),
            intern_map: HashMap::new(),
            marks: Vec::new(),
        };
        pool.intern("");
        pool
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut pool = Self {
            strings: Vec::with_capacity(capacity),
            intern_map: HashMap::with_capacity(capacity),
            marks: Vec::with_capacity(capacity),
        };
        pool.intern("");
        pool
    }

    pub fn intern(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.intern_map.get(s) {
            return id;
        }

        let id = StrId(self.strings.len() as u32);
        self.strings.push(s.to_string());
        self.intern_map.insert(s.to_string(), id);
        self.marks.push(false);
        id
    }

    pub fn get(&self, id: StrId) -> Option<&str> {
        self.strings.get(id.index()).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    pub fn clear_marks(&mut self) {
        for mark in &mut self.marks {
            *mark = false;
        }
        if !self.marks.is_empty() {
            self.marks[0] = true;
        }
    }

    pub fn mark(&mut self, id: StrId) {
        if let Some(mark) = self.marks.get_mut(id.index()) {
            *mark = true;
        }
    }

    pub fn is_marked(&self, id: StrId) -> bool {
        self.marks.get(id.index()).copied().unwrap_or(false)
    }
}

impl Default for StringPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for StringPool {
    fn clone(&self) -> Self {
        Self {
            strings: self.strings.clone(),
            intern_map: self.intern_map.clone(),
            marks: self.marks.clone(),
        }
    }
}
