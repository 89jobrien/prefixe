# Plan: SOLID / Hexagonal Architecture Refactor

## Goal

Restructure `prefixe` into a clean hexagonal architecture: pure domain layer with zero
infrastructure dependencies, typed ports, a crate-level error type, and infrastructure
adapters isolated in their own module.

## Architecture

- Crates affected: `crates/prefixe`
- New files:
  - `src/error.rs` — crate `Error` enum (thiserror)
  - `src/domain.rs` — pure domain types + ports (no serde, no std::fs)
  - `src/infra/mod.rs` — infrastructure module root
  - `src/infra/toml_store.rs` — TOML DTO types + FilePrefixStore + FileProbeStore
  - `src/infra/path.rs` — PathResolver port + EnvPathResolver adapter
  - `src/engine.rs` — PrefixEngine service
- Deleted: `src/lib.rs` refactored into the above modules, re-exported from lib.rs
- Data flow:
  `EnvPathResolver` → `FilePrefixStore/FileProbeStore` → `PrefixEngine` → `rewrite`/`audit`

## Tech Stack

- Rust edition 2024
- New dependency: `thiserror = "1"` (crate Error type)
- Existing: `serde`, `toml`, `shell-words`

## Tasks

### Task 1: Add thiserror and define crate Error type (closes #10)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/Cargo.toml`, `crates/prefixe/src/error.rs`
**Run**: `cargo nextest run -p prefixe`

1. Add dependency to `crates/prefixe/Cargo.toml`:

   ```toml
   [dependencies]
   thiserror = "1"
   ```

   Also add to workspace `Cargo.toml`:

   ```toml
   thiserror = "1"
   ```

   And use workspace = true in crate.

