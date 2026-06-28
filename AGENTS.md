# prefixe — Agent Operating Guide

A Rust workspace for shell command prefix learning and scripting. Library crate
(`prefixe`) + CLI binary (`prefixe-cli`). Hexagonal architecture with ports/adapters
pattern. Workspace version: 0.4.0 (Rust edition 2024).

## Build, Test, Lint Commands

### Quick Reference

```bash
# Build all crates
cargo build --workspace

# Run tests (primary test runner)
cargo nextest run --workspace

# Validate doc examples
cargo test --doc

# Lint (strict)
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --all -- --check

# Full CI check (local simulation)
cargo fmt --all && cargo clippy --workspace -- -D warnings \
  && cargo nextest run --workspace && cargo test --doc
```

### Running Individual Tests

```bash
# All tests in a crate
cargo nextest run -p prefixe
cargo nextest run -p prefixe-cli

# By test name
cargo nextest run test_name

# Integration tests only
cargo test --test '*'

# Doc tests
cargo test --doc
```

## Code Style Guidelines

### Formatting & Linting

- **Max width**: 100 characters (rustfmt.toml enforced)
- **Edition**: 2024 — match ergonomics, auto-ref-deref
- **Strict clippy**: `cargo clippy --workspace -- -D warnings` enforced
- **No warnings**: Zero clippy warnings policy

### Naming Conventions

- **Structs/Enums**: PascalCase (`CommandPrefix`, `ParseError`)
- **Functions/Methods/Variables**: snake_case (`parse_shell_script`, `prefix_map`)
- **Constants**: SCREAMING_SNAKE_CASE
- **Modules**: snake_case (`shell_parser`, `script_utils`)
- **Files**: snake_case.rs

### Imports & Dependencies

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use thiserror::Error;
```

Shared workspace dependencies in `Cargo.toml [workspace.dependencies]`: serde,
toml, shell-words, tempfile, thiserror, glob, proptest, serde_json, regex.

### Error Handling

- Primary: `anyhow::Result<T>` for fallible operations
- Custom errors: `thiserror::Error` for domain-specific errors
- No `unwrap()` / `expect()` in library code — propagate with `?`

### Testing Patterns

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptive_name() {
        // Test implementation
    }
}
```

- Unit tests: Same file as implementation
- Integration tests: `tests/` directory as separate files
- Doc tests: Validate examples in doc comments with `cargo test --doc`
- Property-based: Use `proptest` for generative testing

## Project Structure

```
prefixe/
├── crates/
│   ├── prefixe/        # Library: core parsing, shell abstractions
│   └── prefixe-cli/    # Binary: CLI entry point
├── docs/
│   └── book/           # mdBook documentation
├── Cargo.toml          # Workspace manifest
└── rust-toolchain.toml # Pinned Rust version
```

### Workspace Layout

- **`crates/prefixe/`** — Library providing shell prefix learning API
- **`crates/prefixe-cli/`** — CLI binary using the library
- **Version inheritance**: Both crates inherit from `[workspace.package]` via
  `version.workspace = true`

### Module Organization

- **Ports**: Abstract traits/interfaces for external integration
- **Adapters**: Concrete implementations of ports (shell-specific, filesystem)
- **Domain**: Core business logic (learning algorithm, prefix matching)
- **Utils**: Shared helpers (parsing, validation)

## Parallel Agent Dispatch

> **Capacity: 3 concurrent slots** (single workspace, merge conflicts likely with
> dependency updates)

### Setup (first dispatch only)

```bash
echo ".worktrees/" >> .gitignore
git add -A && git commit -m "chore: add worktree exclusion"
```

### Merge Conflict Handling

When two agents both edit `[workspace.dependencies]`:

- Conflict is expected — resolution always keeps **both** entries
- No code change needed — just resolve the TOML conflict
- Example:

  ```toml
  # Before
  serde = { version = "1", features = ["derive"] }

  # Agent A adds:
  chrono = "0.4"
  # Agent B adds:
  uuid = { version = "1", features = ["v4"] }

  # Resolved (keep all three):
  serde = { version = "1", features = ["derive"] }
  chrono = "0.4"
  uuid = { version = "1", features = ["v4"] }
  ```

## Versioning

- **Workspace version**: Single source at root `Cargo.toml` `[workspace.package]`
- **Inheritance**: Both crates use `version.workspace = true`
- **Bumping**: Edit root `[workspace.package] version` only — no per-crate versions
- **Tag format**: Push `vX.Y.Z` after version bump commit

## Commit Guidelines

### Pre-Commit Checks

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
cargo test --doc
```

### Commit Message Style

- Type-first: `<type>(<scope>): <description>`
- Types: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`
- Example: `feat(cli): add shell detection subcommand`

## Key Dependencies

| Crate         | Purpose                              |
| ------------- | ------------------------------------ |
| `serde`       | Serialization (derive-based)         |
| `toml`        | TOML parsing (shell config files)    |
| `shell-words` | Shell argument splitting & escaping  |
| `tempfile`    | Temporary file/dir isolation (tests) |
| `thiserror`   | Domain error types                   |
| `glob`        | File globbing (pattern matching)     |
| `proptest`    | Property-based testing               |
| `serde_json`  | JSON serialization                   |
| `regex`       | Pattern matching (prefix parsing)    |

## Development Workflow

1. **Read CLAUDE.md first** — project-specific guidance in `CLAUDE.md`
2. **Build**: `cargo build --workspace`
3. **Test**: `cargo nextest run --workspace` (preferred over `cargo test`)
4. **Lint**: `cargo clippy --workspace -- -D warnings`
5. **Format**: `cargo fmt --all`
6. **Commit**: Follow conventional format; type-first

## Self-Protection

This is a single-developer learning project — no container/destructive operations
to guard against.
