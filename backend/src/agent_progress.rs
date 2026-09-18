//! Cross-failure progress budget for a single agent run.
//!
//! Empty text, malformed actions, permission denials, failed tools and rejected
//! completion claims must not reset one another's retry counters: only a tool
//! the host actually executed successfully does. A successful re-read that
//! returns the same content is still a successful action (verification is
//! legitimate work), but a run that keeps repeating the identical action with
//! the identical result is looping and is stopped separately. The loop watch
//! below judges the run as a whole rather than one step against the last.

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

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts_without_progress
    }

    /// Give back the attempt just begun when it was lost to the model server
    /// rather than to the model (a context overflow answered by compaction):
    /// it says nothing about whether the model's work is going anywhere.
    pub fn refund_attempt(&mut self) {
        self.attempts_without_progress = self.attempts_without_progress.saturating_sub(1);
    }

    pub fn identical_repeats(&self) -> u32 {
        self.identical_repeats
    }
}

/// Repetition across a whole run, not only from one step to the next.
///
/// `ProgressGuard` compares a result with the one immediately before it, which
/// catches a stutter and nothing else. A run that rotates through fourteen
/// files matches nothing consecutively: one such run read for 43 steps,
/// changed no file, repeated 20 of its 43 calls, and was still going when the
/// user stopped it (night decision 61). This watches the run instead of the
/// step: how long since anything changed, and how much of what it is doing it
/// has done before.
pub struct LoopWatch {
    seen: std::collections::HashSet<u64>,
    steps_without_change: u32,
    revisits: u32,
    warned: bool,
    changes_expected: bool,
    rewriting: Option<String>,
    rewrites: u32,
    last_written: Option<u64>,
    identical_rewrites: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopVerdict {
    /// Going somewhere, or too early to say.
    Working,
    /// Going over old ground: tell the model, once.
    Circling,
    /// Going nowhere: stop the run and keep what it did.
    Stuck,
}

impl LoopWatch {
    /// Steps of inspection with nothing changed before the model is told. A
    /// large task genuinely reads a lot before its first write, so this is
    /// generous; it is the repeats that make it a loop.
    pub const WARN_STEPS: u32 = 8;
    const WARN_REVISITS: u32 = 3;
    /// ... and before the run ends. There is no step ceiling above this any
    /// more (owner, 2026-09-18): what the run is doing is what ends it.
    pub const STOP_STEPS: u32 = 18;
    const STOP_REVISITS: u32 = 6;
    /// Writes of one file with nothing run against it in between before the
    /// model is told to check what it has written. Rewriting the same file
    /// over and over without ever building or opening it is the other way to
    /// make no progress while looking busy.
    const WARN_REWRITES: u32 = 4;
    /// Writes of one file with *the same text* and nothing run in between
    /// before the run ends. Only a write that changes nothing proves the run
    /// is spinning: a model working through a long file writes it in parts on
    /// purpose (the instructions tell it to), and different text each time may
    /// be converging on something, which only a build can settle.
    const STOP_IDENTICAL_REWRITES: u32 = 3;

    /// Inspection alone, however fresh it looks, is not the task: a run asked
    /// to build something that has changed nothing for this long, and has
    /// gone over old ground at least twice, has stopped working on it. Work
    /// that is genuinely new every step is not stopped here at all, because a
    /// large project legitimately reads a great deal.
    pub const STOP_WITHOUT_CHANGE: u32 = 30;

    /// `changes_expected` is false for a run that may not write anything (a
    /// plan or a question): there, only repetition can prove a loop.
    pub fn new(changes_expected: bool) -> Self {
        Self {
            seen: std::collections::HashSet::new(),
            steps_without_change: 0,
            revisits: 0,
            warned: false,
            changes_expected,
            rewriting: None,
            rewrites: 0,
            last_written: None,
            identical_rewrites: 0,
        }
    }

