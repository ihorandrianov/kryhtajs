//! Error types

use core::fmt;

pub type Result<T> = core::result::Result<T, JSError>;

#[derive(Debug, Clone)]
pub enum JSError {
    SyntaxError(SyntaxError),
    TypeError(TypeError),
    ReferenceError(ReferenceError),
    RangeError(RangeError),
    InternalError(&'static str),
    OutOfMemory,
    StackOverflow,
    Thrown(ThrownValue),
}

#[derive(Debug, Clone)]
pub struct SyntaxError {
    pub message: &'static str,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: &'static str,
}

#[derive(Debug, Clone)]
pub struct ReferenceError {
    pub name: &'static str,
}

#[derive(Debug, Clone)]
pub struct RangeError {
    pub message: &'static str,
}

#[derive(Debug, Clone)]
pub struct ThrownValue {
    pub message: &'static str,
}

impl JSError {
    pub fn syntax(message: &'static str, line: u32, column: u32) -> Self {
        JSError::SyntaxError(SyntaxError {
            message,
            line,
            column,
        })
    }

    pub fn type_error(message: &'static str) -> Self {
        JSError::TypeError(TypeError { message })
    }

    pub fn reference_error(name: &'static str) -> Self {
        JSError::ReferenceError(ReferenceError { name })
    }

    pub fn range_error(message: &'static str) -> Self {
        JSError::RangeError(RangeError { message })
    }
}

impl fmt::Display for JSError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JSError::SyntaxError(e) => {
                write!(f, "SyntaxError: {} at {}:{}", e.message, e.line, e.column)
            }
            JSError::TypeError(e) => write!(f, "TypeError: {}", e.message),
            JSError::ReferenceError(e) => write!(f, "ReferenceError: {} is not defined", e.name),
            JSError::RangeError(e) => write!(f, "RangeError: {}", e.message),
            JSError::InternalError(msg) => write!(f, "InternalError: {}", msg),
            JSError::OutOfMemory => write!(f, "OutOfMemory"),
            JSError::StackOverflow => write!(f, "StackOverflow"),
            JSError::Thrown(e) => write!(f, "Thrown: {}", e.message),
        }
    }
}
