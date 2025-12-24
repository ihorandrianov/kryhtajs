//! Virtual machine for bytecode execution

use crate::bytecode::{Chunk, OpCode};
use crate::error::{JSError, Result};
use crate::fixed_collections::{FixedMap, FixedStack};
use crate::fixed_string::{FixedStringPool, StrId};
use crate::object::{Object, ObjectKind};
use crate::pool::Pool;
use crate::value::{JSValue, ObjId};
use crate::{MAX_CALL_DEPTH, MAX_GLOBALS, MAX_OBJECTS, MAX_STACK, MAX_STRINGS, MAX_STRING_BYTES};

#[derive(Clone, Copy)]
pub struct CallFrame {
    pub return_addr: usize,
    pub bp: usize,
    pub function: Option<ObjId>,
}

impl Default for CallFrame {
    fn default() -> Self { Self { return_addr: 0, bp: 0, function: None } }
}

pub struct VM {
    pub objects: Pool<Object, MAX_OBJECTS>,
    pub strings: FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
    pub stack: FixedStack<JSValue, MAX_STACK>,
    pub frames: FixedStack<CallFrame, MAX_CALL_DEPTH>,
    pub globals: FixedMap<StrId, JSValue, MAX_GLOBALS>,
    chunk: Chunk,
    ip: usize,
}

impl VM {
    pub const fn new() -> Self {
        Self {
            objects: Pool::new(),
            strings: FixedStringPool::new(),
            stack: FixedStack::new(),
            frames: FixedStack::new(),
            globals: FixedMap::new(),
            chunk: Chunk::new(),
            ip: 0,
        }
    }

    pub fn init(&mut self) { self.strings.init(); }

    pub fn reset(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.ip = 0;
    }

    pub fn run(&mut self, chunk: Chunk) -> Result<JSValue> {
        self.chunk = chunk;
        self.ip = 0;
        self.stack.clear();
        self.frames.clear();
        self.frames.push(CallFrame::default());
        self.execute()
    }

