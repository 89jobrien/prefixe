use prefixe::{OriginalCommand, PrefixStore, ProbeEntry, ProbeState, ProbeStore, StatsStore};

use crate::payload::PostHookOutput;

/// Extract the leading command word from a shell command string.
fn command_key(cmd: &str) -> String {
    shell_words::split(cmd.trim())
        .ok()
        .and_then(|tokens| tokens.into_iter().next())
        .unwrap_or_else(|| cmd.trim().to_string())
}

/// Handle a bare command failure: write a Pending probe and return a systemMessage
/// telling Claude to retry with the first candidate prefix.
///
/// No-op (returns silent output) if no candidates are configured or a probe already exists.
pub fn handle_failure(
    command: &str,
    prefix_store: &dyn PrefixStore,
    probe_store: &dyn ProbeStore,
    stats_store: &dyn StatsStore,
) -> PostHookOutput {
    let config = prefix_store.load();
    if config.candidate_prefixes.is_empty() {
        return PostHookOutput::silent();
    }

    let key = command_key(command);
    let existing = probe_store.load();

    // Don't duplicate probes for the same command
    if existing
        .iter()
        .any(|p| p.original_command.as_str() == command)
    {
        return PostHookOutput::silent();
    }

    let candidate = &config.candidate_prefixes[0];
    let prefixed = format!("{} {}", candidate.prefix.join(" "), command);

    let mut probes = existing;
    probes.push(ProbeEntry {
        key: key.clone(),
        prefix: candidate.prefix.clone(),
        success_when: candidate.success_when.clone(),
        original_command: OriginalCommand::from(command),
        state: ProbeState::Pending,
        candidate_index: 0,
    });
    let _ = probe_store.write(&probes);

    // Update stats
    let mut stats = stats_store.load();
    stats.global.probes_initiated += 1;
    stats.by_command.entry(key).or_default().probes_initiated += 1;
    let _ = stats_store.save(&stats);

    PostHookOutput::allow_with_message(format!("Command failed. Retry with: {prefixed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::testing::{FakePrefixStore, FakeProbeStore, FakeStatsStore};
    use prefixe::{CandidatePrefix, OriginalCommand, PrefixConfig, ProbeState, SuccessPredicate};

    fn config_with_one_candidate() -> PrefixConfig {
        PrefixConfig {
            mappings: Default::default(),
            candidate_prefixes: vec![CandidatePrefix {
                prefix: vec![
                    "op".to_string(),
                    "plugin".to_string(),
                    "run".to_string(),
                    "--".to_string(),
                ],
                success_when: SuccessPredicate::exit_zero(),
            }],
        }
    }

    #[test]
    fn on_bare_failure_writes_pending_probe_and_returns_message() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::empty();
        let stats_store = FakeStatsStore::new();

        let result = handle_failure("gh issue list", &prefix_store, &probe_store, &stats_store);

        let probes = probe_store.load();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].state, ProbeState::Pending);
        assert_eq!(probes[0].candidate_index, 0);
        assert_eq!(probes[0].key, "gh");
        assert!(
            result
                .system_message
                .as_deref()
                .unwrap_or("")
                .contains("op plugin run -- gh issue list")
        );

        let stats = stats_store.load();
        assert_eq!(stats.global.probes_initiated, 1);
        assert_eq!(stats.by_command["gh"].probes_initiated, 1);
    }

    #[test]
    fn on_failure_with_existing_pending_probe_does_not_duplicate() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec!["op".to_string()],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Pending,
            candidate_index: 0,
        }]);
        let stats_store = FakeStatsStore::new();

        handle_failure("gh issue list", &prefix_store, &probe_store, &stats_store);

        assert_eq!(probe_store.load().len(), 1); // no duplicate
    }

    #[test]
    fn on_failure_with_no_candidates_returns_silent() {
        let prefix_store = FakePrefixStore::new(PrefixConfig::default());
        let probe_store = FakeProbeStore::empty();
        let stats_store = FakeStatsStore::new();

        let result = handle_failure("grep foo .", &prefix_store, &probe_store, &stats_store);
        assert!(result.system_message.is_none());
    }
}
