use std::collections::{HashMap, VecDeque};

use crate::Kont;
use crate::ast::AstArena;
use crate::cekh::{CEKH, Control};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::error::{JSError, Result};
use crate::gc::GC;
use crate::object::{Object, ObjectKind};
use crate::parser::Parser;
use crate::string_pool::{StrId, StringPool};
use crate::value::{JSValue, ObjId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FiberId(pub u32);

#[derive(Debug, Clone)]
pub enum FiberStatus {
    Ready,
    Running,
    Blocked { effect: StrId, args: Vec<JSValue> },
    BlockedOnHost { effect: StrId, args: Vec<JSValue> },
    Completed(JSValue),
    Failed(JSError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BuiltinEffect {
    Print,
    Fork,
    Join,
    Gc,
    Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectKind {
    Builtin(BuiltinEffect),
    Host,
}

#[derive(Debug, Clone)]
pub struct Fiber {
    pub id: FiberId,

    pub control: Control,
    pub cont: ContId,
    pub env: EnvId,

    pub status: FiberStatus,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Done(JSValue),
    Suspended { effect: StrId, args: Vec<JSValue> },
}

#[derive(Debug, Clone)]
pub enum EffectResult {
    Resume,
    Block,
    Spawned(FiberId),
}

pub struct Runtime {
    pub interpreter: CEKH,
    pub fibers: Vec<Fiber>,
    pub ready_queue: VecDeque<FiberId>,
    pub current: Option<FiberId>,
    pub(crate) next_fiber_id: u32,
    pub(crate) join_waiters: HashMap<FiberId, Vec<FiberId>>,
    pub(crate) gc: GC,
    /// Session AST — grows across `eval` calls so function bodies from
    /// earlier evals stay resolvable.
    pub(crate) ast: AstArena,
    /// Which effect names are handleable by the runtime itself (builtins)
    /// or granted to cross the host boundary. Anything else faults at the
    /// perform site unless an in-language handler catches it first.
    pub effects: HashMap<StrId, EffectKind>,
}

impl Runtime {
    /// The builtin effect names every runtime understands out of the box.
    /// Also used by `snapshot::read_runtime` to re-seed a restored runtime's
    /// registry (full grant persistence is a later task).
    pub(crate) fn builtin_effects(strings: &mut StringPool) -> HashMap<StrId, EffectKind> {
        use BuiltinEffect::*;
        [
            ("Print", Print),
            ("Fork", Fork),
            ("Join", Join),
            ("Gc", Gc),
            ("Snapshot", Snapshot),
        ]
        .into_iter()
        .map(|(name, b)| (strings.intern(name), EffectKind::Builtin(b)))
        .collect()
    }

    pub fn new() -> Self {
        let mut interpreter = CEKH::new();
        let effects = Self::builtin_effects(&mut interpreter.strings);
        Self {
            interpreter,
            fibers: Vec::new(),
            ready_queue: VecDeque::new(),
            current: None,
            next_fiber_id: 0,
            join_waiters: HashMap::new(),
            gc: GC::new(),
            ast: AstArena::new(),
            effects,
        }
    }

    /// Grant the script permission to perform `name` as a host effect.
    /// Ungranted, unhandled effects fail at the perform site.
    pub fn grant(&mut self, name: &str) {
        let id = self.interpreter.strings.intern(name);
        self.effects.insert(id, EffectKind::Host);
    }

    /// Reconstruct a runtime from bytes produced by `snapshot::write_runtime`.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Runtime> {
        crate::snapshot::read_runtime(bytes)
    }

    /// Parse and run source against this runtime's persistent session state.
    /// Parses against clones and commits only on success, so a syntax error
    /// leaves the session (string pool, AST, globals) intact.
    pub fn eval(&mut self, source: &str) -> Result<JSValue> {
        let parser =
            Parser::with_state(source, self.interpreter.strings.clone(), self.ast.clone())?;
        let (arena, strings) = parser.parse_program()?;
        self.interpreter.strings = strings;
        self.ast = arena;

        let ast = std::mem::take(&mut self.ast);
        let result = self.run(&ast);
        self.ast = ast;
        result
    }

    pub fn run(&mut self, ast: &AstArena) -> Result<JSValue> {
        self.interpreter.fresh_exec_setup(ast)?;

        // Each run is a fresh top-level execution: fibers from a previous
        // run are dead (completed, failed, or abandoned blocked waiters).
        self.fibers.clear();
        self.ready_queue.clear();
        self.join_waiters.clear();
        self.current = None;

        let root_fiber = Fiber {
            id: FiberId(0),
            control: self.interpreter.control.clone(),
            cont: self.interpreter.cont,
            env: self.interpreter.env,
            status: FiberStatus::Ready,
        };

        self.fibers.push(root_fiber);
        self.ready_queue.push_back(FiberId(0));
        self.next_fiber_id = 1;

        self.run_scheduler(ast)
    }

    /// Continue a runtime restored from a snapshot: enter the scheduler
    /// without the per-run reset.
    pub fn run_resumed(&mut self) -> Result<JSValue> {
        let ast = std::mem::take(&mut self.ast);
        let result = self.run_scheduler(&ast);
        self.ast = ast;
        result
    }

    fn run_scheduler(&mut self, ast: &AstArena) -> Result<JSValue> {
        let mut main_result = JSValue::Undefined;

        loop {
            if self.current.is_none() {
                if self.select_next_fiber().is_none() {
                    return Ok(main_result);
                }
            }

            match self.run_current_fiber(ast) {
                Ok(Outcome::Done(value)) => {
                    if self.current == Some(FiberId(0)) {
                        main_result = value;
                    }
                    self.complete_current_fiber(value);
                    self.current = None;
                }
                Ok(Outcome::Suspended { effect, args }) => {
                    match self.handle_effect(effect, args, ast)? {
                        EffectResult::Resume => {}
                        EffectResult::Block => {
                            self.current = None;
                        }
                        EffectResult::Spawned(id) => {
                            self.interpreter.control = Control::Value(JSValue::Int(id.0 as i32));
                        }
                    }
                }
                Err(err) => {
                    // The root fiber's failure is the program's failure;
                    // a child fiber's failure is contained and surfaces
                    // as a throw in whoever joins it.
                    if self.current == Some(FiberId(0)) {
                        return Err(err);
                    }
                    self.fail_current_fiber(err);
                    self.current = None;
                }
            }
        }
    }

    fn select_next_fiber(&mut self) -> Option<FiberId> {
        let fiber_id = self.ready_queue.pop_front()?;
        self.current = Some(fiber_id);

        let fiber = self.fibers.iter_mut().find(|f| f.id == fiber_id);

        let fiber = fiber.expect("Current fiber not found in fibers list");

        assert!(
            matches!(fiber.status, FiberStatus::Ready),
            "Selected fiber is not Ready"
        );

        self.interpreter.control = fiber.control.clone();
        self.interpreter.cont = fiber.cont;
        self.interpreter.env = fiber.env;
        fiber.status = FiberStatus::Running;

        Some(fiber_id)
    }

    fn run_current_fiber(&mut self, ast: &AstArena) -> Result<Outcome> {
        assert!(self.current.is_some(), "No current fiber to run");
        self.interpreter.run(ast)
    }

    fn complete_current_fiber(&mut self, value: JSValue) {
        assert!(self.current.is_some(), "No current fiber to complete");
        let fiber_id = self.current.unwrap();

        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber should be found");

        fiber.status = FiberStatus::Completed(value);

        if let Some(waiters) = self.join_waiters.remove(&fiber_id) {
            for waiter_id in waiters {
                let result = self.join_result("ok", value);
                self.unblock_fiber(waiter_id, result);
            }
            // Result delivered to every joiner — the fiber is consumed.
            self.remove_fiber(fiber_id);
        }
    }

    fn remove_fiber(&mut self, fiber_id: FiberId) {
        if let Some(pos) = self.fibers.iter().position(|f| f.id == fiber_id) {
            self.fibers.swap_remove(pos);
        }
    }

    fn spawn_fiber(&mut self, cont: ContId, env: EnvId, control: Control) -> FiberId {
        let id = self.get_next_fiber_id();

        let fiber = Fiber {
            id,
            control,
            cont,
            env,
            status: FiberStatus::Ready,
        };

        self.fibers.push(fiber);
        self.ready_queue.push_back(id);
        id
    }

    fn get_next_fiber_id(&mut self) -> FiberId {
        let id = FiberId(self.next_fiber_id);
        self.next_fiber_id += 1;
        id
    }

    fn handle_effect(
        &mut self,
        effect: StrId,
        args: Vec<JSValue>,
        ast: &AstArena,
    ) -> Result<EffectResult> {
        if self.gc.should_collect(&self.interpreter) {
            self.gc
                .collect(&mut self.interpreter, &self.fibers, &self.effects);
        }

        match self.effects.get(&effect).copied() {
            Some(EffectKind::Builtin(b)) => match b {
                BuiltinEffect::Print => self.handle_print(args),
                BuiltinEffect::Fork => self.handle_fork(args, ast),
                BuiltinEffect::Join => self.handle_join(args),
                BuiltinEffect::Gc => self.handle_gc(),
                BuiltinEffect::Snapshot => self.handle_snapshot(args, ast),
            },
            Some(EffectKind::Host) => self.handle_host_effect(effect, args),
            None => {
                let name = self.interpreter.strings.get(effect).unwrap_or("?");
                Err(JSError::Message(format!(
                    "effect '{name}' is not handled and not granted"
                )))
            }
        }
    }

    /// Move a granted effect from the language runtime to the host: no
    /// in-language handler caught it, so it blocks the performing fiber
    /// until a later task's hosted run loop resumes it with a host answer.
    fn handle_host_effect(&mut self, effect: StrId, args: Vec<JSValue>) -> Result<EffectResult> {
        // Validate now so an unconvertible argument faults the performing
        // fiber at the perform site, not later when the host reads the call.
        for arg in &args {
            crate::host::to_host_value(&self.interpreter, *arg)?;
        }

        let fiber_id = self.current.expect("No current fiber for host effect");
        self.save_current_fiber_state();
        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber not found");
        fiber.status = FiberStatus::BlockedOnHost { effect, args };
        Ok(EffectResult::Block)
    }

    fn handle_fork(&mut self, args: Vec<JSValue>, _ast: &AstArena) -> Result<EffectResult> {
        let JSValue::Function(k) = args.first().copied().unwrap_or(JSValue::Undefined) else {
            return Err(JSError::type_error("Fork: expected function"));
        };

        let fn_obj = self
            .interpreter
            .objects
            .get(k.into_arena_id())
            .ok_or(JSError::InternalError("Invalid function object"))?;

        let ObjectKind::Function(func_data) = &fn_obj.kind else {
            return Err(JSError::type_error("Fork: not a function"));
        };

        let func_data = func_data.clone();

        let control = if let Some(expr_body) = func_data.expr_body {
            Control::Expr(expr_body)
        } else {
            Control::Stmt(func_data.body)
        };

        let cont = self.interpreter.conts.alloc(Kont::Halt);
        let env = func_data.env;

        let id = self.spawn_fiber(cont, env, control);
        Ok(EffectResult::Spawned(id))
    }

    fn handle_join(&mut self, args: Vec<JSValue>) -> Result<EffectResult> {
        let JSValue::Int(id) = args.first().copied().unwrap_or(JSValue::Undefined) else {
            return Err(JSError::type_error("Join: expected fiber id"));
        };

        let target_id = FiberId(id as u32);
        let current_id = self.current.expect("No current fiber");

        let target = self
            .fibers
            .iter()
            .find(|f| f.id == target_id)
            .ok_or(JSError::runtime_error("Join: fiber not found"))?;

        match &target.status {
            FiberStatus::Completed(value) => {
                let value = *value;
                let result = self.join_result("ok", value);
                self.interpreter.control = Control::Value(result);
                self.remove_fiber(target_id);
                Ok(EffectResult::Resume)
            }
            FiberStatus::Failed(err) => {
                let err = err.clone();
                let err_val = self.error_value(&err);
                let result = self.join_result("err", err_val);
                self.interpreter.control = Control::Value(result);
                self.remove_fiber(target_id);
                Ok(EffectResult::Resume)
            }
            _ => {
                self.join_waiters
                    .entry(target_id)
                    .or_default()
                    .push(current_id);

                self.save_current_fiber_state();
                Ok(EffectResult::Block)
            }
        }
    }

    fn save_current_fiber_state(&mut self) {
        let fiber_id = self.current.expect("No current fiber");
        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber not found");

        fiber.control = self.interpreter.control.clone();
        fiber.cont = self.interpreter.cont;
        fiber.env = self.interpreter.env;
        fiber.status = FiberStatus::Ready;
    }

    pub fn gc_stats(&self) -> crate::gc::GCStats {
        self.gc.stats
    }

    pub fn fiber_count(&self) -> usize {
        self.fibers.len()
    }

    fn handle_gc(&mut self) -> Result<EffectResult> {
        self.gc
            .collect(&mut self.interpreter, &self.fibers, &self.effects);
        self.interpreter.control = Control::Value(JSValue::Undefined);
        Ok(EffectResult::Resume)
    }

    fn handle_snapshot(&mut self, args: Vec<JSValue>, ast: &AstArena) -> Result<EffectResult> {
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

        // `eval`/`run_resumed` move the session AST out of `self.ast` into a
        // local for the duration of the run (to satisfy the borrow checker),
        // passing it down to us as `ast`. `write_runtime` serializes
        // `rt.ast`, so without this it would snapshot an empty arena.
        // Swap it in just for the write, then restore whatever was there
        // before (rather than assuming empty) so we don't clobber state a
        // caller may have left in `self.ast`.
        let prev_ast = std::mem::replace(&mut self.ast, ast.clone());
        let bytes = crate::snapshot::write_runtime(self, &ready);

        // Write to a temp file and rename so a process kill mid-write can't
        // truncate/corrupt the last good checkpoint on disk.
        let tmp_path = format!("{path}.tmp");
        let write_result = std::fs::write(&tmp_path, &bytes)
            .map_err(|e| JSError::Message(format!("Snapshot: cannot write {tmp_path}: {e}")))
            .and_then(|_| {
                std::fs::rename(&tmp_path, &path).map_err(|e| {
                    JSError::Message(format!("Snapshot: cannot rename {tmp_path}: {e}"))
                })
            });
        self.ast = prev_ast;
        write_result?;

        // The live run continues, seeing "saved".
        self.interpreter.control = Control::Value(JSValue::String(saved));
        Ok(EffectResult::Resume)
    }

    fn handle_print(&mut self, args: Vec<JSValue>) -> Result<EffectResult> {
        let output: Vec<String> = args
            .iter()
            .map(|arg| self.interpreter.to_string(*arg))
            .collect();
        let msg = output.join(" ");

        #[cfg(feature = "wasm")]
        web_sys::console::log_1(&msg.into());

        #[cfg(not(feature = "wasm"))]
        println!("{}", msg);

        self.interpreter.control = Control::Value(JSValue::Undefined);
        Ok(EffectResult::Resume)
    }

    fn fail_current_fiber(&mut self, error: JSError) {
        let fiber_id = self.current.expect("No current fiber to fail");

        let err_val = self.error_value(&error);
        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber should be found");

        fiber.status = FiberStatus::Failed(error);

        if let Some(waiters) = self.join_waiters.remove(&fiber_id) {
            for waiter_id in waiters {
                let result = self.join_result("err", err_val);
                self.unblock_fiber(waiter_id, result);
            }
            // Error delivered to every joiner — the fiber is consumed.
            self.remove_fiber(fiber_id);
        }
    }

    /// A fiber's error, surfaced as a plain JS value in another fiber.
    fn error_value(&mut self, error: &JSError) -> JSValue {
        JSValue::String(self.interpreter.strings.intern(&error.to_string()))
    }

    /// A fiber's fate as a value: {ok: result} or {err: reason}.
    /// Allocated per receiver so joiners don't alias one wrapper object.
    fn join_result(&mut self, key: &str, value: JSValue) -> JSValue {
        let key = self.interpreter.strings.intern(key);
        let mut obj = Object::new();
        obj.set(key, value);
        let id = self.interpreter.objects.alloc(obj);
        JSValue::Object(ObjId(id.index() as u32))
    }

    fn unblock_fiber(&mut self, fiber_id: FiberId, value: JSValue) {
        let fiber = self.fibers.iter_mut().find(|f| f.id == fiber_id);
        assert!(fiber.is_some(), "Fiber to unblock not found");
        let fiber = fiber.unwrap();

        fiber.control = Control::Value(value);
        fiber.status = FiberStatus::Ready;
        self.ready_queue.push_back(fiber_id);
    }
}

#[cfg(test)]
mod host_effect_tests {
    use super::*;

    #[test]
    fn ungranted_effect_faults_with_named_error() {
        let mut rt = Runtime::new();
        let err = rt.eval("perform Nope!(1)").unwrap_err();
        assert_eq!(
            err.to_string(),
            "effect 'Nope' is not handled and not granted"
        );
    }

    #[test]
    fn granted_effect_blocks_the_fiber() {
        let mut rt = Runtime::new();
        rt.grant("Ask");
        // Scheduler drains with the root fiber blocked on the host;
        // RunOutcome (Task 3) will surface this — for now eval returns Undefined.
        rt.eval("perform Ask!(\"q\")").unwrap();
        assert!(
            rt.fibers
                .iter()
                .any(|f| matches!(f.status, FiberStatus::BlockedOnHost { .. }))
        );
    }

    #[test]
    fn in_language_handler_beats_a_grant() {
        let mut rt = Runtime::new();
        rt.grant("Ask");
        let v = rt
            .eval("handle { perform Ask!(1) } with { Ask!(x, resume) -> resume(x + 1) }")
            .unwrap();
        assert_eq!(v, JSValue::Int(2));
    }

    #[test]
    fn unconvertible_args_fault_at_perform_site() {
        let mut rt = Runtime::new();
        rt.grant("Ask");
        let err = rt.eval("perform Ask!((x) => x)").unwrap_err();
        assert!(err.to_string().contains("cannot convert"), "{err}");
    }
}
