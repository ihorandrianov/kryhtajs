use std::collections::{HashMap, VecDeque};

use crate::Kont;
use crate::ast::AstArena;
use crate::cekh::{CEKH, Control};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::error::{JSError, Result};
use crate::gc::GC;
use crate::host::{CallId, HostValue, PendingCall, RunOutcome};
use crate::object::{Object, ObjectKind};
use crate::parser::Parser;
use crate::replay::{LogEvent, LogHeader, LogMode, LogWriter};
use crate::string_pool::{StrId, StringPool};
use crate::value::{JSValue, ObjId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FiberId(pub u32);

#[derive(Debug, Clone)]
pub enum FiberStatus {
    Ready,
    Running,
    /// Never constructed anywhere; kept because snapshot tag 2 encodes it —
    /// removing the variant would orphan the tag in the v2 wire format.
    Blocked { effect: StrId, args: Vec<JSValue> },
    BlockedOnHost { effect: StrId, args: Vec<HostValue> },
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
    /// Replay-log state. Off by default; see src/replay.rs.
    pub(crate) log: LogMode,
}

impl Runtime {
    /// The builtin effect names every runtime understands out of the box.
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
            log: LogMode::Off,
        }
    }

    /// Grant the script permission to perform `name` as a host effect.
    /// Ungranted, unhandled effects fail at the perform site. Grants are
    /// frozen once a recorded or replaying run starts: the log header is
    /// the single source of truth for what the run was allowed to do.
    pub fn grant(&mut self, name: &str) -> Result<()> {
        if matches!(self.log, LogMode::Recording(_) | LogMode::Replaying) {
            return Err(JSError::Message(
                "grant: grants are frozen once a recorded run starts".to_string(),
            ));
        }
        let id = self.interpreter.strings.intern(name);
        self.effects.insert(id, EffectKind::Host);
        Ok(())
    }

    /// Host-granted effect names, sorted so the log header is byte-stable
    /// regardless of HashMap iteration order.
    pub(crate) fn granted_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .effects
            .iter()
            .filter(|(_, kind)| matches!(kind, EffectKind::Host))
            .map(|(id, _)| self.interpreter.strings.get(*id).unwrap_or("").to_string())
            .collect();
        names.sort();
        names
    }

    /// Reconstruct a runtime from bytes produced by `snapshot::write_runtime`.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Runtime> {
        crate::snapshot::read_runtime(bytes)
    }

    /// Host-triggered checkpoint. Callable between runs — typically after
    /// `run_hosted` returned `Pending` — when `self.ast` holds the session AST.
    pub fn snapshot(&mut self, path: &str) -> Result<()> {
        let ready = self.ready_queue.clone();
        let bytes = crate::snapshot::write_runtime(self, &ready);
        crate::snapshot::write_snapshot_file(path, &bytes)
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

    /// Run to completion, requiring the whole program to stay inside the
    /// language: a host call surfacing is an error rather than a value.
    /// Use `run_hosted` for programs that perform granted host effects.
    pub fn run(&mut self, ast: &AstArena) -> Result<JSValue> {
        match self.run_hosted(ast)? {
            RunOutcome::Done(v) => Ok(v),
            RunOutcome::Pending(calls) => Err(JSError::Message(format!(
                "effect '{}' suspended to the host; use run_hosted/eval_hosted",
                calls[0].effect
            ))),
        }
    }

    pub fn run_hosted(&mut self, ast: &AstArena) -> Result<RunOutcome> {
        if !matches!(self.log, LogMode::Off) {
            return Err(JSError::Message(
                "run_hosted: a recorded run must start via eval_hosted (the log header embeds the source text)"
                    .to_string(),
            ));
        }
        self.run_hosted_fresh(ast)
    }

    pub(crate) fn run_hosted_fresh(&mut self, ast: &AstArena) -> Result<RunOutcome> {
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

        let result = self.run_scheduler(ast);
        self.finish_run(result)
    }

    /// Continue a runtime restored from a snapshot: enter the scheduler
    /// without the per-run reset, re-surfacing any still-pending host calls.
    pub fn run_resumed(&mut self) -> Result<RunOutcome> {
        self.run_hosted_continue()
    }

    /// Re-enter the scheduler after `resume_with` — no per-run reset,
    /// mirroring `run_resumed`.
    pub fn run_hosted_continue(&mut self) -> Result<RunOutcome> {
        if matches!(self.log, LogMode::Replaying) {
            return Err(JSError::Message(
                "run_hosted_continue: runtime is replaying".to_string(),
            ));
        }
        if let LogMode::Recording(w) = &mut self.log {
            if !w.done_written {
                w.append(&LogEvent::Continue)?;
            }
        }
        self.continue_scheduler()
    }

    pub(crate) fn continue_scheduler(&mut self) -> Result<RunOutcome> {
        let ast = std::mem::take(&mut self.ast);
        let result = self.run_scheduler(&ast);
        self.ast = ast;
        self.finish_run(result)
    }

    /// Seal a completed recorded run with a Done event (once). The result
    /// is recorded when it's plain data; code values record as None.
    pub(crate) fn finish_run(&mut self, result: Result<RunOutcome>) -> Result<RunOutcome> {
        let Ok(RunOutcome::Done(v)) = &result else {
            return result;
        };
        if !matches!(self.log, LogMode::Recording(_)) {
            return result;
        }
        let host_result = crate::host::to_host_value(&self.interpreter, *v).ok();
        if let LogMode::Recording(w) = &mut self.log {
            if !w.done_written {
                w.append(&LogEvent::Done {
                    result: host_result,
                })?;
                w.done_written = true;
            }
        }
        result
    }

    pub fn eval_hosted(&mut self, source: &str) -> Result<RunOutcome> {
        if matches!(self.log, LogMode::Recording(_) | LogMode::Replaying) {
            return Err(JSError::Message(
                "eval_hosted: a recorded or replaying run is already active on this runtime"
                    .to_string(),
            ));
        }
        let parser =
            Parser::with_state(source, self.interpreter.strings.clone(), self.ast.clone())?;
        let (arena, strings) = parser.parse_program()?;
        self.interpreter.strings = strings;
        self.ast = arena;

        if matches!(self.log, LogMode::Armed { .. }) {
            let LogMode::Armed { path } = std::mem::replace(&mut self.log, LogMode::Off) else {
                unreachable!("checked by matches! above");
            };
            let header = LogHeader {
                source: source.to_string(),
                grants: self.granted_names(),
            };
            self.log = LogMode::Recording(LogWriter::create(&path, &header)?);
        }

        let ast = std::mem::take(&mut self.ast);
        let result = self.run_hosted_fresh(&ast);
        self.ast = ast;
        result
    }

    /// Deliver a host answer to a suspended `perform` and mark its fiber
    /// ready. Derived fresh from fiber state each call rather than stored,
    /// since fiber state is already the single source of truth for what's
    /// pending (see `pending_calls`).
    pub fn resume_with(&mut self, id: CallId, value: HostValue) -> Result<()> {
        if matches!(self.log, LogMode::Replaying) {
            return Err(JSError::Message(
                "resume_with: runtime is replaying; answers come from the log".to_string(),
            ));
        }
        let fiber_id = id.0;
        let pending = self.fibers.iter().find_map(|f| match (&f.id, &f.status) {
            (fid, FiberStatus::BlockedOnHost { effect, args }) if *fid == fiber_id => {
                Some((*effect, args.clone()))
            }
            _ => None,
        });
        let Some((effect, args)) = pending else {
            return Err(JSError::Message(format!(
                "resume_with: no pending host call for fiber {}",
                fiber_id.0
            )));
        };
        if matches!(self.log, LogMode::Recording(_)) {
            let effect_name = self
                .interpreter
                .strings
                .get(effect)
                .unwrap_or("")
                .to_string();
            let event = LogEvent::HostAnswer {
                call_id: fiber_id.0,
                effect: effect_name,
                args,
                answer: value.clone(),
            };
            if let LogMode::Recording(w) = &mut self.log {
                // Write-ahead: if the append fails, the answer is NOT
                // applied — the log never lags the runtime.
                w.append(&event)?;
            }
        }
        self.apply_host_answer(fiber_id, &value);
        Ok(())
    }

    pub(crate) fn apply_host_answer(&mut self, fiber_id: FiberId, value: &HostValue) {
        let v = crate::host::from_host_value(&mut self.interpreter, value);
        self.unblock_fiber(fiber_id, v);
    }

    /// Pending-call derivation (single source of truth — never stored).
    /// Args were already converted to `HostValue` at block time
    /// (`handle_host_effect`), so this is just a read, not a fallible
    /// conversion — draining can't fail because of what a sibling fiber did
    /// to a shared arg after the block.
    fn pending_calls(&self) -> Vec<PendingCall> {
        let mut calls = Vec::new();
        for fiber in &self.fibers {
            if let FiberStatus::BlockedOnHost { effect, args } = &fiber.status {
                let name = self
                    .interpreter
                    .strings
                    .get(*effect)
                    .unwrap_or("")
                    .to_string();
                calls.push(PendingCall {
                    id: CallId(fiber.id),
                    effect: name,
                    args: args.clone(),
                });
            }
        }
        calls
    }

    fn run_scheduler(&mut self, ast: &AstArena) -> Result<RunOutcome> {
        let mut main_result = JSValue::Undefined;

        loop {
            if self.current.is_none() {
                if self.select_next_fiber().is_none() {
                    let pending = self.pending_calls();
                    if !pending.is_empty() {
                        return Ok(RunOutcome::Pending(pending));
                    }
                    // Prefer the root fiber's recorded result: on a continued
                    // or resumed run, the root may have completed in an
                    // earlier scheduler entry, where the local `main_result`
                    // never saw it.
                    let root = self.fibers.iter().find_map(|f| match (&f.id, &f.status) {
                        (FiberId(0), FiberStatus::Completed(v)) => Some(*v),
                        _ => None,
                    });
                    return Ok(RunOutcome::Done(root.unwrap_or(main_result)));
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
        // Convert to HostValue now, not at drain time: this both faults an
        // unconvertible argument at the perform site, and freezes what the
        // host will see. A sibling fiber can still run before the ready
        // queue drains; if the args were left as JSValue and converted
        // later, a shared arg object mutated by a sibling after this block
        // would change the host-visible call, and a sibling adding a
        // function property to it would turn draining itself into a
        // fallible operation that could wedge the whole run.
        let mut host_args = Vec::with_capacity(args.len());
        for arg in args {
            host_args.push(crate::host::to_host_value(&self.interpreter, arg)?);
        }

        let fiber_id = self.current.expect("No current fiber for host effect");
        self.save_current_fiber_state();
        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber not found");
        fiber.status = FiberStatus::BlockedOnHost {
            effect,
            args: host_args,
        };
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

        // Replay is read-only reconstruction: the original run already
        // wrote this snapshot, and on a from-start run Snapshot! always
        // returned "saved" — deterministic, so nothing needs the log.
        if !matches!(self.log, LogMode::Replaying) {
            let prev_ast = std::mem::replace(&mut self.ast, ast.clone());
            let bytes = crate::snapshot::write_runtime(self, &ready);
            self.ast = prev_ast;
            crate::snapshot::write_snapshot_file(&path, &bytes)?;
        }

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
        use crate::host::RunOutcome;
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        // eval_hosted surfaces the block as RunOutcome::Pending rather than
        // leaving the caller to inspect fiber internals.
        let outcome = rt.eval_hosted("perform Ask!(\"q\")").unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].effect, "Ask");
        assert!(
            rt.fibers
                .iter()
                .any(|f| matches!(f.status, FiberStatus::BlockedOnHost { .. }))
        );
    }

    #[test]
    fn host_effect_suspends_and_resumes_end_to_end() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let outcome = rt.eval_hosted("perform Ask!(\"q\", 42)").unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].effect, "Ask");
        assert_eq!(
            calls[0].args,
            vec![HostValue::Str("q".to_string()), HostValue::Int(42)]
        );

        rt.resume_with(calls[0].id, HostValue::Str("answer".to_string()))
            .unwrap();
        let RunOutcome::Done(v) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done");
        };
        let JSValue::String(id) = v else {
            panic!("expected string result, got {v:?}");
        };
        assert_eq!(rt.interpreter.strings.get(id), Some("answer"));
    }

    #[test]
    fn plain_eval_errors_when_a_host_call_surfaces() {
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let err = rt.eval("perform Ask!(1)").unwrap_err();
        assert!(err.to_string().contains("Ask"), "{err}");
        assert!(err.to_string().contains("run_hosted"), "{err}");
    }

    #[test]
    fn resume_with_unknown_call_id_is_an_error() {
        use crate::host::{CallId, HostValue};
        let mut rt = Runtime::new();
        let err = rt
            .resume_with(CallId(FiberId(99)), HostValue::Int(1))
            .unwrap_err();
        assert!(err.to_string().contains("no pending host call"), "{err}");
    }

    #[test]
    fn in_language_handler_beats_a_grant() {
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let v = rt
            .eval("handle { perform Ask!(1) } with { Ask!(x, resume) -> resume(x + 1) }")
            .unwrap();
        assert_eq!(v, JSValue::Int(2));
    }

    #[test]
    fn unconvertible_args_fault_at_perform_site() {
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let err = rt.eval("perform Ask!((x) => x)").unwrap_err();
        assert!(err.to_string().contains("cannot convert"), "{err}");
    }

    #[test]
    fn two_fibers_pend_concurrently_and_answer_out_of_order() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let outcome = rt
            .eval_hosted(
                "let a = perform Fork!(() => perform Ask!(\"first\"));\n\
                 let b = perform Fork!(() => perform Ask!(\"second\"));\n\
                 let ra = perform Join!(a);\n\
                 let rb = perform Join!(b);\n\
                 [ra.ok, rb.ok]",
            )
            .unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending, root should be blocked on Join");
        };
        assert_eq!(calls.len(), 2);

        let first = calls
            .iter()
            .find(|c| c.args == vec![HostValue::Str("first".into())])
            .unwrap();
        let second = calls
            .iter()
            .find(|c| c.args == vec![HostValue::Str("second".into())])
            .unwrap();

        // Answer in reverse arrival order.
        rt.resume_with(second.id, HostValue::Int(2)).unwrap();
        rt.resume_with(first.id, HostValue::Int(1)).unwrap();

        let RunOutcome::Done(v) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done");
        };
        let hv = crate::host::to_host_value(&rt.interpreter, v).unwrap();
        assert_eq!(
            hv,
            HostValue::Array(vec![HostValue::Int(1), HostValue::Int(2)])
        );
    }

    #[test]
    fn answering_a_subset_returns_the_remainder() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let outcome = rt
            .eval_hosted(
                "let a = perform Fork!(() => perform Ask!(\"one\"));\n\
                 let b = perform Fork!(() => perform Ask!(\"two\"));\n\
                 let ra = perform Join!(a);\n\
                 let rb = perform Join!(b);\n\
                 0",
            )
            .unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending")
        };
        assert_eq!(calls.len(), 2);

        // Match by args, not position: fiber scheduling order is not part
        // of the contract, only which call carries which args.
        let one = calls
            .iter()
            .find(|c| c.args == vec![HostValue::Str("one".into())])
            .unwrap();
        let two = calls
            .iter()
            .find(|c| c.args == vec![HostValue::Str("two".into())])
            .unwrap();

        rt.resume_with(one.id, HostValue::Int(1)).unwrap();
        let RunOutcome::Pending(rest) = rt.run_hosted_continue().unwrap() else {
            panic!("expected the unanswered call to still pend");
        };
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, two.id);

        rt.resume_with(rest[0].id, HostValue::Int(2)).unwrap();
        let RunOutcome::Done(v) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done")
        };
        assert_eq!(v, JSValue::Int(0));
    }

    #[test]
    fn root_result_survives_while_child_pends() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        // The root fiber forks a child that blocks on a host effect, then
        // completes on its own. run_hosted_continue must report the root's
        // recorded result once the child is answered, not Undefined.
        let outcome = rt
            .eval_hosted("let a = perform Fork!(() => perform Ask!(\"bg\")); 7")
            .unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending, child should be blocked on Ask");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].effect, "Ask");
        assert_eq!(calls[0].args, vec![HostValue::Str("bg".into())]);

        rt.resume_with(calls[0].id, HostValue::Int(99)).unwrap();
        let RunOutcome::Done(v) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(v, JSValue::Int(7));
    }

    #[test]
    fn pending_host_call_survives_snapshot_round_trip() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        let RunOutcome::Pending(calls) = rt.eval_hosted("perform Ask!(\"q\")").unwrap() else {
            panic!()
        };

        let ready = rt.ready_queue.clone();
        let bytes = {
            // In-memory round trip: same codec `Runtime::snapshot` writes to disk.
            crate::snapshot::write_runtime(&rt, &ready)
        };
        let mut rt2 = Runtime::from_snapshot(&bytes).unwrap();

        let RunOutcome::Pending(calls2) = rt2.run_resumed().unwrap() else {
            panic!("restored runtime must re-surface the pending call");
        };
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].effect, "Ask");
        assert_eq!(calls2[0].args, calls[0].args);
        assert_eq!(calls2[0].id, calls[0].id);

        rt2.resume_with(calls2[0].id, HostValue::Int(5)).unwrap();
        let RunOutcome::Done(v) = rt2.run_hosted_continue().unwrap() else {
            panic!()
        };
        assert_eq!(v, JSValue::Int(5));
    }

    #[test]
    fn pending_call_args_are_frozen_at_block_time_not_drain_time() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        // `a` forks first, so it lands at the front of the ready queue and
        // runs (and blocks on Ask, converting `o`) before `b` gets a turn.
        // `b` then mutates the same shared object. If conversion happened at
        // drain time instead of block time, the surfaced call would see
        // `b`'s mutation even though it happened after `a` had already
        // asked.
        let outcome = rt
            .eval_hosted(
                "let o = {v: 1};\n\
                 let a = perform Fork!(() => perform Ask!(o));\n\
                 let b = perform Fork!(() => { o.v = 2; 0 });\n\
                 0",
            )
            .unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending, a should be blocked on Ask");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].effect, "Ask");
        assert_eq!(
            calls[0].args,
            vec![HostValue::Object(vec![(
                "v".to_string(),
                HostValue::Int(1)
            )])],
            "pending call must reflect the object's value at block time, not after b's later mutation"
        );

        rt.resume_with(calls[0].id, HostValue::Int(99)).unwrap();
        let RunOutcome::Done(v) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(v, JSValue::Int(0));
    }
}
