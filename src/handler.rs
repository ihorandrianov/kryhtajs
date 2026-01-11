//! Handler infrastructure for algebraic effects

use crate::arena::{Arena, ArenaId};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::string_pool::StrId;
use crate::value::ObjId;

pub type HandlerId = ArenaId<Handler>;

#[derive(Clone)]
pub struct Handler {
    pub effect_name: StrId,
    pub handler_fn: ObjId,
    pub return_fn: Option<ObjId>,
    pub env: EnvId,
    pub delimiter: ContId,
}

impl Handler {
    pub fn new(
        effect_name: StrId,
        handler_fn: ObjId,
        return_fn: Option<ObjId>,
        env: EnvId,
        delimiter: ContId,
    ) -> Self {
        Self {
            effect_name,
            handler_fn,
            return_fn,
            env,
            delimiter,
        }
    }
}

#[derive(Clone)]
pub struct CapturedCont {
    pub cont: ContId,
    pub env: EnvId,
}

impl CapturedCont {
    pub fn new(cont: ContId, env: EnvId) -> Self {
        Self { cont, env }
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

    pub fn alloc(&mut self, handler: Handler) -> HandlerId {
        self.arena.alloc(handler)
    }

    pub fn get(&self, id: HandlerId) -> Option<&Handler> {
        self.arena.get(id)
    }

    pub fn get_mut(&mut self, id: HandlerId) -> Option<&mut Handler> {
        self.arena.get_mut(id)
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
