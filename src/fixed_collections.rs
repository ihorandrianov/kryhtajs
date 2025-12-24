//! Fixed-capacity collections for no_std

use core::mem::MaybeUninit;

pub struct FixedStack<T: Copy, const N: usize> {
    data: [MaybeUninit<T>; N],
    len: usize,
}

impl<T: Copy, const N: usize> FixedStack<T, N> {
    pub const fn new() -> Self {
        Self { data: unsafe { MaybeUninit::uninit().assume_init() }, len: 0 }
    }

    #[inline(always)]
    pub fn push(&mut self, value: T) -> bool {
        if self.len >= N { return false; }
        self.data[self.len].write(value);
        self.len += 1;
        true
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 { return None; }
        self.len -= 1;
        Some(unsafe { self.data[self.len].assume_init_read() })
    }

    #[inline(always)]
    pub fn peek(&self) -> Option<&T> {
        if self.len == 0 { return None; }
        Some(unsafe { self.data[self.len - 1].assume_init_ref() })
    }

    #[inline(always)]
    pub fn peek_at(&self, offset: usize) -> Option<&T> {
        if offset >= self.len { return None; }
        Some(unsafe { self.data[self.len - 1 - offset].assume_init_ref() })
    }

    #[inline(always)]
    pub fn get(&self, idx: usize) -> Option<&T> {
        if idx >= self.len { return None; }
        Some(unsafe { self.data[idx].assume_init_ref() })
    }

    #[inline(always)]
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut T> {
        if idx >= self.len { return None; }
        Some(unsafe { self.data[idx].assume_init_mut() })
    }

    #[inline(always)]
    pub fn len(&self) -> usize { self.len }

    #[inline(always)]
    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn clear(&mut self) { self.len = 0; }

    pub fn truncate(&mut self, new_len: usize) {
        if new_len < self.len { self.len = new_len; }
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        if a < self.len && b < self.len && a != b {
            unsafe {
                let va = self.data[a].assume_init_read();
                let vb = self.data[b].assume_init_read();
                self.data[a].write(vb);
                self.data[b].write(va);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len).map(move |i| unsafe { self.data[i].assume_init_ref() })
    }
}

impl<T: Copy, const N: usize> Default for FixedStack<T, N> {
    fn default() -> Self { Self::new() }
}

pub struct FixedVec<T: Copy, const N: usize> {
    data: [MaybeUninit<T>; N],
    len: usize,
}

impl<T: Copy, const N: usize> FixedVec<T, N> {
    pub const fn new() -> Self {
        Self { data: unsafe { MaybeUninit::uninit().assume_init() }, len: 0 }
    }

    #[inline(always)]
    pub fn push(&mut self, value: T) -> bool {
        if self.len >= N { return false; }
        self.data[self.len].write(value);
        self.len += 1;
        true
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 { return None; }
        self.len -= 1;
        Some(unsafe { self.data[self.len].assume_init_read() })
    }

    #[inline(always)]
    pub fn get(&self, idx: usize) -> Option<T> {
        if idx >= self.len { return None; }
        Some(unsafe { self.data[idx].assume_init_read() })
    }

    #[inline(always)]
    pub fn set(&mut self, idx: usize, value: T) -> bool {
        if idx >= self.len { return false; }
        self.data[idx].write(value);
        true
    }

    #[inline(always)]
    pub fn len(&self) -> usize { self.len }

    #[inline(always)]
    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn clear(&mut self) { self.len = 0; }

    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        (0..self.len).map(move |i| unsafe { self.data[i].assume_init_read() })
    }

    pub fn clone_to<const M: usize>(&self, other: &mut FixedVec<T, M>) {
        other.clear();
        for i in 0..self.len.min(M) {
            other.push(unsafe { self.data[i].assume_init_read() });
        }
    }

    pub fn as_slice(&self) -> &[T] {
        unsafe { core::slice::from_raw_parts(self.data.as_ptr() as *const T, self.len) }
    }

    pub fn last(&self) -> Option<T> {
        if self.len == 0 { None } else { Some(unsafe { self.data[self.len - 1].assume_init_read() }) }
    }
}

impl<T: Copy, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self { Self::new() }
}

impl<T: Copy, const N: usize> Clone for FixedVec<T, N> {
    fn clone(&self) -> Self {
        let mut new = Self::new();
        for i in 0..self.len {
            new.push(unsafe { self.data[i].assume_init_read() });
        }
        new
    }
}

pub struct FixedMap<K: Copy + Eq, V: Copy, const N: usize> {
    keys: [MaybeUninit<K>; N],
    values: [MaybeUninit<V>; N],
    occupied: [bool; N],
    len: usize,
}

impl<K: Copy + Eq, V: Copy, const N: usize> FixedMap<K, V, N> {
    pub const fn new() -> Self {
        Self {
            keys: unsafe { MaybeUninit::uninit().assume_init() },
            values: unsafe { MaybeUninit::uninit().assume_init() },
            occupied: [false; N],
            len: 0,
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> bool {
        for i in 0..N {
            if self.occupied[i] {
                let k = unsafe { self.keys[i].assume_init_read() };
                if k == key {
                    self.values[i].write(value);
                    return true;
                }
            }
        }
        for i in 0..N {
            if !self.occupied[i] {
                self.keys[i].write(key);
                self.values[i].write(value);
                self.occupied[i] = true;
                self.len += 1;
                return true;
            }
        }
        false
    }

    pub fn get(&self, key: &K) -> Option<V> {
        for i in 0..N {
            if self.occupied[i] {
                if unsafe { self.keys[i].assume_init_ref() } == key {
                    return Some(unsafe { self.values[i].assume_init_read() });
                }
            }
        }
        None
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        for i in 0..N {
            if self.occupied[i] {
                if unsafe { self.keys[i].assume_init_ref() } == key {
                    return Some(unsafe { self.values[i].assume_init_mut() });
                }
            }
        }
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        for i in 0..N {
            if self.occupied[i] {
                if unsafe { self.keys[i].assume_init_ref() } == key {
                    let v = unsafe { self.values[i].assume_init_read() };
                    self.occupied[i] = false;
                    self.len -= 1;
                    return Some(v);
                }
            }
        }
        None
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn clear(&mut self) {
        for i in 0..N { self.occupied[i] = false; }
        self.len = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = (K, V)> + '_ {
        (0..N).filter_map(move |i| {
            if self.occupied[i] {
                Some((unsafe { self.keys[i].assume_init_read() }, unsafe { self.values[i].assume_init_read() }))
            } else { None }
        })
    }

    pub fn values(&self) -> impl Iterator<Item = V> + '_ { self.iter().map(|(_, v)| v) }
}

impl<K: Copy + Eq, V: Copy, const N: usize> Default for FixedMap<K, V, N> {
    fn default() -> Self { Self::new() }
}
