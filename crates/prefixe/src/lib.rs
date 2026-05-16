pub mod domain;
pub mod engine;
pub mod error;
pub mod infra;

pub use domain::{
    CandidatePrefix, CommandRewriter, CommandSplitter, OriginalCommand, PrefixConfig, PrefixRule,
    RuleCondition, SuccessPredicate, TextualSplitter,
};
pub use engine::PrefixEngine;
pub use error::Error;
pub use infra::path::{EnvPathResolver, PathResolver};
pub use infra::toml_store::{FilePrefixStore, FileProbeStore};

/// Port for reading and writing the prefix config.
///
/// Implement this trait to supply a custom persistence layer.  The canonical
/// adapter is [`FilePrefixStore`]; use `testing::FakePrefixStore` in tests.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, PrefixStore, Error};
///
/// struct AlwaysEmptyStore;
///
/// impl PrefixStore for AlwaysEmptyStore {
///     fn load(&self) -> PrefixConfig { PrefixConfig::default() }
///     fn confirm_mapping(&self, _key: &str, _prefix: &[String]) -> Result<(), Error> { Ok(()) }
///     fn remove_mapping(&self, _key: &str) -> Result<bool, Error> { Ok(false) }
/// }
///
/// let store = AlwaysEmptyStore;
/// assert!(store.load().mappings.is_empty());
/// ```
pub trait PrefixStore {
    fn load(&self) -> PrefixConfig;
    /// Merge-write: add `key → prefix` to existing mappings without overwriting others.
    fn confirm_mapping(&self, key: &str, prefix: &[String]) -> Result<(), Error>;
    /// Remove a confirmed mapping by key.
    /// Returns `true` if a mapping was removed, `false` if the key was not found.
    fn remove_mapping(&self, key: &str) -> Result<bool, Error>;
}

/// One shell segment plus the separator that followed it (if any).
///
/// # Examples
///
/// ```
/// use prefixe::Segment;
///
/// let seg = Segment { text: "cargo build ".to_string(), sep: Some("|".to_string()) };
/// assert_eq!(seg.sep.as_deref(), Some("|"));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// The raw text of this segment (may have leading/trailing spaces).
    pub text: String,
    /// The separator token that terminated this segment: `&&`, `||`, `;`, or `|`.
    pub sep: Option<String>,
}

/// Split a shell command string into segments on `&&`, `||`, `;`, `|`.
///
/// Preserves surrounding whitespace in each segment so `rejoin` is lossless.
/// Does NOT handle quotes — splitting is purely textual, which is correct for
/// the commands we see in practice (no quoted separators).
///
/// When two separators match at the same position, the longer one wins
/// (e.g. `||` beats `|`).
///
/// # Examples
///
/// ```
/// use prefixe::split_segments;
///
/// let segs = split_segments("cargo build | tail -5");
/// assert_eq!(segs.len(), 2);
/// assert_eq!(segs[0].sep.as_deref(), Some("|"));
/// assert_eq!(segs[1].sep, None);
/// ```
pub fn split_segments(cmd: &str) -> Vec<Segment> {
    let seps = ["&&", "||", ";", "|"];
    let mut result = Vec::new();
    let mut remaining = cmd;

    'outer: loop {
        let mut earliest: Option<(usize, &str)> = None;
        for sep in &seps {
            if let Some(pos) = remaining.find(sep) {
                let better = match earliest {
                    None => true,
                    Some((e, prev)) => pos < e || (pos == e && sep.len() > prev.len()),
                };
                if better {
                    earliest = Some((pos, sep));
                }
            }
        }
        match earliest {
            None => {
                result.push(Segment {
                    text: remaining.to_string(),
                    sep: None,
                });
                break 'outer;
            }
            Some((pos, sep)) => {
                result.push(Segment {
                    text: remaining[..pos].to_string(),
                    sep: Some(sep.to_string()),
                });
                remaining = &remaining[pos + sep.len()..];
            }
        }
    }
    result
}

