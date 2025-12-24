//! Bytecode compiler
//!
//! Compiles AST to bytecode for the VM.
//! Requires alloc feature for parsing stage.

use crate::bytecode::{Chunk, OpCode};
use crate::error::{JSError, Result};
use crate::fixed_string::FixedStringPool;
use crate::parser::{BinaryOp, Expr, Stmt, UnaryOp};
use crate::{MAX_STRINGS, MAX_STRING_BYTES};

#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Local variable
#[cfg(feature = "alloc")]
struct Local {
    name: String,
    depth: u32,
}

/// Compiler state
#[cfg(feature = "alloc")]
pub struct Compiler<'a> {
    strings: &'a mut FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
    chunk: Chunk,
    locals: Vec<Local>,
    scope_depth: u32,
    loop_starts: Vec<usize>,
    loop_breaks: Vec<Vec<usize>>,
}

#[cfg(feature = "alloc")]
impl<'a> Compiler<'a> {
    pub fn new(strings: &'a mut FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>) -> Self {
        Self {
            strings,
            chunk: Chunk::new(),
            locals: Vec::new(),
            scope_depth: 0,
            loop_starts: Vec::new(),
            loop_breaks: Vec::new(),
        }
    }

    pub fn compile(mut self, stmts: &[Stmt]) -> Result<Chunk> {
        for stmt in stmts {
            self.compile_stmt(stmt)?;
        }
        self.emit(OpCode::Halt);
        Ok(self.chunk)
    }

    fn emit(&mut self, op: OpCode) {
        self.chunk.write_op(op);
    }

    fn emit_byte(&mut self, byte: u8) {
        self.chunk.write(byte);
    }

    fn emit_u16(&mut self, value: u16) {
        self.chunk.write((value >> 8) as u8);
        self.chunk.write((value & 0xff) as u8);
    }

    fn emit_i16(&mut self, value: i16) {
        self.emit_u16(value as u16);
    }

    fn emit_jump(&mut self, op: OpCode) -> usize {
        self.emit(op);
        let offset = self.chunk.len();
        self.emit_u16(0xFFFF);
        offset
    }

