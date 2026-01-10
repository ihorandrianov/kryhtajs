//! Arena-based AST

use crate::string_pool::StrId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StmtId(pub u32);

impl ExprId {
    pub const NONE: ExprId = ExprId(u32::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl StmtId {
    pub const NONE: StmtId = StmtId(u32::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PatternId(pub u32);

impl PatternId {
    pub const NONE: PatternId = PatternId(u32::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Pattern for pattern matching
#[derive(Debug, Clone, Copy)]
pub enum Pattern {
    /// Wildcard pattern: _
    Wildcard,
    /// Literal pattern: 1, "hello", true, null, undefined
    Literal(ExprId),
    /// Variable binding: x (binds matched value to name)
    Var(StrId),
    /// Array destructuring: [a, b, c]
    Array {
        elems_start: u32,
        elems_count: u16,
    },
    /// Object destructuring: {x, y: pat}
    Object {
        fields_start: u32,
        fields_count: u16,
    },
}

impl Default for Pattern {
    fn default() -> Self {
        Pattern::Wildcard
    }
}

/// Object pattern field: key: pattern or shorthand key
#[derive(Debug, Clone, Copy, Default)]
pub struct PatternField {
    pub key: StrId,
    pub pattern: PatternId,
}

/// Match arm: pattern => body
#[derive(Debug, Clone, Copy, Default)]
pub struct MatchArm {
    pub pattern: PatternId,
    pub guard: ExprId,  // ExprId::NONE if no guard
    pub body: ExprId,
}

/// Effect handler clause: EffectName!(params, resume) => body
#[derive(Debug, Clone, Copy, Default)]
pub struct EffectClause {
    pub effect: StrId,           // effect name (e.g., "Get")
    pub params_start: u32,       // parameters including resume as last
    pub params_count: u16,
    pub body: ExprId,
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
    Float(u32),
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
        args_start: u32,
        args_count: u16,
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
        elems_start: u32,
        elems_count: u16,
    },
    Object {
        props_start: u32,
        props_count: u16,
    },
    Function {
        name: StrId,
        params_start: u32,
        params_count: u16,
        body: StmtId,
    },
    Arrow {
        params_start: u32,
        params_count: u16,
        body: ExprId,
        is_block: bool,
    },
    This,
    /// Block expression: { stmts; final_expr }
    /// Evaluates statements in order, returns final expression's value
    Block {
        stmts_start: u32,
        stmts_count: u32,
        final_expr: ExprId,  // ExprId::NONE means undefined
    },
    /// Match expression: match (scrutinee) { arms }
    Match {
        scrutinee: ExprId,
        arms_start: u32,
        arms_count: u16,
    },
    /// Perform effect: perform EffectName!(args)
    Perform {
        effect: StrId,
        args_start: u32,
        args_count: u16,
    },
    /// Handle expression: handle { body } with { clauses }
    Handle {
        body: ExprId,
        clauses_start: u32,
        clauses_count: u16,
        return_param: StrId,    // parameter name for return clause (StrId::NONE if no return)
        return_body: ExprId,    // ExprId::NONE if no return clause
    },
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
    Var { name: StrId, init: ExprId },
    If { test: ExprId, consequent: StmtId, alternate: StmtId },
    While { test: ExprId, body: StmtId },
    For { init: StmtId, test: ExprId, update: ExprId, body: StmtId },
    Block { stmts_start: u32, stmts_count: u32 },
    Declarations { stmts_start: u32, stmts_count: u32 },
    Return(ExprId),
    Break,
    Continue,
    Throw(ExprId),
    Try { body: StmtId, catch_param: StrId, catch_body: StmtId, finally_body: StmtId },
    Function { name: StrId, params_start: u32, params_count: u16, body: StmtId },
}

impl Default for Stmt {
    fn default() -> Self {
        Stmt::Empty
    }
}

pub struct AstArena {
    pub exprs: Vec<Expr>,
    pub stmts: Vec<Stmt>,
    pub patterns: Vec<Pattern>,
    pub expr_lists: Vec<ExprId>,
    pub stmt_lists: Vec<StmtId>,
    pub param_lists: Vec<StrId>,
    pub prop_lists: Vec<PropEntry>,
    pub pattern_lists: Vec<PatternId>,
    pub pattern_fields: Vec<PatternField>,
    pub arms: Vec<MatchArm>,
    pub effect_clauses: Vec<EffectClause>,
    pub floats: Vec<f64>,
    pub root_start: u32,
    pub root_count: u32,
}

impl AstArena {
    pub fn new() -> Self {
        Self {
            exprs: Vec::new(),
            stmts: Vec::new(),
            patterns: Vec::new(),
            expr_lists: Vec::new(),
            stmt_lists: Vec::new(),
            param_lists: Vec::new(),
            prop_lists: Vec::new(),
            pattern_lists: Vec::new(),
            pattern_fields: Vec::new(),
            arms: Vec::new(),
            effect_clauses: Vec::new(),
            floats: Vec::new(),
            root_start: 0,
            root_count: 0,
        }
    }

    pub fn alloc_expr(&mut self, expr: Expr) -> ExprId {
        let id = self.exprs.len() as u32;
        self.exprs.push(expr);
        ExprId(id)
    }

    pub fn alloc_stmt(&mut self, stmt: Stmt) -> StmtId {
        let id = self.stmts.len() as u32;
        self.stmts.push(stmt);
        StmtId(id)
    }

    pub fn get_expr(&self, id: ExprId) -> Option<Expr> {
        if id.is_none() {
            return None;
        }
        self.exprs.get(id.index()).copied()
    }

    pub fn get_stmt(&self, id: StmtId) -> Option<Stmt> {
        if id.is_none() {
            return None;
        }
        self.stmts.get(id.index()).copied()
    }

    pub fn add_float(&mut self, f: f64) -> u32 {
        for (i, existing) in self.floats.iter().enumerate() {
            if *existing == f || (existing.is_nan() && f.is_nan()) {
                return i as u32;
            }
        }
        let id = self.floats.len() as u32;
        self.floats.push(f);
        id
    }

    pub fn get_float(&self, idx: u32) -> Option<f64> {
        self.floats.get(idx as usize).copied()
    }

    pub fn start_expr_list(&self) -> u32 {
        self.expr_lists.len() as u32
    }

    pub fn push_expr_list(&mut self, expr: ExprId) {
        self.expr_lists.push(expr);
    }

    pub fn get_expr_list(&self, start: u32, count: u16) -> &[ExprId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.expr_lists.len() {
            &self.expr_lists[s..e]
        } else {
            &[]
        }
    }

    pub fn start_stmt_list(&self) -> u32 {
        self.stmt_lists.len() as u32
    }

    pub fn push_stmt_list(&mut self, stmt: StmtId) {
        self.stmt_lists.push(stmt);
    }

    pub fn get_stmt_list(&self, start: u32, count: u32) -> &[StmtId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.stmt_lists.len() {
            &self.stmt_lists[s..e]
        } else {
            &[]
        }
    }

    pub fn start_param_list(&self) -> u32 {
        self.param_lists.len() as u32
    }

    pub fn push_param_list(&mut self, name: StrId) {
        self.param_lists.push(name);
    }

    pub fn get_param_list(&self, start: u32, count: u16) -> &[StrId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.param_lists.len() {
            &self.param_lists[s..e]
        } else {
            &[]
        }
    }

    pub fn start_prop_list(&self) -> u32 {
        self.prop_lists.len() as u32
    }

    pub fn push_prop_list(&mut self, key: StrId, value: ExprId) {
        self.prop_lists.push(PropEntry { key, value });
    }

    pub fn get_prop_list(&self, start: u32, count: u16) -> &[PropEntry] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.prop_lists.len() {
            &self.prop_lists[s..e]
        } else {
            &[]
        }
    }

    // Pattern methods
    pub fn alloc_pattern(&mut self, pattern: Pattern) -> PatternId {
        let id = self.patterns.len() as u32;
        self.patterns.push(pattern);
        PatternId(id)
    }

    pub fn get_pattern(&self, id: PatternId) -> Option<Pattern> {
        if id.is_none() {
            return None;
        }
        self.patterns.get(id.index()).copied()
    }

    pub fn start_pattern_list(&self) -> u32 {
        self.pattern_lists.len() as u32
    }

    pub fn push_pattern_list(&mut self, pattern: PatternId) {
        self.pattern_lists.push(pattern);
    }

    pub fn get_pattern_list(&self, start: u32, count: u16) -> &[PatternId] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.pattern_lists.len() {
            &self.pattern_lists[s..e]
        } else {
            &[]
        }
    }

    pub fn start_pattern_field_list(&self) -> u32 {
        self.pattern_fields.len() as u32
    }

    pub fn push_pattern_field(&mut self, key: StrId, pattern: PatternId) {
        self.pattern_fields.push(PatternField { key, pattern });
    }

    pub fn get_pattern_field_list(&self, start: u32, count: u16) -> &[PatternField] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.pattern_fields.len() {
            &self.pattern_fields[s..e]
        } else {
            &[]
        }
    }

    // Match arm methods
    pub fn start_arm_list(&self) -> u32 {
        self.arms.len() as u32
    }

    pub fn push_arm(&mut self, pattern: PatternId, guard: ExprId, body: ExprId) {
        self.arms.push(MatchArm { pattern, guard, body });
    }

    pub fn get_arm_list(&self, start: u32, count: u16) -> &[MatchArm] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.arms.len() {
            &self.arms[s..e]
        } else {
            &[]
        }
    }

    // Effect clause methods
    pub fn start_effect_clause_list(&self) -> u32 {
        self.effect_clauses.len() as u32
    }

    pub fn push_effect_clause(&mut self, effect: StrId, params_start: u32, params_count: u16, body: ExprId) {
        self.effect_clauses.push(EffectClause { effect, params_start, params_count, body });
    }

    pub fn get_effect_clause_list(&self, start: u32, count: u16) -> &[EffectClause] {
        let s = start as usize;
        let e = s + count as usize;
        if e <= self.effect_clauses.len() {
            &self.effect_clauses[s..e]
        } else {
            &[]
        }
    }

    pub fn clear(&mut self) {
        self.exprs.clear();
        self.stmts.clear();
        self.patterns.clear();
        self.expr_lists.clear();
        self.stmt_lists.clear();
        self.param_lists.clear();
        self.prop_lists.clear();
        self.pattern_lists.clear();
        self.pattern_fields.clear();
        self.arms.clear();
        self.effect_clauses.clear();
        self.floats.clear();
        self.root_start = 0;
        self.root_count = 0;
    }
}

impl Default for AstArena {
    fn default() -> Self {
        Self::new()
    }
}
