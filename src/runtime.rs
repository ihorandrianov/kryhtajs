use std::collections::{HashMap, VecDeque};

use crate::Kont;
use crate::ast::AstArena;
use crate::cekh::{CEKH, Control};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::error::{JSError, Result};
use crate::fuel::{MeterId, Meters};
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

    pub meter: MeterId,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Done(JSValue),
    Suspended { effect: StrId, args: Vec<JSValue> },
    /// Slice allowance exhausted mid-fiber; interpreter state is intact
    /// and the fiber can be re-entered.
    Preempted,
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
    pub(crate) meters: Meters,
    /// Slice quantum: max steps per scheduler slice. Always on.
    pub(crate) quantum: u64,
    /// Execution index: total steps across the whole run. Deterministic.
    pub(crate) steps_total: u64,
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
            meters: Meters::new(),
            quantum: crate::fuel::DEFAULT_QUANTUM,
            steps_total: 0,
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

    /// Set the root fuel budget (None = unlimited). Fresh runtimes only:
    /// the budget is part of a run's identity, so it cannot change once
    /// fibers exist, and it is frozen while a log is recording/replaying
    /// (the log header is the source of truth for the recorded config).
    /// The budget is spent, not reset: a later run on the same runtime
    /// starts from whatever the root meter has left, and fuel carved for
    /// never-joined children is forfeited.
    pub fn set_fuel(&mut self, budget: Option<u64>) -> Result<()> {
        if matches!(self.log, LogMode::Recording(_) | LogMode::Replaying) {
            return Err(JSError::Message(
                "set_fuel: fuel config is frozen while a log is recording or replaying"
                    .to_string(),
            ));
        }
        if self.next_fiber_id != 0 || !self.fibers.is_empty() {
            return Err(JSError::Message(
                "set_fuel: requires a fresh runtime (budget is part of the run's identity)"
                    .to_string(),
            ));
        }
        self.meters.set_root_budget(budget);
        Ok(())
    }

    /// Set the slice quantum. Same freeze rules as set_fuel: the quantum
    /// determines fiber interleaving, hence replay identity.
    pub fn set_quantum(&mut self, quantum: u64) -> Result<()> {
        if quantum == 0 {
            return Err(JSError::Message(
                "set_quantum: quantum must be at least 1".to_string(),
            ));
        }
        if matches!(self.log, LogMode::Recording(_) | LogMode::Replaying) {
            return Err(JSError::Message(
                "set_quantum: fuel config is frozen while a log is recording or replaying"
                    .to_string(),
            ));
        }
        if self.next_fiber_id != 0 || !self.fibers.is_empty() {
            return Err(JSError::Message(
                "set_quantum: requires a fresh runtime".to_string(),
            ));
        }
        self.quantum = quantum;
        Ok(())
    }

    /// Top up the root meter after an OutOfFuel pause. Refusing any other
    /// state keeps the API honest: fuel cannot be silently topped up
    /// mid-run, and the check is derivable from meter state alone, so it
    /// survives snapshot/restore. (Recording appends a Refuel event —
    /// added in the replay task.)
    pub fn add_fuel(&mut self, amount: u64) -> Result<()> {
        if matches!(self.log, LogMode::Replaying) {
            return Err(JSError::Message(
                "add_fuel: runtime is replaying; refuels come from the log".to_string(),
            ));
        }
        if self.meters.remaining(Meters::ROOT) != Some(0) {
            return Err(JSError::Message(
                "add_fuel: runtime is not out of fuel".to_string(),
            ));
        }
        // Write-ahead like resume_with: the Refuel record hits disk before
        // the meter is topped up, so the log never lags the runtime.
        if matches!(self.log, LogMode::Recording(_)) {
            let steps = self.steps_total;
            let event = LogEvent::Refuel { amount };
            if let LogMode::Recording(w) = &mut self.log {
                w.append(&event, steps)?;
            }
        }
        self.meters.add(Meters::ROOT, amount);
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
            RunOutcome::OutOfFuel { spent } => Err(JSError::Message(format!(
                "run out of fuel after {spent} steps; use run_hosted/eval_hosted and add_fuel to resume"
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

        // Reset the meter arena and step index for this run, but preserve
        // whatever root budget was configured — a fresh run should not
        // silently regain unlimited fuel just because it's a new run.
        let root_budget = self.meters.remaining(Meters::ROOT);
        self.meters = Meters::new();
        self.meters.set_root_budget(root_budget);
        self.steps_total = 0;

        let root_fiber = Fiber {
            id: FiberId(0),
            control: self.interpreter.control.clone(),
            cont: self.interpreter.cont,
            env: self.interpreter.env,
            status: FiberStatus::Ready,
            meter: Meters::ROOT,
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
        let steps = self.steps_total;
        if let LogMode::Recording(w) = &mut self.log {
            if !w.done_written {
                w.append(&LogEvent::Continue, steps)?;
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
        let steps = self.steps_total;
        if let LogMode::Recording(w) = &mut self.log {
            if !w.done_written {
                w.append(
                    &LogEvent::Done {
                        result: host_result,
                    },
                    steps,
                )?;
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
                fuel: self.meters.remaining(Meters::ROOT),
                quantum: self.quantum,
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
        let is_pending = self.fibers.iter().any(|f| {
            f.id == fiber_id && matches!(f.status, FiberStatus::BlockedOnHost { .. })
        });
        if !is_pending {
            return Err(JSError::Message(format!(
                "resume_with: no pending host call for fiber {}",
                fiber_id.0
            )));
        }
        if matches!(self.log, LogMode::Recording(_)) {
            // Only the recording path needs effect/args off the fiber — the
            // clone would be wasted work on every plain (non-logged) call.
            let (effect, args) = self
                .fibers
                .iter()
                .find_map(|f| match (&f.id, &f.status) {
                    (fid, FiberStatus::BlockedOnHost { effect, args }) if *fid == fiber_id => {
                        Some((*effect, args.clone()))
                    }
                    _ => None,
                })
                .expect("checked pending above");
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
            let steps = self.steps_total;
            if let LogMode::Recording(w) = &mut self.log {
                // Write-ahead: if the append fails, the answer is NOT
                // applied — the log never lags the runtime.
                w.append(&event, steps)?;
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

            let meter_id = self.current_fiber_meter();
            let allowance = match self.meters.remaining(meter_id) {
                None => self.quantum,
                Some(0) => {
                    if meter_id == Meters::ROOT {
                        // Pause the whole run. The fiber goes back to the
                        // FRONT of the queue so a refueled continue picks
                        // up exactly where the pause happened.
                        self.save_current_fiber_state();
                        let id = self.current.take().expect("selected fiber");
                        self.ready_queue.push_front(id);
                        return Ok(RunOutcome::OutOfFuel {
                            spent: self.steps_total,
                        });
                    }
                    // Carved meter ran dry: the fiber fails, contained
                    // like any child failure; parents see {err} at Join.
                    self.fail_current_fiber(JSError::Message("out_of_fuel".to_string()));
                    self.current = None;
                    continue;
                }
                Some(rem) => self.quantum.min(rem),
            };

            let result = self.run_current_fiber(ast, allowance);
            let spent = self.interpreter.steps_spent;
            self.meters.charge(meter_id, spent);
            self.steps_total += spent;

            match result {
                Ok(Outcome::Done(value)) => {
                    if self.current == Some(FiberId(0)) {
                        main_result = value;
                    }
                    self.complete_current_fiber(value);
                    self.current = None;
                }
                Ok(Outcome::Suspended { effect, args }) => {
                    match self.handle_effect(effect, args, ast) {
                        Ok(EffectResult::Resume) => {}
                        Ok(EffectResult::Block) => {
                            self.current = None;
                        }
                        Ok(EffectResult::Spawned(id)) => {
                            self.interpreter.control = Control::Value(JSValue::Int(id.0 as i32));
                        }
                        Err(err) => {
                            // Contain like the interpreter-error arm below:
                            // the root fiber's failure is the program's
                            // failure; a child fiber's effect-handler failure
                            // (e.g. a carve fork with insufficient fuel) is
                            // contained and surfaces at whoever joins it.
                            if self.current == Some(FiberId(0)) {
                                return Err(err);
                            }
                            self.fail_current_fiber(err);
                            self.current = None;
                        }
                    }
                }
                Ok(Outcome::Preempted) => {
                    self.save_current_fiber_state();
                    let id = self.current.take().expect("preempted without current");
                    self.ready_queue.push_back(id);
                    // The new GC opportunity: fiber state is fully saved,
                    // no loose values in flight (unlike the Suspended arm,
                    // whose taken args are not GC roots — that check stays
                    // in handle_effect untouched).
                    if self.gc.should_collect(&self.interpreter) {
                        self.gc
                            .collect(&mut self.interpreter, &self.fibers, &self.effects);
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

    fn run_current_fiber(&mut self, ast: &AstArena, allowance: u64) -> Result<Outcome> {
        assert!(self.current.is_some(), "No current fiber to run");
        self.interpreter.run(ast, allowance)
    }

    fn current_fiber_meter(&self) -> MeterId {
        let id = self.current.expect("No current fiber");
        self.fibers
            .iter()
            .find(|f| f.id == id)
            .expect("Current fiber not found")
            .meter
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
            let meter = self.fibers[pos].meter;
            // Refund only when no other fiber (any status) still holds
            // this meter: the last consumed holder returns the leftover.
            // Never-joined fibers are never removed, so they forfeit —
            // documented in the spec.
            let shared = self
                .fibers
                .iter()
                .any(|f| f.id != fiber_id && f.meter == meter);
            if !shared {
                self.meters.refund_into_parent(meter);
            }
            self.fibers.swap_remove(pos);
        }
    }

    fn spawn_fiber(
        &mut self,
        cont: ContId,
        env: EnvId,
        control: Control,
        meter: MeterId,
    ) -> FiberId {
        let id = self.get_next_fiber_id();

        let fiber = Fiber {
            id,
            control,
            cont,
            env,
            status: FiberStatus::Ready,
            meter,
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

        let parent_meter = self.current_fiber_meter();
        let child_meter = match args.get(1).copied() {
            None | Some(JSValue::Undefined) => parent_meter,
            Some(JSValue::Object(oid)) => {
                let fuel_key = self.interpreter.strings.intern("fuel");
                let obj = self
                    .interpreter
                    .objects
                    .get(oid.into_arena_id())
                    .ok_or(JSError::InternalError("Invalid options object"))?;
                match obj.get(fuel_key) {
                    None | Some(JSValue::Undefined) => parent_meter,
                    Some(JSValue::Int(n)) if n >= 0 => {
                        self.meters.carve(parent_meter, n as u64)?
                    }
                    Some(_) => {
                        return Err(JSError::type_error(
                            "Fork: fuel must be a non-negative integer",
                        ));
                    }
                }
            }
            Some(_) => {
                return Err(JSError::type_error("Fork: options must be an object"));
            }
        };

        let id = self.spawn_fiber(cont, env, control, child_meter);
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

#[cfg(test)]
mod fuel_budget_tests {
    use super::*;
    use crate::host::RunOutcome;

    #[test]
    fn root_budget_pauses_an_infinite_loop() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(5_000)).unwrap();
        let outcome = rt.eval_hosted("while (true) { let x = 1; }").unwrap();
        let RunOutcome::OutOfFuel { spent } = outcome else {
            panic!("expected OutOfFuel, got {outcome:?}");
        };
        assert!(spent >= 5_000);
    }

    #[test]
    fn refuel_and_continue_finishes_a_bounded_program() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(50)).unwrap();
        let outcome = rt
            .eval_hosted("let x = 0; while (x < 100) { x = x + 1; } x;")
            .unwrap();
        assert!(matches!(outcome, RunOutcome::OutOfFuel { .. }));
        rt.add_fuel(1_000_000).unwrap();
        let outcome = rt.run_hosted_continue().unwrap();
        let RunOutcome::Done(v) = outcome else {
            panic!("expected Done after refuel, got {outcome:?}");
        };
        assert_eq!(v, crate::value::JSValue::Int(100));
    }

    #[test]
    fn add_fuel_requires_the_out_of_fuel_state() {
        let mut rt = Runtime::new();
        let err = rt.add_fuel(10).unwrap_err();
        assert!(err.to_string().contains("not out of fuel"), "{err}");
    }

    #[test]
    fn set_fuel_requires_a_fresh_runtime() {
        let mut rt = Runtime::new();
        rt.eval("1;").unwrap();
        let err = rt.set_fuel(Some(10)).unwrap_err();
        assert!(err.to_string().contains("fresh"), "{err}");
    }

    #[test]
    fn set_quantum_rejects_zero() {
        let mut rt = Runtime::new();
        let err = rt.set_quantum(0).unwrap_err();
        assert!(err.to_string().contains("quantum"), "{err}");
    }

    #[test]
    fn plain_run_reports_out_of_fuel_as_error() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(100)).unwrap();
        let err = rt.eval("while (true) { let x = 1; }").unwrap_err();
        assert!(err.to_string().contains("out of fuel"), "{err}");
    }
}

#[cfg(test)]
mod fuel_scheduler_tests {
    use super::*;

    // NOTE: the user-visible fairness win (a spinner no longer starves
    // its siblings) is only observable once a run can END despite the
    // spinner — that needs the root budget, so it's tested in Task 4/5
    // (fuel_fork_tests::fork_without_options_shares_the_parent_meter).
    // Here we test the mechanics the quantum adds: charging and GC.

    #[test]
    fn steps_total_accumulates_across_slices() {
        let mut rt = Runtime::new();
        rt.eval("let x = 0; while (x < 100) { x = x + 1; }").unwrap();
        assert!(rt.steps_total > 100);
    }

    #[test]
    fn steps_total_is_deterministic() {
        let mut a = Runtime::new();
        let mut b = Runtime::new();
        a.eval("let x = 0; while (x < 100) { x = x + 1; }").unwrap();
        b.eval("let x = 0; while (x < 100) { x = x + 1; }").unwrap();
        assert_eq!(a.steps_total, b.steps_total);
    }

    #[test]
    fn effect_free_loop_no_longer_starves_gc() {
        // Allocates in a loop that performs no effects. Before this task,
        // GC only ran inside handle_effect, so collections stayed at 0.
        let mut rt = Runtime::new();
        rt.eval(
            "let i = 0;\n\
             while (i < 20000) { let o = { a: 1 }; i = i + 1; }",
        )
        .unwrap();
        assert!(
            rt.gc_stats().collections > 0,
            "GC never ran during an effect-free allocating loop"
        );
    }
}

#[cfg(test)]
mod fuel_fork_tests {
    use super::*;
    use crate::value::JSValue;

    #[test]
    fn carved_child_that_spins_fails_with_out_of_fuel_at_join() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(1_000_000)).unwrap();
        let v = rt
            .eval(
                "let f = perform Fork!(() => { while (true) { let x = 1; } }, { fuel: 2000 });\n\
                 let r = perform Join!(f);\n\
                 r.err;",
            )
            .unwrap();
        let JSValue::String(s) = v else {
            panic!("expected err string, got {v:?}");
        };
        assert!(rt.interpreter.strings.get(s).unwrap().contains("out_of_fuel"));
    }

    #[test]
    fn unspent_carved_fuel_is_refunded_at_join() {
        use crate::host::{HostValue, RunOutcome};
        let mut rt = Runtime::new();
        rt.set_fuel(Some(100_000)).unwrap();
        rt.grant("Probe").unwrap();

        // A host-effect pause between the fork and the join gives us a
        // mid-flight observation point: by the time we're blocked on
        // Probe, the carve has already happened (proving deduction), but
        // the join hasn't run yet, so the child's leftover fuel is still
        // parked in the child meter, not yet folded back into root
        // (proving the eventual refund is a distinct, later event).
        let outcome = rt
            .eval_hosted(
                "let f = perform Fork!(() => 42, { fuel: 50000 });\n\
                 perform Probe!();\n\
                 perform Join!(f);",
            )
            .unwrap();
        let RunOutcome::Pending(calls) = outcome else {
            panic!("expected Pending on Probe");
        };
        // Root was charged the full 50,000 up front at carve time; the
        // child's near-total leftover hasn't been refunded yet, so root
        // must still be at or below the post-carve level here.
        let mid_flight = rt.meters.remaining(crate::fuel::Meters::ROOT).unwrap();
        assert!(
            mid_flight <= 50_000,
            "carve did not deduct up front: root has {mid_flight}"
        );

        rt.resume_with(calls[0].id, HostValue::Undefined).unwrap();
        let RunOutcome::Done(_) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done after resuming Probe and joining");
        };
        // The child spent a handful of steps of its 50k carve; nearly all
        // of it must be back in the root meter now that the join has
        // consumed the fiber and refunded its leftover.
        let rem = rt.meters.remaining(crate::fuel::Meters::ROOT).unwrap();
        assert!(rem > 90_000, "leftover not refunded: root has {rem}");
    }

    #[test]
    fn fork_without_options_shares_the_parent_meter() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(5_000)).unwrap();
        // Shared meter: the forked spinner drains the ROOT budget, so the
        // whole run pauses rather than the child failing alone.
        let outcome = rt
            .eval_hosted(
                "let f = perform Fork!(() => { while (true) { let x = 1; } });\n\
                 perform Join!(f);",
            )
            .unwrap();
        assert!(matches!(
            outcome,
            crate::host::RunOutcome::OutOfFuel { .. }
        ));
    }

    #[test]
    fn fork_with_insufficient_fuel_fails_the_forking_fiber() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(100)).unwrap();
        let err = rt
            .eval("let f = perform Fork!(() => 1, { fuel: 5000 }); 1;")
            .unwrap_err();
        assert!(err.to_string().contains("insufficient fuel"), "{err}");
    }

    #[test]
    fn fork_with_non_integer_fuel_is_a_type_error() {
        let mut rt = Runtime::new();
        let err = rt
            .eval("perform Fork!(() => 1, { fuel: \"lots\" });")
            .unwrap_err();
        assert!(err.to_string().contains("fuel"), "{err}");
    }

    #[test]
    fn carved_child_forking_beyond_its_fuel_is_contained_at_join() {
        use crate::host::{to_host_value, HostValue};
        let mut rt = Runtime::new();
        rt.set_fuel(Some(1_000_000)).unwrap();
        // The child holds a 2,000-step carve. Inside it, a fork asking for
        // 100,000 exceeds the child's own meter, so the carve fails inside
        // an effect handler. That failure must be CONTAINED (not kill the
        // run): the root joins the child, sees {err} with "insufficient
        // fuel", then runs past the join to return a value that proves it
        // survived. Before the fix, this handle_effect error propagated out
        // of the scheduler and killed the whole run.
        let v = rt
            .eval(
                "let child = perform Fork!(() => {\n\
                     perform Fork!(() => 1, { fuel: 100000 });\n\
                     1;\n\
                 }, { fuel: 2000 });\n\
                 let r = perform Join!(child);\n\
                 let survived = 40 + 2;\n\
                 let out = { reason: r.err, survived: survived };\n\
                 out;",
            )
            .unwrap();
        let HostValue::Object(fields) = to_host_value(&rt.interpreter, v).unwrap() else {
            panic!("expected object result");
        };
        let reason = fields.iter().find(|(k, _)| k == "reason").map(|(_, x)| x);
        let survived = fields.iter().find(|(k, _)| k == "survived").map(|(_, x)| x);
        assert!(
            matches!(reason, Some(HostValue::Str(s)) if s.contains("insufficient fuel")),
            "join did not surface the child's insufficient-fuel failure: {reason:?}"
        );
        assert_eq!(
            survived,
            Some(&HostValue::Int(42)),
            "root did not run past the join"
        );
    }

    #[test]
    fn zero_fuel_carve_fails_the_child_at_out_of_fuel() {
        let mut rt = Runtime::new();
        rt.set_fuel(Some(1_000_000)).unwrap();
        // Zero-budget carve corner: the carve succeeds (0 <= remaining) and
        // the child gets a meter with remaining 0, so at its very first
        // selection the scheduler fails it with out_of_fuel. The parent's
        // join sees {err}; the run is unaffected.
        let v = rt
            .eval(
                "let f = perform Fork!(() => 1, { fuel: 0 });\n\
                 let r = perform Join!(f);\n\
                 r.err;",
            )
            .unwrap();
        let JSValue::String(s) = v else {
            panic!("expected err string, got {v:?}");
        };
        assert!(rt.interpreter.strings.get(s).unwrap().contains("out_of_fuel"));
    }
}
