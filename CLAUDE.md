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
  where possible: add a fixture to `crates/bluing-hir/tests/fixtures` (see
  "Testing" in DESIGN.md) rather than guessing what rustc does.

## Checks

CI runs these; run them before committing:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build -p bluing-syntax -p bluing-hir -p bluing-std --target wasm32-unknown-unknown --locked
```
