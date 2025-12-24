//! JavaScript parser
//!
//! Recursive descent parser producing an AST.

use crate::error::{JSError, Result};
use crate::lexer::{Lexer, Token};

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// AST Node
#[derive(Debug, Clone)]
pub enum Expr {
    // Literals
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    #[cfg(feature = "alloc")]
    String(String),
    #[cfg(feature = "alloc")]
    Identifier(String),

    // Compound expressions
    #[cfg(feature = "alloc")]
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    #[cfg(feature = "alloc")]
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    #[cfg(feature = "alloc")]
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    #[cfg(feature = "alloc")]
    Member {
        object: Box<Expr>,
        property: Box<Expr>,
        computed: bool,
    },
    #[cfg(feature = "alloc")]
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    #[cfg(feature = "alloc")]
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    #[cfg(feature = "alloc")]
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    #[cfg(feature = "alloc")]
    Array(Vec<Expr>),
    #[cfg(feature = "alloc")]
    Object(Vec<(String, Expr)>),
    #[cfg(feature = "alloc")]
    Function {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    #[cfg(feature = "alloc")]
    Arrow {
        params: Vec<String>,
        body: Box<ArrowBody>,
    },
    This,
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone)]
pub enum ArrowBody {
    Expr(Expr),
    Block(Vec<Stmt>),
}

/// Statement
#[derive(Debug, Clone)]
pub enum Stmt {
    #[cfg(feature = "alloc")]
    Expr(Expr),
    #[cfg(feature = "alloc")]
    Let {
        name: String,
        init: Option<Expr>,
    },
    #[cfg(feature = "alloc")]
    Const {
        name: String,
        init: Expr,
    },
    #[cfg(feature = "alloc")]
    If {
        test: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    #[cfg(feature = "alloc")]
    While {
        test: Expr,
        body: Box<Stmt>,
    },
    #[cfg(feature = "alloc")]
    For {
        init: Option<Box<Stmt>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    #[cfg(feature = "alloc")]
    Block(Vec<Stmt>),
    #[cfg(feature = "alloc")]
    Return(Option<Expr>),
    Break,
    Continue,
    #[cfg(feature = "alloc")]
    Throw(Expr),
    #[cfg(feature = "alloc")]
    Try {
        body: Vec<Stmt>,
        catch_param: Option<String>,
        catch_body: Option<Vec<Stmt>>,
        finally_body: Option<Vec<Stmt>>,
    },
    #[cfg(feature = "alloc")]
    Function {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Parser state
#[cfg(feature = "alloc")]
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
}

#[cfg(feature = "alloc")]
impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Result<Self> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self { lexer, current })
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

    pub fn parse_program(&mut self) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !self.check(&Token::Eof) {
            stmts.push(self.parse_statement()?);
        }
        Ok(stmts)
    }

