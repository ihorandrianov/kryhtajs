//! Growable arena allocator with index-based references

use std::marker::PhantomData;

/// Arena ID - an index-based reference that is always Copy
/// regardless of the type parameter T.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ArenaId<T> {
    index: u32,
    _marker: PhantomData<fn() -> T>,
}

// Manual Clone/Copy implementations that don't require T: Clone/Copy
impl<T> Clone for ArenaId<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            _marker: PhantomData,
        }
    }
}

impl<T> Copy for ArenaId<T> {}

impl<T> ArenaId<T> {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            _marker: PhantomData,
        }
    }

    pub fn index(&self) -> usize {
        self.index as usize
    }
}

impl<T> Default for ArenaId<T> {
    fn default() -> Self {
        Self::new(u32::MAX)
    }
}

pub struct Arena<T> {
    data: Vec<T>,
    free_list: Vec<u32>,
    marks: Vec<bool>,
    allocations: u64,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            free_list: Vec::new(),
            marks: Vec::new(),
            allocations: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            free_list: Vec::new(),
            marks: Vec::with_capacity(capacity),
            allocations: 0,
        }
    }

    /// Cumulative allocation count (never reset) — the GC compares
    /// this against a snapshot to decide when to collect.
    pub fn allocations(&self) -> u64 {
        self.allocations
    }

    pub fn alloc(&mut self, value: T) -> ArenaId<T> {
        self.allocations += 1;
        if let Some(index) = self.free_list.pop() {
            self.data[index as usize] = value;
            self.marks[index as usize] = false;
            ArenaId::new(index)
        } else {
            let index = self.data.len() as u32;
            self.data.push(value);
            self.marks.push(false);
            ArenaId::new(index)
        }
    }

    pub fn get(&self, id: ArenaId<T>) -> Option<&T> {
        self.data.get(id.index())
    }

    pub fn get_mut(&mut self, id: ArenaId<T>) -> Option<&mut T> {
        self.data.get_mut(id.index())
    }

    pub fn take(&mut self, id: ArenaId<T>) -> Option<T>
    where
        T: Default,
    {
        self.data
            .get_mut(id.index())
            .map(|slot| std::mem::take(slot))
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn mark(&mut self, id: ArenaId<T>) {
        if let Some(mark) = self.marks.get_mut(id.index()) {
            *mark = true;
        }
    }

    pub fn sweep(&mut self) {
        self.free_list.clear();
        for (i, mark) in self.marks.iter_mut().enumerate() {
            if !*mark {
                self.free_list.push(i as u32);
            }
            *mark = false;
        }
    }

    pub fn is_marked(&self, id: ArenaId<T>) -> bool {
        self.marks.get(id.index()).copied().unwrap_or(false)
    }

    pub fn iter(&self) -> impl Iterator<Item = (ArenaId<T>, &T)> {
        self.data
            .iter()
            .enumerate()
            .map(|(i, v)| (ArenaId::new(i as u32), v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ArenaId<T>, &mut T)> {
        self.data
            .iter_mut()
            .enumerate()
            .map(|(i, v)| (ArenaId::new(i as u32), v))
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for Arena<T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            free_list: self.free_list.clone(),
            marks: self.marks.clone(),
            allocations: self.allocations,
        }
    }
}
