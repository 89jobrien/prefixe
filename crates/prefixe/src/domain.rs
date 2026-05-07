use std::collections::HashMap;

/// Pure domain config — no serialization dependencies.
#[derive(Debug, Clone, Default)]
pub struct PrefixConfig {
    pub mappings: HashMap<String, Vec<String>>,
    pub candidate_prefixes: Vec<Vec<String>>,
    pub learn_on_successful_fallback: bool,
}

/// Newtype wrapper for an original (pre-rewrite) shell command string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OriginalCommand(pub String);

impl OriginalCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for OriginalCommand {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for OriginalCommand {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for OriginalCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
