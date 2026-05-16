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

/// Per-candidate success predicate. Default: exit code 0, everything else permissive.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SuccessPredicate {
    /// Required exit code. `None` = any. Default: `Some(0)`.
    pub exit_code: Option<i32>,
    /// Regex stdout must match. `None` = not checked.
    pub stdout_matches: Option<String>,
    /// Regex stderr must match. `None` = not checked.
    pub stderr_matches: Option<String>,
    /// If `true`, stderr must be empty or absent.
    pub stderr_absent: bool,
}

impl SuccessPredicate {
    pub fn exit_zero() -> Self {
        Self {
            exit_code: Some(0),
            ..Default::default()
        }
    }
}

/// One entry in `candidate_prefixes`: the tokens to prepend and the predicate
/// used to decide whether the attempt succeeded.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidatePrefix {
    pub prefix: Vec<String>,
    pub success_when: SuccessPredicate,
}

/// Pure domain config — no serialization dependencies.
///
/// # Examples
///
/// ```
/// use prefixe::{CandidatePrefix, PrefixConfig, SuccessPredicate};
///
/// let config = PrefixConfig {
///     mappings: [("gh".to_string(), vec!["op".to_string(), "plugin".to_string(),
///                "run".to_string(), "--".to_string()])]
///         .into_iter()
///         .collect(),
///     candidate_prefixes: vec![CandidatePrefix {
///         prefix: vec!["op".to_string(), "plugin".to_string(), "run".to_string(),
///                      "--".to_string()],
///         success_when: SuccessPredicate::exit_zero(),
///     }],
/// };
/// assert!(config.mappings.contains_key("gh"));
/// assert_eq!(config.candidate_prefixes.len(), 1);
/// ```
#[derive(Debug, Clone, Default)]
pub struct PrefixConfig {
    pub mappings: HashMap<String, Vec<String>>,
    /// Ordered list of candidate prefixes to try when a command fails bare.
    pub candidate_prefixes: Vec<CandidatePrefix>,
}

/// Newtype wrapper for an original (pre-rewrite) shell command string.
///
/// # Examples
///
/// ```
/// use prefixe::OriginalCommand;
///
/// let cmd = OriginalCommand::from("gh issue list");
/// assert_eq!(cmd.as_str(), "gh issue list");
/// assert_eq!(cmd.to_string(), "gh issue list");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OriginalCommand(pub String);

impl OriginalCommand {
    /// Return the command string as a `&str`.
    ///
    /// # Examples
    ///
    /// ```
    /// use prefixe::OriginalCommand;
    ///
    /// let cmd = OriginalCommand::from("cargo build");
    /// assert_eq!(cmd.as_str(), "cargo build");
    /// ```
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
///
/// Implement this trait to plug in custom rewriting strategies.  The
/// [`PrefixEngine`](crate::PrefixEngine) provides the standard implementation.
///
/// # Examples
///
/// ```
/// use prefixe::{CommandRewriter, RewriteResult};
///
/// struct NoOpRewriter;
///
/// impl CommandRewriter for NoOpRewriter {
///     fn rewrite(&self, cmd: &str) -> RewriteResult {
///         RewriteResult { rewritten: cmd.to_string(), probes: vec![] }
///     }
/// }
///
/// let r = NoOpRewriter;
/// assert_eq!(r.rewrite("echo hi").rewritten, "echo hi");
/// ```
pub trait CommandRewriter {
    fn rewrite(&self, cmd: &str) -> crate::RewriteResult;
}

/// Port: strategy for splitting and rejoining compound shell commands.
///
/// Implement this trait to customise how compound commands are tokenised
/// before prefix injection.  The default implementation is [`TextualSplitter`].
///
/// # Examples
///
/// ```
/// use prefixe::{CommandSplitter, TextualSplitter};
///
/// let s = TextualSplitter;
/// let segs = s.split("a && b");
/// assert_eq!(segs.len(), 2);
/// assert_eq!(s.rejoin(&segs), "a && b");
/// ```
pub trait CommandSplitter {
    fn split(&self, cmd: &str) -> Vec<crate::Segment>;
    fn rejoin(&self, segs: &[crate::Segment]) -> String;
}

/// Default textual splitter — does not handle quoted separators.
///
/// Splits on `&&`, `||`, `;`, and `|` (textually, not shell-grammatically).
///
/// # Examples
///
/// ```
/// use prefixe::{CommandSplitter, TextualSplitter};
///
/// let s = TextualSplitter;
/// let segs = s.split("git add -A && git commit -m 'msg'");
/// assert_eq!(segs.len(), 2);
/// assert_eq!(segs[0].sep.as_deref(), Some("&&"));
/// ```
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_prefix_has_success_predicate() {
        let c = CandidatePrefix {
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            success_when: SuccessPredicate::exit_zero(),
        };
        assert_eq!(c.success_when.exit_code, Some(0));
        assert!(!c.success_when.stderr_absent);
    }

    #[test]
    fn prefix_config_uses_candidate_prefix_vec() {
        let config = PrefixConfig {
            mappings: HashMap::new(),
            candidate_prefixes: vec![CandidatePrefix {
                prefix: vec!["op".to_string()],
                success_when: SuccessPredicate::default(),
            }],
        };
        assert_eq!(config.candidate_prefixes.len(), 1);
    }
}