    fn parse_statement(&mut self) -> Result<Stmt> {
        match &self.current {
            Token::Let => self.parse_let(),
            Token::Const => self.parse_const(),
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
                Ok(Stmt::Break)
            }
            Token::Continue => {
                self.advance()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Ok(Stmt::Continue)
            }
            Token::Throw => self.parse_throw(),
            Token::Try => self.parse_try(),
            Token::LBrace => self.parse_block(),
            Token::Semicolon => {
                self.advance()?;
                Ok(Stmt::Empty)
            }
            _ => {
                let expr = self.parse_expression()?;
                if self.check(&Token::Semicolon) {
                    self.advance()?;
                }
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_let(&mut self) -> Result<Stmt> {
        self.consume(Token::Let)?;
        let name = match self.advance()? {
            Token::Identifier(s) => String::from(s),
            _ => return Err(JSError::syntax("Expected identifier", self.lexer.line(), self.lexer.column())),
        };
        let init = if self.check(&Token::Eq) {
            self.advance()?;
            Some(self.parse_expression()?)
        } else {
            None
        };
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(Stmt::Let { name, init })
    }

    fn parse_const(&mut self) -> Result<Stmt> {
        self.consume(Token::Const)?;
        let name = match self.advance()? {
            Token::Identifier(s) => String::from(s),
            _ => return Err(JSError::syntax("Expected identifier", self.lexer.line(), self.lexer.column())),
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
        let consequent = Box::new(self.parse_statement()?);
        let alternate = if self.check(&Token::Else) {
            self.advance()?;
            Some(Box::new(self.parse_statement()?))
        } else {
            None
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
        let body = Box::new(self.parse_statement()?);
        Ok(Stmt::While { test, body })
    }

    fn parse_for(&mut self) -> Result<Stmt> {
        self.consume(Token::For)?;
        self.consume(Token::LParen)?;

        let init = if self.check(&Token::Semicolon) {
            None
        } else if self.check(&Token::Let) {
            Some(Box::new(self.parse_let()?))
        } else {
            let expr = self.parse_expression()?;
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            Some(Box::new(Stmt::Expr(expr)))
        };

        if !self.check(&Token::Semicolon) && init.is_some() {
            // Already consumed semicolon in parse_let
        } else if self.check(&Token::Semicolon) {
            self.advance()?;
        }

        let test = if self.check(&Token::Semicolon) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.consume(Token::Semicolon)?;

        let update = if self.check(&Token::RParen) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.consume(Token::RParen)?;

        let body = Box::new(self.parse_statement()?);
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
            Token::Identifier(s) => String::from(s),
            _ => return Err(JSError::syntax("Expected function name", self.lexer.line(), self.lexer.column())),
        };
        let (params, body) = self.parse_function_params_body()?;
        Ok(Stmt::Function { name, params, body })
    }

    fn parse_function_params_body(&mut self) -> Result<(Vec<String>, Vec<Stmt>)> {
        self.consume(Token::LParen)?;
        let mut params = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                match self.advance()? {
                    Token::Identifier(s) => params.push(String::from(s)),
                    _ => return Err(JSError::syntax("Expected parameter name", self.lexer.line(), self.lexer.column())),
                }
                if !self.check(&Token::Comma) {
                    break;
                }
                self.advance()?;
            }
        }
        self.consume(Token::RParen)?;
        self.consume(Token::LBrace)?;
        let mut body = Vec::new();
        while !self.check(&Token::RBrace) {
            body.push(self.parse_statement()?);
        }
        self.consume(Token::RBrace)?;
        Ok((params, body))
    }

    fn parse_return(&mut self) -> Result<Stmt> {
        self.consume(Token::Return)?;
        if self.check(&Token::Semicolon) || self.check(&Token::RBrace) || self.check(&Token::Eof) {
            if self.check(&Token::Semicolon) {
                self.advance()?;
            }
            return Ok(Stmt::Return(None));
        }
        let expr = self.parse_expression()?;
        if self.check(&Token::Semicolon) {
            self.advance()?;
        }
        Ok(Stmt::Return(Some(expr)))
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
        self.consume(Token::LBrace)?;
        let mut body = Vec::new();
        while !self.check(&Token::RBrace) {
            body.push(self.parse_statement()?);
        }
        self.consume(Token::RBrace)?;

        let (catch_param, catch_body) = if self.check(&Token::Catch) {
            self.advance()?;
            let param = if self.check(&Token::LParen) {
                self.advance()?;
                let name = match self.advance()? {
                    Token::Identifier(s) => String::from(s),
                    _ => return Err(JSError::syntax("Expected catch parameter", self.lexer.line(), self.lexer.column())),
                };
                self.consume(Token::RParen)?;
                Some(name)
            } else {
                None
            };
            self.consume(Token::LBrace)?;
            let mut catch_stmts = Vec::new();
            while !self.check(&Token::RBrace) {
                catch_stmts.push(self.parse_statement()?);
            }
            self.consume(Token::RBrace)?;
            (param, Some(catch_stmts))
        } else {
            (None, None)
        };

        let finally_body = if self.check(&Token::Finally) {
            self.advance()?;
            self.consume(Token::LBrace)?;
            let mut finally_stmts = Vec::new();
            while !self.check(&Token::RBrace) {
                finally_stmts.push(self.parse_statement()?);
            }
            self.consume(Token::RBrace)?;
            Some(finally_stmts)
        } else {
            None
        };

        Ok(Stmt::Try {
            body,
            catch_param,
            catch_body,
            finally_body,
        })
    }

    fn parse_block(&mut self) -> Result<Stmt> {
        self.consume(Token::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&Token::RBrace) {
            stmts.push(self.parse_statement()?);
        }
        self.consume(Token::RBrace)?;
        Ok(Stmt::Block(stmts))
    }

    fn parse_expression(&mut self) -> Result<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr> {
        let left = self.parse_ternary()?;

        if self.check(&Token::Eq) {
            self.advance()?;
            let right = self.parse_assignment()?;
            return Ok(Expr::Assign {
                target: Box::new(left),
                value: Box::new(right),
            });
        }

        Ok(left)
    }

    fn parse_ternary(&mut self) -> Result<Expr> {
        let test = self.parse_or()?;

        if self.check(&Token::Question) {
            self.advance()?;
            let consequent = self.parse_assignment()?;
            self.consume(Token::Colon)?;
            let alternate = self.parse_assignment()?;
            return Ok(Expr::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            });
        }

        Ok(test)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.check(&Token::PipePipe) {
            self.advance()?;
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_or()?;
        while self.check(&Token::AmpAmp) {
            self.advance()?;
            let right = self.parse_bitwise_or()?;
            left = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_xor()?;
        while self.check(&Token::Pipe) {
            self.advance()?;
            let right = self.parse_bitwise_xor()?;
            left = Expr::Binary {
                op: BinaryOp::BitOr,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_and()?;
        while self.check(&Token::Caret) {
            self.advance()?;
            let right = self.parse_bitwise_and()?;
            left = Expr::Binary {
                op: BinaryOp::BitXor,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_equality()?;
        while self.check(&Token::Amp) {
            self.advance()?;
            let right = self.parse_equality()?;
            left = Expr::Binary {
                op: BinaryOp::BitAnd,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match &self.current {
                Token::EqEqEq => BinaryOp::Eq,
                Token::BangEqEq => BinaryOp::Ne,
                Token::EqEq => BinaryOp::Eq, // Treat == as === (stricter mode)
                Token::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_comparison()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
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
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr> {
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
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match &self.current {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_multiplicative()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
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
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_exponentiation(&mut self) -> Result<Expr> {
        let left = self.parse_unary()?;
        if self.check(&Token::StarStar) {
            self.advance()?;
            let right = self.parse_exponentiation()?; // Right associative
            return Ok(Expr::Binary {
                op: BinaryOp::Pow,
                left: Box::new(left),
                right: Box::new(right),
            });
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        match &self.current {
            Token::Minus => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                })
            }
            Token::Plus => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Plus,
                    operand: Box::new(operand),
                })
            }
            Token::Bang => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                })
            }
            Token::Tilde => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::BitNot,
                    operand: Box::new(operand),
                })
            }
            Token::TypeOf => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::TypeOf,
                    operand: Box::new(operand),
                })
            }
            Token::PlusPlus => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::PreInc,
                    operand: Box::new(operand),
                })
            }
            Token::MinusMinus => {
                self.advance()?;
                let operand = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::PreDec,
                    operand: Box::new(operand),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_call()?;

        loop {
            match &self.current {
                Token::PlusPlus => {
                    self.advance()?;
                    expr = Expr::Unary {
                        op: UnaryOp::PostInc,
                        operand: Box::new(expr),
                    };
                }
                Token::MinusMinus => {
                    self.advance()?;
                    expr = Expr::Unary {
                        op: UnaryOp::PostDec,
                        operand: Box::new(expr),
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            match &self.current {
                Token::LParen => {
                    self.advance()?;
                    let mut args = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            args.push(self.parse_expression()?);
                            if !self.check(&Token::Comma) {
                                break;
                            }
                            self.advance()?;
                        }
                    }
                    self.consume(Token::RParen)?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                Token::Dot => {
                    self.advance()?;
                    let property = match self.advance()? {
                        Token::Identifier(s) => Expr::String(String::from(s)),
                        _ => {
                            return Err(JSError::syntax(
                                "Expected property name",
                                self.lexer.line(),
                                self.lexer.column(),
                            ))
                        }
                    };
                    expr = Expr::Member {
                        object: Box::new(expr),
                        property: Box::new(property),
                        computed: false,
                    };
                }
                Token::LBracket => {
                    self.advance()?;
                    let index = self.parse_expression()?;
                    self.consume(Token::RBracket)?;
                    expr = Expr::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.advance()? {
            Token::Undefined => Ok(Expr::Undefined),
            Token::Null => Ok(Expr::Null),
            Token::True => Ok(Expr::Bool(true)),
            Token::False => Ok(Expr::Bool(false)),
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::String(s) => Ok(Expr::String(String::from(s))),
            Token::Identifier(s) => Ok(Expr::Identifier(String::from(s))),
            Token::This => Ok(Expr::This),
            Token::LParen => {
                let expr = self.parse_expression()?;
                self.consume(Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => {
                let mut elements = Vec::new();
                if !self.check(&Token::RBracket) {
                    loop {
                        elements.push(self.parse_expression()?);
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBracket)?;
                Ok(Expr::Array(elements))
            }
            Token::LBrace => {
                let mut properties = Vec::new();
                if !self.check(&Token::RBrace) {
                    loop {
                        let key = match self.advance()? {
                            Token::Identifier(s) => String::from(s),
                            Token::String(s) => String::from(s),
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
                        properties.push((key, value));
                        if !self.check(&Token::Comma) {
                            break;
                        }
                        self.advance()?;
                    }
                }
                self.consume(Token::RBrace)?;
                Ok(Expr::Object(properties))
            }
            Token::Function => {
                let name = if let Token::Identifier(s) = &self.current {
                    let n = String::from(*s);
                    self.advance()?;
                    Some(n)
                } else {
                    None
                };
                let (params, body) = self.parse_function_params_body()?;
                Ok(Expr::Function { name, params, body })
            }
            _ => Err(JSError::syntax(
                "Unexpected token",
                self.lexer.line(),
                self.lexer.column(),
            )),
        }
    }
}