    fn execute(&mut self) -> Result<JSValue> {
        loop {
            let op = self.read_op()?;

            match op {
                OpCode::Halt => {
                    return Ok(self.stack.pop().unwrap_or(JSValue::Undefined));
                }
                OpCode::PushUndefined => self.push(JSValue::Undefined)?,
                OpCode::PushNull => self.push(JSValue::Null)?,
                OpCode::PushTrue => self.push(JSValue::Bool(true))?,
                OpCode::PushFalse => self.push(JSValue::Bool(false))?,

                OpCode::PushI8 => {
                    let n = self.read_i8()?;
                    self.push(JSValue::Int(n as i32))?;
                }
                OpCode::PushI16 => {
                    let n = self.read_i16()?;
                    self.push(JSValue::Int(n as i32))?;
                }
                OpCode::PushI32 => {
                    let n = self.read_i32()?;
                    self.push(JSValue::Int(n))?;
                }
                OpCode::PushFloat => {
                    let idx = self.read_u16()?;
                    let f = self.chunk.get_float(idx)
                        .ok_or(JSError::InternalError("Invalid float constant"))?;
                    self.push(JSValue::Float(f))?;
                }
                OpCode::PushString => {
                    let idx = self.read_u16()?;
                    let str_id = self.chunk.get_string(idx)
                        .ok_or(JSError::InternalError("Invalid string constant"))?;
                    self.push(JSValue::String(StrId(str_id)))?;
                }

                OpCode::Pop => {
                    self.pop()?;
                }
                OpCode::Dup => {
                    let val = *self.peek(0)?;
                    self.push(val)?;
                }
                OpCode::Swap => {
                    let len = self.stack.len();
                    if len < 2 {
                        return Err(JSError::InternalError("Stack underflow"));
                    }
                    self.stack.swap(len - 1, len - 2);
                }
                OpCode::GetLocal => {
                    let idx = self.read_u8()?;
                    let bp = self.frames.peek().map(|f| f.bp).unwrap_or(0);
                    let val = self.stack.get(bp + idx as usize)
                        .copied()
                        .unwrap_or(JSValue::Undefined);
                    self.push(val)?;
                }
                OpCode::SetLocal => {
                    let idx = self.read_u8()?;
                    let val = *self.peek(0)?;
                    let bp = self.frames.peek().map(|f| f.bp).unwrap_or(0);
                    if let Some(slot) = self.stack.get_mut(bp + idx as usize) {
                        *slot = val;
                    }
                }
                OpCode::GetGlobal => {
                    let idx = self.read_u16()?;
                    let str_id = self.chunk.get_string(idx)
                        .map(StrId)
                        .ok_or(JSError::InternalError("Invalid string constant"))?;
                    let val = self.globals.get(&str_id).unwrap_or(JSValue::Undefined);
                    self.push(val)?;
                }
                OpCode::SetGlobal => {
                    let idx = self.read_u16()?;
                    let str_id = self.chunk.get_string(idx)
                        .map(StrId)
                        .ok_or(JSError::InternalError("Invalid string constant"))?;
                    let val = *self.peek(0)?;
                    self.globals.insert(str_id, val);
                }
                OpCode::GetCapture | OpCode::SetCapture => {
                    let _ = self.read_u8()?; // TODO
                }
                OpCode::Add => self.binary_op(|a, b| a + b)?,
                OpCode::Sub => self.binary_op(|a, b| a - b)?,
                OpCode::Mul => self.binary_op(|a, b| a * b)?,
                OpCode::Div => self.binary_op(|a, b| a / b)?,
                OpCode::Mod => self.binary_op(|a, b| libm::fmod(a, b))?,
                OpCode::Pow => self.binary_op(libm::pow)?,

                OpCode::Neg => {
                    let val = self.pop()?;
                    let n = self.to_number(val);
                    self.push(JSValue::Float(-n))?;
                }
                OpCode::Plus => {
                    let val = self.pop()?;
                    let n = self.to_number(val);
                    self.push(JSValue::Float(n))?;
                }
                OpCode::Inc => {
                    let val = self.pop()?;
                    let n = self.to_number(val);
                    self.push(JSValue::Float(n + 1.0))?;
                }
                OpCode::Dec => {
                    let val = self.pop()?;
                    let n = self.to_number(val);
                    self.push(JSValue::Float(n - 1.0))?;
                }
                OpCode::BitAnd => self.bitwise_op(|a, b| a & b)?,
                OpCode::BitOr => self.bitwise_op(|a, b| a | b)?,
                OpCode::BitXor => self.bitwise_op(|a, b| a ^ b)?,
                OpCode::BitNot => {
                    let val = self.pop()?;
                    let n = self.to_i32(val);
                    self.push(JSValue::Int(!n))?;
                }
                OpCode::Shl => self.bitwise_op(|a, b| a << (b & 31))?,
                OpCode::Shr => self.bitwise_op(|a, b| a >> (b & 31))?,
                OpCode::UShr => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let au = self.to_i32(a) as u32;
                    let bu = self.to_i32(b) as u32 & 31;
                    self.push(JSValue::Int((au >> bu) as i32))?;
                }
                OpCode::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.push(JSValue::Bool(self.strict_equals(a, b)))?;
                }
                OpCode::Ne => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.push(JSValue::Bool(!self.strict_equals(a, b)))?;
                }
                OpCode::Lt => self.comparison_op(|a, b| a < b)?,
                OpCode::Le => self.comparison_op(|a, b| a <= b)?,
                OpCode::Gt => self.comparison_op(|a, b| a > b)?,
                OpCode::Ge => self.comparison_op(|a, b| a >= b)?,
                OpCode::Not => {
                    let val = self.pop()?;
                    self.push(JSValue::Bool(!val.is_truthy()))?;
                }
                OpCode::Jump => {
                    let offset = self.read_i16()?;
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
                OpCode::JumpTrue => {
                    let offset = self.read_i16()?;
                    let val = self.pop()?;
                    if val.is_truthy() {
                        self.ip = (self.ip as isize + offset as isize) as usize;
                    }
                }
                OpCode::JumpFalse => {
                    let offset = self.read_i16()?;
                    let val = self.pop()?;
                    if !val.is_truthy() {
                        self.ip = (self.ip as isize + offset as isize) as usize;
                    }
                }
                OpCode::JumpTrueKeep => {
                    let offset = self.read_i16()?;
                    let val = *self.peek(0)?;
                    if val.is_truthy() {
                        self.ip = (self.ip as isize + offset as isize) as usize;
                    }
                }
                OpCode::JumpFalseKeep => {
                    let offset = self.read_i16()?;
                    let val = *self.peek(0)?;
                    if !val.is_truthy() {
                        self.ip = (self.ip as isize + offset as isize) as usize;
                    }
                }
                OpCode::Call => {
                    let arg_count = self.read_u8()? as usize;
                    // TODO: Implement proper function calls
                    for _ in 0..arg_count {
                        self.pop()?;
                    }
                    self.pop()?; // callee
                    self.push(JSValue::Undefined)?;
                }
                OpCode::Return => {
                    let result = self.pop()?;
                    if let Some(frame) = self.frames.pop() {
                        self.stack.truncate(frame.bp);
                        self.ip = frame.return_addr;
                        if self.frames.is_empty() {
                            return Ok(result);
                        }
                        self.push(result)?;
                    } else {
                        return Ok(result);
                    }
                }
                OpCode::Closure => {
                    self.push(JSValue::Undefined)?; // TODO
                }
                OpCode::NewObject => {
                    let obj = Object::new();
                    let id = self.objects.alloc(obj)
                        .ok_or(JSError::OutOfMemory)?;
                    self.push(JSValue::Object(ObjId(id)))?;
                }
                OpCode::NewArray => {
                    let obj = Object::array();
                    let id = self.objects.alloc(obj)
                        .ok_or(JSError::OutOfMemory)?;
                    self.push(JSValue::Array(ObjId(id)))?;
                }
                OpCode::ArrayPush => {
                    let elem = self.pop()?;
                    let arr = *self.peek(0)?;
                    if let JSValue::Array(obj_id) = arr {
                        if let Some(obj) = self.objects.get_mut(obj_id.0) {
                            if let Some(data) = obj.as_array_mut() {
                                data.elements.push(elem);
                            }
                        }
                    }
                }

                OpCode::GetProp => {
                    let key = self.pop()?;
                    let obj = self.pop()?;
                    let val = self.get_property(obj, key)?;
                    self.push(val)?;
                }
                OpCode::SetProp => {
                    let val = self.pop()?;
                    let key = self.pop()?;
                    let obj = *self.peek(0)?;
                    self.set_property(obj, key, val)?;
                }
                OpCode::GetElem => {
                    let idx = self.pop()?;
                    let obj = self.pop()?;
                    self.push(self.get_element(obj, idx)?)?;
                }
                OpCode::SetElem => {
                    let val = self.pop()?;
                    let idx = self.pop()?;
                    self.set_element(*self.peek(0)?, idx, val)?;
                }
                OpCode::TypeOf => {
                    let val = self.pop()?;
                    let type_str = val.type_of();
                    let str_id = self.strings.intern(type_str)
                        .ok_or(JSError::OutOfMemory)?;
                    self.push(JSValue::String(str_id))?;
                }
                OpCode::InstanceOf => {
                    self.pop()?;
                    self.pop()?;
                    self.push(JSValue::Bool(false))?;
                }
                OpCode::In => {
                    self.pop()?;
                    self.pop()?;
                    self.push(JSValue::Bool(false))?;
                }
                OpCode::Throw => {
                    let _val = self.pop()?;
                    return Err(JSError::Thrown(crate::error::ThrownValue {
                        message: "exception thrown",
                    }));
                }

                OpCode::Nop => {}
            }
        }
    }

    fn read_op(&mut self) -> Result<OpCode> {
        let byte = self.read_u8()?;
        OpCode::from_u8(byte).ok_or(JSError::InternalError("Invalid opcode"))
    }

    fn read_u8(&mut self) -> Result<u8> {
        let byte = self.chunk.get(self.ip)
            .ok_or(JSError::InternalError("Read past end"))?;
        self.ip += 1;
        Ok(byte)
    }

    fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    fn read_u16(&mut self) -> Result<u16> {
        let hi = self.read_u8()? as u16;
        let lo = self.read_u8()? as u16;
        Ok((hi << 8) | lo)
    }

    fn read_i16(&mut self) -> Result<i16> {
        Ok(self.read_u16()? as i16)
    }

    fn read_i32(&mut self) -> Result<i32> {
        let b0 = self.read_u8()? as i32;
        let b1 = self.read_u8()? as i32;
        let b2 = self.read_u8()? as i32;
        let b3 = self.read_u8()? as i32;
        Ok((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
    }

    #[inline(always)]
    fn push(&mut self, val: JSValue) -> Result<()> {
        if !self.stack.push(val) { return Err(JSError::StackOverflow); }
        Ok(())
    }

    #[inline(always)]
    fn pop(&mut self) -> Result<JSValue> {
        self.stack.pop().ok_or(JSError::InternalError("Stack underflow"))
    }

    #[inline(always)]
    fn peek(&self, distance: usize) -> Result<&JSValue> {
        self.stack.peek_at(distance).ok_or(JSError::InternalError("Stack underflow"))
    }

    fn to_number(&self, val: JSValue) -> f64 {
        match val {
            JSValue::Undefined => f64::NAN,
            JSValue::Null => 0.0,
            JSValue::Bool(b) => if b { 1.0 } else { 0.0 },
            JSValue::Int(n) => n as f64,
            JSValue::Float(f) => f,
            JSValue::String(_) => f64::NAN,
            _ => f64::NAN,
        }
    }

    fn to_i32(&self, val: JSValue) -> i32 {
        let n = self.to_number(val);
        if n.is_nan() || n.is_infinite() { 0 } else { n as i32 }
    }

    fn binary_op<F: Fn(f64, f64) -> f64>(&mut self, op: F) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = op(self.to_number(a), self.to_number(b));
        self.push(JSValue::Float(result))
    }

    fn bitwise_op<F: Fn(i32, i32) -> i32>(&mut self, op: F) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = op(self.to_i32(a), self.to_i32(b));
        self.push(JSValue::Int(result))
    }

    fn comparison_op<F: Fn(f64, f64) -> bool>(&mut self, op: F) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = op(self.to_number(a), self.to_number(b));
        self.push(JSValue::Bool(result))
    }

    fn strict_equals(&self, a: JSValue, b: JSValue) -> bool {
        match (a, b) {
            (JSValue::Undefined, JSValue::Undefined) => true,
            (JSValue::Null, JSValue::Null) => true,
            (JSValue::Bool(a), JSValue::Bool(b)) => a == b,
            (JSValue::Int(a), JSValue::Int(b)) => a == b,
            (JSValue::Float(a), JSValue::Float(b)) => a == b,
            (JSValue::Int(a), JSValue::Float(b)) => (a as f64) == b,
            (JSValue::Float(a), JSValue::Int(b)) => a == (b as f64),
            (JSValue::String(a), JSValue::String(b)) => a == b,
            (JSValue::Object(a), JSValue::Object(b)) => a == b,
            (JSValue::Array(a), JSValue::Array(b)) => a == b,
            (JSValue::Function(a), JSValue::Function(b)) => a == b,
            _ => false,
        }
    }

    fn get_property(&self, obj: JSValue, key: JSValue) -> Result<JSValue> {
        let obj_id = match obj {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => id,
            _ => return Ok(JSValue::Undefined),
        };

        let str_id = match key {
            JSValue::String(id) => id,
            _ => return Ok(JSValue::Undefined),
        };

        if let Some(obj) = self.objects.get(obj_id.0) {
            Ok(obj.get(str_id).unwrap_or(JSValue::Undefined))
        } else {
            Ok(JSValue::Undefined)
        }
    }

    fn set_property(&mut self, obj: JSValue, key: JSValue, val: JSValue) -> Result<()> {
        let obj_id = match obj {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => id,
            _ => return Err(JSError::type_error("Cannot set property of non-object")),
        };

        let str_id = match key {
            JSValue::String(id) => id,
            _ => return Err(JSError::type_error("Property key must be string")),
        };

        if let Some(obj) = self.objects.get_mut(obj_id.0) {
            obj.set(str_id, val);
        }
        Ok(())
    }

    fn get_element(&self, obj: JSValue, idx: JSValue) -> Result<JSValue> {
        if let JSValue::Array(obj_id) = obj {
            if let Some(obj) = self.objects.get(obj_id.0) {
                if let Some(data) = obj.as_array() {
                    let i = self.to_i32(idx) as usize;
                    return Ok(data.elements.get(i).unwrap_or(JSValue::Undefined));
                }
            }
        }
        self.get_property(obj, idx)
    }

    fn set_element(&mut self, obj: JSValue, idx: JSValue, val: JSValue) -> Result<()> {
        if let JSValue::Array(obj_id) = obj {
            let i = self.to_i32(idx) as usize;
            if let Some(obj) = self.objects.get_mut(obj_id.0) {
                if let Some(data) = obj.as_array_mut() {
                    if i < data.elements.len() {
                        data.elements.set(i, val);
                        return Ok(());
                    } else if i == data.elements.len() {
                        data.elements.push(val);
                        return Ok(());
                    }
                }
            }
        }
        self.set_property(obj, idx, val)
    }

    pub fn gc(&mut self) {
        self.objects.clear_marks();
        self.strings.clear_marks();
        for i in 0..self.stack.len() {
            if let Some(val) = self.stack.get(i) { self.mark_value(*val); }
        }
        let mut global_values: [Option<JSValue>; 64] = [None; 64];
        let mut global_count = 0;
        for (_, val) in self.globals.iter() {
            if global_count < 64 { global_values[global_count] = Some(val); global_count += 1; }
        }
        for i in 0..global_count {
            if let Some(val) = global_values[i] { self.mark_value(val); }
        }
    }

    fn mark_value(&mut self, val: JSValue) {
        match val {
            JSValue::String(str_id) => {
                self.strings.mark(str_id);
            }
            JSValue::Object(obj_id) | JSValue::Array(obj_id) | JSValue::Function(obj_id) => {
                self.mark_object(obj_id);
            }
            _ => {}
        }
    }

    fn mark_object(&mut self, obj_id: ObjId) {
        if self.objects.is_marked(obj_id.0) {
            return;
        }
        self.objects.mark(obj_id.0);

        // Collect references to mark (to avoid borrow issues)
        let mut to_mark_strings: [Option<StrId>; 64] = [None; 64];
        let mut to_mark_objects: [Option<ObjId>; 64] = [None; 64];
        let mut to_mark_values: [Option<JSValue>; 64] = [None; 64];
        let mut str_count = 0;
        let mut obj_count = 0;
        let mut val_count = 0;

        if let Some(obj) = self.objects.get(obj_id.0) {
            // Collect property keys and values
            for (key, val) in obj.properties.iter() {
                if str_count < 64 {
                    to_mark_strings[str_count] = Some(key);
                    str_count += 1;
                }
                if val_count < 64 {
                    to_mark_values[val_count] = Some(val.value);
                    val_count += 1;
                }
            }

            // Collect prototype
            if let Some(proto) = obj.prototype {
                if obj_count < 64 {
                    to_mark_objects[obj_count] = Some(proto);
                    obj_count += 1;
                }
            }

            // Collect array elements or captures
            match &obj.kind {
                ObjectKind::Array(data) => {
                    for elem in data.elements.iter() {
                        if val_count < 64 {
                            to_mark_values[val_count] = Some(elem);
                            val_count += 1;
                        }
                    }
                }
                ObjectKind::Function(func) => {
                    if let Some(name) = func.name {
                        if str_count < 64 {
                            to_mark_strings[str_count] = Some(name);
                            str_count += 1;
                        }
                    }
                    for cap in func.captures.iter() {
                        if val_count < 64 {
                            to_mark_values[val_count] = Some(cap);
                            val_count += 1;
                        }
                    }
                }
                ObjectKind::BoundFunction(bound) => {
                    if obj_count < 64 {
                        to_mark_objects[obj_count] = Some(bound.target);
                        obj_count += 1;
                    }
                    if val_count < 64 {
                        to_mark_values[val_count] = Some(bound.this_arg);
                        val_count += 1;
                    }
                    for arg in bound.bound_args.iter() {
                        if val_count < 64 {
                            to_mark_values[val_count] = Some(arg);
                            val_count += 1;
                        }
                    }
                }
                ObjectKind::Ordinary => {}
            }
        }

        // Now mark everything we collected
        for i in 0..str_count {
            if let Some(s) = to_mark_strings[i] {
                self.strings.mark(s);
            }
        }
        for i in 0..obj_count {
            if let Some(o) = to_mark_objects[i] {
                self.mark_object(o);
            }
        }
        for i in 0..val_count {
            if let Some(v) = to_mark_values[i] {
                self.mark_value(v);
            }
        }
    }
}

impl Default for VM {
    fn default() -> Self {
        let mut vm = Self::new();
        vm.init();
        vm
    }
}
