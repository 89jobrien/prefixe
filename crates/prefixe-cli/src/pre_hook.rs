use prefixe::{ProbeState, ProbeStore};

/// Check if `command` matches a Pending probe's expected retry command.
/// If it does, transitions the probe to `Probing` and returns `true`.
/// Returns `false` if no match (caller should proceed with normal pre-hook logic).
pub fn check_probe_match(command: &str, probe_store: &dyn ProbeStore) -> bool {
    let mut probes = probe_store.load();
    let cmd = command.trim();

    for probe in probes.iter_mut() {
        if probe.state != ProbeState::Pending {
            continue;
        }
        let expected = format!(
            "{} {}",
            probe.prefix.join(" "),
            probe.original_command.as_str()
        );
        if cmd == expected.trim() {
            probe.state = ProbeState::Probing;
            let _ = probe_store.write(&probes);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::testing::FakeProbeStore;
    use prefixe::{OriginalCommand, ProbeEntry, ProbeState, SuccessPredicate};

    fn pending_probe(original: &str, prefix: &[&str]) -> ProbeEntry {
        ProbeEntry {
            key: original.split_whitespace().next().unwrap_or("").to_string(),
            prefix: prefix.iter().map(|s| s.to_string()).collect(),
            success_when: SuccessPredicate::exit_zero(),
            original_command: OriginalCommand::from(original),
            state: ProbeState::Pending,
            candidate_index: 0,
        }
    }

    #[test]
    fn pending_probe_match_transitions_to_probing() {
        let probe_store = FakeProbeStore::with_entries(vec![pending_probe(
            "gh issue list",
            &["op", "plugin", "run", "--"],
        )]);

        let result = check_probe_match("op plugin run -- gh issue list", &probe_store);

        assert!(result, "should have matched pending probe");
        let probes = probe_store.load();
        assert_eq!(probes[0].state, ProbeState::Probing);
    }

    #[test]
    fn non_probe_command_returns_false() {
        let probe_store = FakeProbeStore::empty();
        let result = check_probe_match("gh issue list", &probe_store);
        assert!(!result);
    }

    #[test]
    fn already_probing_entry_not_re_matched() {
        let mut probe = pending_probe("gh issue list", &["op", "plugin", "run", "--"]);
        probe.state = ProbeState::Probing;
        let probe_store = FakeProbeStore::with_entries(vec![probe]);

        let result = check_probe_match("op plugin run -- gh issue list", &probe_store);
        assert!(!result);
    }
}
