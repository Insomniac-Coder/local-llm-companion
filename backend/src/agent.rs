//! Coding agent loop (§28–§30, §45, §82): plan -> tool -> observe -> repeat.
//! Bounded by max_iterations (30) and per-tool retries (3). Cancellable (§46).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentState {
    Idle,
    Planning,
    ExecutingTool,
    WaitingPermission,
    Observing,
    /// Paused between steps while earlier turns are summarized to fit the
    /// model's context window; the run resumes on its own afterwards.
    Compacting,
    Completed,
    Failed,
    Cancelled,
}

impl AgentState {
    /// A run in this state is still working (a paused compaction included).
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Planning
                | Self::ExecutingTool
                | Self::WaitingPermission
                | Self::Observing
                | Self::Compacting
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Chat,
    Plan,
    CodeAssist,
    Agent,
    Autonomous,
}

impl AgentMode {
    pub fn allows_risk(self, risk: crate::permissions::RiskLevel) -> bool {
        !matches!(self, Self::Plan | Self::CodeAssist)
            || risk == crate::permissions::RiskLevel::Safe
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPlan {
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub state: AgentState,
    pub message: String,
    pub iteration: u32,
    /// Semantic UI event kind. This keeps the activity feed structured
    /// without asking the frontend to parse human-readable messages.
    #[serde(default = "default_event_kind")]
    pub kind: String,
    /// Structured tool information for started/completed activity rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Unified diff for file mutations when one is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Present on WAITING_PERMISSION so the UI can render Allow/Deny.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_tool: Option<PendingTool>,
    /// Context for one actual agent model request, never a sum of its journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<AgentContextUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextUsage {
    pub estimated_tokens: u32,
    pub prompt_tokens: Option<u32>,
    pub generated_tokens: Option<u32>,
    pub context_limit: u32,
    pub output_reserve: u32,
    pub turns: u32,
    pub pruned_turns: u32,
    pub images: u32,
    /// request = assembled input; response = runtime replied (usage optional).
    pub phase: String,
    /// Times this run summarized earlier turns to stay within the window.
    #[serde(default)]
    pub compactions: u32,
    /// Tokens the transcript may occupy (window minus the reply's reserve
    /// and a margin): the room automatic compaction measures against.
    #[serde(default)]
    pub history_room: u32,
    /// Automatic compaction threshold as a share of `history_room`; 0 = off.
    #[serde(default)]
    pub compact_at_pct: u32,
}

impl AgentContextUsage {
    pub fn for_turns(
        turns: &[crate::llamaserver::ChatTurn],
        context_limit: u32,
        output_reserve: u32,
        pruned_turns: u32,
    ) -> Self {
        // Text + a small estimated message-framing allowance. Only the
        // runtime's tokenizer can report actual tokens, especially for images.
        let text_chars: usize = turns.iter().map(|turn| turn.content.chars().count()).sum();
        Self {
            estimated_tokens: ((text_chars.saturating_add(3) / 4)
                .saturating_add(turns.len().saturating_mul(8)))
            .min(u32::MAX as usize) as u32,
            prompt_tokens: None,
            generated_tokens: None,
            context_limit,
            output_reserve,
            turns: turns.len().min(u32::MAX as usize) as u32,
            pruned_turns,
            images: turns
                .iter()
                .map(|turn| turn.images.len())
                .sum::<usize>()
                .min(u32::MAX as usize) as u32,
            phase: "request".into(),
            compactions: 0,
            history_room: 0,
            compact_at_pct: 0,
        }
    }

    pub fn with_compaction(mut self, compactions: u32, history_room: u32, compact_at_pct: u32) -> Self {
        self.compactions = compactions;
        self.history_room = history_room;
        self.compact_at_pct = compact_at_pct;
        self
    }

    pub fn reported(mut self, prompt_tokens: u32, generated_tokens: u32) -> Self {
        // SidecarClient maps omitted usage to zero. An assembled nonempty
        // agent request cannot genuinely have zero prompt tokens.
        self.phase = "response".into();
        self.prompt_tokens = (prompt_tokens > 0).then_some(prompt_tokens);
        self.generated_tokens = (prompt_tokens > 0).then_some(generated_tokens);
        self
    }
}

fn default_event_kind() -> String {
    "status".into()
}

/// A tool call awaiting user approval (§26).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTool {
    pub tool: String,
    pub args: serde_json::Value,
    pub reason: String,
}

impl AgentEvent {
    pub fn new(state: AgentState, message: String, iteration: u32) -> Self {
        Self {
            state,
            message,
            iteration,
            kind: default_event_kind(),
            tool: None,
            args: None,
            output: None,
            diff: None,
            pending_tool: None,
            context_usage: None,
        }
    }

    pub fn activity(kind: &str, state: AgentState, message: String, iteration: u32) -> Self {
        Self {
            kind: kind.into(),
            ..Self::new(state, message, iteration)
        }
    }

