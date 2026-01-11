# KryhtaJS

*крихта (kryhta)* — Ukrainian for "crumb"

A JavaScript interpreter with a CEKH machine and algebraic effects, written in Rust.

## CEKH Machine

Interprets AST directly with explicit state:
- **C** (Control) — current expression/statement
- **E** (Environment) — variable bindings
- **K** (Kontinuation) — what to do next
- **H** (Handlers) — effect handlers

Continuations as data enables algebraic effects.

## Build

```bash
cargo build --bin kryhta
cargo run --bin kryhta          # REPL
cargo run --bin kryhta file.js  # Run file
```

## Example

```javascript
// Pattern matching
function fib(n) {
    return match (n) {
        0 => 0,
        1 => 1,
        x => fib(x - 1) + fib(x - 2)
    }
}

// Algebraic effects
handle {
    let x = perform Get!();
    perform Put!(x + 1);
    perform Get!()
} with {
    Get!(resume) -> resume(42),
    Put!(value, resume) -> resume(value * 2)
}
```

## License

MIT
