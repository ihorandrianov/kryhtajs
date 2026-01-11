//! Continuation - explicit "what to do next" for the CEKH machine
//!
//! Each continuation frame represents a pending computation.
//! The continuation stack makes control flow explicit.

use crate::arena::{Arena, ArenaId};
use crate::ast::{BinaryOp, ExprId, StmtId, UnaryOp};
use crate::env::EnvId;
use crate::string_pool::StrId;
use crate::value::JSValue;

/// Continuation ID - index into the continuation arena
pub type ContId = ArenaId<Kont>;

/// Continuation frame - what to do when we have a value
#[derive(Clone, Debug, PartialEq)]
pub enum Kont {
    /// Program complete - return final value
    Halt,

    // Unary/Binary Operators
    /// Unary operator waiting for operand
    UnaryK {
        op: UnaryOp,
        k: ContId,
    },

    /// Binary operator - evaluated left, waiting for right
    BinaryLeftK {
        op: BinaryOp,
        right: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// Binary operator - have both values, apply operator
    BinaryRightK {
        op: BinaryOp,
        left: JSValue,
        k: ContId,
    },

    // Short-Circuit Operators
    /// && - short-circuit if left is falsy
    AndK {
        right: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// || - short-circuit if left is truthy
    OrK {
        right: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// ?? - short-circuit if left is not nullish
    NullishK {
        right: ExprId,
        env: EnvId,
        k: ContId,
    },

    //Property/Index Access    /// obj.prop - have obj, access property
    MemberK {
        property: StrId,
        k: ContId,
    },

    /// obj[idx] - evaluating obj
    IndexObjK {
        index: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// obj[idx] - have obj, evaluating index
    IndexKeyK {
        obj: JSValue,
        k: ContId,
    },

    //Assignment    /// x = expr - evaluating expr
    AssignVarK {
        name: StrId,
        env: EnvId,
        k: ContId,
    },

    /// obj.prop = expr - evaluating obj
    AssignMemberObjK {
        property: StrId,
        value: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// obj.prop = expr - have obj, evaluating value
    AssignMemberValK {
        obj: JSValue,
        property: StrId,
        k: ContId,
    },

    /// obj[key] = expr - evaluating obj
    AssignIndexObjK {
        index: ExprId,
        value: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// obj[key] = expr - have obj, evaluating key
    AssignIndexKeyK {
        obj: JSValue,
        value: ExprId,
        env: EnvId,
        k: ContId,
    },

    /// obj[key] = expr - have obj and key, evaluating value
    AssignIndexValK {
        obj: JSValue,
        key: JSValue,
        k: ContId,
    },

    //Conditionals    /// if (test) { consequent } else { alternate }
    IfK {
        consequent: StmtId,
        alternate: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// test ? consequent : alternate
    CondK {
        consequent: ExprId,
        alternate: ExprId,
        env: EnvId,
        k: ContId,
    },

    //Loops    /// while (test) { body } - loop marker for break/continue
    WhileK {
        test: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// while body evaluated, continue to test
    WhileBodyK {
        test: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// for loop - evaluating test
    ForTestK {
        test: ExprId,
        update: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// for loop - test result check
    ForTestResultK {
        test: ExprId,
        update: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// for loop - evaluating body
    ForBodyK {
        test: ExprId,
        update: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// for loop - evaluating update
    ForUpdateK {
        test: ExprId,
        update: ExprId,
        body: StmtId,
        env: EnvId,
        k: ContId,
    },

    //Function Calls    /// Evaluating callee - args to evaluate after
    CalleeK {
        args_start: u32,
        args_count: u16,
        env: EnvId,
        k: ContId,
    },

    /// Evaluating arguments - have callee, collecting args
    ArgsK {
        callee: JSValue,
        done: Vec<JSValue>,
        args_start: u32,
        args_idx: u16,
        args_count: u16,
        env: EnvId,
        k: ContId,
    },

    /// Return point - restore caller environment
    ReturnK {
        env: EnvId,
        k: ContId,
    },

    /// Return expression - convert value to Returning control
    ReturnExprK {
        k: ContId,
    },

    // Literals
    /// Array literal - evaluating elements
    ArrayK {
        done: Vec<JSValue>,
        elems_start: u32,
        elems_idx: u16,
        elems_count: u16,
        env: EnvId,
        k: ContId,
    },

    /// Object literal - evaluating property values
    ObjectK {
        done: Vec<(StrId, JSValue)>,
        props_start: u32,
        props_idx: u16,
        props_count: u16,
        env: EnvId,
        k: ContId,
    },

    //Variable Binding    /// let x = init
    LetK {
        name: StrId,
        env: EnvId,
        k: ContId,
    },

    /// const x = init
    ConstK {
        name: StrId,
        env: EnvId,
        k: ContId,
    },

    /// var x = init
    VarK {
        name: StrId,
        env: EnvId,
        k: ContId,
    },

    //Statement Sequences    /// Block/sequence of statements
    SeqK {
        stmts_start: u32,
        stmts_idx: u32,
        stmts_count: u32,
        env: EnvId,
        k: ContId,
    },

    /// Expression statement - discard value
    ExprStmtK {
        k: ContId,
    },

    //Error Handling    /// try { body } catch (e) { handler } finally { finally }
    TryK {
        catch_param: StrId,
        catch_body: StmtId,
        finally_body: StmtId,
        env: EnvId,
        k: ContId,
    },

    /// Finally clause pending
    FinallyK {
        finally_body: StmtId,
        result: Option<JSValue>,
        thrown: Option<JSValue>,
        env: EnvId,
        k: ContId,
    },

    /// Throw expression - evaluating value to throw
    ThrowK {
        k: ContId,
    },

    //Block Expression    /// Block expression - evaluating statements, then final expr
    BlockK {
        stmts_start: u32,
        stmts_idx: u32,
        stmts_count: u32,
        final_expr: ExprId,
        env: EnvId,
        k: ContId,
    },

    //Pattern Matching    /// Match expression - trying arms in order
    MatchK {
        arms_start: u32,
        arms_idx: u16,
        arms_count: u16,
        env: EnvId,
        k: ContId,
    },

    //  Effect Handlers
    /// Handler frame - marks handler boundary
    HandlerK {
        clauses_start: u32,
        clauses_count: u16,
        env: EnvId,
        return_body: ExprId,
        return_param: StrId,
        k: ContId,
    },

    PerformArgsK {
        effect: StrId,
        done: Vec<JSValue>,
        args_start: u32,
        args_idx: u16,
        args_count: u16,
        env: EnvId,
        k: ContId,
    },
}

impl Kont {
    /// Get the outer continuation (for unwinding)
    pub fn outer(&self) -> Option<ContId> {
        match self {
            Kont::Halt => None,
            Kont::UnaryK { k, .. }
            | Kont::BinaryLeftK { k, .. }
            | Kont::BinaryRightK { k, .. }
            | Kont::AndK { k, .. }
            | Kont::OrK { k, .. }
            | Kont::NullishK { k, .. }
            | Kont::MemberK { k, .. }
            | Kont::IndexObjK { k, .. }
            | Kont::IndexKeyK { k, .. }
            | Kont::AssignVarK { k, .. }
            | Kont::AssignMemberObjK { k, .. }
            | Kont::AssignMemberValK { k, .. }
            | Kont::AssignIndexObjK { k, .. }
            | Kont::AssignIndexKeyK { k, .. }
            | Kont::AssignIndexValK { k, .. }
            | Kont::IfK { k, .. }
            | Kont::CondK { k, .. }
            | Kont::WhileK { k, .. }
            | Kont::WhileBodyK { k, .. }
            | Kont::ForTestK { k, .. }
            | Kont::ForTestResultK { k, .. }
            | Kont::ForBodyK { k, .. }
            | Kont::ForUpdateK { k, .. }
            | Kont::CalleeK { k, .. }
            | Kont::ArgsK { k, .. }
            | Kont::ReturnK { k, .. }
            | Kont::ReturnExprK { k, .. }
            | Kont::ArrayK { k, .. }
            | Kont::ObjectK { k, .. }
            | Kont::LetK { k, .. }
            | Kont::ConstK { k, .. }
            | Kont::VarK { k, .. }
            | Kont::SeqK { k, .. }
            | Kont::ExprStmtK { k, .. }
            | Kont::TryK { k, .. }
            | Kont::FinallyK { k, .. }
            | Kont::ThrowK { k, .. }
            | Kont::BlockK { k, .. }
            | Kont::MatchK { k, .. }
            | Kont::HandlerK { k, .. }
            | Kont::PerformArgsK { k, .. } => Some(*k),
        }
    }

    /// Check if this is a loop continuation (for break/continue)
    pub fn is_loop(&self) -> bool {
        matches!(
            self,
            Kont::WhileK { .. }
                | Kont::WhileBodyK { .. }
                | Kont::ForTestK { .. }
                | Kont::ForTestResultK { .. }
                | Kont::ForBodyK { .. }
                | Kont::ForUpdateK { .. }
        )
    }
}

/// Continuation arena
pub struct ContArena {
    arena: Arena<Kont>,
    halt: ContId,
}

impl ContArena {
    pub fn new() -> Self {
        let mut arena = Arena::new();
        let halt = arena.alloc(Kont::Halt);
        Self { arena, halt }
    }

    /// Get the halt continuation
    pub fn halt(&self) -> ContId {
        self.halt
    }

    /// Allocate a new continuation
    pub fn alloc(&mut self, kont: Kont) -> ContId {
        self.arena.alloc(kont)
    }

    /// Get a continuation
    pub fn get(&self, id: ContId) -> Option<&Kont> {
        self.arena.get(id)
    }

    /// Get mutable continuation
    pub fn get_mut(&mut self, id: ContId) -> Option<&mut Kont> {
        self.arena.get_mut(id)
    }
}

impl Default for ContArena {
    fn default() -> Self {
        Self::new()
    }
}
