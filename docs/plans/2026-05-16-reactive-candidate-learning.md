# Reactive Candidate Prefix Learning

**Date:** 2026-05-16
**Status:** Approved

## Goal

Fix `candidate_prefixes` so it learns which prefix a command needs reactively (on failure)
rather than proactively (applied blindly to every unknown command). Proactive rewriting
remains for confirmed mappings only. The learning loop is fully automatic but always notifies
Claude on first confirmation.

## Problem

The current design applies `candidate_prefixes.first()` to every command with no confirmed
mapping. Combined with `learn_on_successful_fallback = true`, this caused every command that
ran successfully to accumulate a spurious confirmed mapping, polluting the prefix store with
entries like `grep`, `ls`, `cargo`, `echo` all mapped to `op plugin run --`.

## Architecture

### Crates affected

- `prefixe` — domain types, state machine logic, `ProbeStore` trait, `FileProbeStore` adapter
- `coursers` — hook layer: reads Claude Code JSON, writes `systemMessage`, calls `prefixe`

### New / changed types

#### `CandidatePrefix` (replaces `Vec<String>` elements in `candidate_prefixes`)

```rust
pub struct CandidatePrefix {
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicate,
}

pub struct SuccessPredicate {
    /// Required exit code. `None` = any. Default: `Some(0)`.
    pub exit_code: Option<i32>,
    /// Regex stdout must match. `None` = not checked.
    pub stdout_matches: Option<String>,
    /// Regex stderr must match. `None` = not checked.
    pub stderr_matches: Option<String>,
    /// If true, stderr must be empty/absent.
    pub stderr_absent: bool,
}
```

`SuccessPredicate::default()` → `exit_code = Some(0)`, all others permissive.

#### `PrefixConfig`

```rust
pub struct PrefixConfig {
    pub mappings: HashMap<String, Vec<String>>,
    pub candidate_prefixes: Vec<CandidatePrefix>,  // was Vec<Vec<String>>
}
```

`learn_on_successful_fallback` is removed — learning is now always reactive and the
predicate controls what counts as success.

#### `ProbeState` (new)

```rust
pub enum ProbeState {
    /// Command failed bare; waiting for Claude to retry with candidate.
    Pending,
    /// Pre-hook rewrote the retry; waiting on post-hook exit code.
    Probing,
}
```

#### `ProbeEntry` (extended)

```rust
pub struct ProbeEntry {
    pub key: String,
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicate,  // carried from CandidatePrefix
    pub original_command: OriginalCommand,
    pub state: ProbeState,
    pub candidate_index: usize,
}
```

### State machine

```
[no probe] ──(bare cmd fails)──► Pending(index=0)
Pending    ──(pre-hook sees prefixed retry)──► Probing
Probing    ──(predicate passes)──► [confirmed mapping] + notify Claude
Probing    ──(predicate fails, next exists)──► Pending(index+1) + suggest next
Probing    ──(predicate fails, exhausted)──► [removed] + ask Claude
```

### Hook responsibilities

#### `prefixe` library (pure, hook-agnostic)

- `CandidatePrefix`, `SuccessPredicate`, `ProbeState`, `ProbeEntry`
- `PrefixConfig` with `candidate_prefixes: Vec<CandidatePrefix>`
- `SuccessPredicate::evaluate(exit_code, stdout, stderr) -> bool`
- `rewrite_command` — confirmed mappings only, no candidate probing
- `ProbeStore` trait + `FileProbeStore` adapter (TOML at `.ctx/candidates.toml`)
- `PrefixStore` + `FilePrefixStore` unchanged

#### `coursers` hook layer

- **post-hook on failure:**
  1. Check `candidate_prefixes` non-empty
  2. Check no existing `Pending`/`Probing` probe for this command key
  3. Write `Pending` probe at `candidate_index = 0`
  4. Emit `systemMessage`: `"Command failed. Retry with: <prefix[0]> <cmd>"`

- **pre-hook:**
  1. Check confirmed mappings → rewrite silently if found (unchanged)
  2. Check if incoming command matches `<candidate.prefix> <original_cmd>` for a
     `Pending` probe → transition probe to `Probing`, allow command through unchanged

