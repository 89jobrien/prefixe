use crate::{
    AuditState, Error, PrefixStore, ProbeStore, RewriteResult, audit_state,
    domain::{CommandRewriter, PrefixRule, RuleCondition},
    rejoin, rewrite_command, split_segments,
};

/// Encapsulates `PrefixStore` + `ProbeStore` and exposes the full use-case API.
///
/// This is the primary entry point for downstream code — wire it in your
/// composition root with concrete adapters and use it everywhere else via the
/// `CommandRewriter` trait or the inherent methods.
///
/// # Examples
///
/// ```
/// use prefixe::{
///     Error, OriginalCommand, PrefixConfig, PrefixEngine, PrefixStore,
///     ProbeEntry, ProbeStore,
/// };
///
/// struct EmptyPrefixStore;
/// impl PrefixStore for EmptyPrefixStore {
///     fn load(&self) -> PrefixConfig { PrefixConfig::default() }
///     fn confirm_mapping(&self, _: &str, _: &[String]) -> Result<(), Error> { Ok(()) }
///     fn remove_mapping(&self, _: &str) -> Result<bool, Error> { Ok(false) }
/// }
///
/// struct EmptyProbeStore;
/// impl ProbeStore for EmptyProbeStore {
///     fn load(&self) -> Vec<ProbeEntry> { vec![] }
///     fn write(&self, _: &[ProbeEntry]) -> Result<(), Error> { Ok(()) }
///     fn remove_matching(&self, _: &OriginalCommand) -> Result<(), Error> { Ok(()) }
/// }
///
/// let engine = PrefixEngine::new(EmptyPrefixStore, EmptyProbeStore);
/// let result = engine.rewrite("echo hi");
/// assert_eq!(result.rewritten, "echo hi");
/// ```
pub struct PrefixEngine<P: PrefixStore, Q: ProbeStore> {
    prefix_store: P,
    // probe_store is accessed directly by the post-hook CLI, not through the engine
    #[allow(dead_code)]
    probe_store: Q,
}

impl<P: PrefixStore, Q: ProbeStore> PrefixEngine<P, Q> {
    /// Construct a new engine from a prefix store and a probe store.
    ///
    /// # Examples
    ///
    /// ```
    /// use prefixe::{
    ///     Error, OriginalCommand, PrefixConfig, PrefixEngine, PrefixStore,
    ///     ProbeEntry, ProbeStore,
    /// };
    ///
    /// struct EmptyPrefixStore;
    /// impl PrefixStore for EmptyPrefixStore {
    ///     fn load(&self) -> PrefixConfig { PrefixConfig::default() }
    ///     fn confirm_mapping(&self, _: &str, _: &[String]) -> Result<(), Error> { Ok(()) }
    ///     fn remove_mapping(&self, _: &str) -> Result<bool, Error> { Ok(false) }
    /// }
    ///
    /// struct EmptyProbeStore;
    /// impl ProbeStore for EmptyProbeStore {
    ///     fn load(&self) -> Vec<ProbeEntry> { vec![] }
    ///     fn write(&self, _: &[ProbeEntry]) -> Result<(), Error> { Ok(()) }
    ///     fn remove_matching(&self, _: &OriginalCommand) -> Result<(), Error> { Ok(()) }
    /// }
    ///
    /// let _engine = PrefixEngine::new(EmptyPrefixStore, EmptyProbeStore);
    /// ```
    pub fn new(prefix_store: P, probe_store: Q) -> Self {
        Self {
            prefix_store,
            probe_store,
        }
    }

    /// Rewrite `cmd` with learned prefixes injected into each shell segment.
    pub fn rewrite(&self, cmd: &str) -> RewriteResult {
        let config = self.prefix_store.load();
        rewrite_command(cmd, &config)
    }

    /// Return a snapshot of current confirmed mappings.
    pub fn audit(&self) -> AuditState {
        audit_state(&self.prefix_store)
    }

    /// Persist a confirmed prefix mapping.
    pub fn confirm(&self, key: &str, prefix: &[String]) -> Result<(), Error> {
        self.prefix_store.confirm_mapping(key, prefix)
    }

    /// Remove a confirmed mapping. Returns `true` if it existed.
    pub fn forget(&self, key: &str) -> Result<bool, Error> {
        self.prefix_store.remove_mapping(key)
    }

    /// Evaluate a slice of conditions (AND semantics). Returns `true` if all
    /// conditions hold, or the slice is empty.
    pub fn evaluate_conditions(&self, conditions: &[RuleCondition]) -> bool {
        conditions.iter().all(|c| match c {
            RuleCondition::EnvVarSet(var) => std::env::var_os(var).is_some(),
            RuleCondition::CwdGlob(pattern) => {
                let Ok(cwd) = std::env::current_dir() else {
                    return false;
                };
                let cwd_str = cwd.to_string_lossy();
                glob::Pattern::new(pattern)
                    .map(|p| p.matches(&cwd_str))
                    .unwrap_or(false)
            }
            RuleCondition::GitRoot => {
                let Ok(cwd) = std::env::current_dir() else {
                    return false;
                };
                let mut dir = cwd.as_path();
                loop {
                    if dir.join(".git").exists() {
                        return true;
                    }
                    match dir.parent() {
                        Some(p) => dir = p,
                        None => return false,
                    }
                }
            }
        })
    }

    /// Rewrite `cmd` using an explicit, ordered list of [`PrefixRule`]s.
    ///
    /// Rules are sorted by descending priority (stable sort preserves definition
    /// order for ties). The first rule whose `key` matches the leading word(s)
    /// of a segment *and* whose conditions all pass is applied; no further rules
    /// for that segment are tried.
    pub fn rewrite_with_rules(&self, cmd: &str, rules: &[PrefixRule]) -> RewriteResult {
        // Stable sort: higher priority first, ties keep original order.
        let mut sorted: Vec<&PrefixRule> = rules.iter().collect();
        sorted.sort_by_key(|rule| std::cmp::Reverse(rule.priority));

        let mut segs = split_segments(cmd);

        for seg in &mut segs {
            let trimmed = seg.text.trim();
            if trimmed.is_empty() || trimmed.contains("$(") || trimmed.contains('`') {
                continue;
            }
            let tokens = match shell_words::split(trimmed) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let first = match tokens.first() {
                Some(t) => t.as_str(),
                None => continue,
            };
            let second = tokens.get(1).map(|s| s.as_str());

            // Find first matching rule.
            let matched = sorted.iter().find(|r| {
                let key_matches = if let Some(sec) = second {
                    let two = format!("{first} {sec}");
                    r.key == two || r.key == first
                } else {
                    r.key == first
                };
                key_matches && self.evaluate_conditions(&r.conditions)
            });

            if let Some(rule) = matched {
                let leading_len = seg.text.len() - seg.text.trim_start().len();
                let leading = &seg.text[..leading_len];
                let trailing_start = leading_len + trimmed.len();
                let trailing = &seg.text[trailing_start..];
                let prefix_str = rule.prefix.join(" ");
                seg.text = format!("{leading}{prefix_str} {trimmed}{trailing}");
            }
        }

        RewriteResult {
            rewritten: rejoin(&segs),
        }
    }
}

impl<P: PrefixStore, Q: ProbeStore> CommandRewriter for PrefixEngine<P, Q> {
    fn rewrite(&self, cmd: &str) -> RewriteResult {
        PrefixEngine::rewrite(self, cmd)
    }
}
