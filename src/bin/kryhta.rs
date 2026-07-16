//! KryhtaJS REPL and CLI
//!
//! Uses Runtime with CEKH machine for direct AST interpretation.

use kryhta::{JSError, JSValue, Result, RunOutcome, Runtime};

use std::io::{self, BufRead, Write};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("--resume") => match args.get(2) {
            Some(path) => resume(path),
            None => {
                eprintln!("Error: Usage: kryhta --resume <file.snap>");
                std::process::exit(1);
            }
        },
        Some("--record") => match (args.get(2), args.get(3)) {
            (Some(log), Some(script)) => record(log, script),
            _ => {
                eprintln!("Error: Usage: kryhta --record <file.klog> <script.js>");
                std::process::exit(1);
            }
        },
        Some("--replay") => match args.get(2) {
            Some(path) => replay_log(path),
            None => {
                eprintln!("Error: Usage: kryhta --replay <file.klog>");
                std::process::exit(1);
            }
        },
        Some(path) => {
            let source = std::fs::read_to_string(path)
                .map_err(|_| JSError::InternalError("Failed to read file"))?;
            run(&source)
        }
        None => repl(),
    }
}

fn resume(path: &str) -> Result<()> {
    let attempt = std::fs::read(path)
        .map_err(|e| JSError::Message(format!("snapshot: cannot read {path}: {e}")))
        .and_then(|bytes| {
            Runtime::from_snapshot(&bytes)
                .map_err(|e| JSError::Message(format!("snapshot: cannot resume {path}: {e}")))
        });

    let mut runtime = match attempt {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    match runtime.run_resumed() {
        Ok(kryhta::RunOutcome::Done(result)) => {
            if !matches!(result, JSValue::Undefined) {
                print_value(&result, &runtime);
            }
            Ok(())
        }
        Ok(kryhta::RunOutcome::Pending(calls)) => {
            eprintln!(
                "Error: snapshot is waiting on host effect '{}'; the CLI has no host tools — resume it from a Rust embedder",
                calls[0].effect
            );
            std::process::exit(1);
        }
        Ok(kryhta::RunOutcome::OutOfFuel { spent }) => {
            eprintln!("out of fuel after {spent} steps");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn record(log_path: &str, script_path: &str) -> Result<()> {
    let source = std::fs::read_to_string(script_path)
        .map_err(|_| JSError::InternalError("Failed to read file"))?;
    let mut runtime = Runtime::new();
    if let Err(e) = runtime
        .record_to(log_path)
        .and_then(|_| runtime.eval_hosted(&source))
        .map(|outcome| report_outcome(outcome, &runtime))
    {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
    Ok(())
}

fn replay_log(path: &str) -> Result<()> {
    match Runtime::resume_from_log(path) {
        Ok((runtime, outcome)) => {
            report_outcome(outcome, &runtime);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn report_outcome(outcome: RunOutcome, runtime: &Runtime) {
    match outcome {
        RunOutcome::Done(result) => {
            if !matches!(result, JSValue::Undefined) {
                print_value(&result, runtime);
            }
        }
        RunOutcome::Pending(calls) => {
            eprintln!(
                "Error: run is waiting on host effect '{}'; the CLI has no host tools — answer it from a Rust embedder",
                calls[0].effect
            );
            std::process::exit(1);
        }
        RunOutcome::OutOfFuel { spent } => {
            eprintln!("out of fuel after {spent} steps");
            std::process::exit(1);
        }
    }
}

fn repl() -> Result<()> {
    println!("KryhtaJS v0.2.0 — крихта (Runtime)");
    println!("Type 'exit' or Ctrl+D to quit\n");

    let stdin = io::stdin();
    let mut runtime = Runtime::new();

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let line = line.trim();
                if line == "exit" {
                    break;
                }
                if line.is_empty() {
                    continue;
                }

                match eval_line(&mut runtime, line) {
                    Ok(result) => {
                        print_value(&result, &runtime);
                    }
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }
    }

    println!("\nGoodbye!");
    Ok(())
}

fn eval_line(runtime: &mut Runtime, source: &str) -> Result<JSValue> {
    runtime.eval(source)
}

fn run(source: &str) -> Result<()> {
    let mut runtime = Runtime::new();
    match eval_line(&mut runtime, source) {
        Ok(result) => {
            if !matches!(result, JSValue::Undefined) {
                print_value(&result, &runtime);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

fn print_value(val: &JSValue, runtime: &Runtime) {
    match val {
        JSValue::Undefined => println!("undefined"),
        JSValue::Null => println!("null"),
        JSValue::Bool(b) => println!("{}", b),
        JSValue::Int(n) => println!("{}", n),
        JSValue::Float(f) => {
            if f.is_nan() {
                println!("NaN");
            } else if f.is_infinite() {
                if *f > 0.0 {
                    println!("Infinity");
                } else {
                    println!("-Infinity");
                }
            } else if f.fract() == 0.0 {
                println!("{}", *f as i64);
            } else {
                println!("{}", f);
            }
        }
        JSValue::String(str_id) => {
            if let Some(s) = runtime.interpreter.strings.get(*str_id) {
                println!("'{}'", s);
            } else {
                println!("<invalid string>");
            }
        }
        JSValue::Object(_) => println!("[object Object]"),
        JSValue::Array(id) => {
            if let Some(obj) = runtime.interpreter.objects.get(id.into_arena_id()) {
                if let Some(arr) = obj.as_array() {
                    let parts: Vec<String> = arr
                        .elements
                        .iter()
                        .map(|v| format_value(v, runtime))
                        .collect();
                    println!("[{}]", parts.join(", "));
                } else {
                    println!("[object Array]");
                }
            } else {
                println!("[object Array]");
            }
        }
        JSValue::Function(_) | JSValue::Continuation(_, _) => println!("[Function]"),
        JSValue::Handler(_) => println!("[Handler]"),
    }
}

fn format_value(val: &JSValue, runtime: &Runtime) -> String {
    match val {
        JSValue::Undefined => "undefined".to_string(),
        JSValue::Null => "null".to_string(),
        JSValue::Bool(b) => b.to_string(),
        JSValue::Int(n) => n.to_string(),
        JSValue::Float(f) => {
            if f.is_nan() {
                "NaN".to_string()
            } else if f.is_infinite() {
                if *f > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }
            } else {
                f.to_string()
            }
        }
        JSValue::String(str_id) => {
            if let Some(s) = runtime.interpreter.strings.get(*str_id) {
                format!("'{}'", s)
            } else {
                "<invalid>".to_string()
            }
        }
        JSValue::Object(_) => "[object Object]".to_string(),
        JSValue::Array(_) => "[Array]".to_string(),
        JSValue::Function(_) | JSValue::Continuation(_, _) => "[Function]".to_string(),
        JSValue::Handler(_) => "[Handler]".to_string(),
    }
}
