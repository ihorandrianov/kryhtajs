//! Bytecode definitions

use crate::fixed_collections::FixedVec;
use crate::{MAX_BYTECODE, MAX_CONSTANTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    PushUndefined = 0x00, PushNull = 0x01, PushTrue = 0x02, PushFalse = 0x03,
    PushI8 = 0x04, PushI16 = 0x05, PushI32 = 0x06, PushFloat = 0x07, PushString = 0x08,
    Pop = 0x09, Dup = 0x0A, Swap = 0x0B,
    GetLocal = 0x10, SetLocal = 0x11, GetGlobal = 0x12, SetGlobal = 0x13,
    GetCapture = 0x14, SetCapture = 0x15,
    GetProp = 0x20, SetProp = 0x21, GetElem = 0x22, SetElem = 0x23,
    Add = 0x30, Sub = 0x31, Mul = 0x32, Div = 0x33, Mod = 0x34, Pow = 0x35,
    Neg = 0x36, Plus = 0x37, Inc = 0x38, Dec = 0x39,
    BitAnd = 0x40, BitOr = 0x41, BitXor = 0x42, BitNot = 0x43,
    Shl = 0x44, Shr = 0x45, UShr = 0x46,
    Eq = 0x50, Ne = 0x51, Lt = 0x52, Le = 0x53, Gt = 0x54, Ge = 0x55,
    Not = 0x60,
    Jump = 0x70, JumpTrue = 0x71, JumpFalse = 0x72, JumpTrueKeep = 0x73, JumpFalseKeep = 0x74,
    Call = 0x80, Return = 0x81, Closure = 0x82,
    NewObject = 0x90, NewArray = 0x91, ArrayPush = 0x92,
    TypeOf = 0xA0, InstanceOf = 0xA1, In = 0xA2, Throw = 0xA3,
    Nop = 0xFE, Halt = 0xFF,
}

impl OpCode {
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(OpCode::PushUndefined),
            0x01 => Some(OpCode::PushNull),
            0x02 => Some(OpCode::PushTrue),
            0x03 => Some(OpCode::PushFalse),
            0x04 => Some(OpCode::PushI8),
            0x05 => Some(OpCode::PushI16),
            0x06 => Some(OpCode::PushI32),
            0x07 => Some(OpCode::PushFloat),
            0x08 => Some(OpCode::PushString),
            0x09 => Some(OpCode::Pop),
            0x0A => Some(OpCode::Dup),
            0x0B => Some(OpCode::Swap),
            0x10 => Some(OpCode::GetLocal),
            0x11 => Some(OpCode::SetLocal),
            0x12 => Some(OpCode::GetGlobal),
            0x13 => Some(OpCode::SetGlobal),
            0x14 => Some(OpCode::GetCapture),
            0x15 => Some(OpCode::SetCapture),
            0x20 => Some(OpCode::GetProp),
            0x21 => Some(OpCode::SetProp),
            0x22 => Some(OpCode::GetElem),
            0x23 => Some(OpCode::SetElem),
            0x30 => Some(OpCode::Add),
            0x31 => Some(OpCode::Sub),
            0x32 => Some(OpCode::Mul),
            0x33 => Some(OpCode::Div),
            0x34 => Some(OpCode::Mod),
            0x35 => Some(OpCode::Pow),
            0x36 => Some(OpCode::Neg),
            0x37 => Some(OpCode::Plus),
            0x38 => Some(OpCode::Inc),
            0x39 => Some(OpCode::Dec),
            0x40 => Some(OpCode::BitAnd),
            0x41 => Some(OpCode::BitOr),
            0x42 => Some(OpCode::BitXor),
            0x43 => Some(OpCode::BitNot),
            0x44 => Some(OpCode::Shl),
            0x45 => Some(OpCode::Shr),
            0x46 => Some(OpCode::UShr),
            0x50 => Some(OpCode::Eq),
            0x51 => Some(OpCode::Ne),
            0x52 => Some(OpCode::Lt),
            0x53 => Some(OpCode::Le),
            0x54 => Some(OpCode::Gt),
            0x55 => Some(OpCode::Ge),
            0x60 => Some(OpCode::Not),
            0x70 => Some(OpCode::Jump),
            0x71 => Some(OpCode::JumpTrue),
            0x72 => Some(OpCode::JumpFalse),
            0x73 => Some(OpCode::JumpTrueKeep),
            0x74 => Some(OpCode::JumpFalseKeep),
            0x80 => Some(OpCode::Call),
            0x81 => Some(OpCode::Return),
            0x82 => Some(OpCode::Closure),
            0x90 => Some(OpCode::NewObject),
            0x91 => Some(OpCode::NewArray),
            0x92 => Some(OpCode::ArrayPush),
            0xA0 => Some(OpCode::TypeOf),
            0xA1 => Some(OpCode::InstanceOf),
            0xA2 => Some(OpCode::In),
            0xA3 => Some(OpCode::Throw),
            0xFE => Some(OpCode::Nop),
            0xFF => Some(OpCode::Halt),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub code: FixedVec<u8, MAX_BYTECODE>,
    pub floats: FixedVec<f64, MAX_CONSTANTS>,
    pub strings: FixedVec<u16, MAX_CONSTANTS>,
}

impl Chunk {
    pub const fn new() -> Self {
        Self {
            code: FixedVec::new(),
            floats: FixedVec::new(),
            strings: FixedVec::new(),
        }
    }

    pub fn write(&mut self, byte: u8) -> bool {
        self.code.push(byte)
    }

    pub fn write_op(&mut self, op: OpCode) -> bool {
        self.write(op as u8)
    }

    pub fn add_float(&mut self, f: f64) -> Option<u16> {
        let idx = self.floats.len() as u16;
        if self.floats.push(f) {
            Some(idx)
        } else {
            None
        }
    }

    pub fn add_string(&mut self, str_id: u16) -> Option<u16> {
        let idx = self.strings.len() as u16;
        if self.strings.push(str_id) {
            Some(idx)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize { self.code.len() }
    pub fn is_empty(&self) -> bool { self.code.is_empty() }

    #[inline(always)]
    pub fn get(&self, offset: usize) -> Option<u8> { self.code.get(offset) }

    #[inline(always)]
    pub fn get_float(&self, idx: u16) -> Option<f64> { self.floats.get(idx as usize) }

    #[inline(always)]
    pub fn get_string(&self, idx: u16) -> Option<u16> { self.strings.get(idx as usize) }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}
