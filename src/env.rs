//! Environment - variable bindings for the CEKH machine
//!
//! Environments form an immutable chain of binding frames.
//! Each frame maps variable names (StrId) to values (JSValue).
//! Closures capture environments by storing EnvId.

use std::collections::HashMap;

use crate::arena::{Arena, ArenaId};
use crate::string_pool::StrId;
use crate::value::JSValue;

/// Environment ID - index into the environment arena
pub type EnvId = ArenaId<Env>;

const SMALL_ENV_CAPACITY: usize = 2;

/// Bindings storage - inline for small environments, heap for large
#[derive(Clone, Debug, PartialEq)]
enum Bindings {
    Small {
        entries: [(StrId, JSValue); SMALL_ENV_CAPACITY],
        len: u8,
    },
    Large(HashMap<StrId, JSValue>),
}

impl Bindings {
    fn new() -> Self {
        Bindings::Small {
            entries: [(StrId(0), JSValue::Undefined); SMALL_ENV_CAPACITY],
            len: 0,
        }
    }

    fn get(&self, name: StrId) -> Option<JSValue> {
        match self {
            Bindings::Small { entries, len } => {
                for i in 0..(*len as usize) {
                    if entries[i].0 == name {
                        return Some(entries[i].1);
                    }
                }
                None
            }
            Bindings::Large(map) => map.get(&name).copied(),
        }
    }

    fn insert(&mut self, name: StrId, value: JSValue) {
        match self {
            Bindings::Small { entries, len } => {
                for i in 0..(*len as usize) {
                    if entries[i].0 == name {
                        entries[i].1 = value;
                        return;
                    }
                }
                if (*len as usize) < SMALL_ENV_CAPACITY {
                    entries[*len as usize] = (name, value);
                    *len += 1;
                } else {
                    let mut map = HashMap::with_capacity(SMALL_ENV_CAPACITY + 1);
                    for i in 0..SMALL_ENV_CAPACITY {
                        map.insert(entries[i].0, entries[i].1);
                    }
                    map.insert(name, value);
                    *self = Bindings::Large(map);
                }
            }
            Bindings::Large(map) => {
                map.insert(name, value);
            }
        }
    }

    fn contains(&self, name: StrId) -> bool {
        match self {
            Bindings::Small { entries, len } => {
                for i in 0..(*len as usize) {
                    if entries[i].0 == name {
                        return true;
                    }
                }
                false
            }
            Bindings::Large(map) => map.contains_key(&name),
        }
    }

    fn to_hashmap(&self) -> HashMap<StrId, JSValue> {
        match self {
            Bindings::Small { entries, len } => {
                let mut map = HashMap::with_capacity(*len as usize);
                for i in 0..(*len as usize) {
                    map.insert(entries[i].0, entries[i].1);
                }
                map
            }
            Bindings::Large(map) => map.clone(),
        }
    }

    fn iter(&self) -> BindingsIter<'_> {
        match self {
            Bindings::Small { entries, len } => BindingsIter::Small {
                entries,
                len: *len as usize,
                index: 0,
            },
            Bindings::Large(map) => BindingsIter::Large(map.iter()),
        }
    }
}

enum BindingsIter<'a> {
    Small {
        entries: &'a [(StrId, JSValue); SMALL_ENV_CAPACITY],
        len: usize,
        index: usize,
    },
    Large(std::collections::hash_map::Iter<'a, StrId, JSValue>),
}

impl<'a> Iterator for BindingsIter<'a> {
    type Item = (StrId, JSValue);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            BindingsIter::Small {
                entries,
                len,
                index,
            } => {
                if *index < *len {
                    let (k, v) = entries[*index];
                    *index += 1;
                    Some((k, v))
                } else {
                    None
                }
            }
            BindingsIter::Large(iter) => iter.next().map(|(&k, &v)| (k, v)),
        }
    }
}

/// A single environment frame
#[derive(Clone, Debug, PartialEq)]
pub struct Env {
    /// Variable bindings in this frame
    bindings: Bindings,
    /// Parent environment (lexical scope chain)
    parent: Option<EnvId>,
}

