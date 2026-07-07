# Durable Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `perform Snapshot!("file.snap")` serializes the whole runtime to a self-contained binary file; `kryhta --resume file.snap` wakes it up inside that effect with the value `"restored"`.

**Architecture:** New zero-dependency `src/snapshot.rs` with `ByteWriter`/`ByteReader` primitives and a `write_*`/`read_*` pair per machine type; a `Snapshot` runtime effect handled in `Runtime::handle_effect`; `Runtime::run` split into setup + `run_scheduler()` so resume enters the loop without the per-run reset.

**Tech Stack:** Rust edition 2024, no new dependencies. Spec: `docs/superpowers/specs/2026-07-07-durable-execution-design.md`.

## Global Constraints

- Zero new dependencies (dev-dependencies included).
- All `ByteReader` reads are bounds-checked and return `Result` — corrupt input must never panic.
- File format: magic `KRHT`, version byte `1`, little-endian, length-prefixed sequences.
- TDD: every task writes its failing test first and watches it fail.
- `cargo fmt` before every commit. `git add` files individually (never `git add .`).
- All existing tests (29) must stay green after every task.

---

### Task 1: ByteWriter / ByteReader primitives

**Files:**
- Create: `src/snapshot.rs`
- Modify: `src/lib.rs` (add `pub mod snapshot;` after `pub mod runtime;`)

**Interfaces:**
- Produces: `snapshot::ByteWriter` — `new()`, `u8(&mut self, v: u8)`, `u16(&mut self, v: u16)`, `u32(&mut self, v: u32)`, `u64(&mut self, v: u64)`, `i32(&mut self, v: i32)`, `f64(&mut self, v: f64)`, `bool_(&mut self, v: bool)`, `str_(&mut self, s: &str)` (u32 length + UTF-8 bytes), `finish(self) -> Vec<u8>`.
- Produces: `snapshot::ByteReader` — `new(&[u8])`, matching `u8()/u16()/u32()/u64()/i32()/f64()/bool_()/str_()` methods all returning `Result<T>`, plus `is_at_end(&self) -> bool`. Errors are `JSError::Message(String)` (Task 2 adds the variant; use `JSError::InternalError("snapshot: truncated input")` until then — Task 2 swaps it).

- [ ] **Step 1: Write the failing tests** (in `src/snapshot.rs` under `#[cfg(test)]`)

```rust
//! Snapshot serialization: whole-runtime durable execution.
//!
//! Format: magic "KRHT", version u8, little-endian, length-prefixed.

use crate::error::{JSError, Result};

pub struct ByteWriter {
    buf: Vec<u8>,
}

pub struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
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
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib snapshot 2>&1 | tail -5`
Expected: compile error (methods not defined). That is the RED for API-shape tests.

- [ ] **Step 3: Implement**

```rust
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
    JSError::InternalError("snapshot: truncated input")
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
            .map_err(|_| JSError::InternalError("snapshot: invalid UTF-8"))
    }

    pub fn is_at_end(&self) -> bool {
        self.pos == self.buf.len()
    }
}
```

Add `pub mod snapshot;` to `src/lib.rs` after `pub mod runtime;`.

- [ ] **Step 4: Verify green**

Run: `cargo test 2>&1 | grep "test result"`
Expected: all suites pass, lib tests now include the 3 new ones.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/snapshot.rs src/lib.rs
git commit -m "add snapshot byte writer/reader primitives"
```

---

### Task 2: `JSError::Message(String)` owned error variant

**Files:**
- Modify: `src/error.rs`
- Modify: `src/snapshot.rs` (swap `truncated()`/UTF-8 errors to the new variant)

**Interfaces:**
- Produces: `JSError::Message(String)` — the only owned-string error variant; used for all snapshot errors and for restoring failed fibers' errors.

- [ ] **Step 1: Failing test** (in `src/error.rs` `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_variant_displays_its_string() {
        let e = JSError::Message("snapshot: bad magic".to_string());
        assert_eq!(e.to_string(), "snapshot: bad magic");
    }
}
```

- [ ] **Step 2: Verify failure** — `cargo test --lib message_variant` → compile error (no such variant).

- [ ] **Step 3: Implement** — add to the enum and Display:

```rust
    // in enum JSError:
    /// Owned-message error: snapshot failures and restored fiber errors.
    Message(String),

    // in impl fmt::Display, before the closing brace of the match:
    JSError::Message(msg) => write!(f, "{}", msg),
