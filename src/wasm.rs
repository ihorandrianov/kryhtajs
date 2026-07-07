//! WebAssembly bindings for KryhtaJS
//!
//! Provides a simple evaluate(code) -> result API for browser usage.

use wasm_bindgen::prelude::*;

use crate::runtime::Runtime;

#[wasm_bindgen]
pub fn evaluate(source: &str) -> String {
    let mut runtime = Runtime::new();
    match runtime.eval(source) {
        Ok(val) => runtime.interpreter.to_string(val),
        Err(e) => format!("Error: {}", e),
    }
}
