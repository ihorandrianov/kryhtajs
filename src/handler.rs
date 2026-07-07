//! Handler infrastructure for algebraic effects

use crate::ExprId;
use crate::arena::{Arena, ArenaId};
use crate::env::EnvId;
use crate::string_pool::StrId;

pub type HandlerId = ArenaId<Handler>;

#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    pub clauses_start: u32,
    pub clauses_count: u16,
    pub return_param: StrId,
    pub return_body: ExprId,
    pub env: EnvId,
}

impl Handler {
    pub fn new(
        clauses_start: u32,
        clauses_count: u16,
        return_param: StrId,
        return_body: ExprId,
        env: EnvId,
    ) -> Self {
        Self {
            clauses_start,
            clauses_count,
            return_param,
            return_body,
            env,
        }
    }
}

pub struct HandlerArena {
    arena: Arena<Handler>,
}

impl HandlerArena {
    pub fn new() -> Self {
        Self {
            arena: Arena::new(),
        }
    }

    pub fn arena(&self) -> &Arena<Handler> {
        &self.arena
    }

    pub fn from_arena(arena: Arena<Handler>) -> Self {
        Self { arena }
    }

    pub fn alloc(&mut self, handler: Handler) -> HandlerId {
        self.arena.alloc(handler)
    }

    pub fn allocations(&self) -> u64 {
        self.arena.allocations()
    }

    pub fn get(&self, id: HandlerId) -> Option<&Handler> {
        self.arena.get(id)
    }

    pub fn get_mut(&mut self, id: HandlerId) -> Option<&mut Handler> {
        self.arena.get_mut(id)
    }

    pub fn is_marked(&self, id: HandlerId) -> bool {
        self.arena.is_marked(id)
    }

    pub fn mark(&mut self, id: HandlerId) {
        self.arena.mark(id)
    }

    pub fn sweep(&mut self) {
        self.arena.sweep()
    }
}

impl Default for HandlerArena {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum Effect {
    Custom {
        name: StrId,
        args: Vec<crate::value::JSValue>,
    },
}
