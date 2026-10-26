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
| Const generics: `const N: usize` parameters and arguments | In |
| Generic associated types | Not yet |
| Explicit `for<'a>` HRTBs beyond closure bounds, specialization | Out for now |
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
| `bluing-base` | File ids, diagnostics and their codes, line/column mapping |
| `bluing-syntax` | Parsing and subset checking |
| `bluing-hir` | Module tree, item lowering (HIR) and name resolution |
| `bluing-std` | Bluing's standard library sources, embedded in the compiler |
| `bluing-cli` | The `blu` command |

## Files, crates and the standard library

- Source files use the `.blu` extension. `mod foo;` follows Rust's rules with
  `.blu` in place of `.rs`: from a crate root or `mod.blu` file it loads
  `foo.blu` or `foo/mod.blu`; from `bar.blu` it loads `bar/foo.blu` or
  `bar/foo/mod.blu`. Finding both is an error, as in rustc. The extension is
  packaging, not meaning: the rustc oracle copies a crate to `.rs` files and
  compiles it unchanged.
- The compiler reads files through a `SourceProvider` trait, so the browser
  host supplies them from memory and the desktop host from disk.
- A script package is one crate. Its only dependency is `std`, which is the
  only name in the extern prelude. Paths through `core::` or `alloc::` are not
  supported yet; they will become re-export crates of `std`.
- `std` is written in Bluing and compiled from embedded source in **library
  mode**: `unsafe` is allowed, `#[lang = "..."]` attributes mark the items the
  compiler must know about (`Sized`, `Copy`, `Option`, ...), and there is no
  implicit prelude. This mirrors how Rust's own standard library is built with
  unstable features its users cannot use.
- `std`'s module paths mirror Rust's exactly (`std::option::Option`,
  `std::marker::Copy`, `std::prelude::rust_2024`), so the same script compiles
  against both, and the rustc oracle can test paths into `std`.
- `std` only contains complete, real definitions. Types that need machinery
  from later milestones (`String`, `Vec`, `Box` need generics checking, the
  allocator and `unsafe` codegen) arrive with that machinery, and until then
  scripts and examples cannot name them.

## Name resolution

Name resolution follows rustc's 2024-edition behavior.

- **Namespaces.** Types (modules, structs, enums, unions, traits, type aliases,
  enum variants) and values (functions, constants, statics, tuple and unit
  struct constructors, tuple and unit variants). Macros only exist as
  compiler built-ins for now.
- **Unqualified names** are looked up in this order: generic parameters of the
  enclosing items (innermost first), `Self`, the module's items and explicit
  imports, its glob imports, the extern prelude (`std`), the standard prelude
  (`std::prelude::rust_2024`), and finally primitive types. An item named like
  a primitive (`struct u8;`) shadows the primitive.
- **Imports** (`use`) are resolved to a fixed point. A lookup is only trusted
  once it is *determinate*: it is undetermined while the module still has an
  unresolved single import that could bind the name, or a glob import whose
  source module could still gain names. When no import makes progress, the
  remaining ones are reported as errors.
- **Glob imports** have lower priority than items and explicit imports. Two
  globs providing the same name for different definitions is an error only
  where the name is used.
- **Duplicate definitions** of a name in one namespace of a module, and
  explicit imports that collide with items or with each other, are errors.
- **Uniform paths.** The first segment of a `use` path is looked up through
  the same scopes as any other path, so `use Option::Some;` and
  `use u8 as Byte;` work. Items and explicit imports shadow the outer scopes
  (extern prelude, standard prelude, primitives). A name that only a glob
  import provides is ambiguous in a `use` path if an outer scope also has it;
  in other paths the glob shadows the outer scope. A leading `::` means an
  extern crate. (Each rule here was checked against rustc 1.98.)
- **Visibility.** `pub`, `pub(crate)`, `pub(super)`, `pub(self)` and
  `pub(in path)` (an ancestor module); private means visible in the defining
  module and its descendants. Every path segment after the first must be
  visible from where the path is written. A single import may not re-export
  a name more widely than the name itself is visible; a glob import re-exports
  each name at the narrower of the two visibilities.
- **Associated paths** such as `T::Item`, `Self::Output` or `<T as Trait>::X`
  are resolved as far as the module system can go; the rest is left as a
  partial resolution for type checking.

## Milestone 2 scope

Milestone 2 lowers every item to HIR and resolves every path in item
signatures: field types, function parameters and return types, `impl` headers,
trait references, bounds, `where` clauses, type aliases, and the types of
constants and statics. Lifetimes in signatures resolve to declared parameters,
`'static`, or `'_`. It also reports what Rust rejects at the level of names:
unresolved, private and ambiguous names, duplicate definitions (items,
imports, variants, fields, associated items, generic parameters, including
ones that shadow the enclosing trait's or impl's), invalid visibilities and
re-exports, paths of the wrong kind (a struct used as a trait, a variant or
module used as a type, a trait used as a type without `dyn`), and items that
need a body but have none.

