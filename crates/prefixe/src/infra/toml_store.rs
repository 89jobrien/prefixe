use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    Error, PrefixStore, ProbeEntry, ProbeStore,
    domain::{OriginalCommand, PrefixConfig},
    infra::path::PathResolver,
};

// ── PrefixConfig TOML DTO ────────────────────────────────────────────────────

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

// ── ProbeEntry TOML DTO ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProbeEntryToml {
    pub key: String,
    pub prefix: Vec<String>,
    pub original_command: String,
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
            original_command: e.original_command.0.clone(),
        }
    }
}

impl From<ProbeEntryToml> for ProbeEntry {
    fn from(t: ProbeEntryToml) -> Self {
        Self {
            key: t.key,
            prefix: t.prefix,
            original_command: OriginalCommand(t.original_command),
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
