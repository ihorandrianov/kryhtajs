//! CEKH Machine - Control, Environment, Kontinuation, Handlers
//!
//! A CEK machine evaluates expressions by maintaining explicit state:
//! - C (Control): What we're currently evaluating
//! - E (Environment): Variable bindings
//! - K (Kontinuation): What to do with the result
//! - H (Handlers): Effect handlers (user implements semantics)
//!
//! This design makes closures trivial (capture E) and enables
//! first-class continuations for algebraic effects.

use std::collections::HashMap;

use crate::arena::Arena;
use crate::ast::{AstArena, BinaryOp, Expr, ExprId, Pattern, PatternId, Stmt, StmtId, UnaryOp};
use crate::builtins::call_native;
use crate::cont::{ContArena, ContId, Kont};
use crate::env::{EnvArena, EnvId};
use crate::error::{JSError, Result};
use crate::object::{FunctionData, NativeFn, Object, ObjectKind, Property};
use crate::runtime::Outcome;
use crate::string_pool::{StrId, StringPool};
use crate::value::{JSValue, ObjId};
use crate::{Handler, HandlerArena};

/// Control - what we're currently evaluating
#[derive(Clone, Debug)]
pub enum Control {
    /// Evaluate an expression
    Expr(ExprId),
    /// Execute a statement
    Stmt(StmtId),
    /// We have a value, apply continuation
    Value(JSValue),
    /// Returning from function
    Returning(JSValue),
    /// Halted - execution complete
    Halted(JSValue),
    /// Suspend - user space effect found
    Suspend { effect: StrId, args: Vec<JSValue> },
}

/// The CEKH machine state
pub struct CEKH {
    /// Current control
    pub control: Control,
    /// Current environment
    pub env: EnvId,
    /// Current continuation
    pub cont: ContId,

    /// Environment arena
    pub envs: EnvArena,
    /// Continuation arena
    pub conts: ContArena,
    /// Handler arena
    pub handlers: HandlerArena,
    /// Object arena
    pub objects: Arena<Object>,
    /// String pool
    pub strings: StringPool,

    /// Global variables
    pub globals: HashMap<StrId, JSValue>,
}

impl CEKH {
    pub fn new() -> Self {
        let envs = EnvArena::new();
        let conts = ContArena::new();
        let handlers = HandlerArena::new();
        let env = envs.global();
        let cont = conts.halt();

        let mut machine = Self {
            control: Control::Value(JSValue::Undefined),
            env,
            cont,
            envs,
            handlers,
            conts,
            objects: Arena::new(),
            strings: StringPool::new(),
            globals: HashMap::new(),
        };

        machine.setup_builtins();
        machine
    }

    /// Cumulative allocations across all GC-managed arenas.
    pub fn total_allocations(&self) -> u64 {
        self.objects.allocations()
            + self.envs.allocations()
            + self.conts.allocations()
            + self.handlers.allocations()
    }

    pub fn fresh_exec_setup(&mut self, ast: &AstArena) -> Result<JSValue> {
        let stmts = ast.get_stmt_list(ast.root_start, ast.root_count);
        if stmts.is_empty() {
            return Ok(JSValue::Undefined);
        }

        self.control = Control::Stmt(stmts[0]);
        self.env = self.envs.global();
        self.cont = if stmts.len() > 1 {
            self.conts.alloc(Kont::SeqK {
                stmts_start: ast.root_start,
                stmts_idx: 1,
                stmts_count: ast.root_count,
                env: self.env,
                k: self.conts.halt(),
            })
        } else {
            self.conts.halt()
        };

        Ok(JSValue::Null)
    }

    pub fn run(&mut self, ast: &AstArena) -> Result<Outcome> {
        loop {
            match self.control {
                Control::Halted(val) => return Ok(Outcome::Done(val)),
                Control::Suspend {
                    effect,
                    ref mut args,
                } => {
                    return Ok(Outcome::Suspended {
                        effect: effect,
                        args: std::mem::take(args),
                    });
                }
                _ => self.step(ast)?,
            }
        }
    }

    pub fn step(&mut self, ast: &AstArena) -> Result<()> {
        match std::mem::replace(&mut self.control, Control::Value(JSValue::Undefined)) {
            Control::Expr(expr_id) => self.step_expr(expr_id, ast),
            Control::Stmt(stmt_id) => self.step_stmt(stmt_id, ast),
            Control::Value(val) => self.apply_cont(val, ast),
            Control::Returning(val) => self.handle_return(val, ast),
            Control::Halted(val) => {
                self.control = Control::Halted(val);
                Ok(())
            }
            Control::Suspend { effect, args } => {
                self.control = Control::Suspend { effect, args };
                Ok(())
            }
        }
    }

