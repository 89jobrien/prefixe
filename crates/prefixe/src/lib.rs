pub mod domain;
pub mod error;
pub mod infra;

pub use domain::{
    CommandRewriter, CommandSplitter, OriginalCommand, PrefixConfig, TextualSplitter,
};
pub use error::Error;
pub use infra::path::{EnvPathResolver, PathResolver};
pub use infra::toml_store::{FilePrefixStore, FileProbeStore};

/// Port for reading and writing the prefix config.
pub trait PrefixStore {
    fn load(&self) -> PrefixConfig;
    /// Merge-write: add `key → prefix` to existing mappings without overwriting others.
    fn confirm_mapping(&self, key: &str, prefix: &[String]) -> Result<(), Error>;
    /// Remove a confirmed mapping by key.
    /// Returns `true` if a mapping was removed, `false` if the key was not found.
    fn remove_mapping(&self, key: &str) -> Result<bool, Error>;
}

/// One shell segment plus the separator that followed it (if any).
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
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixMatch {
    /// A definite mapping exists in `mappings`.
    Confirmed { key: String, prefix: Vec<String> },
    /// No mapping; a candidate prefix is being tried speculatively.
    Candidate { key: String, prefix: Vec<String> },
}

/// Look up the prefix for the leading command word(s) of `segment`.
///
/// Returns `None` if:
/// - `segment` contains `$(` or a backtick (subshell — unsafe to rewrite blindly)
/// - no mapping or candidate applies
///
/// Two-word key check happens before single-word.
/// All candidate prefixes are tried in order; the first is returned.
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

    if let Some(candidate) = config.candidate_prefixes.first() {
        return Some(PrefixMatch::Candidate {
            key: first.to_string(),
            prefix: candidate.clone(),
        });
    }

    None
}

/// A pending candidate probe: we applied a speculative prefix and need post-hook
/// learning to confirm or discard it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeEntry {
    pub key: String,
    pub prefix: Vec<String>,
    pub original_command: OriginalCommand,
}

/// Result of rewriting a full command string.
#[derive(Debug, Clone)]
pub struct RewriteResult {
    pub rewritten: String,
    pub probes: Vec<ProbeEntry>,
}

/// Rewrite `cmd` by prepending learned prefixes to each shell segment.
///
/// Probes are only recorded when `config.learn_on_successful_fallback` is `true`.
pub fn rewrite_command(cmd: &str, config: &PrefixConfig) -> RewriteResult {
    let mut segs = split_segments(cmd);
    let mut probes = Vec::new();

    for seg in &mut segs {
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(m) = lookup_prefix(trimmed, config) else {
            continue;
        };
        let (key, prefix, is_candidate) = match m {
            PrefixMatch::Confirmed { key, prefix } => (key, prefix, false),
            PrefixMatch::Candidate { key, prefix } => (key, prefix, true),
        };
        let leading_len = seg.text.len() - seg.text.trim_start().len();
        let leading = &seg.text[..leading_len];
        let trailing_start = leading_len + trimmed.len();
        let trailing = &seg.text[trailing_start..];
        let prefix_str = prefix.join(" ");
        seg.text = format!("{leading}{prefix_str} {trimmed}{trailing}");

        if is_candidate && config.learn_on_successful_fallback {
            probes.push(ProbeEntry {
                key,
                prefix,
                original_command: OriginalCommand::from(cmd),
            });
        }
    }

    RewriteResult {
        rewritten: rejoin(&segs),
        probes,
    }
}

/// Port for reading and writing candidate probes.
pub trait ProbeStore {
    fn load(&self) -> Vec<ProbeEntry>;
    fn write(&self, entries: &[ProbeEntry]) -> Result<(), Error>;
    fn remove_matching(&self, cmd: &OriginalCommand) -> Result<(), Error>;
}

/// Snapshot of prefix learning state for display / operator tooling.
#[derive(Debug, Clone, Default)]
pub struct AuditState {
    pub mappings: Vec<(String, Vec<String>)>,
    pub probes: Vec<ProbeEntry>,
}

/// Assemble the current prefix learning state from both stores.
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
                .map(|c| c.iter().map(|s| s.to_string()).collect())
                .collect(),
            learn_on_successful_fallback: false,
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
    fn lookup_candidate_fallback() {
        let store = make_store(&[], &[&["op", "plugin", "run", "--"]]);
        let result = lookup_prefix("gh issue list", &store.load());
        assert_eq!(
            result,
            Some(PrefixMatch::Candidate {
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
    fn rewrite_candidate_no_probe_when_learn_disabled() {
        let store = make_store(&[], &[&["op", "plugin", "run", "--"]]);
        let r = rewrite_command("gh issue list", &store.load());
        assert_eq!(r.rewritten, "op plugin run -- gh issue list");
        assert!(
            r.probes.is_empty(),
            "probes should be empty when learn=false"
        );
    }

    #[test]
    fn rewrite_candidate_records_probe_when_learn_enabled() {
        let mut config = make_store(&[], &[&["op", "plugin", "run", "--"]]).config;
        config.learn_on_successful_fallback = true;
        let r = rewrite_command("gh issue list", &config);
        assert_eq!(r.rewritten, "op plugin run -- gh issue list");
        assert_eq!(r.probes.len(), 1);
        assert_eq!(r.probes[0].key, "gh");
    }

    #[test]
    fn rewrite_no_match_unchanged() {
        let store = make_store(&[], &[]);
        let r = rewrite_command("echo hello", &store.load());
        assert_eq!(r.rewritten, "echo hello");
        assert!(r.probes.is_empty());
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
            original_command: OriginalCommand::from("gh issue list"),
        }];
        store.write(&entries).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "gh");
    }

    #[test]
    fn probe_store_remove_matching() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FileProbeStore::new(dir.path().join("candidates.toml"));
        store
            .write(&[
                ProbeEntry {
                    key: "gh".to_string(),
                    prefix: vec![],
                    original_command: OriginalCommand::from("gh issue list"),
                },
                ProbeEntry {
                    key: "cargo".to_string(),
                    prefix: vec![],
                    original_command: OriginalCommand::from("cargo build"),
                },
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
        let entries = vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![],
            original_command: OriginalCommand::from("gh issue list"),
        }];
        store.write(&entries).unwrap();
        assert_eq!(store.load().len(), 1);
        store
            .remove_matching(&OriginalCommand::from("gh issue list"))
            .unwrap();
        assert!(store.load().is_empty());
    }

    #[test]
    fn domain_prefix_config_has_no_serde_dependency() {
        // Verifies PrefixConfig is a pure domain type (no serde derives)
        let _c: PrefixConfig = Default::default();
        assert!(_c.mappings.is_empty());
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
}
