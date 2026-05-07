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
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
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

/// Explicit paths — use in tests and when paths are known at construction time.
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
