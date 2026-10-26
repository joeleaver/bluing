# Working on Bluing

Read [DESIGN.md](DESIGN.md) first. It is the canonical record of scope,
decisions and the roadmap; update it when a decision changes.

## Rules

- **No technical debt to get something working sooner.** This is absolute.
  If the right way is slower, slow down and do it the right way. No stubs
  presented as done, no special cases standing in for the general mechanism,
  no "fix later" hacks. When something genuinely has to wait, say so
  explicitly in DESIGN.md or a tracked issue, not only in a code comment.
- **Source files stay around 500 lines or less.** Split by responsibility
  before a file grows past that, rather than after.
- **The subset rule is the spec.** Every accepted program must be valid Rust
  that means the same thing. When unsure what Bluing should do, rustc's
  behavior (stable, NLL) is the answer; for `unsafe` code, Miri's.
- Behavior changes come with tests. Language behavior is tested against rustc
  where possible: add a fixture to `crates/bluing/tests/fixtures` (see
  "Testing" in DESIGN.md) rather than guessing what rustc does.

## Where things stand

The roadmap in DESIGN.md marks what is done. As of 2026-10-25: milestones 1,
2 and 3a are done; **next is 3b, patterns and `match`** ("Milestone 3:
type checking" in DESIGN.md lists its scope): every pattern kind, `match`,
`if let`, `while let`, `let ... else`, and exhaustiveness and refutability
checking. Pattern lowering lives in `crates/bluing-hir/src/body/pat.rs`
(today it reports everything beyond 3a's binding patterns as not supported
yet) and pattern checking in `crates/bluing-typeck/src/pat.rs`.

## How the compiler fits together

`bluing` (the driver, `crates/bluing/src/lib.rs`) runs the phases in order:

1. `bluing-syntax`: parse with rust-analyzer's parser, subset and attribute
   checks.
2. `bluing-hir`: module tree and item lowering (`lower/`), name resolution
   (`resolve/`), body lowering with local names resolved (`body/`).
3. `bluing-ty`: types, generics and every item's signature, impl checks.
4. `bluing-typeck`: inference and checking of bodies.

`bluing-std` embeds the standard library's Bluing sources; `bluing-base`
holds file ids and the one list of diagnostic codes.

## Working practices

- **Not supported yet is not an error.** Valid Rust that Bluing cannot
  check yet must report `not-yet-supported`, never a false error (a missing
  coercion is not a type mismatch, a method from `std` that does not exist
  yet is not "no method found").
- **Ask rustc before implementing a rule.** When unsure how rustc behaves,
  write a tiny file and compile it with
  `rustc --edition 2024 --crate-type lib --emit=metadata --target wasm32-unknown-unknown`,
  then encode the answer as a fixture.
- **Fixtures move as features land.** `accept_*` fixtures use only what is
  implemented; constructs from later milestones go in `subset_*` fixtures,
  which pin them as not supported yet. When a feature lands, move its cases
  from `subset_*` into `accept_*`/`reject_*`.
- **Verify multi-file edits.** After an edit touching several files (and
  after `cargo fmt` reflows code), check every intended change landed; a
  test for each hook is the reliable way.

## Checks

CI runs these; run them before committing:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build -p bluing --target wasm32-unknown-unknown --locked
```
