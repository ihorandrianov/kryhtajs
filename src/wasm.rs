//! WebAssembly bindings for KryhtaJS
//!
//! Provides a simple evaluate(code) -> result API for browser usage.

use wasm_bindgen::prelude::*;

use crate::parser::Parser;
use crate::runtime::Runtime;

#[wasm_bindgen]
pub fn evaluate(source: &str) -> String {
    let mut runtime = Runtime::new();
    let strings = std::mem::take(&mut runtime.interpreter.strings);

    let parser = match Parser::with_strings(source, strings) {
        Ok(p) => p,
        Err(e) => return format!("Parse error: {}", e),
    };

    let (arena, strings) = match parser.parse_program() {
        Ok(r) => r,
        Err(e) => return format!("Parse error: {}", e),
    };

    runtime.interpreter.strings = strings;

    match runtime.run(&arena) {
        Ok(val) => runtime.interpreter.to_string(val),
        Err(e) => format!("Error: {}", e),
    }
}
