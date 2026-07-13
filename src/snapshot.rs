//! Snapshot serialization: whole-runtime durable execution.
//!
//! Format: magic "KRHT", version u8, little-endian, length-prefixed.

use std::collections::{HashMap, VecDeque};

use crate::arena::Arena;
use crate::ast::{
    AstArena, BinaryOp, EffectClause, Expr, ExprId, MatchArm, Pattern, PatternField, PatternId,
    PropEntry, Stmt, StmtId, UnaryOp,
};
use crate::cekh::{CEKH, Control};
use crate::cont::{ContArena, Kont};
use crate::env::{Env, EnvArena, EnvId};
use crate::error::{JSError, Result};
use crate::gc::GC;
use crate::handler::{Handler, HandlerArena};
use crate::host::HostValue;
use crate::object::{
    ArrayData, BoundFunctionData, FunctionData, NativeFn, Object, ObjectKind, Property,
};
use crate::runtime::{BuiltinEffect, EffectKind, Fiber, FiberId, FiberStatus, Runtime};
use crate::string_pool::{StrId, StringPool};
use crate::value::{JSValue, ObjId};
use crate::{ContId, HandlerId};

pub struct ByteWriter {
    buf: Vec<u8>,
}

pub struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool_(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn str_(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

fn truncated() -> JSError {
    JSError::Message("snapshot: truncated input".to_string())
}

impl<'a> ByteReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(truncated)?;
        if end > self.buf.len() {
            return Err(truncated());
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bool_(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn str_(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| JSError::Message("snapshot: invalid UTF-8".to_string()))
    }

    pub fn is_at_end(&self) -> bool {
        self.pos == self.buf.len()
    }

    pub fn pos(&self) -> usize {
        self.pos
    }
}

fn write_value(w: &mut ByteWriter, v: JSValue) {
    match v {
        JSValue::Undefined => w.u8(0),
        JSValue::Null => w.u8(1),
        JSValue::Bool(b) => {
            w.u8(2);
            w.bool_(b);
        }
        JSValue::Int(n) => {
            w.u8(3);
            w.i32(n);
        }
        JSValue::Float(f) => {
            w.u8(4);
            w.f64(f);
        }
        JSValue::String(s) => {
            w.u8(5);
            w.u32(s.0);
        }
        JSValue::Object(o) => {
            w.u8(6);
            w.u32(o.0);
        }
        JSValue::Function(o) => {
            w.u8(7);
            w.u32(o.0);
        }
        JSValue::Array(o) => {
            w.u8(8);
            w.u32(o.0);
        }
        JSValue::Handler(h) => {
            w.u8(9);
            w.u32(h.index() as u32);
        }
        JSValue::Continuation(k, e) => {
            w.u8(10);
            w.u32(k.index() as u32);
            w.u32(e.index() as u32);
        }
    }
}

fn read_value(r: &mut ByteReader) -> Result<JSValue> {
    Ok(match r.u8()? {
        0 => JSValue::Undefined,
        1 => JSValue::Null,
        2 => JSValue::Bool(r.bool_()?),
        3 => JSValue::Int(r.i32()?),
        4 => JSValue::Float(r.f64()?),
        5 => JSValue::String(StrId(r.u32()?)),
        6 => JSValue::Object(ObjId(r.u32()?)),
        7 => JSValue::Function(ObjId(r.u32()?)),
        8 => JSValue::Array(ObjId(r.u32()?)),
        9 => JSValue::Handler(HandlerId::new(r.u32()?)),
        10 => {
            let k = ContId::new(r.u32()?);
            let e = EnvId::new(r.u32()?);
            JSValue::Continuation(k, e)
        }
        tag => return Err(JSError::Message(format!("snapshot: bad value tag {tag}"))),
    })
}

fn write_seq_values(w: &mut ByteWriter, vs: &[JSValue]) {
    w.u32(vs.len() as u32);
    for v in vs {
        write_value(w, *v);
    }
}

fn read_seq_values(r: &mut ByteReader) -> Result<Vec<JSValue>> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        out.push(read_value(r)?);
    }
    Ok(out)
}

// HostValue is self-contained (owns its Strings, no arena ids), so unlike
// `write_value`/`read_value` this codec needs no runtime/interner access.
pub(crate) fn write_host_value(w: &mut ByteWriter, v: &HostValue) {
    match v {
        HostValue::Undefined => w.u8(0),
        HostValue::Null => w.u8(1),
        HostValue::Bool(b) => {
            w.u8(2);
            w.bool_(*b);
        }
        HostValue::Int(n) => {
            w.u8(3);
            w.i32(*n);
        }
        HostValue::Float(f) => {
            w.u8(4);
            w.f64(*f);
        }
        HostValue::Str(s) => {
            w.u8(5);
            w.str_(s);
        }
        HostValue::Array(items) => {
            w.u8(6);
            w.u32(items.len() as u32);
            for item in items {
                write_host_value(w, item);
            }
        }
        HostValue::Object(pairs) => {
            w.u8(7);
            w.u32(pairs.len() as u32);
            for (key, value) in pairs {
                w.str_(key);
                write_host_value(w, value);
            }
        }
    }
}

/// Recursion guard for `read_host_value`: this is the codec's only
/// recursive decoder, so a crafted snapshot of nested Array/Object tags
/// (~5 bytes per level) could otherwise overflow the stack. Real values
/// can't get this deep — cycles are rejected at the host boundary.
const MAX_HOST_VALUE_DEPTH: u32 = 256;

pub(crate) fn read_host_value(r: &mut ByteReader) -> Result<HostValue> {
    read_host_value_at(r, 0)
}

fn read_host_value_at(r: &mut ByteReader, depth: u32) -> Result<HostValue> {
    if depth > MAX_HOST_VALUE_DEPTH {
        return Err(JSError::Message(
            "snapshot: host value nested too deep".to_string(),
        ));
    }
    Ok(match r.u8()? {
        0 => HostValue::Undefined,
        1 => HostValue::Null,
        2 => HostValue::Bool(r.bool_()?),
        3 => HostValue::Int(r.i32()?),
        4 => HostValue::Float(r.f64()?),
        5 => HostValue::Str(r.str_()?),
        6 => {
            let n = r.u32()? as usize;
            let mut items = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                items.push(read_host_value_at(r, depth + 1)?);
            }
            HostValue::Array(items)
        }
        7 => {
            let n = r.u32()? as usize;
            let mut pairs = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                let key = r.str_()?;
                let value = read_host_value_at(r, depth + 1)?;
                pairs.push((key, value));
            }
            HostValue::Object(pairs)
        }
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad host value tag {tag}"
            )));
        }
    })
}

fn write_seq_host_values(w: &mut ByteWriter, vs: &[HostValue]) {
    w.u32(vs.len() as u32);
    for v in vs {
        write_host_value(w, v);
    }
}

fn read_seq_host_values(r: &mut ByteReader) -> Result<Vec<HostValue>> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        out.push(read_host_value(r)?);
    }
    Ok(out)
}

fn write_opt_u32(w: &mut ByteWriter, v: Option<u32>) {
    match v {
        Some(x) => {
            w.bool_(true);
            w.u32(x);
        }
        None => w.bool_(false),
    }
}

fn read_opt_u32(r: &mut ByteReader) -> Result<Option<u32>> {
    Ok(if r.bool_()? { Some(r.u32()?) } else { None })
}

fn write_object(w: &mut ByteWriter, obj: &Object) {
    w.u32(obj.properties.len() as u32);
    for (key, prop) in &obj.properties {
        w.u32(key.0);
        write_value(w, prop.value);
        w.bool_(prop.writable);
        w.bool_(prop.enumerable);
        w.bool_(prop.configurable);
    }
    write_opt_u32(w, obj.prototype.map(|p| p.0));
    match &obj.kind {
        ObjectKind::Ordinary => w.u8(0),
        ObjectKind::Array(a) => {
            w.u8(1);
            write_seq_values(w, &a.elements);
        }
        ObjectKind::Function(f) => {
            w.u8(2);
            w.u32(f.params_start);
            w.u16(f.params_count);
            w.u32(f.body.0);
            write_opt_u32(w, f.expr_body.map(|e| e.0));
            w.u32(f.env.index() as u32);
            write_opt_u32(w, f.name.map(|n| n.0));
        }
        ObjectKind::BoundFunction(b) => {
            w.u8(3);
            w.u32(b.target.0);
            write_value(w, b.this_arg);
            write_seq_values(w, &b.bound_args);
        }
        ObjectKind::NativeFunction(nf) => {
            w.u8(4);
            w.u8(*nf as u8);
        }
    }
}

fn read_native_fn(tag: u8) -> Result<NativeFn> {
    Ok(match tag {
        0 => NativeFn::MathFloor,
        1 => NativeFn::MathCeil,
        2 => NativeFn::MathRound,
        3 => NativeFn::MathAbs,
        4 => NativeFn::MathSqrt,
        5 => NativeFn::MathPow,
        6 => NativeFn::MathMin,
        7 => NativeFn::MathMax,
        8 => NativeFn::MathSin,
        9 => NativeFn::MathCos,
        10 => NativeFn::MathLog,
        11 => NativeFn::MathExp,
        12 => NativeFn::MathTrunc,
        13 => NativeFn::MathSign,
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad native fn tag {tag}"
            )));
        }
    })
}

fn read_object(r: &mut ByteReader) -> Result<Object> {
    let n = r.u32()? as usize;
    let mut properties = HashMap::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let key = StrId(r.u32()?);
        let value = read_value(r)?;
        let writable = r.bool_()?;
        let enumerable = r.bool_()?;
        let configurable = r.bool_()?;
        properties.insert(
            key,
            Property {
                value,
                writable,
                enumerable,
                configurable,
            },
        );
    }
    let prototype = read_opt_u32(r)?.map(ObjId);
    let kind = match r.u8()? {
        0 => ObjectKind::Ordinary,
        1 => ObjectKind::Array(ArrayData {
            elements: read_seq_values(r)?,
        }),
        2 => {
            let params_start = r.u32()?;
            let params_count = r.u16()?;
            let body = StmtId(r.u32()?);
            let expr_body = read_opt_u32(r)?.map(ExprId);
            let env = EnvId::new(r.u32()?);
            let name = read_opt_u32(r)?.map(StrId);
            ObjectKind::Function(FunctionData {
                params_start,
                params_count,
                body,
                expr_body,
                env,
                name,
            })
        }
        3 => {
            let target = ObjId(r.u32()?);
            let this_arg = read_value(r)?;
            let bound_args = read_seq_values(r)?;
            ObjectKind::BoundFunction(BoundFunctionData {
                target,
                this_arg,
                bound_args,
            })
        }
        4 => {
            let tag = r.u8()?;
            ObjectKind::NativeFunction(read_native_fn(tag)?)
        }
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad object kind tag {tag}"
            )));
        }
    };
    Ok(Object {
        properties,
        prototype,
        kind,
    })
}