- **post-hook on exit (Probing probe matched):**
  - Predicate passes → `confirm_mapping(key, prefix)`, remove probe,
    emit `systemMessage`: `"Candidate <prefix> succeeded for <cmd> — mapping confirmed."`
  - Predicate fails, next candidate exists → write `Pending` probe at `index+1`,
    emit `systemMessage`: `"Failed. Retry with: <prefix[index+1]> <cmd>"`
  - Predicate fails, all exhausted → remove probe,
    emit `systemMessage`: `"All candidates failed for <cmd>. Add a confirmed mapping
manually or specify a prefix to try."`

- **post-hook on exit (confirmed mapping):** silent.

### TOML config schema

```toml
[mappings]
gh = ["op", "plugin", "run", "--"]

[[candidate_prefixes]]
prefix = ["op", "plugin", "run", "--"]
# success_when defaults to exit_code = 0

[[candidate_prefixes]]
prefix = ["dotenvx", "run", "--"]
success_when.exit_code = 0
success_when.stderr_absent = true

[[candidate_prefixes]]
prefix = ["OPENAI_API_KEY=..."]
success_when.exit_code = 0
success_when.stdout_matches = "^(?!Error)"
```

## Stats tracking

A `StatsStore` port with a TOML file adapter (structured for future SQLite migration).
Lives in `prefixe`. Written by `coursers` hook layer on each state transition.

### Types

```rust
pub struct PrefixStats {
    pub global: GlobalStats,
    pub by_prefix: HashMap<String, PrefixStats>,   // key = prefix tokens joined by " "
    pub by_command: HashMap<String, CommandStats>,  // key = command word
}

pub struct GlobalStats {
    pub probes_initiated: u64,
    pub probes_confirmed: u64,
    pub probes_exhausted: u64,
}

pub struct PrefixCounters {
    pub tried: u64,
    pub confirmed: u64,
    pub failed: u64,
}

pub struct CommandStats {
    pub probes_initiated: u64,
    pub confirmed_prefix: Option<String>,
    pub confirmed_at: Option<String>,   // ISO 8601
}
```

### TOML on disk

```toml
[global]
probes_initiated = 14
probes_confirmed = 3
probes_exhausted = 2

[by_prefix."op plugin run --"]
tried = 10
confirmed = 3
failed = 4

[by_command.gh]
probes_initiated = 3
confirmed_prefix = "op plugin run --"
confirmed_at = "2026-05-16T10:32:00Z"
```

### Port

```rust
pub trait StatsStore {
    fn load(&self) -> PrefixStats;
    fn save(&self, stats: &PrefixStats) -> Result<(), Error>;
}
```

`FileStatsStore` is the TOML adapter. Swapping to SQLite = new adapter, same trait.

### When stats are written

| Event                        | Counters incremented                                                                              |
| ---------------------------- | ------------------------------------------------------------------------------------------------- |
| Pending probe written        | `global.probes_initiated`, `by_command.<key>.probes_initiated`                                    |
| Probing attempt              | `by_prefix.<prefix>.tried`                                                                        |
| Confirmed                    | `global.probes_confirmed`, `by_prefix.<prefix>.confirmed`, `by_command.<key>.confirmed_prefix/at` |
| Probing failed (next exists) | `by_prefix.<prefix>.failed`                                                                       |
| Exhausted                    | `global.probes_exhausted`, `by_prefix.<prefix>.failed`                                            |

## Tech decisions

| Decision                               | Rationale                                                              |
| -------------------------------------- | ---------------------------------------------------------------------- |
| `SuccessPredicate` per candidate       | Commands need different success signals; exit 0 alone is insufficient  |
| `ProbeState` as enum                   | Pending and Probing are semantically distinct; collapsing loses safety |
| `candidate_index` on probe             | Enables cycling through candidates without re-querying config          |
| Notify on first confirmation only      | Silent on confirmed mappings; avoids noise on every run                |
| Learning always reactive               | Eliminates the "every command gets a spurious mapping" failure mode    |
| `learn_on_successful_fallback` removed | Subsumed by the new reactive model; no longer meaningful               |
| `StatsStore` as a port                 | Keeps TOML adapter swappable for SQLite without touching domain logic  |
| Running counters (not append log)      | Cheap reads for display; TOML is sufficient at this scale              |

## Out of scope

- Per-command candidate lists (candidates are global; apply to any unknown command that fails)
- Confirmation requiring explicit Claude approval (notification is sufficient)
- Retry count / backoff (one attempt per candidate, then escalate)
- Persistence of exhausted-candidate history (probes are ephemeral; no long-term failure log)
- Changes to `PrefixRule` / `rewrite_with_rules` (conditional rules unaffected)
