use std::collections::VecDeque;

use crate::ast::AstArena;
use crate::cekh::{CEKH, Control};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::error::{JSError, Result};
use crate::string_pool::StrId;
use crate::value::JSValue;

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
    next_fiber_id: u32,
    // TODO: IO handler placeholder
    io: (),
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            interpreter: CEKH::new(),
            fibers: Vec::new(),
            ready_queue: VecDeque::new(),
            current: None,
            next_fiber_id: 0,
            io: (),
        }
    }

    pub fn run(&mut self, ast: &AstArena) -> Result<JSValue> {
        self.interpreter.fresh_exec_setup(ast)?;

        let root_fiber = Fiber {
            id: FiberId(0),
            cont: self.interpreter.cont,
            env: self.interpreter.env,
            status: FiberStatus::Ready,
        };

        self.fibers.push(root_fiber);
        self.ready_queue.push_back(FiberId(0));
        self.next_fiber_id = 1;

        let mut main_result = JSValue::Undefined;

        loop {
            if self.current.is_none() {
                if self.select_next_fiber().is_none() {
                    return Ok(main_result);
                }
            }

            match self.run_current_fiber(ast)? {
                Outcome::Done(value) => {
                    if self.current == Some(FiberId(0)) {
                        main_result = value;
                    }
                    self.complete_current_fiber(value);
                    self.current = None;
                }
                Outcome::Suspended { effect, args } => {
                    match self.handle_effect(effect, args)? {
                        EffectResult::Resume => {}
                        EffectResult::Block => {
                            // TODO: block current fiber, select next
                            self.current = None;
                        }
                        EffectResult::Spawned(id) => {
                            // TODO: continue current, queue new one
                            self.interpreter.control = Control::Value(JSValue::Int(id.0 as i32));
                        }
                    }
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
    }

    fn spawn_fiber(&mut self, cont: ContId, env: EnvId) -> FiberId {
        let id = FiberId(self.next_fiber_id);
        self.next_fiber_id += 1;

        let fiber = Fiber {
            id,
            cont,
            env,
            status: FiberStatus::Ready,
        };

        self.fibers.push(fiber);
        self.ready_queue.push_back(id);
        id
    }

    fn handle_effect(&mut self, effect: StrId, args: Vec<JSValue>) -> Result<EffectResult> {
        let effect_name = self.interpreter.strings.get(effect).unwrap_or("");

        match effect_name {
            "Print" => self.handle_print(args),
            _ => Err(JSError::runtime_error("Unknown effect")),
        }
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

        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber should be found");

        fiber.status = FiberStatus::Failed(error);
    }

    fn block_current_fiber(&mut self, effect: StrId, args: Vec<JSValue>) {
        let fiber_id = self.current.expect("No current fiber to block");

        let fiber = self
            .fibers
            .iter_mut()
            .find(|f| f.id == fiber_id)
            .expect("Current fiber not found");

        fiber.cont = self.interpreter.cont;
        fiber.env = self.interpreter.env;
        fiber.status = FiberStatus::Blocked { effect, args };
    }

    fn unblock_fiber(&mut self, fiber_id: FiberId, _value: JSValue) {
        let fiber = self.fibers.iter_mut().find(|f| f.id == fiber_id);
        assert!(fiber.is_some(), "Fiber to unblock not found");
        let fiber = fiber.unwrap();

        assert!(
            matches!(fiber.status, FiberStatus::Blocked { .. }),
            "Fiber to unblock is not Blocked"
        );

        fiber.status = FiberStatus::Ready;
        self.ready_queue.push_back(fiber_id);
    }
}