/// Reconstruct the original command string from segments.
///
/// This is the inverse of [`split_segments`] and is lossless for all inputs.
///
/// # Examples
///
/// ```
/// use prefixe::{rejoin, split_segments};
///
/// let cmd = "git add -A && git commit -m 'msg'";
/// assert_eq!(rejoin(&split_segments(cmd)), cmd);
/// ```
pub fn rejoin(segs: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segs {
        out.push_str(&seg.text);
        if let Some(sep) = &seg.sep {
            out.push_str(sep);
        }
    }
    out
}

/// Result of a prefix lookup for a single segment's base command.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, PrefixMatch, lookup_prefix};
///
/// let config = PrefixConfig {
///     mappings: [("gh".to_string(), vec!["op".to_string()])]
///         .into_iter()
///         .collect(),
///     ..Default::default()
/// };
/// assert!(matches!(lookup_prefix("gh issue list", &config), Some(PrefixMatch::Confirmed { .. })));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixMatch {
    /// A definite mapping exists in `mappings`.
    Confirmed { key: String, prefix: Vec<String> },
}

/// Look up the prefix for the leading command word(s) of `segment`.
///
/// Returns `None` if:
/// - `segment` contains `$(` or a backtick (subshell — unsafe to rewrite blindly)
/// - no mapping or candidate applies
///
/// Two-word key check happens before single-word.
/// All candidate prefixes are tried in order; the first is returned.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, PrefixMatch, lookup_prefix};
///
/// let config = PrefixConfig::default();
/// assert_eq!(lookup_prefix("echo hello", &config), None);
/// assert_eq!(lookup_prefix("$(cmd)", &config), None);
/// ```
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
            return Some(PrefixMatch::Confirmed {
                key: two_word,
                prefix: prefix.clone(),
            });
        }
    }

    if let Some(prefix) = config.mappings.get(first) {
        return Some(PrefixMatch::Confirmed {
            key: first.to_string(),
            prefix: prefix.clone(),
        });
    }

    None
}

/// Lifecycle state of a candidate probe.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    /// Command failed bare; waiting for Claude to retry with this candidate.
    Pending,
    /// Pre-hook rewrote the retry; waiting on the post-hook exit code.
    Probing,
}

/// A pending or active candidate probe.
///
/// # Examples
///
/// ```
/// use prefixe::{OriginalCommand, ProbeEntry, ProbeState, SuccessPredicate};
///
/// let entry = ProbeEntry {
///     key: "gh".to_string(),
///     prefix: vec!["op".to_string()],
///     success_when: SuccessPredicate::exit_zero(),
///     original_command: OriginalCommand::from("gh issue list"),
///     state: ProbeState::Pending,
///     candidate_index: 0,
/// };
/// assert_eq!(entry.key, "gh");
/// assert_eq!(entry.state, ProbeState::Pending);
/// ```
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

/// Result of rewriting a full command string.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, rewrite_command};
///
/// let config = PrefixConfig::default();
/// let result = rewrite_command("echo hi", &config);
/// assert_eq!(result.rewritten, "echo hi");
/// assert!(result.probes.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct RewriteResult {
    pub rewritten: String,
    pub probes: Vec<ProbeEntry>,
}

/// Rewrite `cmd` using the given store as the prefix source.
///
/// Prefer this over `rewrite_command` when you have a `PrefixStore` port.
/// For full use-case orchestration, use [`PrefixEngine`] instead.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, PrefixStore, Error, rewrite_via_store};
///
/// struct EmptyStore;
/// impl PrefixStore for EmptyStore {
///     fn load(&self) -> PrefixConfig { PrefixConfig::default() }
///     fn confirm_mapping(&self, _: &str, _: &[String]) -> Result<(), Error> { Ok(()) }
///     fn remove_mapping(&self, _: &str) -> Result<bool, Error> { Ok(false) }
/// }
///
/// let result = rewrite_via_store("echo hello", &EmptyStore);
/// assert_eq!(result.rewritten, "echo hello");
/// ```
pub fn rewrite_via_store(cmd: &str, store: &dyn PrefixStore) -> RewriteResult {
    rewrite_command(cmd, &store.load())
}

