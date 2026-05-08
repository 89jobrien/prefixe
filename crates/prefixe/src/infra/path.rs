use std::path::PathBuf;

/// Port: resolves file system paths for prefix config and probe store.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use prefixe::PathResolver;
///
/// struct FixedResolver;
///
/// impl PathResolver for FixedResolver {
///     fn prefix_config_path(&self) -> PathBuf { PathBuf::from("/tmp/prefixes.toml") }
///     fn probe_store_path(&self) -> PathBuf { PathBuf::from("/tmp/candidates.toml") }
/// }
///
/// let r = FixedResolver;
/// assert_eq!(r.prefix_config_path(), PathBuf::from("/tmp/prefixes.toml"));
/// ```
pub trait PathResolver {
    fn prefix_config_path(&self) -> PathBuf;
    fn probe_store_path(&self) -> PathBuf;
}

/// Reads paths from environment variables (production adapter).
///
/// Uses `CRS_RX_PREFIXES` for the prefix config and `CRS_CTX_DIR` for the
/// probe store directory, falling back to XDG/home-relative defaults.
///
/// # Examples
///
/// ```
/// use prefixe::{EnvPathResolver, PathResolver};
///
/// // Resolves from env or falls back to XDG defaults.
/// let r = EnvPathResolver;
/// let _ = r.prefix_config_path(); // does not panic
/// let _ = r.probe_store_path();
/// ```
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
