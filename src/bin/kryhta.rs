//! KryhtaJS REPL and CLI

use kryhta::{JSValue, Result, VM, JSError};
use kryhta::{ArenaParser, ArenaCompiler, AstArena};
use kryhta::fixed_string::FixedStringPool;
use kryhta::{MAX_STRING_BYTES, MAX_STRINGS};

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
    println!("KryhtaJS v0.1.0 — крихта");
    println!("Type 'exit' or Ctrl+D to quit\n");

    let stdin = io::stdin();
    let mut vm = Box::new(VM::default());

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let line = line.trim();
                if line == "exit" { break; }
                if line.is_empty() { continue; }

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
    let mut strings: FixedStringPool<MAX_STRING_BYTES, MAX_STRINGS> = FixedStringPool::default();
    let mut ast = AstArena::new();

    let mut parser = ArenaParser::new(source, &mut strings, &mut ast)?;
    parser.parse_program()?;

    let compiler = ArenaCompiler::new(&strings, &ast);
    let chunk = compiler.compile()?;

    vm.run(chunk)
}

fn run(source: &str) -> Result<()> {
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
