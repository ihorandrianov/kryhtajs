//! Arena-based bytecode compiler
//!
//! Compiles arena AST to bytecode. No heap allocation.

use crate::ast::{AstArena, BinaryOp, Expr, ExprId, Stmt, StmtId, UnaryOp};
use crate::bytecode::{Chunk, OpCode};
use crate::error::{JSError, Result};
use crate::fixed_collections::{FixedStack, FixedVec};
use crate::fixed_string::{FixedStringPool, StrId};
use crate::{MAX_STRINGS, MAX_STRING_BYTES};

/// Maximum local variables per function
const MAX_LOCALS: usize = 256;

/// Maximum loop nesting depth
const MAX_LOOP_DEPTH: usize = 16;

/// Maximum break statements per loop
const MAX_BREAKS_PER_LOOP: usize = 16;

/// Local variable
#[derive(Clone, Copy)]
struct Local {
    name: StrId,
    depth: u32,
}

/// Break jump addresses for a single loop
#[derive(Clone, Copy, Default)]
struct LoopBreaks {
    breaks: [usize; MAX_BREAKS_PER_LOOP],
    count: u8,
}

/// Arena-based compiler
pub struct Compiler<'a> {
    #[allow(dead_code)]
    strings: &'a FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
    ast: &'a AstArena,
    chunk: Chunk,
    locals: FixedVec<Local, MAX_LOCALS>,
    scope_depth: u32,
    loop_starts: FixedStack<usize, MAX_LOOP_DEPTH>,
    loop_breaks: FixedStack<LoopBreaks, MAX_LOOP_DEPTH>,
}

impl<'a> Compiler<'a> {
    pub fn new(
        strings: &'a FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS>,
        ast: &'a AstArena,
    ) -> Self {
        Self {
            strings,
            ast,
            chunk: Chunk::new(),
            locals: FixedVec::new(),
            scope_depth: 0,
            loop_starts: FixedStack::new(),
            loop_breaks: FixedStack::new(),
        }
    }