impl Env {
    /// Create an empty environment (global scope)
    pub fn empty() -> Self {
        Self {
            bindings: Bindings::new(),
            parent: None,
        }
    }

    /// Create an environment with a parent
    pub fn with_parent(parent: EnvId) -> Self {
        Self {
            bindings: Bindings::new(),
            parent: Some(parent),
        }
    }

    /// Create an environment with bindings and parent
    pub fn with_bindings(bindings: HashMap<StrId, JSValue>, parent: Option<EnvId>) -> Self {
        if bindings.len() <= SMALL_ENV_CAPACITY {
            let mut small_bindings = Bindings::new();
            for (k, v) in bindings {
                small_bindings.insert(k, v);
            }
            Self {
                bindings: small_bindings,
                parent,
            }
        } else {
            Self {
                bindings: Bindings::Large(bindings),
                parent,
            }
        }
    }

    /// Create an environment with a slice of bindings (avoids HashMap allocation)
    pub fn with_binding_slice(bindings: &[(StrId, JSValue)], parent: Option<EnvId>) -> Self {
        if bindings.len() <= SMALL_ENV_CAPACITY {
            let mut small_bindings = Bindings::new();
            for &(k, v) in bindings {
                small_bindings.insert(k, v);
            }
            Self {
                bindings: small_bindings,
                parent,
            }
        } else {
            let map: HashMap<_, _> = bindings.iter().copied().collect();
            Self {
                bindings: Bindings::Large(map),
                parent,
            }
        }
    }
}

/// Environment operations
impl Env {
    /// Get binding in this frame only (not parent)
    pub fn get_local(&self, name: StrId) -> Option<JSValue> {
        self.bindings.get(name)
    }

    /// Set binding in this frame
    pub fn set_local(&mut self, name: StrId, value: JSValue) {
        self.bindings.insert(name, value);
    }

    /// Check if binding exists in this frame
    pub fn has_local(&self, name: StrId) -> bool {
        self.bindings.contains(name)
    }

    /// Get parent environment
    pub fn parent(&self) -> Option<EnvId> {
        self.parent
    }

    pub fn get_bindings(&self) -> HashMap<StrId, JSValue> {
        self.bindings.to_hashmap()
    }

    pub fn iter_bindings(&self) -> impl Iterator<Item = (StrId, JSValue)> + '_ {
        self.bindings.iter()
    }
}

/// Environment arena with lookup operations
pub struct EnvArena {
    arena: Arena<Env>,
    /// Global environment ID (always 0)
    global: EnvId,
}

impl EnvArena {
    pub fn new() -> Self {
        let mut arena = Arena::new();
        let global = arena.alloc(Env::empty());
        Self { arena, global }
    }

    pub fn arena(&self) -> &Arena<Env> {
        &self.arena
    }

    /// Rebuild from a deserialized arena; slot 0 is the global env.
    pub fn from_arena(arena: Arena<Env>) -> Self {
        Self {
            arena,
            global: ArenaId::new(0),
        }
    }

    /// Get the global environment
    pub fn global(&self) -> EnvId {
        self.global
    }

    pub fn allocations(&self) -> u64 {
        self.arena.allocations()
    }

    /// Allocate a new empty environment with parent
    pub fn extend(&mut self, parent: EnvId) -> EnvId {
        self.arena.alloc(Env::with_parent(parent))
    }

    pub fn is_marked(&self, id: ArenaId<Env>) -> bool {
        self.arena.is_marked(id)
    }

    pub fn mark(&mut self, id: ArenaId<Env>) {
        self.arena.mark(id);
    }

    pub fn sweep(&mut self) {
        self.arena.sweep()
    }

    /// Allocate a new environment with bindings
    pub fn extend_with(&mut self, parent: EnvId, bindings: HashMap<StrId, JSValue>) -> EnvId {
        self.arena.alloc(Env::with_bindings(bindings, Some(parent)))
    }

