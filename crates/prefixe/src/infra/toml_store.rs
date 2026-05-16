use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    CandidatePrefix, CommandStats, Error, GlobalStats, PrefixCounters, PrefixStats, PrefixStore,
    ProbeEntry, ProbeState, ProbeStore, StatsStore, SuccessPredicate,
    domain::{OriginalCommand, PrefixConfig},
    infra::path::PathResolver,
};

// ── PrefixConfig TOML DTO ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SuccessPredicateDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_matches: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_matches: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stderr_absent: bool,
}

impl From<SuccessPredicateDto> for SuccessPredicate {
    fn from(dto: SuccessPredicateDto) -> Self {
        Self {
            exit_code: dto.exit_code,
            stdout_matches: dto.stdout_matches,
            stderr_matches: dto.stderr_matches,
            stderr_absent: dto.stderr_absent,
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
    fn from(dto: CandidatePrefixDto) -> Self {
        Self {
            prefix: dto.prefix,
            success_when: dto.success_when.into(),
        }
    }
}

impl From<&CandidatePrefix> for CandidatePrefixDto {
    fn from(c: &CandidatePrefix) -> Self {
        Self {
            prefix: c.prefix.clone(),
            success_when: SuccessPredicateDto::from(&c.success_when),
        }
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

// ── ProbeEntry TOML DTO ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProbeEntryToml {
    pub key: String,
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicateDto,
    pub original_command: String,
    pub state: String, // "Pending" | "Probing"
    pub candidate_index: usize,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct ProbeFile {
    #[serde(default)]
    pub probes: Vec<ProbeEntryToml>,
}

impl From<&ProbeEntry> for ProbeEntryToml {
    fn from(e: &ProbeEntry) -> Self {
        Self {
            key: e.key.clone(),
            prefix: e.prefix.clone(),
            success_when: SuccessPredicateDto::from(&e.success_when),
            original_command: e.original_command.0.clone(),
            state: match e.state {
                ProbeState::Pending => "Pending".to_string(),
                ProbeState::Probing => "Probing".to_string(),
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
            state: if t.state == "Probing" {
                ProbeState::Probing
            } else {
                ProbeState::Pending
            },
            candidate_index: t.candidate_index,
        }
    }
}

// ── FilePrefixStore ──────────────────────────────────────────────────────────

/// File-backed implementation reading `~/.config/rx/prefixes.toml`.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use prefixe::{FilePrefixStore, PrefixStore};
///
/// let store = FilePrefixStore::new(PathBuf::from("/tmp/nonexistent-prefixes.toml"));
/// // Missing file returns an empty config without panicking.
/// let config = store.load();
/// assert!(config.mappings.is_empty());
/// ```
pub struct FilePrefixStore {
    pub path: PathBuf,
}

impl FilePrefixStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn from_resolver(r: &dyn PathResolver) -> Self {
        Self::new(r.prefix_config_path())
    }

    pub fn default_path() -> PathBuf {
        std::env::var_os("CRS_RX_PREFIXES")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                    });
                base.join("rx").join("prefixes.toml")
            })
    }

    fn load_config(&self) -> PrefixConfig {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return PrefixConfig::default(),
        };
        match toml::from_str::<PrefixConfigDto>(&content) {
            Ok(dto) => dto.into(),
            Err(e) => {
                eprintln!(
                    "prefixe: warn: could not parse {}: {e}; using empty config",
                    self.path.display()
                );
                PrefixConfig::default()
            }
        }
    }

    fn write_config(&self, config: &PrefixConfig) -> Result<(), Error> {
        let dto = PrefixConfigDto::from(config);
        let serialized = toml::to_string_pretty(&dto)?;
        std::fs::write(&self.path, serialized)?;
        Ok(())
    }
}

impl PrefixStore for FilePrefixStore {
    fn load(&self) -> PrefixConfig {
        self.load_config()
    }

    fn confirm_mapping(&self, key: &str, prefix: &[String]) -> Result<(), Error> {
        let mut config = self.load_config();
        config.mappings.insert(key.to_string(), prefix.to_vec());
        self.write_config(&config)
    }

    fn remove_mapping(&self, key: &str) -> Result<bool, Error> {
        let mut config = self.load_config();
        if config.mappings.remove(key).is_none() {
            return Ok(false);
        }
        self.write_config(&config)?;
        Ok(true)
    }
}

// ── FileProbeStore ───────────────────────────────────────────────────────────

/// File-backed probe store at `.ctx/candidates.toml`.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use prefixe::{FileProbeStore, ProbeStore};
///
/// let store = FileProbeStore::new(PathBuf::from("/tmp/nonexistent-candidates.toml"));
/// // Missing file returns an empty list without panicking.
/// assert!(store.load().is_empty());
/// ```
pub struct FileProbeStore {
    pub path: PathBuf,
}

impl FileProbeStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn from_resolver(r: &dyn PathResolver) -> Self {
        Self::new(r.probe_store_path())
    }

    pub fn default_path() -> PathBuf {
        std::env::var_os("CRS_CTX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(".ctx").to_path_buf())
            .join("candidates.toml")
    }
}

impl ProbeStore for FileProbeStore {
    fn load(&self) -> Vec<ProbeEntry> {
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

    fn write(&self, entries: &[ProbeEntry]) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ProbeFile {
            probes: entries.iter().map(ProbeEntryToml::from).collect(),
        };
        let serialized = toml::to_string_pretty(&file)?;
        std::fs::write(&self.path, serialized)?;
        Ok(())
    }

    fn remove_matching(&self, cmd: &OriginalCommand) -> Result<(), Error> {
        let mut entries = self.load();
        let before = entries.len();
        entries.retain(|e| e.original_command != *cmd);
        if entries.len() < before {
            self.write(&entries)?;
        }
        Ok(())
    }
}