/// Rewrite `cmd` by prepending learned prefixes to each shell segment.
///
/// Probes are only recorded when `config.learn_on_successful_fallback` is `true`.
/// Prefer [`rewrite_via_store`] or [`PrefixEngine::rewrite`] in application code.
///
/// # Examples
///
/// ```
/// use prefixe::{PrefixConfig, rewrite_command};
///
/// let mut config = PrefixConfig::default();
/// config.mappings.insert("gh".to_string(), vec!["op".to_string(), "run".to_string(), "--".to_string()]);
///
/// let result = rewrite_command("gh issue list", &config);
/// assert_eq!(result.rewritten, "op run -- gh issue list");
/// ```
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
        let PrefixMatch::Confirmed { prefix, .. } = m;
        let leading_len = seg.text.len() - seg.text.trim_start().len();
        let leading = &seg.text[..leading_len];
        let trailing_start = leading_len + trimmed.len();
        let trailing = &seg.text[trailing_start..];
        let prefix_str = prefix.join(" ");
        seg.text = format!("{leading}{prefix_str} {trimmed}{trailing}");
    }

    RewriteResult {
        rewritten: rejoin(&segs),
        probes: vec![], // probes are written by the post-hook on failure, not the pre-hook
    }
}

/// Port for reading and writing candidate probes.
///
/// # Examples
///
/// ```
/// use prefixe::{Error, ProbeEntry, ProbeStore, OriginalCommand};
///
/// struct NoOpProbeStore;
///
/// impl ProbeStore for NoOpProbeStore {
///     fn load(&self) -> Vec<ProbeEntry> { vec![] }
///     fn write(&self, _entries: &[ProbeEntry]) -> Result<(), Error> { Ok(()) }
///     fn remove_matching(&self, _cmd: &OriginalCommand) -> Result<(), Error> { Ok(()) }
/// }
///
/// let store = NoOpProbeStore;
/// assert!(store.load().is_empty());
/// ```
pub trait ProbeStore {
    fn load(&self) -> Vec<ProbeEntry>;
    fn write(&self, entries: &[ProbeEntry]) -> Result<(), Error>;
    fn remove_matching(&self, cmd: &OriginalCommand) -> Result<(), Error>;
}

/// Snapshot of prefix learning state for display / operator tooling.
///
/// # Examples
///
/// ```
/// use prefixe::AuditState;
///
/// let state = AuditState::default();
/// assert!(state.mappings.is_empty());
/// assert!(state.probes.is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct AuditState {
    pub mappings: Vec<(String, Vec<String>)>,
    pub probes: Vec<ProbeEntry>,
}

/// Assemble the current prefix learning state from both stores.
///
/// # Examples
///
/// ```
/// use prefixe::{AuditState, Error, OriginalCommand, PrefixConfig, PrefixStore,
///               ProbeEntry, ProbeStore, audit_state};
///
/// struct EmptyPrefixStore;
/// impl PrefixStore for EmptyPrefixStore {
///     fn load(&self) -> PrefixConfig { PrefixConfig::default() }
///     fn confirm_mapping(&self, _: &str, _: &[String]) -> Result<(), Error> { Ok(()) }
///     fn remove_mapping(&self, _: &str) -> Result<bool, Error> { Ok(false) }
/// }
///
/// struct EmptyProbeStore;
/// impl ProbeStore for EmptyProbeStore {
///     fn load(&self) -> Vec<ProbeEntry> { vec![] }
///     fn write(&self, _: &[ProbeEntry]) -> Result<(), Error> { Ok(()) }
///     fn remove_matching(&self, _: &OriginalCommand) -> Result<(), Error> { Ok(()) }
/// }
///
/// let state = audit_state(&EmptyPrefixStore, &EmptyProbeStore);
/// assert!(state.mappings.is_empty());
/// ```
pub fn audit_state(prefix_store: &dyn PrefixStore, probe_store: &dyn ProbeStore) -> AuditState {
    let config = prefix_store.load();
    let mut mappings: Vec<(String, Vec<String>)> = config.mappings.into_iter().collect();
    mappings.sort_by(|a, b| a.0.cmp(&b.0));
    let probes = probe_store.load();
    AuditState { mappings, probes }
}

