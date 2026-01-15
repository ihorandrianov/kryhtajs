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

#[derive(Debug)]
pub struct Fiber {
    pub id: FiberId,
    pub cont: ContId,
    pub env: EnvId,
    pub status: FiberStatus,
}

pub enum Outcome {
    Done(JSValue),
    Suspended { effect: StrId, args: Vec<JSValue> },
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
        todo!("main loop: run fibers, handle effects, poll IO")
    }

    fn spawn_fiber(&mut self, cont: ContId, env: EnvId) -> FiberId {
        todo!("create fiber, add to ready queue")
    }

    fn schedule_next(&mut self) -> Option<FiberId> {
        todo!("pick next ready fiber")
    }

    fn handle_effect(&mut self, effect: StrId, args: Vec<JSValue>) -> Result<()> {
        todo!("dispatch effect to appropriate handler")
    }

    fn complete_fiber(&mut self, fiber_id: FiberId, value: JSValue) {
        todo!("mark fiber as completed")
    }

    fn fail_fiber(&mut self, fiber_id: FiberId, error: JSError) {
        todo!("mark fiber as failed")
    }

    fn resume_fiber(&mut self, fiber_id: FiberId, value: JSValue) {
        todo!("move fiber from blocked to ready, store resume value")
    }
}
