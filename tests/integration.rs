//! Integration tests for KryhtaJS

use kryhta::parser::Parser;
use kryhta::{CEKH, JSValue, Result};

fn run_js(source: &str) -> Result<JSValue> {
    let mut machine = CEKH::new();
    let strings = std::mem::take(&mut machine.strings);
    let parser = Parser::with_strings(source, strings)?;
    let (arena, strings) = parser.parse_program()?;
    machine.strings = strings;
    machine.run(&arena)
}

fn run_test_file(source: &str, name: &str) {
    match run_js(source) {
        Ok(JSValue::Bool(true)) => {}
        Ok(JSValue::Bool(false)) => {
            panic!("{}: Test returned false", name);
        }
        Ok(other) => {
            panic!("{}: Expected Bool(true), got {:?}", name, other);
        }
        Err(e) => {
            panic!("{}: {}", name, e);
        }
    }
}

#[test]
fn test_primitives() {
    run_test_file(include_str!("fixtures/test_primitives.js"), "primitives");
}

#[test]
fn test_operators() {
    run_test_file(include_str!("fixtures/test_operators.js"), "operators");
}

#[test]
fn test_variables() {
    run_test_file(include_str!("fixtures/test_variables.js"), "variables");
}

#[test]
fn test_control_flow() {
    run_test_file(
        include_str!("fixtures/test_control_flow.js"),
        "control_flow",
    );
}

#[test]
fn test_objects() {
    run_test_file(include_str!("fixtures/test_objects.js"), "objects");
}

#[test]
fn test_math() {
    run_test_file(include_str!("fixtures/test_math.js"), "math");
}

#[test]
fn test_functions() {
    run_test_file(include_str!("fixtures/test_functions.js"), "functions");
}

#[test]
fn test_match() {
    run_test_file(include_str!("fixtures/test_match.js"), "match");
}

#[test]
fn test_do() {
    run_test_file(include_str!("fixtures/test_do.js"), "do");
}

#[test]
fn test_effects() {
    run_test_file(include_str!("fixtures/test_effects.js"), "effects");
}
