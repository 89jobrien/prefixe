# Plan: Reactive Candidate Prefix Learning

## Goal

Replace the broken proactive `candidate_prefixes` fallback (which applied to every unknown
command) with a reactive learning loop: confirmed mappings rewrite silently, candidates are
tried only after a command fails bare, and every first confirmation notifies Claude.

## Architecture

- **Crates affected**: `prefixe`, `prefixe-cli`
- **New types**: `CandidatePrefix`, `SuccessPredicate`, `ProbeState`, `PrefixStats`,
  `GlobalStats`, `PrefixCounters`, `CommandStats`, `StatsStore`
- **Removed**: `PrefixMatch::Candidate`, `learn_on_successful_fallback`,
  `RewriteResult.probes`, `probes` field from `AuditState`
- **Data flow**:
  - pre-hook: confirmed mapping → rewrite silently | Pending probe match → transition to Probing
  - post-hook (failure): no probe → write Pending, emit systemMessage
  - post-hook (Probing, predicate passes): confirm mapping, update stats, notify Claude
  - post-hook (Probing, predicate fails): cycle to next candidate or ask Claude

## Tech Stack

- Rust edition 2024
- New workspace dep: `serde_json = "1"` (for hook JSON payloads in `prefixe-cli`)
- New workspace dep: `regex = "1"` (for `SuccessPredicate` evaluation in `prefixe-cli`)
- No new deps in `prefixe` library

## Tasks

---

### Task 1: `SuccessPredicate`, `CandidatePrefix`, update `PrefixConfig`

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/domain.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
// in crates/prefixe/src/domain.rs, bottom of file in #[cfg(test)] mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_prefix_has_success_predicate() {
        let c = CandidatePrefix {
            prefix: vec!["op".to_string(), "plugin".to_string(), "run".to_string(), "--".to_string()],
            success_when: SuccessPredicate::exit_zero(),
        };
        assert_eq!(c.success_when.exit_code, Some(0));
        assert!(!c.success_when.stderr_absent);
    }

    #[test]
    fn prefix_config_uses_candidate_prefix_vec() {
        let config = PrefixConfig {
            mappings: HashMap::new(),
            candidate_prefixes: vec![CandidatePrefix {
                prefix: vec!["op".to_string()],
                success_when: SuccessPredicate::default(),
            }],
        };
        assert_eq!(config.candidate_prefixes.len(), 1);
    }
}
```

Run: `cargo nextest run -p prefixe -- candidate_prefix`
Expected: FAIL (types don't exist yet)

2. Implement — replace `PrefixConfig` and add new types in `domain.rs`:

```rust
/// Per-candidate success predicate. Default: exit code 0, everything else permissive.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SuccessPredicate {
    /// Required exit code. `None` = any. Default: `Some(0)`.
    pub exit_code: Option<i32>,
    /// Regex stdout must match. `None` = not checked.
    pub stdout_matches: Option<String>,
    /// Regex stderr must match. `None` = not checked.
    pub stderr_matches: Option<String>,
    /// If `true`, stderr must be empty or absent.
    pub stderr_absent: bool,
}

impl SuccessPredicate {
    pub fn exit_zero() -> Self {
        Self { exit_code: Some(0), ..Default::default() }
    }
}

/// One entry in `candidate_prefixes`: the tokens to prepend and the predicate
/// used to decide whether the attempt succeeded.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidatePrefix {
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicate,
}

/// Pure domain config — no serialization dependencies.
#[derive(Debug, Clone, Default)]
pub struct PrefixConfig {
    pub mappings: HashMap<String, Vec<String>>,
    /// Ordered list of candidate prefixes to try when a command fails bare.
    pub candidate_prefixes: Vec<CandidatePrefix>,
}
```

Remove `learn_on_successful_fallback` from `PrefixConfig` entirely.

Update the doc-comment example for `PrefixConfig`:

````rust
/// # Examples
///
/// ```
/// use prefixe::{CandidatePrefix, PrefixConfig, SuccessPredicate};
///
/// let config = PrefixConfig {
///     mappings: [("gh".to_string(), vec!["op".to_string(), "plugin".to_string(),
///                "run".to_string(), "--".to_string()])]
///         .into_iter()
///         .collect(),
///     candidate_prefixes: vec![CandidatePrefix {
///         prefix: vec!["op".to_string(), "plugin".to_string(), "run".to_string(),
///                      "--".to_string()],
///         success_when: SuccessPredicate::exit_zero(),
///     }],
/// };
/// assert!(config.mappings.contains_key("gh"));
/// assert_eq!(config.candidate_prefixes.len(), 1);
/// ```
````

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe): add CandidatePrefix, SuccessPredicate; update PrefixConfig"`

---

### Task 2: `ProbeState` enum + update `ProbeEntry`

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/lib.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test (add to `#[cfg(test)] mod tests` in `lib.rs`):

```rust
#[test]
fn probe_entry_has_state_and_candidate_index() {
    let entry = ProbeEntry {
        key: "gh".to_string(),
        prefix: vec!["op".to_string()],
        success_when: SuccessPredicate::exit_zero(),
        original_command: OriginalCommand::from("gh issue list"),
        state: ProbeState::Pending,
        candidate_index: 0,
    };
    assert_eq!(entry.state, ProbeState::Pending);
    assert_eq!(entry.candidate_index, 0);
}

#[test]
fn probe_state_probing_is_distinct_from_pending() {
    assert_ne!(ProbeState::Pending, ProbeState::Probing);
}
```

Run: `cargo nextest run -p prefixe -- probe_entry_has_state`
Expected: FAIL

2. Implement — add `ProbeState` and update `ProbeEntry` in `lib.rs`:

```rust
/// Lifecycle state of a candidate probe.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    /// Command failed bare; waiting for Claude to retry with this candidate.
    Pending,
    /// Pre-hook rewrote the retry; waiting on the post-hook exit code.
    Probing,
}

/// A pending or active candidate probe.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeEntry {
    /// Leading command word (e.g. `"gh"`).
    pub key: String,
    /// Candidate prefix tokens being tried.
    pub prefix: Vec<String>,
    /// Predicate used to judge whether the attempt succeeded.
    pub success_when: SuccessPredicate,
    /// Original command that failed bare.
    pub original_command: OriginalCommand,
    /// Current lifecycle state.
    pub state: ProbeState,
    /// Index into `config.candidate_prefixes` being tried.
    pub candidate_index: usize,
}
```

