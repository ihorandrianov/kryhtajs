use crate::cekh::{CEKH, Control};
use crate::cont::ContId;
use crate::env::EnvId;
use crate::handler::HandlerId;
use crate::runtime::{EffectKind, Fiber, FiberStatus};
use crate::string_pool::StrId;
use crate::value::{JSValue, ObjId};
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Copy)]
pub struct GCStats {
    pub collections: u64,
    pub objects_freed: u64,
    pub strings_freed: u64,
}

pub enum WorkItem {
    Value(JSValue),
    Env(EnvId),
    Cont(ContId),
}

pub struct GC {
    pub stats: GCStats,
    threshold: u64,
    /// interpreter.total_allocations() at the last collection
    alloc_snapshot: u64,
    work_list: Vec<WorkItem>,
}

impl GC {
    pub fn new() -> Self {
        Self {
            stats: GCStats::default(),
            threshold: 1000,
            alloc_snapshot: 0,
            work_list: Vec::with_capacity(1000),
        }
    }

    pub fn should_collect(&self, interpreter: &CEKH) -> bool {
        interpreter.total_allocations() - self.alloc_snapshot > self.threshold
    }

    /// Re-baseline the allocation snapshot to the interpreter's current
    /// total. Used when restoring a runtime from a snapshot, so the fresh
    /// `GC::new()` doesn't immediately think a threshold's worth of
    /// allocations has happened and trigger a spurious collection.
    pub fn reprime_baseline(&mut self, interpreter: &CEKH) {
        self.alloc_snapshot = interpreter.total_allocations();
    }

    pub fn collect(
        &mut self,
        interpreter: &mut CEKH,
        fibers: &[Fiber],
        effects: &HashMap<StrId, EffectKind>,
    ) {
        self.mark(interpreter, fibers, effects);
        self.sweep(interpreter);
        self.alloc_snapshot = interpreter.total_allocations();
        self.stats.collections += 1;
    }

    fn mark(
        &mut self,
        interpreter: &mut CEKH,
        fibers: &[Fiber],
        effects: &HashMap<StrId, EffectKind>,
    ) {
        self.work_list.clear();

        // Grant names must survive collection even when no fiber currently
        // references them (e.g. a grant with no pending perform).
        for key in effects.keys() {
            self.work_list.push(WorkItem::Value(JSValue::String(*key)));
        }
        let control = interpreter.control.clone();
        let env = interpreter.env;
        let cont = interpreter.cont;
        match &control {
            Control::Value(v) | Control::Returning(v) | Control::Halted(v) => {
                self.work_list.push(WorkItem::Value(*v));
            }
            Control::Suspend { args, .. } => {
                for arg in args {
                    self.work_list.push(WorkItem::Value(*arg));
                }
            }
            Control::Expr(_) | Control::Stmt(_) => {}
        }

        self.work_list.push(WorkItem::Env(env));
        self.work_list.push(WorkItem::Cont(cont));

        // `var` declarations and assignments to undeclared names live only
        // in the globals map — it is a root set of its own.
        for value in interpreter.globals.values() {
            self.work_list.push(WorkItem::Value(*value));
        }

        for fiber in fibers {
            match &fiber.control {
                Control::Value(v) | Control::Returning(v) | Control::Halted(v) => {
                    self.work_list.push(WorkItem::Value(*v));
                }
                Control::Suspend { args, .. } => {
                    for arg in args {
                        self.work_list.push(WorkItem::Value(*arg));
                    }
                }
                Control::Expr(_) | Control::Stmt(_) => {}
            }

            self.work_list.push(WorkItem::Env(fiber.env));
            self.work_list.push(WorkItem::Cont(fiber.cont));

            // A completed fiber's result (until joined) and a blocked fiber's
            // effect args live only in its status.
            match &fiber.status {
                FiberStatus::Completed(value) => {
                    self.work_list.push(WorkItem::Value(*value));
                }
                FiberStatus::Blocked { args, .. } => {
                    for arg in args {
                        self.work_list.push(WorkItem::Value(*arg));
                    }
                }
                FiberStatus::BlockedOnHost { effect, args } => {
                    // The effect name and args live only in the status while
                    // the fiber waits for the host; without these roots a GC
                    // could sweep them before the host reads the call.
                    self.work_list
                        .push(WorkItem::Value(JSValue::String(*effect)));
                    for arg in args {
                        self.work_list.push(WorkItem::Value(*arg));
                    }
                }
                FiberStatus::Ready | FiberStatus::Running | FiberStatus::Failed(_) => {}
            }
        }

        while let Some(item) = self.work_list.pop() {
            match item {
                WorkItem::Value(value) => self.trace_value(value, interpreter),
                WorkItem::Env(env) => self.trace_env(env, interpreter),
                WorkItem::Cont(cont) => self.trace_cont(cont, interpreter),
            }
        }
    }

    fn sweep(&mut self, interpreter: &mut CEKH) {
        interpreter.objects.sweep();
        interpreter.envs.sweep();
        interpreter.conts.sweep();
        interpreter.handlers.sweep();
        // Strings are interned forever for now
    }

    fn trace_value(&mut self, value: JSValue, interpreter: &mut CEKH) {
        match value {
            JSValue::String(str_id) => {
                self.mark_string(str_id, interpreter);
            }
            JSValue::Object(obj_id) | JSValue::Array(obj_id) | JSValue::Function(obj_id) => {
                self.trace_object(obj_id, interpreter);
            }
            JSValue::Handler(handler_id) => {
                self.trace_handler(handler_id, interpreter);
            }
            JSValue::Continuation(cont_id, env_id) => {
                self.work_list.push(WorkItem::Env(env_id));
                self.work_list.push(WorkItem::Cont(cont_id));
            }
            JSValue::Undefined
            | JSValue::Null
            | JSValue::Bool(_)
            | JSValue::Int(_)
            | JSValue::Float(_) => {}
        }
    }

