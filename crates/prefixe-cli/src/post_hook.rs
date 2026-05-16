use prefixe::{OriginalCommand, PrefixStore, ProbeEntry, ProbeState, ProbeStore, StatsStore};

use crate::payload::PostHookOutput;
use crate::predicate::evaluate_predicate;

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

/// Handle the result of a Probing command.
///
/// Finds a `Probing` probe whose expected command matches `command`, evaluates the
/// success predicate, then either confirms the mapping, cycles to the next candidate,
/// or exhausts all candidates and notifies Claude.
pub fn handle_probe_result(
    command: &str,
    exit_code: i64,
    stdout: &str,
    stderr: &str,
    prefix_store: &dyn PrefixStore,
    probe_store: &dyn ProbeStore,
    stats_store: &dyn StatsStore,
) -> PostHookOutput {
    let mut probes = probe_store.load();
    let cmd = command.trim();

    let probe_idx = probes.iter().position(|p| {
        p.state == ProbeState::Probing
            && format!("{} {}", p.prefix.join(" "), p.original_command.as_str()).trim() == cmd
    });

    let Some(idx) = probe_idx else {
        return PostHookOutput::silent();
    };

    let probe = probes.remove(idx);
    let prefix_key = probe.prefix.join(" ");
    let config = prefix_store.load();

    let mut stats = stats_store.load();
    stats.by_prefix.entry(prefix_key.clone()).or_default().tried += 1;

    if evaluate_predicate(&probe.success_when, exit_code, stdout, stderr) {
        let _ = prefix_store.confirm_mapping(&probe.key, &probe.prefix);
        let _ = probe_store.write(&probes);

        stats.global.probes_confirmed += 1;
        stats
            .by_prefix
            .entry(prefix_key.clone())
            .or_default()
            .confirmed += 1;
        let cmd_stats = stats.by_command.entry(probe.key.clone()).or_default();
        cmd_stats.confirmed_prefix = Some(prefix_key);
        cmd_stats.confirmed_at = Some(now_iso8601());
        let _ = stats_store.save(&stats);

        return PostHookOutput::allow_with_message(format!(
            "Candidate `{}` succeeded for `{}` — mapping confirmed.",
            probe.prefix.join(" "),
            probe.original_command.as_str(),
        ));
    }

    stats.by_prefix.entry(prefix_key).or_default().failed += 1;

    let next_index = probe.candidate_index + 1;
    if next_index < config.candidate_prefixes.len() {
        let next = &config.candidate_prefixes[next_index];
        let prefixed = format!(
            "{} {}",
            next.prefix.join(" "),
            probe.original_command.as_str()
        );
        probes.push(ProbeEntry {
            key: probe.key,
            prefix: next.prefix.clone(),
            success_when: next.success_when.clone(),
            original_command: probe.original_command,
            state: ProbeState::Pending,
            candidate_index: next_index,
        });
        let _ = probe_store.write(&probes);
        let _ = stats_store.save(&stats);
        PostHookOutput::allow_with_message(format!("Candidate failed. Retry with: {prefixed}"))
    } else {
        let _ = probe_store.write(&probes);
        stats.global.probes_exhausted += 1;
        let _ = stats_store.save(&stats);
        PostHookOutput::allow_with_message(format!(
            "All candidates failed for `{}`. Add a confirmed mapping manually \
             or specify a prefix to try.",
            probe.original_command.as_str(),
        ))
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn unix_to_ymd_hms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let y = 1970 + days / 365;
    let yd = days % 365;
    let (mo, d) = month_day(yd, is_leap(y));
    (y, mo, d, h, m, s)
}

fn is_leap(y: u64) -> bool {
    y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
}

fn month_day(day_of_year: u64, leap: bool) -> (u64, u64) {
    let months = if leap {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut remaining = day_of_year;
    for (i, &days) in months.iter().enumerate() {
        if remaining < days {
            return ((i + 1) as u64, remaining + 1);
        }
        remaining -= days;
    }
    (12, 31)
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

    fn config_with_two_candidates() -> PrefixConfig {
        PrefixConfig {
            mappings: Default::default(),
            candidate_prefixes: vec![
                CandidatePrefix {
                    prefix: vec![
                        "op".to_string(),
                        "plugin".to_string(),
                        "run".to_string(),
                        "--".to_string(),
                    ],
                    success_when: SuccessPredicate::exit_zero(),
                },
                CandidatePrefix {
                    prefix: vec!["sudo".to_string()],
                    success_when: SuccessPredicate::exit_zero(),
                },
            ],
        }
    }

    #[test]
    fn probing_success_confirms_mapping_and_notifies() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Probing,
            candidate_index: 0,
        }]);
        let stats_store = FakeStatsStore::new();

        let result = handle_probe_result(
            "op plugin run -- gh issue list",
            0,
            "",
            "",
            &prefix_store,
            &probe_store,
            &stats_store,
        );

        assert!(
            result
                .system_message
                .as_deref()
                .unwrap_or("")
                .contains("succeeded")
        );
        assert_eq!(probe_store.load().len(), 0);

        let stats = stats_store.load();
        assert_eq!(stats.global.probes_confirmed, 1);
        assert_eq!(stats.by_prefix["op plugin run --"].confirmed, 1);
    }

    #[test]
    fn probing_failure_cycles_to_next_candidate() {
        let prefix_store = FakePrefixStore::new(config_with_two_candidates());
        let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Probing,
            candidate_index: 0,
        }]);
        let stats_store = FakeStatsStore::new();

        let result = handle_probe_result(
            "op plugin run -- gh issue list",
            1,
            "",
            "",
            &prefix_store,
            &probe_store,
            &stats_store,
        );

        assert!(
            result
                .system_message
                .as_deref()
                .unwrap_or("")
                .contains("sudo gh issue list")
        );
        let probes = probe_store.load();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].state, ProbeState::Pending);
        assert_eq!(probes[0].candidate_index, 1);

        let stats = stats_store.load();
        assert_eq!(stats.by_prefix["op plugin run --"].failed, 1);
    }

    #[test]
    fn probing_failure_exhausted_asks_user() {
        let prefix_store = FakePrefixStore::new(config_with_one_candidate());
        let probe_store = FakeProbeStore::with_entries(vec![ProbeEntry {
            key: "gh".to_string(),
            prefix: vec![
                "op".to_string(),
                "plugin".to_string(),
                "run".to_string(),
                "--".to_string(),
            ],
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from("gh issue list"),
            state: ProbeState::Probing,
            candidate_index: 0,
        }]);
        let stats_store = FakeStatsStore::new();

        let result = handle_probe_result(
            "op plugin run -- gh issue list",
            1,
            "",
            "",
            &prefix_store,
            &probe_store,
            &stats_store,
        );

        assert!(
            result
                .system_message
                .as_deref()
                .unwrap_or("")
                .contains("All candidates failed")
        );
        assert_eq!(probe_store.load().len(), 0);

        let stats = stats_store.load();
        assert_eq!(stats.global.probes_exhausted, 1);
    }
}