```

In `src/snapshot.rs`, change `truncated()` to `JSError::Message("snapshot: truncated input".to_string())` and the UTF-8 error to `JSError::Message("snapshot: invalid UTF-8".to_string())`.

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/error.rs src/snapshot.rs
git commit -m "add owned JSError::Message variant for snapshot errors"
```

---

### Task 3: Arena and wrapper reconstruction plumbing

**Files:**
- Modify: `src/arena.rs`
- Modify: `src/env.rs`, `src/cont.rs`, `src/handler.rs` (wrapper accessors/constructors)
- Modify: `src/runtime.rs` (field visibility)

**Interfaces:**
- Produces on `Arena<T>`: `slots(&self) -> &[T]`, `free_list(&self) -> &[u32]`, `from_parts(data: Vec<T>, free_list: Vec<u32>, allocations: u64) -> Arena<T>` (marks rebuilt as `vec![false; data.len()]`).
- Produces on `EnvArena`: `arena(&self) -> &Arena<Env>`, `from_arena(arena: Arena<Env>) -> EnvArena` (global = `ArenaId::new(0)`).
- Produces on `ContArena`: `arena(&self) -> &Arena<Kont>`, `from_arena(arena: Arena<Kont>) -> ContArena` (halt = `ArenaId::new(0)`).
- Produces on `HandlerArena`: `arena(&self) -> &Arena<Handler>`, `from_arena(arena: Arena<Handler>) -> HandlerArena`.
- Changes in `src/runtime.rs`: fields `next_fiber_id`, `join_waiters`, `ast`, and `gc` become `pub(crate)` (`gc` is not serialized, but `read_runtime` constructs `Runtime` as a struct literal, so the field must be visible in-crate). `Fiber`, `FiberStatus`, `Runtime.fibers`, `ready_queue`, `current`, `interpreter` are already `pub`.

Rationale a task implementer needs: `EnvArena::new()`/`ContArena::new()` pre-allocate the global env / halt cont at slot 0; `from_arena` must NOT re-allocate — the deserialized data already contains slot 0.