Function bodies and initializer expressions are kept as syntax pointers and
resolved in milestone 3. So are the signature checks that need the meaning of
types rather than just names: generic argument counts, lifetime elision rules
(for example a reference without a lifetime in a struct field), where
`impl Trait` and `_` may appear, and whether a trait can be used as `dyn`.

Constructs the compiler does not handle yet produce an explicit "not supported
yet" error instead of being skipped:

- items inside function bodies and other blocks
- `#[cfg]`, `#[cfg_attr]` and `#[path]`
- macro calls in item position
- unions
- generic associated types
- explicit `for<'a>` binders and `use<..>` bounds

Attributes follow an allowlist: inert attributes (`allow`, `warn`, `deny`,
`forbid`, `expect`, `doc`, `inline`, `cold`, `must_use`, `deprecated`, and
`rustfmt::` / `clippy::` tool attributes), the built-in derives, and
`repr(C)` / `repr(transparent)`. Anything else is an error, so an attribute is
never silently ignored.

## Milestone 3: type checking

Milestone 3 is split into parts. Each part is complete for what it covers.
From the first part on, function bodies are checked, and any construct in a
body that the type checker does not handle yet is reported as "not supported
yet" rather than accepted unchecked.

- **3a: bodies and monomorphic types.**
  - Function bodies and constant initializers are lowered to body HIR
    (expressions, statements, patterns) with every name resolved: local
    variables and their scopes, loop labels, and item paths in both
    namespaces.
  - Types get an interned representation. Signatures are lowered from HIR
    types, with the checks that need them: generic argument counts, lifetime
    elision, and where `impl Trait` and `_` may appear.
  - Type inference for code that does not instantiate generic items:
    literals (with integer and float literal inference and fallback), local
    variables, `let` with or without annotations, blocks, `if`, `loop` and
    `while` with labels, `break` (with values) and `continue`, `return`, calls
    to functions and to tuple-struct and variant constructors, struct
    literals, field access, tuples, arrays and indexing, references and
    dereferencing, the arithmetic, comparison, logical and bitwise operators
    on primitive types, assignment and compound assignment, inherent method
    calls with automatic referencing and dereferencing, irrefutable binding
    patterns in `let` and parameters (names, `_`, tuples, structs), and the
    coercion of the never type `!` (from `return`, `break`, `loop {}`, ...)
    to any type.
- **3b: patterns and `match`.** All patterns, `match`, `if let`, `while let`,
  `let ... else`, and exhaustiveness and refutability checking.
- **3c: generics and traits.** Instantiating generic items with inference,
  trait bounds and a trait solver, trait methods and associated items,
  operator traits, `Copy`, `Clone`, `Sized`, and the built-in derives.
- **3d: coercions, casts and constants.** Reborrowing, unsizing and deref
  coercions, `as` casts, constant evaluation of array lengths, the
  deny-by-default lints that make rustc reject a program (such as
  `overflowing_literals`), and checks on `unsafe` operations.

Macro calls in bodies (`println!`, `vec!`, `assert!`, ...) expand to calls
into `std`, so they arrive with the standard library in milestone 6. Closures,
`dyn Trait` and `impl Trait` values are milestone 7; `async` is milestone 8.

Until milestone 5, programs that rustc rejects only because of moves, borrows
or assignments to immutable variables are accepted: rustc checks those on MIR
too, in its borrow checker.

## Testing

- **The rustc oracle.** Each file in `crates/bluing-hir/tests/fixtures` is a
  small crate (one or more files) with a header stating which diagnostics
  Bluing must report and whether rustc accepts it. The test runs both
  compilers on every fixture. It also enforces the subset rule directly: a
  fixture Bluing accepts and rustc rejects fails, whatever its header says.
  Fixtures named `subset_*` cover valid Rust that Bluing reports as
  unsupported.
- **Resolution targets.** `tests/resolutions.rs` checks what names resolve
  to, not only that they resolve.
- Behavior that rustc decides was checked against rustc 1.98 while writing
  the resolver; when in doubt, add a fixture rather than reasoning about it.

## Roadmap

1. **Syntax and subset checking.** *(done)* `blu check` parses and enforces
   the subset rules.
2. **Name resolution and HIR.** *(done)* All items and their signatures, the
   module tree, imports and visibility, and the start of `std`. See
   "Milestone 2 scope".
3. **Type checking**, in four parts; see "Milestone 3: type checking".
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