Export `ProbeState` in the public API (add to `pub use` block at top of `lib.rs`):

```rust
pub use domain::{
    CandidatePrefix, CommandRewriter, CommandSplitter, OriginalCommand, PrefixConfig,
    PrefixRule, RuleCondition, SuccessPredicate, TextualSplitter,
};
```

Also export `ProbeState`:

```rust
pub use self::ProbeState;  // add to existing pub items
```

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe): add ProbeState enum, extend ProbeEntry with state/candidate_index/success_when"`

---

### Task 3: Remove candidate fallback from `lookup_prefix`; remove `PrefixMatch::Candidate`

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/lib.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
#[test]
fn lookup_prefix_returns_none_for_unknown_command_even_with_candidates() {
    // candidate_prefixes no longer causes lookup_prefix to return Candidate
    let config = PrefixConfig {
        mappings: HashMap::new(),
        candidate_prefixes: vec![CandidatePrefix {
            prefix: vec!["op".to_string()],
            success_when: SuccessPredicate::exit_zero(),
        }],
    };
    assert_eq!(lookup_prefix("grep foo .", &config), None);
}
```

Run: `cargo nextest run -p prefixe -- lookup_prefix_returns_none_for_unknown`
Expected: FAIL (currently returns `Some(PrefixMatch::Candidate { .. })`)

2. Implement:

Remove `PrefixMatch::Candidate` variant. `PrefixMatch` becomes:

```rust
/// Result of a confirmed prefix lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixMatch {
    pub key: String,
    pub prefix: Vec<String>,
}
```

Update `lookup_prefix` — remove the candidate fallback block entirely:

```rust
pub fn lookup_prefix(segment: &str, config: &PrefixConfig) -> Option<PrefixMatch> {
    let trimmed = segment.trim();
    if trimmed.contains("$(") || trimmed.contains('`') {
        return None;
    }

    let tokens = shell_words::split(trimmed).ok()?;
    let first = tokens.first()?.as_str();
    let second = tokens.get(1).map(|s| s.as_str());

    if let Some(second) = second {
        let two_word = format!("{first} {second}");
        if let Some(prefix) = config.mappings.get(&two_word) {
            return Some(PrefixMatch { key: two_word, prefix: prefix.clone() });
        }
    }

    if let Some(prefix) = config.mappings.get(first) {
        return Some(PrefixMatch { key: first.to_string(), prefix: prefix.clone() });
    }

    None
}
```

Update `testing::FakePrefixStore` to use the new `PrefixConfig` (no `learn_on_successful_fallback`).

Fix all tests that reference `PrefixMatch::Confirmed { key, prefix }` → `PrefixMatch { key, prefix }`.
Fix all tests that reference `PrefixMatch::Candidate` → delete them (now tested by the new test above).

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "refactor(prefixe): remove PrefixMatch::Candidate; lookup_prefix never returns candidate fallback"`

---

### Task 4: Remove `probes` from `RewriteResult`; simplify `rewrite_command`

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/lib.rs`, `crates/prefixe/tests/property_tests.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
#[test]
fn rewrite_result_has_no_probes_field() {
    // RewriteResult is now just { rewritten: String }
    let r = RewriteResult { rewritten: "echo hi".to_string() };
    assert_eq!(r.rewritten, "echo hi");
}
```

Run: `cargo nextest run -p prefixe -- rewrite_result_has_no_probes`
Expected: FAIL (struct has `probes` field)

2. Implement:

Replace `RewriteResult`:

```rust
/// Result of rewriting a full command string.
#[derive(Debug, Clone)]
pub struct RewriteResult {
    pub rewritten: String,
}
```

Simplify `rewrite_command`:

```rust
pub fn rewrite_command(cmd: &str, config: &PrefixConfig) -> RewriteResult {
    let mut segs = split_segments(cmd);

    for seg in &mut segs {
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(m) = lookup_prefix(trimmed, config) else {
            continue;
        };
        let leading_len = seg.text.len() - seg.text.trim_start().len();
        let leading = &seg.text[..leading_len];
        let trailing_start = leading_len + trimmed.len();
        let trailing = &seg.text[trailing_start..];
        seg.text = format!("{leading}{} {trimmed}{trailing}", m.prefix.join(" "));
    }

    RewriteResult { rewritten: rejoin(&segs) }
}
```

In `tests/property_tests.rs`, update `EchoRewriter`:

```rust
impl CommandRewriter for EchoRewriter {
    fn rewrite(&self, _cmd: &str) -> RewriteResult {
        RewriteResult { rewritten: self.fixed_output.clone() }
    }
}
```

Remove `AuditState.probes` field and update `audit_state`:

```rust
#[derive(Debug, Clone, Default)]
pub struct AuditState {
    pub mappings: Vec<(String, Vec<String>)>,
}

pub fn audit_state(prefix_store: &dyn PrefixStore) -> AuditState {
    let config = prefix_store.load();
    let mut mappings: Vec<(String, Vec<String>)> = config.mappings.into_iter().collect();
    mappings.sort_by(|a, b| a.0.cmp(&b.0));
    AuditState { mappings }
}
```

Update `PrefixEngine::audit` signature accordingly.

Fix all remaining compile errors from removing `probes`.

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "refactor(prefixe): remove probes from RewriteResult; simplify rewrite_command and AuditState"`

---

### Task 5: Stats types + `StatsStore` trait

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/lib.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
#[test]
fn prefix_stats_defaults_to_zero() {
    let stats = PrefixStats::default();
    assert_eq!(stats.global.probes_initiated, 0);
    assert_eq!(stats.global.probes_confirmed, 0);
    assert_eq!(stats.global.probes_exhausted, 0);
    assert!(stats.by_prefix.is_empty());
    assert!(stats.by_command.is_empty());
}

