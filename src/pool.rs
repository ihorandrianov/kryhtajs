//! Static memory pool with free list

use core::mem::MaybeUninit;

pub struct Pool<T, const N: usize> {
    storage: [MaybeUninit<T>; N],
    next_free: [u16; N],
    free_head: u16,
    len: u16,
    marks: [bool; N],
}

impl<T, const N: usize> Pool<T, N> {
    const NONE: u16 = u16::MAX;

    pub const fn new() -> Self {
        Self {
            storage: unsafe { MaybeUninit::uninit().assume_init() },
            next_free: [0; N],
            free_head: Self::NONE,
            len: 0,
            marks: [false; N],
        }
    }

    pub fn alloc(&mut self, value: T) -> Option<u16> {
        let idx = if self.free_head != Self::NONE {
            let idx = self.free_head;
            self.free_head = self.next_free[idx as usize];
            idx
        } else if (self.len as usize) < N {
            let idx = self.len;
            self.len += 1;
            idx
        } else {
            return None;
        };
        self.storage[idx as usize].write(value);
        self.marks[idx as usize] = false;
        Some(idx)
    }

    pub fn free(&mut self, idx: u16) {
        debug_assert!((idx as usize) < self.len as usize);
        unsafe { self.storage[idx as usize].assume_init_drop() };
        self.next_free[idx as usize] = self.free_head;
        self.free_head = idx;
    }

    #[inline(always)]
    pub fn get(&self, idx: u16) -> Option<&T> {
        if (idx as usize) < self.len as usize {
            Some(unsafe { self.storage[idx as usize].assume_init_ref() })
        } else { None }
    }

    #[inline(always)]
    pub fn get_mut(&mut self, idx: u16) -> Option<&mut T> {
        if (idx as usize) < self.len as usize {
            Some(unsafe { self.storage[idx as usize].assume_init_mut() })
        } else { None }
    }

    #[inline(always)]
    pub unsafe fn get_unchecked(&self, idx: u16) -> &T {
        self.storage.get_unchecked(idx as usize).assume_init_ref()
    }

    #[inline(always)]
    pub unsafe fn get_unchecked_mut(&mut self, idx: u16) -> &mut T {
        self.storage.get_unchecked_mut(idx as usize).assume_init_mut()
    }

    pub fn len(&self) -> usize { self.len as usize }
    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub const fn capacity(&self) -> usize { N }

    pub fn clear_marks(&mut self) {
        for i in 0..self.len as usize { self.marks[i] = false; }
    }

    #[inline(always)]
    pub fn mark(&mut self, idx: u16) {
        if (idx as usize) < self.len as usize { self.marks[idx as usize] = true; }
    }

    #[inline(always)]
    pub fn is_marked(&self, idx: u16) -> bool {
        if (idx as usize) < self.len as usize { self.marks[idx as usize] } else { false }
    }

    pub fn sweep(&mut self) -> usize where T: Default {
        let mut freed = 0;
        for i in 0..self.len as usize {
            if !self.marks[i] && !self.is_in_free_list(i as u16) {
                self.free(i as u16);
                freed += 1;
            }
        }
        freed
    }

    fn is_in_free_list(&self, idx: u16) -> bool {
        let mut current = self.free_head;
        while current != Self::NONE {
            if current == idx { return true; }
            current = self.next_free[current as usize];
        }
        false
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &T)> {
        (0..self.len).filter_map(move |i| {
            if !self.is_in_free_list(i) { self.get(i).map(|v| (i, v)) } else { None }
        })
    }
}

impl<T: Default, const N: usize> Default for Pool<T, N> {
    fn default() -> Self { Self::new() }
}