2. Write `src/error.rs`:

   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum Error {
       #[error("I/O error: {0}")]
       Io(#[from] std::io::Error),
       #[error("TOML serialization error: {0}")]
       Serialize(#[from] toml::ser::Error),
       #[error("TOML parse error: {0}")]
       Parse(#[from] toml::de::Error),
   }
   ```

3. Re-export from `lib.rs`:

   ```rust
   pub mod error;
   pub use error::Error;
   ```

4. Verify: `cargo check -p prefixe` → clean.
5. `git commit -m "feat(prefixe): introduce crate Error type with thiserror (#10)"`

---

### Task 2: Introduce OriginalCommand newtype (closes #9)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/domain.rs` (new)
**Run**: `cargo nextest run -p prefixe`

1. Create `src/domain.rs` with the newtype:

   ```rust
   /// Newtype wrapper for an original (pre-rewrite) shell command string.
   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct OriginalCommand(pub String);

   impl OriginalCommand {
       pub fn as_str(&self) -> &str {
           &self.0
       }
   }

   impl From<&str> for OriginalCommand {
       fn from(s: &str) -> Self {
           Self(s.to_string())
       }
   }

   impl From<String> for OriginalCommand {
       fn from(s: String) -> Self {
           Self(s)
       }
   }
   ```

2. Update `ProbeEntry` in `lib.rs` to use `OriginalCommand`:

   ```rust
   pub struct ProbeEntry {
       pub key: String,
       pub prefix: Vec<String>,
       pub original_command: OriginalCommand,
   }
   ```

3. Update `ProbeStore::remove_matching` signature:

   ```rust
   fn remove_matching(&self, cmd: &OriginalCommand) -> Result<(), crate::Error>;
   ```

4. Update `FileProbeStore`, `FakeProbeStore`, and all call sites in tests.

5. Write failing test first:

   ```rust
   #[test]
   fn original_command_from_str() {
       let cmd = OriginalCommand::from("gh issue list");
       assert_eq!(cmd.as_str(), "gh issue list");
   }
   ```

6. Verify: `cargo nextest run -p prefixe` → all green.
7. `git commit -m "refactor(prefixe): OriginalCommand newtype for ProbeStore port (#9)"`

---

### Task 3: Separate PrefixConfig domain type from TOML DTO (closes #12)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/domain.rs`, `crates/prefixe/src/infra/toml_store.rs` (new)
**Run**: `cargo nextest run -p prefixe`

1. In `src/domain.rs`, add a clean domain config with no serde:

   ```rust
   use std::collections::HashMap;

   /// Pure domain config — no serialization dependencies.
   #[derive(Debug, Clone, Default)]
   pub struct PrefixConfig {
       pub mappings: HashMap<String, Vec<String>>,
       pub candidate_prefixes: Vec<Vec<String>>,
       pub learn_on_successful_fallback: bool,
   }
   ```

2. Create `src/infra/mod.rs`:

   ```rust
   pub mod toml_store;
   pub mod path;
   ```

3. Create `src/infra/toml_store.rs` with the TOML DTO:

   ```rust
   use std::collections::HashMap;
   use serde::{Deserialize, Serialize};
   use crate::domain::PrefixConfig;

   #[derive(Debug, Clone, Serialize, Deserialize, Default)]
   pub(crate) struct PrefixConfigDto {
       #[serde(default)]
       pub mappings: HashMap<String, Vec<String>>,
       #[serde(default)]
       pub candidate_prefixes: Vec<Vec<String>>,
       #[serde(default)]
       pub learn_on_successful_fallback: bool,
   }

   impl From<PrefixConfigDto> for PrefixConfig {
       fn from(dto: PrefixConfigDto) -> Self {
           Self {
               mappings: dto.mappings,
               candidate_prefixes: dto.candidate_prefixes,
               learn_on_successful_fallback: dto.learn_on_successful_fallback,
           }
       }
   }

   impl From<&PrefixConfig> for PrefixConfigDto {
       fn from(cfg: &PrefixConfig) -> Self {
           Self {
               mappings: cfg.mappings.clone(),
               candidate_prefixes: cfg.candidate_prefixes.clone(),
               learn_on_successful_fallback: cfg.learn_on_successful_fallback,
           }
       }
   }
   ```

4. Remove `#[derive(Serialize, Deserialize)]` from `PrefixConfig` in `lib.rs`; use only
   `PrefixConfigDto` in `FilePrefixStore`.

5. Write failing test:

   ```rust
   #[test]
   fn prefix_config_has_no_serde_dependency() {
       // Compiles only if PrefixConfig does not require serde
       let _c: crate::domain::PrefixConfig = Default::default();
   }
   ```

6. Verify: `cargo nextest run -p prefixe` → green. `cargo clippy -p prefixe -- -D warnings` → clean.
7. `git commit -m "refactor(prefixe): separate PrefixConfig domain type from TOML DTO (#12)"`

---

### Task 4: Move ProbeEntryToml and ProbeFile to infra module (closes #13)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/infra/toml_store.rs`
**Run**: `cargo nextest run -p prefixe`

1. Move `ProbeEntryToml`, `ProbeFile`, and their `From` impls from `lib.rs` into
   `src/infra/toml_store.rs`.

2. Move `FileProbeStore` and `FilePrefixStore` into `src/infra/toml_store.rs` as well,
   since they own the TOML serialization logic.

3. Re-export from `lib.rs`:

   ```rust
   pub use infra::toml_store::{FilePrefixStore, FileProbeStore};
   ```

4. Verify: `cargo nextest run -p prefixe` → green. No compile errors.
5. `git commit -m "refactor(prefixe): move TOML infra types to infra::toml_store module (#13)"`

---

### Task 5: Extract PathResolver port and EnvPathResolver adapter (closes #14)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/infra/path.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

   ```rust
   #[cfg(any(test, feature = "testing"))]
   #[test]
   fn explicit_path_resolver_returns_given_paths() {
       use crate::infra::path::{ExplicitPathResolver, PathResolver};
       let r = ExplicitPathResolver {
           prefix_config: std::path::PathBuf::from("/tmp/prefixes.toml"),
           probe_store: std::path::PathBuf::from("/tmp/candidates.toml"),
       };
       assert_eq!(r.prefix_config_path(), std::path::PathBuf::from("/tmp/prefixes.toml"));
       assert_eq!(r.probe_store_path(), std::path::PathBuf::from("/tmp/candidates.toml"));
   }
   ```

2. Create `src/infra/path.rs`:

   ```rust
   use std::path::PathBuf;

   /// Port: resolves file system paths for prefix config and probe store.
   pub trait PathResolver {
       fn prefix_config_path(&self) -> PathBuf;
       fn probe_store_path(&self) -> PathBuf;
   }

   /// Reads paths from environment variables (production adapter).
   pub struct EnvPathResolver;

   impl PathResolver for EnvPathResolver {
       fn prefix_config_path(&self) -> PathBuf {
           std::env::var_os("CRS_RX_PREFIXES")
               .map(PathBuf::from)
               .unwrap_or_else(|| {
                   let base = std::env::var_os("XDG_CONFIG_HOME")
                       .map(PathBuf::from)
                       .unwrap_or_else(|| {
                           PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                               .join(".config")
                       });
                   base.join("rx").join("prefixes.toml")
               })
       }

       fn probe_store_path(&self) -> PathBuf {
           std::env::var_os("CRS_CTX_DIR")
               .map(PathBuf::from)
               .unwrap_or_else(|| std::path::Path::new(".ctx").to_path_buf())
               .join("candidates.toml")
       }
   }

   /// Explicit paths for testing.
   #[cfg(any(test, feature = "testing"))]
   pub struct ExplicitPathResolver {
       pub prefix_config: PathBuf,
       pub probe_store: PathBuf,
   }

   #[cfg(any(test, feature = "testing"))]
   impl PathResolver for ExplicitPathResolver {
       fn prefix_config_path(&self) -> PathBuf {
           self.prefix_config.clone()
       }
       fn probe_store_path(&self) -> PathBuf {
           self.probe_store.clone()
       }
   }
   ```

3. Remove `default_path()` from `FilePrefixStore` and `FileProbeStore` — path is now
   injected via `PathResolver`. Update constructors:

   ```rust
   impl FilePrefixStore {
       pub fn new(path: std::path::PathBuf) -> Self { Self { path } }
       pub fn from_resolver(r: &dyn PathResolver) -> Self {
           Self::new(r.prefix_config_path())
       }
   }
   ```

4. Verify: `cargo nextest run -p prefixe` → green.
5. `git commit -m "refactor(prefixe): PathResolver port + EnvPathResolver adapter (#14)"`

---

### Task 6: Introduce CommandSplitter trait (closes #17)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/domain.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

   ```rust
   #[test]
   fn textual_splitter_splits_pipeline() {
       use crate::domain::{CommandSplitter, TextualSplitter, Segment};
       let s = TextualSplitter;
       let segs = s.split("a | b");
       assert_eq!(segs.len(), 2);
       assert_eq!(segs[0].sep.as_deref(), Some("|"));
       let rejoined = s.rejoin(&segs);
       assert_eq!(rejoined, "a | b");
   }
   ```

2. Add to `src/domain.rs`:

   ```rust
   /// Port: strategy for splitting and rejoining compound shell commands.
   pub trait CommandSplitter {
       fn split(&self, cmd: &str) -> Vec<crate::Segment>;
       fn rejoin(&self, segs: &[crate::Segment]) -> String;
   }

   /// Default textual splitter (does not handle quoted separators).
   pub struct TextualSplitter;

   impl CommandSplitter for TextualSplitter {
       fn split(&self, cmd: &str) -> Vec<crate::Segment> {
           crate::split_segments(cmd)
       }
       fn rejoin(&self, segs: &[crate::Segment]) -> String {
           crate::rejoin(segs)
       }
   }
   ```

3. Re-export `CommandSplitter` and `TextualSplitter` from `lib.rs`.

4. Verify: `cargo nextest run -p prefixe` → green.
5. `git commit -m "feat(prefixe): CommandSplitter trait with TextualSplitter default (#17)"`

---

### Task 7: Introduce CommandRewriter trait (closes #16)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/domain.rs`, `crates/prefixe/src/engine.rs` (new)
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

   ```rust
   #[test]
   fn command_rewriter_trait_is_mockable() {
       use crate::{CommandRewriter, RewriteResult};
       struct NoOpRewriter;
       impl CommandRewriter for NoOpRewriter {
           fn rewrite(&self, cmd: &str) -> RewriteResult {
               RewriteResult { rewritten: cmd.to_string(), probes: vec![] }
           }
       }
       let r = NoOpRewriter;
       assert_eq!(r.rewrite("echo hi").rewritten, "echo hi");
   }
   ```

2. Add to `src/domain.rs`:

   ```rust
   use crate::RewriteResult;

   /// Port: strategy for rewriting a shell command with prefix injection.
   pub trait CommandRewriter {
       fn rewrite(&self, cmd: &str) -> RewriteResult;
   }
   ```

3. Re-export from `lib.rs`.

4. Add `FakeRewriter` to `testing` module:

   ```rust
   pub struct FakeRewriter {
       pub result: RewriteResult,
   }
   impl CommandRewriter for FakeRewriter {
       fn rewrite(&self, _cmd: &str) -> RewriteResult {
           self.result.clone()
       }
   }
   ```

5. Verify: `cargo nextest run -p prefixe` → green.
6. `git commit -m "feat(prefixe): CommandRewriter trait as rewrite strategy port (#16)"`

---

### Task 8: Introduce PrefixEngine service (closes #11)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/engine.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

   ```rust
   #[test]
   fn prefix_engine_rewrite_confirmed() {
       use crate::{PrefixConfig, PrefixEngine};
       use crate::testing::{FakePrefixStore, FakeProbeStore};
       use std::collections::HashMap;

       let store = FakePrefixStore::new(PrefixConfig {
           mappings: [("gh".to_string(), vec!["op".to_string(), "run".to_string(), "--".to_string()])]
               .into_iter().collect(),
           ..Default::default()
       });
       let probes = FakeProbeStore::empty();
       let engine = PrefixEngine::new(store, probes);
       let r = engine.rewrite("gh issue list");
       assert_eq!(r.rewritten, "op run -- gh issue list");
   }

   #[test]
   fn prefix_engine_audit_returns_sorted_mappings() {
       use crate::{PrefixConfig, PrefixEngine};
       use crate::testing::{FakePrefixStore, FakeProbeStore};

       let store = FakePrefixStore::new(PrefixConfig {
           mappings: [
               ("gh".to_string(), vec!["op".to_string()]),
               ("cargo".to_string(), vec!["dotenvx".to_string()]),
           ].into_iter().collect(),
           ..Default::default()
       });
       let engine = PrefixEngine::new(store, FakeProbeStore::empty());
       let audit = engine.audit();
       assert_eq!(audit.mappings[0].0, "cargo");
       assert_eq!(audit.mappings[1].0, "gh");
   }
   ```

2. Create `src/engine.rs`:

   ```rust
   use crate::{
       AuditState, PrefixStore, ProbeStore, RewriteResult,
       domain::CommandRewriter,
       rewrite_command, audit_state,
   };

   /// Encapsulates PrefixStore + ProbeStore and exposes the full use-case API.
   pub struct PrefixEngine<P: PrefixStore, Q: ProbeStore> {
       prefix_store: P,
       probe_store: Q,
   }

   impl<P: PrefixStore, Q: ProbeStore> PrefixEngine<P, Q> {
       pub fn new(prefix_store: P, probe_store: Q) -> Self {
           Self { prefix_store, probe_store }
       }

       pub fn rewrite(&self, cmd: &str) -> RewriteResult {
           let config = self.prefix_store.load();
           rewrite_command(cmd, &config)
       }

       pub fn audit(&self) -> AuditState {
           audit_state(&self.prefix_store, &self.probe_store)
       }

       pub fn confirm(&self, key: &str, prefix: &[String]) -> Result<(), crate::Error> {
           self.prefix_store.confirm_mapping(key, prefix).map_err(crate::Error::from)
       }

       pub fn forget(&self, key: &str) -> Result<bool, crate::Error> {
           self.prefix_store.remove_mapping(key).map_err(crate::Error::from)
       }
   }

   impl<P: PrefixStore, Q: ProbeStore> CommandRewriter for PrefixEngine<P, Q> {
       fn rewrite(&self, cmd: &str) -> RewriteResult {
           PrefixEngine::rewrite(self, cmd)
       }
   }
   ```

3. Re-export from `lib.rs`:

   ```rust
   pub mod engine;
   pub use engine::PrefixEngine;
   ```

4. Verify: `cargo nextest run -p prefixe` → green.
5. `git commit -m "feat(prefixe): PrefixEngine service encapsulating store composition (#11)"`

---

### Task 9: Wire rewrite_command through PrefixStore port (closes #15)

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/lib.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test verifying the store-based API:

   ```rust
   #[test]
   fn rewrite_via_store_port_confirmed() {
       use crate::testing::FakePrefixStore;
       use crate::{PrefixConfig, rewrite_via_store};
       let store = FakePrefixStore::new(PrefixConfig {
           mappings: [("gh".to_string(), vec!["op".to_string()])]
               .into_iter().collect(),
           ..Default::default()
       });
       let r = rewrite_via_store("gh issue list", &store);
       assert_eq!(r.rewritten, "op gh issue list");
   }
   ```

2. Add `rewrite_via_store` to `lib.rs`:

   ```rust
   /// Rewrite `cmd` using the given store as the prefix source.
   /// Prefer this over `rewrite_command` when you have a `PrefixStore` port.
   pub fn rewrite_via_store(cmd: &str, store: &dyn PrefixStore) -> RewriteResult {
       rewrite_command(cmd, &store.load())
   }
   ```

   Keep `rewrite_command(cmd, &PrefixConfig)` as `pub(crate)` so existing engine code
   still compiles; it is no longer part of the public API.

3. Update all public call sites in tests and docs to use `rewrite_via_store` or
   `PrefixEngine::rewrite`.

4. Verify: `cargo nextest run -p prefixe` → green. `cargo clippy -p prefixe -- -D warnings` → clean.
5. `git commit -m "refactor(prefixe): rewrite_command internal; add rewrite_via_store port API (#15)"`

---

### Task 10: Final validation and version bump

**Run**: `cargo nextest run -p prefixe`

1. Run full suite: `cargo nextest run -p prefixe` → all green.
2. Run: `cargo clippy -p prefixe -- -D warnings` → zero warnings.
3. Run: `cargo fmt --all --check` → no diff.
4. Bump version to `0.3.0` in `Cargo.toml` (breaking: trait signatures changed).
5. Commit: `git commit -m "chore: bump prefixe to 0.3.0 after hexagonal refactor"`
6. Push: `git push`
7. Publish: `cargo publish --manifest-path crates/prefixe/Cargo.toml`
8. Close GH issues #9–#17.
