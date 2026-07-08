//! Integration tests for KryhtaJS

use kryhta::{JSValue, Result, Runtime};

fn eval(runtime: &mut Runtime, source: &str) -> Result<JSValue> {
    runtime.eval(source)
}

fn run_js(source: &str) -> Result<JSValue> {
    let mut runtime = Runtime::new();
    eval(&mut runtime, source)
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
fn test_runtime_consecutive_runs() {
    let mut runtime = Runtime::new();
    assert_eq!(eval(&mut runtime, "1 + 1").unwrap(), JSValue::Int(2));
    assert_eq!(eval(&mut runtime, "2 + 2").unwrap(), JSValue::Int(4));
}

#[test]
fn test_runtime_globals_persist_across_runs() {
    let mut runtime = Runtime::new();
    eval(&mut runtime, "let x = 5").unwrap();
    assert_eq!(eval(&mut runtime, "x * 2").unwrap(), JSValue::Int(10));
}

#[test]
fn test_functions_persist_across_evals() {
    let mut runtime = Runtime::new();
    runtime.eval("function inc(x) { return x + 1 }").unwrap();
    assert_eq!(runtime.eval("inc(41)").unwrap(), JSValue::Int(42));
}

#[test]
fn test_eval_survives_syntax_error() {
    let mut runtime = Runtime::new();
    runtime.eval("let msg = 'hello'").unwrap();
    assert!(runtime.eval("let ] nonsense").is_err());
    assert_eq!(
        runtime.eval("msg === 'hello'").unwrap(),
        JSValue::Bool(true),
        "session state must survive a failed parse"
    );
}

#[test]
fn test_gc_effect_collects_and_resumes() {
    run_test_file("perform Gc!(); true", "gc_effect");
}

#[test]
fn test_gc_preserves_globals() {
    run_test_file(include_str!("fixtures/test_gc_globals.js"), "gc_globals");
}

#[test]
fn test_gc_preserves_fiber_results() {
    run_test_file(
        include_str!("fixtures/test_gc_fiber_results.js"),
        "gc_fiber_results",
    );
}

#[test]
fn test_gc_triggers_automatically() {
    let source = r#"
        let i = 0;
        while (i < 500) {
            let o = { a: i };
            let f = perform Fork!(function() { return 0 });
            perform Join!(f);
            i = i + 1;
        }
        true
    "#;
    let mut runtime = Runtime::new();
    assert_eq!(eval(&mut runtime, source).unwrap(), JSValue::Bool(true));
    assert!(
        runtime.gc_stats().collections > 0,
        "GC never triggered despite allocating past the threshold"
    );
}

#[test]
fn test_fork_body_gets_fresh_scope() {
    run_test_file(include_str!("fixtures/test_fork_scope.js"), "fork_scope");
}

#[test]
fn test_joined_fibers_are_reaped() {
    let source = r#"
        let i = 0;
        while (i < 100) {
            let f = perform Fork!(function() { return i });
            perform Join!(f);
            i = i + 1;
        }
        true
    "#;
    let mut runtime = Runtime::new();
    assert_eq!(eval(&mut runtime, source).unwrap(), JSValue::Bool(true));
    assert_eq!(
        runtime.fiber_count(),
        1,
        "joined fibers should be reaped; only the root fiber remains"
    );
}

#[test]
fn test_match_object_pattern_requires_keys() {
    run_test_file(
        r#"
        let a = match ({err: "boom"}) { {ok} => 1, {err} => 2 };
        let b = match ({ok: 5}) { {ok} => ok, {err} => 0 };
        let c = match ({}) { {ok} => 1, {err} => 2, _ => 3 };
        a === 2 && b === 5 && c === 3
        "#,
        "match_object_pattern_requires_keys",
    );
}

#[test]
fn test_exceptions_are_not_in_the_language() {
    let mut runtime = Runtime::new();
    assert!(
        runtime.eval("throw 1").is_err(),
        "throw must be a syntax error"
    );
    assert!(
        runtime.eval("try { 1 } catch (e) { 2 }").is_err(),
        "try/catch must be a syntax error"
    );
}

#[test]
fn test_handler_abort_replaces_catch() {
    run_test_file(
        r#"
        let r = handle {
            perform Fail!("boom");
            "unreachable"
        } with {
            Fail!(msg, resume) -> msg
        };
        r === "boom"
        "#,
        "handler_abort_replaces_catch",
    );
}

#[test]
fn test_fiber_failure_contained() {
    run_test_file(
        include_str!("fixtures/test_fiber_failure_contained.js"),
        "fiber_failure_contained",
    );
}

#[test]
fn test_join_failed_fiber_throws() {
    run_test_file(
        include_str!("fixtures/test_join_failed_fiber.js"),
        "join_failed_fiber",
    );
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

#[test]
fn test_handlers() {
    run_test_file(include_str!("fixtures/test_handlers.js"), "handlers");
}

#[test]
fn test_snapshot_saved_and_restored() {
    let dir = std::env::temp_dir().join("kryhta_snap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("basic.snap");
    let path_str = path.to_str().unwrap();

    let source = format!(
        r#"
        var log = [];
        var state = {{ count: 41 }};
        let outcome = perform Snapshot!("{path_str}");
        state.count = state.count + 1;
        outcome
        "#
    );

    let mut runtime = Runtime::new();
    let first = eval(&mut runtime, &source).unwrap();
    assert_eq!(runtime.interpreter.to_string(first), "saved");

    let bytes = std::fs::read(&path).unwrap();
    let mut restored = Runtime::from_snapshot(&bytes).unwrap();
    let second = restored.run_resumed().unwrap();
    assert_eq!(restored.interpreter.to_string(second), "restored");
    // the increment after the snapshot ran again in the restored world
    assert_eq!(
        restored.eval("state.count").unwrap(),
        JSValue::Int(42),
        "restored run continues from the snapshot point"
    );
}

#[test]
fn test_snapshot_requires_string_path() {
    let mut runtime = Runtime::new();
    assert!(eval(&mut runtime, "perform Snapshot!(42)").is_err());
}

#[test]
fn test_snapshot_round_trip_equivalence() {
    let dir = std::env::temp_dir().join("kryhta_snap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fibers.snap");
    let source = include_str!("fixtures/test_snapshot_fibers.js")
        .replace("__SNAP_PATH__", path.to_str().unwrap());

    let mut original = Runtime::new();
    let uninterrupted = eval(&mut original, &source).unwrap();
    assert_eq!(uninterrupted, JSValue::Int(52));

    let bytes = std::fs::read(&path).unwrap();
    let mut restored = Runtime::from_snapshot(&bytes).unwrap();
    let resumed = restored.run_resumed().unwrap();
    assert_eq!(
        resumed,
        JSValue::Int(52),
        "restored run must produce the identical result"
    );
}
