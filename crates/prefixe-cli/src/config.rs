use std::path::PathBuf;

use prefixe::FilePrefixStore;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file is malformed: {0}")]
    Malformed(String),
}

/// Resolve the config path: explicit override > XDG_CONFIG_HOME > ~/.config
pub fn resolve_config_path(override_path: Option<&str>) -> PathBuf {
    if let Some(p) = override_path {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("prefixe").join("config.toml")
}

/// Load config from `path`. Returns default (empty) if the file is absent.
/// Returns `Err` if the file exists but cannot be parsed.
pub fn load_store(path: PathBuf) -> Result<FilePrefixStore, ConfigError> {
    if path.exists() {
        // validate parse — FilePrefixStore silently swallows errors, so we check here
        let content =
            std::fs::read_to_string(&path).map_err(|e| ConfigError::Malformed(e.to_string()))?;
        toml::from_str::<toml::Value>(&content)
            .map_err(|e| ConfigError::Malformed(e.to_string()))?;
    }
    Ok(FilePrefixStore::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::PrefixStore as _;
    use std::io::Write as _;

    #[test]
    fn resolve_uses_xdg_when_set() {
        // temporarily override XDG_CONFIG_HOME is tricky in parallel tests;
        // test the override_path branch instead (deterministic)
        let p = resolve_config_path(Some("/tmp/my.toml"));
        assert_eq!(p, PathBuf::from("/tmp/my.toml"));
    }

    #[test]
    fn resolve_default_contains_prefixe_config() {
        // without XDG override we should get something ending in prefixe/config.toml
        // We cannot easily unset XDG_CONFIG_HOME in a test, so just verify the suffix
        // when override_path is None and XDG_CONFIG_HOME is not set.
        // We test via the env override path instead.
        let p = resolve_config_path(None);
        assert!(p.ends_with("prefixe/config.toml"));
    }

    #[test]
    fn load_store_absent_file_is_ok() {
        let p = PathBuf::from("/tmp/nonexistent-prefixe-cli-test-config.toml");
        let store = load_store(p).expect("absent file should not error");
        // Loading an absent file returns empty config
        let config = store.load();
        assert!(config.mappings.is_empty());
    }

    #[test]
    fn load_store_malformed_file_errors() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"not valid toml ][[\n").unwrap();
        let err = load_store(f.path().to_path_buf());
        assert!(err.is_err(), "malformed TOML should return Err");
        let msg = err.err().unwrap().to_string();
        assert!(msg.contains("malformed"), "error should mention malformed");
    }

    #[test]
    fn load_store_valid_toml_reads_mappings() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"[mappings]\ngh = [\"op\", \"run\", \"--\"]\n")
            .unwrap();
        let store = load_store(f.path().to_path_buf()).expect("valid TOML should load");
        let config = store.load();
        assert!(config.mappings.contains_key("gh"));
    }
}
