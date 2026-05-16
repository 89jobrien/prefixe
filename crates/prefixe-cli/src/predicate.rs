#![allow(dead_code)] // used in Tasks 9-11 (post_hook, pre_hook)

use prefixe::SuccessPredicate;
use regex::Regex;

/// Evaluate a `SuccessPredicate` against the result of running a command.
///
/// Returns `true` if all configured conditions pass.
pub fn evaluate_predicate(
    pred: &SuccessPredicate,
    exit_code: i64,
    stdout: &str,
    stderr: &str,
) -> bool {
    if pred
        .exit_code
        .is_some_and(|required| exit_code != required as i64)
    {
        return false;
    }
    if pred.stderr_absent && !stderr.trim().is_empty() {
        return false;
    }
    if pred
        .stdout_matches
        .as_deref()
        .is_some_and(|p| !Regex::new(p).map(|re| re.is_match(stdout)).unwrap_or(false))
    {
        return false;
    }
    if pred
        .stderr_matches
        .as_deref()
        .is_some_and(|p| !Regex::new(p).map(|re| re.is_match(stderr)).unwrap_or(false))
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefixe::SuccessPredicate;

    #[test]
    fn evaluate_exit_zero_passes() {
        let pred = SuccessPredicate::exit_zero();
        assert!(evaluate_predicate(&pred, 0, "", ""));
    }

    #[test]
    fn evaluate_exit_zero_fails_on_nonzero() {
        let pred = SuccessPredicate::exit_zero();
        assert!(!evaluate_predicate(&pred, 1, "", ""));
    }

    #[test]
    fn evaluate_stderr_absent_fails_when_stderr_present() {
        let pred = SuccessPredicate {
            exit_code: Some(0),
            stderr_absent: true,
            ..Default::default()
        };
        assert!(!evaluate_predicate(&pred, 0, "", "some error"));
    }

    #[test]
    fn evaluate_stdout_matches_regex() {
        let pred = SuccessPredicate {
            exit_code: Some(0),
            stdout_matches: Some(r"issue \d+".to_string()),
            ..Default::default()
        };
        assert!(evaluate_predicate(&pred, 0, "issue 42 opened", ""));
        assert!(!evaluate_predicate(&pred, 0, "no match here", ""));
    }

    #[test]
    fn evaluate_stderr_matches_regex() {
        let pred = SuccessPredicate {
            exit_code: Some(0),
            stderr_matches: Some(r"^$".to_string()),
            ..Default::default()
        };
        assert!(evaluate_predicate(&pred, 0, "", ""));
        assert!(!evaluate_predicate(&pred, 0, "", "error occurred"));
    }
}
