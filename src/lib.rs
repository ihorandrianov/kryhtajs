//! KryhtaJS: A tiny, safe, no_std JavaScript engine
//!
//! крихта (kryhta) — Ukrainian for "crumb"

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bytecode;
pub mod error;
pub mod fixed_collections;
pub mod fixed_string;
pub mod object;
pub mod pool;
pub mod value;
pub mod ast;
pub mod lexer;
pub mod parser_arena;
pub mod compiler_arena;

#[cfg(feature = "alloc")]
pub mod compiler;
#[cfg(feature = "alloc")]
pub mod parser;

pub mod builtins;
pub mod gc;
pub mod vm;

pub use error::{JSError, Result};
pub use fixed_string::StrId;
pub use value::{JSValue, ObjId};
pub use vm::VM;
pub use ast::{AstArena, ExprId, StmtId};
pub use parser_arena::Parser as ArenaParser;
pub use compiler_arena::Compiler as ArenaCompiler;

pub const MAX_OBJECTS: usize = 512;
pub const MAX_STRING_BYTES: usize = 4096;
pub const MAX_STRINGS: usize = 256;
pub const MAX_STACK: usize = 256;
pub const MAX_CALL_DEPTH: usize = 32;
pub const MAX_GLOBALS: usize = 64;
pub const MAX_BYTECODE: usize = 8192;
pub const MAX_CONSTANTS: usize = 256;