#[test]
fn prefix_counters_increment() {
    let mut counters = PrefixCounters::default();
    counters.tried += 1;
    counters.confirmed += 1;
    assert_eq!(counters.tried, 1);
    assert_eq!(counters.failed, 0);
}
```

Run: `cargo nextest run -p prefixe -- prefix_stats_defaults`
Expected: FAIL

2. Implement — add to `lib.rs`:

```rust
/// Running counters for a specific candidate prefix.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrefixCounters {
    pub tried: u64,
    pub confirmed: u64,
    pub failed: u64,
}

/// Running counters for a specific command key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandStats {
    pub probes_initiated: u64,
    /// Prefix tokens joined by `" "` when a mapping was confirmed.
    pub confirmed_prefix: Option<String>,
    /// ISO 8601 timestamp of confirmation.
    pub confirmed_at: Option<String>,
}

/// Global running totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GlobalStats {
    pub probes_initiated: u64,
    pub probes_confirmed: u64,
    pub probes_exhausted: u64,
}

/// Full stats snapshot.
#[derive(Debug, Clone, Default)]
pub struct PrefixStats {
    pub global: GlobalStats,
    /// Key: prefix tokens joined by `" "` (e.g. `"op plugin run --"`).
    pub by_prefix: HashMap<String, PrefixCounters>,
    /// Key: command word (e.g. `"gh"`).
    pub by_command: HashMap<String, CommandStats>,
}

/// Port for reading and writing prefix learning stats.
pub trait StatsStore {
    fn load(&self) -> PrefixStats;
    fn save(&self, stats: &PrefixStats) -> Result<(), Error>;
}
```

Add `in-memory test double` in `testing` module:

```rust
pub struct FakeStatsStore {
    pub stats: std::cell::RefCell<PrefixStats>,
}

impl FakeStatsStore {
    pub fn new() -> Self {
        Self { stats: std::cell::RefCell::new(PrefixStats::default()) }
    }
}

impl StatsStore for FakeStatsStore {
    fn load(&self) -> PrefixStats {
        self.stats.borrow().clone()
    }
    fn save(&self, stats: &PrefixStats) -> Result<(), Error> {
        *self.stats.borrow_mut() = stats.clone();
        Ok(())
    }
}
```

Export new types from `lib.rs`:

```rust
pub use self::{
    CommandStats, GlobalStats, PrefixCounters, PrefixStats, ProbeState, StatsStore,
};
```

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe): add PrefixStats, GlobalStats, PrefixCounters, CommandStats, StatsStore trait"`

---

### Task 6: Update TOML DTOs for `PrefixConfig`, `CandidatePrefix`, `ProbeEntry`

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/infra/toml_store.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
// in toml_store.rs tests
#[test]
fn prefix_config_dto_round_trips_candidate_prefix() {
    let dir = TempDir::new().unwrap();
    let store = FilePrefixStore::new(dir.path().join("prefixes.toml"));
    // Force a candidate_prefix through a raw TOML write then load
    std::fs::write(
        dir.path().join("prefixes.toml"),
        r#"
[[candidate_prefixes]]
prefix = ["op", "plugin", "run", "--"]

[candidate_prefixes.success_when]
exit_code = 0
stderr_absent = true
"#,
    ).unwrap();
    let config = store.load();
    assert_eq!(config.candidate_prefixes.len(), 1);
    assert_eq!(config.candidate_prefixes[0].prefix, vec!["op", "plugin", "run", "--"]);
    assert_eq!(config.candidate_prefixes[0].success_when.exit_code, Some(0));
    assert!(config.candidate_prefixes[0].success_when.stderr_absent);
}

#[test]
fn probe_entry_toml_round_trips_state_and_index() {
    let dir = TempDir::new().unwrap();
    let store = FileProbeStore::new(dir.path().join("candidates.toml"));
    let entries = vec![ProbeEntry {
        key: "gh".to_string(),
        prefix: vec!["op".to_string()],
        success_when: SuccessPredicate::exit_zero(),
        original_command: OriginalCommand::from("gh issue list"),
        state: ProbeState::Pending,
        candidate_index: 0,
    }];
    store.write(&entries).unwrap();
    let loaded = store.load();
    assert_eq!(loaded[0].state, ProbeState::Pending);
    assert_eq!(loaded[0].candidate_index, 0);
}
```

Run: `cargo nextest run -p prefixe -- prefix_config_dto_round_trips`
Expected: FAIL

2. Implement — replace DTOs in `toml_store.rs`:

```rust
use crate::domain::{CandidatePrefix, SuccessPredicate};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SuccessPredicateDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_matches: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_matches: Option<String>,
    #[serde(default)]
    pub stderr_absent: bool,
}

impl From<SuccessPredicateDto> for SuccessPredicate {
    fn from(d: SuccessPredicateDto) -> Self {
        Self {
            exit_code: d.exit_code,
            stdout_matches: d.stdout_matches,
            stderr_matches: d.stderr_matches,
            stderr_absent: d.stderr_absent,
        }
    }
}

