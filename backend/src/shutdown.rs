//! Companion closing, and why. A task cut short by it says so instead of
//! blaming the model server that was stopped under it: closing used to stop
//! the model server first, and a task in flight then recorded "the model
//! request failed ... retry when the model is ready" (owner, 2026-09-18).

use std::sync::OnceLock;

static REASON: OnceLock<String> = OnceLock::new();

/// Marks the process as closing, for this reason ("its window was closed").
pub fn begin(reason: &str) {
    let _ = REASON.set(reason.to_string());
}

/// Why Companion is closing, once it is.
pub fn reason() -> Option<&'static str> {
    REASON.get().map(String::as_str)
}

/// The last word of a task that was running when Companion closed.
pub fn task_message(reason: &str) -> String {
    format!(
        "Stopped: Companion was closed while this task was running ({reason}). Existing files and completed actions were kept; send a message in this chat to continue."
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_task_cut_short_by_closing_says_why() {
        let message = super::task_message("its window was closed");
        assert!(message.contains("Companion was closed while this task was running (its window was closed)"));
        assert!(!message.contains("model"), "the model server is not blamed: {message}");
    }
}