    pub fn context(state: AgentState, iteration: u32, usage: AgentContextUsage) -> Self {
        Self {
            kind: "context".into(),
            context_usage: Some(usage),
            ..Self::new(state, "Agent input context updated".into(), iteration)
        }
    }

    pub fn tool_activity(
        kind: &str,
        state: AgentState,
        message: String,
        iteration: u32,
        tool: String,
        args: serde_json::Value,
        output: Option<String>,
        diff: Option<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            tool: Some(tool),
            args: Some(args),
            output,
            diff,
            ..Self::new(state, message, iteration)
        }
    }
}

pub struct AgentLimits {
    pub max_iterations: u32,
    pub max_retries_per_tool: u32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_iterations: 30,
            max_retries_per_tool: 3,
        }
    }
}

/// Cancellation token shared with the API Stop button (§46).
#[derive(Debug, Default, Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Deterministic skeleton of the loop; Stage 10 plugs the LLM planner in.
pub fn run_stub_plan(
    task: &str,
    limits: &AgentLimits,
    cancel: &CancelToken,
    mut on_event: impl FnMut(AgentEvent),
) -> AgentState {
    on_event(AgentEvent::new(
        AgentState::Planning,
        format!("Planning: {task}"),
        0,
    ));
    // §94: explicit plan the UI can display.
    let plan = AgentPlan {
        steps: vec![
            "Inspect workspace tree".into(),
            "Search relevant symbols".into(),
            "Read relevant files".into(),
            "Modify code via patch".into(),
            "Build and run tests".into(),
        ],
    };
    for (i, step) in plan.steps.iter().enumerate() {
        if cancel.is_cancelled() {
            on_event(AgentEvent::new(
                AgentState::Cancelled,
                "Cancelled by user.".into(),
                i as u32,
            ));
            return AgentState::Cancelled;
        }
        if i as u32 >= limits.max_iterations {
            on_event(AgentEvent::new(
                AgentState::Failed,
                "Iteration budget exhausted.".into(),
                i as u32,
            ));
            return AgentState::Failed;
        }
        on_event(AgentEvent::new(
            AgentState::ExecutingTool,
            step.clone(),
            i as u32,
        ));
    }
    on_event(AgentEvent::new(
        AgentState::Completed,
        "Plan skeleton complete (LLM executor lands in Stage 10).".into(),
        plan.steps.len() as u32,
    ));
    AgentState::Completed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_counts_only_the_submitted_transcript() {
        use crate::llamaserver::ChatTurn;
        let initial = vec![
            ChatTurn::text("system", "s".repeat(400)),
            ChatTurn::text("user", "inspect"),
        ];
        let mut working = initial.clone();
        working.push(ChatTurn::text(
            "user",
            format!("Tool read_file returned:\n{}", "x".repeat(4000)),
        ));
        let before = AgentContextUsage::for_turns(&initial, 32768, 1024, 0);
        let after = AgentContextUsage::for_turns(&working, 32768, 1024, 0);
        assert!(after.estimated_tokens > before.estimated_tokens + 1000);
        assert_eq!(after.prompt_tokens, None);
        working.remove(2);
        let pruned = AgentContextUsage::for_turns(&working, 32768, 1024, 1);
        assert_eq!(pruned.estimated_tokens, before.estimated_tokens);
        assert_eq!(pruned.pruned_turns, 1);
    }

    #[test]
    fn runtime_context_distinguishes_missing_usage_from_reported_input() {
        let request = AgentContextUsage::for_turns(
            &[crate::llamaserver::ChatTurn::text("user", "hello")],
            8192,
            1024,
            0,
        );
        assert_eq!(request.clone().reported(0, 0).prompt_tokens, None);
        let reported = request.reported(1500, 0);
        assert_eq!(reported.prompt_tokens, Some(1500));
        assert_eq!(reported.generated_tokens, Some(0));
        let event = AgentEvent::context(AgentState::Planning, 3, reported);
        let roundtrip: AgentEvent =
            serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        assert_eq!(roundtrip.context_usage.unwrap().prompt_tokens, Some(1500));
        let legacy: AgentEvent =
            serde_json::from_str(r#"{"state":"PLANNING","message":"old","iteration":1}"#).unwrap();
        assert!(legacy.context_usage.is_none());
    }

    #[test]
    fn completes_stub_plan() {
        let cancel = CancelToken::new();
        let mut states = vec![];
        let end = run_stub_plan("fix build", &AgentLimits::default(), &cancel, |e| {
            states.push(e.state)
        });
        assert_eq!(end, AgentState::Completed);
        assert!(states.contains(&AgentState::Planning));
    }

    #[test]
    fn cancellation_stops_loop() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let end = run_stub_plan("x", &AgentLimits::default(), &cancel, |_| {});
        assert_eq!(end, AgentState::Cancelled);
    }

    #[test]
    fn iteration_budget_enforced() {
        let cancel = CancelToken::new();
        let limits = AgentLimits {
            max_iterations: 2,
            max_retries_per_tool: 3,
        };
        let end = run_stub_plan("long task", &limits, &cancel, |_| {});
        assert_eq!(end, AgentState::Failed);
    }
}
