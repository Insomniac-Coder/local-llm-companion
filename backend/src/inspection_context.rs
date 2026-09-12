//! Request-local evidence selection. Original history and the full activity
//! journal are never edited; only transient copies of tool results are released.
use crate::llamaserver::ChatTurn;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

struct Chunk {
    id: String,
    turn: usize,
    source: String,
    score: usize,
    keep: bool,
    released: bool,
}

#[derive(Default)]
pub struct InspectionContext {
    chunks: Vec<Chunk>,
    terms: BTreeSet<String>,
}

fn terms(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| {
            s.len() > 3
                && !matches!(
                    *s,
                    "this"
                        | "that"
                        | "these"
                        | "those"
                        | "what"
                        | "which"
                        | "where"
                        | "have"
                        | "with"
                        | "from"
                        | "about"
                        | "more"
                        | "tell"
                        | "please"
                        | "project"
                        | "could"
                        | "would"
                        | "want"
                        | "deeper"
                        | "into"
                        | "explain"
                        | "does"
                        | "doesn"
                )
        })
        .map(str::to_owned)
        .collect()
}

impl InspectionContext {
    pub fn new(history: &[ChatTurn]) -> Self {
        let mut result = Self::default();
        // Earlier user messages supply the subject of vague follow-ups, without
        // treating unrelated system instructions as relevance keywords.
        for turn in history.iter().rev().filter(|t| t.role == "user").take(4) {
            result.terms.extend(terms(&turn.content));
        }
        result
    }

    pub fn record(&mut self, turns: &mut [ChatTurn], tool: &str, args: &Value) {
        if tool == "manage_context" {
            return;
        }
        let index = turns.len() - 1;
        let id = format!("chunk-{}", self.chunks.len() + 1);
        let source = format!("{tool} {args}");
        let lower_source = source.to_lowercase();
        let lower_body = turns[index].content.to_lowercase();
        let score = self
            .terms
            .iter()
            .map(|term| {
                usize::from(lower_source.contains(term)) * 8
                    + usize::from(lower_body.contains(term)) * 2
            })
            .sum();
        // Coordinates are retained even when the body is released. Never treat
        // a shortened excerpt as a semantic summary or verified complete file.
        let coordinates = turns[index]
            .content
            .lines()
            .find(|l| l.starts_with("[Lines "))
            .unwrap_or("");
        let source = format!("{source} {coordinates}");
        turns[index].content = format!("[Evidence {id}: {source}]\n{}", turns[index].content);
        self.chunks.push(Chunk {
            id,
            turn: index,
            source,
            score,
            keep: false,
            released: false,
        });
    }

    fn release(chunk: &mut Chunk, turns: &mut [ChatTurn]) {
        turns[chunk.turn].content = format!("[Evidence {}: {}]\n[Body released from working context, not summarized. Full output is in file activity. Re-read this path/range if needed.]", chunk.id, chunk.source);
        chunk.released = true;
        chunk.keep = false;
    }

