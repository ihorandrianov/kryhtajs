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
use crate::string_pool::StrId;
use crate::value::{JSValue, ObjId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FiberId(pub u32);

#[derive(Debug, Clone)]
pub enum FiberStatus {
    Ready,
    Running,
    Blocked { effect: StrId, args: Vec<JSValue> },
    Completed(JSValue),
    Failed(JSError),
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
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            interpreter: CEKH::new(),
            fibers: Vec::new(),
            ready_queue: VecDeque::new(),
            current: None,
            next_fiber_id: 0,
            join_waiters: HashMap::new(),
            gc: GC::new(),
            ast: AstArena::new(),
        }
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
            self.gc.collect(&mut self.interpreter, &self.fibers);
        }

        let effect_name = self.interpreter.strings.get(effect).unwrap_or("");

        match effect_name {
            "Print" => self.handle_print(args),
            "Fork" => self.handle_fork(args, ast),
            "Join" => self.handle_join(args),
            "Gc" => self.handle_gc(),
            _ => Err(JSError::runtime_error("Unknown effect")),
        }
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
        self.gc.collect(&mut self.interpreter, &self.fibers);
        self.interpreter.control = Control::Value(JSValue::Undefined);
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
