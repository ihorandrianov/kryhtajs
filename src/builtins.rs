//! Built-in JavaScript functions for CEKH machine
//!
//! Native functions are implemented via the NativeFn enum.
//! The call_native function dispatches to the appropriate implementation.

use crate::object::NativeFn;
use crate::string_pool::StringPool;
use crate::value::JSValue;

/// Helper to convert JSValue to f64
fn to_number(val: JSValue, _strings: &StringPool) -> f64 {
    match val {
        JSValue::Undefined => f64::NAN,
        JSValue::Null => 0.0,
        JSValue::Bool(true) => 1.0,
        JSValue::Bool(false) => 0.0,
        JSValue::Int(n) => n as f64,
        JSValue::Float(f) => f,
        JSValue::String(_) => f64::NAN, // Simplified - full impl would parse
        _ => f64::NAN,
    }
}

/// Helper to return a numeric result (Int or Float)
fn num_result(n: f64) -> JSValue {
    if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        JSValue::Int(n as i32)
    } else {
        JSValue::Float(n)
    }
}

/// Call a native function with up to 2 arguments
pub fn call_native(
    native_fn: &NativeFn,
    arg0: JSValue,
    arg1: JSValue,
    strings: &StringPool,
) -> JSValue {
    let n0 = to_number(arg0, strings);
    let n1 = to_number(arg1, strings);

    match native_fn {
        NativeFn::MathFloor => num_result(n0.floor()),
        NativeFn::MathCeil => num_result(n0.ceil()),
        NativeFn::MathRound => num_result((n0 + 0.5).floor()),
        NativeFn::MathAbs => num_result(n0.abs()),
        NativeFn::MathSqrt => num_result(n0.sqrt()),
        NativeFn::MathPow => num_result(n0.powf(n1)),
        NativeFn::MathMin => {
            if n0.is_nan() || n1.is_nan() {
                JSValue::Float(f64::NAN)
            } else {
                num_result(n0.min(n1))
            }
        }
        NativeFn::MathMax => {
            if n0.is_nan() || n1.is_nan() {
                JSValue::Float(f64::NAN)
            } else {
                num_result(n0.max(n1))
            }
        }
        NativeFn::MathSin => num_result(n0.sin()),
        NativeFn::MathCos => num_result(n0.cos()),
        NativeFn::MathLog => num_result(n0.ln()),
        NativeFn::MathExp => num_result(n0.exp()),
        NativeFn::MathTrunc => num_result(n0.trunc()),
        NativeFn::MathSign => {
            if n0.is_nan() {
                JSValue::Float(f64::NAN)
            } else if n0 == 0.0 {
                JSValue::Int(0)
            } else if n0 > 0.0 {
                JSValue::Int(1)
            } else {
                JSValue::Int(-1)
            }
        }
    }
}
