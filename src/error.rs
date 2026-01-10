//! Error types

use std::fmt;

pub type Result<T> = std::result::Result<T, JSError>;

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
    UncaughtException(String),
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

    /// SyntaxError without location info
    pub fn syntax_error(message: &'static str) -> Self {
        JSError::SyntaxError(SyntaxError {
            message,
            line: 0,
            column: 0,
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

    pub fn runtime_error(message: &'static str) -> Self {
        JSError::InternalError(message)
    }

    /// Shorthand: JSError::SyntaxError("message")
    #[allow(non_snake_case)]
    pub fn SyntaxError(message: &'static str) -> Self {
        Self::syntax_error(message)
    }

    /// Shorthand: JSError::TypeError("message")
    #[allow(non_snake_case)]
    pub fn TypeError(message: &'static str) -> Self {
        Self::type_error(message)
    }
}

impl fmt::Display for JSError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JSError::SyntaxError(e) => {
                if e.line > 0 {
                    write!(f, "SyntaxError: {} at {}:{}", e.message, e.line, e.column)
                } else {
                    write!(f, "SyntaxError: {}", e.message)
                }
            }
            JSError::TypeError(e) => write!(f, "TypeError: {}", e.message),
            JSError::ReferenceError(e) => write!(f, "ReferenceError: {} is not defined", e.name),
            JSError::RangeError(e) => write!(f, "RangeError: {}", e.message),
            JSError::InternalError(msg) => write!(f, "InternalError: {}", msg),
            JSError::OutOfMemory => write!(f, "OutOfMemory"),
            JSError::StackOverflow => write!(f, "StackOverflow"),
            JSError::Thrown(e) => write!(f, "Thrown: {}", e.message),
            JSError::UncaughtException(msg) => write!(f, "UncaughtException: {}", msg),
        }
    }
}

// Convenience constructors for CEKH
impl From<&'static str> for JSError {
    fn from(msg: &'static str) -> Self {
        JSError::InternalError(msg)
    }
}
