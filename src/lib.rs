//! KryhtaJS: A tiny JavaScript engine with algebraic effects
//!
//! крихта (kryhta) — Ukrainian for "crumb"
//!
//! This engine uses a CEKH machine (Control, Environment, Kontinuation, Handlers)
//! for evaluation. The H component provides infrastructure for algebraic effects.

#[cfg(feature = "wasm")]
pub mod wasm;

pub mod arena;
pub mod ast;
pub mod builtins;
pub mod cekh;
pub mod cont;
pub mod env;
pub mod error;
pub mod gc;
pub mod handler;
pub mod lexer;
pub mod object;
pub mod parser;
pub mod runtime;
pub mod string_pool;
pub mod value;

pub use arena::{Arena, ArenaId};
pub use ast::{AstArena, Expr, ExprId, Stmt, StmtId};
pub use cekh::{CEKH, Control};
pub use cont::{ContArena, ContId, Kont};
pub use env::{Env, EnvArena, EnvId};
pub use error::{JSError, Result};
pub use handler::{Handler, HandlerArena, HandlerId};
pub use runtime::Runtime;
pub use string_pool::{StrId, StringPool};
pub use value::{JSValue, ObjId};
