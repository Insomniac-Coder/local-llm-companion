//! Cross-failure progress budget for a single agent run.
//!
//! Empty text, malformed actions, permission denials, failed tools and rejected
//! completion claims must not reset one another's retry counters: only a tool
//! the host actually executed successfully does. A successful re-read that
//! returns the same content is still a successful action (verification is
//! legitimate work), but a run that keeps repeating the identical action with
//! the identical result is looping and is stopped separately. The whole-run
//! iteration limit remains independently enforced.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Identical (tool, args, output) triples in a row that end the run.
pub const MAX_IDENTICAL_REPEATS: u32 = 5;
/// Identical repeats before the model is told it is repeating itself.
pub const REPEAT_WARNING: u32 = 2;

pub struct ProgressGuard {
    attempts_without_progress: u32,
    max_attempts_without_progress: u32,
    last_success: Option<u64>,
    identical_repeats: u32,
}

/// Outcome of observing one executed tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// A successful action with a result the run has not seen just before.
    NewEvidence,
    /// The same action returned the same result as the previous success.
    /// `warn` is true when the model should be told; `stop` when the run is
    /// clearly looping.
    Repeated { warn: bool, stop: bool },
    /// The tool failed, was denied or was invalid: no progress.
    NoProgress,
}

impl Observation {
    pub fn is_progress(self) -> bool {
        !matches!(self, Observation::NoProgress)
    }
}

impl ProgressGuard {
    pub fn new(max_attempts_without_progress: u32) -> Self {
        Self {
            attempts_without_progress: 0,
            max_attempts_without_progress: max_attempts_without_progress.clamp(1, 8),
            last_success: None,
            identical_repeats: 0,
        }
    }

    /// Call immediately before each primary model request, not while waiting
    /// for user approval. False means stop now without spending another request.
    pub fn begin_attempt(&mut self) -> bool {
        if self.attempts_without_progress >= self.max_attempts_without_progress {
            return false;
        }
        self.attempts_without_progress += 1;
        true
    }

    /// Call only after the host has executed a tool and observed its result.
    /// Parsing an action, prose, error messages and proposed diffs are not proof
    /// of progress; a successful execution is, even when it repeats a result.
    pub fn observe_tool_result(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
        output: &str,
        succeeded: bool,
    ) -> Observation {
        if !succeeded {
            return Observation::NoProgress;
        }
        self.record_success(tool, args, output)
    }

    pub fn record_success(&mut self, tool: &str, args: &serde_json::Value, output: &str) -> Observation {
        let mut signature = DefaultHasher::new();
        tool.hash(&mut signature);
        args.to_string().hash(&mut signature);
        output.hash(&mut signature);
        let signature = signature.finish();
        // Any successful execution is progress against the failure budget.
        self.attempts_without_progress = 0;
        if self.last_success == Some(signature) {
            self.identical_repeats += 1;
            Observation::Repeated {
                warn: self.identical_repeats >= REPEAT_WARNING,
                stop: self.identical_repeats >= MAX_IDENTICAL_REPEATS,
            }
        } else {
            self.last_success = Some(signature);
            self.identical_repeats = 0;
            Observation::NewEvidence
        }
    }

    /// An actual before/after difference is progress even when the tool's
    /// success text repeats. Never call this for an unexecuted/proposed patch.
    pub fn confirmed_change(&mut self) {
        self.attempts_without_progress = 0;
        self.identical_repeats = 0;
    }

    pub fn attempts_without_progress(&self) -> u32 {
        self.attempts_without_progress
    }

    pub fn identical_repeats(&self) -> u32 {
        self.identical_repeats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn alternating_empty_malformed_rejected_and_failed_attempts_share_one_budget() {
        let mut guard = ProgressGuard::new(4);
        for outcome in ["empty", "malformed", "permission denied", "failed tool"] {
            assert!(guard.begin_attempt(), "{outcome}");
            // None of these outcomes has a successful host observation.
            assert_eq!(
                guard.observe_tool_result("read_file", &json!({"path":"missing"}), outcome, false),
                Observation::NoProgress
            );
        }
        assert_eq!(guard.attempts_without_progress(), 4);
        assert!(!guard.begin_attempt());
        assert!(
            !guard.begin_attempt(),
            "a stopped budget cannot silently restart"
        );
    }

    #[test]
    fn verification_rereads_are_progress_but_identical_loops_are_stopped() {
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"README.md"});
        assert!(guard.begin_attempt());
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "same contents", true),
            Observation::NewEvidence
        );
        // A second identical read is legitimate verification: no failure budget
        // is consumed, but the model is warned on the repeat after that.
        assert!(guard.begin_attempt());
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "same contents", true),
            Observation::Repeated { warn: false, stop: false }
        );
        assert_eq!(guard.attempts_without_progress(), 0);
        assert!(guard.begin_attempt());
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "same contents", true),
            Observation::Repeated { warn: true, stop: false }
        );
        for _ in 0..(MAX_IDENTICAL_REPEATS - 3) {
            assert!(guard.begin_attempt());
            assert!(!matches!(
                guard.observe_tool_result("read_file", &args, "same contents", true),
                Observation::Repeated { stop: true, .. }
            ));
        }
        assert!(guard.begin_attempt());
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "same contents", true),
            Observation::Repeated { warn: true, stop: true }
        );
        // A different result breaks the streak.
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "changed contents", true),
            Observation::NewEvidence
        );
        assert_eq!(guard.identical_repeats(), 0);
    }

    #[test]
    fn a_completed_task_is_not_failed_by_a_strict_reviewer() {
        // Final answer (no tool) -> reviewer bounces -> verification read ->
        // final answer -> reviewer bounces: none of that is a failure streak.
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"calculator.js"});
        for round in 0..6 {
            assert!(guard.begin_attempt(), "round {round}");
            if round % 2 == 1 {
                assert!(guard
                    .observe_tool_result("read_file", &args, "fixed contents", true)
                    .is_progress());
            }
        }
        assert!(guard.begin_attempt());
    }

    #[test]
    fn useful_observation_or_actual_file_change_resets_the_shared_budget() {
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"main.rs"});
        for _ in 0..3 {
            assert!(guard.begin_attempt());
        }
        assert!(guard
            .observe_tool_result("read_file", &args, "revision one", true)
            .is_progress());
        assert_eq!(guard.attempts_without_progress(), 0);
        for _ in 0..3 {
            assert!(guard.begin_attempt());
        }
        guard.confirmed_change();
        assert_eq!(guard.attempts_without_progress(), 0);
        assert!(guard.begin_attempt());
    }

    #[test]
    fn a_correction_can_succeed_after_prior_failure_without_resetting_on_the_failure() {
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"main.rs"});
        assert!(guard.begin_attempt());
        assert_eq!(
            guard.observe_tool_result("read_file", &args, "not found", false),
            Observation::NoProgress
        );
        assert_eq!(guard.attempts_without_progress(), 1);
        assert!(guard.begin_attempt());
        assert!(guard
            .observe_tool_result("read_file", &args, "actual contents", true)
            .is_progress());
        assert_eq!(guard.attempts_without_progress(), 0);
    }

    #[test]
    fn invalid_configuration_cannot_disable_the_guard() {
        let mut zero = ProgressGuard::new(0);
        assert!(zero.begin_attempt());
        assert!(!zero.begin_attempt());
        let mut unlimited = ProgressGuard::new(u32::MAX);
        for _ in 0..8 {
            assert!(unlimited.begin_attempt());
        }
        assert!(!unlimited.begin_attempt());
    }
}
