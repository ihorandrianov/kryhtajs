//! Snapshot serialization: whole-runtime durable execution.
//!
//! Format: magic "KRHT", version u8, little-endian, length-prefixed.

use std::collections::HashMap;

use crate::ast::{ExprId, StmtId};
use crate::cekh::Control;
use crate::env::{Env, EnvId};
use crate::error::{JSError, Result};
use crate::handler::Handler;
use crate::object::{
    ArrayData, BoundFunctionData, FunctionData, NativeFn, Object, ObjectKind, Property,
};
use crate::runtime::{Fiber, FiberId, FiberStatus};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