- [ ] **Step 1: Failing test** (in `src/arena.rs` `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_parts_reconstructs_arena() {
        let mut a: Arena<u32> = Arena::new();
        let id0 = a.alloc(10);
        let _id1 = a.alloc(20);
        a.mark(id0);
        a.sweep(); // frees slot 1

        let data = a.slots().to_vec();
        let free = a.free_list().to_vec();
        let allocs = a.allocations();

        let b: Arena<u32> = Arena::from_parts(data, free, allocs);
        assert_eq!(b.get(ArenaId::new(0)), Some(&10));
        assert_eq!(b.len(), a.len());
        assert_eq!(b.free_list(), a.free_list());
        assert_eq!(b.allocations(), allocs);
        // freed slot is reused first, exactly like the original
        let mut b = b;
        let reused = b.alloc(99);
        assert_eq!(reused.index(), 1);
    }
}
```

- [ ] **Step 2: Verify failure** — `cargo test --lib from_parts` → compile error.

- [ ] **Step 3: Implement**

```rust
// in impl<T> Arena<T>:
    pub fn slots(&self) -> &[T] {
        &self.data
    }

    pub fn free_list(&self) -> &[u32] {
        &self.free_list
    }

    /// Rebuild an arena from serialized parts. Marks are transient GC
    /// state and start cleared.
    pub fn from_parts(data: Vec<T>, free_list: Vec<u32>, allocations: u64) -> Self {
        let marks = vec![false; data.len()];
        Self {
            data,
            free_list,
            marks,
            allocations,
        }
    }
```

Wrappers (same shape in all three files):

```rust
// src/env.rs, impl EnvArena:
    pub fn arena(&self) -> &Arena<Env> {
        &self.arena
    }

    /// Rebuild from a deserialized arena; slot 0 is the global env.
    pub fn from_arena(arena: Arena<Env>) -> Self {
        Self {
            arena,
            global: ArenaId::new(0),
        }
    }

// src/cont.rs, impl ContArena:
    pub fn arena(&self) -> &Arena<Kont> {
        &self.arena
    }

    /// Rebuild from a deserialized arena; slot 0 is the halt continuation.
    pub fn from_arena(arena: Arena<Kont>) -> Self {
        Self {
            arena,
            halt: ArenaId::new(0),
        }
    }

// src/handler.rs, impl HandlerArena:
    pub fn arena(&self) -> &Arena<Handler> {
        &self.arena
    }

    pub fn from_arena(arena: Arena<Handler>) -> Self {
        Self { arena }
    }
```

In `src/runtime.rs` change:

```rust
    next_fiber_id: u32,
    join_waiters: HashMap<FiberId, Vec<FiberId>>,
```
to
```rust
    pub(crate) next_fiber_id: u32,
    pub(crate) join_waiters: HashMap<FiberId, Vec<FiberId>>,
```
and `ast: AstArena,` to `pub(crate) ast: AstArena,`, and `gc: GC,` to `pub(crate) gc: GC,`.

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/arena.rs src/env.rs src/cont.rs src/handler.rs src/runtime.rs
git commit -m "add arena reconstruction plumbing for snapshots"
```

---

### Task 4: Leaf codecs — values, heap objects, envs, fibers

**Files:**
- Modify: `src/snapshot.rs`

**Interfaces:**
- Consumes: `ByteWriter`/`ByteReader` (Task 1), `JSError::Message` (Task 2).
- Produces (all private to `snapshot.rs` except noted): `write_value/read_value` (`JSValue`), `write_object/read_object` (`Object`), `write_env/read_env` (`Env`), `write_handler/read_handler` (`Handler`), `write_control/read_control` (`Control`), `write_fiber/read_fiber` (`Fiber`), `write_strings/read_strings` (`StringPool`).

Encoding rules (exact, used by every codec):
- Ids (`StrId`, `ObjId`, `FiberId`, `ExprId`, `StmtId`, `PatternId`): raw `u32` payload. `ArenaId<T>` (`EnvId`, `ContId`, `HandlerId`): `u32` via `.index() as u32`, decode `ArenaId::new(v)`.
- `Option<X>`: `bool_` presence flag, then X if present.
- Sequences: `u32` count, then elements.
- `HashMap`: `u32` count, then (key, value) pairs in iteration order (order irrelevant — rebuilt by insertion).

`JSValue` tags: 0 Undefined, 1 Null, 2 Bool(bool_), 3 Int(i32), 4 Float(f64), 5 String(u32), 6 Object(u32), 7 Function(u32), 8 Array(u32), 9 Handler(u32), 10 Continuation(u32 cont, u32 env).

`ObjectKind` tags: 0 Ordinary, 1 Array(elements: seq of value), 2 Function(FunctionData), 3 BoundFunction(target u32, this_arg value, bound_args seq), 4 NativeFunction(u8 tag). `NativeFn` tags 0..=13 in declaration order (MathFloor=0 … MathSign=13); decode via explicit `match`, unknown tag → `Err`.

`FunctionData` fields in order: `params_start` u32, `params_count` u16, `body` u32, `expr_body` Option<u32>, `env` u32, `name` Option<u32>.

`Property` fields in order: value, `writable` bool, `enumerable` bool, `configurable` bool.
`Object` fields: properties map (StrId → Property), `prototype` Option<u32>, kind.
`Env`: bindings as seq of (StrId, value) via `iter_bindings()`, parent Option<u32>; decode with `Env::with_binding_slice(&pairs, parent)`.
`Handler` fields: `clauses_start` u32, `clauses_count` u16, `return_param` u32, `return_body` u32, `env` u32.
`Control` tags: 0 Expr(u32), 1 Stmt(u32), 2 Value(value), 3 Returning(value), 4 Halted(value), 5 Suspend(effect u32, args seq).
`FiberStatus` tags: 0 Ready, 1 Running, 2 Blocked(effect u32, args seq), 3 Completed(value), 4 Failed(err.to_string() as str_; decode `JSError::Message(s)`).
`Fiber` fields: id u32, control, cont u32, env u32, status.
`StringPool`: seq of `str_` from `.get(StrId(i))` for `i in 0..len()`; decode into a fresh pool via repeated `intern` **in index order** (interning in order reproduces identical ids because the pool is append-only).

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn value_and_object_round_trip() {
        use crate::object::{Object, Property};
        use crate::string_pool::StrId;
        use crate::value::JSValue;

        let mut w = ByteWriter::new();
        write_value(&mut w, JSValue::Continuation(crate::ContId::new(3), crate::EnvId::new(9)));
        write_value(&mut w, JSValue::Float(6.25));

        let mut obj = Object::new();
        obj.properties.insert(StrId(4), Property::readonly(JSValue::Int(7)));
        obj.prototype = Some(crate::ObjId(11));
        write_object(&mut w, &obj);

        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert_eq!(read_value(&mut r).unwrap(), JSValue::Continuation(crate::ContId::new(3), crate::EnvId::new(9)));
        assert_eq!(read_value(&mut r).unwrap(), JSValue::Float(6.25));
        let obj2 = read_object(&mut r).unwrap();
        assert_eq!(obj2.get(StrId(4)), Some(JSValue::Int(7)));
        assert_eq!(obj2.prototype, Some(crate::ObjId(11)));
        assert!(!obj2.properties[&StrId(4)].writable);
        assert!(r.is_at_end());
    }
```

- [ ] **Step 2: Verify failure** — `cargo test --lib value_and_object` → compile error.

- [ ] **Step 3: Implement all leaf codecs.** Full code for the two shape-classes; every other codec is the same projection of the field tables above (write fields in table order; read them back in the same order; enums = tag byte + `match`, unknown tag → `Err(JSError::Message(format!("snapshot: bad <type> tag {tag}")))`).

```rust
use crate::value::{JSValue, ObjId};

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
        5 => JSValue::String(crate::StrId(r.u32()?)),
        6 => JSValue::Object(ObjId(r.u32()?)),
        7 => JSValue::Function(ObjId(r.u32()?)),
        8 => JSValue::Array(ObjId(r.u32()?)),
        9 => JSValue::Handler(crate::HandlerId::new(r.u32()?)),
        10 => {
            let k = crate::ContId::new(r.u32()?);
            let e = crate::EnvId::new(r.u32()?);
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
```

Struct-shaped example (`Object`); `Env`, `Handler`, `Fiber`, `Property`, `FunctionData`, `BoundFunctionData`, `Control`, `FiberStatus`, `StringPool` follow the field tables identically:

```rust
use crate::object::{ArrayData, BoundFunctionData, FunctionData, NativeFn, Object, ObjectKind, Property};

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
```

`read_object` mirrors it; the `NativeFn` decode is an explicit match over tags 0..=13 (`0 => NativeFn::MathFloor, … 13 => NativeFn::MathSign`), unknown → `Err`.

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/snapshot.rs
git commit -m "add leaf codecs for values, objects, envs, fibers"
```

---

### Task 5: Kont and AST codecs

**Files:**
- Modify: `src/snapshot.rs`

**Interfaces:**
- Consumes: everything from Task 4.
- Produces: `write_kont/read_kont` (`Kont`), `write_ast/read_ast` (`AstArena`), `write_expr/read_expr`, `write_stmt/read_stmt`, `write_pattern/read_pattern`.

**Kont tag table** (tag, then fields in this exact order; field types per encoding rules of Task 4):

| tag | variant | fields |
|-----|---------|--------|
| 0 | Halt | — |
| 1 | UnaryK | op u8, k |
| 2 | BinaryLeftK | op u8, right, env, k |
| 3 | BinaryRightK | op u8, left value, k |
| 4 | AndK | right, env, k |
| 5 | OrK | right, env, k |
| 6 | NullishK | right, env, k |
| 7 | MemberK | property, k |
| 8 | IndexObjK | index, env, k |
| 9 | IndexKeyK | obj value, k |
| 10 | AssignVarK | name, env, k |
| 11 | UpdateVarK | name, is_pre bool, is_inc bool, env, k |
| 12 | AssignMemberObjK | property, value(ExprId), env, k |
| 13 | AssignMemberValK | obj value, property, k |
| 14 | AssignIndexObjK | index, value(ExprId), env, k |
| 15 | AssignIndexKeyK | obj value, value(ExprId), env, k |
| 16 | AssignIndexValK | obj value, key value, k |
| 17 | IfK | consequent, alternate, env, k |
| 18 | CondK | consequent, alternate, env, k |
| 19 | WhileK | test, body, env, k |
| 20 | WhileBodyK | test, body, env, k |
| 21 | ForTestK | test, update, body, env, k |
| 22 | ForTestResultK | test, update, body, env, k |
| 23 | ForBodyK | test, update, body, env, k |
| 24 | ForUpdateK | test, update, body, env, k |
| 25 | CalleeK | args_start u32, args_count u16, env, k |
| 26 | ArgsK | callee value, done seq, args_start u32, args_idx u16, args_count u16, env, k |
| 27 | ReturnK | env, k |
| 28 | ReturnExprK | k |
| 29 | ArrayK | done seq, elems_start u32, elems_idx u16, elems_count u16, env, k |
| 30 | ObjectK | done seq of (StrId, value), props_start u32, props_idx u16, props_count u16, env, k |
| 31 | LetK | name, env, k |
| 32 | ConstK | name, env, k |
| 33 | VarK | name, env, k |
| 34 | SeqK | stmts_start u32, stmts_idx u32, stmts_count u32, env, k |
| 35 | ExprStmtK | k |
| 36 | BlockK | stmts_start u32, stmts_idx u32, stmts_count u32, final_expr, env, k |
| 37 | MatchK | arms_start u32, arms_idx u16, arms_count u16, env, k |
| 38 | HandlerK | clauses_start u32, clauses_count u16, env, return_body, return_param, k |
| 39 | PerformArgsK | effect, done seq, args_start u32, args_idx u16, args_count u16, env, k |
| 40 | HandleWithK | body, env, k |

`BinaryOp` and `UnaryOp` are `#[repr(u8)]`: encode `op as u8`; decode with an explicit match over declaration order (`BinaryOp`: Add=0 … NullishCoalesce=20; `UnaryOp`: Neg=0 … PostDec=8), unknown → `Err`.

**Expr tag table** (`src/ast.rs` declaration order): 0 Empty, 1 Undefined, 2 Null, 3 Bool(bool), 4 Int(i32), 5 Float(u32), 6 String(StrId), 7 Identifier(StrId), 8 Binary(op u8, left, right), 9 Unary(op u8, operand), 10 Call(callee, args_start u32, args_count u16), 11 Member(object, property), 12 Index(object, index), 13 Assign(target, value), 14 Conditional(test, consequent, alternate), 15 Array(elems_start u32, elems_count u16), 16 Object(props_start u32, props_count u16), 17 Function(name, params_start u32, params_count u16, body), 18 Handler(clauses_start u32, clauses_count u16, return_param, return_body), 19 Arrow(params_start u32, params_count u16, body ExprId, is_block bool), 20 This, 21 Block(stmts_start u32, stmts_count u32, final_expr), 22 Match(scrutinee, arms_start u32, arms_count u16), 23 Perform(effect, args_start u32, args_count u16), 24 Handle(body, clauses_start u32, clauses_count u16, return_param, return_body), 25 HandleWith(body, handler).

**Stmt tag table:** 0 Empty, 1 Expr(ExprId), 2 Let(name, init), 3 Const(name, init), 4 Var(name, init), 5 If(test, consequent, alternate), 6 While(test, body), 7 For(init StmtId, test, update, body), 8 Block(stmts_start u32, stmts_count u32), 9 Declarations(stmts_start u32, stmts_count u32), 10 Return(ExprId), 11 Break, 12 Continue, 13 Function(name, params_start u32, params_count u16, body).

**Pattern tag table:** 0 Wildcard, 1 Literal(ExprId), 2 Var(StrId), 3 Array(elems_start u32, elems_count u16), 4 Object(fields_start u32, fields_count u16).

**AstArena** serializes its 12 `Vec`s in declaration order (`exprs, stmts, patterns, expr_lists, stmt_lists, param_lists, prop_lists, pattern_lists, pattern_fields, arms, effect_clauses, floats`) each as u32 count + elements, then `root_start` u32, `root_count` u32. Flat-struct elements: `PropEntry`(key, value ExprId), `PatternField`(key, pattern), `MatchArm`(pattern, guard, body), `EffectClause`(effect, params_start u32, params_count u16, body).

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn ast_round_trips_through_bytes() {
        use crate::parser::Parser;

        let parser = Parser::new(
            "function f(x) { return match (x) { {ok} => ok, _ => 0 } } f({ok: 5})",
        )
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
```

- [ ] **Step 2: Verify failure** — `cargo test --lib "round_trip"` → compile error.

- [ ] **Step 3: Implement** the tag tables exactly as specified. Each variant: write tag byte, write fields in table order; read is the mirror `match` with unknown tag → `Err(JSError::Message(...))`. This is ~400 lines of mechanical code; the tables above are normative.

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/snapshot.rs
git commit -m "add Kont and AST snapshot codecs"
```

---

### Task 6: `write_runtime`/`read_runtime`, `Runtime::from_snapshot`, scheduler split

**Files:**
- Modify: `src/snapshot.rs`
- Modify: `src/runtime.rs`

**Interfaces:**
- Produces: `snapshot::write_runtime(rt: &Runtime, ready_override: &VecDeque<FiberId>) -> Vec<u8>` — serializes magic `KRHT`, version `1u8`, then sections: strings, ast, objects, envs, conts, handlers, globals (map StrId→value), fibers (seq), scheduler (`ready_override` as seq of u32, `next_fiber_id` u32, join_waiters map u32→seq of u32). `current` is NOT serialized — restore always starts with `current = None`. The `ready_override` parameter lets the Snapshot! handler put the performing fiber at the front without mutating the live queue.
- Produces: `snapshot::read_runtime(bytes: &[u8]) -> Result<Runtime>` — validates magic/version, reconstructs everything; `gc` is `GC::new()` re-primed via a new `GC::reprime_baseline(&mut self, interpreter: &CEKH)` (sets `alloc_snapshot = interpreter.total_allocations()`, add it to `src/gc.rs` in this task) so restore doesn't trigger a spurious immediate collection.
- Produces: `Runtime::from_snapshot(bytes: &[u8]) -> Result<Runtime>` (delegates to `read_runtime`) and `Runtime::run_resumed(&mut self) -> Result<JSValue>`.
- Refactor: extract the scheduler loop of `Runtime::run` (everything from `let mut main_result` through the end of `loop { ... }`) into `fn run_scheduler(&mut self, ast: &AstArena) -> Result<JSValue>`. `run()` keeps its reset + root-fiber setup and ends with `self.run_scheduler(ast)`. `run_resumed()` is:

```rust
    /// Continue a runtime restored from a snapshot: enter the scheduler
    /// without the per-run reset.
    pub fn run_resumed(&mut self) -> Result<JSValue> {
        let ast = std::mem::take(&mut self.ast);
        let result = self.run_scheduler(&ast);
        self.ast = ast;
        result
    }
```

Arena sections use Task 3 plumbing: write `slots()` (u32 count + per-element codec), `free_list()` (u32 count + u32s), `allocations()` u64; read via `Arena::from_parts` then wrapper `from_arena`. `CEKH` is rebuilt as a struct literal (all fields are `pub`): `CEKH { control, env, cont, envs, conts, handlers, objects, strings, globals }`. Serialize `interpreter.control/env/cont` (they are stale-but-harmless; `select_next_fiber` overwrites them on resume).

- [ ] **Step 1: Failing test**

```rust
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
```

- [ ] **Step 2: Verify failure** — `cargo test --lib runtime_round` → compile error.

- [ ] **Step 3: Implement** `write_runtime`/`read_runtime` (sections in the interface order), `from_snapshot`, and the `run`/`run_scheduler` split. Magic check:

```rust
pub const MAGIC: &[u8; 4] = b"KRHT";
pub const VERSION: u8 = 1;
```

`read_runtime` starts:

```rust
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
    // ... sections ...
}
```

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass (including all 29 pre-existing tests — the `run` refactor must not change behavior).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/snapshot.rs src/runtime.rs
git commit -m "add whole-runtime snapshot codec and scheduler split"
```

---

### Task 7: The `Snapshot!` effect

**Files:**
- Modify: `src/runtime.rs`
- Test: `tests/integration.rs`

**Interfaces:**
- Consumes: `snapshot::write_runtime`, `Runtime::from_snapshot`, `run_resumed` (Task 6).
- Produces: runtime effect `Snapshot` — `perform Snapshot!(path)` writes the file, evaluates to `"saved"`; a runtime restored from the file evaluates the same effect to `"restored"`.

- [ ] **Step 1: Failing tests** (in `tests/integration.rs`)

```rust
#[test]
fn test_snapshot_saved_and_restored() {
    let dir = std::env::temp_dir().join("kryhta_snap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("basic.snap");
    let path_str = path.to_str().unwrap();

    let source = format!(
        r#"
        var log = [];
        var state = {{ count: 41 }};
        let outcome = perform Snapshot!("{path_str}");
        state.count = state.count + 1;
        outcome
        "#
    );

    let mut runtime = Runtime::new();
    let first = eval(&mut runtime, &source).unwrap();
    assert_eq!(runtime.interpreter.to_string(first), "saved");

    let bytes = std::fs::read(&path).unwrap();
    let mut restored = Runtime::from_snapshot(&bytes).unwrap();
    let second = restored.run_resumed().unwrap();
    assert_eq!(restored.interpreter.to_string(second), "restored");
    // the increment after the snapshot ran again in the restored world
    assert_eq!(
        restored.eval("state.count").unwrap(),
        JSValue::Int(42),
        "restored run continues from the snapshot point"
    );
}

#[test]
fn test_snapshot_requires_string_path() {
    let mut runtime = Runtime::new();
    assert!(eval(&mut runtime, "perform Snapshot!(42)").is_err());
}
```

- [ ] **Step 2: Verify failure**

Run: `cargo test --test integration test_snapshot 2>&1 | tail -5`
Expected: FAIL with `Unknown effect`.

- [ ] **Step 3: Implement** in `src/runtime.rs`:

```rust
        // in handle_effect's match:
            "Snapshot" => self.handle_snapshot(args),
```

```rust
    fn handle_snapshot(&mut self, args: Vec<JSValue>) -> Result<EffectResult> {
        let JSValue::String(path_id) = args.first().copied().unwrap_or(JSValue::Undefined) else {
            return Err(JSError::type_error("Snapshot: expected file path string"));
        };
        let path = self
            .interpreter
            .strings
            .get(path_id)
            .unwrap_or("")
            .to_string();

        let restored = self.interpreter.strings.intern("restored");
        let saved = self.interpreter.strings.intern("saved");
        let fiber_id = self.current.expect("No current fiber for Snapshot");

        // The file must contain a machine that wakes up seeing "restored".
        self.interpreter.control = Control::Value(JSValue::String(restored));
        self.save_current_fiber_state();

        let mut ready = self.ready_queue.clone();
        ready.push_front(fiber_id);
        let bytes = crate::snapshot::write_runtime(self, &ready);
        std::fs::write(&path, &bytes)
            .map_err(|e| JSError::Message(format!("Snapshot: cannot write {path}: {e}")))?;

        // The live run continues, seeing "saved".
        self.interpreter.control = Control::Value(JSValue::String(saved));
        Ok(EffectResult::Resume)
    }
```

Note for the implementer: `save_current_fiber_state` sets the live fiber's status to `Ready`; that is already the behavior `handle_join` relies on and is harmless here (the fiber is not in the live ready queue).

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/runtime.rs tests/integration.rs
git commit -m "add Snapshot! effect for durable execution"
```

---

### Task 8: Load-bearing round-trip test with fibers

**Files:**
- Create: `tests/fixtures/test_snapshot_fibers.js`
- Test: `tests/integration.rs`

**Interfaces:** consumes everything; produces no new API. This is the equivalence guarantee: a restored run finishes with exactly the value an uninterrupted run produces, across closures, mutated state, a blocked fiber, and a completed-unjoined fiber.

- [ ] **Step 1: Write fixture and failing test**

`tests/fixtures/test_snapshot_fibers.js`:

```javascript
// Snapshot mid-run with fibers in every state, then verify the world
// is intact: closure state, heap objects, blocked and unjoined fibers.
var acc = { total: 0 };

function mkAdder(n) {
    return function() { return n + acc.total };
}
var add10 = mkAdder(10);

// completed-but-unjoined fiber: its result sits parked in its status
var parked = perform Fork!(function() { return { answer: 32 } });
var quick = perform Fork!(function() { return 0 });
perform Join!(quick);

acc.total = 10;

perform Snapshot!("__SNAP_PATH__");

// everything below runs in BOTH worlds and must agree
let joined = perform Join!(parked);
let fromFiber = match (joined) { {ok} => ok.answer, {err} => -1 };
add10() + fromFiber   // 10 + 10 + 32 = 52
```

Test in `tests/integration.rs`:

```rust
#[test]
fn test_snapshot_round_trip_equivalence() {
    let dir = std::env::temp_dir().join("kryhta_snap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fibers.snap");
    let source = include_str!("fixtures/test_snapshot_fibers.js")
        .replace("__SNAP_PATH__", path.to_str().unwrap());

    let mut original = Runtime::new();
    let uninterrupted = eval(&mut original, &source).unwrap();
    assert_eq!(uninterrupted, JSValue::Int(52));

    let bytes = std::fs::read(&path).unwrap();
    let mut restored = Runtime::from_snapshot(&bytes).unwrap();
    let resumed = restored.run_resumed().unwrap();
    assert_eq!(resumed, JSValue::Int(52), "restored run must produce the identical result");
}
```

- [ ] **Step 2: Verify it fails or passes honestly**

Run: `cargo test --test integration test_snapshot_round_trip 2>&1 | tail -5`
Expected: PASS if Tasks 4–7 are correct — this test exists to catch codec bugs, so if it FAILS, the failure output names the broken codec path; fix the codec, not the test. If it passes first try, temporarily corrupt one codec locally (e.g. swap two Kont tags) and confirm the test catches it, then revert — that is the "watch it fail" for an equivalence test.

- [ ] **Step 3: Verify full suite** — `cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add tests/fixtures/test_snapshot_fibers.js tests/integration.rs
git commit -m "add snapshot round-trip equivalence test with fibers"
```

---

### Task 9: CLI `--resume` and end-to-end test

**Files:**
- Modify: `src/bin/kryhta.rs`
- Test: `tests/cli.rs` (create)

**Interfaces:**
- Produces: `kryhta --resume <file.snap>` — loads the snapshot, runs to completion, prints the final value exactly like a normal run.

- [ ] **Step 1: Failing test** (`tests/cli.rs`)

```rust
//! End-to-end CLI test: snapshot in one process, resume in another.

use std::process::Command;

#[test]
fn resume_continues_a_snapshotted_run() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let snap = dir.join("cli.snap");
    let script = dir.join("job.js");
    std::fs::write(
        &script,
        format!(
            r#"
            var state = {{ count: 41 }};
            let outcome = perform Snapshot!("{}");
            state.count = state.count + 1;
            perform Print!(outcome, state.count);
            state.count
            "#,
            snap.to_str().unwrap()
        ),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");

    let run1 = Command::new(bin).arg(&script).output().unwrap();
    assert!(run1.status.success());
    assert!(String::from_utf8_lossy(&run1.stdout).contains("saved 42"));

    let run2 = Command::new(bin).arg("--resume").arg(&snap).output().unwrap();
    assert!(run2.status.success());
    assert!(String::from_utf8_lossy(&run2.stdout).contains("restored 42"));
}

#[test]
fn resume_rejects_garbage_files() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("garbage.snap");
    std::fs::write(&bad, b"not a snapshot").unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");
    let out = Command::new(bin).arg("--resume").arg(&bad).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("snapshot"));
}
```

- [ ] **Step 2: Verify failure** — `cargo test --test cli 2>&1 | tail -5` → FAIL (`--resume` treated as a script path).

- [ ] **Step 3: Implement** in `src/bin/kryhta.rs` `main`:

```rust
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("--resume") => {
            let path = args.get(2).ok_or(JSError::InternalError(
                "Usage: kryhta --resume <file.snap>",
            ))?;
            resume(path)
        }
        Some(path) => {
            let source = std::fs::read_to_string(path)
                .map_err(|_| JSError::InternalError("Failed to read file"))?;
            run(&source)
        }
        None => repl(),
    }
}

fn resume(path: &str) -> Result<()> {
    let attempt = std::fs::read(path)
        .map_err(|e| JSError::Message(format!("snapshot: cannot read {path}: {e}")))
        .and_then(|bytes| Runtime::from_snapshot(&bytes));

    let mut runtime = match attempt {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    match runtime.run_resumed() {
        Ok(result) => {
            if !matches!(result, JSValue::Undefined) {
                print_value(&result, &runtime);
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
```

(`run` and `repl` keep their existing bodies; `main`'s old `if args.len() > 1` dispatch is replaced by the match above. Every error path prints to stderr and exits nonzero, which is what the CLI test asserts.)

- [ ] **Step 4: Verify green** — `cargo test 2>&1 | grep "test result"` → all suites (lib, integration, cli) pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/bin/kryhta.rs tests/cli.rs
git commit -m "add --resume CLI for durable execution"
```

---

### Task 10: Flagship example and README

**Files:**
- Create: `examples/durable_counter.js`
- Modify: `README.md`

- [ ] **Step 1: Write the example**

`examples/durable_counter.js`:

```javascript
// Durable counter: run it, kill it, resume it — it continues.
//
//   cargo run --bin kryhta examples/durable_counter.js
//   cargo run --bin kryhta -- --resume counter.snap
var state = { i: 0 };

while (state.i < 5) {
    state.i = state.i + 1;
    let outcome = perform Snapshot!("counter.snap");
    perform Print!(outcome, "i =", state.i);
}
state.i
```

- [ ] **Step 2: Verify it works end-to-end by hand**

```bash
cargo run --bin kryhta examples/durable_counter.js
cargo run --bin kryhta -- --resume counter.snap
rm counter.snap
```
Expected: first run prints `saved i = 1` … `saved i = 5`; resume prints `restored i = 5` (last checkpoint) and finishes.

- [ ] **Step 3: Add a Durable Execution section to README.md** after the Errors section:

```markdown
## Durable execution

A running program — including every suspended fiber — can checkpoint itself
to a self-contained file and be resumed later, even after the process dies:

​```javascript
match (perform Snapshot!("job.snap")) {
    "saved"    => perform Print!("checkpoint written"),
    "restored" => perform Print!("welcome back")
}
​```

​```bash
kryhta job.js              # runs, writes job.snap at the Snapshot! call
kryhta --resume job.snap   # wakes up inside that call, seeing "restored"
​```

The snapshot contains the whole machine (fibers, heap, continuations, AST),
so resuming does not need the original source file.
```

(Remove the zero-width separators when writing the actual file — nested code fences shown escaped here.)

- [ ] **Step 4: Full suite + fmt** — `cargo fmt && cargo test 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
git add examples/durable_counter.js README.md
git commit -m "add durable counter example and README section"
```