    pub fn compile(mut self) -> Result<Chunk> {
        // Compile root statements
        let stmts = self
            .ast
            .get_stmt_list(self.ast.root_start, self.ast.root_count);
        for &stmt_id in stmts {
            self.compile_stmt(stmt_id)?;
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
        self.chunk.code.set(offset, (jump >> 8) as u8);
        self.chunk.code.set(offset + 1, (jump & 0xff) as u8);
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

    fn add_local(&mut self, name: StrId) -> u8 {
        let idx = self.locals.len() as u8;
        self.locals.push(Local {
            name,
            depth: self.scope_depth,
        });
        idx
    }

    fn resolve_local(&self, name: StrId) -> Option<u8> {
        for i in (0..self.locals.len()).rev() {
            if let Some(local) = self.locals.get(i) {
                if local.name == name {
                    return Some(i as u8);
                }
            }
        }
        None
    }

    fn compile_stmt(&mut self, stmt_id: StmtId) -> Result<()> {
        let stmt = self
            .ast
            .get_stmt(stmt_id)
            .ok_or_else(|| JSError::InternalError("Invalid statement ID"))?;

        match stmt {
            Stmt::Empty => {}
            Stmt::Expr(expr_id) => {
                self.compile_expr(expr_id)?;
                self.emit(OpCode::Pop);
            }
            Stmt::Let { name, init } => {
                if init.is_some() {
                    self.compile_expr(init)?;
                } else {
                    self.emit(OpCode::PushUndefined);
                }
                self.add_local(name);
            }
            Stmt::Const { name, init } => {
                self.compile_expr(init)?;
                self.add_local(name);
            }
            Stmt::Block {
                stmts_start,
                stmts_count,
            } => {
                self.begin_scope();
                let stmts = self.ast.get_stmt_list(stmts_start, stmts_count);
                for &s in stmts {
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
                if alternate.is_some() {
                    let end_jump = self.emit_jump(OpCode::Jump);
                    self.patch_jump(else_jump);
                    self.compile_stmt(alternate)?;
                    self.patch_jump(end_jump);
                } else {
                    self.patch_jump(else_jump);
                }
            }
            Stmt::While { test, body } => {
                let loop_start = self.chunk.len();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(LoopBreaks::default());

                self.compile_expr(test)?;
                let exit_jump = self.emit_jump(OpCode::JumpFalse);
                self.compile_stmt(body)?;

                // Jump back to start
                self.emit(OpCode::Jump);
                let back_jump = (self.chunk.len() - loop_start + 2) as i16;
                self.emit_i16(-back_jump);

                self.patch_jump(exit_jump);

                // Patch breaks
                if let Some(breaks) = self.loop_breaks.pop() {
                    for i in 0..breaks.count as usize {
                        self.patch_jump(breaks.breaks[i]);
                    }
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

                if init.is_some() {
                    self.compile_stmt(init)?;
                }

                let loop_start = self.chunk.len();
                self.loop_starts.push(loop_start);
                self.loop_breaks.push(LoopBreaks::default());

                let exit_jump = if test.is_some() {
                    self.compile_expr(test)?;
                    Some(self.emit_jump(OpCode::JumpFalse))
                } else {
                    None
                };

                self.compile_stmt(body)?;

                if update.is_some() {
                    self.compile_expr(update)?;
                    self.emit(OpCode::Pop);
                }

                self.emit(OpCode::Jump);
                let back_jump = (self.chunk.len() - loop_start + 2) as i16;
                self.emit_i16(-back_jump);

                if let Some(exit) = exit_jump {
                    self.patch_jump(exit);
                }

                // Patch breaks
                if let Some(breaks) = self.loop_breaks.pop() {
                    for i in 0..breaks.count as usize {
                        self.patch_jump(breaks.breaks[i]);
                    }
                }
                self.loop_starts.pop();
                self.end_scope();
            }
            Stmt::Return(expr_id) => {
                if expr_id.is_some() {
                    self.compile_expr(expr_id)?;
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
                if let Some(breaks) = self.loop_breaks.get_mut(self.loop_breaks.len() - 1) {
                    if (breaks.count as usize) < MAX_BREAKS_PER_LOOP {
                        breaks.breaks[breaks.count as usize] = jump;
                        breaks.count += 1;
                    }
                }
            }
            Stmt::Continue => {
                if let Some(&start) = self.loop_starts.peek() {
                    self.emit(OpCode::Jump);
                    let back_jump = (self.chunk.len() - start + 2) as i16;
                    self.emit_i16(-back_jump);
                } else {
                    return Err(JSError::syntax("continue outside loop", 1, 1));
                }
            }
            Stmt::Throw(expr_id) => {
                self.compile_expr(expr_id)?;
                self.emit(OpCode::Throw);
            }
            Stmt::Function {
                name,
                params_start: _,
                params_count: _,
                body: _,
            } => {
                // TODO: Compile function properly
                self.emit(OpCode::PushUndefined);
                self.add_local(name);
            }
            Stmt::Try { .. } => {
                return Err(JSError::InternalError("try/catch not yet implemented"));
            }
        }
        Ok(())
    }

    fn compile_expr(&mut self, expr_id: ExprId) -> Result<()> {
        let expr = self
            .ast
            .get_expr(expr_id)
            .ok_or_else(|| JSError::InternalError("Invalid expression ID"))?;

        match expr {
            Expr::Empty => {
                self.emit(OpCode::PushUndefined);
            }
            Expr::Undefined => self.emit(OpCode::PushUndefined),
            Expr::Null => self.emit(OpCode::PushNull),
            Expr::Bool(true) => self.emit(OpCode::PushTrue),
            Expr::Bool(false) => self.emit(OpCode::PushFalse),
            Expr::Int(n) => {
                if n >= i8::MIN as i32 && n <= i8::MAX as i32 {
                    self.emit(OpCode::PushI8);
                    self.emit_byte(n as i8 as u8);
                } else if n >= i16::MIN as i32 && n <= i16::MAX as i32 {
                    self.emit(OpCode::PushI16);
                    self.emit_i16(n as i16);
                } else {
                    self.emit(OpCode::PushI32);
                    self.emit_byte((n >> 24) as u8);
                    self.emit_byte((n >> 16) as u8);
                    self.emit_byte((n >> 8) as u8);
                    self.emit_byte(n as u8);
                }
            }
            Expr::Float(idx) => {
                if let Some(f) = self.ast.floats.get(idx as usize) {
                    if let Some(idx) = self.chunk.add_float(f) {
                        self.emit(OpCode::PushFloat);
                        self.emit_u16(idx);
                    }
                }
            }
            Expr::String(str_id) => {
                if let Some(idx) = self.chunk.add_string(str_id.0) {
                    self.emit(OpCode::PushString);
                    self.emit_u16(idx);
                }
            }
            Expr::Identifier(name) => {
                if let Some(idx) = self.resolve_local(name) {
                    self.emit(OpCode::GetLocal);
                    self.emit_byte(idx);
                } else {
                    if let Some(idx) = self.chunk.add_string(name.0) {
                        self.emit(OpCode::GetGlobal);
                        self.emit_u16(idx);
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

                let target_expr = self
                    .ast
                    .get_expr(target)
                    .ok_or_else(|| JSError::InternalError("Invalid target expression"))?;

                match target_expr {
                    Expr::Identifier(name) => {
                        if let Some(idx) = self.resolve_local(name) {
                            self.emit(OpCode::SetLocal);
                            self.emit_byte(idx);
                        } else {
                            if let Some(idx) = self.chunk.add_string(name.0) {
                                self.emit(OpCode::SetGlobal);
                                self.emit_u16(idx);
                            }
                        }
                    }
                    Expr::Member { .. } => {
                        return Err(JSError::syntax("Property assignment not yet supported", 1, 1));
                    }
                    Expr::Index { .. } => {
                        return Err(JSError::syntax("Index assignment not yet supported", 1, 1));
                    }
                    _ => return Err(JSError::syntax("Invalid assignment target", 1, 1)),
                }
            }
            Expr::Call {
                callee,
                args_start,
                args_count,
            } => {
                self.compile_expr(callee)?;
                let args = self.ast.get_expr_list(args_start, args_count);
                for &arg in args {
                    self.compile_expr(arg)?;
                }
                self.emit(OpCode::Call);
                self.emit_byte(args_count);
            }
            Expr::Array {
                elems_start,
                elems_count,
            } => {
                self.emit(OpCode::NewArray);
                let elems = self.ast.get_expr_list(elems_start, elems_count);
                for &elem in elems {
                    self.compile_expr(elem)?;
                    self.emit(OpCode::ArrayPush);
                }
            }
            Expr::Object {
                props_start,
                props_count,
            } => {
                self.emit(OpCode::NewObject);
                let props = self.ast.get_prop_list(props_start, props_count);
                for prop in props {
                    if let Some(idx) = self.chunk.add_string(prop.key.0) {
                        self.emit(OpCode::PushString);
                        self.emit_u16(idx);
                    }
                    self.compile_expr(prop.value)?;
                    self.emit(OpCode::SetProp);
                }
            }
            Expr::Member { object, property } => {
                self.compile_expr(object)?;
                if let Some(idx) = self.chunk.add_string(property.0) {
                    self.emit(OpCode::PushString);
                    self.emit_u16(idx);
                }
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
