//! JavaScript value representation

use crate::fixed_string::StrId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ObjId(pub u16);

impl ObjId {
    pub const NULL: ObjId = ObjId(u16::MAX);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FnId(pub u16);

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
}

impl JSValue {
    #[inline]
    pub fn is_truthy(&self) -> bool {
        match self {
            JSValue::Undefined | JSValue::Null => false,
            JSValue::Bool(b) => *b,
            JSValue::Int(n) => *n != 0,
            JSValue::Float(f) => *f != 0.0 && !f.is_nan(),
            JSValue::String(s) => s.0 != 0,
            JSValue::Object(_) | JSValue::Function(_) | JSValue::Array(_) => true,
        }
    }

    #[inline]
    pub fn is_nullish(&self) -> bool { matches!(self, JSValue::Undefined | JSValue::Null) }

    pub fn type_of(&self) -> &'static str {
        match self {
            JSValue::Undefined => "undefined",
            JSValue::Null => "object", // JS quirk: typeof null === "object"
            JSValue::Bool(_) => "boolean",
            JSValue::Int(_) | JSValue::Float(_) => "number",
            JSValue::String(_) => "string",
            JSValue::Object(_) | JSValue::Array(_) => "object",
            JSValue::Function(_) => "function",
        }
    }

    #[inline]
    pub fn as_object(&self) -> Option<ObjId> {
        match self {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => Some(*id),
            _ => None,
        }
    }
}

impl Default for JSValue {
    fn default() -> Self {
        JSValue::Undefined
    }
}
