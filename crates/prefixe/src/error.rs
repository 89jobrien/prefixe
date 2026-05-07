/// Crate-level error type for all fallible prefixe operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
}