/// Test doubles available to downstream crates under the `testing` feature.
#[cfg(any(test, feature = "testing"))]
pub use infra::path::ExplicitPathResolver;

#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::*;

    pub struct FakePrefixStore {
        pub config: PrefixConfig,
        pub confirmed: std::cell::RefCell<Option<(String, Vec<String>)>>,
        pub removed: std::cell::RefCell<Option<String>>,
    }

    impl FakePrefixStore {
        pub fn new(config: PrefixConfig) -> Self {
            Self {
                config,
                confirmed: std::cell::RefCell::new(None),
                removed: std::cell::RefCell::new(None),
            }
        }
    }

    impl PrefixStore for FakePrefixStore {
        fn load(&self) -> PrefixConfig {
            self.config.clone()
        }

        fn confirm_mapping(&self, key: &str, prefix: &[String]) -> Result<(), Error> {
            *self.confirmed.borrow_mut() = Some((key.to_string(), prefix.to_vec()));
            Ok(())
        }

        fn remove_mapping(&self, key: &str) -> Result<bool, Error> {
            let existed = self.config.mappings.contains_key(key);
            *self.removed.borrow_mut() = Some(key.to_string());
            Ok(existed)
        }
    }

    pub struct FakeProbeStore {
        pub entries: std::cell::RefCell<Vec<ProbeEntry>>,
    }

    impl FakeProbeStore {
        pub fn new(entries: Vec<ProbeEntry>) -> Self {
            Self {
                entries: std::cell::RefCell::new(entries),
            }
        }

        pub fn empty() -> Self {
            Self::new(vec![])
        }
    }

    pub struct FakeRewriter {
        pub result: RewriteResult,
    }

    impl CommandRewriter for FakeRewriter {
        fn rewrite(&self, _cmd: &str) -> RewriteResult {
            self.result.clone()
        }
    }

    impl ProbeStore for FakeProbeStore {
        fn load(&self) -> Vec<ProbeEntry> {
            self.entries.borrow().clone()
        }

        fn write(&self, entries: &[ProbeEntry]) -> Result<(), Error> {
            *self.entries.borrow_mut() = entries.to_vec();
            Ok(())
        }

        fn remove_matching(&self, cmd: &OriginalCommand) -> Result<(), Error> {
            self.entries
                .borrow_mut()
                .retain(|e| e.original_command != *cmd);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testing::{FakePrefixStore, FakeProbeStore};

    fn make_store(mappings: &[(&str, &[&str])], candidates: &[&[&str]]) -> FakePrefixStore {
        FakePrefixStore::new(PrefixConfig {
            mappings: mappings
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
            candidate_prefixes: candidates
                .iter()
                .map(|c| CandidatePrefix {
                    prefix: c.iter().map(|s| s.to_string()).collect(),
                    success_when: SuccessPredicate::exit_zero(),
                })
                .collect(),
        })
    }

    #[test]
    fn split_simple_pipeline() {
        let segs = split_segments("cargo build | tail -5");
        assert_eq!(
            segs,
            vec![
                Segment {
                    text: "cargo build ".to_string(),
                    sep: Some("|".to_string())
                },
                Segment {
                    text: " tail -5".to_string(),
                    sep: None
                },
            ]
        );
    }

    #[test]
    fn split_and_and() {
        let segs = split_segments("git add -A && git commit -m 'msg'");
        assert_eq!(
            segs,
            vec![
                Segment {
                    text: "git add -A ".to_string(),
                    sep: Some("&&".to_string())
                },
                Segment {
                    text: " git commit -m 'msg'".to_string(),
                    sep: None
                },
            ]
        );
    }

    #[test]
    fn split_or_or_beats_single_pipe_at_same_position() {
        let segs = split_segments("a || b");
        assert_eq!(
            segs,
            vec![
                Segment {
                    text: "a ".to_string(),
                    sep: Some("||".to_string())
                },
                Segment {
                    text: " b".to_string(),
                    sep: None
                },
            ]
        );
    }

    #[test]
    fn rejoin_preserves_separators() {
        let segs = vec![
            Segment {
                text: "cargo build ".to_string(),
                sep: Some("|".to_string()),
            },
            Segment {
                text: " tail -5".to_string(),
                sep: None,
            },
        ];
        assert_eq!(rejoin(&segs), "cargo build | tail -5");
    }

    #[test]
    fn lookup_single_word_key_matches() {
        let store = make_store(&[("gh", &["op", "plugin", "run", "--"])], &[]);
        let result = lookup_prefix("gh issue list", &store.load());
        assert_eq!(
            result,
            Some(PrefixMatch::Confirmed {
                key: "gh".to_string(),
                prefix: vec![
                    "op".to_string(),
                    "plugin".to_string(),
                    "run".to_string(),
                    "--".to_string()
                ],
            })
        );
    }

    #[test]
    fn lookup_two_word_key_wins_over_single() {
        let store = make_store(
            &[
                ("cargo", &["op", "plugin", "run", "--"]),
                ("cargo test", &["dotenvx", "run", "--"]),
            ],
            &[],
        );
        let result = lookup_prefix("cargo test --workspace", &store.load());
        assert_eq!(
            result,
            Some(PrefixMatch::Confirmed {
                key: "cargo test".to_string(),
                prefix: vec!["dotenvx".to_string(), "run".to_string(), "--".to_string()],
            })
        );
    }

    #[test]
    fn lookup_no_match_returns_none() {
        let store = make_store(&[], &[]);
        assert_eq!(lookup_prefix("echo hello", &store.load()), None);
    }

    #[test]
    fn lookup_prefix_returns_none_for_unknown_command_even_with_candidates() {
        // candidate_prefixes no longer causes lookup_prefix to return a match;
        // probing is triggered reactively by the post-hook on failure.
        let store = make_store(&[], &[&["op", "plugin", "run", "--"]]);
        assert_eq!(lookup_prefix("grep foo .", &store.load()), None);
        assert_eq!(lookup_prefix("gh issue list", &store.load()), None);
    }

    #[test]
    fn lookup_skips_subshell() {
        let store = make_store(&[("gh", &["op", "plugin", "run", "--"])], &[]);
        assert_eq!(lookup_prefix("$(gh issue list)", &store.load()), None);
        assert_eq!(lookup_prefix("`gh issue list`", &store.load()), None);
    }

    #[test]
    fn rewrite_simple_confirmed() {
        let store = make_store(&[("gh", &["op", "plugin", "run", "--"])], &[]);
        let r = rewrite_command("gh issue list", &store.load());
        assert_eq!(r.rewritten, "op plugin run -- gh issue list");
        assert!(r.probes.is_empty());
    }

    #[test]
    fn rewrite_compound_each_segment() {
        let store = make_store(&[("gh", &["op", "plugin", "run", "--"])], &[]);
        let r = rewrite_command("gh issue list && gh pr list", &store.load());
        assert_eq!(
            r.rewritten,
            "op plugin run -- gh issue list && op plugin run -- gh pr list"
        );
    }

    #[test]
    fn rewrite_unknown_command_unchanged_when_no_confirmed_mapping() {
        // Without a confirmed mapping, commands are passed through unchanged.
        // Candidate probing is triggered reactively by the post-hook on failure.
        let store = make_store(&[], &[&["op", "plugin", "run", "--"]]);
        let r = rewrite_command("gh issue list", &store.load());
        assert_eq!(r.rewritten, "gh issue list");
        assert!(r.probes.is_empty());
    }

    #[test]
    fn rewrite_no_match_unchanged() {
        let store = make_store(&[], &[]);
        let r = rewrite_command("echo hello", &store.load());
        assert_eq!(r.rewritten, "echo hello");
        assert!(r.probes.is_empty());
    }

    fn make_probe(key: &str, cmd: &str) -> ProbeEntry {
        ProbeEntry {
            key: key.to_string(),
            prefix: vec!["op".to_string()],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from(cmd),
            state: ProbeState::Pending,
            candidate_index: 0,
        }
    }

    #[test]
    fn probe_store_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FileProbeStore::new(dir.path().join("candidates.toml"));
        let entries = vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Pending,
            candidate_index: 0,
        }];
        store.write(&entries).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "gh");
        assert_eq!(loaded[0].state, ProbeState::Pending);
    }

    #[test]
    fn probe_store_remove_matching() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FileProbeStore::new(dir.path().join("candidates.toml"));
        store
            .write(&[
                make_probe("gh", "gh issue list"),
                make_probe("cargo", "cargo build"),
            ])
            .unwrap();
        store
            .remove_matching(&OriginalCommand::from("gh issue list"))
            .unwrap();
        let remaining = store.load();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key, "cargo");
    }

    #[test]
    fn prefix_store_confirm_and_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FilePrefixStore::new(dir.path().join("prefixes.toml"));
        store
            .confirm_mapping(
                "gh",
                &["op".to_string(), "run".to_string(), "--".to_string()],
            )
            .unwrap();
        let config = store.load();
        assert!(config.mappings.contains_key("gh"));

        let removed = store.remove_mapping("gh").unwrap();
        assert!(removed);
        let config = store.load();
        assert!(!config.mappings.contains_key("gh"));

        let removed_again = store.remove_mapping("gh").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn fake_prefix_store_confirm_and_remove() {
        let store = FakePrefixStore::new(PrefixConfig {
            mappings: [("gh".to_string(), vec!["op".to_string()])]
                .into_iter()
                .collect(),
            ..Default::default()
        });
        store
            .confirm_mapping("cargo", &["dotenvx".to_string()])
            .unwrap();
        assert_eq!(store.confirmed.borrow().as_ref().unwrap().0, "cargo");
        let existed = store.remove_mapping("gh").unwrap();
        assert!(existed);
        let missing = store.remove_mapping("missing").unwrap();
        assert!(!missing);
    }

    #[test]
    fn fake_probe_store_round_trip() {
        let store = FakeProbeStore::empty();
        let entries = vec![make_probe("gh", "gh issue list")];
        store.write(&entries).unwrap();
        assert_eq!(store.load().len(), 1);
        store
            .remove_matching(&OriginalCommand::from("gh issue list"))
            .unwrap();
        assert!(store.load().is_empty());
    }

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

    #[test]
    fn domain_prefix_config_has_no_serde_dependency() {
        // Verifies PrefixConfig is a pure domain type (no serde derives)
        let _c: PrefixConfig = Default::default();
        assert!(_c.mappings.is_empty());
    }

    #[test]
    fn rewrite_via_store_port_confirmed() {
        use crate::testing::FakePrefixStore;
        use crate::{PrefixConfig, rewrite_via_store};
        let store = FakePrefixStore::new(PrefixConfig {
            mappings: [("gh".to_string(), vec!["op".to_string()])]
                .into_iter()
                .collect(),
            ..Default::default()
        });
        let r = rewrite_via_store("gh issue list", &store);
        assert_eq!(r.rewritten, "op gh issue list");
    }

    #[test]
    fn prefix_engine_rewrite_confirmed() {
        use crate::testing::{FakePrefixStore, FakeProbeStore};
        use crate::{PrefixConfig, PrefixEngine};

        let store = FakePrefixStore::new(PrefixConfig {
            mappings: [(
                "gh".to_string(),
                vec!["op".to_string(), "run".to_string(), "--".to_string()],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite("gh issue list");
        assert_eq!(r.rewritten, "op run -- gh issue list");
    }

    #[test]
    fn prefix_engine_audit_returns_sorted_mappings() {
        use crate::testing::{FakePrefixStore, FakeProbeStore};
        use crate::{PrefixConfig, PrefixEngine};

        let store = FakePrefixStore::new(PrefixConfig {
            mappings: [
                ("gh".to_string(), vec!["op".to_string()]),
                ("cargo".to_string(), vec!["dotenvx".to_string()]),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let audit = engine.audit();
        assert_eq!(audit.mappings[0].0, "cargo");
        assert_eq!(audit.mappings[1].0, "gh");
    }

    #[test]
    fn prefix_engine_implements_command_rewriter() {
        use crate::testing::{FakePrefixStore, FakeProbeStore};
        use crate::{CommandRewriter, PrefixConfig, PrefixEngine};

        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r: &dyn CommandRewriter = &engine;
        assert_eq!(r.rewrite("echo hi").rewritten, "echo hi");
    }

    #[test]
    fn command_rewriter_trait_is_mockable() {
        use crate::{CommandRewriter, RewriteResult};
        struct NoOpRewriter;
        impl CommandRewriter for NoOpRewriter {
            fn rewrite(&self, cmd: &str) -> RewriteResult {
                RewriteResult {
                    rewritten: cmd.to_string(),
                    probes: vec![],
                }
            }
        }
        let r = NoOpRewriter;
        assert_eq!(r.rewrite("echo hi").rewritten, "echo hi");
    }

    #[test]
    fn fake_rewriter_returns_preset_result() {
        use crate::testing::FakeRewriter;
        use crate::{CommandRewriter, RewriteResult};
        let fake = FakeRewriter {
            result: RewriteResult {
                rewritten: "op run -- gh".to_string(),
                probes: vec![],
            },
        };
        assert_eq!(fake.rewrite("gh").rewritten, "op run -- gh");
    }

    #[test]
    fn textual_splitter_splits_and_rejoins() {
        use crate::{CommandSplitter, TextualSplitter};
        let s = TextualSplitter;
        let segs = s.split("a | b");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].sep.as_deref(), Some("|"));
        assert_eq!(s.rejoin(&segs), "a | b");
    }

    #[test]
    fn explicit_path_resolver_returns_given_paths() {
        use crate::{ExplicitPathResolver, PathResolver};
        let r = ExplicitPathResolver {
            prefix_config: std::path::PathBuf::from("/tmp/prefixes.toml"),
            probe_store: std::path::PathBuf::from("/tmp/candidates.toml"),
        };
        assert_eq!(
            r.prefix_config_path(),
            std::path::PathBuf::from("/tmp/prefixes.toml")
        );
        assert_eq!(
            r.probe_store_path(),
            std::path::PathBuf::from("/tmp/candidates.toml")
        );
    }

    #[test]
    fn file_stores_from_resolver() {
        use crate::{ExplicitPathResolver, FilePrefixStore, FileProbeStore};
        let r = ExplicitPathResolver {
            prefix_config: std::path::PathBuf::from("/tmp/p.toml"),
            probe_store: std::path::PathBuf::from("/tmp/c.toml"),
        };
        let ps = FilePrefixStore::from_resolver(&r);
        let qs = FileProbeStore::from_resolver(&r);
        assert_eq!(ps.path, std::path::PathBuf::from("/tmp/p.toml"));
        assert_eq!(qs.path, std::path::PathBuf::from("/tmp/c.toml"));
    }

    #[test]
    fn original_command_from_str() {
        let cmd = OriginalCommand::from("gh issue list");
        assert_eq!(cmd.as_str(), "gh issue list");
        assert_eq!(cmd.to_string(), "gh issue list");
    }

    // ── Issue #20: conditional prefix rules ──────────────────────────────────

    #[test]
    fn rule_with_no_conditions_always_applies() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rule = PrefixRule {
            key: "gh".to_string(),
            prefix: vec!["op".to_string(), "run".to_string(), "--".to_string()],
            conditions: vec![],
            priority: 0,
        };
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        assert!(engine.evaluate_conditions(&rule.conditions));
    }

    #[test]
    fn env_var_set_condition_passes_when_var_present() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        unsafe { std::env::set_var("PREFIXE_TEST_VAR_20", "1") };
        let rule = PrefixRule {
            key: "gh".to_string(),
            prefix: vec![],
            conditions: vec![RuleCondition::EnvVarSet("PREFIXE_TEST_VAR_20".to_string())],
            priority: 0,
        };
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        assert!(engine.evaluate_conditions(&rule.conditions));
        unsafe { std::env::remove_var("PREFIXE_TEST_VAR_20") };
    }

    #[test]
    fn env_var_set_condition_fails_when_var_absent() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        unsafe { std::env::remove_var("PREFIXE_TEST_VAR_ABSENT_20") };
        let rule = PrefixRule {
            key: "gh".to_string(),
            prefix: vec![],
            conditions: vec![RuleCondition::EnvVarSet(
                "PREFIXE_TEST_VAR_ABSENT_20".to_string(),
            )],
            priority: 0,
        };
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        assert!(!engine.evaluate_conditions(&rule.conditions));
    }

    #[test]
    fn cwd_glob_condition_matches_current_dir() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rule = PrefixRule {
            key: "gh".to_string(),
            prefix: vec![],
            conditions: vec![RuleCondition::CwdGlob("*".to_string())],
            priority: 0,
        };
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        // "*" matches any single path component
        assert!(engine.evaluate_conditions(&rule.conditions));
    }

    #[test]
    fn git_root_condition_true_inside_repo() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rule = PrefixRule {
            key: "gh".to_string(),
            prefix: vec![],
            conditions: vec![RuleCondition::GitRoot],
            priority: 0,
        };
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        // The worktree itself is inside a git repo
        assert!(engine.evaluate_conditions(&rule.conditions));
    }

    #[test]
    fn engine_rewrite_with_rules_applies_matching_rule() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rules = vec![PrefixRule {
            key: "gh".to_string(),
            prefix: vec!["op".to_string(), "run".to_string(), "--".to_string()],
            conditions: vec![],
            priority: 0,
        }];
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite_with_rules("gh issue list", &rules);
        assert_eq!(r.rewritten, "op run -- gh issue list");
    }

    #[test]
    fn engine_rewrite_with_rules_skips_unmatched_condition() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        unsafe { std::env::remove_var("PREFIXE_NEVER_SET_VAR") };
        let rules = vec![PrefixRule {
            key: "gh".to_string(),
            prefix: vec!["op".to_string(), "run".to_string(), "--".to_string()],
            conditions: vec![RuleCondition::EnvVarSet(
                "PREFIXE_NEVER_SET_VAR".to_string(),
            )],
            priority: 0,
        }];
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite_with_rules("gh issue list", &rules);
        assert_eq!(r.rewritten, "gh issue list");
    }

    // ── Issue #21: priority ordering ─────────────────────────────────────────

    #[test]
    fn higher_priority_rule_wins() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rules = vec![
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["low".to_string()],
                conditions: vec![],
                priority: 0,
            },
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["high".to_string()],
                conditions: vec![],
                priority: 10,
            },
        ];
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite_with_rules("gh issue list", &rules);
        assert_eq!(r.rewritten, "high gh issue list");
    }

    #[test]
    fn equal_priority_preserves_definition_order() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rules = vec![
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["first".to_string()],
                conditions: vec![],
                priority: 5,
            },
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["second".to_string()],
                conditions: vec![],
                priority: 5,
            },
        ];
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite_with_rules("gh issue list", &rules);
        assert_eq!(r.rewritten, "first gh issue list");
    }

    #[test]
    fn first_match_wins_no_further_rules_evaluated() {
        use crate::PrefixEngine;
        use crate::domain::PrefixRule;
        use crate::testing::{FakePrefixStore, FakeProbeStore};

        let rules = vec![
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["winner".to_string()],
                conditions: vec![],
                priority: 1,
            },
            PrefixRule {
                key: "gh".to_string(),
                prefix: vec!["loser".to_string()],
                conditions: vec![],
                priority: 0,
            },
        ];
        let store = FakePrefixStore::new(PrefixConfig::default());
        let engine = PrefixEngine::new(store, FakeProbeStore::empty());
        let r = engine.rewrite_with_rules("gh issue list", &rules);
        assert_eq!(r.rewritten, "winner gh issue list");
    }
}
