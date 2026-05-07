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

/// Port: strategy for splitting and rejoining compound shell commands.
pub trait CommandSplitter {
    fn split(&self, cmd: &str) -> Vec<crate::Segment>;
    fn rejoin(&self, segs: &[crate::Segment]) -> String;
}

/// Default textual splitter — does not handle quoted separators.
pub struct TextualSplitter;

impl CommandSplitter for TextualSplitter {
    fn split(&self, cmd: &str) -> Vec<crate::Segment> {
        crate::split_segments(cmd)
    }

    fn rejoin(&self, segs: &[crate::Segment]) -> String {
        crate::rejoin(segs)
    }
}

impl std::fmt::Display for OriginalCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
