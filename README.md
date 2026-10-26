# Bluing

Bluing is a strongly typed scripting language that is a **strict subset of Rust**.
Every valid Bluing program is a valid Rust program and means the same thing,
so what you know about Rust carries over directly, and Rust tooling (rustfmt,
rust-analyzer, syntax highlighting) works on Bluing scripts.

The compiler is written in Rust and is meant to run anywhere wasm runs,
including inside a browser tab. It compiles scripts to WebAssembly modules, so
they run at JIT speed without a native toolchain, and without shipping rustc
or LLVM.

> Bluing is the treatment that turns steel's surface into a protective finish
> that resists rust. Bluing is Rust, made safe for scripting.

## Status

Early. Today `blu check` loads a script and its module files, enforces the
subset rules, resolves every name, and type-checks item signatures and
function bodies that do not use generics or traits, all checked against
rustc. Generics and traits, borrow checking and code generation are next;
see [DESIGN.md](DESIGN.md) for the plan.

```bash
cargo run -p bluing-cli -- check examples/enemy.blu
```

## What's in, what's out

- **In:** structs, enums, traits, generics, closures, pattern matching,
  references and lifetimes with full borrow checking, `async`/`.await`.
- **Gated:** `unsafe` is supported (Bluing's standard library is written with
  it) but is off for scripts unless a package opts in.
- **Out:** threads, FFI, inline assembly, proc macros, crates.io dependencies.
  `macro_rules!` is not supported yet.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