// ── Stats TOML DTOs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GlobalStatsDto {
    #[serde(default)]
    pub probes_initiated: u64,
    #[serde(default)]
    pub probes_confirmed: u64,
    #[serde(default)]
    pub probes_exhausted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PrefixCountersDto {
    #[serde(default)]
    pub tried: u64,
    #[serde(default)]
    pub confirmed: u64,
    #[serde(default)]
    pub failed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CommandStatsDto {
    #[serde(default)]
    pub probes_initiated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PrefixStatsDto {
    #[serde(default)]
    pub global: GlobalStatsDto,
    #[serde(default)]
    pub by_prefix: HashMap<String, PrefixCountersDto>,
    #[serde(default)]
    pub by_command: HashMap<String, CommandStatsDto>,
}

impl From<PrefixStatsDto> for PrefixStats {
    fn from(d: PrefixStatsDto) -> Self {
        Self {
            global: GlobalStats {
                probes_initiated: d.global.probes_initiated,
                probes_confirmed: d.global.probes_confirmed,
                probes_exhausted: d.global.probes_exhausted,
            },
            by_prefix: d
                .by_prefix
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        PrefixCounters {
                            tried: v.tried,
                            confirmed: v.confirmed,
                            failed: v.failed,
                        },
                    )
                })
                .collect(),
            by_command: d
                .by_command
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        CommandStats {
                            probes_initiated: v.probes_initiated,
                            confirmed_prefix: v.confirmed_prefix,
                            confirmed_at: v.confirmed_at,
                        },
                    )
                })
                .collect(),
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
            by_prefix: s
                .by_prefix
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        PrefixCountersDto {
                            tried: v.tried,
                            confirmed: v.confirmed,
                            failed: v.failed,
                        },
                    )
                })
                .collect(),
            by_command: s
                .by_command
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        CommandStatsDto {
                            probes_initiated: v.probes_initiated,
                            confirmed_prefix: v.confirmed_prefix.clone(),
                            confirmed_at: v.confirmed_at.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

// ── FileStatsStore ───────────────────────────────────────────────────────────

/// File-backed stats store using TOML at `~/.config/rx/prefix-stats.toml`.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use prefixe::{FileStatsStore, StatsStore};
///
/// let store = FileStatsStore::new(PathBuf::from("/tmp/nonexistent-stats.toml"));
/// let stats = store.load();
/// assert_eq!(stats.global.probes_initiated, 0);
/// ```
pub struct FileStatsStore {
    pub path: PathBuf,
}

impl FileStatsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProbeState, SuccessPredicate};
    use tempfile::TempDir;

    #[test]
    fn prefix_config_dto_round_trips_candidate_prefix() {
        let dir = TempDir::new().unwrap();
        let store = FilePrefixStore::new(dir.path().join("prefixes.toml"));
        std::fs::write(
            dir.path().join("prefixes.toml"),
            "[[candidate_prefixes]]\nprefix = [\"op\", \"plugin\", \"run\", \"--\"]\n\
             [candidate_prefixes.success_when]\nexit_code = 0\nstderr_absent = true\n",
        )
        .unwrap();
        let config = store.load();
        assert_eq!(config.candidate_prefixes.len(), 1);
        assert_eq!(
            config.candidate_prefixes[0].prefix,
            vec!["op", "plugin", "run", "--"]
        );
        assert_eq!(config.candidate_prefixes[0].success_when.exit_code, Some(0));
        assert!(config.candidate_prefixes[0].success_when.stderr_absent);
    }

    #[test]
    fn probe_entry_toml_round_trips_state_and_index() {
        let dir = TempDir::new().unwrap();
        let store = FileProbeStore::new(dir.path().join("candidates.toml"));
        let entries = vec![crate::ProbeEntry {
            key: "gh".to_string(),
            prefix: vec!["op".to_string()],
            success_when: SuccessPredicate::exit_zero(),
            original_command: crate::OriginalCommand::from("gh issue list"),
            state: ProbeState::Pending,
            candidate_index: 2,
        }];
        store.write(&entries).unwrap();
        let loaded = store.load();
        assert_eq!(loaded[0].state, ProbeState::Pending);
        assert_eq!(loaded[0].candidate_index, 2);
    }

    #[test]
    fn file_stats_store_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = FileStatsStore::new(dir.path().join("stats.toml"));
        let mut stats = PrefixStats::default();
        stats.global.probes_initiated = 5;
        stats.global.probes_confirmed = 2;
        stats.by_prefix.insert(
            "op plugin run --".to_string(),
            PrefixCounters {
                tried: 3,
                confirmed: 2,
                failed: 1,
            },
        );
        stats.by_command.insert(
            "gh".to_string(),
            CommandStats {
                probes_initiated: 3,
                confirmed_prefix: Some("op plugin run --".to_string()),
                confirmed_at: Some("2026-05-16T10:00:00Z".to_string()),
            },
        );
        store.save(&stats).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.global.probes_initiated, 5);
        assert_eq!(loaded.global.probes_confirmed, 2);
        assert_eq!(loaded.by_prefix["op plugin run --"].tried, 3);
        assert_eq!(
            loaded.by_command["gh"].confirmed_prefix.as_deref(),
            Some("op plugin run --")
        );
    }

    #[test]
    fn file_stats_store_missing_file_returns_default() {
        let store = FileStatsStore::new(std::path::PathBuf::from(
            "/tmp/nonexistent-stats-prefixe-abc.toml",
        ));
        let stats = store.load();
        assert_eq!(stats.global.probes_initiated, 0);
    }
}
