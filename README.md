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

## Errors

There is no `try`/`catch`/`throw`. Errors are values and effects:

- **Expected failures** are values — return `{err: reason}` and destructure
  with `match`. Object patterns are structural: `{ok}` only matches when the
  key exists, so `{ok}`/`{err}` arms discriminate.
- **Recoverable failures** are effects — `perform Fail!(reason)` and let a
  handler decide: call `resume(fallback)` to continue, or don't resume to
  abort the handled block with the clause's value (this is what `catch`
  would have been, but with the power to resume).
- **Faults** (calling a non-function, unknown effects, ...) kill the fiber.
  The root fiber's fault ends the program; a child fiber's fault surfaces
  where it's joined.

```javascript
// Recoverable: handler aborts (never resumes)
let r = handle {
    perform Fail!("boom");
    "unreachable"
} with {
    Fail!(msg, resume) -> msg
};  // r === "boom"

// Fibers: Join yields the fiber's fate as a value
let f = perform Fork!(function() { return 42 });
match (perform Join!(f)) {
    {ok} => ok,      // fiber returned: ok === 42
    {err} => 0       // fiber crashed: err is the reason
}
```

## License

MIT
