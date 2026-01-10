//! Handler infrastructure for algebraic effects
//!
//! This module provides the data structures for effect handlers.
//! The actual effect semantics (perform/handle/resume) are left
//! for you to implement.
//!
//! ## Overview
//!
//! Algebraic effects allow computations to "perform" operations
//! without specifying how they're handled. Handlers intercept
//! these operations and can:
//! - Provide a value and resume execution
//! - Transform or discard the continuation
//! - Perform other effects
//!
//! ## Extension Points
//!
//! 1. Add `Control::Perform(effect, args)` variant in cekh.rs
//! 2. Search handler stack for matching effect
//! 3. Capture continuation (K) up to HandlerK
//! 4. Call handler function with args + captured continuation
//!
//! The captured continuation is a first-class value that can be:
//! - Resumed once (one-shot continuation)
//! - Resumed multiple times (multi-shot, requires cloning K)
//! - Discarded (like an exception)

use crate::arena::{Arena, ArenaId};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::string_pool::StrId;
use crate::value::ObjId;

/// Handler ID - index into the handler arena
pub type HandlerId = ArenaId<Handler>;

/// Effect handler
///
/// When an effect is performed, the runtime searches the handler
/// stack for a handler matching the effect name. The handler
/// receives the effect arguments and a captured continuation.
#[derive(Clone)]
pub struct Handler {
    /// Effect name this handler catches
    pub effect_name: StrId,

    /// Handler function - called when effect is performed
    /// Receives: (args..., continuation)
    pub handler_fn: ObjId,

    /// Optional return clause - transforms final value
    /// If None, final value passes through unchanged
    pub return_fn: Option<ObjId>,

    /// Environment where handler was installed
    pub env: EnvId,

    /// Continuation at handler installation point
    /// (for delimiting captured continuations)
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

/// Captured continuation - a resumable computation
///
/// When an effect is performed, the continuation from the
/// perform site up to the handler is captured as this structure.
/// The handler can then resume execution by providing a value.
#[derive(Clone)]
pub struct CapturedCont {
    /// The captured continuation frames
    pub cont: ContId,

    /// Environment at capture point
    pub env: EnvId,
}

impl CapturedCont {
    pub fn new(cont: ContId, env: EnvId) -> Self {
        Self { cont, env }
    }
}

/// Handler arena
pub struct HandlerArena {
    arena: Arena<Handler>,
}

impl HandlerArena {
    pub fn new() -> Self {
        Self {
            arena: Arena::new(),
        }
    }

    /// Allocate a new handler
    pub fn alloc(&mut self, handler: Handler) -> HandlerId {
        self.arena.alloc(handler)
    }

    /// Get a handler
    pub fn get(&self, id: HandlerId) -> Option<&Handler> {
        self.arena.get(id)
    }

    /// Get mutable handler
    pub fn get_mut(&mut self, id: HandlerId) -> Option<&mut Handler> {
        self.arena.get_mut(id)
    }
}

impl Default for HandlerArena {
    fn default() -> Self {
        Self::new()
    }
}

/// Effect operation placeholder
///
/// This enum represents effect operations that can be performed.
/// You'll extend this based on your effect system design.
#[derive(Clone, Debug)]
pub enum Effect {
    /// Custom effect with name and arguments
    Custom {
        name: StrId,
        args: Vec<crate::value::JSValue>,
    },
    // Add built-in effects here, e.g.:
    // Async(AsyncOp),
    // State(StateOp),
    // Exception(JSValue),
}

// ============================================================
// IMPLEMENTATION GUIDE
// ============================================================
//
// To implement algebraic effects:
//
// 1. Add Control::Perform variant:
//    ```
//    pub enum Control {
//        // ... existing variants ...
//        Perform(Effect),
//    }
//    ```
//
// 2. In step function, handle Perform:
//    ```
//    Control::Perform(effect) => {
//        // Search handler stack for matching handler
//        let handler = self.find_handler(&effect)?;
//
//        // Capture continuation up to handler's delimiter
//        let captured = self.capture_cont(handler.delimiter);
//
//        // Call handler function with args + continuation
//        self.call_handler(handler, effect.args(), captured)
//    }
//    ```
//
// 3. Implement resume:
//    ```
//    fn resume(&mut self, captured: CapturedCont, value: JSValue) {
//        // Restore captured continuation
//        self.cont = captured.cont;
//        self.env = captured.env;
//        // Continue with provided value
//        self.control = Control::Value(value);
//    }
//    ```
//
// 4. For multi-shot continuations, clone the ContArena segment
//    (requires deep copy of continuation chain)
//
// Example effect: State
// ```javascript
// handle {
//     let x = perform Get();
//     perform Put(x + 1);
//     perform Get()
// } with {
//     Get: (k) => (s) => k(s)(s),
//     Put: (v, k) => (s) => k(undefined)(v),
//     return: (x) => (s) => x
// }
// ```