    fn step_expr(&mut self, expr_id: ExprId, ast: &AstArena) -> Result<()> {
        let expr = ast
            .get_expr(expr_id)
            .ok_or(JSError::InternalError("Invalid expression ID"))?;

        match expr {
            Expr::Empty | Expr::Undefined => {
                self.control = Control::Value(JSValue::Undefined);
            }
            Expr::Null => {
                self.control = Control::Value(JSValue::Null);
            }
            Expr::Bool(b) => {
                self.control = Control::Value(JSValue::Bool(b));
            }
            Expr::Int(n) => {
                self.control = Control::Value(JSValue::Int(n));
            }
            Expr::Float(idx) => {
                let f = ast.get_float(idx).unwrap_or(0.0);
                self.control = Control::Value(JSValue::Float(f));
            }
            Expr::String(str_id) => {
                self.control = Control::Value(JSValue::String(str_id));
            }

            Expr::Identifier(name) => {
                if let Some(val) = self.envs.lookup(self.env, name) {
                    self.control = Control::Value(val);
                } else if let Some(val) = self.globals.get(&name) {
                    self.control = Control::Value(*val);
                } else {
                    self.control = Control::Value(JSValue::Undefined);
                }
            }

            Expr::This => {
                // For now, this is undefined (no this binding yet)
                self.control = Control::Value(JSValue::Undefined);
            }

            Expr::Unary { op, operand } => {
                // For increment/decrement on identifiers, we need to update the variable
                let is_update = matches!(
                    op,
                    UnaryOp::PreInc | UnaryOp::PostInc | UnaryOp::PreDec | UnaryOp::PostDec
                );
                if is_update {
                    if let Some(Expr::Identifier(name)) = ast.get_expr(operand) {
                        // Read current value, then use UpdateVarK to update and return
                        self.control = Control::Expr(operand);
                        let is_pre = matches!(op, UnaryOp::PreInc | UnaryOp::PreDec);
                        let is_inc = matches!(op, UnaryOp::PreInc | UnaryOp::PostInc);
                        self.cont = self.conts.alloc(Kont::UpdateVarK {
                            name,
                            is_pre,
                            is_inc,
                            env: self.env,
                            k: self.cont,
                        });
                    } else {
                        // Non-identifier operand - just evaluate and apply (no side effect)
                        self.control = Control::Expr(operand);
                        self.cont = self.conts.alloc(Kont::UnaryK { op, k: self.cont });
                    }
                } else {
                    self.control = Control::Expr(operand);
                    self.cont = self.conts.alloc(Kont::UnaryK { op, k: self.cont });
                }
            }

            Expr::Binary { op, left, right } => {
                // Short-circuit operators need special handling
                match op {
                    BinaryOp::And => {
                        self.control = Control::Expr(left);
                        self.cont = self.conts.alloc(Kont::AndK {
                            right,
                            env: self.env,
                            k: self.cont,
                        });
                    }
                    BinaryOp::Or => {
                        self.control = Control::Expr(left);
                        self.cont = self.conts.alloc(Kont::OrK {
                            right,
                            env: self.env,
                            k: self.cont,
                        });
                    }
                    BinaryOp::NullishCoalesce => {
                        self.control = Control::Expr(left);
                        self.cont = self.conts.alloc(Kont::NullishK {
                            right,
                            env: self.env,
                            k: self.cont,
                        });
                    }
                    _ => {
                        self.control = Control::Expr(left);
                        self.cont = self.conts.alloc(Kont::BinaryLeftK {
                            op,
                            right,
                            env: self.env,
                            k: self.cont,
                        });
                    }
                }
            }

            Expr::Member { object, property } => {
                self.control = Control::Expr(object);
                self.cont = self.conts.alloc(Kont::MemberK {
                    property,
                    k: self.cont,
                });
            }

            Expr::Index { object, index } => {
                self.control = Control::Expr(object);
                self.cont = self.conts.alloc(Kont::IndexObjK {
                    index,
                    env: self.env,
                    k: self.cont,
                });
            }

            Expr::Assign { target, value } => {
                if let Some(target_expr) = ast.get_expr(target) {
                    match target_expr {
                        Expr::Identifier(name) => {
                            self.control = Control::Expr(value);
                            self.cont = self.conts.alloc(Kont::AssignVarK {
                                name,
                                env: self.env,
                                k: self.cont,
                            });
                        }
                        Expr::Member { object, property } => {
                            self.control = Control::Expr(object);
                            self.cont = self.conts.alloc(Kont::AssignMemberObjK {
                                property,
                                value,
                                env: self.env,
                                k: self.cont,
                            });
                        }
                        Expr::Index { object, index } => {
                            self.control = Control::Expr(object);
                            self.cont = self.conts.alloc(Kont::AssignIndexObjK {
                                index,
                                value,
                                env: self.env,
                                k: self.cont,
                            });
                        }
                        _ => {
                            return Err(JSError::type_error("Invalid assignment target"));
                        }
                    }
                } else {
                    return Err(JSError::InternalError("Invalid assignment target"));
                }
            }

            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.control = Control::Expr(test);
                self.cont = self.conts.alloc(Kont::CondK {
                    consequent,
                    alternate,
                    env: self.env,
                    k: self.cont,
                });
            }

            Expr::Array {
                elems_start,
                elems_count,
            } => {
                if elems_count == 0 {
                    let arr_obj = Object::array();
                    let arr_id = self.objects.alloc(arr_obj);
                    self.control = Control::Value(JSValue::Array(ObjId(arr_id.index() as u32)));
                } else {
                    let elems = ast.get_expr_list(elems_start, elems_count);
                    self.control = Control::Expr(elems[0]);
                    self.cont = self.conts.alloc(Kont::ArrayK {
                        done: Vec::new(),
                        elems_start,
                        elems_idx: 1,
                        elems_count,
                        env: self.env,
                        k: self.cont,
                    });
                }
            }

            Expr::Object {
                props_start,
                props_count,
            } => {
                if props_count == 0 {
                    let obj = Object::new();
                    let obj_id = self.objects.alloc(obj);
                    self.control = Control::Value(JSValue::Object(ObjId(obj_id.index() as u32)));
                } else {
                    let props = ast.get_prop_list(props_start, props_count);
                    self.control = Control::Expr(props[0].value);
                    self.cont = self.conts.alloc(Kont::ObjectK {
                        done: Vec::new(),
                        props_start,
                        props_idx: 1,
                        props_count,
                        env: self.env,
                        k: self.cont,
                    });
                }
            }

            Expr::Call {
                callee,
                args_start,
                args_count,
            } => {
                self.control = Control::Expr(callee);
                self.cont = self.conts.alloc(Kont::CalleeK {
                    args_start,
                    args_count,
                    env: self.env,
                    k: self.cont,
                });
            }

            Expr::Function {
                name,
                params_start,
                params_count,
                body,
            } => {
                let func_data = FunctionData::new(
                    params_start,
                    params_count,
                    body,
                    self.env, // Capture current environment
                    if name.0 != 0 { Some(name) } else { None },
                );
                let func_obj = Object::closure(func_data);
                let func_id = self.objects.alloc(func_obj);
                self.control = Control::Value(JSValue::Function(ObjId(func_id.index() as u32)));
            }

            Expr::Arrow {
                params_start,
                params_count,
                body,
                is_block,
            } => {
                let func_data = if is_block {
                    // Arrow with block body: body is actually a StmtId stored as ExprId
                    // This is a parser quirk - the body ExprId contains a StmtId value
                    FunctionData::arrow_block(params_start, params_count, StmtId(body.0), self.env)
                } else {
                    FunctionData::arrow_expr(params_start, params_count, body, self.env)
                };
                let func_obj = Object::closure(func_data);
                let func_id = self.objects.alloc(func_obj);
                self.control = Control::Value(JSValue::Function(ObjId(func_id.index() as u32)));
            }

            Expr::Block {
                stmts_start,
                stmts_count,
                final_expr,
            } => {
                if stmts_count == 0 {
                    if final_expr.is_none() {
                        self.control = Control::Value(JSValue::Undefined);
                    } else {
                        self.control = Control::Expr(final_expr);
                    }
                } else {
                    let stmts = ast.get_stmt_list(stmts_start, stmts_count);
                    self.control = Control::Stmt(stmts[0]);
                    self.cont = self.conts.alloc(Kont::BlockK {
                        stmts_start,
                        stmts_idx: 1,
                        stmts_count,
                        final_expr,
                        env: self.env,
                        k: self.cont,
                    });
                }
            }

            Expr::Match {
                scrutinee,
                arms_start,
                arms_count,
            } => {
                self.control = Control::Expr(scrutinee);
                self.cont = self.conts.alloc(Kont::MatchK {
                    arms_start,
                    arms_idx: 0,
                    arms_count,
                    env: self.env,
                    k: self.cont,
                });
            }

            Expr::Perform {
                effect,
                args_start,
                args_count,
            } => {
                if args_count > 0 {
                    let args = ast.get_expr_list(args_start, args_count);
                    self.control = Control::Expr(args[0]);
                    self.cont = self.conts.alloc(Kont::PerformArgsK {
                        effect,
                        args_start,
                        done: Vec::with_capacity(args_count as usize),
                        args_idx: 1,
                        args_count,
                        env: self.env,
                        k: self.cont,
                    });
                } else {
                    self.do_perform(effect, Vec::new(), ast)?;
                }
            }

            Expr::Handle {
                body,
                clauses_start,
                clauses_count,
                return_param,
                return_body,
            } => {
                self.cont = self.conts.alloc(Kont::HandlerK {
                    clauses_start,
                    clauses_count,
                    env: self.env,
                    return_body,
                    return_param,
                    k: self.cont,
                });
                self.control = Control::Expr(body);
            }

            Expr::Handler {
                clauses_start,
                clauses_count,
                return_param,
                return_body,
            } => {
                let handler = Handler::new(
                    clauses_start,
                    clauses_count,
                    return_param,
                    return_body,
                    self.env,
                );
                let handler_id = self.handlers.alloc(handler);
                self.control = Control::Value(JSValue::Handler(handler_id));
            }

            Expr::HandleWith { body, handler } => {
                self.control = Control::Expr(handler);
                self.cont = self.conts.alloc(Kont::HandleWithK {
                    body,
                    env: self.env,
                    k: self.cont,
                });
            }
        }

