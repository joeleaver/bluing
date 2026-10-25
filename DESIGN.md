# Bluing design

## Goals

1. **A strict subset of Rust.** Every program Bluing accepts is accepted by
   stable rustc and behaves the same. Knowledge, tooling and LLM fluency in
   Rust transfer without change.
2. **A scripting language, not a Rust competitor.** Fast compiles, small
   download, embeddable. Low-level features are dropped when they cost more
   than they give a script author.
3. **Runs anywhere wasm runs.** The compiler itself is a wasm module that can
   run in a browser tab; its output is wasm that the host's JIT optimizes.
4. **Safe by construction.** Scripts run in a wasm sandbox; on top of that,
   ordinary scripts cannot use `unsafe`.

## The subset rule

> Every valid Bluing program is a valid Rust program and means the same thing.

Consequences:

- **rustc is the test oracle.** Conformance tests compile each program with
  both Bluing and rustc and compare the result. Bluing must reject what rustc
  rejects, and should accept what rustc accepts within the supported subset.
- For tests that use `unsafe`, **Miri** is the oracle: the rule only covers
  programs without undefined behavior.
- **Borrow checking targets NLL, not Polonius.** Stable rustc uses NLL
  (RFC 2094). Accepting programs only Polonius accepts would break the rule.
- Scripts can be compiled natively with rustc for release builds, against a
  native implementation of the host API.
- No syntax or semantics of our own. Where Bluing needs configuration (for
  example the `unsafe` opt-in), it goes in package metadata, not in source.

## Feature scope

| Area | Status |
|---|---|
| Structs, enums, `impl`, `match`, patterns, modules | In |
| Traits, generics (monomorphized), `impl Trait` | In |
| References, lifetimes, elision, variance, full NLL borrow checking | In |
| Closures with capture inference (RFC 2229), `Fn(&T)` bounds | In |
| `dyn Trait` | In |
| `async fn`, `async` blocks and closures, async fn in traits | In |
| `unsafe`, raw pointers, `MaybeUninit`, unions | Gated: per-package opt-in |
| `macro_rules!` | Not yet |
| Explicit `for<'a>` HRTBs beyond closure bounds, GATs, specialization | Out for now |
| Threads, `Send`/`Sync`, atomics | Out |
| FFI (`extern`), inline assembly, `repr` beyond `C`/`transparent` | Out, permanently |
| Proc macros, `build.rs`, crates.io dependencies | Out |

Built-in derives (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `PartialOrd`,
`Ord`, `Hash`, `Default`) and built-in macros (`println!`, `format!`,
`vec!`, `assert!` and friends) are handled by the compiler.

## `unsafe`

The compiler supports `unsafe` fully. Bluing's standard library (`Vec`,
`String`, `Rc`, `RefCell`, `Pin`, iterators, ...) is written in Bluing using it,
the same way Rust's is.

Scripts cannot use `unsafe` unless their package opts in: the default behaves
as if every script had `#![forbid(unsafe_code)]`. Users never need `unsafe`
for their own types; structs, enums, trees (`Box`), shared graphs
(`Rc<RefCell<T>>`) and index-based structures are all safe Rust. The opt-in
is per package so a vetted helper library can use `unsafe` internally while
the scripts calling it stay safe.

Keeping `unsafe` off by default keeps scripts deterministic and safe to
hot-reload. With it on, the wasm sandbox still protects the host: undefined
behavior can only corrupt the script's own memory.

Supporting `unsafe` fixes these runtime decisions:

- Values live in **wasm linear memory**, not WasmGC; pointers are offsets.
- `repr(Rust)` layout is ours to choose, but `repr(C)`, `repr(transparent)`,
  `size_of`/`align_of` and Rust's guaranteed niches (`Option<&T>`,
  `Option<Box<T>>`, `Option<NonNull<T>>` are pointer-sized) are honored.
- Host APIs take handles or bounds-checked slices, never raw pointers they
  trust.

## Async

