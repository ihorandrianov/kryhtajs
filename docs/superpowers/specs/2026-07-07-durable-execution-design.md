# Durable Execution — Design Spec

**Date:** 2026-07-07
**Status:** Approved for implementation planning

## Context and goal

KryhtaJS's north star is an **effects-first scripting language for durable,
sandboxed agent workflows**: the small, deterministic, snapshottable
orchestration layer that agent plans are written in, while heavy lifting stays
in the host. Durable execution is the first pillar: a running program —
including every suspended fiber — can be serialized to disk mid-run, the
process killed, and execution resumed later exactly where it left off.

This is feasible because the CEKH machine's entire state is plain data:
index-based arenas (`u32` ids, no pointers), enum payloads, `Vec`s, and a few
`HashMap`s. Native functions are dispatch tags (`NativeFn::MathFloor`, …), not
function pointers, so nothing needs relinking on restore.

Closest prior art: Golem (durable WASM), Temporal/Restate (framework-level
durable execution), LangGraph checkpointing. KryhtaJS's angle is
language-level: the continuation *is* the checkpoint.

## Language semantics

One new runtime effect, handled by the scheduler like `Print!`/`Fork!`/`Gc!`:

```javascript
match (perform Snapshot!("job.snap")) {
    "saved"    => perform Print!("checkpoint written"),
    "restored" => perform Print!("welcome back")
}
```

- In the original run, the effect writes the snapshot file and evaluates to
  the string `"saved"`; execution continues normally.
- When a process resumes from that file, execution wakes **inside the same
  effect**, which evaluates to `"restored"` — fork(2) semantics.
- The snapshot captures the **whole runtime**: every fiber (ready, blocked on
  Join, completed-but-unjoined, failed), the object/env/cont/handler arenas,
  the string pool, globals, the scheduler queues, and the AST arena.
- Snapshots are **self-contained**: resuming does not need the original
  source file (the AST is inside the snapshot).
- Argument: one string, the file path. Non-string argument is a `TypeError`
  (faults the performing fiber, consistent with other runtime effects).
- I/O failure while writing the file faults the performing fiber (fault-class
  problem, not an expected-error value).

## Architecture

New module `src/snapshot.rs`, zero dependencies:

- `ByteWriter` / `ByteReader`: little-endian primitives (`u8`, `u16`, `u32`,
  `u64`, `f64`, `i32`), length-prefixed byte strings, length-prefixed
  sequences.
- `write_runtime(&Runtime) -> Vec<u8>` and
  `read_runtime(&[u8]) -> Result<Runtime>`.
- A `write_*`/`read_*` pair per machine type. The large mechanical item is
  `Kont` (~35 variants): tag byte + fields. `Bindings` (env storage)
  serializes as a `(StrId, JSValue)` list regardless of Small/Large
  representation; the Small/Large split is rebuilt naturally on insert.
- `StringPool` serializes only the strings vec; the intern map is rebuilt on
  load.

**Scheduler refactor (required):** `Runtime::run` currently resets fiber
state per run. Split it into setup + `run_scheduler()` (the loop). Resume
constructs the `Runtime` from the snapshot and enters `run_scheduler()`
directly — no reset, no special restore logic anywhere else.

**Snapshot moment** (in `handle_effect`):

1. Set the performing fiber's control to `Value("restored")` and sync it into
   the fibers list (as `save_current_fiber_state` does).
2. Serialize the entire runtime to bytes; write the file.
3. Set the live control to `Value("saved")` and continue.

The file contains a machine one instruction away from seeing `"restored"`.

## File format

```
magic   "KRHT" (4 bytes)
version u8      (1)
sections, fixed order:
  string pool → AST arena → objects → envs → conts → handlers
  → globals → fibers → scheduler (ready queue, current, next_fiber_id,
    join_waiters)
```

- Version mismatch or bad magic → hard error, no migration attempts in v1.
- GC state is **not** serialized: a fresh `GC` (fresh stats, snapshot
  baseline re-primed from `total_allocations()` at load) is correct by
  construction, since collection is an optimization.
- Truncated/corrupt input must produce `Err`, never a panic: all reads are
  bounds-checked (`ByteReader` returns `Result`).

## JSError round-trip

`JSError` messages are `&'static str` and cannot round-trip through a file.
Add one owned variant, `JSError::Message(String)`. Failed fibers serialize
their error's `Display` string and restore as `Message`. No other error
variant changes.

## CLI

- `kryhta --resume job.snap` — explicit flag, no file-extension magic.
- Resume runs the scheduler to completion and prints the final value exactly
  as a normal run would.
- Public API: `Runtime::from_snapshot(bytes: &[u8]) -> Result<Runtime>` and
  `Runtime::run_resumed(&mut self) -> Result<JSValue>`, which enters the
  scheduler loop without the per-run reset.

## Out of scope (v1)

- wasm/browser persistence (the bytes-in/bytes-out API makes this a follow-up:
  host stores bytes in IndexedDB or downloads them).
- External/automatic triggers (REPL command, SIGINT, every-N-effects) — same
  machinery, later.
- Per-fiber portable continuations (heap slicing/id remapping) — approach C,
  future work.
- Snapshot-format migrations between engine versions.
- Multiple snapshots/branching timelines (works incidentally — one file per
  `Snapshot!` call — but no tooling around it).

## Testing (TDD)

1. **Round-trip equivalence (load-bearing):** a fixture that builds
   non-trivial state — closures over mutated envs, nested objects/arrays, a
   fiber blocked on Join, a completed-unjoined fiber — then performs
   `Snapshot!` targeting a temp-file path. The test reads the file bytes,
   restores a fresh `Runtime` from them, runs it to completion, and asserts
   the final value equals what the uninterrupted run produced.
2. **Discrimination:** original run sees `"saved"`; restored run sees
   `"restored"`.
3. **Self-containment:** resume works with only the `.snap` bytes (no source).
4. **Rejection:** bad magic, wrong version, truncated file → `Err`, no panic.
5. **End-to-end CLI:** run a script that snapshots and exits; `--resume` it;
   assert continued output. (Process-kill drama is the demo, not the test.)

## Flagship demo (post-implementation)

A counter loop that increments, prints, snapshots, and sleeps — kill the
process at any point, `--resume`, and it continues from the exact iteration.
Goes in `examples/` and the README.