    /// One executed tool. `changed` means the workspace actually moved: a
    /// successful write, edit, delete or command, never a read.
    pub fn observe(&mut self, tool: &str, args: &serde_json::Value, changed: bool) -> LoopVerdict {
        let mut signature = DefaultHasher::new();
        tool.hash(&mut signature);
        args.to_string().hash(&mut signature);
        let signature = signature.finish();
        let path = args.get("path").and_then(|path| path.as_str()).unwrap_or("");
        match (tool, changed) {
            // Anything that produces evidence about the code clears the count:
            // a build, a test, a page opened.
            ("execute_command", true) | ("preview_page", _) => {
                self.rewriting = None;
                self.rewrites = 0;
                self.last_written = None;
                self.identical_rewrites = 0;
            }
            // Appending is how a long file is built in parts: never churn.
            ("write_file" | "edit_file", true) if !path.is_empty() => {
                let mut written = DefaultHasher::new();
                for field in ["content", "new", "patch"] {
                    if let Some(text) = args.get(field).and_then(|value| value.as_str()) {
                        text.hash(&mut written);
                    }
                }
                let written = written.finish();
                if self.rewriting.as_deref() == Some(path) {
                    self.rewrites += 1;
                    if self.last_written == Some(written) {
                        self.identical_rewrites += 1;
                    } else {
                        self.identical_rewrites = 1;
                    }
                } else {
                    self.rewriting = Some(path.to_string());
                    self.rewrites = 1;
                    self.identical_rewrites = 1;
                }
                self.last_written = Some(written);
            }
            _ => {}
        }
        if self.identical_rewrites >= Self::STOP_IDENTICAL_REWRITES {
            return LoopVerdict::Stuck;
        }
        if self.rewrites == Self::WARN_REWRITES {
            return LoopVerdict::Circling;
        }
        if changed {
            self.seen.clear();
            self.seen.insert(signature);
            self.steps_without_change = 0;
            self.revisits = 0;
            self.warned = false;
            return LoopVerdict::Working;
        }
        self.steps_without_change += 1;
        if !self.seen.insert(signature) {
            self.revisits += 1;
        }
        let stuck = (self.steps_without_change >= Self::STOP_STEPS
            && self.revisits >= Self::STOP_REVISITS)
            || (self.changes_expected
                && self.steps_without_change >= Self::STOP_WITHOUT_CHANGE
                && self.revisits >= 2);
        if stuck {
            return LoopVerdict::Stuck;
        }
        if !self.warned
            && self.steps_without_change >= Self::WARN_STEPS
            && self.revisits >= Self::WARN_REVISITS
        {
            self.warned = true;
            return LoopVerdict::Circling;
        }
        LoopVerdict::Working
    }

    pub fn steps_without_change(&self) -> u32 {
        self.steps_without_change
    }

    pub fn revisits(&self) -> u32 {
        self.revisits
    }

    /// The file being written over and over with nothing run against it, and
    /// how many times, once that is what the run is doing.
    pub fn rewriting(&self) -> Option<(&str, u32)> {
        (self.rewrites >= Self::WARN_REWRITES)
            .then(|| self.rewriting.as_deref().map(|path| (path, self.rewrites)))
            .flatten()
    }

