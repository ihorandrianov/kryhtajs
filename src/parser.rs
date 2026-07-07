//! JavaScript parser

use crate::ast::{AstArena, BinaryOp, Expr, ExprId, Pattern, PatternId, Stmt, StmtId, UnaryOp};
use crate::error::{JSError, Result};
use crate::lexer::{Lexer, Token};
use crate::string_pool::{StrId, StringPool};

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
    arena: AstArena,
    strings: StringPool,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Result<Self> {
        Self::with_strings(source, StringPool::new())
    }

    pub fn with_strings(source: &'a str, strings: StringPool) -> Result<Self> {
        Self::with_state(source, strings, AstArena::new())
    }

    /// Parse into an existing arena so ids from earlier parses stay valid
    /// (functions defined in a previous eval keep working).
    pub fn with_state(source: &'a str, strings: StringPool, arena: AstArena) -> Result<Self> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self {
            lexer,
            current,
            arena,
            strings,
        })
    }

    fn advance(&mut self) -> Result<Token<'a>> {
        let prev = std::mem::replace(&mut self.current, self.lexer.next_token()?);
        Ok(prev)
    }

    fn check(&self, token: &Token) -> bool {
        std::mem::discriminant(&self.current) == std::mem::discriminant(token)
    }

    fn consume(&mut self, expected: Token) -> Result<()> {
        if !self.check(&expected) {
            return Err(JSError::syntax(
                "Unexpected token",
                self.lexer.line(),
                self.lexer.column(),
            ));
        }
        self.advance()?;
        Ok(())
    }

    pub fn parse_program(mut self) -> Result<(AstArena, StringPool)> {
        // Collect all root statements first, then push them to stmt_lists
        // This is needed because nested blocks also use stmt_lists during parsing
        let mut root_stmts = Vec::new();
        while !self.check(&Token::Eof) {
            let stmt = self.parse_statement()?;
            root_stmts.push(stmt);
        }
        let start = self.arena.start_stmt_list();
        for stmt in &root_stmts {
            self.arena.push_stmt_list(*stmt);
        }
        self.arena.root_start = start;
        self.arena.root_count = root_stmts.len() as u32;
        Ok((self.arena, self.strings))
    }

    fn parse_statement(&mut self) -> Result<StmtId> {
        match &self.current {
            Token::Let => self.parse_let(),
            Token::Const => self.parse_const(),
            Token::Var => self.parse_var(),
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::For => self.parse_for(),
            Token::Function => self.parse_function_decl(),
            Token::Return => self.parse_return(),
            Token::Break => {
                self.advance()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Ok(self.arena.alloc_stmt(Stmt::Break))
            }
            Token::Continue => {
                self.advance()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Ok(self.arena.alloc_stmt(Stmt::Continue))
            }
            // Exceptions are not part of the language: expected failures are
            // values ({ok}/{err} + match), recoverable ones are effects
            // (perform/handle), and faults kill the fiber (observed at Join).
            Token::Throw => Err(JSError::syntax(
                "'throw' is not supported — return an {err: ...} value or perform an effect",
                self.lexer.line(),
                self.lexer.column(),
            )),
            Token::Try => Err(JSError::syntax(
                "'try' is not supported — use 'handle { ... } with { ... }' for recoverable errors",
                self.lexer.line(),
                self.lexer.column(),
            )),
            Token::LBrace => self.parse_block(),
            Token::Semicolon => {
                self.advance()?;
                Ok(self.arena.alloc_stmt(Stmt::Empty))
            }
            _ => {
                let expr = self.parse_expression()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Ok(self.arena.alloc_stmt(Stmt::Expr(expr)))
            }
        }
    }

    fn parse_let(&mut self) -> Result<StmtId> {
        self.consume(Token::Let)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.strings.intern(s),
            _ => {
                return Err(JSError::syntax(
                    "Expected identifier",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        };
        let init = if self.check(&Token::Eq) {
            self.advance()?;
            self.parse_expression()?
        } else {
            ExprId::NONE
        };
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(self.arena.alloc_stmt(Stmt::Let { name, init }))
    }

    fn parse_const(&mut self) -> Result<StmtId> {
        self.consume(Token::Const)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.strings.intern(s),
            _ => {
                return Err(JSError::syntax(
                    "Expected identifier",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        };
        self.consume(Token::Eq)?;
        let init = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(self.arena.alloc_stmt(Stmt::Const { name, init }))
    }

    fn parse_var(&mut self) -> Result<StmtId> {
        self.consume(Token::Var)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.strings.intern(s),
            _ => {
                return Err(JSError::syntax(
                    "Expected identifier",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        };
        let init = if self.check(&Token::Eq) {
            self.advance()?;
            self.parse_expression()?
        } else {
            ExprId::NONE
        };
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(self.arena.alloc_stmt(Stmt::Var { name, init }))
    }

    fn parse_if(&mut self) -> Result<StmtId> {
        self.consume(Token::If)?;
        self.consume(Token::LParen)?;
        let test = self.parse_expression()?;
        self.consume(Token::RParen)?;
        let consequent = self.parse_statement()?;
        let alternate = if self.check(&Token::Else) {
            self.advance()?;
            self.parse_statement()?
        } else {
            StmtId::NONE
        };
        Ok(self.arena.alloc_stmt(Stmt::If {
            test,
            consequent,
            alternate,
        }))
    }

    fn parse_while(&mut self) -> Result<StmtId> {
        self.consume(Token::While)?;
        self.consume(Token::LParen)?;
        let test = self.parse_expression()?;
        self.consume(Token::RParen)?;
        let body = self.parse_statement()?;
        Ok(self.arena.alloc_stmt(Stmt::While { test, body }))
    }

    fn parse_for(&mut self) -> Result<StmtId> {
        self.consume(Token::For)?;
        self.consume(Token::LParen)?;

        let init = if self.check(&Token::Semicolon) {
            self.advance()?;
            StmtId::NONE
        } else if self.check(&Token::Let) {
            let stmt = self.parse_let()?;
            stmt
        } else if self.check(&Token::Var) {
            let stmt = self.parse_var()?;
            stmt
        } else {
            let expr = self.parse_expression()?;
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            self.arena.alloc_stmt(Stmt::Expr(expr))
        };

        let test = if self.check(&Token::Semicolon) {
            self.advance()?;
            ExprId::NONE
        } else {
            let e = self.parse_expression()?;
            self.consume(Token::Semicolon)?;
            e
        };

        let update = if self.check(&Token::RParen) {
            ExprId::NONE
        } else {
            self.parse_expression()?
        };
        self.consume(Token::RParen)?;

        let body = self.parse_statement()?;
        Ok(self.arena.alloc_stmt(Stmt::For {
            init,
            test,
            update,
            body,
        }))
    }

    fn parse_function_decl(&mut self) -> Result<StmtId> {
        self.consume(Token::Function)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.strings.intern(s),
            _ => {
                return Err(JSError::syntax(
                    "Expected function name",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        };
        let (params_start, params_count, body) = self.parse_function_params_body()?;
        Ok(self.arena.alloc_stmt(Stmt::Function {
            name,
            params_start,
            params_count,
            body,
        }))
    }

    fn parse_function_params_body(&mut self) -> Result<(u32, u16, StmtId)> {
        self.consume(Token::LParen)?;
        let params_start = self.arena.start_param_list();
        let mut params_count = 0u16;
        if !self.check(&Token::RParen) {
            loop {
                match self.advance()? {
                    Token::Identifier(s) => {
                        self.arena.push_param_list(self.strings.intern(s));
                        params_count += 1;
                    }
                    _ => {
                        return Err(JSError::syntax(
                            "Expected parameter name",
                            self.lexer.line(),
                            self.lexer.column(),
                        ));
                    }
                }
                if !self.check(&Token::Comma) {
                    break;
                }
                self.advance()?;
            }
        }
        self.consume(Token::RParen)?;
        let body = self.parse_block()?;
        Ok((params_start, params_count, body))
    }

    fn parse_return(&mut self) -> Result<StmtId> {
        self.consume(Token::Return)?;
        if self.check(&Token::Semicolon) || self.check(&Token::RBrace) || self.check(&Token::Eof) {
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            return Ok(self.arena.alloc_stmt(Stmt::Return(ExprId::NONE)));
        }
        let expr = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(self.arena.alloc_stmt(Stmt::Return(expr)))
    }

    fn parse_block(&mut self) -> Result<StmtId> {
        self.consume(Token::LBrace)?;
        // Collect statements first (nested blocks may also push to stmt_lists)
        let mut block_stmts = Vec::new();
        while !self.check(&Token::RBrace) {
            let stmt = self.parse_statement()?;
            block_stmts.push(stmt);
        }
        self.consume(Token::RBrace)?;
        let start = self.arena.start_stmt_list();
        for stmt in &block_stmts {
            self.arena.push_stmt_list(*stmt);
        }
        Ok(self.arena.alloc_stmt(Stmt::Block {
            stmts_start: start,
            stmts_count: block_stmts.len() as u32,
        }))
    }

    /// Parse a block expression: { stmts; final_expr }
    /// Like Rust: last expression without semicolon is the value
    fn parse_block_expr(&mut self) -> Result<ExprId> {
        self.consume(Token::LBrace)?;

        let mut stmts = Vec::new();
        let mut final_expr = ExprId::NONE;

        while !self.check(&Token::RBrace) {
            let is_stmt_keyword = matches!(
                &self.current,
                Token::Let
                    | Token::Const
                    | Token::Var
                    | Token::If
                    | Token::While
                    | Token::For
                    | Token::Function
                    | Token::Return
                    | Token::Break
                    | Token::Continue
                    | Token::Throw
                    | Token::Try
            );

            if is_stmt_keyword {
                let stmt = self.parse_statement()?;
                stmts.push(stmt);
            } else {
                let expr = self.parse_expression()?;

                if self.check(&Token::Semicolon) {
                    self.advance()?;
                    let stmt = self.arena.alloc_stmt(Stmt::Expr(expr));
                    stmts.push(stmt);
                } else if self.check(&Token::RBrace) {
                    final_expr = expr;
                } else {
                    let stmt = self.arena.alloc_stmt(Stmt::Expr(expr));
                    stmts.push(stmt);
                }
            }
        }

        self.consume(Token::RBrace)?;

        let stmts_start = self.arena.start_stmt_list();
        for stmt in &stmts {
            self.arena.push_stmt_list(*stmt);
        }

        Ok(self.arena.alloc_expr(Expr::Block {
            stmts_start,
            stmts_count: stmts.len() as u32,
            final_expr,
        }))
    }

    fn parse_expression(&mut self) -> Result<ExprId> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<ExprId> {
        let left = self.parse_ternary()?;

        if self.check(&Token::Eq) {
            self.advance()?;
            let right = self.parse_assignment()?;
            return Ok(self.arena.alloc_expr(Expr::Assign {
                target: left,
                value: right,
            }));
        }

        Ok(left)
    }

    fn parse_ternary(&mut self) -> Result<ExprId> {
        let test = self.parse_or()?;

        if self.check(&Token::Question) {
            self.advance()?;
            let consequent = self.parse_assignment()?;
            self.consume(Token::Colon)?;
            let alternate = self.parse_assignment()?;
            return Ok(self.arena.alloc_expr(Expr::Conditional {
                test,
                consequent,
                alternate,
            }));
        }

        Ok(test)
    }

    fn parse_or(&mut self) -> Result<ExprId> {
        let mut left = self.parse_and()?;
        while self.check(&Token::PipePipe) {
            self.advance()?;
            let right = self.parse_and()?;
            left = self.arena.alloc_expr(Expr::Binary {
                op: BinaryOp::Or,
                left,
                right,
            });
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<ExprId> {
        let mut left = self.parse_equality()?;
        while self.check(&Token::AmpAmp) {
            self.advance()?;
            let right = self.parse_equality()?;
            left = self.arena.alloc_expr(Expr::Binary {
                op: BinaryOp::And,
                left,
                right,
            });
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<ExprId> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match &self.current {
                Token::EqEqEq | Token::EqEq => BinaryOp::Eq,
                Token::BangEqEq | Token::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_comparison()?;
            left = self.arena.alloc_expr(Expr::Binary { op, left, right });
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<ExprId> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match &self.current {
                Token::Lt => BinaryOp::Lt,
                Token::LtEq => BinaryOp::Le,
                Token::Gt => BinaryOp::Gt,
                Token::GtEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_additive()?;
            left = self.arena.alloc_expr(Expr::Binary { op, left, right });
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<ExprId> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match &self.current {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_multiplicative()?;
            left = self.arena.alloc_expr(Expr::Binary { op, left, right });
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<ExprId> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match &self.current {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                Token::Percent => BinaryOp::Mod,
                Token::StarStar => BinaryOp::Pow,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_unary()?;
            left = self.arena.alloc_expr(Expr::Binary { op, left, right });
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<ExprId> {
        let op = match &self.current {
            Token::Minus => Some(UnaryOp::Neg),
            Token::Plus => Some(UnaryOp::Plus),
            Token::Bang => Some(UnaryOp::Not),
            Token::Tilde => Some(UnaryOp::BitNot),
            Token::TypeOf => Some(UnaryOp::TypeOf),
            Token::PlusPlus => Some(UnaryOp::PreInc),
            Token::MinusMinus => Some(UnaryOp::PreDec),
            _ => None,
        };

        if let Some(op) = op {
            self.advance()?;
            let operand = self.parse_unary()?;
            return Ok(self.arena.alloc_expr(Expr::Unary { op, operand }));
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<ExprId> {
        let mut expr = self.parse_call()?;

        loop {
            match &self.current {
                Token::PlusPlus => {
                    self.advance()?;
                    expr = self.arena.alloc_expr(Expr::Unary {
                        op: UnaryOp::PostInc,
                        operand: expr,
                    });
                }
                Token::MinusMinus => {
                    self.advance()?;
                    expr = self.arena.alloc_expr(Expr::Unary {
                        op: UnaryOp::PostDec,
                        operand: expr,
                    });
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call(&mut self) -> Result<ExprId> {
        let mut expr = self.parse_primary()?;

        loop {
            match &self.current {
                Token::LParen => {
                    self.advance()?;
                    // Collect args in temp vec to avoid nested calls corrupting expr_lists
                    let mut args = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            let arg = self.parse_expression()?;
                            args.push(arg);
                            if !self.check(&Token::Comma) {
                                break;
                            }
                            self.advance()?;
                        }
                    }
                    self.consume(Token::RParen)?;
                    let args_start = self.arena.start_expr_list();
                    let args_count = args.len() as u16;
                    for arg in args {
                        self.arena.push_expr_list(arg);
                    }
                    expr = self.arena.alloc_expr(Expr::Call {
                        callee: expr,
                        args_start,
                        args_count,
                    });
                }
                Token::Dot => {
                    self.advance()?;
                    let property = match self.advance()? {
                        Token::Identifier(s) => self.strings.intern(s),
                        _ => {
                            return Err(JSError::syntax(
                                "Expected property name",
                                self.lexer.line(),
                                self.lexer.column(),
                            ));
                        }
                    };
                    expr = self.arena.alloc_expr(Expr::Member {
                        object: expr,
                        property,
                    });
                }
                Token::LBracket => {
                    self.advance()?;
                    let index = self.parse_expression()?;
                    self.consume(Token::RBracket)?;
                    expr = self.arena.alloc_expr(Expr::Index {
                        object: expr,
                        index,
                    });
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<ExprId> {
        match self.advance()? {
            Token::Undefined => Ok(self.arena.alloc_expr(Expr::Undefined)),
            Token::Null => Ok(self.arena.alloc_expr(Expr::Null)),
            Token::True => Ok(self.arena.alloc_expr(Expr::Bool(true))),
            Token::False => Ok(self.arena.alloc_expr(Expr::Bool(false))),
            Token::Number(n) => {
                if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
                    Ok(self.arena.alloc_expr(Expr::Int(n as i32)))
                } else {
                    let idx = self.arena.add_float(n);
                    Ok(self.arena.alloc_expr(Expr::Float(idx)))
                }
            }
            Token::String(s) => {
                let str_id = self.strings.intern(s);
                Ok(self.arena.alloc_expr(Expr::String(str_id)))
            }
            Token::Identifier(s) => {
                let str_id = self.strings.intern(s);
                Ok(self.arena.alloc_expr(Expr::Identifier(str_id)))
            }
            Token::This => Ok(self.arena.alloc_expr(Expr::This)),
            Token::LParen => {
                // Could be: (expr) or (params) => body
                // Try to parse as arrow function params first
                self.try_parse_arrow_or_paren()
            }
            Token::LBracket => {
                // Collect elements in temp vec to avoid nested exprs corrupting expr_lists
                let mut elems = Vec::new();
                if !self.check(&Token::RBracket) {
                    loop {
                        let elem = self.parse_expression()?;
                        elems.push(elem);
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBracket)?;
                let elems_start = self.arena.start_expr_list();
                let elems_count = elems.len() as u16;
                for elem in elems {
                    self.arena.push_expr_list(elem);
                }
                Ok(self.arena.alloc_expr(Expr::Array {
                    elems_start,
                    elems_count,
                }))
            }
            Token::LBrace => {
                // Collect props in temp vec to avoid nested exprs corrupting prop_lists
                let mut props = Vec::new();
                if !self.check(&Token::RBrace) {
                    loop {
                        let key = match self.advance()? {
                            Token::Identifier(s) => self.strings.intern(s),
                            Token::String(s) => self.strings.intern(s),
                            _ => {
                                return Err(JSError::syntax(
                                    "Expected property name",
                                    self.lexer.line(),
                                    self.lexer.column(),
                                ));
                            }
                        };
                        self.consume(Token::Colon)?;
                        let value = self.parse_expression()?;
                        props.push((key, value));
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBrace)?;
                let props_start = self.arena.start_prop_list();
                let props_count = props.len() as u16;
                for (key, value) in props {
                    self.arena.push_prop_list(key, value);
                }
                Ok(self.arena.alloc_expr(Expr::Object {
                    props_start,
                    props_count,
                }))
            }
            Token::Function => {
                let name = if let Token::Identifier(s) = &self.current {
                    let n = self.strings.intern(s);
                    self.advance()?;
                    n
                } else {
                    StrId::EMPTY
                };
                let (params_start, params_count, body) = self.parse_function_params_body()?;
                Ok(self.arena.alloc_expr(Expr::Function {
                    name,
                    params_start,
                    params_count,
                    body,
                }))
            }
            Token::Match => self.parse_match_expr(),
            Token::Perform => self.parse_perform_expr(),
            Token::Handler => self.parse_handler_expr(),
            Token::Handle => self.parse_handle_expr(),
            Token::Do => self.parse_block_expr(),
            _ => Err(JSError::syntax(
                "Unexpected token",
                self.lexer.line(),
                self.lexer.column(),
            )),
        }
    }

    // Arrow function or parenthesized expression

    /// Parse either (expr) or (params) => body
    /// Called after consuming '('
    fn try_parse_arrow_or_paren(&mut self) -> Result<ExprId> {
        // Case 1: () => body (no params)
        if self.check(&Token::RParen) {
            self.advance()?;
            if self.check(&Token::Arrow) {
                self.advance()?;
                return self.parse_arrow_body(0, 0);
            } else {
                return Err(JSError::syntax(
                    "Empty parentheses",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        }

        // Try to collect identifiers as potential arrow params
        let params_start = self.arena.start_param_list();
        #[allow(unused_assignments)]
        let mut params_count = 0u16;
        let mut is_arrow_params = true;

        // First token must be identifier for arrow params
        let first_ident = match &self.current {
            Token::Identifier(s) => {
                let id = self.strings.intern(s);
                self.advance()?;
                Some(id)
            }
            _ => None,
        };

        if let Some(first_id) = first_ident {
            self.arena.push_param_list(first_id);
            params_count = 1;

            // Collect more params separated by commas
            while self.check(&Token::Comma) && is_arrow_params {
                self.advance()?;
                match &self.current {
                    Token::Identifier(s) => {
                        let id = self.strings.intern(s);
                        self.arena.push_param_list(id);
                        params_count += 1;
                        self.advance()?;
                    }
                    _ => {
                        is_arrow_params = false;
                    }
                }
            }

            // Check for ) followed by =>
            if is_arrow_params && self.check(&Token::RParen) {
                self.advance()?;
                if self.check(&Token::Arrow) {
                    self.advance()?;
                    return self.parse_arrow_body(params_start, params_count);
                } else {
                    // Not an arrow function - reconstruct as expression
                    // For single identifier, return it as identifier expr
                    if params_count == 1 {
                        return Ok(self.arena.alloc_expr(Expr::Identifier(first_id)));
                    }
                    // Multiple identifiers without => is a syntax error (no comma operator)
                    return Err(JSError::syntax(
                        "Unexpected comma in expression",
                        self.lexer.line(),
                        self.lexer.column(),
                    ));
                }
            }
        }

        // Not valid arrow params - parse as expression
        // We need to handle partial consumption... this is tricky
        // For now, fall back to error if we got here in a weird state
        if !is_arrow_params || first_ident.is_none() {
            // Parse the rest as an expression from current position
            // If first_ident was consumed but we're not in arrow mode,
            // we need to handle it differently
            if first_ident.is_some() {
                // We consumed an identifier and maybe some commas
                // This is an error state for now
                return Err(JSError::syntax(
                    "Invalid parenthesized expression",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
            // First token wasn't identifier - parse as regular expression
            let expr = self.parse_expression()?;
            self.consume(Token::RParen)?;
            return Ok(expr);
        }

        Err(JSError::syntax(
            "Unexpected token in parentheses",
            self.lexer.line(),
            self.lexer.column(),
        ))
    }

    /// Parse arrow function body (expression or block)
    fn parse_arrow_body(&mut self, params_start: u32, params_count: u16) -> Result<ExprId> {
        if self.check(&Token::LBrace) {
            let body = self.parse_block()?;
            Ok(self.arena.alloc_expr(Expr::Arrow {
                params_start,
                params_count,
                body: ExprId(body.0),
                is_block: true,
            }))
        } else {
            let body = self.parse_expression()?;
            Ok(self.arena.alloc_expr(Expr::Arrow {
                params_start,
                params_count,
                body,
                is_block: false,
            }))
        }
    }

    /// Parse: match (scrutinee) { pattern => expr, ... }
    fn parse_match_expr(&mut self) -> Result<ExprId> {
        self.consume(Token::LParen)?;
        let scrutinee = self.parse_expression()?;
        self.consume(Token::RParen)?;
        self.consume(Token::LBrace)?;

        let arms_start = self.arena.start_arm_list();
        let mut arms_count = 0u16;

        while !self.check(&Token::RBrace) {
            let (pattern, guard, body) = self.parse_match_arm()?;
            self.arena.push_arm(pattern, guard, body);
            arms_count += 1;

            if self.check(&Token::Comma) {
                self.advance()?;
            }
        }

        self.consume(Token::RBrace)?;

        Ok(self.arena.alloc_expr(Expr::Match {
            scrutinee,
            arms_start,
            arms_count,
        }))
    }

    /// Parse: pattern => expr  or  pattern if guard => expr
    fn parse_match_arm(&mut self) -> Result<(PatternId, ExprId, ExprId)> {
        let pattern = self.parse_pattern()?;

        let guard = if self.check(&Token::If) {
            self.advance()?;
            self.parse_expression()?
        } else {
            ExprId::NONE
        };

        self.consume(Token::Arrow)?;
        let body = self.parse_expression()?;

        Ok((pattern, guard, body))
    }

    /// Parse a pattern: _, literal, identifier, [patterns], {fields}
    fn parse_pattern(&mut self) -> Result<PatternId> {
        match &self.current {
            Token::Identifier(s) if *s == "_" => {
                self.advance()?;
                Ok(self.arena.alloc_pattern(Pattern::Wildcard))
            }

            Token::Identifier(s) => {
                let name = self.strings.intern(s);
                self.advance()?;
                Ok(self.arena.alloc_pattern(Pattern::Var(name)))
            }

            Token::Undefined => {
                self.advance()?;
                let expr_id = self.arena.alloc_expr(Expr::Undefined);
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }
            Token::Null => {
                self.advance()?;
                let expr_id = self.arena.alloc_expr(Expr::Null);
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }
            Token::True => {
                self.advance()?;
                let expr_id = self.arena.alloc_expr(Expr::Bool(true));
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }
            Token::False => {
                self.advance()?;
                let expr_id = self.arena.alloc_expr(Expr::Bool(false));
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }
            Token::Number(n) => {
                let n = *n;
                self.advance()?;
                let expr_id = if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
                    self.arena.alloc_expr(Expr::Int(n as i32))
                } else {
                    let idx = self.arena.add_float(n);
                    self.arena.alloc_expr(Expr::Float(idx))
                };
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }
            Token::String(s) => {
                let str_id = self.strings.intern(s);
                self.advance()?;
                let expr_id = self.arena.alloc_expr(Expr::String(str_id));
                Ok(self.arena.alloc_pattern(Pattern::Literal(expr_id)))
            }

            Token::LBracket => {
                self.advance()?;
                // Collect elements in temp vec to avoid nested patterns corrupting indices
                let mut elems = Vec::new();

                if !self.check(&Token::RBracket) {
                    loop {
                        let elem_pat = self.parse_pattern()?;
                        elems.push(elem_pat);
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBracket)?;

                let elems_start = self.arena.start_pattern_list();
                let elems_count = elems.len() as u16;
                for elem in elems {
                    self.arena.push_pattern_list(elem);
                }

                Ok(self.arena.alloc_pattern(Pattern::Array {
                    elems_start,
                    elems_count,
                }))
            }

            Token::LBrace => {
                self.advance()?;
                // Collect fields in temp vec to avoid nested patterns corrupting indices
                let mut fields = Vec::new();

                if !self.check(&Token::RBrace) {
                    loop {
                        let key = match &self.current {
                            Token::Identifier(s) => {
                                let k = self.strings.intern(s);
                                self.advance()?;
                                k
                            }
                            Token::String(s) => {
                                let k = self.strings.intern(s);
                                self.advance()?;
                                k
                            }
                            _ => {
                                return Err(JSError::syntax(
                                    "Expected property name in object pattern",
                                    self.lexer.line(),
                                    self.lexer.column(),
                                ));
                            }
                        };

                        let pattern = if self.check(&Token::Colon) {
                            self.advance()?;
                            self.parse_pattern()?
                        } else {
                            self.arena.alloc_pattern(Pattern::Var(key))
                        };

                        fields.push((key, pattern));

                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBrace)?;

                let fields_start = self.arena.start_pattern_field_list();
                let fields_count = fields.len() as u16;
                for (key, pattern) in fields {
                    self.arena.push_pattern_field(key, pattern);
                }

                Ok(self.arena.alloc_pattern(Pattern::Object {
                    fields_start,
                    fields_count,
                }))
            }

            _ => Err(JSError::syntax(
                "Expected pattern",
                self.lexer.line(),
                self.lexer.column(),
            )),
        }
    }

    /// Parse: perform EffectName!(args)
    fn parse_perform_expr(&mut self) -> Result<ExprId> {
        let effect = match &self.current {
            Token::Identifier(s) => {
                let id = self.strings.intern(s);
                self.advance()?;
                id
            }
            _ => {
                return Err(JSError::syntax(
                    "Expected effect name after 'perform'",
                    self.lexer.line(),
                    self.lexer.column(),
                ));
            }
        };

        self.consume(Token::Bang)?;

        self.consume(Token::LParen)?;

        // Collect args in temp vec to avoid nested exprs corrupting expr_lists
        let mut args = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                let arg = self.parse_expression()?;
                args.push(arg);
                if !self.check(&Token::Comma) {
                    break;
                }
                self.advance()?;
            }
        }

        self.consume(Token::RParen)?;

        let args_start = self.arena.start_expr_list();
        let args_count = args.len() as u16;
        for arg in args {
            self.arena.push_expr_list(arg);
        }

        Ok(self.arena.alloc_expr(Expr::Perform {
            effect,
            args_start,
            args_count,
        }))
    }

    fn parse_handler_expr(&mut self) -> Result<ExprId> {
        let (clauses_start, clauses_count, return_param, return_body) =
            self.parse_handler_clauses()?;
        Ok(self.arena.alloc_expr(Expr::Handler {
            clauses_start,
            clauses_count,
            return_param,
            return_body,
        }))
    }

    fn parse_handler_clauses(&mut self) -> Result<(u32, u16, StrId, ExprId)> {
        self.consume(Token::LBrace)?;

        let clauses_start = self.arena.start_effect_clause_list();
        let mut clauses_count = 0u16;
        let mut return_param = StrId::EMPTY;
        let mut return_body = ExprId::NONE;

        while !self.check(&Token::RBrace) {
            if self.check(&Token::Return) {
                self.advance()?;
                self.consume(Token::LParen)?;

                return_param = match &self.current {
                    Token::Identifier(s) => {
                        let id = self.strings.intern(s);
                        self.advance()?;
                        id
                    }
                    _ => {
                        return Err(JSError::syntax(
                            "Expected parameter name in return clause",
                            self.lexer.line(),
                            self.lexer.column(),
                        ));
                    }
                };

                self.consume(Token::RParen)?;
                self.consume(Token::ThinArrow)?;
                return_body = self.parse_expression()?;
            } else {
                let effect = match &self.current {
                    Token::Identifier(s) => {
                        let id = self.strings.intern(s);
                        self.advance()?;
                        id
                    }
                    _ => {
                        return Err(JSError::syntax(
                            "Expected effect name",
                            self.lexer.line(),
                            self.lexer.column(),
                        ));
                    }
                };

                self.consume(Token::Bang)?;

                self.consume(Token::LParen)?;

                let params_start = self.arena.start_param_list();
                let mut params_count = 0u16;

                if !self.check(&Token::RParen) {
                    loop {
                        let param = match &self.current {
                            Token::Identifier(s) => {
                                let id = self.strings.intern(s);
                                self.advance()?;
                                id
                            }
                            _ => {
                                return Err(JSError::syntax(
                                    "Expected parameter name",
                                    self.lexer.line(),
                                    self.lexer.column(),
                                ));
                            }
                        };
                        self.arena.push_param_list(param);
                        params_count += 1;
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }

                self.consume(Token::RParen)?;
                self.consume(Token::ThinArrow)?;

                let clause_body = if self.check(&Token::LBrace) {
                    self.parse_block_expr()?
                } else {
                    self.parse_expression()?
                };

                self.arena
                    .push_effect_clause(effect, params_start, params_count, clause_body);
                clauses_count += 1;
            }

            if self.check(&Token::Comma) {
                self.advance()?;
            }
        }

        self.consume(Token::RBrace)?;

        return Ok((clauses_start, clauses_count, return_param, return_body));
    }

    /// Parse: handle { body } with { effect_clauses }
    fn parse_handle_expr(&mut self) -> Result<ExprId> {
        let body = self.parse_block_expr()?;

        self.consume(Token::With)?;

        if self.check(&Token::LBrace) {
            let (clauses_start, clauses_count, return_param, return_body) =
                self.parse_handler_clauses()?;
            Ok(self.arena.alloc_expr(Expr::Handle {
                body,
                clauses_start,
                clauses_count,
                return_param,
                return_body,
            }))
        } else {
            // Use parse_call to avoid consuming binary operators like ===
            let handler = self.parse_call()?;
            Ok(self.arena.alloc_expr(Expr::HandleWith { body, handler }))
        }
    }
}