fn write_env(w: &mut ByteWriter, env: &Env) {
    let bindings: Vec<(StrId, JSValue)> = env.iter_bindings().collect();
    w.u32(bindings.len() as u32);
    for (k, v) in &bindings {
        w.u32(k.0);
        write_value(w, *v);
    }
    write_opt_u32(w, env.parent().map(|p| p.index() as u32));
}

fn read_env(r: &mut ByteReader) -> Result<Env> {
    let n = r.u32()? as usize;
    let mut pairs = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let k = StrId(r.u32()?);
        let v = read_value(r)?;
        pairs.push((k, v));
    }
    let parent = read_opt_u32(r)?.map(EnvId::new);
    Ok(Env::with_binding_slice(&pairs, parent))
}

fn write_handler(w: &mut ByteWriter, h: &Handler) {
    w.u32(h.clauses_start);
    w.u16(h.clauses_count);
    w.u32(h.return_param.0);
    w.u32(h.return_body.0);
    w.u32(h.env.index() as u32);
}

fn read_handler(r: &mut ByteReader) -> Result<Handler> {
    let clauses_start = r.u32()?;
    let clauses_count = r.u16()?;
    let return_param = StrId(r.u32()?);
    let return_body = ExprId(r.u32()?);
    let env = EnvId::new(r.u32()?);
    Ok(Handler::new(
        clauses_start,
        clauses_count,
        return_param,
        return_body,
        env,
    ))
}

fn write_control(w: &mut ByteWriter, c: &Control) {
    match c {
        Control::Expr(e) => {
            w.u8(0);
            w.u32(e.0);
        }
        Control::Stmt(s) => {
            w.u8(1);
            w.u32(s.0);
        }
        Control::Value(v) => {
            w.u8(2);
            write_value(w, *v);
        }
        Control::Returning(v) => {
            w.u8(3);
            write_value(w, *v);
        }
        Control::Halted(v) => {
            w.u8(4);
            write_value(w, *v);
        }
        Control::Suspend { effect, args } => {
            w.u8(5);
            w.u32(effect.0);
            write_seq_values(w, args);
        }
    }
}

fn read_control(r: &mut ByteReader) -> Result<Control> {
    Ok(match r.u8()? {
        0 => Control::Expr(ExprId(r.u32()?)),
        1 => Control::Stmt(StmtId(r.u32()?)),
        2 => Control::Value(read_value(r)?),
        3 => Control::Returning(read_value(r)?),
        4 => Control::Halted(read_value(r)?),
        5 => {
            let effect = StrId(r.u32()?);
            let args = read_seq_values(r)?;
            Control::Suspend { effect, args }
        }
        tag => return Err(JSError::Message(format!("snapshot: bad control tag {tag}"))),
    })
}

fn write_fiber_status(w: &mut ByteWriter, s: &FiberStatus) {
    match s {
        FiberStatus::Ready => w.u8(0),
        FiberStatus::Running => w.u8(1),
        FiberStatus::Blocked { effect, args } => {
            w.u8(2);
            w.u32(effect.0);
            write_seq_values(w, args);
        }
        FiberStatus::Completed(v) => {
            w.u8(3);
            write_value(w, *v);
        }
        FiberStatus::Failed(err) => {
            w.u8(4);
            w.str_(&err.to_string());
        }
        FiberStatus::BlockedOnHost { effect, args } => {
            w.u8(5);
            w.u32(effect.0);
            write_seq_host_values(w, args);
        }
    }
}

fn read_fiber_status(r: &mut ByteReader) -> Result<FiberStatus> {
    Ok(match r.u8()? {
        0 => FiberStatus::Ready,
        1 => FiberStatus::Running,
        2 => {
            let effect = StrId(r.u32()?);
            let args = read_seq_values(r)?;
            FiberStatus::Blocked { effect, args }
        }
        3 => FiberStatus::Completed(read_value(r)?),
        4 => FiberStatus::Failed(JSError::Message(r.str_()?)),
        5 => {
            let effect = StrId(r.u32()?);
            let args = read_seq_host_values(r)?;
            FiberStatus::BlockedOnHost { effect, args }
        }
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad fiber status tag {tag}"
            )));
        }
    })
}

fn write_fiber(w: &mut ByteWriter, f: &Fiber) {
    w.u32(f.id.0);
    write_control(w, &f.control);
    w.u32(f.cont.index() as u32);
    w.u32(f.env.index() as u32);
    write_fiber_status(w, &f.status);
}

fn read_fiber(r: &mut ByteReader) -> Result<Fiber> {
    let id = FiberId(r.u32()?);
    let control = read_control(r)?;
    let cont = ContId::new(r.u32()?);
    let env = EnvId::new(r.u32()?);
    let status = read_fiber_status(r)?;
    Ok(Fiber {
        id,
        control,
        cont,
        env,
        status,
    })
}

fn write_strings(w: &mut ByteWriter, pool: &StringPool) {
    w.u32(pool.len() as u32);
    for i in 0..pool.len() {
        let s = pool
            .get(StrId(i as u32))
            .expect("snapshot: string pool index in range");
        w.str_(s);
    }
}

fn read_strings(r: &mut ByteReader) -> Result<StringPool> {
    let n = r.u32()? as usize;
    let mut pool = StringPool::new();
    for _ in 0..n {
        let s = r.str_()?;
        pool.intern(&s);
    }
    Ok(pool)
}

fn write_binary_op(w: &mut ByteWriter, op: BinaryOp) {
    w.u8(op as u8);
}

fn read_binary_op(r: &mut ByteReader) -> Result<BinaryOp> {
    Ok(match r.u8()? {
        0 => BinaryOp::Add,
        1 => BinaryOp::Sub,
        2 => BinaryOp::Mul,
        3 => BinaryOp::Div,
        4 => BinaryOp::Mod,
        5 => BinaryOp::Pow,
        6 => BinaryOp::Eq,
        7 => BinaryOp::Ne,
        8 => BinaryOp::Lt,
        9 => BinaryOp::Le,
        10 => BinaryOp::Gt,
        11 => BinaryOp::Ge,
        12 => BinaryOp::And,
        13 => BinaryOp::Or,
        14 => BinaryOp::BitAnd,
        15 => BinaryOp::BitOr,
        16 => BinaryOp::BitXor,
        17 => BinaryOp::Shl,
        18 => BinaryOp::Shr,
        19 => BinaryOp::UShr,
        20 => BinaryOp::NullishCoalesce,
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad binary op tag {tag}"
            )));
        }
    })
}

fn write_unary_op(w: &mut ByteWriter, op: UnaryOp) {
    w.u8(op as u8);
}

fn read_unary_op(r: &mut ByteReader) -> Result<UnaryOp> {
    Ok(match r.u8()? {
        0 => UnaryOp::Neg,
        1 => UnaryOp::Not,
        2 => UnaryOp::BitNot,
        3 => UnaryOp::TypeOf,
        4 => UnaryOp::Plus,
        5 => UnaryOp::PreInc,
        6 => UnaryOp::PreDec,
        7 => UnaryOp::PostInc,
        8 => UnaryOp::PostDec,
        tag => {
            return Err(JSError::Message(format!(
                "snapshot: bad unary op tag {tag}"
            )));
        }
    })
}

