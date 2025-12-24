//! MQuickJS REPL and CLI

use mquickjs::{JSValue, Result, VM};
use mquickjs::compiler::Compiler;
use mquickjs::parser::Parser;

use std::io::{self, BufRead, Write};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        // Execute file
        let source = std::fs::read_to_string(&args[1])
            .map_err(|_| mquickjs::JSError::InternalError("Failed to read file"))?;
        run(&source)?;
    } else {
        // REPL mode
        repl()?;
    }

    Ok(())
}

fn repl() -> Result<()> {
    println!("MQuickJS v0.1.0 - Safe Rust JavaScript Engine (Static Pools)");
    println!("Type 'exit' or Ctrl+D to quit\n");

    let stdin = io::stdin();
    // Use Box to allocate VM on heap (it's too large for stack)
    let mut vm = Box::new(VM::default());

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let line = line.trim();
                if line == "exit" {
                    break;
                }
                if line.is_empty() {
                    continue;
                }

                match eval_line(&mut *vm, line) {
                    Ok(result) => println!("{:?}", result),
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

fn eval_line(vm: &mut VM, source: &str) -> Result<JSValue> {
    let mut parser = Parser::new(source)?;
    let ast = parser.parse_program()?;
    let chunk = Compiler::new(&mut vm.strings).compile(&ast)?;
    vm.run(chunk)
}

fn run(source: &str) -> Result<()> {
    // Use Box to allocate VM on heap (it's too large for stack)
    let mut vm = Box::new(VM::default());
    match eval_line(&mut *vm, source) {
        Ok(result) => {
            if !matches!(result, JSValue::Undefined) {
                println!("{:?}", result);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}
