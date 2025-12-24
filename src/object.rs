//! JavaScript object representation

use crate::fixed_collections::{FixedMap, FixedVec};
use crate::fixed_string::StrId;
use crate::value::{JSValue, ObjId};

pub const MAX_PROPERTIES: usize = 32;
pub const MAX_ARRAY_ELEMENTS: usize = 32;
pub const MAX_CAPTURES: usize = 16;

pub struct Object {
    pub properties: FixedMap<StrId, Property, MAX_PROPERTIES>,
    pub prototype: Option<ObjId>,
    pub kind: ObjectKind,
}

#[derive(Clone, Copy, Debug)]
pub struct Property {
    pub value: JSValue,
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

impl Property {
    pub fn value(val: JSValue) -> Self {
        Self { value: val, writable: true, enumerable: true, configurable: true }
    }
    pub fn readonly(val: JSValue) -> Self {
        Self { value: val, writable: false, enumerable: true, configurable: false }
    }
}

#[derive(Clone)]
pub enum ObjectKind {
    Ordinary,
    Array(ArrayData),
    Function(FunctionData),
    BoundFunction(BoundFunctionData),
}

impl Default for ObjectKind { fn default() -> Self { ObjectKind::Ordinary } }

#[derive(Clone, Default)]
pub struct ArrayData {
    pub elements: FixedVec<JSValue, MAX_ARRAY_ELEMENTS>,
}

#[derive(Clone)]
pub struct FunctionData {
    pub code_offset: u32,
    pub param_count: u8,
    pub local_count: u8,
    pub captures: FixedVec<JSValue, MAX_CAPTURES>,
    pub name: Option<StrId>,
}

impl Default for FunctionData {
    fn default() -> Self {
        Self { code_offset: 0, param_count: 0, local_count: 0, captures: FixedVec::new(), name: None }
    }
}

#[derive(Clone)]
pub struct BoundFunctionData {
    pub target: ObjId,
    pub this_arg: JSValue,
    pub bound_args: FixedVec<JSValue, 8>,
}

impl Object {
    pub fn new() -> Self {
        Self { properties: FixedMap::new(), prototype: None, kind: ObjectKind::Ordinary }
    }

    pub fn array() -> Self {
        Self { properties: FixedMap::new(), prototype: None, kind: ObjectKind::Array(ArrayData::default()) }
    }

    pub fn function(code_offset: u32, param_count: u8, name: Option<StrId>) -> Self {
        Self {
            properties: FixedMap::new(),
            prototype: None,
            kind: ObjectKind::Function(FunctionData {
                code_offset, param_count, local_count: 0, captures: FixedVec::new(), name,
            }),
        }
    }

    pub fn get(&self, key: StrId) -> Option<JSValue> { self.properties.get(&key).map(|p| p.value) }
    pub fn set(&mut self, key: StrId, value: JSValue) -> bool { self.properties.insert(key, Property::value(value)) }
    pub fn is_array(&self) -> bool { matches!(self.kind, ObjectKind::Array(_)) }
    pub fn is_function(&self) -> bool { matches!(self.kind, ObjectKind::Function(_)) }

    pub fn as_array(&self) -> Option<&ArrayData> {
        match &self.kind { ObjectKind::Array(data) => Some(data), _ => None }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut ArrayData> {
        match &mut self.kind { ObjectKind::Array(data) => Some(data), _ => None }
    }

    pub fn as_function(&self) -> Option<&FunctionData> {
        match &self.kind { ObjectKind::Function(data) => Some(data), _ => None }
    }

    pub fn as_function_mut(&mut self) -> Option<&mut FunctionData> {
        match &mut self.kind { ObjectKind::Function(data) => Some(data), _ => None }
    }
}

impl Default for Object { fn default() -> Self { Self::new() } }
