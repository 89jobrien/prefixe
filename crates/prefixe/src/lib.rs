use std::collections::HashMap;

/// Mirrors the `~/.config/rx/prefixes.toml` schema.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct PrefixConfig {
    /// Definite mappings: command (or "cmd sub") → prefix argv.
    #[serde(default)]
    pub mappings: HashMap<String, Vec<String>>,
    /// Candidate prefixes to try when no mapping exists.
    #[serde(default)]
    pub candidate_prefixes: Vec<Vec<String>>,
    /// Whether to persist a successful candidate as a confirmed mapping.
    #[serde(default)]
    pub learn_on_successful_fallback: bool,
}

/// Port for reading and writing the prefix config.
pub trait PrefixStore {
    fn load(&self) -> PrefixConfig;
    /// Merge-write: add `key → prefix` to existing mappings without overwriting others.
    fn confirm_mapping(&self, key: &str, prefix: &[String]);
}

/// File-backed implementation reading `~/.config/rx/prefixes.toml`.
pub struct FilePrefixStore {
    pub path: std::path::PathBuf,
}

impl FilePrefixStore {
    pub fn default_path() -> std::path::PathBuf {
        std::env::var_os("CRS_RX_PREFIXES")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_CONFIG_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                            .join(".config")
                    });
                base.join("rx").join("prefixes.toml")
            })
    }

    /// Remove a confirmed mapping by key. No-op if the key does not exist.
    /// Returns `true` if a mapping was removed, `false` if the key was not found.
    pub fn remove_mapping(&self, key: &str) -> bool {
        let mut config = self.load();
        if config.mappings.remove(key).is_none() {
            return false;
        }
        match toml::to_string_pretty(&config) {
            Ok(serialized) => {
                if let Err(e) = std::fs::write(&self.path, &serialized) {
                    eprintln!(
                        "prefixe: warn: could not write prefixes to {}: {e}",
                        self.path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("prefixe: warn: could not serialize prefixes: {e}");
            }
        }
        true
    }
}

impl PrefixStore for FilePrefixStore {
    fn load(&self) -> PrefixConfig {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return PrefixConfig::default();
        };
        toml::from_str(&content).unwrap_or_default()
    }

    fn confirm_mapping(&self, key: &str, prefix: &[String]) {
        let mut config = self.load();
        config.mappings.insert(key.to_string(), prefix.to_vec());
        match toml::to_string_pretty(&config) {
            Ok(serialized) => {
                if let Err(e) = std::fs::write(&self.path, &serialized) {
                    eprintln!(
                        "prefixe: warn: could not write prefixes to {}: {e}",
                        self.path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("prefixe: warn: could not serialize prefixes: {e}");
            }
        }
    }
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
pub fn split_segments(cmd: &str) -> Vec<Segment> {
    let seps = ["&&", "||", ";", "|"];
    let mut result = Vec::new();
    let mut remaining = cmd;

    'outer: loop {
        let mut earliest: Option<(usize, &str)> = None;
        for sep in &seps {
            if let Some(pos) = remaining.find(sep)
                && earliest.is_none_or(|(e, _)| pos < e)
            {
                earliest = Some((pos, sep));
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
    pub original_command: String,
}

/// Result of rewriting a full command string.
#[derive(Debug, Clone)]
pub struct RewriteResult {
    pub rewritten: String,
    pub probes: Vec<ProbeEntry>,
}

/// Rewrite `cmd` by prepending learned prefixes to each shell segment.
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

        if is_candidate {
            probes.push(ProbeEntry {
                key,
                prefix,
                original_command: cmd.to_string(),
            });
        }
    }

    RewriteResult {
        rewritten: rejoin(&segs),
        probes,
    }
}

/// TOML-serializable wrapper for a list of probe entries.
#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct ProbeFile {
    #[serde(default)]
    probes: Vec<ProbeEntryToml>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ProbeEntryToml {
    key: String,
    prefix: Vec<String>,
    original_command: String,
}

impl From<&ProbeEntry> for ProbeEntryToml {
    fn from(e: &ProbeEntry) -> Self {
        Self {
            key: e.key.clone(),
            prefix: e.prefix.clone(),
            original_command: e.original_command.clone(),
        }
    }
}

impl From<ProbeEntryToml> for ProbeEntry {
    fn from(t: ProbeEntryToml) -> Self {
        Self {
            key: t.key,
            prefix: t.prefix,
            original_command: t.original_command,
        }
    }
}

/// Port for reading and writing candidate probes.
pub trait ProbeStore {
    fn load(&self) -> Vec<ProbeEntry>;
    fn write(&self, entries: &[ProbeEntry]);
    fn remove_matching(&self, cmd: &str);
}

/// File-backed probe store at `.ctx/candidates.toml`.
pub struct FileProbeStore {
    pub path: std::path::PathBuf,
}

impl FileProbeStore {
    pub fn default_path() -> std::path::PathBuf {
        std::env::var_os("CRS_CTX_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(".ctx").to_path_buf())
            .join("candidates.toml")
    }

    pub fn load(&self) -> Vec<ProbeEntry> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        toml::from_str::<ProbeFile>(&content)
            .unwrap_or_default()
            .probes
            .into_iter()
            .map(ProbeEntry::from)
            .collect()
    }

    pub fn write(&self, entries: &[ProbeEntry]) {
        if let Some(parent) = self.path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "prefixe: warn: could not create directory {}: {e}",
                parent.display()
            );
            return;
        }
        let file = ProbeFile {
            probes: entries.iter().map(ProbeEntryToml::from).collect(),
        };
        match toml::to_string_pretty(&file) {
            Ok(serialized) => {
                if let Err(e) = std::fs::write(&self.path, &serialized) {
                    eprintln!(
                        "prefixe: warn: could not write candidates to {}: {e}",
                        self.path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("prefixe: warn: could not serialize candidates: {e}");
            }
        }
    }

    pub fn remove_matching(&self, cmd: &str) {
        let mut entries = self.load();
        let before = entries.len();
        entries.retain(|e| e.original_command != cmd);
        if entries.len() < before {
            self.write(&entries);
        }
    }
}

impl ProbeStore for FileProbeStore {
    fn load(&self) -> Vec<ProbeEntry> {
        self.load()
    }
    fn write(&self, entries: &[ProbeEntry]) {
        self.write(entries);
    }
    fn remove_matching(&self, cmd: &str) {
        self.remove_matching(cmd);
    }
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

#[cfg(test)]
pub struct FakePrefixStore {
    pub config: PrefixConfig,
    pub written: std::cell::RefCell<Option<(String, Vec<String>)>>,
}

#[cfg(test)]
impl PrefixStore for FakePrefixStore {
    fn load(&self) -> PrefixConfig {
        self.config.clone()
    }
    fn confirm_mapping(&self, key: &str, prefix: &[String]) {
        *self.written.borrow_mut() = Some((key.to_string(), prefix.to_vec()));
    }
}

#[cfg(test)]
pub struct FakeProbeStore {
    pub entries: std::cell::RefCell<Vec<ProbeEntry>>,
}

#[cfg(test)]
impl ProbeStore for FakeProbeStore {
    fn load(&self) -> Vec<ProbeEntry> {
        self.entries.borrow().clone()
    }
    fn write(&self, entries: &[ProbeEntry]) {
        *self.entries.borrow_mut() = entries.to_vec();
    }
    fn remove_matching(&self, cmd: &str) {
        self.entries
            .borrow_mut()
            .retain(|e| e.original_command != cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store(mappings: &[(&str, &[&str])], candidates: &[&[&str]]) -> FakePrefixStore {
        FakePrefixStore {
            config: PrefixConfig {
                mappings: mappings
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                    .collect(),
                candidate_prefixes: candidates
                    .iter()
                    .map(|c| c.iter().map(|s| s.to_string()).collect())
                    .collect(),
                learn_on_successful_fallback: false,
            },
            written: std::cell::RefCell::new(None),
        }
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
    fn rewrite_candidate_records_probe() {
        let store = make_store(&[], &[&["op", "plugin", "run", "--"]]);
        let r = rewrite_command("gh issue list", &store.load());
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
        let store = FileProbeStore {
            path: dir.path().join("candidates.toml"),
        };
        let entries = vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            original_command: "gh issue list".to_string(),
        }];
        store.write(&entries);
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "gh");
    }

    #[test]
    fn probe_store_remove_matching() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FileProbeStore {
            path: dir.path().join("candidates.toml"),
        };
        store.write(&[
            ProbeEntry {
                key: "gh".to_string(),
                prefix: vec![],
                original_command: "gh issue list".to_string(),
            },
            ProbeEntry {
                key: "cargo".to_string(),
                prefix: vec![],
                original_command: "cargo build".to_string(),
            },
        ]);
        store.remove_matching("gh issue list");
        let remaining = store.load();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key, "cargo");
    }
}
