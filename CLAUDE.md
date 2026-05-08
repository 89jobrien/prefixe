# prefixe CLAUDE.md

## Project

Rust workspace: `crates/prefixe` (lib), `crates/prefixe-cli` (binary). Hexagonal architecture —
ports/adapters pattern. Workspace version in root `Cargo.toml` `[workspace.package]`.

## Build & Test

- `cargo nextest run --workspace` — primary test runner
- `cargo test --doc` — validate doc examples (required for #23-style doc work)
- `cargo clippy --workspace -- -D warnings` — zero warnings enforced
- `just ci` if a `justfile` is added — align local gates with CI

## Parallel Agent Dispatch

- Add `.worktrees/` to `.gitignore` before first dispatch (not present by default)
- When two slots both add `[workspace.dependencies]` entries, expect a merge conflict — resolution
  is always keep both entries, no code change needed
- Cap at 3 concurrent slots for this repo (single crate, conflicts likely if more)

## Versioning

- Workspace version lives only in root `Cargo.toml` `[workspace.package]` — crates inherit via
  `version.workspace = true`
- Tag format: `vX.Y.Z` pushed after version bump commit
