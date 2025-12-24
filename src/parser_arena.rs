//! Arena-based JavaScript parser
//!
//! No heap allocation - all AST nodes stored in fixed-size arena.

use crate::ast::{AstArena, BinaryOp, Expr, ExprId, Stmt, StmtId, UnaryOp};
use crate::error::{JSError, Result};
use crate::fixed_string::{FixedStringPool, StrId};
use crate::lexer::{Lexer, Token};
use crate::{MAX_STRINGS, MAX_STRING_BYTES};

/// Arena-based parser
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
    strings: &'a mut FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
    ast: &'a mut AstArena,
}

impl<'a> Parser<'a> {
    pub fn new(
        source: &'a str,
        strings: &'a mut FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
        ast: &'a mut AstArena,
    ) -> Result<Self> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self {
            lexer,
            current,
            strings,
            ast,
        })
    }

    fn advance(&mut self) -> Result<Token<'a>> {
        let prev = core::mem::replace(&mut self.current, self.lexer.next_token()?);
        Ok(prev)
    }

    fn check(&self, token: &Token) -> bool {
        core::mem::discriminant(&self.current) == core::mem::discriminant(token)
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

    fn intern(&mut self, s: &str) -> Result<StrId> {
        self.strings
            .intern(s)
            .ok_or_else(|| JSError::InternalError("String pool full"))
    }

    fn alloc_expr(&mut self, expr: Expr) -> Result<ExprId> {
        self.ast
            .alloc_expr(expr)
            .ok_or_else(|| JSError::InternalError("AST arena full"))
    }

    fn alloc_stmt(&mut self, stmt: Stmt) -> Result<StmtId> {
        self.ast
            .alloc_stmt(stmt)
            .ok_or_else(|| JSError::InternalError("AST arena full"))
    }

    /// Parse program into arena, set root in ast
    pub fn parse_program(&mut self) -> Result<()> {
        let start = self.ast.start_stmt_list();
        let mut count = 0u16;

        while !self.check(&Token::Eof) {
            let stmt = self.parse_statement()?;
            self.ast
                .push_stmt_list(stmt)
                .ok_or_else(|| JSError::InternalError("Statement list full"))?;
            count += 1;
        }

        self.ast.root_start = start;
        self.ast.root_count = count;
        Ok(())
    }

    fn parse_statement(&mut self) -> Result<StmtId> {
        let stmt = match &self.current {
            Token::Let => self.parse_let()?,
            Token::Const => self.parse_const()?,
            Token::If => self.parse_if()?,
            Token::While => self.parse_while()?,
            Token::For => self.parse_for()?,
            Token::Function => self.parse_function_decl()?,
            Token::Return => self.parse_return()?,
            Token::Break => {
                self.advance()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Stmt::Break
            }
            Token::Continue => {
                self.advance()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Stmt::Continue
            }
            Token::Throw => self.parse_throw()?,
            Token::Try => self.parse_try()?,
            Token::LBrace => self.parse_block()?,
            Token::Semicolon => {
                self.advance()?;
                Stmt::Empty
            }
            _ => {
                let expr = self.parse_expression()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Stmt::Expr(expr)
            }
        };
        self.alloc_stmt(stmt)
    }

    fn parse_let(&mut self) -> Result<Stmt> {
        self.consume(Token::Let)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.intern(s)?,
            _ => {
                return Err(JSError::syntax(
                    "Expected identifier",
                    self.lexer.line(),
                    self.lexer.column(),
                ))
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
        Ok(Stmt::Let { name, init })
    }

    fn parse_const(&mut self) -> Result<Stmt> {
        self.consume(Token::Const)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.intern(s)?,
            _ => {
                return Err(JSError::syntax(
                    "Expected identifier",
                    self.lexer.line(),
                    self.lexer.column(),
                ))
            }
        };
        self.consume(Token::Eq)?;
        let init = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(Stmt::Const { name, init })
    }

    fn parse_if(&mut self) -> Result<Stmt> {
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
        Ok(Stmt::If {
            test,
            consequent,
            alternate,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt> {
        self.consume(Token::While)?;
        self.consume(Token::LParen)?;
        let test = self.parse_expression()?;
        self.consume(Token::RParen)?;
        let body = self.parse_statement()?;
        Ok(Stmt::While { test, body })
    }

    fn parse_for(&mut self) -> Result<Stmt> {
        self.consume(Token::For)?;
        self.consume(Token::LParen)?;

        let init = if self.check(&Token::Semicolon) {
            self.advance()?;
            StmtId::NONE
        } else if self.check(&Token::Let) {
            let stmt = self.parse_let()?;
            self.alloc_stmt(stmt)?
        } else if self.check(&Token::Const) {
            let stmt = self.parse_const()?;
            self.alloc_stmt(stmt)?
        } else {
            let expr = self.parse_expression()?;
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            self.alloc_stmt(Stmt::Expr(expr))?
        };

        let test = if self.check(&Token::Semicolon) {
            self.advance()?;
            ExprId::NONE
        } else {
            let expr = self.parse_expression()?;
            self.consume(Token::Semicolon)?;
            expr
        };

        let update = if self.check(&Token::RParen) {
            ExprId::NONE
        } else {
            self.parse_expression()?
        };
        self.consume(Token::RParen)?;

        let body = self.parse_statement()?;
        Ok(Stmt::For {
            init,
            test,
            update,
            body,
        })
    }

    fn parse_function_decl(&mut self) -> Result<Stmt> {
        self.consume(Token::Function)?;
        let name = match self.advance()? {
            Token::Identifier(s) => self.intern(s)?,
            _ => {
                return Err(JSError::syntax(
                    "Expected function name",
                    self.lexer.line(),
                    self.lexer.column(),
                ))
            }
        };
        let (params_start, params_count, body) = self.parse_function_params_body()?;
        Ok(Stmt::Function {
            name,
            params_start,
            params_count,
            body,
        })
    }

    fn parse_function_params_body(&mut self) -> Result<(u16, u8, StmtId)> {
        self.consume(Token::LParen)?;

        let params_start = self.ast.start_param_list();
        let mut params_count = 0u8;

        if !self.check(&Token::RParen) {
            loop {
                match self.advance()? {
                    Token::Identifier(s) => {
                        let name = self.intern(s)?;
                        self.ast
                            .push_param_list(name)
                            .ok_or_else(|| JSError::InternalError("Param list full"))?;
                        params_count += 1;
                    }
                    _ => {
                        return Err(JSError::syntax(
                            "Expected parameter name",
                            self.lexer.line(),
                            self.lexer.column(),
                        ))
                    }
                }
                if !self.check(&Token::Comma) {
                    break;
                }
                self.advance()?;
            }
        }
        self.consume(Token::RParen)?;

        // Parse body as block
        let body = self.parse_block()?;
        let body_id = self.alloc_stmt(body)?;

        Ok((params_start, params_count, body_id))
    }

    fn parse_return(&mut self) -> Result<Stmt> {
        self.consume(Token::Return)?;
        if self.check(&Token::Semicolon) || self.check(&Token::RBrace) || self.check(&Token::Eof) {
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            return Ok(Stmt::Return(ExprId::NONE));
        }
        let expr = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(Stmt::Return(expr))
    }

    fn parse_throw(&mut self) -> Result<Stmt> {
        self.consume(Token::Throw)?;
        let expr = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(Stmt::Throw(expr))
    }

    fn parse_try(&mut self) -> Result<Stmt> {
        self.consume(Token::Try)?;

        // Parse try body
        let body = self.parse_block()?;
        let body_id = self.alloc_stmt(body)?;

        // Parse catch
        let (catch_param, catch_body) = if self.check(&Token::Catch) {
            self.advance()?;
            let param = if self.check(&Token::LParen) {
                self.advance()?;
                let name = match self.advance()? {
                    Token::Identifier(s) => self.intern(s)?,
                    _ => {
                        return Err(JSError::syntax(
                            "Expected catch parameter",
                            self.lexer.line(),
                            self.lexer.column(),
                        ))
                    }
                };
                self.consume(Token::RParen)?;
                name
            } else {
                StrId::NONE
            };
            let catch_block = self.parse_block()?;
            let catch_id = self.alloc_stmt(catch_block)?;
            (param, catch_id)
        } else {
            (StrId::NONE, StmtId::NONE)
        };

        // Parse finally
        let finally_body = if self.check(&Token::Finally) {
            self.advance()?;
            let finally_block = self.parse_block()?;
            self.alloc_stmt(finally_block)?
        } else {
            StmtId::NONE
        };

        Ok(Stmt::Try {
            body: body_id,
            catch_param,
            catch_body,
            finally_body,
        })
    }

    fn parse_block(&mut self) -> Result<Stmt> {
        self.consume(Token::LBrace)?;

        let stmts_start = self.ast.start_stmt_list();
        let mut stmts_count = 0u16;

        while !self.check(&Token::RBrace) {
            let stmt = self.parse_statement()?;
            self.ast
                .push_stmt_list(stmt)
                .ok_or_else(|| JSError::InternalError("Statement list full"))?;
            stmts_count += 1;
        }
        self.consume(Token::RBrace)?;

        Ok(Stmt::Block {
            stmts_start,
            stmts_count,
        })
    }

    fn parse_expression(&mut self) -> Result<ExprId> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<ExprId> {
        let left = self.parse_ternary()?;

        if self.check(&Token::Eq) {
            self.advance()?;
            let right = self.parse_assignment()?;
            let expr = Expr::Assign {
                target: left,
                value: right,
            };
            return self.alloc_expr(expr);
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
            let expr = Expr::Conditional {
                test,
                consequent,
                alternate,
            };
            return self.alloc_expr(expr);
        }

        Ok(test)
    }

    fn parse_or(&mut self) -> Result<ExprId> {
        let mut left = self.parse_and()?;
        while self.check(&Token::PipePipe) {
            self.advance()?;
            let right = self.parse_and()?;
            let expr = Expr::Binary {
                op: BinaryOp::Or,
                left,
                right,
            };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<ExprId> {
        let mut left = self.parse_bitwise_or()?;
        while self.check(&Token::AmpAmp) {
            self.advance()?;
            let right = self.parse_bitwise_or()?;
            let expr = Expr::Binary {
                op: BinaryOp::And,
                left,
                right,
            };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<ExprId> {
        let mut left = self.parse_bitwise_xor()?;
        while self.check(&Token::Pipe) {
            self.advance()?;
            let right = self.parse_bitwise_xor()?;
            let expr = Expr::Binary {
                op: BinaryOp::BitOr,
                left,
                right,
            };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<ExprId> {
        let mut left = self.parse_bitwise_and()?;
        while self.check(&Token::Caret) {
            self.advance()?;
            let right = self.parse_bitwise_and()?;
            let expr = Expr::Binary {
                op: BinaryOp::BitXor,
                left,
                right,
            };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<ExprId> {
        let mut left = self.parse_equality()?;
        while self.check(&Token::Amp) {
            self.advance()?;
            let right = self.parse_equality()?;
            let expr = Expr::Binary {
                op: BinaryOp::BitAnd,
                left,
                right,
            };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<ExprId> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match &self.current {
                Token::EqEqEq => BinaryOp::Eq,
                Token::BangEqEq => BinaryOp::Ne,
                Token::EqEq => BinaryOp::Eq,
                Token::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_comparison()?;
            let expr = Expr::Binary { op, left, right };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<ExprId> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match &self.current {
                Token::Lt => BinaryOp::Lt,
                Token::LtEq => BinaryOp::Le,
                Token::Gt => BinaryOp::Gt,
                Token::GtEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_shift()?;
            let expr = Expr::Binary { op, left, right };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<ExprId> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match &self.current {
                Token::LtLt => BinaryOp::Shl,
                Token::GtGt => BinaryOp::Shr,
                Token::GtGtGt => BinaryOp::UShr,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_additive()?;
            let expr = Expr::Binary { op, left, right };
            left = self.alloc_expr(expr)?;
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
            let expr = Expr::Binary { op, left, right };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<ExprId> {
        let mut left = self.parse_exponentiation()?;
        loop {
            let op = match &self.current {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                Token::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_exponentiation()?;
            let expr = Expr::Binary { op, left, right };
            left = self.alloc_expr(expr)?;
        }
        Ok(left)
    }

    fn parse_exponentiation(&mut self) -> Result<ExprId> {
        let left = self.parse_unary()?;
        if self.check(&Token::StarStar) {
            self.advance()?;
            let right = self.parse_exponentiation()?;
            let expr = Expr::Binary {
                op: BinaryOp::Pow,
                left,
                right,
            };
            return self.alloc_expr(expr);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<ExprId> {
        let (op, has_op) = match &self.current {
            Token::Minus => (UnaryOp::Neg, true),
            Token::Plus => (UnaryOp::Plus, true),
            Token::Bang => (UnaryOp::Not, true),
            Token::Tilde => (UnaryOp::BitNot, true),
            Token::TypeOf => (UnaryOp::TypeOf, true),
            Token::PlusPlus => (UnaryOp::PreInc, true),
            Token::MinusMinus => (UnaryOp::PreDec, true),
            _ => (UnaryOp::Neg, false),
        };

        if has_op {
            self.advance()?;
            let operand = self.parse_unary()?;
            let expr = Expr::Unary { op, operand };
            return self.alloc_expr(expr);
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<ExprId> {
        let mut expr = self.parse_call()?;

        loop {
            let op = match &self.current {
                Token::PlusPlus => UnaryOp::PostInc,
                Token::MinusMinus => UnaryOp::PostDec,
                _ => break,
            };
            self.advance()?;
            let unary = Expr::Unary { op, operand: expr };
            expr = self.alloc_expr(unary)?;
        }

        Ok(expr)
    }

    fn parse_call(&mut self) -> Result<ExprId> {
        let mut expr = self.parse_primary()?;

        loop {
            match &self.current {
                Token::LParen => {
                    self.advance()?;

                    let args_start = self.ast.start_expr_list();
                    let mut args_count = 0u8;

                    if !self.check(&Token::RParen) {
                        loop {
                            let arg = self.parse_expression()?;
                            self.ast
                                .push_expr_list(arg)
                                .ok_or_else(|| JSError::InternalError("Expr list full"))?;
                            args_count += 1;
                            if !self.check(&Token::Comma) {
                                break;
                            }
                            self.advance()?;
                        }
                    }
                    self.consume(Token::RParen)?;

                    let call = Expr::Call {
                        callee: expr,
                        args_start,
                        args_count,
                    };
                    expr = self.alloc_expr(call)?;
                }
                Token::Dot => {
                    self.advance()?;
                    let property = match self.advance()? {
                        Token::Identifier(s) => self.intern(s)?,
                        _ => {
                            return Err(JSError::syntax(
                                "Expected property name",
                                self.lexer.line(),
                                self.lexer.column(),
                            ))
                        }
                    };
                    let member = Expr::Member {
                        object: expr,
                        property,
                    };
                    expr = self.alloc_expr(member)?;
                }
                Token::LBracket => {
                    self.advance()?;
                    let index = self.parse_expression()?;
                    self.consume(Token::RBracket)?;
                    let idx_expr = Expr::Index { object: expr, index };
                    expr = self.alloc_expr(idx_expr)?;
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<ExprId> {
        let expr = match self.advance()? {
            Token::Undefined => Expr::Undefined,
            Token::Null => Expr::Null,
            Token::True => Expr::Bool(true),
            Token::False => Expr::Bool(false),
            Token::Number(n) => {
                // Try to store as int if possible
                let is_int = n == libm::trunc(n);
                if is_int && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
                    Expr::Int(n as i32)
                } else {
                    let idx = self
                        .ast
                        .add_float(n)
                        .ok_or_else(|| JSError::InternalError("Float pool full"))?;
                    Expr::Float(idx)
                }
            }
            Token::String(s) => {
                let id = self.intern(s)?;
                Expr::String(id)
            }
            Token::Identifier(s) => {
                let id = self.intern(s)?;
                Expr::Identifier(id)
            }
            Token::This => Expr::This,
            Token::LParen => {
                let inner = self.parse_expression()?;
                self.consume(Token::RParen)?;
                return Ok(inner);
            }
            Token::LBracket => {
                // Array literal
                let elems_start = self.ast.start_expr_list();
                let mut elems_count = 0u8;

                if !self.check(&Token::RBracket) {
                    loop {
                        let elem = self.parse_expression()?;
                        self.ast
                            .push_expr_list(elem)
                            .ok_or_else(|| JSError::InternalError("Expr list full"))?;
                        elems_count += 1;
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBracket)?;

                Expr::Array {
                    elems_start,
                    elems_count,
                }
            }
            Token::LBrace => {
                // Object literal
                let props_start = self.ast.start_prop_list();
                let mut props_count = 0u8;

                if !self.check(&Token::RBrace) {
                    loop {
                        let key = match self.advance()? {
                            Token::Identifier(s) => self.intern(s)?,
                            Token::String(s) => self.intern(s)?,
                            _ => {
                                return Err(JSError::syntax(
                                    "Expected property name",
                                    self.lexer.line(),
                                    self.lexer.column(),
                                ))
                            }
                        };
                        self.consume(Token::Colon)?;
                        let value = self.parse_expression()?;
                        self.ast
                            .push_prop_list(key, value)
                            .ok_or_else(|| JSError::InternalError("Prop list full"))?;
                        props_count += 1;
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBrace)?;

                Expr::Object {
                    props_start,
                    props_count,
                }
            }
            Token::Function => {
                // Function expression
                let name = if let Token::Identifier(s) = &self.current {
                    let n = self.intern(s)?;
                    self.advance()?;
                    n
                } else {
                    StrId::NONE
                };
                let (params_start, params_count, body) = self.parse_function_params_body()?;
                Expr::Function {
                    name,
                    params_start,
                    params_count,
                    body,
                }
            }
            _ => {
                return Err(JSError::syntax(
                    "Unexpected token",
                    self.lexer.line(),
                    self.lexer.column(),
                ))
            }
        };

        self.alloc_expr(expr)
    }
}
