use std::collections::HashMap;

/// Pure domain config — no serialization dependencies.
///
/// # Examples
///
/// ```
/// use prefixe::PrefixConfig;
///
/// let config = PrefixConfig {
///     mappings: [("gh".to_string(), vec!["op".to_string(), "run".to_string(), "--".to_string()])]
///         .into_iter()
///         .collect(),
///     candidate_prefixes: vec![],
///     learn_on_successful_fallback: false,
/// };
/// assert!(config.mappings.contains_key("gh"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct PrefixConfig {
    pub mappings: HashMap<String, Vec<String>>,
    pub candidate_prefixes: Vec<Vec<String>>,
    pub learn_on_successful_fallback: bool,
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
