# KryhtaJS

*крихта (kryhta)* — Ukrainian for "crumb"

A tiny, safe, simple `no_std` JavaScript engine written in Rust, designed to run on microcontrollers.

## Why?

Modern embedded devices are powerful enough to run scripting languages, but most JS engines require megabytes of RAM and a heap allocator. This project explores: **what's the minimal JS engine that can run on a $4 microcontroller?**

## Design Principles

### Zero Runtime Allocation

No `malloc`. No `Box`. No `Vec`. Everything uses fixed-size arrays allocated at compile time:

```rust
pub struct VM {
    stack: FixedStack<JSValue, 256>,      // 256 value slots
    objects: Pool<Object, 512>,            // 512 objects max
    strings: FixedStringPool<4096, 256>,   // 4KB string storage
}
```

Memory budget is known at compile time. No fragmentation. No OOM surprises.

### Arena-Based AST

The parser doesn't allocate. AST nodes live in a pre-sized arena, referenced by indices:

```rust
struct ExprId(u16);  // 2 bytes instead of Box<Expr>

struct Block {
    stmts_start: u16,  // index into statement list
    stmts_count: u16,
}
```

Total AST overhead: ~36KB. Fits comfortably on Pico with room for bytecode, stack, and objects.

### Safe Rust

No `unsafe` in business logic. The few `unsafe` blocks are isolated in low-level primitives (`FixedVec`, `Pool`) with clear invariants.

## Architecture

```
Source Code
    │
    ▼
┌─────────┐    Tokens     ┌─────────┐    AST      ┌──────────┐
│  Lexer  │──────────────▶│ Parser  │────────────▶│ Compiler │
└─────────┘               └─────────┘             └────┬─────┘
                                                       │
                                                  Bytecode
                                                       │
                                                       ▼
                                                 ┌─────────┐
                                                 │   VM    │
                                                 └─────────┘
```

| Component | Description |
|-----------|-------------|
| `lexer.rs` | Zero-copy tokenizer |
| `parser_arena.rs` | Recursive descent, outputs to arena |
| `ast.rs` | Arena-based AST with index references |
| `compiler_arena.rs` | AST → bytecode |
| `bytecode.rs` | Stack-based bytecode |
| `vm.rs` | Interpreter with mark-sweep GC |

## Supported JavaScript

- [x] Primitives: `undefined`, `null`, `true`, `false`, numbers, strings
- [x] Operators: arithmetic, comparison, logical, bitwise
- [x] Variables: `let`, `const`
- [x] Control flow: `if`/`else`, `while`, `for`
- [x] Objects and arrays
- [ ] Functions and closures (in progress)
- [ ] Prototypes
- [ ] `try`/`catch`

Not planned: `eval`, `with`, regex, modules, async/await.

## Building

```bash
cargo build                    # with std
cargo build --no-default-features  # no_std for embedded
```

## Memory Usage

| Component | Size |
|-----------|------|
| AST Arena | ~20 KB |
| Bytecode | 8 KB |
| Object Pool | ~16 KB |
| String Pool | 4 KB |
| Value Stack | 4 KB |
| Call Stack | 1 KB |
| **Total** | **~53 KB** |

## References

- [mquickjs](https://github.com/bellard/mquickjs) — Compact C implementation that inspired this project's architecture

## Status

Experimental. A learning project exploring interpreter design under tight constraints.

## License

MIT — see [LICENSE](LICENSE)
