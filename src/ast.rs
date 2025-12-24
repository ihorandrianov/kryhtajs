//! Arena-based AST for no_std parsing

use crate::fixed_collections::FixedVec;
use crate::fixed_string::StrId;

pub const MAX_AST_NODES: usize = 512;
pub const MAX_LIST_ITEMS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExprId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StmtId(pub u16);

impl ExprId {
    pub const NONE: ExprId = ExprId(u16::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u16::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u16::MAX
    }
}

impl StmtId {
    pub const NONE: StmtId = StmtId(u16::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u16::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u16::MAX
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    NullishCoalesce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    TypeOf,
    Plus,
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Debug, Clone, Copy)]
pub enum Expr {
    Empty,
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Float(u16),
    String(StrId),
    Identifier(StrId),
    Binary {
        op: BinaryOp,
        left: ExprId,
        right: ExprId,
    },
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Call {
        callee: ExprId,
        args_start: u16,
        args_count: u8,
    },
    Member {
        object: ExprId,
        property: StrId,
    },
    Index {
        object: ExprId,
        index: ExprId,
    },
    Assign {
        target: ExprId,
        value: ExprId,
    },
    Conditional {
        test: ExprId,
        consequent: ExprId,
        alternate: ExprId,
    },
    Array {
        elems_start: u16,
        elems_count: u8,
    },
    Object {
        props_start: u16,
        props_count: u8,
    },
    Function {
        name: StrId,
        params_start: u16,
        params_count: u8,
        body: StmtId,
    },
    Arrow {
        params_start: u16,
        params_count: u8,
        body: ExprId,
        is_block: bool,
    },
    This,
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Empty
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PropEntry {
    pub key: StrId,
    pub value: ExprId,
}

#[derive(Debug, Clone, Copy)]
pub enum Stmt {
    Empty,
    Expr(ExprId),
    Let { name: StrId, init: ExprId },
    Const { name: StrId, init: ExprId },
    If { test: ExprId, consequent: StmtId, alternate: StmtId },
    While { test: ExprId, body: StmtId },
    For { init: StmtId, test: ExprId, update: ExprId, body: StmtId },
    Block { stmts_start: u16, stmts_count: u16 },
    Return(ExprId),
    Break,
    Continue,
    Throw(ExprId),
    Try { body: StmtId, catch_param: StrId, catch_body: StmtId, finally_body: StmtId },
    Function { name: StrId, params_start: u16, params_count: u8, body: StmtId },
}

impl Default for Stmt {
    fn default() -> Self {
        Stmt::Empty
    }
}

pub struct AstArena {
    pub exprs: FixedVec<Expr, MAX_AST_NODES>,
    pub stmts: FixedVec<Stmt, MAX_AST_NODES>,
    pub expr_lists: FixedVec<ExprId, MAX_LIST_ITEMS>,
    pub stmt_lists: FixedVec<StmtId, MAX_LIST_ITEMS>,
    pub param_lists: FixedVec<StrId, MAX_LIST_ITEMS>,
    pub prop_lists: FixedVec<PropEntry, MAX_LIST_ITEMS>,
    pub floats: FixedVec<f64, 128>,
    pub root_start: u16,
    pub root_count: u16,
}

impl AstArena {
    pub fn new() -> Self {
        Self {
            exprs: FixedVec::new(),
            stmts: FixedVec::new(),
            expr_lists: FixedVec::new(),
            stmt_lists: FixedVec::new(),
            param_lists: FixedVec::new(),
            prop_lists: FixedVec::new(),
            floats: FixedVec::new(),
            root_start: 0,
            root_count: 0,
        }
    }

    pub fn alloc_expr(&mut self, expr: Expr) -> Option<ExprId> {
        let id = self.exprs.len() as u16;
        if self.exprs.push(expr) {
            Some(ExprId(id))
        } else {
            None
        }
    }

    pub fn alloc_stmt(&mut self, stmt: Stmt) -> Option<StmtId> {
        let id = self.stmts.len() as u16;
        if self.stmts.push(stmt) {
            Some(StmtId(id))
        } else {
            None
        }
    }

    pub fn get_expr(&self, id: ExprId) -> Option<Expr> {
        if id.is_none() {
            return None;
        }
        self.exprs.get(id.0 as usize)
    }

    pub fn get_stmt(&self, id: StmtId) -> Option<Stmt> {
        if id.is_none() {
            return None;
        }
        self.stmts.get(id.0 as usize)
    }

    pub fn add_float(&mut self, f: f64) -> Option<u16> {
        for (i, existing) in self.floats.iter().enumerate() {
            if existing == f || (existing.is_nan() && f.is_nan()) {
                return Some(i as u16);
            }
        }
        let id = self.floats.len() as u16;
        if self.floats.push(f) {
            Some(id)
        } else {
            None
        }
    }

    pub fn start_expr_list(&self) -> u16 { self.expr_lists.len() as u16 }

    pub fn push_expr_list(&mut self, expr: ExprId) -> Option<()> {
        if self.expr_lists.push(expr) { Some(()) } else { None }
    }

    pub fn get_expr_list(&self, start: u16, count: u8) -> &[ExprId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.expr_lists.len() { &self.expr_lists.as_slice()[s..e] } else { &[] }
    }

    pub fn start_stmt_list(&self) -> u16 { self.stmt_lists.len() as u16 }

    pub fn push_stmt_list(&mut self, stmt: StmtId) -> Option<()> {
        if self.stmt_lists.push(stmt) { Some(()) } else { None }
    }

    pub fn get_stmt_list(&self, start: u16, count: u16) -> &[StmtId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.stmt_lists.len() { &self.stmt_lists.as_slice()[s..e] } else { &[] }
    }

    pub fn start_param_list(&self) -> u16 { self.param_lists.len() as u16 }

    pub fn push_param_list(&mut self, name: StrId) -> Option<()> {
        if self.param_lists.push(name) { Some(()) } else { None }
    }

    pub fn get_param_list(&self, start: u16, count: u8) -> &[StrId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.param_lists.len() { &self.param_lists.as_slice()[s..e] } else { &[] }
    }

    pub fn start_prop_list(&self) -> u16 { self.prop_lists.len() as u16 }

    pub fn push_prop_list(&mut self, key: StrId, value: ExprId) -> Option<()> {
        if self.prop_lists.push(PropEntry { key, value }) { Some(()) } else { None }
    }

    pub fn get_prop_list(&self, start: u16, count: u8) -> &[PropEntry] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.prop_lists.len() { &self.prop_lists.as_slice()[s..e] } else { &[] }
    }

    pub fn clear(&mut self) {
        self.exprs.clear();
        self.stmts.clear();
        self.expr_lists.clear();
        self.stmt_lists.clear();
        self.param_lists.clear();
        self.prop_lists.clear();
        self.floats.clear();
        self.root_start = 0;
        self.root_count = 0;
    }

    pub fn stats(&self) -> AstStats {
        AstStats {
            exprs_used: self.exprs.len(),
            exprs_capacity: MAX_AST_NODES,
            stmts_used: self.stmts.len(),
            stmts_capacity: MAX_AST_NODES,
            lists_used: self.expr_lists.len() + self.stmt_lists.len(),
            lists_capacity: MAX_LIST_ITEMS * 2,
        }
    }
}

impl Default for AstArena {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct AstStats {
    pub exprs_used: usize,
    pub exprs_capacity: usize,
    pub stmts_used: usize,
    pub stmts_capacity: usize,
    pub lists_used: usize,
    pub lists_capacity: usize,
}