    fn patch_jump(&mut self, offset: usize) {
        let jump = self.chunk.len() - offset - 2;
        if jump > u16::MAX as usize {
            return; // TODO: proper error
        }
        // Patch the jump offset in the bytecode
        if let Some(hi) = self.chunk.code.get(offset) {
            self.chunk.code.set(offset, (jump >> 8) as u8);
        }
        if let Some(lo) = self.chunk.code.get(offset + 1) {
            self.chunk.code.set(offset + 1, (jump & 0xff) as u8);
        }
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    fn end_scope(&mut self) {
        self.scope_depth -= 1;
        while let Some(local) = self.locals.last() {
            if local.depth <= self.scope_depth {
                break;
            }
            self.emit(OpCode::Pop);
            self.locals.pop();
        }
    }

    fn add_local(&mut self, name: String) -> u8 {
        let idx = self.locals.len() as u8;
        self.locals.push(Local {
            name,
            depth: self.scope_depth,
        });
        idx
    }

    fn resolve_local(&self, name: &str) -> Option<u8> {
        for (i, local) in self.locals.iter().enumerate().rev() {
            if local.name == name {
                return Some(i as u8);
            }
        }
        None
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Expr(expr) => {
                self.compile_expr(expr)?;
                self.emit(OpCode::Pop);
            }
            Stmt::Let { name, init } => {
                if let Some(init) = init {
                    self.compile_expr(init)?;
                } else {
                    self.emit(OpCode::PushUndefined);
                }
                self.add_local(name.clone());
            }
            Stmt::Const { name, init } => {
                self.compile_expr(init)?;
                self.add_local(name.clone());
            }
            Stmt::Block(stmts) => {
                self.begin_scope();
                for s in stmts {
                    self.compile_stmt(s)?;
                }
                self.end_scope();
            }
            Stmt::If {
                test,
                consequent,
                alternate,
            } => {
                self.compile_expr(test)?;
                let else_jump = self.emit_jump(OpCode::JumpFalse);
                self.compile_stmt(consequent)?;
                if let Some(alt) = alternate {
                    let end_jump = self.emit_jump(OpCode::Jump);
                    self.patch_jump(else_jump);
                    self.compile_stmt(alt)?;
                    self.patch_jump(end_jump);
                } else {
                    self.patch_jump(else_jump);
                }
            }
            Stmt::While { test, body } => {
                let loop_start = self.chunk.len();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(Vec::new());

                self.compile_expr(test)?;
                let exit_jump = self.emit_jump(OpCode::JumpFalse);
                self.compile_stmt(body)?;

                // Jump back to start
                self.emit(OpCode::Jump);
                let back_jump = (self.chunk.len() - loop_start + 2) as i16;
                self.emit_i16(-back_jump);

                self.patch_jump(exit_jump);

                let breaks = self.loop_breaks.pop().unwrap();
                for brk in breaks {
                    self.patch_jump(brk);
                }
                self.loop_starts.pop();
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                self.begin_scope();

                if let Some(init) = init {
                    self.compile_stmt(init)?;
                }

                let loop_start = self.chunk.len();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(Vec::new());

                let exit_jump = if let Some(test) = test {
                    self.compile_expr(test)?;
                    Some(self.emit_jump(OpCode::JumpFalse))
                } else {
                    None
                };

                self.compile_stmt(body)?;

                if let Some(update) = update {
                    self.compile_expr(update)?;
                    self.emit(OpCode::Pop);
                }

                self.emit(OpCode::Jump);
                let back_jump = (self.chunk.len() - loop_start + 2) as i16;
                self.emit_i16(-back_jump);

                if let Some(exit) = exit_jump {
                    self.patch_jump(exit);
                }

                let breaks = self.loop_breaks.pop().unwrap();
                for brk in breaks {
                    self.patch_jump(brk);
                }
                self.loop_starts.pop();
                self.end_scope();
            }
            Stmt::Return(expr) => {
                if let Some(e) = expr {
                    self.compile_expr(e)?;
                } else {
                    self.emit(OpCode::PushUndefined);
                }
                self.emit(OpCode::Return);
            }
            Stmt::Break => {
                if self.loop_breaks.is_empty() {
                    return Err(JSError::syntax("break outside loop", 1, 1));
                }
                let jump = self.emit_jump(OpCode::Jump);
                self.loop_breaks.last_mut().unwrap().push(jump);
            }
            Stmt::Continue => {
                if let Some(&start) = self.loop_starts.last() {
                    self.emit(OpCode::Jump);
                    let back_jump = (self.chunk.len() - start + 2) as i16;
                    self.emit_i16(-back_jump);
                } else {
                    return Err(JSError::syntax("continue outside loop", 1, 1));
                }
            }
            Stmt::Throw(expr) => {
                self.compile_expr(expr)?;
                self.emit(OpCode::Throw);
            }
            Stmt::Function { name, params: _, body: _ } => {
                // TODO: Compile function properly
                self.emit(OpCode::PushUndefined);
                self.add_local(name.clone());
            }
            Stmt::Try { .. } => {
                return Err(JSError::InternalError("try/catch not yet implemented"));
            }
            Stmt::Empty => {}
        }
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Undefined => self.emit(OpCode::PushUndefined),
            Expr::Null => self.emit(OpCode::PushNull),
            Expr::Bool(true) => self.emit(OpCode::PushTrue),
            Expr::Bool(false) => self.emit(OpCode::PushFalse),
            Expr::Number(n) => {
                let n = *n;
                let is_int = n == libm::trunc(n);
                if is_int && n >= i8::MIN as f64 && n <= i8::MAX as f64 {
                    self.emit(OpCode::PushI8);
                    self.emit_byte(n as i8 as u8);
                } else if is_int && n >= i16::MIN as f64 && n <= i16::MAX as f64 {
                    self.emit(OpCode::PushI16);
                    self.emit_i16(n as i16);
                } else if is_int && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
                    self.emit(OpCode::PushI32);
                    let i = n as i32;
                    self.emit_byte((i >> 24) as u8);
                    self.emit_byte((i >> 16) as u8);
                    self.emit_byte((i >> 8) as u8);
                    self.emit_byte(i as u8);
                } else {
                    if let Some(idx) = self.chunk.add_float(n) {
                        self.emit(OpCode::PushFloat);
                        self.emit_u16(idx);
                    }
                }
            }
            Expr::String(s) => {
                if let Some(str_id) = self.strings.intern(s) {
                    if let Some(idx) = self.chunk.add_string(str_id.0) {
                        self.emit(OpCode::PushString);
                        self.emit_u16(idx);
                    }
                }
            }
            Expr::Identifier(name) => {
                if let Some(idx) = self.resolve_local(name) {
                    self.emit(OpCode::GetLocal);
                    self.emit_byte(idx);
                } else {
                    if let Some(str_id) = self.strings.intern(name) {
                        if let Some(idx) = self.chunk.add_string(str_id.0) {
                            self.emit(OpCode::GetGlobal);
                            self.emit_u16(idx);
                        }
                    }
                }
            }
            Expr::Binary { op, left, right } => {
                // Short-circuit for && and ||
                match op {
                    BinaryOp::And => {
                        self.compile_expr(left)?;
                        let jump = self.emit_jump(OpCode::JumpFalseKeep);
                        self.emit(OpCode::Pop);
                        self.compile_expr(right)?;
                        self.patch_jump(jump);
                        return Ok(());
                    }
                    BinaryOp::Or => {
                        self.compile_expr(left)?;
                        let jump = self.emit_jump(OpCode::JumpTrueKeep);
                        self.emit(OpCode::Pop);
                        self.compile_expr(right)?;
                        self.patch_jump(jump);
                        return Ok(());
                    }
                    _ => {}
                }

                self.compile_expr(left)?;
                self.compile_expr(right)?;
                match op {
                    BinaryOp::Add => self.emit(OpCode::Add),
                    BinaryOp::Sub => self.emit(OpCode::Sub),
                    BinaryOp::Mul => self.emit(OpCode::Mul),
                    BinaryOp::Div => self.emit(OpCode::Div),
                    BinaryOp::Mod => self.emit(OpCode::Mod),
                    BinaryOp::Pow => self.emit(OpCode::Pow),
                    BinaryOp::Eq => self.emit(OpCode::Eq),
                    BinaryOp::Ne => self.emit(OpCode::Ne),
                    BinaryOp::Lt => self.emit(OpCode::Lt),
                    BinaryOp::Le => self.emit(OpCode::Le),
                    BinaryOp::Gt => self.emit(OpCode::Gt),
                    BinaryOp::Ge => self.emit(OpCode::Ge),
                    BinaryOp::BitAnd => self.emit(OpCode::BitAnd),
                    BinaryOp::BitOr => self.emit(OpCode::BitOr),
                    BinaryOp::BitXor => self.emit(OpCode::BitXor),
                    BinaryOp::Shl => self.emit(OpCode::Shl),
                    BinaryOp::Shr => self.emit(OpCode::Shr),
                    BinaryOp::UShr => self.emit(OpCode::UShr),
                    BinaryOp::And | BinaryOp::Or => unreachable!(),
                    BinaryOp::NullishCoalesce => self.emit(OpCode::Nop), // TODO
                }
            }
            Expr::Unary { op, operand } => {
                self.compile_expr(operand)?;
                match op {
                    UnaryOp::Neg => self.emit(OpCode::Neg),
                    UnaryOp::Not => self.emit(OpCode::Not),
                    UnaryOp::BitNot => self.emit(OpCode::BitNot),
                    UnaryOp::TypeOf => self.emit(OpCode::TypeOf),
                    UnaryOp::Plus => self.emit(OpCode::Plus),
                    UnaryOp::PreInc | UnaryOp::PostInc => self.emit(OpCode::Inc),
                    UnaryOp::PreDec | UnaryOp::PostDec => self.emit(OpCode::Dec),
                }
            }
            Expr::Assign { target, value } => {
                self.compile_expr(value)?;
                self.emit(OpCode::Dup);

                match target.as_ref() {
                    Expr::Identifier(name) => {
                        if let Some(idx) = self.resolve_local(name) {
                            self.emit(OpCode::SetLocal);
                            self.emit_byte(idx);
                        } else {
                            if let Some(str_id) = self.strings.intern(name) {
                                if let Some(idx) = self.chunk.add_string(str_id.0) {
                                    self.emit(OpCode::SetGlobal);
                                    self.emit_u16(idx);
                                }
                            }
                        }
                    }
                    _ => return Err(JSError::syntax("Invalid assignment target", 1, 1)),
                }
            }
            Expr::Call { callee, args } => {
                self.compile_expr(callee)?;
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.emit(OpCode::Call);
                self.emit_byte(args.len() as u8);
            }
            Expr::Array(elements) => {
                self.emit(OpCode::NewArray);
                for elem in elements {
                    self.compile_expr(elem)?;
                    self.emit(OpCode::ArrayPush);
                }
            }
            Expr::Object(props) => {
                self.emit(OpCode::NewObject);
                for (key, value) in props {
                    if let Some(str_id) = self.strings.intern(key) {
                        if let Some(idx) = self.chunk.add_string(str_id.0) {
                            self.emit(OpCode::PushString);
                            self.emit_u16(idx);
                        }
                    }
                    self.compile_expr(value)?;
                    self.emit(OpCode::SetProp);
                }
            }
            Expr::Member { object, property, .. } => {
                self.compile_expr(object)?;
                self.compile_expr(property)?;
                self.emit(OpCode::GetProp);
            }
            Expr::Index { object, index } => {
                self.compile_expr(object)?;
                self.compile_expr(index)?;
                self.emit(OpCode::GetElem);
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.compile_expr(test)?;
                let else_jump = self.emit_jump(OpCode::JumpFalse);
                self.compile_expr(consequent)?;
                let end_jump = self.emit_jump(OpCode::Jump);
                self.patch_jump(else_jump);
                self.compile_expr(alternate)?;
                self.patch_jump(end_jump);
            }
            Expr::This => {
                self.emit(OpCode::PushUndefined); // TODO: proper this
            }
            Expr::Function { .. } | Expr::Arrow { .. } => {
                self.emit(OpCode::PushUndefined); // TODO: function compilation
            }
        }
        Ok(())
    }
}
