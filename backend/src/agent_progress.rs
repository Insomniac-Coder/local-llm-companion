//! Cross-failure progress budget for a single agent run.
//!
//! Empty text, malformed actions, permission denials, failed tools and rejected
//! completion claims must not reset one another's retry counters. Only genuinely
//! new successful tool evidence (or a host-confirmed file change) resets this
//! budget. The normal whole-run iteration limit remains independently enforced.

use std::collections::{hash_map::DefaultHasher, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

pub struct ProgressGuard {
    attempts_without_progress: u32,
    max_attempts_without_progress: u32,
    successful_observations: HashSet<u64>,
    recent_observations: VecDeque<u64>,
}

impl ProgressGuard {
    pub fn new(max_attempts_without_progress: u32) -> Self {
        Self {
            attempts_without_progress: 0,
            max_attempts_without_progress: max_attempts_without_progress.clamp(1, 8),
            successful_observations: HashSet::new(),
            recent_observations: VecDeque::new(),
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
    /// of progress. Re-reading identical content cannot indefinitely reset it.
    pub fn observe_tool_result(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
        output: &str,
        succeeded: bool,
    ) -> bool {
        if !succeeded {
            return false;
        }
        self.record_success(tool, args, output)
    }

    pub fn record_success(&mut self, tool: &str, args: &serde_json::Value, output: &str) -> bool {
        let mut signature = DefaultHasher::new();
        tool.hash(&mut signature);
        args.to_string().hash(&mut signature);
        output.hash(&mut signature);
        let signature = signature.finish();
        if self.successful_observations.insert(signature) {
            self.recent_observations.push_back(signature);
            if self.recent_observations.len() > 64 {
                if let Some(oldest) = self.recent_observations.pop_front() {
                    self.successful_observations.remove(&oldest);
                }
            }
            self.attempts_without_progress = 0;
            true
        } else {
            false
        }
    }

    /// An actual before/after difference is progress even when the tool's
    /// success text repeats. Never call this for an unexecuted/proposed patch.
    pub fn confirmed_change(&mut self) {
        self.attempts_without_progress = 0;
    }

    pub fn attempts_without_progress(&self) -> u32 {
        self.attempts_without_progress
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
            assert!(!guard.observe_tool_result(
                "read_file",
                &json!({"path":"missing"}),
                outcome,
                false
            ));
        }
        assert_eq!(guard.attempts_without_progress(), 4);
        assert!(!guard.begin_attempt());
        assert!(
            !guard.begin_attempt(),
            "a stopped budget cannot silently restart"
        );
    }

    #[test]
    fn repeated_successful_reads_do_not_sustain_an_unproductive_loop() {
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"README.md"});
        assert!(guard.begin_attempt());
        assert!(guard.observe_tool_result("read_file", &args, "same contents", true));
        for _ in 0..4 {
            assert!(guard.begin_attempt());
            assert!(!guard.observe_tool_result("read_file", &args, "same contents", true));
        }
        assert!(!guard.begin_attempt());
    }

    #[test]
    fn useful_observation_or_actual_file_change_resets_the_shared_budget() {
        let mut guard = ProgressGuard::new(4);
        let args = json!({"path":"main.rs"});
        for _ in 0..3 {
            assert!(guard.begin_attempt());
        }
        assert!(guard.observe_tool_result("read_file", &args, "revision one", true));
        assert_eq!(guard.attempts_without_progress(), 0);
        for _ in 0..3 {
            assert!(guard.begin_attempt());
        }
        assert!(guard.observe_tool_result("read_file", &args, "revision two", true));
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
        assert!(!guard.observe_tool_result("read_file", &args, "not found", false));
        assert_eq!(guard.attempts_without_progress(), 1);
        assert!(guard.begin_attempt());
        assert!(guard.observe_tool_result("read_file", &args, "actual contents", true));
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

    #[test]
    fn fingerprint_memory_stays_bounded() {
        let mut guard = ProgressGuard::new(4);
        for index in 0..80 {
            assert!(guard.begin_attempt());
            assert!(guard.record_success(
                "read_file",
                &json!({"path":format!("file-{index}")}),
                "contents"
            ));
        }
        assert_eq!(guard.successful_observations.len(), 64);
        assert_eq!(guard.recent_observations.len(), 64);
        assert!(!guard.record_success("read_file", &json!({"path":"file-79"}), "contents"));
    }
}