    pub fn manage(&mut self, args: &Value, turns: &mut [ChatTurn]) -> Result<String, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Selection {
            #[serde(default)]
            keep: Vec<String>,
            #[serde(default)]
            release: Vec<String>,
        }
        let selection: Selection = serde_json::from_value(args.clone())
            .map_err(|_| "Use {\"keep\":[\"chunk-1\"],\"release\":[\"chunk-2\"]}".to_string())?;
        // Validate everything before making any change.
        for id in selection.keep.iter().chain(&selection.release) {
            let Some(chunk) = self.chunks.iter().find(|c| &c.id == id) else {
                return Err(format!("Unknown evidence ID {id}"));
            };
            if selection.keep.contains(id) && selection.release.contains(id) {
                return Err(format!("Cannot both keep and release {id}"));
            }
            if selection.keep.contains(id) && chunk.released {
                return Err(format!(
                    "{id} was already released. Re-read {} to restore it.",
                    chunk.source
                ));
            }
        }
        for chunk in &mut self.chunks {
            if selection.keep.contains(&chunk.id) {
                chunk.keep = true;
            }
            if selection.release.contains(&chunk.id) {
                Self::release(chunk, turns);
            }
        }
        Ok(format!("Kept {:?}; released {:?}. This changes only working context, not files or saved activity.", selection.keep, selection.release))
    }

    pub fn compact(&mut self, turns: &mut [ChatTurn], context: u32, reserve: u32) -> bool {
        // Conservative text estimate (not token-exact); reserve answer/framing room.
        let budget = context.saturating_sub(reserve).saturating_sub(1024) as usize * 3;
        let mut total: usize = turns.iter().map(|t| t.content.chars().count() + 32).sum();
        let newest = self.chunks.last().map(|c| c.turn);
        let mut candidates: Vec<_> = self
            .chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.keep && !c.released && Some(c.turn) != newest)
            .map(|(i, c)| (c.score, c.turn, i))
            .collect();
        candidates.sort(); // Lowest relevance first, oldest first for ties.
        for (_, _, i) in candidates {
            if total <= budget {
                break;
            }
            let chunk = &mut self.chunks[i];
            let before = turns[chunk.turn].content.chars().count();
            // Tiny results are cheaper than the release notice.
            if before <= chunk.source.chars().count() + 250 {
                continue;
            }
            Self::release(chunk, turns);
            total = total - before + turns[chunk.turn].content.chars().count();
        }
        total <= budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn add(memory: &mut InspectionContext, turns: &mut Vec<ChatTurn>, path: &str, text: &str) {
        turns.push(ChatTurn::text("assistant", "read"));
        turns.push(ChatTurn::text("user", text));
        memory.record(
            turns,
            "read_file",
            &serde_json::json!({"path":path,"start_line":17000,"end_line":17100}),
        );
    }
    #[test]
    fn relevance_beats_recency_and_history_is_untouched() {
        let mut turns = vec![
            ChatTurn::text("system", "identity"),
            ChatTurn::text("user", "Explain renderer synchronization"),
        ];
        let mut memory = InspectionContext::new(&turns);
        add(
            &mut memory,
            &mut turns,
            "src/renderer.cpp",
            &"synchronization evidence ".repeat(400),
        );
        add(
            &mut memory,
            &mut turns,
            "tools/release.py",
            &"unrelated tooling ".repeat(600),
        );
        add(&mut memory, &mut turns, "tests/render.cpp", "latest result");
        assert!(memory.compact(&mut turns, 6500, 1000));
        assert!(!memory.chunks[0].released, "older relevant source stays");
        assert!(
            memory.chunks[1].released,
            "newer irrelevant output goes first"
        );
        assert_eq!(turns[1].content, "Explain renderer synchronization");
        assert!(turns[5].content.contains("start_line"));
        assert!(turns.last().unwrap().content.contains("latest result"));
    }
    #[test]
    fn model_can_keep_and_release_chunks_but_not_restore_missing_evidence() {
        let mut turns = vec![ChatTurn::text("user", "question")];
        let mut memory = InspectionContext::new(&turns);
        add(&mut memory, &mut turns, "src/a.cpp", &"a".repeat(8000));
        add(&mut memory, &mut turns, "src/b.cpp", &"b".repeat(8000));
        assert!(memory
            .manage(
                &serde_json::json!({"keep":["chunk-1"],"release":["chunk-2"]}),
                &mut turns
            )
            .is_ok());
        assert!(
            !memory.compact(&mut turns, 2000, 1000),
            "pinned evidence is never silently dropped"
        );
        assert!(turns[2].content.contains(&"a".repeat(8000)));
        assert!(turns[4].content.contains("Body released"));
        assert!(memory
            .manage(&serde_json::json!({"keep":["chunk-2"]}), &mut turns)
            .unwrap_err()
            .contains("Re-read"));
        assert!(memory
            .manage(
                &serde_json::json!({"release":["chunk-99","chunk-1"]}),
                &mut turns
            )
            .is_err());
        assert!(!memory.chunks[0].released, "invalid selection is atomic");
    }
}