`async` is in; threads are out. The host (for example a game engine) owns a
single-threaded executor that polls ready tasks in a fixed order each tick,
so execution is deterministic. Scripts use the real `Future`, `Poll`,
`Context` and `Waker` types; host async functions (`next_frame()`,
`sleep(secs)`, I/O) are declared in a Rust crate so scripts still compile
natively.

The compiler lowers `async` to state machines, as rustc does: locals live
across an `.await` move into the future's state, borrows held across an
`.await` make it self-referential (hence `Pin`), and dropping a pending
future drops exactly the live state. Wasm stack switching was rejected
because it gives fibers, not Rust's lazy, poll-based, cancel-by-drop futures.

## Hosts: browser and native desktop

Bluing runs in two kinds of host: the browser and native desktop apps.
**Both run the same wasm.** There is one compiler and one backend, and wasm is
its only output format.

| | Browser | Native desktop |
|---|---|---|
| Compiler | `bluing` library built for wasm32, run in a web worker | The same library, built natively and linked into the app |
| Runs scripts with | The browser's own wasm engine (`WebAssembly.instantiate`) | An embedded wasm runtime: **wasmtime** (Cranelift JIT) by default, **wasmi** (interpreter) where JIT is unavailable or binary size matters |
| Host API | JS glue generated from the host binding description | Rust host functions registered with the runtime |

Why one output format instead of a native backend as well:

- **Same semantics everywhere.** A script can't behave differently on desktop
  and in the browser, which matters for determinism and collaboration.
- **One backend to write and test.** A second backend (native code through
  Cranelift, or a bytecode VM) would double the codegen and conformance work.
- **The sandbox comes for free on desktop too.** wasmtime provides fuel limits,
  memory caps and capability-scoped imports, the same isolation as the browser.
- **Platforms without JIT** (for example iOS) use wasmi, which runs the same
  modules more slowly.

Requirements this puts on the runtime: NaN canonicalization on (wasmtime has a
setting for it) so float results match across hosts, and async host calls
(wasmtime supports them) so the engine's executor works the same way natively.

A native release build through rustc stays available as an option, since
scripts are valid Rust, but the normal path on every host is compile to wasm
and run it.

## Pipeline

```
source ─ parse ─ name resolution ─ type check (with region variables) ─ MIR ─┬─ borrow check
                                                                             └─ wasm codegen
```

- **Parsing** uses rust-analyzer's parser (`ra_ap_syntax`): lossless and
  error-tolerant, which suits an editor. It builds for `wasm32-unknown-unknown`.
- **Everything after parsing is our own.** rust-analyzer's type inference
  largely erases lifetimes, so it can't feed a borrow checker; rustc's
  `rustc_borrowck` is tied to rustc's internals and can't be vendored.
- **Borrow checking is a pure analysis pass.** Lifetimes never change
  generated code, so codegen does not depend on it and both can be developed
  in parallel. `polonius-engine`'s fact model and a-mir-formality are useful
  references; the target behavior is NLL.
- **Codegen** lowers MIR to wasm directly: a stackifier turns the control-flow
  graph into wasm's structured blocks, and the compiler writes the final
  module itself, so there is no linker step.

## Crates

| Crate | Role |
|---|---|
| `bluing-syntax` | Parsing and subset checking |
| `bluing-cli` | The `blu` command |

## Roadmap

1. **Syntax and subset checking.** *(done)* `blu check` parses and enforces
   the subset rules.
2. **Name resolution and HIR** for structs, enums, functions, `impl`s and
   modules.
3. **Type checking** of non-generic code, then generics and traits.
4. **MIR and wasm codegen** for integer and struct code; first runnable
   scripts, verified against rustc.
5. **Borrow checking**: moves and initialization, then regions and NLL.
6. **Standard library** in Bluing: `Vec`, `String`, `Option`, `Result`,
   `Box`, `Rc`, `RefCell`, iterators.
7. **Closures, `dyn Trait`, `impl Trait`.**
8. **Async**: state-machine lowering and the host executor interface.
9. **Host embedding**: a derive macro for host bindings, hot reload, and the
   two hosts: the browser (compiler in a web worker) and native desktop
   (compiler linked in, scripts run on wasmtime or wasmi).
