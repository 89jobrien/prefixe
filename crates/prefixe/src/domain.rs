use std::collections::HashMap;

/// A single condition that must hold for a [`PrefixRule`] to be evaluated.
///
/// All conditions in a rule are AND-combined.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleCondition {
    /// The named environment variable is set (non-empty value).
    EnvVarSet(String),
    /// The current working directory matches the given glob pattern.
    CwdGlob(String),
    /// The process is running inside a git repository (any `.git` ancestor).
    GitRoot,
}

/// A prefix rule with an optional key-specific prefix, conditions, and priority.
///
/// Rules are evaluated in descending priority order; the first matching rule
/// for a given command word wins.
#[derive(Debug, Clone)]
pub struct PrefixRule {
    /// The command word (or two-word phrase) this rule targets.
    pub key: String,
    /// Prefix tokens to prepend when this rule matches.
    pub prefix: Vec<String>,
    /// Conditions that must ALL be true for this rule to apply.
    /// An empty list means the rule always applies.
    pub conditions: Vec<RuleCondition>,
    /// Evaluation order: higher values are tried first. Default is `0`.
    pub priority: u32,
}

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

/// Port: strategy for rewriting a shell command with prefix injection.
pub trait CommandRewriter {
    fn rewrite(&self, cmd: &str) -> crate::RewriteResult;
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
