//! JavaScript object representation

use std::collections::HashMap;

use crate::ast::{ExprId, StmtId};
use crate::env::EnvId;
use crate::string_pool::StrId;
use crate::value::{JSValue, ObjId};

pub struct Object {
    pub properties: HashMap<StrId, Property>,
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
        Self {
            value: val,
            writable: true,
            enumerable: true,
            configurable: true,
        }
    }

    pub fn readonly(val: JSValue) -> Self {
        Self {
            value: val,
            writable: false,
            enumerable: true,
            configurable: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeFn {
    MathFloor,
    MathCeil,
    MathRound,
    MathAbs,
    MathSqrt,
    MathPow,
    MathMin,
    MathMax,
    MathSin,
    MathCos,
    MathLog,
    MathExp,
    MathTrunc,
    MathSign,
}

#[derive(Clone)]
pub enum ObjectKind {
    Ordinary,
    Array(ArrayData),
    Function(FunctionData),
    BoundFunction(BoundFunctionData),
    NativeFunction(NativeFn),
}

impl Default for ObjectKind {
    fn default() -> Self {
        ObjectKind::Ordinary
    }
}

#[derive(Clone, Default)]
pub struct ArrayData {
    pub elements: Vec<JSValue>,
}

/// Function data for CEKH machine (AST-based closures)
#[derive(Clone, Copy)]
pub struct FunctionData {
    /// Start index into AstArena.param_lists
    pub params_start: u32,
    /// Number of parameters
    pub params_count: u16,
    /// Function body (statement)
    pub body: StmtId,
    /// For arrow functions with expression body (implicit return)
    pub expr_body: Option<ExprId>,
    /// Captured environment (closure!)
    pub env: EnvId,
    /// Function name (for named functions)
    pub name: Option<StrId>,
}

impl FunctionData {
    /// Create a new function (regular function declaration/expression)
    pub fn new(
        params_start: u32,
        params_count: u16,
        body: StmtId,
        env: EnvId,
        name: Option<StrId>,
    ) -> Self {
        Self {
            params_start,
            params_count,
            body,
            expr_body: None,
            env,
            name,
        }
    }

    /// Create an arrow function with expression body
    pub fn arrow_expr(params_start: u32, params_count: u16, expr_body: ExprId, env: EnvId) -> Self {
        Self {
            params_start,
            params_count,
            body: StmtId::NONE,
            expr_body: Some(expr_body),
            env,
            name: None,
        }
    }

    /// Create an arrow function with block body
    pub fn arrow_block(params_start: u32, params_count: u16, body: StmtId, env: EnvId) -> Self {
        Self {
            params_start,
            params_count,
            body,
            expr_body: None,
            env,
            name: None,
        }
    }
}

#[derive(Clone)]
pub struct BoundFunctionData {
    pub target: ObjId,
    pub this_arg: JSValue,
    pub bound_args: Vec<JSValue>,
}

impl Object {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
            prototype: None,
            kind: ObjectKind::Ordinary,
        }
    }

    pub fn array() -> Self {
        Self {
            properties: HashMap::new(),
            prototype: None,
            kind: ObjectKind::Array(ArrayData::default()),
        }
    }

    pub fn closure(func_data: FunctionData) -> Self {
        Self {
            properties: HashMap::new(),
            prototype: None,
            kind: ObjectKind::Function(func_data),
        }
    }

    pub fn native_function(native_fn: NativeFn) -> Self {
        Self {
            properties: HashMap::new(),
            prototype: None,
            kind: ObjectKind::NativeFunction(native_fn),
        }
    }

    pub fn get(&self, key: StrId) -> Option<JSValue> {
        self.properties.get(&key).map(|p| p.value)
    }

    pub fn set(&mut self, key: StrId, value: JSValue) {
        self.properties.insert(key, Property::value(value));
    }

    pub fn is_array(&self) -> bool {
        matches!(self.kind, ObjectKind::Array(_))
    }

    pub fn is_function(&self) -> bool {
        matches!(self.kind, ObjectKind::Function(_))
    }

    pub fn as_array(&self) -> Option<&ArrayData> {
        match &self.kind {
            ObjectKind::Array(data) => Some(data),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut ArrayData> {
        match &mut self.kind {
            ObjectKind::Array(data) => Some(data),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&FunctionData> {
        match &self.kind {
            ObjectKind::Function(data) => Some(data),
            _ => None,
        }
    }

    pub fn as_function_mut(&mut self) -> Option<&mut FunctionData> {
        match &mut self.kind {
            ObjectKind::Function(data) => Some(data),
            _ => None,
        }
    }
}

impl Default for Object {
    fn default() -> Self {
        Self::new()
    }
}
