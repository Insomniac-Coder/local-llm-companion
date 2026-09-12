//! Server-side generation lifecycle (§46: Stop actually stops).
//!
//! The UI fetch abort alone only closes the SSE reader — the sidecar request
//! would keep burning GPU and its tokens would be lost. The tracker keeps the
//! active generation's cancel flag, partial text, and task handle so
//! `POST /api/chat/stop` (or a superseding new turn) can:
//! 1. flag cancellation (the sidecar stream breaks at the next chunk),
//! 2. abort the task (drops the reqwest stream even if stuck),
//! 3. persist whatever was streamed so far — never silently discard (§83).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::task::JoinHandle;

pub struct ActiveGeneration {
    pub id: String,
    pub conversation_id: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub partial: Arc<Mutex<String>>,
    pub persisted: Arc<AtomicBool>,
    pub handle: JoinHandle<()>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CancelOutcome {
    pub stopped: bool,
    pub id: Option<String>,
    pub chars_kept: usize,
}

#[derive(Default)]
pub struct GenerationTracker {
    current: Option<ActiveGeneration>,
}

impl GenerationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.current
            .as_ref()
            .map(|g| !g.handle.is_finished())
            .unwrap_or(false)
    }

    pub fn current_id(&self) -> Option<String> {
        self.current.as_ref().map(|g| g.id.clone())
    }

    /// Conversation (if any) with a live generation — ACTIVE residency (§126).
    pub fn active_conversation(&self) -> Option<String> {
        self.current
            .as_ref()
            .filter(|g| !g.handle.is_finished())
            .and_then(|g| g.conversation_id.clone())
    }

    pub fn insert(&mut self, gen: ActiveGeneration) {
        self.current = Some(gen);
    }

    /// Remove the tracked generation if it is the given id (completion cleanup).
    pub fn clear_if(&mut self, id: &str) {
        if self.current.as_ref().map(|g| g.id.as_str()) == Some(id) {
            self.current = None;
        }
    }

    /// Cancel whatever is running: flag + abort + persist partial exactly once.
    /// Safe to call when idle (returns `stopped: false`).
    pub async fn cancel_current(
        &mut self,
        storage: &tokio::sync::Mutex<crate::storage::Storage>,
    ) -> CancelOutcome {
        let Some(gen) = self.current.take() else {
            return CancelOutcome {
                stopped: false,
                id: None,
                chars_kept: 0,
            };
        };
        gen.cancel.store(true, Ordering::SeqCst);
        gen.handle.abort();
        // Claim the single persistence right; the background task (if still
        // alive) will see `persisted == true` and skip its own save.
        let claimed = gen
            .persisted
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        let text = gen.partial.lock().expect("lock").clone();
        let mut kept = 0usize;
        if claimed {
            if let (Some(cid), true) = (gen.conversation_id.clone(), !text.is_empty()) {
                let st = storage.lock().await;
                if st
                    .add_message(&crate::storage::Message {
                        id: gen.id.clone(),
                        conversation_id: cid,
                        role: "assistant".into(),
                        content: text.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    })
                    .is_ok()
                {
                    kept = text.len();
                    if st
                        .message_activities(&gen.id)
                        .map(|events| !events.is_empty())
                        .unwrap_or(false)
                    {
                        let _ = st.record_message_activity(&gen.id, &crate::agent::AgentEvent::activity(
                            "status", crate::agent::AgentState::Cancelled,
                            "Stopped by you. Completed inspections and the partial response were kept.".into(), 0,
                        ));
                    }
                }
            }
        }
        tracing::info!(id = %gen.id, kept, "generation cancelled; partial kept");
        CancelOutcome {
            stopped: true,
            id: Some(gen.id),
            chars_kept: kept,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_persists_partial_and_is_idempotent() {
        let storage = tokio::sync::Mutex::new(crate::storage::Storage::open_in_memory().unwrap());
        storage
            .lock()
            .await
            .create_conversation(&crate::storage::Conversation {
                id: "c1".into(),
                title: "t".into(),
                model_id: "m".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                mode: "chat".into(),
                workspace: "".into(),
                reasoning_default: false,
                search_default: false,
                last_model: "".into(),
                priority: "normal".into(),
                related_to: "".into(),
            })
            .unwrap();

        let mut tracker = GenerationTracker::new();
        assert!(!tracker.is_active());
        let out = tracker.cancel_current(&storage).await;
        assert!(!out.stopped, "idle cancel must report stopped:false");

        // Fake a stuck generation holding partial text.
        let partial = Arc::new(Mutex::new("Hello wo".to_string()));
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        });
        tracker.insert(ActiveGeneration {
            id: "g1".into(),
            conversation_id: Some("c1".into()),
            cancel: Arc::new(AtomicBool::new(false)),
            partial: partial.clone(),
            persisted: Arc::new(AtomicBool::new(false)),
            handle,
        });
        assert!(tracker.is_active());

        let out = tracker.cancel_current(&storage).await;
        assert!(out.stopped);
        assert_eq!(out.id.as_deref(), Some("g1"));
        assert!(out.chars_kept > 0);
        let msgs = storage.lock().await.messages_for("c1").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello wo", "persist exactly what streamed");

        // Second cancel finds nothing and persists nothing more.
        let out = tracker.cancel_current(&storage).await;
        assert!(!out.stopped);
        assert_eq!(storage.lock().await.messages_for("c1").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_without_conversation_keeps_nothing_but_reports_stopped() {
        let storage = tokio::sync::Mutex::new(crate::storage::Storage::open_in_memory().unwrap());
        let mut tracker = GenerationTracker::new();
        tracker.insert(ActiveGeneration {
            id: "g2".into(),
            conversation_id: None,
            cancel: Arc::new(AtomicBool::new(false)),
            partial: Arc::new(Mutex::new("ephemeral".into())),
            persisted: Arc::new(AtomicBool::new(false)),
            handle: tokio::spawn(async {}),
        });
        let out = tracker.cancel_current(&storage).await;
        assert!(out.stopped);
        assert_eq!(out.chars_kept, 0);
    }
}