        Ok(())
    }

    fn step_stmt(&mut self, stmt_id: StmtId, ast: &AstArena) -> Result<()> {
        if stmt_id.is_none() {
            self.control = Control::Value(JSValue::Undefined);
            return Ok(());
        }

        let stmt = ast
            .get_stmt(stmt_id)
            .ok_or(JSError::InternalError("Invalid statement ID"))?;

        match stmt {
            Stmt::Empty => {
                self.control = Control::Value(JSValue::Undefined);
            }

            Stmt::Expr(expr) => {
                self.control = Control::Expr(expr);
                self.cont = self.conts.alloc(Kont::ExprStmtK { k: self.cont });
            }

            Stmt::Let { name, init } => {
                if init.is_some() {
                    self.control = Control::Expr(init);
                    self.cont = self.conts.alloc(Kont::LetK {
                        name,
                        env: self.env,
                        k: self.cont,
                    });
                } else {
                    self.envs.define(self.env, name, JSValue::Undefined);
                    self.control = Control::Value(JSValue::Undefined);
                }
            }

            Stmt::Const { name, init } => {
                if init.is_some() {
                    self.control = Control::Expr(init);
                    self.cont = self.conts.alloc(Kont::ConstK {
                        name,
                        env: self.env,
                        k: self.cont,
                    });
                } else {
                    return Err(JSError::syntax_error("const requires initializer"));
                }
            }

            Stmt::Var { name, init } => {
                if init.is_some() {
                    self.control = Control::Expr(init);
                    self.cont = self.conts.alloc(Kont::VarK {
                        name,
                        env: self.env,
                        k: self.cont,
                    });
                } else {
                    self.globals.insert(name, JSValue::Undefined);
                    self.control = Control::Value(JSValue::Undefined);
                }
            }

            Stmt::If {
                test,
                consequent,
                alternate,
            } => {
                self.control = Control::Expr(test);
                self.cont = self.conts.alloc(Kont::IfK {
                    consequent,
                    alternate,
                    env: self.env,
                    k: self.cont,
                });
            }

            Stmt::While { test, body } => {
                self.control = Control::Expr(test);
                self.cont = self.conts.alloc(Kont::WhileK {
                    test,
                    body,
                    env: self.env,
                    k: self.cont,
                });
            }

            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                let loop_env = self.envs.extend(self.env);

                if init.is_some() {
                    self.control = Control::Stmt(init);
                    self.env = loop_env;
                    self.cont = self.conts.alloc(Kont::ForTestK {
                        test,
                        update,
                        body,
                        env: loop_env,
                        k: self.cont,
                    });
                } else {
                    self.control = Control::Expr(test);
                    self.env = loop_env;
                    self.cont = self.conts.alloc(Kont::ForTestK {
                        test,
                        update,
                        body,
                        env: loop_env,
                        k: self.cont,
                    });
                }
            }

            Stmt::Block {
                stmts_start,
                stmts_count,
            } => {
                let stmts = ast.get_stmt_list(stmts_start, stmts_count);
                if stmts.is_empty() {
                    self.control = Control::Value(JSValue::Undefined);
                } else {
                    let block_env = self.envs.extend(self.env);
                    self.control = Control::Stmt(stmts[0]);
                    self.env = block_env;
                    if stmts.len() > 1 {
                        self.cont = self.conts.alloc(Kont::SeqK {
                            stmts_start,
                            stmts_idx: 1,
                            stmts_count,
                            env: block_env,
                            k: self.cont,
                        });
                    }
                }
            }

            Stmt::Declarations {
                stmts_start,
                stmts_count,
            } => {
                let stmts = ast.get_stmt_list(stmts_start, stmts_count);
                if stmts.is_empty() {
                    self.control = Control::Value(JSValue::Undefined);
                } else {
                    self.control = Control::Stmt(stmts[0]);
                    if stmts.len() > 1 {
                        self.cont = self.conts.alloc(Kont::SeqK {
                            stmts_start,
                            stmts_idx: 1,
                            stmts_count,
                            env: self.env,
                            k: self.cont,
                        });
                    }
                }
            }

            Stmt::Return(expr) => {
                if expr.is_some() {
                    self.control = Control::Expr(expr);
                    self.cont = self.conts.alloc(Kont::ReturnExprK { k: self.cont });
                } else {
                    self.control = Control::Returning(JSValue::Undefined);
                }
            }

            Stmt::Break => {
                self.handle_break()?;
            }

            Stmt::Continue => {
                self.handle_continue(ast)?;
            }

            Stmt::Function {
                name,
                params_start,
                params_count,
                body,
            } => {
                let func_data =
                    FunctionData::new(params_start, params_count, body, self.env, Some(name));
                let func_obj = Object::closure(func_data);
                let func_id = self.objects.alloc(func_obj);
                let func_val = JSValue::Function(ObjId(func_id.index() as u32));

                self.envs.define(self.env, name, func_val);
                self.globals.insert(name, func_val);
                self.control = Control::Value(JSValue::Undefined);
            }
        }

        Ok(())
    }

    fn apply_cont(&mut self, val: JSValue, ast: &AstArena) -> Result<()> {
        // Take ownership to avoid cloning. The old continuation slot becomes Halt
        // (which is fine - we're done with it, and GC will eventually reclaim it).
        let kont = self
            .conts
            .take(self.cont)
            .ok_or(JSError::InternalError("Invalid continuation"))?;

        match kont {
            Kont::Halt => {
                self.control = Control::Halted(val);
            }

            Kont::UnaryK { op, k } => {
                let result = self.apply_unary(op, val)?;
                self.control = Control::Value(result);
                self.cont = k;
            }

            Kont::BinaryLeftK { op, right, env, k } => {
                self.control = Control::Expr(right);
                self.env = env;
                self.cont = self.conts.alloc(Kont::BinaryRightK { op, left: val, k });
            }

            Kont::BinaryRightK { op, left, k } => {
                let result = self.apply_binary(op, left, val)?;
                self.control = Control::Value(result);
                self.cont = k;
            }

            Kont::AndK { right, env, k } => {
                if !self.is_truthy(val) {
                    self.control = Control::Value(val);
                    self.cont = k;
                } else {
                    self.control = Control::Expr(right);
                    self.env = env;
                    self.cont = k;
                }
            }

            Kont::OrK { right, env, k } => {
                if self.is_truthy(val) {
                    self.control = Control::Value(val);
                    self.cont = k;
                } else {
                    self.control = Control::Expr(right);
                    self.env = env;
                    self.cont = k;
                }
            }

            Kont::NullishK { right, env, k } => {
                if !matches!(val, JSValue::Null | JSValue::Undefined) {
                    self.control = Control::Value(val);
                    self.cont = k;
                } else {
                    self.control = Control::Expr(right);
                    self.env = env;
                    self.cont = k;
                }
            }

            Kont::MemberK { property, k } => {
                let result = self.get_property(val, property)?;
                self.control = Control::Value(result);
                self.cont = k;
            }

            Kont::IndexObjK { index, env, k } => {
                self.control = Control::Expr(index);
                self.env = env;
                self.cont = self.conts.alloc(Kont::IndexKeyK { obj: val, k });
            }

            Kont::IndexKeyK { obj, k } => {
                let result = self.get_index(obj, val)?;
                self.control = Control::Value(result);
                self.cont = k;
            }

            Kont::AssignVarK { name, env, k } => {
                if !self.envs.assign(env, name, val) {
                    self.globals.insert(name, val);
                }
                self.control = Control::Value(val);
                self.cont = k;
            }

            Kont::UpdateVarK {
                name,
                is_pre,
                is_inc,
                env,
                k,
            } => {
                // val is the current value of the variable
                let old_val = val;
                let n = self.to_number(old_val);
                let new_n = if is_inc { n + 1.0 } else { n - 1.0 };
                let new_val = self.number_value(new_n);

                // Update the variable
                if !self.envs.assign(env, name, new_val) {
                    self.globals.insert(name, new_val);
                }

                // Return old (post) or new (pre) value
                let result = if is_pre { new_val } else { old_val };
                self.control = Control::Value(result);
                self.cont = k;
            }

            Kont::AssignMemberObjK {
                property,
                value,
                env,
                k,
            } => {
                self.control = Control::Expr(value);
                self.env = env;
                self.cont = self.conts.alloc(Kont::AssignMemberValK {
                    obj: val,
                    property,
                    k,
                });
            }

            Kont::AssignMemberValK { obj, property, k } => {
                self.set_property(obj, property, val)?;
                self.control = Control::Value(val);
                self.cont = k;
            }

            Kont::AssignIndexObjK {
                index,
                value,
                env,
                k,
            } => {
                self.control = Control::Expr(index);
                self.env = env;
                self.cont = self.conts.alloc(Kont::AssignIndexKeyK {
                    obj: val,
                    value,
                    env,
                    k,
                });
            }

            Kont::AssignIndexKeyK { obj, value, env, k } => {
                self.control = Control::Expr(value);
                self.env = env;
                self.cont = self.conts.alloc(Kont::AssignIndexValK { obj, key: val, k });
            }

            Kont::AssignIndexValK { obj, key, k } => {
                self.set_index(obj, key, val)?;
                self.control = Control::Value(val);
                self.cont = k;
            }

            Kont::IfK {
                consequent,
                alternate,
                env,
                k,
            } => {
                self.env = env;
                self.cont = k;
                if self.is_truthy(val) {
                    if consequent.is_some() {
                        self.control = Control::Stmt(consequent);
                    } else {
                        self.control = Control::Value(JSValue::Undefined);
                    }
                } else if alternate.is_some() {
                    self.control = Control::Stmt(alternate);
                } else {
                    self.control = Control::Value(JSValue::Undefined);
                }
            }

            Kont::CondK {
                consequent,
                alternate,
                env,
                k,
            } => {
                self.env = env;
                self.cont = k;
                if self.is_truthy(val) {
                    self.control = Control::Expr(consequent);
                } else {
                    self.control = Control::Expr(alternate);
                }
            }

            Kont::WhileK { test, body, env, k } => {
                if self.is_truthy(val) {
                    self.control = Control::Stmt(body);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::WhileBodyK { test, body, env, k });
                } else {
                    self.control = Control::Value(JSValue::Undefined);
                    self.cont = k;
                }
            }

            Kont::WhileBodyK { test, body, env, k } => {
                self.control = Control::Expr(test);
                self.env = env;
                self.cont = self.conts.alloc(Kont::WhileK { test, body, env, k });
            }

            Kont::ForTestK {
                test,
                update,
                body,
                env,
                k,
            } => {
                if test.is_some() {
                    self.control = Control::Expr(test);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ForTestResultK {
                        test,
                        update,
                        body,
                        env,
                        k,
                    });
                } else {
                    self.control = Control::Stmt(body);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ForBodyK {
                        test,
                        update,
                        body,
                        env,
                        k,
                    });
                }
            }

            Kont::ForTestResultK {
                test,
                update,
                body,
                env,
                k,
            } => {
                if self.is_truthy(val) {
                    self.control = Control::Stmt(body);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ForBodyK {
                        test,
                        update,
                        body,
                        env,
                        k,
                    });
                } else {
                    self.control = Control::Value(JSValue::Undefined);
                    self.cont = k;
                }
            }

            Kont::ForBodyK {
                test,
                update,
                body,
                env,
                k,
            } => {
                if update.is_some() {
                    self.control = Control::Expr(update);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ForUpdateK {
                        test,
                        update,
                        body,
                        env,
                        k,
                    });
                } else {
                    if test.is_some() {
                        self.control = Control::Expr(test);
                    } else {
                        self.control = Control::Value(JSValue::Bool(true));
                    }
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ForTestK {
                        test,
                        update,
                        body,
                        env,
                        k,
                    });
                }
            }

            Kont::ForUpdateK {
                test,
                update,
                body,
                env,
                k,
            } => {
                if test.is_some() {
                    self.control = Control::Expr(test);
                } else {
                    self.control = Control::Value(JSValue::Bool(true));
                }
                self.env = env;
                self.cont = self.conts.alloc(Kont::ForTestK {
                    test,
                    update,
                    body,
                    env,
                    k,
                });
            }

            Kont::CalleeK {
                args_start,
                args_count,
                env,
                k,
            } => {
                if args_count == 0 {
                    self.call_function(val, Vec::new(), k, ast)?;
                } else {
                    let args = ast.get_expr_list(args_start, args_count);
                    self.control = Control::Expr(args[0]);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ArgsK {
                        callee: val,
                        done: Vec::with_capacity(args_count as usize),
                        args_start,
                        args_idx: 1,
                        args_count,
                        env,
                        k,
                    });
                }
            }

            Kont::ArgsK {
                callee,
                mut done,
                args_start,
                args_idx,
                args_count,
                env,
                k,
            } => {
                done.push(val);

                if args_idx >= args_count {
                    self.call_function(callee, done, k, ast)?;
                } else {
                    let args = ast.get_expr_list(args_start, args_count);
                    self.control = Control::Expr(args[args_idx as usize]);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ArgsK {
                        callee,
                        done,
                        args_start,
                        args_idx: args_idx + 1,
                        args_count,
                        env,
                        k,
                    });
                }
            }

            Kont::HandleWithK { body, env, k } => {
                let JSValue::Handler(handler_id) = val else {
                    return Err(JSError::syntax_error("Expected handler"));
                };

                let handler = self
                    .handlers
                    .get(handler_id)
                    .ok_or(JSError::runtime_error("Invalid handler"))?;

                self.control = Control::Expr(body);
                self.env = env;
                self.cont = self.conts.alloc(Kont::HandlerK {
                    clauses_start: handler.clauses_start,
                    clauses_count: handler.clauses_count,
                    env: handler.env,
                    return_param: handler.return_param,
                    return_body: handler.return_body,
                    k,
                });
            }

            Kont::ReturnK { env, k } => {
                self.env = env;
                self.cont = k;
                self.control = Control::Value(val);
            }

            Kont::ReturnExprK { k } => {
                self.cont = k;
                self.control = Control::Returning(val);
            }

            Kont::ArrayK {
                mut done,
                elems_start,
                elems_idx,
                elems_count,
                env,
                k,
            } => {
                done.push(val);

                if elems_idx >= elems_count {
                    let mut arr_obj = Object::array();
                    if let ObjectKind::Array(ref mut data) = arr_obj.kind {
                        data.elements = done;
                    }
                    let arr_id = self.objects.alloc(arr_obj);
                    self.control = Control::Value(JSValue::Array(ObjId(arr_id.index() as u32)));
                    self.cont = k;
                } else {
                    let elems = ast.get_expr_list(elems_start, elems_count);
                    self.control = Control::Expr(elems[elems_idx as usize]);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ArrayK {
                        done,
                        elems_start,
                        elems_idx: elems_idx + 1,
                        elems_count,
                        env,
                        k,
                    });
                }
            }

            Kont::ObjectK {
                mut done,
                props_start,
                props_idx,
                props_count,
                env,
                k,
            } => {
                let props = ast.get_prop_list(props_start, props_count);
                let key = props[props_idx as usize - 1].key;
                done.push((key, val));

                if props_idx >= props_count {
                    let mut obj = Object::new();
                    for (key, value) in done {
                        obj.properties.insert(key, Property::value(value));
                    }
                    let obj_id = self.objects.alloc(obj);
                    self.control = Control::Value(JSValue::Object(ObjId(obj_id.index() as u32)));
                    self.cont = k;
                } else {
                    self.control = Control::Expr(props[props_idx as usize].value);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::ObjectK {
                        done,
                        props_start,
                        props_idx: props_idx + 1,
                        props_count,
                        env,
                        k,
                    });
                }
            }

            Kont::LetK { name, env, k } | Kont::ConstK { name, env, k } => {
                self.envs.define(env, name, val);
                self.control = Control::Value(JSValue::Undefined);
                self.cont = k;
            }

            Kont::VarK { name, env: _, k } => {
                self.globals.insert(name, val);
                self.control = Control::Value(JSValue::Undefined);
                self.cont = k;
            }

            Kont::SeqK {
                stmts_start,
                stmts_idx,
                stmts_count,
                env,
                k,
            } => {
                let stmts = ast.get_stmt_list(stmts_start, stmts_count);
                if stmts_idx >= stmts_count {
                    self.control = Control::Value(val);
                    self.cont = k;
                } else {
                    self.control = Control::Stmt(stmts[stmts_idx as usize]);
                    self.env = env;
                    if stmts_idx + 1 < stmts_count {
                        self.cont = self.conts.alloc(Kont::SeqK {
                            stmts_start,
                            stmts_idx: stmts_idx + 1,
                            stmts_count,
                            env,
                            k,
                        });
                    } else {
                        self.cont = k;
                    }
                }
            }

            Kont::ExprStmtK { k } => {
                self.control = Control::Value(val);
                self.cont = k;
            }

            Kont::BlockK {
                stmts_start,
                stmts_idx,
                stmts_count,
                final_expr,
                env,
                k,
            } => {
                if stmts_idx < stmts_count {
                    let stmts = ast.get_stmt_list(stmts_start, stmts_count);
                    self.control = Control::Stmt(stmts[stmts_idx as usize]);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::BlockK {
                        stmts_start,
                        stmts_idx: stmts_idx + 1,
                        stmts_count,
                        final_expr,
                        env,
                        k,
                    });
                } else {
                    self.env = env;
                    if final_expr.is_none() {
                        self.control = Control::Value(JSValue::Undefined);
                        self.cont = k;
                    } else {
                        self.control = Control::Expr(final_expr);
                        self.cont = k;
                    }
                }
            }

            Kont::MatchK {
                arms_start,
                arms_idx,
                arms_count,
                env,
                k,
            } => {
                self.try_match_arms(val, arms_start, arms_idx, arms_count, env, k, ast)?;
            }

            Kont::HandlerK {
                return_body,
                return_param,
                env,
                k,
                ..
            } => {
                // Apply the return clause if present
                if return_body.is_some() {
                    // Bind the return parameter and evaluate return_body
                    let return_env = self.envs.bind(env, return_param, val);
                    self.control = Control::Expr(return_body);
                    self.env = return_env;
                    self.cont = k;
                } else {
                    self.control = Control::Value(val);
                    self.cont = k;
                }
            }

            Kont::PerformArgsK {
                effect,
                mut done,
                args_start,
                args_idx,
                args_count,
                env,
                k,
            } => {
                done.push(val);
                if args_idx < args_count {
                    let args = ast.get_expr_list(args_start, args_count);
                    self.control = Control::Expr(args[args_idx as usize]);
                    self.env = env;
                    self.cont = self.conts.alloc(Kont::PerformArgsK {
                        effect,
                        done,
                        args_start,
                        args_idx: args_idx + 1,
                        args_count,
                        env,
                        k,
                    });
                } else {
                    // Pop PerformArgsK before calling do_perform so that the
                    // captured continuation doesn't include PerformArgsK itself
                    self.cont = k;
                    self.do_perform(effect, done, ast)?;
                }
            }
        }

        Ok(())
    }

    fn handle_return(&mut self, val: JSValue, _ast: &AstArena) -> Result<()> {
        let mut k = self.cont;
        loop {
            let Some(kont) = self.conts.get(k) else {
                break;
            };
            match kont {
                Kont::Halt => {
                    self.control = Control::Halted(val);
                    return Ok(());
                }
                Kont::ReturnK { env, k: outer } => {
                    self.env = *env;
                    self.cont = *outer;
                    self.control = Control::Value(val);
                    return Ok(());
                }
                _ => {
                    if let Some(outer) = kont.outer() {
                        k = outer;
                    } else {
                        break;
                    }
                }
            }
        }

        self.control = Control::Halted(val);
        Ok(())
    }

    fn handle_break(&mut self) -> Result<()> {
        let mut k = self.cont;
        loop {
            if let Some(kont) = self.conts.get(k).cloned() {
                match kont {
                    Kont::Halt => {
                        return Err(JSError::syntax_error("break outside loop"));
                    }
                    Kont::WhileK { k: outer, .. }
                    | Kont::WhileBodyK { k: outer, .. }
                    | Kont::ForTestK { k: outer, .. }
                    | Kont::ForTestResultK { k: outer, .. }
                    | Kont::ForBodyK { k: outer, .. }
                    | Kont::ForUpdateK { k: outer, .. } => {
                        self.control = Control::Value(JSValue::Undefined);
                        self.cont = outer;
                        return Ok(());
                    }
                    _ => {
                        if let Some(outer) = kont.outer() {
                            k = outer;
                        } else {
                            break;
                        }
                    }
                }
            } else {
                break;
            }
        }

        Err(JSError::syntax_error("break outside loop"))
    }

    fn handle_continue(&mut self, _ast: &AstArena) -> Result<()> {
        let mut k = self.cont;
        loop {
            if let Some(kont) = self.conts.get(k).cloned() {
                match kont {
                    Kont::Halt => {
                        return Err(JSError::syntax_error("continue outside loop"));
                    }
                    Kont::WhileK {
                        test,
                        body,
                        env,
                        k: outer,
                    }
                    | Kont::WhileBodyK {
                        test,
                        body,
                        env,
                        k: outer,
                    } => {
                        self.control = Control::Expr(test);
                        self.env = env;
                        self.cont = self.conts.alloc(Kont::WhileK {
                            test,
                            body,
                            env,
                            k: outer,
                        });
                        return Ok(());
                    }
                    Kont::ForTestK {
                        test,
                        update,
                        body,
                        env,
                        k: outer,
                    }
                    | Kont::ForTestResultK {
                        test,
                        update,
                        body,
                        env,
                        k: outer,
                    }
                    | Kont::ForBodyK {
                        test,
                        update,
                        body,
                        env,
                        k: outer,
                    }
                    | Kont::ForUpdateK {
                        test,
                        update,
                        body,
                        env,
                        k: outer,
                    } => {
                        if update.is_some() {
                            self.control = Control::Expr(update);
                            self.env = env;
                            self.cont = self.conts.alloc(Kont::ForUpdateK {
                                test,
                                update,
                                body,
                                env,
                                k: outer,
                            });
                        } else if test.is_some() {
                            self.control = Control::Expr(test);
                            self.env = env;
                            self.cont = self.conts.alloc(Kont::ForTestK {
                                test,
                                update,
                                body,
                                env,
                                k: outer,
                            });
                        } else {
                            self.control = Control::Value(JSValue::Bool(true));
                            self.env = env;
                            self.cont = self.conts.alloc(Kont::ForTestK {
                                test,
                                update,
                                body,
                                env,
                                k: outer,
                            });
                        }
                        return Ok(());
                    }
                    _ => {
                        if let Some(outer) = kont.outer() {
                            k = outer;
                        } else {
                            break;
                        }
                    }
                }
            } else {
                break;
            }
        }

        Err(JSError::syntax_error("continue outside loop"))
    }

    fn call_function(
        &mut self,
        callee: JSValue,
        args: Vec<JSValue>,
        k: ContId,
        ast: &AstArena,
    ) -> Result<()> {
        let obj_id = match callee {
            JSValue::Function(id) => id,
            JSValue::Continuation(cont_id, env_id) => {
                let value = args.first().copied().unwrap_or(JSValue::Undefined);

                // Rebase the captured continuation so that when it reaches
                // the HandlerK, it returns to our current continuation (k)
                // instead of continuing past the handler.
                let rebased_cont = self.rebase_continuation(cont_id, k);
                self.cont = rebased_cont;
                self.env = env_id;
                self.control = Control::Value(value);
                return Ok(());
            }
            _ => return Err(JSError::type_error("not a function")),
        };

        let obj = self
            .objects
            .get(obj_id.into_arena_id())
            .ok_or(JSError::InternalError("Invalid function object"))?;

        match &obj.kind {
            ObjectKind::NativeFunction(native_fn) => {
                let arg0 = args.first().copied().unwrap_or(JSValue::Undefined);
                let arg1 = args.get(1).copied().unwrap_or(JSValue::Undefined);
                let result = call_native(native_fn, arg0, arg1, &self.strings);
                self.control = Control::Value(result);
                self.cont = k;
            }

            ObjectKind::Function(func_data) => {
                let func_data = *func_data;

                let params = ast.get_param_list(func_data.params_start, func_data.params_count);

                let mut bindings_buf: [(StrId, JSValue); 8] = Default::default();
                let binding_count = params.len().min(8);
                for (i, param) in params.iter().enumerate().take(8) {
                    let arg = args.get(i).copied().unwrap_or(JSValue::Undefined);
                    bindings_buf[i] = (*param, arg);
                }

                let call_env = self
                    .envs
                    .extend_with_slice(func_data.env, &bindings_buf[..binding_count]);

                self.cont = self.conts.alloc(Kont::ReturnK { env: self.env, k });

                self.env = call_env;

                if let Some(expr_body) = func_data.expr_body {
                    self.control = Control::Expr(expr_body);
                    let inner_k = self.cont;
                    self.cont = self.conts.alloc(Kont::ReturnExprK { k: inner_k });
                } else if func_data.body.is_some() {
                    self.control = Control::Stmt(func_data.body);
                } else {
                    self.control = Control::Value(JSValue::Undefined);
                }
            }

            _ => return Err(JSError::type_error("not a function")),
        }

        Ok(())
    }

    fn apply_unary(&mut self, op: UnaryOp, val: JSValue) -> Result<JSValue> {
        match op {
            UnaryOp::Neg => {
                let n = self.to_number(val);
                Ok(JSValue::Float(-n))
            }
            UnaryOp::Plus => {
                let n = self.to_number(val);
                Ok(JSValue::Float(n))
            }
            UnaryOp::Not => Ok(JSValue::Bool(!self.is_truthy(val))),
            UnaryOp::BitNot => {
                let n = self.to_i32(val);
                Ok(JSValue::Int(!n))
            }
            UnaryOp::TypeOf => {
                let type_str = match val {
                    JSValue::Undefined => "undefined",
                    JSValue::Null => "object", // yes, really
                    JSValue::Bool(_) => "boolean",
                    JSValue::Int(_) | JSValue::Float(_) => "number",
                    JSValue::String(_) => "string",
                    JSValue::Object(_) | JSValue::Array(_) => "object",
                    JSValue::Function(_) | JSValue::Continuation(_, _) => "function",
                    JSValue::Handler(_) => "handler",
                };
                let str_id = self.strings.intern(type_str);
                Ok(JSValue::String(str_id))
            }
            UnaryOp::PreInc | UnaryOp::PostInc => {
                let n = self.to_number(val);
                Ok(self.number_value(n + 1.0))
            }
            UnaryOp::PreDec | UnaryOp::PostDec => {
                let n = self.to_number(val);
                Ok(self.number_value(n - 1.0))
            }
        }
    }

    fn apply_binary(&mut self, op: BinaryOp, left: JSValue, right: JSValue) -> Result<JSValue> {
        match op {
            BinaryOp::Add => {
                if matches!(left, JSValue::String(_)) || matches!(right, JSValue::String(_)) {
                    let left_str = self.to_string(left);
                    let right_str = self.to_string(right);
                    let result = format!("{}{}", left_str, right_str);
                    let str_id = self.strings.intern(&result);
                    Ok(JSValue::String(str_id))
                } else {
                    let l = self.to_number(left);
                    let r = self.to_number(right);
                    Ok(self.number_value(l + r))
                }
            }
            BinaryOp::Sub => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(self.number_value(l - r))
            }
            BinaryOp::Mul => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(self.number_value(l * r))
            }
            BinaryOp::Div => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Float(l / r))
            }
            BinaryOp::Mod => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Float(l % r))
            }
            BinaryOp::Pow => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Float(l.powf(r)))
            }

            BinaryOp::Eq => Ok(JSValue::Bool(self.strict_equals(left, right))),
            BinaryOp::Ne => Ok(JSValue::Bool(!self.strict_equals(left, right))),
            BinaryOp::Lt => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Bool(l < r))
            }
            BinaryOp::Le => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Bool(l <= r))
            }
            BinaryOp::Gt => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Bool(l > r))
            }
            BinaryOp::Ge => {
                let l = self.to_number(left);
                let r = self.to_number(right);
                Ok(JSValue::Bool(l >= r))
            }

            BinaryOp::BitAnd => {
                let l = self.to_i32(left);
                let r = self.to_i32(right);
                Ok(JSValue::Int(l & r))
            }
            BinaryOp::BitOr => {
                let l = self.to_i32(left);
                let r = self.to_i32(right);
                Ok(JSValue::Int(l | r))
            }
            BinaryOp::BitXor => {
                let l = self.to_i32(left);
                let r = self.to_i32(right);
                Ok(JSValue::Int(l ^ r))
            }
            BinaryOp::Shl => {
                let l = self.to_i32(left);
                let r = self.to_i32(right) as u32 & 0x1f;
                Ok(JSValue::Int(l << r))
            }
            BinaryOp::Shr => {
                let l = self.to_i32(left);
                let r = self.to_i32(right) as u32 & 0x1f;
                Ok(JSValue::Int(l >> r))
            }
            BinaryOp::UShr => {
                let l = self.to_i32(left) as u32;
                let r = self.to_i32(right) as u32 & 0x1f;
                Ok(JSValue::Int((l >> r) as i32))
            }

            BinaryOp::And | BinaryOp::Or | BinaryOp::NullishCoalesce => {
                unreachable!("Short-circuit operators handled in step_expr")
            }
        }
    }

    fn get_property(&self, obj: JSValue, property: StrId) -> Result<JSValue> {
        let obj_id = match obj {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => id,
            _ => return Ok(JSValue::Undefined),
        };

        let obj = self
            .objects
            .get(obj_id.into_arena_id())
            .ok_or(JSError::InternalError("Invalid object"))?;

        if let ObjectKind::Array(ref data) = obj.kind {
            if self
                .strings
                .get(property)
                .map(|s| s == "length")
                .unwrap_or(false)
            {
                return Ok(JSValue::Int(data.elements.len() as i32));
            }
        }

        Ok(obj.get(property).unwrap_or(JSValue::Undefined))
    }

    fn set_property(&mut self, obj: JSValue, property: StrId, value: JSValue) -> Result<()> {
        let obj_id = match obj {
            JSValue::Object(id) | JSValue::Array(id) | JSValue::Function(id) => id,
            _ => return Err(JSError::type_error("Cannot set property of non-object")),
        };

        let obj = self
            .objects
            .get_mut(obj_id.into_arena_id())
            .ok_or(JSError::InternalError("Invalid object"))?;

        obj.set(property, value);
        Ok(())
    }

    fn get_index(&self, obj: JSValue, key: JSValue) -> Result<JSValue> {
        let obj_id = match obj {
            JSValue::Array(id) => id,
            JSValue::Object(id) | JSValue::Function(id) => {
                if let JSValue::String(str_id) = key {
                    return self.get_property(obj, str_id);
                }
                id
            }
            _ => return Ok(JSValue::Undefined),
        };

        let obj = self
            .objects
            .get(obj_id.into_arena_id())
            .ok_or(JSError::InternalError("Invalid object"))?;

        if let ObjectKind::Array(ref data) = obj.kind {
            let idx = self.to_i32(key);
            if idx >= 0 {
                return Ok(data
                    .elements
                    .get(idx as usize)
                    .copied()
                    .unwrap_or(JSValue::Undefined));
            }
        }

        Ok(JSValue::Undefined)
    }

    fn set_index(&mut self, obj: JSValue, key: JSValue, value: JSValue) -> Result<()> {
        let obj_id = match obj {
            JSValue::Array(id) => id,
            JSValue::Object(id) | JSValue::Function(id) => {
                if let JSValue::String(str_id) = key {
                    return self.set_property(obj, str_id, value);
                }
                id
            }
            _ => return Err(JSError::type_error("Cannot set index of non-object")),
        };

        let idx = self.to_i32(key);

        let obj = self
            .objects
            .get_mut(obj_id.into_arena_id())
            .ok_or(JSError::InternalError("Invalid object"))?;

        if let ObjectKind::Array(ref mut data) = obj.kind {
            if idx >= 0 {
                let idx = idx as usize;
                if idx >= data.elements.len() {
                    data.elements.resize(idx + 1, JSValue::Undefined);
                }
                data.elements[idx] = value;
            }
        }

        Ok(())
    }

    fn is_truthy(&self, val: JSValue) -> bool {
        match val {
            JSValue::Undefined | JSValue::Null => false,
            JSValue::Bool(b) => b,
            JSValue::Int(n) => n != 0,
            JSValue::Float(f) => f != 0.0 && !f.is_nan(),
            JSValue::String(str_id) => self
                .strings
                .get(str_id)
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            JSValue::Object(_)
            | JSValue::Array(_)
            | JSValue::Function(_)
            | JSValue::Continuation(_, _)
            | JSValue::Handler(_) => true,
        }
    }

    fn to_number(&self, val: JSValue) -> f64 {
        match val {
            JSValue::Undefined => f64::NAN,
            JSValue::Null => 0.0,
            JSValue::Bool(true) => 1.0,
            JSValue::Bool(false) => 0.0,
            JSValue::Int(n) => n as f64,
            JSValue::Float(f) => f,
            JSValue::String(str_id) => self
                .strings
                .get(str_id)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }

    fn to_i32(&self, val: JSValue) -> i32 {
        match val {
            JSValue::Int(n) => n,
            JSValue::Float(f) => f as i32,
            _ => self.to_number(val) as i32,
        }
    }

    pub fn to_string(&self, val: JSValue) -> String {
        match val {
            JSValue::Undefined => "undefined".to_string(),
            JSValue::Null => "null".to_string(),
            JSValue::Bool(true) => "true".to_string(),
            JSValue::Bool(false) => "false".to_string(),
            JSValue::Int(n) => n.to_string(),
            JSValue::Float(f) => {
                if f.is_nan() {
                    "NaN".to_string()
                } else if f.is_infinite() {
                    if f > 0.0 {
                        "Infinity".to_string()
                    } else {
                        "-Infinity".to_string()
                    }
                } else {
                    f.to_string()
                }
            }
            JSValue::String(str_id) => self.strings.get(str_id).unwrap_or("").to_string(),
            JSValue::Object(_) => "[object Object]".to_string(),
            JSValue::Array(_) => "[object Array]".to_string(),
            JSValue::Function(_) => "[object Function]".to_string(),
            JSValue::Continuation(_, _) => "[cont Continuation]".to_string(),
            JSValue::Handler(_) => "[handler Handler]".to_string(),
        }
    }

    fn strict_equals(&self, a: JSValue, b: JSValue) -> bool {
        match (a, b) {
            (JSValue::Undefined, JSValue::Undefined) => true,
            (JSValue::Null, JSValue::Null) => true,
            (JSValue::Bool(a), JSValue::Bool(b)) => a == b,
            (JSValue::Int(a), JSValue::Int(b)) => a == b,
            (JSValue::Float(a), JSValue::Float(b)) => a == b,
            (JSValue::Int(a), JSValue::Float(b)) => (a as f64) == b,
            (JSValue::Float(a), JSValue::Int(b)) => a == (b as f64),
            (JSValue::String(a), JSValue::String(b)) => a == b,
            (JSValue::Object(a), JSValue::Object(b)) => a == b,
            (JSValue::Array(a), JSValue::Array(b)) => a == b,
            (JSValue::Function(a), JSValue::Function(b)) => a == b,
            (JSValue::Continuation(a, _), JSValue::Continuation(b, _)) => a == b,
            (JSValue::Handler(a), JSValue::Handler(b)) => a == b,
            _ => false,
        }
    }

    fn number_value(&self, n: f64) -> JSValue {
        if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
            JSValue::Int(n as i32)
        } else {
            JSValue::Float(n)
        }
    }

    fn try_match_arms(
        &mut self,
        val: JSValue,
        arms_start: u32,
        arms_idx: u16,
        arms_count: u16,
        env: EnvId,
        k: ContId,
        ast: &AstArena,
    ) -> Result<()> {
        let arms = ast.get_arm_list(arms_start, arms_count);

        for idx in (arms_idx as usize)..arms.len() {
            let arm = &arms[idx];

            if let Some(bindings) = self.match_pattern(arm.pattern, val, ast)? {
                let match_env = self.envs.extend_with(env, bindings);

                if arm.guard.is_some() {
                    // TODO: Handle guards
                    self.env = match_env;
                    self.control = Control::Expr(arm.body);
                    self.cont = k;
                    return Ok(());
                }

                self.env = match_env;
                self.control = Control::Expr(arm.body);
                self.cont = k;
                return Ok(());
            }
        }

        Err(JSError::runtime_error(
            "No pattern matched in match expression",
        ))
    }

    fn match_pattern(
        &self,
        pattern_id: PatternId,
        val: JSValue,
        ast: &AstArena,
    ) -> Result<Option<HashMap<StrId, JSValue>>> {
        let pattern = ast
            .get_pattern(pattern_id)
            .ok_or(JSError::InternalError("Invalid pattern ID"))?;

        match pattern {
            Pattern::Wildcard => Ok(Some(HashMap::new())),

            Pattern::Var(name) => {
                let mut bindings = HashMap::new();
                bindings.insert(name, val);
                Ok(Some(bindings))
            }

            Pattern::Literal(expr_id) => {
                let lit_val = self.eval_literal(expr_id, ast)?;
                if self.strict_equals(val, lit_val) {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(None)
                }
            }

            Pattern::Array {
                elems_start,
                elems_count,
            } => {
                let arr_id = match val {
                    JSValue::Array(id) => id,
                    _ => return Ok(None),
                };

                let arr_obj = self
                    .objects
                    .get(arr_id.into_arena_id())
                    .ok_or(JSError::InternalError("Invalid array"))?;

                let elements = match &arr_obj.kind {
                    ObjectKind::Array(data) => &data.elements,
                    _ => return Ok(None),
                };

                let patterns = ast.get_pattern_list(elems_start, elems_count);

                if patterns.len() != elements.len() {
                    return Ok(None);
                }

                let mut all_bindings = HashMap::new();
                for (pat_id, elem) in patterns.iter().zip(elements.iter()) {
                    match self.match_pattern(*pat_id, *elem, ast)? {
                        Some(bindings) => all_bindings.extend(bindings),
                        None => return Ok(None),
                    }
                }

                Ok(Some(all_bindings))
            }

            Pattern::Object {
                fields_start,
                fields_count,
            } => {
                let obj_id = match val {
                    JSValue::Object(id) => id,
                    _ => return Ok(None),
                };

                let obj = self
                    .objects
                    .get(obj_id.into_arena_id())
                    .ok_or(JSError::InternalError("Invalid object"))?;

                let fields = ast.get_pattern_field_list(fields_start, fields_count);

                let mut all_bindings = HashMap::new();
                for field in fields {
                    // Structural matching: every key in the pattern must exist
                    // on the value, so {ok} and {err} arms discriminate.
                    let Some(prop_val) = obj.get(field.key) else {
                        return Ok(None);
                    };
                    match self.match_pattern(field.pattern, prop_val, ast)? {
                        Some(bindings) => all_bindings.extend(bindings),
                        None => return Ok(None),
                    }
                }

                Ok(Some(all_bindings))
            }
        }
    }

    fn eval_literal(&self, expr_id: ExprId, ast: &AstArena) -> Result<JSValue> {
        let expr = ast
            .get_expr(expr_id)
            .ok_or(JSError::InternalError("Invalid expression ID"))?;

        match expr {
            Expr::Undefined => Ok(JSValue::Undefined),
            Expr::Null => Ok(JSValue::Null),
            Expr::Bool(b) => Ok(JSValue::Bool(b)),
            Expr::Int(n) => Ok(JSValue::Int(n)),
            Expr::Float(idx) => {
                let f = ast.get_float(idx).unwrap_or(0.0);
                Ok(JSValue::Float(f))
            }
            Expr::String(str_id) => Ok(JSValue::String(str_id)),
            _ => Err(JSError::InternalError("Expected literal in pattern")),
        }
    }

    fn do_perform(&mut self, effect: StrId, args: Vec<JSValue>, ast: &AstArena) -> Result<()> {
        let mut k = self.cont;
        loop {
            if let Some(kont) = self.conts.get(k).cloned() {
                match kont {
                    Kont::Halt => {
                        self.control = Control::Suspend { effect, args };
                        return Ok(());
                    }
                    Kont::HandlerK {
                        clauses_start,
                        clauses_count,
                        env,
                        return_body: _,
                        return_param: _,
                        k: kk,
                    } => {
                        let clauses = ast.get_effect_clause_list(clauses_start, clauses_count);
                        let Some(clause) = clauses.iter().find(|c| c.effect == effect) else {
                            k = kk;
                            continue;
                        };
                        let params = ast.get_param_list(clause.params_start, clause.params_count);
                        let resume = JSValue::Continuation(self.cont, self.env);
                        let mut bindings = HashMap::new();
                        for (i, param) in params.iter().enumerate() {
                            if i < args.len() {
                                bindings.insert(*param, args[i].clone());
                            }
                        }
                        if let Some(last) = params.last() {
                            bindings.insert(*last, resume);
                        }

                        let clause_env = self.envs.extend_with(env, bindings);
                        self.env = clause_env;
                        self.control = Control::Expr(clause.body);
                        self.cont = kk;
                        return Ok(());
                    }
                    other => {
                        k = other.outer().unwrap();
                    }
                }
            }
        }
    }

    /// Rebase a captured continuation so that when it reaches the HandlerK,
    /// it returns to `new_return_k` after applying the return clause.
    /// This implements proper delimited continuation semantics for resume:
    /// - The resumed value flows through the continuation up to the handler
    /// - The handler's return clause transforms the value
    /// - Then the transformed value returns to the resume call site
    fn rebase_continuation(&mut self, cont: ContId, new_return_k: ContId) -> ContId {
        // Walk the continuation chain and rebuild it with HandlerK's k replaced
        let mut stack = Vec::new();
        let mut current = cont;

        // Collect continuations until we find HandlerK
        loop {
            let Some(kont) = self.conts.get(current).cloned() else {
                break;
            };

            match &kont {
                Kont::Halt => {
                    // No handler found, just return original
                    // (shouldn't happen in well-formed code)
                    return cont;
                }
                Kont::HandlerK {
                    clauses_start,
                    clauses_count,
                    env,
                    return_body,
                    return_param,
                    k: _,
                } => {
                    // Found the handler - rebuild with k pointing to resume call site
                    let new_handler = Kont::HandlerK {
                        clauses_start: *clauses_start,
                        clauses_count: *clauses_count,
                        env: *env,
                        return_body: *return_body,
                        return_param: *return_param,
                        k: new_return_k,
                    };
                    let mut result = self.conts.alloc(new_handler);

                    // Rebuild the stack in reverse order
                    while let Some(k) = stack.pop() {
                        result = self.rebuild_kont_with_k(k, result);
                    }

                    return result;
                }
                _ => {
                    stack.push(kont.clone());
                    current = kont.outer().unwrap_or(self.conts.halt());
                }
            }
        }

        // No handler found, return original
        cont
    }

    /// Rebuild a continuation with a new outer continuation k
    fn rebuild_kont_with_k(&mut self, kont: Kont, new_k: ContId) -> ContId {
        let new_kont = match kont {
            Kont::Halt => Kont::Halt,
            Kont::UnaryK { op, .. } => Kont::UnaryK { op, k: new_k },
            Kont::BinaryLeftK { op, right, env, .. } => Kont::BinaryLeftK {
                op,
                right,
                env,
                k: new_k,
            },
            Kont::BinaryRightK { op, left, .. } => Kont::BinaryRightK { op, left, k: new_k },
            Kont::AndK { right, env, .. } => Kont::AndK {
                right,
                env,
                k: new_k,
            },
            Kont::OrK { right, env, .. } => Kont::OrK {
                right,
                env,
                k: new_k,
            },
            Kont::NullishK { right, env, .. } => Kont::NullishK {
                right,
                env,
                k: new_k,
            },
            Kont::MemberK { property, .. } => Kont::MemberK { property, k: new_k },
            Kont::IndexObjK { index, env, .. } => Kont::IndexObjK {
                index,
                env,
                k: new_k,
            },
            Kont::IndexKeyK { obj, .. } => Kont::IndexKeyK { obj, k: new_k },
            Kont::AssignVarK { name, env, .. } => Kont::AssignVarK {
                name,
                env,
                k: new_k,
            },
            Kont::UpdateVarK {
                name,
                is_pre,
                is_inc,
                env,
                ..
            } => Kont::UpdateVarK {
                name,
                is_pre,
                is_inc,
                env,
                k: new_k,
            },
            Kont::AssignMemberObjK {
                property,
                value,
                env,
                ..
            } => Kont::AssignMemberObjK {
                property,
                value,
                env,
                k: new_k,
            },
            Kont::AssignMemberValK { obj, property, .. } => Kont::AssignMemberValK {
                obj,
                property,
                k: new_k,
            },
            Kont::AssignIndexObjK {
                index, value, env, ..
            } => Kont::AssignIndexObjK {
                index,
                value,
                env,
                k: new_k,
            },
            Kont::AssignIndexKeyK {
                obj, value, env, ..
            } => Kont::AssignIndexKeyK {
                obj,
                value,
                env,
                k: new_k,
            },
            Kont::AssignIndexValK { obj, key, .. } => Kont::AssignIndexValK { obj, key, k: new_k },
            Kont::IfK {
                consequent,
                alternate,
                env,
                ..
            } => Kont::IfK {
                consequent,
                alternate,
                env,
                k: new_k,
            },
            Kont::CondK {
                consequent,
                alternate,
                env,
                ..
            } => Kont::CondK {
                consequent,
                alternate,
                env,
                k: new_k,
            },
            Kont::WhileK {
                test, body, env, ..
            } => Kont::WhileK {
                test,
                body,
                env,
                k: new_k,
            },
            Kont::WhileBodyK {
                test, body, env, ..
            } => Kont::WhileBodyK {
                test,
                body,
                env,
                k: new_k,
            },
            Kont::ForTestK {
                test,
                update,
                body,
                env,
                ..
            } => Kont::ForTestK {
                test,
                update,
                body,
                env,
                k: new_k,
            },
            Kont::ForTestResultK {
                test,
                update,
                body,
                env,
                ..
            } => Kont::ForTestResultK {
                test,
                update,
                body,
                env,
                k: new_k,
            },
            Kont::ForBodyK {
                test,
                update,
                body,
                env,
                ..
            } => Kont::ForBodyK {
                test,
                update,
                body,
                env,
                k: new_k,
            },
            Kont::ForUpdateK {
                test,
                update,
                body,
                env,
                ..
            } => Kont::ForUpdateK {
                test,
                update,
                body,
                env,
                k: new_k,
            },
            Kont::CalleeK {
                args_start,
                args_count,
                env,
                ..
            } => Kont::CalleeK {
                args_start,
                args_count,
                env,
                k: new_k,
            },
            Kont::ArgsK {
                callee,
                done,
                args_start,
                args_idx,
                args_count,
                env,
                ..
            } => Kont::ArgsK {
                callee,
                done,
                args_start,
                args_idx,
                args_count,
                env,
                k: new_k,
            },
            Kont::ReturnK { env, .. } => Kont::ReturnK { env, k: new_k },
            Kont::ReturnExprK { .. } => Kont::ReturnExprK { k: new_k },
            Kont::ArrayK {
                done,
                elems_start,
                elems_idx,
                elems_count,
                env,
                ..
            } => Kont::ArrayK {
                done,
                elems_start,
                elems_idx,
                elems_count,
                env,
                k: new_k,
            },
            Kont::ObjectK {
                done,
                props_start,
                props_idx,
                props_count,
                env,
                ..
            } => Kont::ObjectK {
                done,
                props_start,
                props_idx,
                props_count,
                env,
                k: new_k,
            },
            Kont::LetK { name, env, .. } => Kont::LetK {
                name,
                env,
                k: new_k,
            },
            Kont::ConstK { name, env, .. } => Kont::ConstK {
                name,
                env,
                k: new_k,
            },
            Kont::VarK { name, env, .. } => Kont::VarK {
                name,
                env,
                k: new_k,
            },
            Kont::SeqK {
                stmts_start,
                stmts_idx,
                stmts_count,
                env,
                ..
            } => Kont::SeqK {
                stmts_start,
                stmts_idx,
                stmts_count,
                env,
                k: new_k,
            },
            Kont::ExprStmtK { .. } => Kont::ExprStmtK { k: new_k },
            Kont::BlockK {
                stmts_start,
                stmts_idx,
                stmts_count,
                final_expr,
                env,
                ..
            } => Kont::BlockK {
                stmts_start,
                stmts_idx,
                stmts_count,
                final_expr,
                env,
                k: new_k,
            },
            Kont::MatchK {
                arms_start,
                arms_idx,
                arms_count,
                env,
                ..
            } => Kont::MatchK {
                arms_start,
                arms_idx,
                arms_count,
                env,
                k: new_k,
            },
            Kont::HandlerK {
                clauses_start,
                clauses_count,
                env,
                return_body,
                return_param,
                ..
            } => Kont::HandlerK {
                clauses_start,
                clauses_count,
                env,
                return_body,
                return_param,
                k: new_k,
            },
            Kont::PerformArgsK {
                effect,
                done,
                args_start,
                args_idx,
                args_count,
                env,
                ..
            } => Kont::PerformArgsK {
                effect,
                done,
                args_start,
                args_idx,
                args_count,
                env,
                k: new_k,
            },
            Kont::HandleWithK { body, env, .. } => Kont::HandleWithK {
                body,
                env,
                k: new_k,
            },
        };
        self.conts.alloc(new_kont)
    }

    fn setup_builtins(&mut self) {
        let mut math_obj = Object::new();

        let add_fn = |obj: &mut Object,
                      objects: &mut Arena<Object>,
                      strings: &mut StringPool,
                      name: &str,
                      native_fn: NativeFn| {
            let name_id = strings.intern(name);
            let fn_obj = Object::native_function(native_fn);
            let fn_id = objects.alloc(fn_obj);
            obj.properties.insert(
                name_id,
                Property::readonly(JSValue::Function(ObjId(fn_id.index() as u32))),
            );
        };

        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "floor",
            NativeFn::MathFloor,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "ceil",
            NativeFn::MathCeil,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "round",
            NativeFn::MathRound,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "abs",
            NativeFn::MathAbs,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "sqrt",
            NativeFn::MathSqrt,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "pow",
            NativeFn::MathPow,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "min",
            NativeFn::MathMin,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "max",
            NativeFn::MathMax,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "sin",
            NativeFn::MathSin,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "cos",
            NativeFn::MathCos,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "log",
            NativeFn::MathLog,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "exp",
            NativeFn::MathExp,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "trunc",
            NativeFn::MathTrunc,
        );
        add_fn(
            &mut math_obj,
            &mut self.objects,
            &mut self.strings,
            "sign",
            NativeFn::MathSign,
        );

        let pi_id = self.strings.intern("PI");
        math_obj.properties.insert(
            pi_id,
            Property::readonly(JSValue::Float(std::f64::consts::PI)),
        );

        let e_id = self.strings.intern("E");
        math_obj.properties.insert(
            e_id,
            Property::readonly(JSValue::Float(std::f64::consts::E)),
        );

        let math_id = self.objects.alloc(math_obj);
        let math_name = self.strings.intern("Math");
        self.globals
            .insert(math_name, JSValue::Object(ObjId(math_id.index() as u32)));
    }
}

impl Default for CEKH {
    fn default() -> Self {
        Self::new()
    }
}
