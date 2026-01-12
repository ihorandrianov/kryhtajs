//! JavaScript value representation

use crate::arena::ArenaId;
use crate::object::Object;
use crate::string_pool::StrId;
use crate::{ContId, EnvId, HandlerId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ObjId(pub u32);

impl ObjId {
    pub const NULL: ObjId = ObjId(u32::MAX);

    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// Convert to ArenaId for object arena access
    pub fn into_arena_id(self) -> ArenaId<Object> {
        ArenaId::new(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JSValue {
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Float(f64),
    String(StrId),
    Object(ObjId),
    Function(ObjId),
    Array(ObjId),
    Handler(HandlerId),
    Continuation(ContId, EnvId),
}

impl JSValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            JSValue::Undefined | JSValue::Null => false,
            JSValue::Bool(b) => *b,
            JSValue::Int(n) => *n != 0,
            JSValue::Float(f) => *f != 0.0 && !f.is_nan(),
            JSValue::String(s) => s.0 != 0,
            JSValue::Object(_) | JSValue::Function(_) | JSValue::Array(_) => true,
            JSValue::Continuation(_, _) => true,
            JSValue::Handler(_) => true,
        }
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, JSValue::Undefined | JSValue::Null)
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            JSValue::Undefined => "undefined",
            JSValue::Null => "object",
            JSValue::Bool(_) => "boolean",
            JSValue::Int(_) | JSValue::Float(_) => "number",
            JSValue::String(_) => "string",
            JSValue::Object(_) | JSValue::Array(_) => "object",
            JSValue::Function(_) | JSValue::Continuation(_, _) => "function",
            JSValue::Handler(_) => "handler",
        }
    }

    pub fn as_object(&self) -> Option<ObjId> {
        match self {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            JSValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            JSValue::Float(f) => Some(*f),
            JSValue::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JSValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl Default for JSValue {
    fn default() -> Self {
        JSValue::Undefined
    }
}

impl From<bool> for JSValue {
    fn from(b: bool) -> Self {
        JSValue::Bool(b)
    }
}

impl From<i32> for JSValue {
    fn from(n: i32) -> Self {
        JSValue::Int(n)
    }
}

impl From<f64> for JSValue {
    fn from(f: f64) -> Self {
        if f.fract() == 0.0 && f >= i32::MIN as f64 && f <= i32::MAX as f64 {
            JSValue::Int(f as i32)
        } else {
            JSValue::Float(f)
        }
    }
}