    fn trace_env(&mut self, env_id: EnvId, interpreter: &mut CEKH) {
        if interpreter.envs.is_marked(env_id) {
            return;
        }

        interpreter.envs.mark(env_id);
        let env = interpreter.envs.get(env_id).expect("Env not found");
        if let Some(parent) = env.parent() {
            self.work_list.push(WorkItem::Env(parent));
        }

        for (_, value) in env.iter_bindings() {
            self.work_list.push(WorkItem::Value(value));
        }
    }

    fn trace_cont(&mut self, cont_id: ContId, interpreter: &mut CEKH) {
        if interpreter.conts.is_marked(cont_id) {
            return;
        }
        interpreter.conts.mark(cont_id);

        let Some(kont) = interpreter.conts.get(cont_id).cloned() else {
            return;
        };

        use crate::cont::Kont;
        match kont {
            Kont::Halt => {}

            Kont::UnaryK { k, .. } => {
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::BinaryLeftK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::BinaryRightK { left, k, .. } => {
                self.work_list.push(WorkItem::Value(left));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AndK { env, k, .. }
            | Kont::OrK { env, k, .. }
            | Kont::NullishK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::MemberK { k, .. } => {
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::IndexObjK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::IndexKeyK { obj, k } => {
                self.work_list.push(WorkItem::Value(obj));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignVarK { env, k, .. } | Kont::UpdateVarK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignMemberObjK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignMemberValK { obj, k, .. } => {
                self.work_list.push(WorkItem::Value(obj));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignIndexObjK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignIndexKeyK { obj, env, k, .. } => {
                self.work_list.push(WorkItem::Value(obj));
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::AssignIndexValK { obj, key, k } => {
                self.work_list.push(WorkItem::Value(obj));
                self.work_list.push(WorkItem::Value(key));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::IfK { env, k, .. }
            | Kont::CondK { env, k, .. }
            | Kont::WhileK { env, k, .. }
            | Kont::WhileBodyK { env, k, .. }
            | Kont::ForTestK { env, k, .. }
            | Kont::ForTestResultK { env, k, .. }
            | Kont::ForBodyK { env, k, .. }
            | Kont::ForUpdateK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::CalleeK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ArgsK {
                callee,
                done,
                env,
                k,
                ..
            } => {
                self.work_list.push(WorkItem::Value(callee));
                for v in done {
                    self.work_list.push(WorkItem::Value(v));
                }
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ReturnK { env, k } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ReturnExprK { k } => {
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ArrayK { done, env, k, .. } => {
                for v in done {
                    self.work_list.push(WorkItem::Value(v));
                }
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ObjectK { done, env, k, .. } => {
                for (_, v) in done {
                    self.work_list.push(WorkItem::Value(v));
                }
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::LetK { env, k, .. } | Kont::ConstK { env, k, .. } | Kont::VarK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::SeqK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::ExprStmtK { k } => {
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::BlockK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::MatchK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::HandlerK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::PerformArgsK { done, env, k, .. } => {
                for v in done {
                    self.work_list.push(WorkItem::Value(v));
                }
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }

            Kont::HandleWithK { env, k, .. } => {
                self.work_list.push(WorkItem::Env(env));
                self.work_list.push(WorkItem::Cont(k));
            }
        }
    }

    fn trace_object(&mut self, obj_id: ObjId, interpreter: &mut CEKH) {
        if interpreter.objects.is_marked(obj_id.into_arena_id()) {
            return;
        }
        interpreter.objects.mark(obj_id.into_arena_id());

        let Some(obj) = interpreter.objects.get(obj_id.into_arena_id()) else {
            return;
        };

        // Collect values to trace (to avoid borrow issues)
        let prop_values: Vec<JSValue> = obj.properties.values().map(|p| p.value).collect();
        let prototype = obj.prototype;

        use crate::object::ObjectKind;
        let kind_data: Vec<WorkItem> = match &obj.kind {
            ObjectKind::Ordinary => vec![],
            ObjectKind::Array(arr) => arr.elements.iter().map(|v| WorkItem::Value(*v)).collect(),
            ObjectKind::Function(func) => {
                vec![WorkItem::Env(func.env)]
            }
            ObjectKind::BoundFunction(bound) => {
                let mut items = vec![
                    WorkItem::Value(JSValue::Function(bound.target)),
                    WorkItem::Value(bound.this_arg),
                ];
                for arg in &bound.bound_args {
                    items.push(WorkItem::Value(*arg));
                }
                items
            }
            ObjectKind::NativeFunction(_) => vec![],
        };

        // Now push to worklist (no more borrow on interpreter)
        for v in prop_values {
            self.work_list.push(WorkItem::Value(v));
        }
        if let Some(proto) = prototype {
            self.work_list.push(WorkItem::Value(JSValue::Object(proto)));
        }
        for item in kind_data {
            self.work_list.push(item);
        }
    }

    fn trace_handler(&mut self, handler_id: HandlerId, interpreter: &mut CEKH) {
        if interpreter.handlers.is_marked(handler_id) {
            return;
        }
        interpreter.handlers.mark(handler_id);

        let Some(handler) = interpreter.handlers.get(handler_id) else {
            return;
        };
        let env = handler.env;

        self.work_list.push(WorkItem::Env(env));
    }

    fn mark_string(&mut self, str_id: StrId, interpreter: &mut CEKH) {
        interpreter.strings.mark(str_id);
    }
}

impl Default for GC {
    fn default() -> Self {
        Self::new()
    }
}
