use crate::{
    AuditState, Error, PrefixStore, ProbeStore, RewriteResult, audit_state,
    domain::CommandRewriter, rewrite_command,
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

    /// Return a snapshot of current mappings and pending probes.
    pub fn audit(&self) -> AuditState {
        audit_state(&self.prefix_store, &self.probe_store)
    }

    /// Persist a confirmed prefix mapping.
    pub fn confirm(&self, key: &str, prefix: &[String]) -> Result<(), Error> {
        self.prefix_store.confirm_mapping(key, prefix)
    }

    /// Remove a confirmed mapping. Returns `true` if it existed.
    pub fn forget(&self, key: &str) -> Result<bool, Error> {
        self.prefix_store.remove_mapping(key)
    }
}

impl<P: PrefixStore, Q: ProbeStore> CommandRewriter for PrefixEngine<P, Q> {
    fn rewrite(&self, cmd: &str) -> RewriteResult {
        PrefixEngine::rewrite(self, cmd)
    }
}
