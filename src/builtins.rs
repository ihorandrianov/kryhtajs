//! Built-in JavaScript functions

use crate::error::Result;
use crate::value::JSValue;
use crate::vm::VM;

pub type NativeFn = fn(&mut VM, &[JSValue]) -> Result<JSValue>;

pub fn math_floor(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::floor(n)))
}

pub fn math_ceil(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::ceil(n)))
}

pub fn math_round(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::round(n)))
}

pub fn math_abs(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::fabs(n)))
}

pub fn math_sqrt(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::sqrt(n)))
}

pub fn math_pow(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let base = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    let exp = to_number(args.get(1).copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::pow(base, exp)))
}

pub fn math_sin(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::sin(n)))
}

pub fn math_cos(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::cos(n)))
}

pub fn math_tan(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::tan(n)))
}

pub fn math_log(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::log(n)))
}

pub fn math_exp(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    let n = to_number(args.first().copied().unwrap_or(JSValue::Undefined));
    Ok(JSValue::Float(libm::exp(n)))
}

pub fn math_min(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    if args.is_empty() {
        return Ok(JSValue::Float(f64::INFINITY));
    }
    let mut min = to_number(args[0]);
    for arg in &args[1..] {
        let n = to_number(*arg);
        if n < min || n.is_nan() {
            min = n;
        }
    }
    Ok(JSValue::Float(min))
}

pub fn math_max(_vm: &mut VM, args: &[JSValue]) -> Result<JSValue> {
    if args.is_empty() {
        return Ok(JSValue::Float(f64::NEG_INFINITY));
    }
    let mut max = to_number(args[0]);
    for arg in &args[1..] {
        let n = to_number(*arg);
        if n > max || n.is_nan() {
            max = n;
        }
    }
    Ok(JSValue::Float(max))
}

fn to_number(value: JSValue) -> f64 {
    match value {
        JSValue::Undefined => f64::NAN,
        JSValue::Null => 0.0,
        JSValue::Bool(b) => if b { 1.0 } else { 0.0 },
        JSValue::Int(n) => n as f64,
        JSValue::Float(f) => f,
        JSValue::String(_) => f64::NAN,
        JSValue::Object(_) | JSValue::Array(_) | JSValue::Function(_) => f64::NAN,
    }
}