    /// The file written with the same text again and again, and how many
    /// times, once that is what ends the run.
    pub fn rewriting_identically(&self) -> Option<(&str, u32)> {
        (self.identical_rewrites >= Self::STOP_IDENTICAL_REWRITES)
            .then(|| {
                self.rewriting
                    .as_deref()
                    .map(|path| (path, self.identical_rewrites))
            })
            .flatten()
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
    fn reading_the_same_files_in_rotation_is_caught_even_though_no_two_are_alike() {
        // The shape of the run that read for 43 steps and changed nothing:
        // a cycle of files, never the same call twice in a row.
        let mut watch = LoopWatch::new(true);
        let files = ["App.tsx", "types.ts", "stars.ts", "planets.ts", "hooks.ts"];
        let mut verdicts = Vec::new();
        for round in 0..6 {
            for file in files {
                let verdict = watch.observe("read_file", &json!({ "path": file }), false);
                verdicts.push(verdict);
                if verdict == LoopVerdict::Stuck {
                    assert!(round >= 2, "a stop must not come before the repeats do");
                    assert!(
                        verdicts.contains(&LoopVerdict::Circling),
                        "the model is told it is circling before the run is stopped"
                    );
                    return;
                }
            }
        }
        panic!("a run that reads the same files round after round must be stopped");
    }

    #[test]
    fn writing_the_same_text_again_and_again_is_stopped_but_real_edits_are_not() {
        // The same file, the same text, nothing run against it: spinning.
        let mut watch = LoopWatch::new(true);
        let same = json!({"path": "src/App.tsx", "content": "export const App = () => null;"});
        let mut verdicts = Vec::new();
        for _ in 0..LoopWatch::STOP_IDENTICAL_REWRITES {
            verdicts.push(watch.observe("write_file", &same, true));
        }
        assert_eq!(verdicts.last(), Some(&LoopVerdict::Stuck), "{verdicts:?}");
        assert_eq!(watch.rewriting_identically().map(|(path, _)| path), Some("src/App.tsx"));

        // The same file with different text every time is a model working on
        // it: warned once to check what it wrote, never stopped for it.
        let mut editing = LoopWatch::new(true);
        let mut verdicts = Vec::new();
        for round in 0..15 {
            verdicts.push(editing.observe(
                "edit_file",
                &json!({"path": "src/App.tsx", "new": format!("line {round}")}),
                true,
            ));
        }
        assert!(!verdicts.contains(&LoopVerdict::Stuck), "different edits are work: {verdicts:?}");
        assert_eq!(
            verdicts.iter().filter(|verdict| **verdict == LoopVerdict::Circling).count(),
            1,
            "told once to check, not nagged: {verdicts:?}"
        );

        // A long file written in parts is exactly what the instructions ask for.
        let mut parts = LoopWatch::new(true);
        for part in 0..12 {
            assert_eq!(
                parts.observe("append_file", &json!({"path": "src/big.ts", "content": format!("part {part}")}), true),
                LoopVerdict::Working
            );
        }

        // Building between writes clears the count.
        let mut checked = LoopWatch::new(true);
        for _ in 0..12 {
            for _ in 0..2 {
                assert_eq!(checked.observe("write_file", &same, true), LoopVerdict::Working);
            }
            assert_eq!(
                checked.observe("execute_command", &json!({"command": "npm run build"}), true),
                LoopVerdict::Working
            );
        }
        assert!(checked.rewriting().is_none());

        // Writing different files is work, not churn.
        let mut spread = LoopWatch::new(true);
        for index in 0..10 {
            assert_eq!(
                spread.observe("write_file", &json!({"path": format!("src/part{index}.ts"), "content": "x"}), true),
                LoopVerdict::Working
            );
        }
    }

    #[test]
    fn a_long_honest_exploration_is_left_alone() {
        let mut watch = LoopWatch::new(true);
        for step in 0..LoopWatch::STOP_WITHOUT_CHANGE + 10 {
            assert_eq!(
                watch.observe("read_file", &json!({ "path": format!("src/file{step}.ts") }), false),
                LoopVerdict::Working,
                "reading forty different files is work, not a loop"
            );
        }
    }

    #[test]
    fn a_change_clears_the_record_so_verification_reads_are_free() {
        let mut watch = LoopWatch::new(true);
        for _ in 0..7 {
            watch.observe("read_file", &json!({ "path": "src/App.tsx" }), false);
        }
        assert!(watch.steps_without_change() >= 7);
        assert_eq!(
            watch.observe("write_file", &json!({ "path": "src/App.tsx" }), true),
            LoopVerdict::Working
        );
        assert_eq!(watch.steps_without_change(), 0);
        assert_eq!(watch.revisits(), 0);
        // Reading the file back after writing it is not a revisit.
        assert_eq!(
            watch.observe("read_file", &json!({ "path": "src/App.tsx" }), false),
            LoopVerdict::Working
        );
    }

    #[test]
    fn a_run_that_may_not_write_is_judged_on_repetition_alone() {
        let mut question = LoopWatch::new(false);
        for step in 0..LoopWatch::STOP_WITHOUT_CHANGE + 5 {
            assert_ne!(
                question.observe("read_file", &json!({ "path": format!("f{step}.rs") }), false),
                LoopVerdict::Stuck,
                "answering a question reads and never writes"
            );
        }
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