fn write_kont(w: &mut ByteWriter, k: &Kont) {
    match k {
        Kont::Halt => w.u8(0),
        Kont::UnaryK { op, k } => {
            w.u8(1);
            write_unary_op(w, *op);
            w.u32(k.index() as u32);
        }
        Kont::BinaryLeftK { op, right, env, k } => {
            w.u8(2);
            write_binary_op(w, *op);
            w.u32(right.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::BinaryRightK { op, left, k } => {
            w.u8(3);
            write_binary_op(w, *op);
            write_value(w, *left);
            w.u32(k.index() as u32);
        }
        Kont::AndK { right, env, k } => {
            w.u8(4);
            w.u32(right.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::OrK { right, env, k } => {
            w.u8(5);
            w.u32(right.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::NullishK { right, env, k } => {
            w.u8(6);
            w.u32(right.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::MemberK { property, k } => {
            w.u8(7);
            w.u32(property.0);
            w.u32(k.index() as u32);
        }
        Kont::IndexObjK { index, env, k } => {
            w.u8(8);
            w.u32(index.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::IndexKeyK { obj, k } => {
            w.u8(9);
            write_value(w, *obj);
            w.u32(k.index() as u32);
        }
        Kont::AssignVarK { name, env, k } => {
            w.u8(10);
            w.u32(name.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::UpdateVarK {
            name,
            is_pre,
            is_inc,
            env,
            k,
        } => {
            w.u8(11);
            w.u32(name.0);
            w.bool_(*is_pre);
            w.bool_(*is_inc);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::AssignMemberObjK {
            property,
            value,
            env,
            k,
        } => {
            w.u8(12);
            w.u32(property.0);
            w.u32(value.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::AssignMemberValK { obj, property, k } => {
            w.u8(13);
            write_value(w, *obj);
            w.u32(property.0);
            w.u32(k.index() as u32);
        }
        Kont::AssignIndexObjK {
            index,
            value,
            env,
            k,
        } => {
            w.u8(14);
            w.u32(index.0);
            w.u32(value.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::AssignIndexKeyK { obj, value, env, k } => {
            w.u8(15);
            write_value(w, *obj);
            w.u32(value.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::AssignIndexValK { obj, key, k } => {
            w.u8(16);
            write_value(w, *obj);
            write_value(w, *key);
            w.u32(k.index() as u32);
        }
        Kont::IfK {
            consequent,
            alternate,
            env,
            k,
        } => {
            w.u8(17);
            w.u32(consequent.0);
            w.u32(alternate.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::CondK {
            consequent,
            alternate,
            env,
            k,
        } => {
            w.u8(18);
            w.u32(consequent.0);
            w.u32(alternate.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::WhileK { test, body, env, k } => {
            w.u8(19);
            w.u32(test.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::WhileBodyK { test, body, env, k } => {
            w.u8(20);
            w.u32(test.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ForTestK {
            test,
            update,
            body,
            env,
            k,
        } => {
            w.u8(21);
            w.u32(test.0);
            w.u32(update.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ForTestResultK {
            test,
            update,
            body,
            env,
            k,
        } => {
            w.u8(22);
            w.u32(test.0);
            w.u32(update.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ForBodyK {
            test,
            update,
            body,
            env,
            k,
        } => {
            w.u8(23);
            w.u32(test.0);
            w.u32(update.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ForUpdateK {
            test,
            update,
            body,
            env,
            k,
        } => {
            w.u8(24);
            w.u32(test.0);
            w.u32(update.0);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::CalleeK {
            args_start,
            args_count,
            env,
            k,
        } => {
            w.u8(25);
            w.u32(*args_start);
            w.u16(*args_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ArgsK {
            callee,
            done,
            args_start,
            args_idx,
            args_count,
            env,
            k,
        } => {
            w.u8(26);
            write_value(w, *callee);
            write_seq_values(w, done);
            w.u32(*args_start);
            w.u16(*args_idx);
            w.u16(*args_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ReturnK { env, k } => {
            w.u8(27);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ReturnExprK { k } => {
            w.u8(28);
            w.u32(k.index() as u32);
        }
        Kont::ArrayK {
            done,
            elems_start,
            elems_idx,
            elems_count,
            env,
            k,
        } => {
            w.u8(29);
            write_seq_values(w, done);
            w.u32(*elems_start);
            w.u16(*elems_idx);
            w.u16(*elems_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ObjectK {
            done,
            props_start,
            props_idx,
            props_count,
            env,
            k,
        } => {
            w.u8(30);
            w.u32(done.len() as u32);
            for (key, value) in done {
                w.u32(key.0);
                write_value(w, *value);
            }
            w.u32(*props_start);
            w.u16(*props_idx);
            w.u16(*props_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::LetK { name, env, k } => {
            w.u8(31);
            w.u32(name.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ConstK { name, env, k } => {
            w.u8(32);
            w.u32(name.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::VarK { name, env, k } => {
            w.u8(33);
            w.u32(name.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::SeqK {
            stmts_start,
            stmts_idx,
            stmts_count,
            env,
            k,
        } => {
            w.u8(34);
            w.u32(*stmts_start);
            w.u32(*stmts_idx);
            w.u32(*stmts_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::ExprStmtK { k } => {
            w.u8(35);
            w.u32(k.index() as u32);
        }
        Kont::BlockK {
            stmts_start,
            stmts_idx,
            stmts_count,
            final_expr,
            env,
            k,
        } => {
            w.u8(36);
            w.u32(*stmts_start);
            w.u32(*stmts_idx);
            w.u32(*stmts_count);
            w.u32(final_expr.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::MatchK {
            arms_start,
            arms_idx,
            arms_count,
            env,
            k,
        } => {
            w.u8(37);
            w.u32(*arms_start);
            w.u16(*arms_idx);
            w.u16(*arms_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::HandlerK {
            clauses_start,
            clauses_count,
            env,
            return_body,
            return_param,
            k,
        } => {
            w.u8(38);
            w.u32(*clauses_start);
            w.u16(*clauses_count);
            w.u32(env.index() as u32);
            w.u32(return_body.0);
            w.u32(return_param.0);
            w.u32(k.index() as u32);
        }
        Kont::PerformArgsK {
            effect,
            done,
            args_start,
            args_idx,
            args_count,
            env,
            k,
        } => {
            w.u8(39);
            w.u32(effect.0);
            write_seq_values(w, done);
            w.u32(*args_start);
            w.u16(*args_idx);
            w.u16(*args_count);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
        Kont::HandleWithK { body, env, k } => {
            w.u8(40);
            w.u32(body.0);
            w.u32(env.index() as u32);
            w.u32(k.index() as u32);
        }
    }
}

fn read_kont(r: &mut ByteReader) -> Result<Kont> {
    Ok(match r.u8()? {
        0 => Kont::Halt,
        1 => Kont::UnaryK {
            op: read_unary_op(r)?,
            k: ContId::new(r.u32()?),
        },
        2 => Kont::BinaryLeftK {
            op: read_binary_op(r)?,
            right: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        3 => Kont::BinaryRightK {
            op: read_binary_op(r)?,
            left: read_value(r)?,
            k: ContId::new(r.u32()?),
        },
        4 => Kont::AndK {
            right: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        5 => Kont::OrK {
            right: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        6 => Kont::NullishK {
            right: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        7 => Kont::MemberK {
            property: StrId(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        8 => Kont::IndexObjK {
            index: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        9 => Kont::IndexKeyK {
            obj: read_value(r)?,
            k: ContId::new(r.u32()?),
        },
        10 => Kont::AssignVarK {
            name: StrId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        11 => Kont::UpdateVarK {
            name: StrId(r.u32()?),
            is_pre: r.bool_()?,
            is_inc: r.bool_()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        12 => Kont::AssignMemberObjK {
            property: StrId(r.u32()?),
            value: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        13 => Kont::AssignMemberValK {
            obj: read_value(r)?,
            property: StrId(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        14 => Kont::AssignIndexObjK {
            index: ExprId(r.u32()?),
            value: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        15 => Kont::AssignIndexKeyK {
            obj: read_value(r)?,
            value: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        16 => Kont::AssignIndexValK {
            obj: read_value(r)?,
            key: read_value(r)?,
            k: ContId::new(r.u32()?),
        },
        17 => Kont::IfK {
            consequent: StmtId(r.u32()?),
            alternate: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        18 => Kont::CondK {
            consequent: ExprId(r.u32()?),
            alternate: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        19 => Kont::WhileK {
            test: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        20 => Kont::WhileBodyK {
            test: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        21 => Kont::ForTestK {
            test: ExprId(r.u32()?),
            update: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        22 => Kont::ForTestResultK {
            test: ExprId(r.u32()?),
            update: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        23 => Kont::ForBodyK {
            test: ExprId(r.u32()?),
            update: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        24 => Kont::ForUpdateK {
            test: ExprId(r.u32()?),
            update: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        25 => Kont::CalleeK {
            args_start: r.u32()?,
            args_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        26 => Kont::ArgsK {
            callee: read_value(r)?,
            done: read_seq_values(r)?,
            args_start: r.u32()?,
            args_idx: r.u16()?,
            args_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        27 => Kont::ReturnK {
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        28 => Kont::ReturnExprK {
            k: ContId::new(r.u32()?),
        },
        29 => Kont::ArrayK {
            done: read_seq_values(r)?,
            elems_start: r.u32()?,
            elems_idx: r.u16()?,
            elems_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        30 => {
            let n = r.u32()? as usize;
            let mut done = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                let key = StrId(r.u32()?);
                let value = read_value(r)?;
                done.push((key, value));
            }
            Kont::ObjectK {
                done,
                props_start: r.u32()?,
                props_idx: r.u16()?,
                props_count: r.u16()?,
                env: EnvId::new(r.u32()?),
                k: ContId::new(r.u32()?),
            }
        }
        31 => Kont::LetK {
            name: StrId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        32 => Kont::ConstK {
            name: StrId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        33 => Kont::VarK {
            name: StrId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        34 => Kont::SeqK {
            stmts_start: r.u32()?,
            stmts_idx: r.u32()?,
            stmts_count: r.u32()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        35 => Kont::ExprStmtK {
            k: ContId::new(r.u32()?),
        },
        36 => Kont::BlockK {
            stmts_start: r.u32()?,
            stmts_idx: r.u32()?,
            stmts_count: r.u32()?,
            final_expr: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        37 => Kont::MatchK {
            arms_start: r.u32()?,
            arms_idx: r.u16()?,
            arms_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        38 => Kont::HandlerK {
            clauses_start: r.u32()?,
            clauses_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            return_body: ExprId(r.u32()?),
            return_param: StrId(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        39 => Kont::PerformArgsK {
            effect: StrId(r.u32()?),
            done: read_seq_values(r)?,
            args_start: r.u32()?,
            args_idx: r.u16()?,
            args_count: r.u16()?,
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        40 => Kont::HandleWithK {
            body: ExprId(r.u32()?),
            env: EnvId::new(r.u32()?),
            k: ContId::new(r.u32()?),
        },
        tag => return Err(JSError::Message(format!("snapshot: bad kont tag {tag}"))),
    })
}

fn write_expr(w: &mut ByteWriter, e: &Expr) {
    match e {
        Expr::Empty => w.u8(0),
        Expr::Undefined => w.u8(1),
        Expr::Null => w.u8(2),
        Expr::Bool(b) => {
            w.u8(3);
            w.bool_(*b);
        }
        Expr::Int(n) => {
            w.u8(4);
            w.i32(*n);
        }
        Expr::Float(idx) => {
            w.u8(5);
            w.u32(*idx);
        }
        Expr::String(s) => {
            w.u8(6);
            w.u32(s.0);
        }
        Expr::Identifier(s) => {
            w.u8(7);
            w.u32(s.0);
        }
        Expr::Binary { op, left, right } => {
            w.u8(8);
            write_binary_op(w, *op);
            w.u32(left.0);
            w.u32(right.0);
        }
        Expr::Unary { op, operand } => {
            w.u8(9);
            write_unary_op(w, *op);
            w.u32(operand.0);
        }
        Expr::Call {
            callee,
            args_start,
            args_count,
        } => {
            w.u8(10);
            w.u32(callee.0);
            w.u32(*args_start);
            w.u16(*args_count);
        }
        Expr::Member { object, property } => {
            w.u8(11);
            w.u32(object.0);
            w.u32(property.0);
        }
        Expr::Index { object, index } => {
            w.u8(12);
            w.u32(object.0);
            w.u32(index.0);
        }
        Expr::Assign { target, value } => {
            w.u8(13);
            w.u32(target.0);
            w.u32(value.0);
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            w.u8(14);
            w.u32(test.0);
            w.u32(consequent.0);
            w.u32(alternate.0);
        }
        Expr::Array {
            elems_start,
            elems_count,
        } => {
            w.u8(15);
            w.u32(*elems_start);
            w.u16(*elems_count);
        }
        Expr::Object {
            props_start,
            props_count,
        } => {
            w.u8(16);
            w.u32(*props_start);
            w.u16(*props_count);
        }
        Expr::Function {
            name,
            params_start,
            params_count,
            body,
        } => {
            w.u8(17);
            w.u32(name.0);
            w.u32(*params_start);
            w.u16(*params_count);
            w.u32(body.0);
        }
        Expr::Handler {
            clauses_start,
            clauses_count,
            return_param,
            return_body,
        } => {
            w.u8(18);
            w.u32(*clauses_start);
            w.u16(*clauses_count);
            w.u32(return_param.0);
            w.u32(return_body.0);
        }
        Expr::Arrow {
            params_start,
            params_count,
            body,
            is_block,
        } => {
            w.u8(19);
            w.u32(*params_start);
            w.u16(*params_count);
            w.u32(body.0);
            w.bool_(*is_block);
        }
        Expr::This => w.u8(20),
        Expr::Block {
            stmts_start,
            stmts_count,
            final_expr,
        } => {
            w.u8(21);
            w.u32(*stmts_start);
            w.u32(*stmts_count);
            w.u32(final_expr.0);
        }
        Expr::Match {
            scrutinee,
            arms_start,
            arms_count,
        } => {
            w.u8(22);
            w.u32(scrutinee.0);
            w.u32(*arms_start);
            w.u16(*arms_count);
        }
        Expr::Perform {
            effect,
            args_start,
            args_count,
        } => {
            w.u8(23);
            w.u32(effect.0);
            w.u32(*args_start);
            w.u16(*args_count);
        }
        Expr::Handle {
            body,
            clauses_start,
            clauses_count,
            return_param,
            return_body,
        } => {
            w.u8(24);
            w.u32(body.0);
            w.u32(*clauses_start);
            w.u16(*clauses_count);
            w.u32(return_param.0);
            w.u32(return_body.0);
        }
        Expr::HandleWith { body, handler } => {
            w.u8(25);
            w.u32(body.0);
            w.u32(handler.0);
        }
    }
}

fn read_expr(r: &mut ByteReader) -> Result<Expr> {
    Ok(match r.u8()? {
        0 => Expr::Empty,
        1 => Expr::Undefined,
        2 => Expr::Null,
        3 => Expr::Bool(r.bool_()?),
        4 => Expr::Int(r.i32()?),
        5 => Expr::Float(r.u32()?),
        6 => Expr::String(StrId(r.u32()?)),
        7 => Expr::Identifier(StrId(r.u32()?)),
        8 => Expr::Binary {
            op: read_binary_op(r)?,
            left: ExprId(r.u32()?),
            right: ExprId(r.u32()?),
        },
        9 => Expr::Unary {
            op: read_unary_op(r)?,
            operand: ExprId(r.u32()?),
        },
        10 => Expr::Call {
            callee: ExprId(r.u32()?),
            args_start: r.u32()?,
            args_count: r.u16()?,
        },
        11 => Expr::Member {
            object: ExprId(r.u32()?),
            property: StrId(r.u32()?),
        },
        12 => Expr::Index {
            object: ExprId(r.u32()?),
            index: ExprId(r.u32()?),
        },
        13 => Expr::Assign {
            target: ExprId(r.u32()?),
            value: ExprId(r.u32()?),
        },
        14 => Expr::Conditional {
            test: ExprId(r.u32()?),
            consequent: ExprId(r.u32()?),
            alternate: ExprId(r.u32()?),
        },
        15 => Expr::Array {
            elems_start: r.u32()?,
            elems_count: r.u16()?,
        },
        16 => Expr::Object {
            props_start: r.u32()?,
            props_count: r.u16()?,
        },
        17 => Expr::Function {
            name: StrId(r.u32()?),
            params_start: r.u32()?,
            params_count: r.u16()?,
            body: StmtId(r.u32()?),
        },
        18 => Expr::Handler {
            clauses_start: r.u32()?,
            clauses_count: r.u16()?,
            return_param: StrId(r.u32()?),
            return_body: ExprId(r.u32()?),
        },
        19 => Expr::Arrow {
            params_start: r.u32()?,
            params_count: r.u16()?,
            body: ExprId(r.u32()?),
            is_block: r.bool_()?,
        },
        20 => Expr::This,
        21 => Expr::Block {
            stmts_start: r.u32()?,
            stmts_count: r.u32()?,
            final_expr: ExprId(r.u32()?),
        },
        22 => Expr::Match {
            scrutinee: ExprId(r.u32()?),
            arms_start: r.u32()?,
            arms_count: r.u16()?,
        },
        23 => Expr::Perform {
            effect: StrId(r.u32()?),
            args_start: r.u32()?,
            args_count: r.u16()?,
        },
        24 => Expr::Handle {
            body: ExprId(r.u32()?),
            clauses_start: r.u32()?,
            clauses_count: r.u16()?,
            return_param: StrId(r.u32()?),
            return_body: ExprId(r.u32()?),
        },
        25 => Expr::HandleWith {
            body: ExprId(r.u32()?),
            handler: ExprId(r.u32()?),
        },
        tag => return Err(JSError::Message(format!("snapshot: bad expr tag {tag}"))),
    })
}

fn write_stmt(w: &mut ByteWriter, s: &Stmt) {
    match s {
        Stmt::Empty => w.u8(0),
        Stmt::Expr(e) => {
            w.u8(1);
            w.u32(e.0);
        }
        Stmt::Let { name, init } => {
            w.u8(2);
            w.u32(name.0);
            w.u32(init.0);
        }
        Stmt::Const { name, init } => {
            w.u8(3);
            w.u32(name.0);
            w.u32(init.0);
        }
        Stmt::Var { name, init } => {
            w.u8(4);
            w.u32(name.0);
            w.u32(init.0);
        }
        Stmt::If {
            test,
            consequent,
            alternate,
        } => {
            w.u8(5);
            w.u32(test.0);
            w.u32(consequent.0);
            w.u32(alternate.0);
        }
        Stmt::While { test, body } => {
            w.u8(6);
            w.u32(test.0);
            w.u32(body.0);
        }
        Stmt::For {
            init,
            test,
            update,
            body,
        } => {
            w.u8(7);
            w.u32(init.0);
            w.u32(test.0);
            w.u32(update.0);
            w.u32(body.0);
        }
        Stmt::Block {
            stmts_start,
            stmts_count,
        } => {
            w.u8(8);
            w.u32(*stmts_start);
            w.u32(*stmts_count);
        }
        Stmt::Declarations {
            stmts_start,
            stmts_count,
        } => {
            w.u8(9);
            w.u32(*stmts_start);
            w.u32(*stmts_count);
        }
        Stmt::Return(e) => {
            w.u8(10);
            w.u32(e.0);
        }
        Stmt::Break => w.u8(11),
        Stmt::Continue => w.u8(12),
        Stmt::Function {
            name,
            params_start,
            params_count,
            body,
        } => {
            w.u8(13);
            w.u32(name.0);
            w.u32(*params_start);
            w.u16(*params_count);
            w.u32(body.0);
        }
    }
}

fn read_stmt(r: &mut ByteReader) -> Result<Stmt> {
    Ok(match r.u8()? {
        0 => Stmt::Empty,
        1 => Stmt::Expr(ExprId(r.u32()?)),
        2 => Stmt::Let {
            name: StrId(r.u32()?),
            init: ExprId(r.u32()?),
        },
        3 => Stmt::Const {
            name: StrId(r.u32()?),
            init: ExprId(r.u32()?),
        },
        4 => Stmt::Var {
            name: StrId(r.u32()?),
            init: ExprId(r.u32()?),
        },
        5 => Stmt::If {
            test: ExprId(r.u32()?),
            consequent: StmtId(r.u32()?),
            alternate: StmtId(r.u32()?),
        },
        6 => Stmt::While {
            test: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
        },
        7 => Stmt::For {
            init: StmtId(r.u32()?),
            test: ExprId(r.u32()?),
            update: ExprId(r.u32()?),
            body: StmtId(r.u32()?),
        },
        8 => Stmt::Block {
            stmts_start: r.u32()?,
            stmts_count: r.u32()?,
        },
        9 => Stmt::Declarations {
            stmts_start: r.u32()?,
            stmts_count: r.u32()?,
        },
        10 => Stmt::Return(ExprId(r.u32()?)),
        11 => Stmt::Break,
        12 => Stmt::Continue,
        13 => Stmt::Function {
            name: StrId(r.u32()?),
            params_start: r.u32()?,
            params_count: r.u16()?,
            body: StmtId(r.u32()?),
        },
        tag => return Err(JSError::Message(format!("snapshot: bad stmt tag {tag}"))),
    })
}

fn write_pattern(w: &mut ByteWriter, p: &Pattern) {
    match p {
        Pattern::Wildcard => w.u8(0),
        Pattern::Literal(e) => {
            w.u8(1);
            w.u32(e.0);
        }
        Pattern::Var(s) => {
            w.u8(2);
            w.u32(s.0);
        }
        Pattern::Array {
            elems_start,
            elems_count,
        } => {
            w.u8(3);
            w.u32(*elems_start);
            w.u16(*elems_count);
        }
        Pattern::Object {
            fields_start,
            fields_count,
        } => {
            w.u8(4);
            w.u32(*fields_start);
            w.u16(*fields_count);
        }
    }
}

fn read_pattern(r: &mut ByteReader) -> Result<Pattern> {
    Ok(match r.u8()? {
        0 => Pattern::Wildcard,
        1 => Pattern::Literal(ExprId(r.u32()?)),
        2 => Pattern::Var(StrId(r.u32()?)),
        3 => Pattern::Array {
            elems_start: r.u32()?,
            elems_count: r.u16()?,
        },
        4 => Pattern::Object {
            fields_start: r.u32()?,
            fields_count: r.u16()?,
        },
        tag => return Err(JSError::Message(format!("snapshot: bad pattern tag {tag}"))),
    })
}

fn write_ast(w: &mut ByteWriter, ast: &AstArena) {
    w.u32(ast.exprs.len() as u32);
    for e in &ast.exprs {
        write_expr(w, e);
    }
    w.u32(ast.stmts.len() as u32);
    for s in &ast.stmts {
        write_stmt(w, s);
    }
    w.u32(ast.patterns.len() as u32);
    for p in &ast.patterns {
        write_pattern(w, p);
    }
    w.u32(ast.expr_lists.len() as u32);
    for e in &ast.expr_lists {
        w.u32(e.0);
    }
    w.u32(ast.stmt_lists.len() as u32);
    for s in &ast.stmt_lists {
        w.u32(s.0);
    }
    w.u32(ast.param_lists.len() as u32);
    for p in &ast.param_lists {
        w.u32(p.0);
    }
    w.u32(ast.prop_lists.len() as u32);
    for p in &ast.prop_lists {
        w.u32(p.key.0);
        w.u32(p.value.0);
    }
    w.u32(ast.pattern_lists.len() as u32);
    for p in &ast.pattern_lists {
        w.u32(p.0);
    }
    w.u32(ast.pattern_fields.len() as u32);
    for f in &ast.pattern_fields {
        w.u32(f.key.0);
        w.u32(f.pattern.0);
    }
    w.u32(ast.arms.len() as u32);
    for a in &ast.arms {
        w.u32(a.pattern.0);
        w.u32(a.guard.0);
        w.u32(a.body.0);
    }
    w.u32(ast.effect_clauses.len() as u32);
    for c in &ast.effect_clauses {
        w.u32(c.effect.0);
        w.u32(c.params_start);
        w.u16(c.params_count);
        w.u32(c.body.0);
    }
    w.u32(ast.floats.len() as u32);
    for f in &ast.floats {
        w.f64(*f);
    }
    w.u32(ast.root_start);
    w.u32(ast.root_count);
}

fn read_ast(r: &mut ByteReader) -> Result<AstArena> {
    let mut ast = AstArena::new();

    let n = r.u32()? as usize;
    ast.exprs = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.exprs.push(read_expr(r)?);
    }

    let n = r.u32()? as usize;
    ast.stmts = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.stmts.push(read_stmt(r)?);
    }

    let n = r.u32()? as usize;
    ast.patterns = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.patterns.push(read_pattern(r)?);
    }

    let n = r.u32()? as usize;
    ast.expr_lists = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.expr_lists.push(ExprId(r.u32()?));
    }

    let n = r.u32()? as usize;
    ast.stmt_lists = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.stmt_lists.push(StmtId(r.u32()?));
    }

    let n = r.u32()? as usize;
    ast.param_lists = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.param_lists.push(StrId(r.u32()?));
    }

    let n = r.u32()? as usize;
    ast.prop_lists = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let key = StrId(r.u32()?);
        let value = ExprId(r.u32()?);
        ast.prop_lists.push(PropEntry { key, value });
    }

    let n = r.u32()? as usize;
    ast.pattern_lists = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.pattern_lists.push(PatternId(r.u32()?));
    }

    let n = r.u32()? as usize;
    ast.pattern_fields = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let key = StrId(r.u32()?);
        let pattern = PatternId(r.u32()?);
        ast.pattern_fields.push(PatternField { key, pattern });
    }

    let n = r.u32()? as usize;
    ast.arms = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let pattern = PatternId(r.u32()?);
        let guard = ExprId(r.u32()?);
        let body = ExprId(r.u32()?);
        ast.arms.push(MatchArm {
            pattern,
            guard,
            body,
        });
    }

    let n = r.u32()? as usize;
    ast.effect_clauses = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let effect = StrId(r.u32()?);
        let params_start = r.u32()?;
        let params_count = r.u16()?;
        let body = ExprId(r.u32()?);
        ast.effect_clauses.push(EffectClause {
            effect,
            params_start,
            params_count,
            body,
        });
    }

    let n = r.u32()? as usize;
    ast.floats = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ast.floats.push(r.f64()?);
    }

    ast.root_start = r.u32()?;
    ast.root_count = r.u32()?;

    Ok(ast)
}

pub const MAGIC: &[u8; 4] = b"KRHT";
pub const VERSION: u8 = 2;

fn write_arena<T>(w: &mut ByteWriter, arena: &Arena<T>, write_elem: fn(&mut ByteWriter, &T)) {
    let slots = arena.slots();
    w.u32(slots.len() as u32);
    for elem in slots {
        write_elem(w, elem);
    }
    let free_list = arena.free_list();
    w.u32(free_list.len() as u32);
    for &idx in free_list {
        w.u32(idx);
    }
    w.u64(arena.allocations());
}

fn read_arena<T>(
    r: &mut ByteReader,
    read_elem: fn(&mut ByteReader) -> Result<T>,
) -> Result<Arena<T>> {
    let n = r.u32()? as usize;
    let mut data = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        data.push(read_elem(r)?);
    }
    let free_n = r.u32()? as usize;
    let mut free_list = Vec::with_capacity(free_n.min(1 << 16));
    for _ in 0..free_n {
        free_list.push(r.u32()?);
    }
    let allocations = r.u64()?;
    Ok(Arena::from_parts(data, free_list, allocations))
}

fn write_globals(w: &mut ByteWriter, globals: &HashMap<StrId, JSValue>) {
    w.u32(globals.len() as u32);
    for (k, v) in globals {
        w.u32(k.0);
        write_value(w, *v);
    }
}

fn read_globals(r: &mut ByteReader) -> Result<HashMap<StrId, JSValue>> {
    let n = r.u32()? as usize;
    let mut map = HashMap::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let k = StrId(r.u32()?);
        let v = read_value(r)?;
        map.insert(k, v);
    }
    Ok(map)
}

/// Serialize the whole runtime: session state (strings/ast), the CEKH heap
/// (objects/envs/conts/handlers/globals), the fiber table, and scheduler
/// bookkeeping. `current` is intentionally NOT serialized — a restored
/// runtime always starts with no fiber selected; `select_next_fiber` picks
/// one from the restored ready queue on the next `run_resumed`.
///
/// `ready_override` lets a caller (e.g. the Snapshot! effect handler) put
/// the performing fiber at the front of the queue in the snapshot without
/// mutating the live `rt.ready_queue`.
pub fn write_runtime(rt: &Runtime, ready_override: &VecDeque<FiberId>) -> Vec<u8> {
    let mut w = ByteWriter::new();
    for &b in MAGIC {
        w.u8(b);
    }
    w.u8(VERSION);

    write_strings(&mut w, &rt.interpreter.strings);
    write_ast(&mut w, &rt.ast);

    write_arena(&mut w, &rt.interpreter.objects, write_object);
    write_arena(&mut w, rt.interpreter.envs.arena(), write_env);
    write_arena(&mut w, rt.interpreter.conts.arena(), write_kont);
    write_arena(&mut w, rt.interpreter.handlers.arena(), write_handler);

    write_control(&mut w, &rt.interpreter.control);
    w.u32(rt.interpreter.env.index() as u32);
    w.u32(rt.interpreter.cont.index() as u32);

    write_globals(&mut w, &rt.interpreter.globals);

    w.u32(rt.fibers.len() as u32);
    for fiber in &rt.fibers {
        write_fiber(&mut w, fiber);
    }

    w.u32(ready_override.len() as u32);
    for id in ready_override {
        w.u32(id.0);
    }
    w.u32(rt.next_fiber_id);
    w.u32(rt.join_waiters.len() as u32);
    for (fiber_id, waiters) in &rt.join_waiters {
        w.u32(fiber_id.0);
        w.u32(waiters.len() as u32);
        for waiter in waiters {
            w.u32(waiter.0);
        }
    }

    w.u32(rt.effects.len() as u32);
    for (name, kind) in &rt.effects {
        w.u32(name.0);
        w.u8(match kind {
            EffectKind::Builtin(BuiltinEffect::Print) => 0,
            EffectKind::Builtin(BuiltinEffect::Fork) => 1,
            EffectKind::Builtin(BuiltinEffect::Join) => 2,
            EffectKind::Builtin(BuiltinEffect::Gc) => 3,
            EffectKind::Builtin(BuiltinEffect::Snapshot) => 4,
            EffectKind::Host => 5,
        });
    }

    w.finish()
}

/// Reconstruct a `Runtime` from bytes produced by `write_runtime`.
/// `current` is not part of the format; the restored runtime starts with
/// no fiber selected.
pub fn read_runtime(bytes: &[u8]) -> Result<Runtime> {
    let mut r = ByteReader::new(bytes);
    let magic = [r.u8()?, r.u8()?, r.u8()?, r.u8()?];
    if &magic != MAGIC {
        return Err(JSError::Message("snapshot: bad magic".to_string()));
    }
    let version = r.u8()?;
    if version != VERSION {
        return Err(JSError::Message(format!(
            "snapshot: unsupported version {version} (expected {VERSION})"
        )));
    }

    let strings = read_strings(&mut r)?;
    let ast = read_ast(&mut r)?;

    let objects = read_arena(&mut r, read_object)?;
    let env_arena = read_arena(&mut r, read_env)?;
    let cont_arena = read_arena(&mut r, read_kont)?;
    // `from_arena` only debug_asserts non-emptiness; a format-valid but
    // crafted/corrupt snapshot with an empty env or cont section would
    // otherwise panic debug builds instead of failing gracefully.
    if env_arena.slots().is_empty() {
        return Err(JSError::Message(
            "snapshot: env arena missing global env at slot 0".to_string(),
        ));
    }
    if cont_arena.slots().is_empty() {
        return Err(JSError::Message(
            "snapshot: cont arena missing halt cont at slot 0".to_string(),
        ));
    }
    let envs = EnvArena::from_arena(env_arena);
    let conts = ContArena::from_arena(cont_arena);
    let handlers = HandlerArena::from_arena(read_arena(&mut r, read_handler)?);

    let control = read_control(&mut r)?;
    let env = EnvId::new(r.u32()?);
    let cont = ContId::new(r.u32()?);

    let globals = read_globals(&mut r)?;

    let n = r.u32()? as usize;
    let mut fibers = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        fibers.push(read_fiber(&mut r)?);
    }

    let n = r.u32()? as usize;
    let mut ready_queue = VecDeque::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        ready_queue.push_back(FiberId(r.u32()?));
    }
    let next_fiber_id = r.u32()?;

    let n = r.u32()? as usize;
    let mut join_waiters = HashMap::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let fiber_id = FiberId(r.u32()?);
        let wn = r.u32()? as usize;
        let mut waiters = Vec::with_capacity(wn.min(1 << 16));
        for _ in 0..wn {
            waiters.push(FiberId(r.u32()?));
        }
        join_waiters.insert(fiber_id, waiters);
    }

    let n = r.u32()? as usize;
    let mut effects = HashMap::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let name = StrId(r.u32()?);
        let kind = match r.u8()? {
            0 => EffectKind::Builtin(BuiltinEffect::Print),
            1 => EffectKind::Builtin(BuiltinEffect::Fork),
            2 => EffectKind::Builtin(BuiltinEffect::Join),
            3 => EffectKind::Builtin(BuiltinEffect::Gc),
            4 => EffectKind::Builtin(BuiltinEffect::Snapshot),
            5 => EffectKind::Host,
            tag => {
                return Err(JSError::Message(format!(
                    "snapshot: bad effect kind tag {tag}"
                )));
            }
        };
        effects.insert(name, kind);
    }

    let interpreter = CEKH {
        control,
        env,
        cont,
        envs,
        conts,
        handlers,
        objects,
        strings,
        globals,
    };

    let mut gc = GC::new();
    gc.reprime_baseline(&interpreter);

    Ok(Runtime {
        interpreter,
        fibers,
        ready_queue,
        current: None,
        next_fiber_id,
        join_waiters,
        gc,
        ast,
        effects,
        // A restored runtime never inherits log mode — LogMode is runtime-local.
        log: crate::replay::LogMode::Off,
    })
}

/// Write to a temp file and rename so a process kill mid-write can't
/// truncate/corrupt the last good checkpoint on disk.
pub(crate) fn write_snapshot_file(path: &str, bytes: &[u8]) -> Result<()> {
    let tmp_path = format!("{path}.tmp");
    std::fs::write(&tmp_path, bytes)
        .map_err(|e| JSError::Message(format!("Snapshot: cannot write {tmp_path}: {e}")))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| JSError::Message(format!("Snapshot: cannot rename {tmp_path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_round_trips_through_bytes() {
        use crate::parser::Parser;

        let parser =
            Parser::new("function f(x) { return match (x) { {ok} => ok, _ => 0 } } f({ok: 5})")
                .unwrap();
        let (ast, _strings) = parser.parse_program().unwrap();

        let mut w = ByteWriter::new();
        write_ast(&mut w, &ast);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let ast2 = read_ast(&mut r).unwrap();
        assert!(r.is_at_end());
        assert_eq!(ast2.exprs.len(), ast.exprs.len());
        assert_eq!(ast2.stmts.len(), ast.stmts.len());
        assert_eq!(ast2.root_start, ast.root_start);
        assert_eq!(ast2.root_count, ast.root_count);
    }

    #[test]
    fn kont_round_trips() {
        use crate::cont::Kont;

        let k = Kont::ArgsK {
            callee: crate::JSValue::Function(crate::ObjId(2)),
            done: vec![crate::JSValue::Int(1)],
            args_start: 5,
            args_idx: 1,
            args_count: 2,
            env: crate::EnvId::new(0),
            k: crate::ContId::new(0),
        };
        let mut w = ByteWriter::new();
        write_kont(&mut w, &k);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert_eq!(read_kont(&mut r).unwrap(), k);
        assert!(r.is_at_end());
    }

    #[test]
    fn primitives_round_trip() {
        let mut w = ByteWriter::new();
        w.u8(7);
        w.u16(65_000);
        w.u32(4_000_000_000);
        w.u64(u64::MAX);
        w.i32(-42);
        w.f64(-0.5);
        w.bool_(true);
        w.str_("крихта");
        let bytes = w.finish();

        let mut r = ByteReader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 65_000);
        assert_eq!(r.u32().unwrap(), 4_000_000_000);
        assert_eq!(r.u64().unwrap(), u64::MAX);
        assert_eq!(r.i32().unwrap(), -42);
        assert_eq!(r.f64().unwrap(), -0.5);
        assert!(r.bool_().unwrap());
        assert_eq!(r.str_().unwrap(), "крихта");
        assert!(r.is_at_end());
    }

    #[test]
    fn truncated_input_errors_not_panics() {
        let mut w = ByteWriter::new();
        w.u32(1234);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes[..2]);
        assert!(r.u32().is_err());
    }

    #[test]
    fn bogus_string_length_errors() {
        let mut w = ByteWriter::new();
        w.u32(u32::MAX); // claims a 4GB string
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert!(r.str_().is_err());
    }

    #[test]
    fn value_and_object_round_trip() {
        use crate::object::{Object, Property};
        use crate::string_pool::StrId;
        use crate::value::JSValue;

        let mut w = ByteWriter::new();
        write_value(
            &mut w,
            JSValue::Continuation(crate::ContId::new(3), crate::EnvId::new(9)),
        );
        write_value(&mut w, JSValue::Float(6.25));

        let mut obj = Object::new();
        obj.properties
            .insert(StrId(4), Property::readonly(JSValue::Int(7)));
        obj.prototype = Some(crate::ObjId(11));
        write_object(&mut w, &obj);

        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert_eq!(
            read_value(&mut r).unwrap(),
            JSValue::Continuation(crate::ContId::new(3), crate::EnvId::new(9))
        );
        assert_eq!(read_value(&mut r).unwrap(), JSValue::Float(6.25));
        let obj2 = read_object(&mut r).unwrap();
        assert_eq!(obj2.get(StrId(4)), Some(JSValue::Int(7)));
        assert_eq!(obj2.prototype, Some(crate::ObjId(11)));
        assert!(!obj2.properties[&StrId(4)].writable);
        assert!(r.is_at_end());
    }

    #[test]
    fn object_kind_variants_round_trip() {
        use crate::object::{
            ArrayData, BoundFunctionData, FunctionData, NativeFn, Object, ObjectKind,
        };

        let mut arr = Object::new();
        arr.kind = ObjectKind::Array(ArrayData {
            elements: vec![JSValue::Int(1), JSValue::Undefined],
        });

        let mut func = Object::new();
        func.kind = ObjectKind::Function(FunctionData {
            params_start: 2,
            params_count: 3,
            body: StmtId(5),
            expr_body: Some(ExprId(6)),
            env: EnvId::new(7),
            name: Some(StrId(8)),
        });

        let mut bound = Object::new();
        bound.kind = ObjectKind::BoundFunction(BoundFunctionData {
            target: ObjId(12),
            this_arg: JSValue::Bool(true),
            bound_args: vec![JSValue::Int(9)],
        });

        let mut native = Object::new();
        native.kind = ObjectKind::NativeFunction(NativeFn::MathSign);

        let mut w = ByteWriter::new();
        write_object(&mut w, &arr);
        write_object(&mut w, &func);
        write_object(&mut w, &bound);
        write_object(&mut w, &native);
        let bytes = w.finish();

        let mut r = ByteReader::new(&bytes);
        match read_object(&mut r).unwrap().kind {
            ObjectKind::Array(a) => {
                assert_eq!(a.elements, vec![JSValue::Int(1), JSValue::Undefined])
            }
            _ => panic!("expected Array"),
        }
        match read_object(&mut r).unwrap().kind {
            ObjectKind::Function(f) => {
                assert_eq!(f.params_start, 2);
                assert_eq!(f.params_count, 3);
                assert_eq!(f.body, StmtId(5));
                assert_eq!(f.expr_body, Some(ExprId(6)));
                assert_eq!(f.env.index(), 7);
                assert_eq!(f.name, Some(StrId(8)));
            }
            _ => panic!("expected Function"),
        }
        match read_object(&mut r).unwrap().kind {
            ObjectKind::BoundFunction(b) => {
                assert_eq!(b.target, ObjId(12));
                assert_eq!(b.this_arg, JSValue::Bool(true));
                assert_eq!(b.bound_args, vec![JSValue::Int(9)]);
            }
            _ => panic!("expected BoundFunction"),
        }
        match read_object(&mut r).unwrap().kind {
            ObjectKind::NativeFunction(nf) => assert_eq!(nf, NativeFn::MathSign),
            _ => panic!("expected NativeFunction"),
        }
        assert!(r.is_at_end());

        // Unknown object-kind tag must error, never panic.
        let mut w2 = ByteWriter::new();
        w2.u32(0); // empty properties map
        w2.bool_(false); // no prototype
        w2.u8(99); // bogus kind tag
        let bytes2 = w2.finish();
        let mut r2 = ByteReader::new(&bytes2);
        assert!(read_object(&mut r2).is_err());
    }

    #[test]
    fn env_round_trip() {
        let env = Env::with_binding_slice(
            &[(StrId(1), JSValue::Int(5)), (StrId(2), JSValue::Bool(true))],
            Some(EnvId::new(3)),
        );
        let mut w = ByteWriter::new();
        write_env(&mut w, &env);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let env2 = read_env(&mut r).unwrap();
        assert_eq!(env2, env);
        assert!(r.is_at_end());
    }

    #[test]
    fn handler_round_trip() {
        let handler = Handler::new(10, 2, StrId(4), ExprId(20), EnvId::new(1));
        let mut w = ByteWriter::new();
        write_handler(&mut w, &handler);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert_eq!(read_handler(&mut r).unwrap(), handler);
        assert!(r.is_at_end());
    }

    #[test]
    fn control_round_trip() {
        let mut w = ByteWriter::new();
        write_control(&mut w, &Control::Expr(ExprId(1)));
        write_control(&mut w, &Control::Stmt(StmtId(2)));
        write_control(&mut w, &Control::Value(JSValue::Int(3)));
        write_control(&mut w, &Control::Returning(JSValue::Int(4)));
        write_control(&mut w, &Control::Halted(JSValue::Int(5)));
        write_control(
            &mut w,
            &Control::Suspend {
                effect: StrId(6),
                args: vec![JSValue::Int(7)],
            },
        );
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);

        assert!(matches!(read_control(&mut r).unwrap(), Control::Expr(e) if e == ExprId(1)));
        assert!(matches!(read_control(&mut r).unwrap(), Control::Stmt(s) if s == StmtId(2)));
        assert!(matches!(read_control(&mut r).unwrap(), Control::Value(v) if v == JSValue::Int(3)));
        assert!(
            matches!(read_control(&mut r).unwrap(), Control::Returning(v) if v == JSValue::Int(4))
        );
        assert!(
            matches!(read_control(&mut r).unwrap(), Control::Halted(v) if v == JSValue::Int(5))
        );
        match read_control(&mut r).unwrap() {
            Control::Suspend { effect, args } => {
                assert_eq!(effect, StrId(6));
                assert_eq!(args, vec![JSValue::Int(7)]);
            }
            _ => panic!("expected Suspend"),
        }
        assert!(r.is_at_end());

        let mut bad = ByteWriter::new();
        bad.u8(99);
        let bad_bytes = bad.finish();
        let mut bad_r = ByteReader::new(&bad_bytes);
        assert!(read_control(&mut bad_r).is_err());
    }

    #[test]
    fn fiber_round_trip() {
        let fiber = Fiber {
            id: FiberId(1),
            control: Control::Suspend {
                effect: StrId(2),
                args: vec![JSValue::Int(3)],
            },
            cont: ContId::new(4),
            env: EnvId::new(5),
            status: FiberStatus::Blocked {
                effect: StrId(6),
                args: vec![JSValue::Bool(false)],
            },
        };
        let mut w = ByteWriter::new();
        write_fiber(&mut w, &fiber);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let fiber2 = read_fiber(&mut r).unwrap();

        assert_eq!(fiber2.id, FiberId(1));
        assert_eq!(fiber2.cont.index(), 4);
        assert_eq!(fiber2.env.index(), 5);
        match fiber2.control {
            Control::Suspend { effect, args } => {
                assert_eq!(effect, StrId(2));
                assert_eq!(args, vec![JSValue::Int(3)]);
            }
            _ => panic!("expected Suspend control"),
        }
        match fiber2.status {
            FiberStatus::Blocked { effect, args } => {
                assert_eq!(effect, StrId(6));
                assert_eq!(args, vec![JSValue::Bool(false)]);
            }
            _ => panic!("expected Blocked status"),
        }
        assert!(r.is_at_end());
    }

    #[test]
    fn fiber_status_all_variants_round_trip() {
        let mut w = ByteWriter::new();
        write_fiber_status(&mut w, &FiberStatus::Ready);
        write_fiber_status(&mut w, &FiberStatus::Running);
        write_fiber_status(&mut w, &FiberStatus::Completed(JSValue::Int(1)));
        write_fiber_status(
            &mut w,
            &FiberStatus::Failed(JSError::Message("boom".to_string())),
        );
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);

        assert!(matches!(
            read_fiber_status(&mut r).unwrap(),
            FiberStatus::Ready
        ));
        assert!(matches!(
            read_fiber_status(&mut r).unwrap(),
            FiberStatus::Running
        ));
        match read_fiber_status(&mut r).unwrap() {
            FiberStatus::Completed(v) => assert_eq!(v, JSValue::Int(1)),
            _ => panic!("expected Completed"),
        }
        match read_fiber_status(&mut r).unwrap() {
            FiberStatus::Failed(e) => assert_eq!(e.to_string(), "boom"),
            _ => panic!("expected Failed"),
        }
        assert!(r.is_at_end());

        let mut bad = ByteWriter::new();
        bad.u8(99);
        let bad_bytes = bad.finish();
        let mut bad_r = ByteReader::new(&bad_bytes);
        assert!(read_fiber_status(&mut bad_r).is_err());
    }

    #[test]
    fn strings_round_trip() {
        let mut pool = StringPool::new();
        let a = pool.intern("alpha");
        let b = pool.intern("beta");
        let empty = pool.intern("");

        let mut w = ByteWriter::new();
        write_strings(&mut w, &pool);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let pool2 = read_strings(&mut r).unwrap();

        assert_eq!(pool2.len(), pool.len());
        assert_eq!(pool2.get(StrId(0)), Some(""));
        assert_eq!(pool2.get(a), Some("alpha"));
        assert_eq!(pool2.get(b), Some("beta"));
        assert_eq!(empty, StrId(0));
        assert!(r.is_at_end());
    }

    #[test]
    fn binary_and_unary_op_round_trip() {
        let bin_ops = [
            BinaryOp::Add,
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Mod,
            BinaryOp::Pow,
            BinaryOp::Eq,
            BinaryOp::Ne,
            BinaryOp::Lt,
            BinaryOp::Le,
            BinaryOp::Gt,
            BinaryOp::Ge,
            BinaryOp::And,
            BinaryOp::Or,
            BinaryOp::BitAnd,
            BinaryOp::BitOr,
            BinaryOp::BitXor,
            BinaryOp::Shl,
            BinaryOp::Shr,
            BinaryOp::UShr,
            BinaryOp::NullishCoalesce,
        ];
        let mut w = ByteWriter::new();
        for op in bin_ops {
            write_binary_op(&mut w, op);
        }
        let unary_ops = [
            UnaryOp::Neg,
            UnaryOp::Not,
            UnaryOp::BitNot,
            UnaryOp::TypeOf,
            UnaryOp::Plus,
            UnaryOp::PreInc,
            UnaryOp::PreDec,
            UnaryOp::PostInc,
            UnaryOp::PostDec,
        ];
        for op in unary_ops {
            write_unary_op(&mut w, op);
        }
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        for op in bin_ops {
            assert_eq!(read_binary_op(&mut r).unwrap(), op);
        }
        for op in unary_ops {
            assert_eq!(read_unary_op(&mut r).unwrap(), op);
        }
        assert!(r.is_at_end());

        let mut bad = ByteWriter::new();
        bad.u8(99);
        let bad_bytes = bad.finish();
        assert!(read_binary_op(&mut ByteReader::new(&bad_bytes)).is_err());
        assert!(read_unary_op(&mut ByteReader::new(&bad_bytes)).is_err());
    }

    #[test]
    fn all_kont_variants_round_trip() {
        let e = ExprId(1);
        let e2 = ExprId(2);
        let e3 = ExprId(3);
        let s = StmtId(1);
        let s2 = StmtId(2);
        let env = EnvId::new(1);
        let k = ContId::new(1);
        let name = StrId(1);

        let variants = vec![
            Kont::Halt,
            Kont::UnaryK {
                op: UnaryOp::Neg,
                k,
            },
            Kont::BinaryLeftK {
                op: BinaryOp::Add,
                right: e,
                env,
                k,
            },
            Kont::BinaryRightK {
                op: BinaryOp::Sub,
                left: JSValue::Int(4),
                k,
            },
            Kont::AndK { right: e, env, k },
            Kont::OrK { right: e, env, k },
            Kont::NullishK { right: e, env, k },
            Kont::MemberK { property: name, k },
            Kont::IndexObjK { index: e, env, k },
            Kont::IndexKeyK {
                obj: JSValue::Undefined,
                k,
            },
            Kont::AssignVarK { name, env, k },
            Kont::UpdateVarK {
                name,
                is_pre: true,
                is_inc: false,
                env,
                k,
            },
            Kont::AssignMemberObjK {
                property: name,
                value: e,
                env,
                k,
            },
            Kont::AssignMemberValK {
                obj: JSValue::Bool(true),
                property: name,
                k,
            },
            Kont::AssignIndexObjK {
                index: e,
                value: e2,
                env,
                k,
            },
            Kont::AssignIndexKeyK {
                obj: JSValue::Bool(false),
                value: e,
                env,
                k,
            },
            Kont::AssignIndexValK {
                obj: JSValue::Int(1),
                key: JSValue::Int(2),
                k,
            },
            Kont::IfK {
                consequent: s,
                alternate: s2,
                env,
                k,
            },
            Kont::CondK {
                consequent: e,
                alternate: e2,
                env,
                k,
            },
            Kont::WhileK {
                test: e,
                body: s,
                env,
                k,
            },
            Kont::WhileBodyK {
                test: e,
                body: s,
                env,
                k,
            },
            Kont::ForTestK {
                test: e,
                update: e2,
                body: s,
                env,
                k,
            },
            Kont::ForTestResultK {
                test: e,
                update: e2,
                body: s,
                env,
                k,
            },
            Kont::ForBodyK {
                test: e,
                update: e2,
                body: s,
                env,
                k,
            },
            Kont::ForUpdateK {
                test: e,
                update: e2,
                body: s,
                env,
                k,
            },
            Kont::CalleeK {
                args_start: 3,
                args_count: 4,
                env,
                k,
            },
            Kont::ArgsK {
                callee: JSValue::Function(ObjId(2)),
                done: vec![JSValue::Int(1)],
                args_start: 5,
                args_idx: 1,
                args_count: 2,
                env,
                k,
            },
            Kont::ReturnK { env, k },
            Kont::ReturnExprK { k },
            Kont::ArrayK {
                done: vec![JSValue::Int(9)],
                elems_start: 1,
                elems_idx: 0,
                elems_count: 3,
                env,
                k,
            },
            Kont::ObjectK {
                done: vec![(name, JSValue::Int(7))],
                props_start: 2,
                props_idx: 1,
                props_count: 5,
                env,
                k,
            },
            Kont::LetK { name, env, k },
            Kont::ConstK { name, env, k },
            Kont::VarK { name, env, k },
            Kont::SeqK {
                stmts_start: 1,
                stmts_idx: 2,
                stmts_count: 3,
                env,
                k,
            },
            Kont::ExprStmtK { k },
            Kont::BlockK {
                stmts_start: 1,
                stmts_idx: 2,
                stmts_count: 3,
                final_expr: e,
                env,
                k,
            },
            Kont::MatchK {
                arms_start: 1,
                arms_idx: 0,
                arms_count: 2,
                env,
                k,
            },
            Kont::HandlerK {
                clauses_start: 1,
                clauses_count: 2,
                env,
                return_body: e,
                return_param: name,
                k,
            },
            Kont::PerformArgsK {
                effect: name,
                done: vec![JSValue::Int(6)],
                args_start: 1,
                args_idx: 0,
                args_count: 2,
                env,
                k,
            },
            Kont::HandleWithK { body: e3, env, k },
        ];

        assert_eq!(variants.len(), 41, "expected all 41 Kont variants covered");

        for variant in &variants {
            let mut w = ByteWriter::new();
            write_kont(&mut w, variant);
            let bytes = w.finish();
            let mut r = ByteReader::new(&bytes);
            let decoded = read_kont(&mut r).unwrap();
            assert_eq!(&decoded, variant);
            assert!(r.is_at_end());
        }

        let mut bad = ByteWriter::new();
        bad.u8(200);
        let bad_bytes = bad.finish();
        assert!(read_kont(&mut ByteReader::new(&bad_bytes)).is_err());
    }

    #[test]
    fn all_expr_variants_round_trip() {
        let e = ExprId(1);
        let e2 = ExprId(2);
        let e3 = ExprId(3);
        let s = StmtId(1);
        let name = StrId(1);

        let variants = vec![
            Expr::Empty,
            Expr::Undefined,
            Expr::Null,
            Expr::Bool(true),
            Expr::Int(42),
            Expr::Float(3),
            Expr::String(name),
            Expr::Identifier(name),
            Expr::Binary {
                op: BinaryOp::Add,
                left: e,
                right: e2,
            },
            Expr::Unary {
                op: UnaryOp::Neg,
                operand: e,
            },
            Expr::Call {
                callee: e,
                args_start: 1,
                args_count: 2,
            },
            Expr::Member {
                object: e,
                property: name,
            },
            Expr::Index {
                object: e,
                index: e2,
            },
            Expr::Assign {
                target: e,
                value: e2,
            },
            Expr::Conditional {
                test: e,
                consequent: e2,
                alternate: e3,
            },
            Expr::Array {
                elems_start: 1,
                elems_count: 2,
            },
            Expr::Object {
                props_start: 1,
                props_count: 2,
            },
            Expr::Function {
                name,
                params_start: 1,
                params_count: 2,
                body: s,
            },
            Expr::Handler {
                clauses_start: 1,
                clauses_count: 2,
                return_param: name,
                return_body: e,
            },
            Expr::Arrow {
                params_start: 1,
                params_count: 2,
                body: e,
                is_block: true,
            },
            Expr::This,
            Expr::Block {
                stmts_start: 1,
                stmts_count: 2,
                final_expr: e,
            },
            Expr::Match {
                scrutinee: e,
                arms_start: 1,
                arms_count: 2,
            },
            Expr::Perform {
                effect: name,
                args_start: 1,
                args_count: 2,
            },
            Expr::Handle {
                body: e,
                clauses_start: 1,
                clauses_count: 2,
                return_param: name,
                return_body: e2,
            },
            Expr::HandleWith {
                body: e,
                handler: e2,
            },
        ];

        assert_eq!(variants.len(), 26, "expected all 26 Expr variants covered");

        for variant in &variants {
            let mut w = ByteWriter::new();
            write_expr(&mut w, variant);
            let bytes = w.finish();
            let mut r = ByteReader::new(&bytes);
            let decoded = read_expr(&mut r).unwrap();
            let mut w2 = ByteWriter::new();
            write_expr(&mut w2, &decoded);
            assert_eq!(w2.finish(), bytes);
            assert!(r.is_at_end());
        }

        let mut bad = ByteWriter::new();
        bad.u8(200);
        let bad_bytes = bad.finish();
        assert!(read_expr(&mut ByteReader::new(&bad_bytes)).is_err());
    }

    #[test]
    fn all_stmt_variants_round_trip() {
        let e = ExprId(1);
        let s = StmtId(1);
        let s2 = StmtId(2);
        let s3 = StmtId(3);
        let name = StrId(1);

        let variants = vec![
            Stmt::Empty,
            Stmt::Expr(e),
            Stmt::Let { name, init: e },
            Stmt::Const { name, init: e },
            Stmt::Var { name, init: e },
            Stmt::If {
                test: e,
                consequent: s,
                alternate: s2,
            },
            Stmt::While { test: e, body: s },
            Stmt::For {
                init: s,
                test: e,
                update: e,
                body: s2,
            },
            Stmt::Block {
                stmts_start: 1,
                stmts_count: 2,
            },
            Stmt::Declarations {
                stmts_start: 1,
                stmts_count: 2,
            },
            Stmt::Return(e),
            Stmt::Break,
            Stmt::Continue,
            Stmt::Function {
                name,
                params_start: 1,
                params_count: 2,
                body: s3,
            },
        ];

        assert_eq!(variants.len(), 14, "expected all 14 Stmt variants covered");

        for variant in &variants {
            let mut w = ByteWriter::new();
            write_stmt(&mut w, variant);
            let bytes = w.finish();
            let mut r = ByteReader::new(&bytes);
            let decoded = read_stmt(&mut r).unwrap();
            let mut w2 = ByteWriter::new();
            write_stmt(&mut w2, &decoded);
            assert_eq!(w2.finish(), bytes);
            assert!(r.is_at_end());
        }

        let mut bad = ByteWriter::new();
        bad.u8(200);
        let bad_bytes = bad.finish();
        assert!(read_stmt(&mut ByteReader::new(&bad_bytes)).is_err());
    }

    #[test]
    fn all_pattern_variants_round_trip() {
        let variants = vec![
            Pattern::Wildcard,
            Pattern::Literal(ExprId(1)),
            Pattern::Var(StrId(1)),
            Pattern::Array {
                elems_start: 1,
                elems_count: 2,
            },
            Pattern::Object {
                fields_start: 1,
                fields_count: 2,
            },
        ];

        assert_eq!(variants.len(), 5, "expected all 5 Pattern variants covered");

        for variant in &variants {
            let mut w = ByteWriter::new();
            write_pattern(&mut w, variant);
            let bytes = w.finish();
            let mut r = ByteReader::new(&bytes);
            let decoded = read_pattern(&mut r).unwrap();
            let mut w2 = ByteWriter::new();
            write_pattern(&mut w2, &decoded);
            assert_eq!(w2.finish(), bytes);
            assert!(r.is_at_end());
        }

        let mut bad = ByteWriter::new();
        bad.u8(200);
        let bad_bytes = bad.finish();
        assert!(read_pattern(&mut ByteReader::new(&bad_bytes)).is_err());
    }

    #[test]
    fn runtime_round_trips_and_continues() {
        use crate::Runtime;

        let mut rt = Runtime::new();
        rt.eval("function inc(x) { return x + 1 } var state = { count: inc(41) }")
            .unwrap();

        let bytes = write_runtime(&rt, &rt.ready_queue);
        let mut rt2 = Runtime::from_snapshot(&bytes).unwrap();
        // restored session state is fully usable: closure + heap survive
        assert_eq!(
            rt2.eval("state.count === 42 && inc(1) === 2").unwrap(),
            crate::JSValue::Bool(true)
        );
    }

    #[test]
    fn bad_magic_and_version_are_rejected() {
        use crate::Runtime;

        assert!(Runtime::from_snapshot(b"NOPE").is_err());

        let rt = Runtime::new();
        let mut bytes = write_runtime(&rt, &rt.ready_queue);
        bytes[4] = 99; // version byte
        assert!(Runtime::from_snapshot(&bytes).is_err());
        bytes.truncate(bytes.len() / 2);
        assert!(Runtime::from_snapshot(&bytes).is_err());
    }

    #[test]
    fn effect_registry_round_trips_including_grants() {
        use crate::runtime::{EffectKind, Runtime};
        let mut rt = Runtime::new();
        rt.grant("FetchUrl").unwrap();
        let bytes = write_runtime(&rt, &rt.ready_queue.clone());
        let mut rt2 = read_runtime(&bytes).unwrap();
        // StringPool has no `lookup(&str)`; `intern` returns the existing id
        // for an already-interned string, so it doubles as a lookup here.
        let fetch = rt2.interpreter.strings.intern("FetchUrl");
        assert_eq!(rt2.effects.get(&fetch), Some(&EffectKind::Host));
        assert_eq!(rt2.effects.len(), rt.effects.len());
    }

    #[test]
    fn blocked_on_host_status_round_trips() {
        use crate::runtime::FiberStatus;
        let mut w = ByteWriter::new();
        write_fiber_status(
            &mut w,
            &FiberStatus::BlockedOnHost {
                effect: StrId(7),
                args: vec![
                    HostValue::Int(1),
                    HostValue::Bool(true),
                    HostValue::Array(vec![HostValue::Str("nested".to_string())]),
                    HostValue::Object(vec![("k".to_string(), HostValue::Null)]),
                ],
            },
        );
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let FiberStatus::BlockedOnHost { effect, args } = read_fiber_status(&mut r).unwrap() else {
            panic!("wrong status variant");
        };
        assert_eq!(effect, StrId(7));
        assert_eq!(
            args,
            vec![
                HostValue::Int(1),
                HostValue::Bool(true),
                HostValue::Array(vec![HostValue::Str("nested".to_string())]),
                HostValue::Object(vec![("k".to_string(), HostValue::Null)]),
            ]
        );
    }

    #[test]
    fn deeply_nested_host_value_errors_instead_of_overflowing() {
        // Crafted input: 300 one-element Array headers with no real payload
        // behind them — cheap bytes, unbounded recursion if uncapped.
        let mut w = ByteWriter::new();
        for _ in 0..300 {
            w.u8(6);
            w.u32(1);
        }
        w.u8(1);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let err = read_host_value(&mut r).unwrap_err();
        assert!(err.to_string().contains("too deep"), "{err}");
    }

    #[test]
    fn host_value_nesting_below_the_cap_round_trips() {
        let mut v = HostValue::Int(1);
        for _ in 0..200 {
            v = HostValue::Array(vec![v]);
        }
        let mut w = ByteWriter::new();
        write_host_value(&mut w, &v);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert_eq!(read_host_value(&mut r).unwrap(), v);
    }
}