    /// Allocate a new environment with bindings from a slice (avoids HashMap allocation)
    pub fn extend_with_slice(&mut self, parent: EnvId, bindings: &[(StrId, JSValue)]) -> EnvId {
        self.arena
            .alloc(Env::with_binding_slice(bindings, Some(parent)))
    }

    /// Create a child environment with a single binding
    pub fn bind(&mut self, parent: EnvId, name: StrId, value: JSValue) -> EnvId {
        self.arena
            .alloc(Env::with_binding_slice(&[(name, value)], Some(parent)))
    }

    /// Look up a variable, walking the environment chain
    pub fn lookup(&self, env_id: EnvId, name: StrId) -> Option<JSValue> {
        let mut current = Some(env_id);
        while let Some(id) = current {
            if let Some(env) = self.arena.get(id) {
                if let Some(value) = env.get_local(name) {
                    return Some(value);
                }
                current = env.parent();
            } else {
                break;
            }
        }
        None
    }

    /// Set a variable, searching the chain for existing binding
    /// Returns true if binding was found and updated, false if not found
    pub fn assign(&mut self, env_id: EnvId, name: StrId, value: JSValue) -> bool {
        let mut current = Some(env_id);
        while let Some(id) = current {
            if let Some(env) = self.arena.get(id) {
                if env.has_local(name) {
                    // Found binding, update it
                    if let Some(env_mut) = self.arena.get_mut(id) {
                        env_mut.set_local(name, value);
                    }
                    return true;
                }
                current = env.parent();
            } else {
                break;
            }
        }
        false
    }

    /// Define a new variable in the given environment frame
    pub fn define(&mut self, env_id: EnvId, name: StrId, value: JSValue) {
        if let Some(env) = self.arena.get_mut(env_id) {
            env.set_local(name, value);
        }
    }

    /// Get direct access to an environment
    pub fn get(&self, env_id: EnvId) -> Option<&Env> {
        self.arena.get(env_id)
    }

    /// Get mutable access to an environment
    pub fn get_mut(&mut self, env_id: EnvId) -> Option<&mut Env> {
        self.arena.get_mut(env_id)
    }
}

impl Default for EnvArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_chain() {
        let mut envs = EnvArena::new();
        let name_x = StrId(1);
        let name_y = StrId(2);

        // Global: x = 1
        envs.define(envs.global(), name_x, JSValue::Int(1));

        // Child: y = 2
        let child = envs.extend(envs.global());
        envs.define(child, name_y, JSValue::Int(2));

        // Can see both x and y from child
        assert_eq!(envs.lookup(child, name_x), Some(JSValue::Int(1)));
        assert_eq!(envs.lookup(child, name_y), Some(JSValue::Int(2)));

        // Global can only see x
        assert_eq!(envs.lookup(envs.global(), name_x), Some(JSValue::Int(1)));
        assert_eq!(envs.lookup(envs.global(), name_y), None);
    }

    #[test]
    fn test_assignment() {
        let mut envs = EnvArena::new();
        let name_x = StrId(1);

        // Global: x = 1
        envs.define(envs.global(), name_x, JSValue::Int(1));

        // Child env
        let child = envs.extend(envs.global());

        // Assignment from child modifies parent's binding
        assert!(envs.assign(child, name_x, JSValue::Int(42)));
        assert_eq!(envs.lookup(envs.global(), name_x), Some(JSValue::Int(42)));
    }

    #[test]
    fn test_shadowing() {
        let mut envs = EnvArena::new();
        let name_x = StrId(1);

        // Global: x = 1
        envs.define(envs.global(), name_x, JSValue::Int(1));

        // Child: x = 2 (shadowing)
        let child = envs.extend(envs.global());
        envs.define(child, name_x, JSValue::Int(2));

        // Child sees shadowed value
        assert_eq!(envs.lookup(child, name_x), Some(JSValue::Int(2)));
        // Global still has original
        assert_eq!(envs.lookup(envs.global(), name_x), Some(JSValue::Int(1)));
    }
}