impl From<&SuccessPredicate> for SuccessPredicateDto {
    fn from(p: &SuccessPredicate) -> Self {
        Self {
            exit_code: p.exit_code,
            stdout_matches: p.stdout_matches.clone(),
            stderr_matches: p.stderr_matches.clone(),
            stderr_absent: p.stderr_absent,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CandidatePrefixDto {
    pub prefix: Vec<String>,
    #[serde(default)]
    pub success_when: SuccessPredicateDto,
}

impl From<CandidatePrefixDto> for CandidatePrefix {
    fn from(d: CandidatePrefixDto) -> Self {
        Self { prefix: d.prefix, success_when: d.success_when.into() }
    }
}

impl From<&CandidatePrefix> for CandidatePrefixDto {
    fn from(c: &CandidatePrefix) -> Self {
        Self { prefix: c.prefix.clone(), success_when: (&c.success_when).into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PrefixConfigDto {
    #[serde(default)]
    pub mappings: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub candidate_prefixes: Vec<CandidatePrefixDto>,
}

impl From<PrefixConfigDto> for PrefixConfig {
    fn from(dto: PrefixConfigDto) -> Self {
        Self {
            mappings: dto.mappings,
            candidate_prefixes: dto.candidate_prefixes.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<&PrefixConfig> for PrefixConfigDto {
    fn from(cfg: &PrefixConfig) -> Self {
        Self {
            mappings: cfg.mappings.clone(),
            candidate_prefixes: cfg.candidate_prefixes.iter().map(Into::into).collect(),
        }
    }
}
```

Update `ProbeEntryToml` for new fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProbeEntryToml {
    pub key: String,
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicateDto,
    pub original_command: String,
    pub state: String,          // "pending" | "probing"
    pub candidate_index: usize,
}

impl From<&ProbeEntry> for ProbeEntryToml {
    fn from(e: &ProbeEntry) -> Self {
        Self {
            key: e.key.clone(),
            prefix: e.prefix.clone(),
            success_when: (&e.success_when).into(),
            original_command: e.original_command.0.clone(),
            state: match e.state {
                ProbeState::Pending => "pending".to_string(),
                ProbeState::Probing => "probing".to_string(),
            },
            candidate_index: e.candidate_index,
        }
    }
}

impl From<ProbeEntryToml> for ProbeEntry {
    fn from(t: ProbeEntryToml) -> Self {
        Self {
            key: t.key,
            prefix: t.prefix,
            success_when: t.success_when.into(),
            original_command: OriginalCommand(t.original_command),
            state: if t.state == "probing" { ProbeState::Probing } else { ProbeState::Pending },
            candidate_index: t.candidate_index,
        }
    }
}
```

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe): update TOML DTOs for CandidatePrefix, SuccessPredicate, ProbeEntry state"`

---

### Task 7: `FileStatsStore` TOML adapter

**Crate**: `prefixe`
**File(s)**: `crates/prefixe/src/infra/toml_store.rs`
**Run**: `cargo nextest run -p prefixe`

1. Write failing test:

```rust
#[test]
fn file_stats_store_round_trips() {
    let dir = TempDir::new().unwrap();
    let store = FileStatsStore::new(dir.path().join("stats.toml"));

    let mut stats = PrefixStats::default();
    stats.global.probes_initiated = 5;
    stats.global.probes_confirmed = 2;
    stats.by_prefix.insert("op plugin run --".to_string(), PrefixCounters {
        tried: 3, confirmed: 2, failed: 1,
    });
    stats.by_command.insert("gh".to_string(), CommandStats {
        probes_initiated: 3,
        confirmed_prefix: Some("op plugin run --".to_string()),
        confirmed_at: Some("2026-05-16T10:00:00Z".to_string()),
    });

    store.save(&stats).unwrap();
    let loaded = store.load();

    assert_eq!(loaded.global.probes_initiated, 5);
    assert_eq!(loaded.global.probes_confirmed, 2);
    assert_eq!(loaded.by_prefix["op plugin run --"].tried, 3);
    assert_eq!(loaded.by_command["gh"].confirmed_prefix.as_deref(), Some("op plugin run --"));
}

#[test]
fn file_stats_store_missing_file_returns_default() {
    let store = FileStatsStore::new(std::path::PathBuf::from("/tmp/nonexistent-stats-abc.toml"));
    let stats = store.load();
    assert_eq!(stats.global.probes_initiated, 0);
}
```

Run: `cargo nextest run -p prefixe -- file_stats_store_round_trips`
Expected: FAIL

2. Implement — add DTOs and `FileStatsStore` to `toml_store.rs`:

```rust
use crate::{CommandStats, GlobalStats, PrefixCounters, PrefixStats, StatsStore};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GlobalStatsDto {
    #[serde(default)] pub probes_initiated: u64,
    #[serde(default)] pub probes_confirmed: u64,
    #[serde(default)] pub probes_exhausted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PrefixCountersDto {
    #[serde(default)] pub tried: u64,
    #[serde(default)] pub confirmed: u64,
    #[serde(default)] pub failed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CommandStatsDto {
    #[serde(default)] pub probes_initiated: u64,
    #[serde(skip_serializing_if = "Option::is_none")] pub confirmed_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub confirmed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PrefixStatsDto {
    #[serde(default)] pub global: GlobalStatsDto,
    #[serde(default)] pub by_prefix: HashMap<String, PrefixCountersDto>,
    #[serde(default)] pub by_command: HashMap<String, CommandStatsDto>,
}

impl From<PrefixStatsDto> for PrefixStats {
    fn from(d: PrefixStatsDto) -> Self {
        Self {
            global: GlobalStats {
                probes_initiated: d.global.probes_initiated,
                probes_confirmed: d.global.probes_confirmed,
                probes_exhausted: d.global.probes_exhausted,
            },
            by_prefix: d.by_prefix.into_iter().map(|(k, v)| (k, PrefixCounters {
                tried: v.tried, confirmed: v.confirmed, failed: v.failed,
            })).collect(),
            by_command: d.by_command.into_iter().map(|(k, v)| (k, CommandStats {
                probes_initiated: v.probes_initiated,
                confirmed_prefix: v.confirmed_prefix,
                confirmed_at: v.confirmed_at,
            })).collect(),
        }
    }
}

impl From<&PrefixStats> for PrefixStatsDto {
    fn from(s: &PrefixStats) -> Self {
        Self {
            global: GlobalStatsDto {
                probes_initiated: s.global.probes_initiated,
                probes_confirmed: s.global.probes_confirmed,
                probes_exhausted: s.global.probes_exhausted,
            },
            by_prefix: s.by_prefix.iter().map(|(k, v)| (k.clone(), PrefixCountersDto {
                tried: v.tried, confirmed: v.confirmed, failed: v.failed,
            })).collect(),
            by_command: s.by_command.iter().map(|(k, v)| (k.clone(), CommandStatsDto {
                probes_initiated: v.probes_initiated,
                confirmed_prefix: v.confirmed_prefix.clone(),
                confirmed_at: v.confirmed_at.clone(),
            })).collect(),
        }
    }
}

/// File-backed stats store using TOML.
pub struct FileStatsStore {
    pub path: PathBuf,
}

impl FileStatsStore {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    pub fn default_path() -> PathBuf {
        std::env::var_os("CRS_RX_STATS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                    });
                base.join("rx").join("prefix-stats.toml")
            })
    }
}

impl StatsStore for FileStatsStore {
    fn load(&self) -> PrefixStats {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return PrefixStats::default();
        };
        toml::from_str::<PrefixStatsDto>(&content)
            .unwrap_or_default()
            .into()
    }

    fn save(&self, stats: &PrefixStats) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let dto = PrefixStatsDto::from(stats);
        let serialized = toml::to_string_pretty(&dto)?;
        std::fs::write(&self.path, serialized)?;
        Ok(())
    }
}
```

Export `FileStatsStore` from `lib.rs`:

```rust
pub use infra::toml_store::{FilePrefixStore, FileProbeStore, FileStatsStore};
```

3. Verify:

```
cargo nextest run -p prefixe               → all green
cargo clippy -p prefixe -- -D warnings     → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe): add FileStatsStore TOML adapter for PrefixStats"`

---

### Task 8: `prefixe-cli` deps + payload types + `evaluate_predicate`

**Crate**: `prefixe-cli`
**File(s)**: `crates/prefixe-cli/Cargo.toml`, `crates/prefixe-cli/src/payload.rs` (new),
`crates/prefixe-cli/src/predicate.rs` (new)
**Run**: `cargo nextest run -p prefixe-cli`

1. Write failing test (add to a new `crates/prefixe-cli/src/predicate.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::SuccessPredicate;

    #[test]
    fn evaluate_exit_zero_passes() {
        let pred = SuccessPredicate::exit_zero();
        assert!(evaluate_predicate(&pred, 0, "", ""));
    }

    #[test]
    fn evaluate_exit_zero_fails_on_nonzero() {
        let pred = SuccessPredicate::exit_zero();
        assert!(!evaluate_predicate(&pred, 1, "", ""));
    }

    #[test]
    fn evaluate_stderr_absent_fails_when_stderr_present() {
        let pred = SuccessPredicate { exit_code: Some(0), stderr_absent: true, ..Default::default() };
        assert!(!evaluate_predicate(&pred, 0, "", "some error"));
    }

    #[test]
    fn evaluate_stdout_matches_regex() {
        let pred = SuccessPredicate {
            exit_code: Some(0),
            stdout_matches: Some(r"issue \d+".to_string()),
            ..Default::default()
        };
        assert!(evaluate_predicate(&pred, 0, "issue 42 opened", ""));
        assert!(!evaluate_predicate(&pred, 0, "no match here", ""));
    }

    #[test]
    fn evaluate_stderr_matches_regex() {
        let pred = SuccessPredicate {
            exit_code: Some(0),
            stderr_matches: Some(r"^$".to_string()),
            ..Default::default()
        };
        assert!(evaluate_predicate(&pred, 0, "", ""));
        assert!(!evaluate_predicate(&pred, 0, "", "error occurred"));
    }
}
```

Run: `cargo nextest run -p prefixe-cli -- evaluate_exit_zero`
Expected: FAIL (module doesn't exist)

2. Add deps to `crates/prefixe-cli/Cargo.toml`:

```toml
[dependencies]
prefixe = { path = "../prefixe" }
toml = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
regex = { workspace = true }
```

Add to workspace `Cargo.toml` `[workspace.dependencies]`:

```toml
serde_json = "1"
regex = "1"
```

Create `crates/prefixe-cli/src/payload.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Subset of the Claude Code hook JSON payload used by both pre- and post-hooks.
#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub tool_name: Option<String>,
    pub tool_input: Option<ToolInput>,
    pub tool_response: Option<ToolResponse>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolInput {
    pub command: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolResponse {
    pub exit_code: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// Output from a post-hook that wants to send a system message to Claude.
#[derive(Debug, Serialize)]
pub struct PostHookOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

impl PostHookOutput {
    pub fn allow_with_message(msg: impl Into<String>) -> Self {
        Self { decision: Some("allow".to_string()), system_message: Some(msg.into()) }
    }

    pub fn silent() -> Self {
        Self { decision: Some("allow".to_string()), system_message: None }
    }
}
```

Create `crates/prefixe-cli/src/predicate.rs`:

```rust
use prefixe::SuccessPredicate;
use regex::Regex;

/// Evaluate a `SuccessPredicate` against the result of running a command.
///
/// Returns `true` if all configured conditions pass.
pub fn evaluate_predicate(pred: &SuccessPredicate, exit_code: i64, stdout: &str, stderr: &str) -> bool {
    if let Some(required) = pred.exit_code {
        if exit_code != required as i64 {
            return false;
        }
    }
    if pred.stderr_absent && !stderr.trim().is_empty() {
        return false;
    }
    if let Some(pattern) = &pred.stdout_matches {
        match Regex::new(pattern) {
            Ok(re) => if !re.is_match(stdout) { return false; },
            Err(_) => return false,
        }
    }
    if let Some(pattern) = &pred.stderr_matches {
        match Regex::new(pattern) {
            Ok(re) => if !re.is_match(stderr) { return false; },
            Err(_) => return false,
        }
    }
    true
}
```

Add to `crates/prefixe-cli/src/main.rs`:

```rust
mod payload;
mod predicate;
```

3. Verify:

```
cargo nextest run -p prefixe-cli           → all green
cargo clippy -p prefixe-cli -- -D warnings → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe-cli): add payload types, evaluate_predicate, serde_json/regex deps"`

---

### Task 9: `prefixe-cli` post-hook — failure path (Pending probe + systemMessage)

**Crate**: `prefixe-cli`
**File(s)**: `crates/prefixe-cli/src/post_hook.rs` (new), `crates/prefixe-cli/src/main.rs`
**Run**: `cargo nextest run -p prefixe-cli`

1. Write failing test (in `post_hook.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::{CandidatePrefix, PrefixConfig, SuccessPredicate};
    use prefixe::testing::{FakePrefixStore, FakeProbeStore, FakeStatsStore};

    fn config_with_one_candidate() -> PrefixConfig {
        PrefixConfig {
            mappings: Default::default(),
            candidate_prefixes: vec![CandidatePrefix {
                prefix: vec!["op".to_string(), "plugin".to_string(),
                             "run".to_string(), "--".to_string()],
                success_when: SuccessPredicate::exit_zero(),
            }],
        }
    }

    #[test]
    fn on_bare_failure_writes_pending_probe_and_returns_message() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::empty();
        let stats_store = FakeStatsStore::new();

        let result = handle_failure(
            "gh issue list",
            &prefix_store,
            &probe_store,
            &stats_store,
        );

        let probes = probe_store.load();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].state, ProbeState::Pending);
        assert_eq!(probes[0].candidate_index, 0);
        assert_eq!(probes[0].key, "gh");
        assert!(result.system_message.as_deref()
            .unwrap_or("").contains("op plugin run -- gh issue list"));

        let stats = stats_store.load();
        assert_eq!(stats.global.probes_initiated, 1);
        assert_eq!(stats.by_command["gh"].probes_initiated, 1);
    }

    #[test]
    fn on_failure_with_existing_pending_probe_does_not_duplicate() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec!["op".to_string()],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Pending,
            candidate_index: 0,
        }]);
        let stats_store = FakeStatsStore::new();

        handle_failure("gh issue list", &prefix_store, &probe_store, &stats_store);

        assert_eq!(probe_store.load().len(), 1); // no duplicate
    }

    #[test]
    fn on_failure_with_no_candidates_returns_silent() {
        let prefix_store = FakePrefixStore::new(PrefixConfig::default());
        let probe_store = FakeProbeStore::empty();
        let stats_store = FakeStatsStore::new();

        let result = handle_failure("grep foo .", &prefix_store, &probe_store, &stats_store);
        assert!(result.system_message.is_none());
    }
}
```

Run: `cargo nextest run -p prefixe-cli -- on_bare_failure`
Expected: FAIL

2. Implement `crates/prefixe-cli/src/post_hook.rs`:

```rust
use prefixe::{
    OriginalCommand, ProbeEntry, ProbeState, ProbeStore, PrefixStore, StatsStore,
};
use crate::payload::PostHookOutput;

/// Extract the leading command word from a shell command string.
fn command_key(cmd: &str) -> String {
    shell_words::split(cmd.trim())
        .ok()
        .and_then(|tokens| tokens.into_iter().next())
        .unwrap_or_else(|| cmd.trim().to_string())
}

/// Handle a bare command failure: write a Pending probe and return a systemMessage
/// telling Claude to retry with the first candidate prefix.
///
/// No-op (returns silent output) if no candidates are configured or a probe already exists.
pub fn handle_failure(
    command: &str,
    prefix_store: &dyn PrefixStore,
    probe_store: &dyn ProbeStore,
    stats_store: &dyn StatsStore,
) -> PostHookOutput {
    let config = prefix_store.load();
    if config.candidate_prefixes.is_empty() {
        return PostHookOutput::silent();
    }

    let key = command_key(command);
    let existing = probe_store.load();

    // Don't duplicate probes for the same command
    if existing.iter().any(|p| p.original_command.as_str() == command) {
        return PostHookOutput::silent();
    }

    let candidate = &config.candidate_prefixes[0];
    let prefixed = format!("{} {}", candidate.prefix.join(" "), command);

    let mut probes = existing;
    probes.push(ProbeEntry {
        key: key.clone(),
        prefix: candidate.prefix.clone(),
        success_when: candidate.success_when.clone(),
        original_command: OriginalCommand::from(command),
        state: ProbeState::Pending,
        candidate_index: 0,
    });
    let _ = probe_store.write(&probes);

    // Update stats
    let mut stats = stats_store.load();
    stats.global.probes_initiated += 1;
    stats.by_command.entry(key).or_default().probes_initiated += 1;
    let _ = stats_store.save(&stats);

    PostHookOutput::allow_with_message(
        format!("Command failed. Retry with: {prefixed}")
    )
}
```

Add to `main.rs`:

```rust
mod post_hook;
```

Add `"post-hook"` subcommand handling in `main()`:

```rust
"post-hook" => {
    let payload: crate::payload::HookPayload = {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap_or(0);
        serde_json::from_str(&input).unwrap_or_else(|_| process::exit(0))
    };
    if payload.tool_name.as_deref() != Some("Bash") {
        process::exit(0);
    }
    let command = match payload.tool_input.as_ref().and_then(|i| i.command.as_deref()) {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => process::exit(0),
    };
    let exit_code = payload.tool_response.as_ref()
        .and_then(|r| r.exit_code)
        .unwrap_or(0);
    if exit_code != 0 {
        let prefix_store = prefixe::FilePrefixStore::new(
            prefixe::FilePrefixStore::default_path()
        );
        let probe_store = prefixe::FileProbeStore::new(
            prefixe::FileProbeStore::default_path()
        );
        let stats_store = prefixe::FileStatsStore::new(
            prefixe::FileStatsStore::default_path()
        );
        let out = post_hook::handle_failure(&command, &prefix_store, &probe_store, &stats_store);
        println!("{}", serde_json::to_string(&out).unwrap_or_default());
    }
}
```

Add `use std::io::Read;` at top of `main.rs`.

3. Verify:

```
cargo nextest run -p prefixe-cli           → all green
cargo clippy -p prefixe-cli -- -D warnings → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe-cli): add post-hook failure path with Pending probe and systemMessage"`

---

### Task 10: `prefixe-cli` pre-hook — detect Pending probe match, transition to Probing

**Crate**: `prefixe-cli`
**File(s)**: `crates/prefixe-cli/src/pre_hook.rs` (new), `crates/prefixe-cli/src/main.rs`
**Run**: `cargo nextest run -p prefixe-cli`

1. Write failing test (in `pre_hook.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::{OriginalCommand, ProbeEntry, ProbeState, SuccessPredicate};
    use prefixe::testing::FakeProbeStore;

    fn pending_probe(original: &str, prefix: &[&str]) -> ProbeEntry {
        ProbeEntry {
            key: original.split_whitespace().next().unwrap_or("").to_string(),
            prefix: prefix.iter().map(|s| s.to_string()).collect(),
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from(original),
            state: ProbeState::Pending,
            candidate_index: 0,
        }
    }

    #[test]
    fn pending_probe_match_transitions_to_probing() {
        let probe_store = FakeProbeStore::with_entries(vec![
            pending_probe("gh issue list", &["op", "plugin", "run", "--"]),
        ]);

        let result = check_probe_match(
            "op plugin run -- gh issue list",
            &probe_store,
        );

        assert!(result, "should have matched pending probe");
        let probes = probe_store.load();
        assert_eq!(probes[0].state, ProbeState::Probing);
    }

    #[test]
    fn non_probe_command_returns_false() {
        let probe_store = FakeProbeStore::empty();
        let result = check_probe_match("gh issue list", &probe_store);
        assert!(!result);
    }

    #[test]
    fn already_probing_entry_not_re_matched() {
        let mut probe = pending_probe("gh issue list", &["op", "plugin", "run", "--"]);
        probe.state = ProbeState::Probing;
        let probe_store = FakeProbeStore::with_entries(vec![probe]);

        // The command matches textually but probe is already Probing
        let result = check_probe_match(
            "op plugin run -- gh issue list",
            &probe_store,
        );
        assert!(!result);
    }
}
```

Run: `cargo nextest run -p prefixe-cli -- pending_probe_match_transitions`
Expected: FAIL

2. Implement `crates/prefixe-cli/src/pre_hook.rs`:

```rust
use prefixe::{ProbeState, ProbeStore};

/// Check if `command` matches a Pending probe's expected retry command.
/// If it does, transitions the probe to `Probing` and returns `true`.
/// Returns `false` if no match (caller should proceed with normal pre-hook logic).
pub fn check_probe_match(command: &str, probe_store: &dyn ProbeStore) -> bool {
    let mut probes = probe_store.load();
    let cmd = command.trim();

    for probe in probes.iter_mut() {
        if probe.state != ProbeState::Pending {
            continue;
        }
        let expected = format!(
            "{} {}",
            probe.prefix.join(" "),
            probe.original_command.as_str()
        );
        if cmd == expected.trim() {
            probe.state = ProbeState::Probing;
            let _ = probe_store.write(&probes);
            return true;
        }
    }
    false
}
```

Update the `"rewrite"` subcommand in `main.rs` to call `check_probe_match` before
the confirmed-mapping rewrite:

```rust
"rewrite" => {
    // ... existing arg parsing ...
    let probe_store = prefixe::FileProbeStore::new(prefixe::FileProbeStore::default_path());

    // If this command is a candidate probe retry, mark it Probing and pass through
    if pre_hook::check_probe_match(&cmd, &probe_store) {
        println!("{cmd}");
        return;
    }

    // Otherwise apply confirmed mappings
    let engine = PrefixEngine::new(store, probe_store);
    let result = engine.rewrite(&cmd);
    println!("{}", result.rewritten);
}
```

Add `mod pre_hook;` to `main.rs`.

3. Verify:

```
cargo nextest run -p prefixe-cli           → all green
cargo clippy -p prefixe-cli -- -D warnings → zero warnings
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe-cli): pre-hook detects Pending probe match, transitions to Probing"`

---

### Task 11: `prefixe-cli` post-hook — Probing resolution with stats

**Crate**: `prefixe-cli`
**File(s)**: `crates/prefixe-cli/src/post_hook.rs`, `crates/prefixe-cli/src/main.rs`
**Run**: `cargo nextest run -p prefixe-cli`

1. Write failing tests (add to `post_hook.rs` tests):

```rust
#[test]
fn probing_success_confirms_mapping_and_notifies() {
    let prefix_store = FakePrefixStore::new(config_with_one_candidate());
    let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
        key: "gh".to_string(),
        prefix: vec!["op".to_string(), "plugin".to_string(),
                     "run".to_string(), "--".to_string()],
        success_when: SuccessPredicate::exit_zero(),
        original_command: OriginalCommand::from("gh issue list"),
        state: ProbeState::Probing,
        candidate_index: 0,
    }]);
    let stats_store = FakeStatsStore::new();

    let result = handle_probe_result(
        "op plugin run -- gh issue list",
        0, "", "",
        &prefix_store, &probe_store, &stats_store,
    );

    // Probe removed
    assert!(probe_store.load().is_empty());
    // Mapping confirmed
    assert!(prefix_store.confirmed.borrow().is_some());
    // Notified
    assert!(result.system_message.as_deref()
        .unwrap_or("").contains("mapping confirmed"));
    // Stats
    let stats = stats_store.load();
    assert_eq!(stats.global.probes_confirmed, 1);
    assert_eq!(stats.by_prefix["op plugin run --"].confirmed, 1);
}

#[test]
fn probing_failure_cycles_to_next_candidate() {
    let config = PrefixConfig {
        mappings: Default::default(),
        candidate_prefixes: vec![
            CandidatePrefix {
                prefix: vec!["op".to_string(), "plugin".to_string(),
                             "run".to_string(), "--".to_string()],
                success_when: SuccessPredicate::exit_zero(),
            },
            CandidatePrefix {
                prefix: vec!["dotenvx".to_string(), "run".to_string(), "--".to_string()],
                success_when: SuccessPredicate::exit_zero(),
            },
        ],
    };
    let prefix_store = FakePrefixStore::new(config);
    let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
        key: "gh".to_string(),
        prefix: vec!["op".to_string(), "plugin".to_string(),
                     "run".to_string(), "--".to_string()],
        success_when: SuccessPredicate::exit_zero(),
        original_command: OriginalCommand::from("gh issue list"),
        state: ProbeState::Probing,
        candidate_index: 0,
    }]);
    let stats_store = FakeStatsStore::new();

    let result = handle_probe_result(
        "op plugin run -- gh issue list",
        1, "", "",  // exit 1 = failure
        &prefix_store, &probe_store, &stats_store,
    );

    // New Pending probe at index 1
    let probes = probe_store.load();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].state, ProbeState::Pending);
    assert_eq!(probes[0].candidate_index, 1);
    assert!(result.system_message.as_deref()
        .unwrap_or("").contains("dotenvx run -- gh issue list"));
}

#[test]
fn probing_failure_exhausted_asks_user() {
    let prefix_store = FakePrefixStore::new(config_with_one_candidate());
    let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
        key: "gh".to_string(),
        prefix: vec!["op".to_string(), "plugin".to_string(),
                     "run".to_string(), "--".to_string()],
        success_when: SuccessPredicate::exit_zero(),
        original_command: OriginalCommand::from("gh issue list"),
        state: ProbeState::Probing,
        candidate_index: 0,
    }]);
    let stats_store = FakeStatsStore::new();

    let result = handle_probe_result(
        "op plugin run -- gh issue list",
        1, "", "",
        &prefix_store, &probe_store, &stats_store,
    );

    assert!(probe_store.load().is_empty()); // probe removed
    assert!(result.system_message.as_deref()
        .unwrap_or("").contains("All candidates failed"));
    let stats = stats_store.load();
    assert_eq!(stats.global.probes_exhausted, 1);
}
```

Run: `cargo nextest run -p prefixe-cli -- probing_success_confirms`
Expected: FAIL

2. Implement — add `handle_probe_result` to `post_hook.rs`:

```rust
use crate::predicate::evaluate_predicate;

/// Handle the result of a Probing command.
///
/// Finds a `Probing` probe whose expected command matches `command`, evaluates
/// the success predicate, then either confirms the mapping, cycles to the next
/// candidate, or exhausts all candidates and asks Claude.
pub fn handle_probe_result(
    command: &str,
    exit_code: i64,
    stdout: &str,
    stderr: &str,
    prefix_store: &dyn PrefixStore,
    probe_store: &dyn ProbeStore,
    stats_store: &dyn StatsStore,
) -> PostHookOutput {
    let mut probes = probe_store.load();
    let cmd = command.trim();

    let probe_idx = probes.iter().position(|p| {
        p.state == ProbeState::Probing
            && format!("{} {}", p.prefix.join(" "), p.original_command.as_str()).trim() == cmd
    });

    let Some(idx) = probe_idx else {
        return PostHookOutput::silent();
    };

    let probe = probes.remove(idx);
    let prefix_key = probe.prefix.join(" ");
    let config = prefix_store.load();

    // Update "tried" counter
    let mut stats = stats_store.load();
    stats.by_prefix.entry(prefix_key.clone()).or_default().tried += 1;

    if evaluate_predicate(&probe.success_when, exit_code, stdout, stderr) {
        // Success: confirm mapping
        let _ = prefix_store.confirm_mapping(&probe.key, &probe.prefix);
        let _ = probe_store.write(&probes);

        stats.global.probes_confirmed += 1;
        stats.by_prefix.entry(prefix_key.clone()).or_default().confirmed += 1;
        let cmd_stats = stats.by_command.entry(probe.key.clone()).or_default();
        cmd_stats.confirmed_prefix = Some(prefix_key);
        cmd_stats.confirmed_at = Some(now_iso8601());
        let _ = stats_store.save(&stats);

        return PostHookOutput::allow_with_message(format!(
            "Candidate `{}` succeeded for `{}` — mapping confirmed.",
            probe.prefix.join(" "),
            probe.original_command.as_str(),
        ));
    }

    // Failure: update failed counter
    stats.by_prefix.entry(prefix_key).or_default().failed += 1;

    let next_index = probe.candidate_index + 1;
    if next_index < config.candidate_prefixes.len() {
        // Cycle to next candidate
        let next = &config.candidate_prefixes[next_index];
        let prefixed = format!("{} {}", next.prefix.join(" "), probe.original_command.as_str());

        probes.push(ProbeEntry {
            key: probe.key,
            prefix: next.prefix.clone(),
            success_when: next.success_when.clone(),
            original_command: probe.original_command,
            state: ProbeState::Pending,
            candidate_index: next_index,
        });
        let _ = probe_store.write(&probes);
        let _ = stats_store.save(&stats);

        PostHookOutput::allow_with_message(
            format!("Candidate failed. Retry with: {prefixed}")
        )
    } else {
        // Exhausted
        let _ = probe_store.write(&probes);
        stats.global.probes_exhausted += 1;
        let _ = stats_store.save(&stats);

        PostHookOutput::allow_with_message(format!(
            "All candidates failed for `{}`. Add a confirmed mapping manually \
             or specify a prefix to try.",
            probe.original_command.as_str(),
        ))
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple ISO 8601 without chrono dep
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn unix_to_ymd_hms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Days since 1970-01-01; approximate month/year via day-of-year
    let y = 1970 + days / 365;
    let yd = days % 365;
    let (mo, d) = month_day(yd, is_leap(y));
    (y, mo, d, h, m, s)
}

fn is_leap(y: u64) -> bool { y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) }

fn month_day(day_of_year: u64, leap: bool) -> (u64, u64) {
    let months = if leap {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut remaining = day_of_year;
    for (i, &days) in months.iter().enumerate() {
        if remaining < days { return ((i + 1) as u64, remaining + 1); }
        remaining -= days;
    }
    (12, 31)
}
```

Update post-hook dispatch in `main.rs` to call `handle_probe_result` when exit_code == 0
or when a Probing probe exists (check probe first regardless of exit code):

```rust
"post-hook" => {
    // ... payload parsing as in Task 9 ...
    let exit_code = payload.tool_response.as_ref()
        .and_then(|r| r.exit_code).unwrap_or(0);
    let stdout = payload.tool_response.as_ref()
        .and_then(|r| r.stdout.as_deref()).unwrap_or("");
    let stderr = payload.tool_response.as_ref()
        .and_then(|r| r.stderr.as_deref()).unwrap_or("");

    let prefix_store = prefixe::FilePrefixStore::new(prefixe::FilePrefixStore::default_path());
    let probe_store  = prefixe::FileProbeStore::new(prefixe::FileProbeStore::default_path());
    let stats_store  = prefixe::FileStatsStore::new(prefixe::FileStatsStore::default_path());

    // First: check if this is a Probing attempt resolving
    let out = post_hook::handle_probe_result(
        &command, exit_code, stdout, stderr,
        &prefix_store, &probe_store, &stats_store,
    );
    if out.system_message.is_some() {
        println!("{}", serde_json::to_string(&out).unwrap_or_default());
        return;
    }

    // Otherwise: if failure, start a new probe cycle
    if exit_code != 0 {
        let out = post_hook::handle_failure(
            &command, &prefix_store, &probe_store, &stats_store,
        );
        println!("{}", serde_json::to_string(&out).unwrap_or_default());
    }
}
```

3. Verify:

```
cargo nextest run -p prefixe-cli           → all green
cargo clippy -p prefixe-cli -- -D warnings → zero warnings
cargo nextest run --workspace              → all green
```

4. Run: `git branch --show-current`
   Commit: `git commit -m "feat(prefixe-cli): post-hook Probing resolution — confirm, cycle, exhaust with stats"`
