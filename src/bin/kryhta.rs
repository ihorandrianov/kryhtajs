//! KryhtaJS REPL and CLI
//!
//! Uses Runtime with CEKH machine for direct AST interpretation.

use kryhta::parser::Parser;
use kryhta::{Runtime, JSError, JSValue, Result};

use std::io::{self, BufRead, Write};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        let source = std::fs::read_to_string(&args[1])
            .map_err(|_| JSError::InternalError("Failed to read file"))?;
        run(&source)?;
    } else {
        repl()?;
    }

    Ok(())
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
    let strings = std::mem::take(&mut runtime.interpreter.strings);
    let parser = Parser::with_strings(source, strings)?;
    let (arena, strings) = parser.parse_program()?;
    runtime.interpreter.strings = strings;

    runtime.run(&arena)
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
