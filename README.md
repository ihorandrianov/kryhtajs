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

## Durable execution

A running program — including every suspended fiber — can checkpoint itself
to a self-contained file and be resumed later, even after the process dies:

```javascript
match (perform Snapshot!("job.snap")) {
    "saved"    => perform Print!("checkpoint written"),
    "restored" => perform Print!("welcome back")
}
```

```bash
kryhta job.js              # runs, writes job.snap at the Snapshot! call
kryhta --resume job.snap   # wakes up inside that call, seeing "restored"
```

The snapshot contains the whole machine (fibers, heap, continuations, AST),
so resuming does not need the original source file.

## Host effects

A script can ask the embedding Rust host for something the language itself
can't provide — a network call, a file read, a human's approval — by
performing an effect the host has **granted**. No in-language `handle`
catches it, so the fiber suspends; the host answers it from Rust:

```rust
use kryhta::{HostValue, Result, RunOutcome, Runtime};

fn main() -> Result<()> {
    let mut rt = Runtime::new();
    rt.grant("AskHuman");

    let mut outcome = rt.eval_hosted(
        "let answer = perform AskHuman!(\"Approve the deploy?\");\n\
         perform Print!(answer);\n\
         answer",
    )?;

    while let RunOutcome::Pending(calls) = outcome {
        for call in calls {
            println!("host got effect {}({:?})", call.effect, call.args);
            rt.resume_with(call.id, HostValue::Str("approved".to_string()))?;
        }
        outcome = rt.run_hosted_continue()?;
    }

    Ok(())
}
```

- Performing an ungranted, unhandled effect faults the fiber at the perform
  site — capability security by construction.
- Other fibers keep running while one is blocked on a host call; when the
  ready queue drains, every pending call surfaces together in
  `RunOutcome::Pending`, answerable in any order or subset.
- Pending calls survive `runtime.snapshot(path)` / restore: a process can
  suspend on a tool call, die, and a later process resumes it and sees the
  same call.

Run `cargo run --example host_effect` to see this loop end to end.

## License

MIT
