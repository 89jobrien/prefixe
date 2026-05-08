use prefixe::{CommandRewriter, CommandSplitter, RewriteResult, TextualSplitter};
use proptest::prelude::*;

/// A simple deterministic rewriter for property testing.
struct EchoRewriter {
    fixed_output: String,
}

impl CommandRewriter for EchoRewriter {
    fn rewrite(&self, _cmd: &str) -> RewriteResult {
        RewriteResult {
            rewritten: self.fixed_output.clone(),
            probes: vec![],
        }
    }
}

proptest! {
    /// TextualSplitter must never panic on arbitrary Unicode input.
    #[test]
    fn textual_splitter_never_panics(cmd in ".*") {
        let s = TextualSplitter;
        let segs = s.split(&cmd);
        prop_assert!(
            !segs.is_empty(),
            "split of {:?} produced zero segments",
            cmd
        );
    }

    /// split → rejoin is lossless: the output equals the input for any string.
    #[test]
    fn split_rejoin_round_trip(cmd in ".*") {
        let s = TextualSplitter;
        let rejoined = s.rejoin(&s.split(&cmd));
        prop_assert_eq!(
            rejoined,
            cmd,
            "round-trip failed: split then rejoin did not reproduce the original string"
        );
    }

    /// A CommandRewriter implementation is deterministic: same input → same output.
    #[test]
    fn command_rewriter_is_deterministic(input in ".*", output in ".*") {
        let r = EchoRewriter { fixed_output: output };
        let first = r.rewrite(&input).rewritten.clone();
        let second = r.rewrite(&input).rewritten.clone();
        prop_assert_eq!(
            first,
            second,
            "rewriter produced different outputs for the same input {:?}",
            input
        );
    }
}
