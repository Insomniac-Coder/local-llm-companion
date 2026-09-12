//! LLM coding-agent loop (§28–30, §45, §82, §94).
//!
//! Each iteration: model reasons over the transcript → emits at most one
//! ```tool fenced call (or a final answer) → gate → execute → observe.
//! Bounded by max_iterations and a shared no-progress budget. Approvals pause
//! the loop (§26); Stop cancels it and aborts any in-flight sidecar request.

use crate::agent::{AgentEvent, AgentLimits, AgentMode, AgentState, CancelToken, PendingTool};
use crate::llamaserver::{ChatTurn, SidecarClient};
use crate::permissions::{PermissionDecision, RiskLevel};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

const TRANSCRIPT_TURNS: usize = 24;
const TOOL_OUTPUT_CHARS: usize = 16_000;

#[derive(Default)]
struct ActionResponsePolicy {
    disable_native_thinking: bool,
    invalid_since_progress: u32,
    continuation_used: bool,
    structured_fallback: bool,
}

impl ActionResponsePolicy {
    fn output_budget(&self, context: u32, estimated_input: u32) -> u32 {
        let cap = if self.disable_native_thinking {
            4096
        } else {
            2048
        };
        cap.min(context.saturating_sub(estimated_input.saturating_add(256)))
    }

    /// One changed-strategy recovery, never unlimited rephrasing of a failure.
    fn recover_invalid(&mut self) -> bool {
        self.invalid_since_progress += 1;
        self.disable_native_thinking = true;
        self.structured_fallback = true;
        self.invalid_since_progress == 1
    }

    fn begin_continuation(&mut self) -> bool {
        if self.continuation_used || self.invalid_since_progress > 0 {
            return false;
        }
        self.continuation_used = true;
        self.disable_native_thinking = true;
        true
    }

    fn useful_progress(&mut self) {
        self.invalid_since_progress = 0;
        self.continuation_used = false;
        // Keep the working template override for this run, not global Chat.
    }
}

fn action_response_invalid_reason(
    completion: &crate::llamaserver::AgentCompletion,
    parsed_call: Option<&ToolCall>,
    output_budget: u32,
) -> Option<String> {
    if completion.is_truncated() {
        Some(format!(
            "The response reached its {output_budget}-token output limit before completing."
        ))
    } else if completion.native_tool_calls_present {
        Some("The model used an unsupported action format.".into())
    } else if completion.text.trim().is_empty() {
        Some(
            if completion.reasoning_present {
                "The model produced reasoning but no visible action or answer."
            } else {
                "The model returned no visible action or answer."
            }
            .into(),
        )
    } else if parsed_call.is_none()
        && (completion.text.contains("```tool") || completion.text.contains("<|tool_call>"))
    {
        Some("The model returned an incomplete or unreadable action.".into())
    } else if completion.text.trim_start().starts_with('{')
        && parse_structured_reply(&completion.text).is_none()
    {
        Some("The model returned an invalid structured action or final answer.".into())
    } else {
        None
    }
}

fn can_continue_response(completion: &crate::llamaserver::AgentCompletion) -> bool {
    completion.is_truncated()
        && !completion.native_tool_calls_present
        && !completion.text.trim().is_empty()
        && completion.text.chars().count() <= 32_768
}

fn continuation_turns(transcript: &[ChatTurn], prefix: &str) -> Vec<ChatTurn> {
    let mut turns = transcript.to_vec();
    turns.push(ChatTurn::text("assistant", prefix));
    turns.push(ChatTurn::text("user", "The previous response was cut off by the output limit. Continue it from the exact next character. Return ONLY the missing suffix: do not repeat the prefix, add an introduction, or open a new tool block. If inside a JSON string, continue that string with correct JSON escaping, then close the JSON and its existing fence or native tool envelope. Do not change formats. Complete only that one response/action. Nothing from the partial response has been executed."));
    turns
}

fn assemble_continuation(prefix: &str, suffix: &str) -> Option<String> {
    if suffix.trim().is_empty() {
        return None;
    }
    // Some models repeat the entire prefix. Accept only an exact extension;
    // never guess overlap boundaries that could corrupt source code or JSON.
    let combined = if suffix.starts_with(prefix) {
        if suffix.len() == prefix.len() {
            return None;
        }
        suffix.to_owned()
    } else {
        format!("{prefix}{suffix}")
    };
    (combined.chars().count() <= 65_536).then_some(combined)
}

/// Structured call the model emits inside a ```tool fence (§31).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredReply {
    kind: String,
    name: String,
    args: serde_json::Value,
    answer: String,
}

fn parse_structured_reply(text: &str) -> Option<StructuredReply> {
    let reply: StructuredReply = serde_json::from_str(text).ok()?;
    if !reply.args.is_object() {
        return None;
    }
    match reply.kind.as_str() {
        // `kind` is authoritative. A tool's optional answer is a note, never a
        // completion; unused tool metadata in a final response never executes.
        "tool" if !reply.name.trim().is_empty() => Some(reply),
        "final" if !reply.answer.trim().is_empty() => Some(reply),
        _ => None,
    }
}

const STRUCTURED_ACTION_INSTRUCTION: &str = "For this response use the enforced JSON envelope, not Markdown or native tool-call notation. Return exactly one object with all four fields: kind, name, args, answer. For an action use kind=tool, the registered tool name, its argument object, and an empty answer string. For a final response use kind=final, an empty name string, an empty args object, and your nonempty final answer string. No extra fields or text. This format changes no task scope, tool permissions, or verification requirements.";

/// Accept one complete, standalone action fence, optionally preceded by a
/// progress note. Multiple, nested, quoted or unfinished fences are ambiguous:
/// never select one action while silently dropping a prerequisite action.
pub fn parse_tool_block(text: &str) -> Option<ToolCall> {
    let mut offset = 0;
    let mut opening = None;
    for line in text.split_inclusive('\n') {
        let plain = line.trim_end();
        let indent = plain.bytes().take_while(|byte| *byte == b' ').count();
        if indent <= 3 && &plain[indent..] == "```tool" {
            opening = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let (start, body_start) = opening?;
    if text[..start].contains("```") {
        return None;
    }

    // Decode JSON before finding the closing fence: a legitimate write_file
    // payload may itself contain Markdown backticks inside a JSON string.
    let body = &text[body_start..];
    let mut values = serde_json::Deserializer::from_str(body).into_iter::<ToolCall>();
    let call = values.next()?.ok()?;
    if call.name.trim().is_empty() || !call.args.is_object() {
        return None;
    }
    let tail = &body[values.byte_offset()..];
    let closing_start = tail.find(|ch: char| !ch.is_whitespace())?;
    // Closing fences must occupy their own line, not appear in JSON/prose.
    if !tail[..closing_start].contains('\n') {
        return None;
    }
    let closing = &tail[closing_start..];
    let closing_line = closing.split_inclusive('\n').next()?;
    if closing_line.trim_end() != "```" || !closing[closing_line.len()..].trim().is_empty() {
        return None;
    }
    Some(call)
}

/// Normalize the native text envelope observed from the installed Gemma model.
/// It must be one entire call, with JSON object arguments and no trailing text.
/// This only parses: registry, mode, permission and argument checks still follow.
fn parse_action_response(text: &str) -> Option<ToolCall> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let reply = parse_structured_reply(trimmed)?;
        return (reply.kind == "tool").then_some(ToolCall {
            name: reply.name,
            args: reply.args,
        });
    }
    if !trimmed.starts_with("<|tool_call>") {
        return parse_tool_block(text);
    }
    let body = trimmed.strip_prefix("<|tool_call>call:")?;
    let (name, body) = body.split_once("{args:")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    let mut values = serde_json::Deserializer::from_str(body).into_iter::<serde_json::Value>();
    let args = values.next()?.ok()?;
    if !args.is_object() || body[values.byte_offset()..].trim() != "}<tool_call|>" {
        return None;
    }
    Some(ToolCall {
        name: name.to_owned(),
        args,
    })
}

/// Return the model's user-visible progress note without its machine-readable
/// tool fence. This is deliberately a concise activity summary, not hidden
/// chain-of-thought.
pub fn visible_progress(text: &str) -> String {
    if text.trim_start().starts_with("<|tool_call>") || parse_structured_reply(text).is_some() {
        return String::new();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("```tool") {
        out.push_str(&rest[..start]);
        let fenced = &rest[start + "```tool".len()..];
        if let Some(end) = fenced.find("```") {
            rest = &fenced[end + 3..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out.trim().chars().take(1200).collect()
}

/// Controlled system prompt (§22): capabilities, tool rules, cwd, OS.
pub fn system_prompt(
    workspace: &str,
    tools: &[crate::tools::ToolDescriptor],
    search_enabled: bool,
) -> String {
    let mut tool_docs = String::new();
    for t in tools {
        if t.name == "web_search" && !search_enabled {
            continue; // never advertise what the run may not use (§117)
        }
        tool_docs.push_str(&format!("- {} ({:?}): {}\n", t.name, t.risk, t.description));
        let args = match t.name {
            "list_directory" => r#"{"path":"."}"#,
            "read_file" => r#"{"path":"relative/path","start_line":1,"end_line":200}"#,
            "delete_file" | "open_path" => r#"{"path":"relative/path"}"#,
            "write_file" => r#"{"path":"relative/file","content":"full file text"}"#,
            "edit_file" => {
                r#"{"path":"relative/file","patch":"--- a/file\n+++ b/file\n@@ -1,1 +1,1 @@\n-old line\n+new line\n"}"#
            }
            "search_text" => r#"{"query":"regex pattern","path":"."}"#,
            "execute_command" => r#"{"command":"command text","cwd":".","timeout_secs":60}"#,
            "web_search" => r#"{"query":"search terms"}"#,
            "git_commit" => r#"{"message":"commit message"}"#,
            "system_info" | "list_processes" => "{}",
            _ => "see tool description",
        };
        tool_docs.push_str(&format!("  args: {args}\n"));
    }
    format!(
        "You are a local coding assistant operating inside one workspace.\n\
         Operating system: {os}\nWorkspace root: {workspace}\n\
         All `path` arguments are relative to the workspace root and cannot escape it.\n\
         Available tools:\n{tool_docs}\n\
         Rules:\n\
         1. The user's current request defines the task and its scope. Agent capability is not an instruction to change anything. Questions, explanations, reviews and diagnoses authorize inspection only; modify files only when the user requested a change. Acknowledgments and greetings never authorize new work or a different project. Never ask for slash-command syntax.\n\
         2. Use one complete action envelope with valid JSON, exactly as in this read_file example:\n\
         ```tool\n\
         {{\"name\":\"read_file\",\"args\":{{\"path\":\"README.md\"}}}}\n\
         ```\n\
         Substitute the actual tool and its arguments. Fence markers must be on separate lines. Never escape JSON object delimiters or repeat fence markers.\n\
         3. Prefer search_text before reading; read before editing; small unified-diff patches via edit_file.\n\
         4. For requested implementation work, run relevant builds/tests with execute_command and iterate on failures. Do not claim that reading a file is a passing test.\n\
         5. Never invent file contents you have not read; never redo a failed identical call.\n\
         6. For build, fix, or change requests, use the tools and complete the work; do not stop at a plan or paste code for the user to apply.\n\
         7. Permission prompts are handled by the host. Do not ask how to proceed when a relevant tool can advance the task.\n\
         8. After changing files, inspect the resulting project and run the most relevant tests, build, or validation available before finishing.\n\
         9. Each turn must EITHER emit exactly one tool call OR, only when the task is done or impossible, give the final summary with NO tool block.\n\
         10. Conversation history defines follow-up references such as 'this project', 'those tasks', and 'it'. If the workspace contains multiple sibling projects and history identifies one of them, confine searches and reads to that project. Do not inspect unrelated sibling projects merely because they are present.\n",
        os = std::env::consts::OS,
        workspace = workspace,
        tool_docs = tool_docs,
    )
}

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "CMakeLists.txt",
    "go.mod",
    "Makefile",
    "build.gradle",
    "pom.xml",
    "*.sln",
];

fn looks_like_project_root(path: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|marker| {
        if *marker == "*.sln" {
            return std::fs::read_dir(path)
                .map(|entries| {
                    entries.flatten().any(|entry| {
                        entry
                            .path()
                            .extension()
                            .map(|extension| extension.eq_ignore_ascii_case("sln"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
        }
        path.join(marker).exists()
    })
}

fn normalized_reference(value: &str) -> String {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A broad folder such as `Code/` may contain many independent projects. For
/// follow-ups like "this project", bind tools to the most recently named child
/// project instead of exposing unrelated siblings to retrieval or the model.
pub(crate) fn focused_workspace_root(root: &Path, conversation: &[String]) -> PathBuf {
    if looks_like_project_root(root) {
        return root.to_path_buf();
    }
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(_) => return root.to_path_buf(),
    };
    let mut candidates = std::fs::read_dir(&canonical_root)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !path.is_dir() || !looks_like_project_root(&path) {
                        return None;
                    }
                    let path = std::fs::canonicalize(path).ok()?;
                    if !path.starts_with(&canonical_root) {
                        return None;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    Some((normalized_reference(&name), path))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    for message in conversation.iter().rev() {
        let message = normalized_reference(message);
        let matches = candidates
            .iter()
            .filter(|(name, _)| {
                !name.is_empty() && format!(" {message} ").contains(&format!(" {name} "))
            })
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return matches[0].1.clone();
        }
        if matches.len() > 1 {
            return root.to_path_buf();
        }
    }
    root.to_path_buf()
}

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub workspace: PathBuf,
    pub task: String,
    pub mode: AgentMode,
    pub conversation_id: String,
    /// Stage 11: Search-toggle consent for this run (§117). web_search is
    /// only offered to the model when true.
    pub search_enabled: bool,
}

#[derive(Debug)]
pub enum ApprovalDecision {
    Approved { session: bool },
    Denied,
}

pub struct LiveRun {
    pub id: String,
    /// Stable wall-clock start, shared with the persisted assistant journal.
    pub started_at: String,
    pub spec: AgentSpec,
    pub cancel: CancelToken,
    pub events: Mutex<Vec<AgentEvent>>,
    pub broadcaster: broadcast::Sender<AgentEvent>,
    /// Ordered journal writer, separate from lossy live broadcast subscribers.
    pub activity_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    pub pending: Mutex<Option<PendingTool>>,
    pub pending_tx: Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
    pub handle: Mutex<Option<JoinHandle<AgentState>>>,
}

impl LiveRun {
    pub fn emit(&self, ev: AgentEvent) {
        let _ = self.activity_tx.send(ev.clone());
        self.events.lock().expect("lock").push(ev.clone());
        let _ = self.broadcaster.send(ev);
    }

    pub fn state(&self) -> AgentState {
        self.events
            .lock()
            .expect("lock")
            .last()
            .map(|e| e.state)
            .unwrap_or(AgentState::Idle)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub id: String,
    pub started_at: String,
    pub task: String,
    pub state: AgentState,
    pub iterations: u32,
    pub conversation_id: String,
}

#[derive(Default)]
pub struct AgentRegistry {
    runs: HashMap<String, Arc<LiveRun>>,
    order: VecDeque<String>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, run: Arc<LiveRun>) {
        self.order.push_back(run.id.clone());
        if self.order.len() > 20 {
            if let Some(old) = self.order.pop_front() {
                self.runs.remove(&old);
            }
        }
        self.runs.insert(run.id.clone(), run);
    }

    pub fn get(&self, id: &str) -> Option<Arc<LiveRun>> {
        self.runs.get(id).cloned()
    }

    pub fn summaries(&self) -> Vec<RunSummary> {
        self.order
            .iter()
            .filter_map(|id| self.runs.get(id))
            .map(|r| {
                let evs = r.events.lock().expect("lock");
                RunSummary {
                    id: r.id.clone(),
                    started_at: r.started_at.clone(),
                    task: r.task_short(),
                    state: evs.last().map(|e| e.state).unwrap_or(AgentState::Idle),
                    iterations: evs.last().map(|e| e.iteration).unwrap_or(0),
                    conversation_id: r.spec.conversation_id.clone(),
                }
            })
            .collect()
    }
}

impl LiveRun {
    fn task_short(&self) -> String {
        self.spec.task.chars().take(80).collect()
    }
}

/// The loop. Owns no locks across awaits except short clones.
pub async fn run_loop(state: crate::api::AppState, run: Arc<LiveRun>) -> AgentState {
    use AgentState as S;
    let spec = run.spec.clone();
    let conversation_scope = if spec.conversation_id.is_empty() {
        Vec::new()
    } else {
        state
            .storage
            .lock()
            .await
            .context_messages_for(&spec.conversation_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content)
            .collect()
    };
    let focused_root = focused_workspace_root(&spec.workspace, &conversation_scope);
    let ws = crate::workspace::WorkspaceManager::new(focused_root.clone());
    let ws_key = focused_root.to_string_lossy().into_owned();
    let limits = AgentLimits {
        max_iterations: state.settings.read().await.agent.max_iterations,
        ..AgentLimits::default()
    };

    // Snapshot the sidecar; without inference there is no reasoning engine (§4).
    let sidecar = {
        let mut llama = state.llama.write().await;
        if llama.is_running() {
            llama
                .running
                .as_ref()
                .map(|r| (r.base_url.clone(), r.cfg.clone()))
        } else {
            None
        }
    };
    let Some((base_url, cfg)) = sidecar else {
        run.emit(AgentEvent::new(
            S::Failed,
            "No inference running. Start inference first, then retry the task.".into(),
            0,
        ));
        return S::Failed;
    };
    let client = match SidecarClient::new(base_url) {
        Ok(c) => c,
        Err(e) => {
            run.emit(AgentEvent::new(
                S::Failed,
                format!("Sidecar client failed: {e}"),
                0,
            ));
            return S::Failed;
        }
    };

    run.emit(AgentEvent::activity(
        "task",
        S::Planning,
        spec.task.clone(),
        0,
    ));
    if focused_root != spec.workspace {
        run.emit(AgentEvent::activity(
            "status",
            S::Planning,
            format!(
                "Focused this follow-up on {} from the conversation; unrelated sibling projects are excluded.",
                focused_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("the referenced project")
            ),
            0,
        ));
    }
    let objective = match spec.mode {
        AgentMode::Plan => format!("Inspect the project using read-only tools and produce a practical implementation plan for this request. Do not implement changes or run commands. Request: {}", spec.task),
        AgentMode::CodeAssist => format!("Inspect the project using read-only tools and answer this exact request from evidence in the project. This is an inspection, not a request for an implementation plan. Resolve follow-up references from the preceding conversation and do not inspect unrelated sibling projects. Request: {}", spec.task),
        _ => spec.task.clone(),
    };
    let mut prompt = system_prompt(&ws_key, &crate::tools::registry(), spec.search_enabled);
    if matches!(spec.mode, AgentMode::Plan | AgentMode::CodeAssist) {
        prompt.push_str("\nThis run is READ ONLY. Only safe inspection tools are permitted. Do not write, edit, delete, create documents, execute commands or change Git state. Finish with findings or a plan, not implementation.");
    }
    let mut transcript = vec![ChatTurn::text("system", prompt)];
    // Each run starts fresh inference, not fresh product memory. Carry the
    // session's derived context and attachment excerpts into follow-up tasks.
    if !spec.conversation_id.is_empty() {
        let st = state.storage.lock().await;
        let workspace = st
            .get_conversation(&spec.conversation_id)
            .ok()
            .flatten()
            .map(|conversation| conversation.workspace)
            .unwrap_or_default();
        if let Ok(memory) = st.memory_context(&spec.conversation_id, &workspace) {
            transcript[0].content.push_str(&memory.text);
        }
        let mut history = st
            .context_messages_for(&spec.conversation_id)
            .unwrap_or_default();
        history.retain(|message| message.id != run.id);
        if history
            .last()
            .map(|message| message.role == "user" && message.content.trim() == spec.task.trim())
            .unwrap_or(false)
        {
            history.pop();
        }
        let attachments = st
            .attachments_for(&spec.conversation_id)
            .unwrap_or_default();
        transcript.extend(crate::api::build_turns(&history, &attachments));
    }
    transcript.push(ChatTurn::text(
            "user",
            format!(
                "Task: {}\n\nAddress this request within its scope. Use the complete tool envelope shown in the system instructions if an action is necessary; otherwise answer directly. Do not invent a new task or mutate files for a question.",
                objective
            ),
        ));
    let mut task_turn_index = transcript.len() - 1;
    let mut progress_guard = crate::agent_progress::ProgressGuard::new(4);
    let mut response_policy = ActionResponsePolicy::default();
    let mut pending_continuation: Option<String> = None;
    let mut completion_reviews = 0u32;
    let mut verification = VerificationState::default();
    let mut failed_calls = FailedCalls::default();
    let mut pruned_turns = 0u32;
    let mut tool_evidence: Vec<String> = Vec::new();

    for it in 1..=limits.max_iterations {
        if run.cancel.is_cancelled() {
            run.emit(AgentEvent::new(
                S::Cancelled,
                "Cancelled by user.".into(),
                it,
            ));
            return S::Cancelled;
        }
        // Keep the transcript bounded so small contexts survive long runs (§21).
        if transcript.len() > TRANSCRIPT_TURNS + 2 {
            while transcript.len() > TRANSCRIPT_TURNS + 2 {
                pruned_turns += 1;
                // Keep the system prompt and current task, not an arbitrary
                // older history row. Prefer dropping old context first.
                if task_turn_index > 1 {
                    transcript.remove(1);
                    task_turn_index -= 1;
                } else {
                    transcript.remove(2);
                }
            }
        }
        if !state.llama.write().await.is_running() {
            run.emit(AgentEvent::activity("error", S::Failed,
                "The model runtime stopped. Completed actions and existing file changes were kept. Load a model before continuing.".into(), it));
            return S::Failed;
        }
        if !progress_guard.begin_attempt() {
            let message = format!("Stopped after {} attempts without new successful tool results. The model was repeating unsuccessful responses rather than advancing the task. Existing files and recorded actions were kept; review the last error before retrying.", progress_guard.attempts_without_progress());
            persist_final(&state, &run, &message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message, it - 1));
            return S::Failed;
        }
        run.emit(AgentEvent::activity(
            "status",
            S::Planning,
            if tool_evidence.is_empty() {
                "Preparing the first action…".into()
            } else {
                "Reviewing the result and preparing the next action…".into()
            },
            it,
        ));
        // Partial visible text is request-local until a complete response has
        // been assembled and validated. It is never executed or journaled as
        // completed work, and never includes native reasoning text.
        let use_structured = response_policy.structured_fallback && pending_continuation.is_none();
        let mut request_turns = pending_continuation
            .as_deref()
            .map(|prefix| continuation_turns(&transcript, prefix))
            .unwrap_or_else(|| transcript.clone());
        if use_structured {
            request_turns.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        }
        let estimated_input =
            crate::agent::AgentContextUsage::for_turns(&request_turns, cfg.n_ctx, 0, pruned_turns)
                .estimated_tokens;
        let output_budget = response_policy.output_budget(cfg.n_ctx, estimated_input);
        if output_budget < 128 {
            let message = "There is not enough room in this model's context for another action. Saved history and existing files were kept. Compact the conversation or use a larger context before retrying.";
            persist_final(&state, &run, message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message.into(), it));
            return S::Failed;
        }
        let input_context = crate::agent::AgentContextUsage::for_turns(
            &request_turns,
            cfg.n_ctx,
            output_budget,
            pruned_turns,
        );
        run.emit(AgentEvent::context(S::Planning, it, input_context.clone()));
        let mut completion = match client
            .agent_chat_turns(
                &request_turns,
                output_budget,
                &cfg,
                response_policy.disable_native_thinking,
                use_structured,
            )
            .await
        {
            Ok(completion) => {
                run.emit(AgentEvent::context(
                    S::Planning,
                    it,
                    input_context.reported(
                        completion.metrics.prompt_tokens,
                        completion.metrics.generated_tokens,
                    ),
                ));
                completion
            }
            Err(e) => {
                run.emit(AgentEvent::activity(
                    "status",
                    S::Planning,
                    format!(
                        "The model request failed: {e}. Attempt {} of 4 without progress.",
                        progress_guard.attempts_without_progress()
                    ),
                    it,
                ));
                transcript.push(ChatTurn::text(
                    "user",
                    format!("The model call failed ({e}). Adjust and continue, or finish with a summary."),
                ));
                continue;
            }
        };
        let continuation_failed = if let Some(prefix) = pending_continuation.take() {
            match assemble_continuation(&prefix, &completion.text) {
                Some(combined) => {
                    completion.text = combined;
                    false
                }
                None => true,
            }
        } else {
            false
        };
        let reply = completion.text.clone();
        let parsed_call = parse_action_response(&reply);
        let invalid_reason = if continuation_failed {
            Some("The continuation returned no usable extension of the partial response.".into())
        } else {
            action_response_invalid_reason(&completion, parsed_call.as_ref(), output_budget)
        };
        if let Some(reason) = invalid_reason {
            if can_continue_response(&completion) && response_policy.begin_continuation() {
                let continuation_input = crate::agent::AgentContextUsage::for_turns(
                    &continuation_turns(&transcript, &reply),
                    cfg.n_ctx,
                    0,
                    pruned_turns,
                )
                .estimated_tokens;
                let retry_budget = response_policy.output_budget(cfg.n_ctx, continuation_input);
                pending_continuation = Some(reply);
                run.emit(AgentEvent::activity("status", S::Planning,
                    format!("{reason} Continuing from the saved partial response once, with native thinking disabled and up to {retry_budget} more output tokens. The assembled action must validate before anything executes."), it));
                continue;
            }
            if !response_policy.recover_invalid() {
                let message = format!("{reason} The controlled retry also failed, so I stopped instead of repeating it. No partial or unreadable action was executed. Existing work was kept.");
                persist_final(&state, &run, &message).await;
                run.emit(AgentEvent::activity("error", S::Failed, message, it));
                return S::Failed;
            }
            let retry_budget = response_policy.output_budget(cfg.n_ctx, estimated_input);
            run.emit(AgentEvent::activity("status", S::Planning,
                format!("{reason} Retrying once with native thinking disabled for this run and up to {retry_budget} output tokens. Incomplete actions are discarded."), it));
            transcript.push(ChatTurn::text(
                "user",
                "Your previous response was empty, cut off, or invalid; no action from it was executed. Return exactly one complete tool envelope as shown in the system example, with a name and object args and its closing fence. Keep file contents short: create one small file or make one small edit per turn. Escape quotes and newlines INSIDE JSON string values only; outer JSON keys must use ordinary quotes. If finished, return a brief final answer instead. Do not repeat an unfinished payload or output multiple actions.",
            ));
            continue;
        }
        // Failed/truncated payloads never pollute the subsequent transcript.
        transcript.push(ChatTurn::text("assistant", reply.clone()));

        let Some(call) = parsed_call else {
            let reply = parse_structured_reply(&reply)
                .filter(|envelope| envelope.kind == "final")
                .map(|envelope| envelope.answer)
                .unwrap_or(reply);
            // Read-only runs deliver findings/plans, not implemented code.
            // A second model critic cannot verify an implementation here and
            // can trap small local models in repeated format-correction loops.
            if matches!(spec.mode, AgentMode::Plan | AgentMode::CodeAssist) {
                persist_final(&state, &run, &reply).await;
                run.emit(AgentEvent::activity("final", S::Completed, reply, it));
                return S::Completed;
            }
            // A tool-free answer is only a candidate completion. Coding models,
            // especially smaller local ones, often summarize after producing a
            // subset of the requested files. Check their claim against the
            // original task and the actual tool evidence before ending the run.
            run.emit(AgentEvent::activity(
                "status",
                S::Observing,
                "Checking the work against your request…".into(),
                it,
            ));

            let review = if verification.needs_evidence() {
                CompletionReview::Continue(verification.remaining())
            } else {
                review_completion(&client, &cfg, &objective, &reply, &tool_evidence).await
            };

            match review {
                CompletionReview::Complete => {
                    persist_final(&state, &run, &reply).await;
                    run.emit(AgentEvent::activity("final", S::Completed, reply, it));
                    return S::Completed;
                }
                CompletionReview::Continue(reason) => {
                    completion_reviews += 1;
                    let reason = reason.trim().chars().take(600).collect::<String>();
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        format!("Found remaining work. Continuing: {reason}"),
                        it,
                    ));
                    transcript.push(ChatTurn::text(
                        "user",
                        format!(
                            "Completion review found remaining work: {reason}\nContinue executing now. Do not summarize again until you have addressed this and verified the result."
                        ),
                    ));
                    if completion_reviews > limits.max_retries_per_tool + 2 {
                        let final_message = format!(
                            "I reviewed the partial work but could not finish it reliably. Remaining work: {reason}"
                        );
                        persist_final(&state, &run, &final_message).await;
                        run.emit(AgentEvent::activity("error", S::Failed, final_message, it));
                        return S::Failed;
                    }
                    continue;
                }
                CompletionReview::Unavailable(reason) => {
                    completion_reviews += 1;
                    transcript.push(ChatTurn::text(
                        "user",
                        format!(
                            "The completion check could not confirm the result ({reason}). Inspect the workspace, verify every requested deliverable, then continue or finish with evidence."
                        ),
                    ));
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        "Completion was not verified. Inspecting the project and continuing…"
                            .into(),
                        it,
                    ));
                    if completion_reviews > limits.max_retries_per_tool {
                        let final_message = "I could not verify that the work is complete because the local model stopped responding reliably. The partial work has been kept.";
                        persist_final(&state, &run, final_message).await;
                        run.emit(AgentEvent::activity(
                            "error",
                            S::Failed,
                            final_message.into(),
                            it,
                        ));
                        return S::Failed;
                    }
                    continue;
                }
            }
        };
        let progress = visible_progress(&reply);
        if !progress.is_empty() {
            run.emit(AgentEvent::activity("thought", S::Planning, progress, it));
        }

        // Validate the call before any gate or execution (§93).
        if !crate::tools::registry().iter().any(|t| t.name == call.name) {
            let known: Vec<&str> = crate::tools::registry().iter().map(|t| t.name).collect();
            transcript.push(ChatTurn::text(
                "user",
                format!(
                    "Unknown tool '{}'. Available: {}. Reply with a valid call or a summary.",
                    call.name,
                    known.join(", ")
                ),
            ));
            run.emit(AgentEvent::new(
                S::Observing,
                format!("Unknown tool '{}'; asked model to correct.", call.name),
                it,
            ));
            continue;
        }
        let risk = crate::tools::risk_of(&call.name);
        if !spec.mode.allows_risk(risk) {
            transcript.push(ChatTurn::text(
                "user",
                format!("'{call}' is disabled in this read-only run. Use reads/search and finish with a plan or findings; the user must explicitly switch to Agent to execute changes.", call = call.name),
            ));
            run.emit(AgentEvent::new(
                S::Observing,
                format!("Blocked {} (read-only mode).", call.name),
                it,
            ));
            continue;
        }

        // --- Gate (§92) ---
        // Agent mode is already folded in: CodeAssist blocks writes above,
        // Chat is refused at spawn; the shared PermissionManager decides.
        // Drop the policy read lock before awaiting a user decision. Otherwise
        // switching Ask → Auto cannot acquire the write lock to release it.
        let decision =
            state
                .permissions
                .read()
                .await
                .decide_in(&call.name, risk, true, Some(&ws_key));
        let approved = match decision {
            PermissionDecision::Allow => true,
            PermissionDecision::Deny { reason } => {
                transcript.push(ChatTurn::text(
                    "user",
                    format!("Denied: {reason}. Work around it or finish."),
                ));
                run.emit(AgentEvent::new(
                    S::Observing,
                    format!("Denied {}: {reason}", call.name),
                    it,
                ));
                continue;
            }
            PermissionDecision::RequireApproval { reason } => {
                match await_approval(&state, &run, &call, &reason, it).await {
                    Some(ApprovalDecision::Approved { session }) => {
                        if session && risk == RiskLevel::Moderate {
                            state
                                .permissions
                                .write()
                                .await
                                .grant_session(&call.name, &ws_key);
                        }
                        true
                    }
                    Some(ApprovalDecision::Denied) | None => {
                        transcript.push(ChatTurn::text(
                            "user",
                            format!(
                                "The user denied '{}'. Adjust the plan or finish with a summary.",
                                call.name
                            ),
                        ));
                        run.emit(AgentEvent::new(
                            S::Observing,
                            format!("User denied {}.", call.name),
                            it,
                        ));
                        continue;
                    }
                }
            }
        };

        // --- Execute ---
        let before = mutation_snapshot(&ws, &call);
        run.emit(AgentEvent::tool_activity(
            "tool_started",
            S::ExecutingTool,
            tool_action_label(&call, false),
            it,
            call.name.clone(),
            call.args.clone(),
            None,
            None,
        ));
        // Stage 20: documents render through the async pipeline (audited).
        let output = if call.name == "create_document" {
            let conv = if run.spec.conversation_id.trim().is_empty() {
                format!("agent:{}", run.id)
            } else {
                run.spec.conversation_id.clone()
            };
            match crate::documents::execute_create_document(
                &state.storage,
                &state.artifacts_dir,
                &conv,
                &call.args,
            )
            .await
            {
                Ok(r) => {
                    audit_tool(&state, &run, &call, &r.output).await;
                    r.output
                }
                Err(e) => format!("(document failed: {e})"),
            }
        } else if call.name == "web_search" {
            if !spec.search_enabled {
                "(web search is OFF for this run — enable Search to use it)".to_string()
            } else {
                // Search opt-in is separate from approvals. Auto never prompts
                // for enabled Search, but an explicit deny still blocks it.
                let settings = state.settings.read().await.clone();
                let global_auto = state.permissions.read().await.autonomy
                    == crate::permissions::AutonomyLevel::Autonomous;
                match settings.search.autonomous.as_str() {
                    "deny" => "(web search is disabled by policy for autonomous runs)".to_string(),
                    _ if global_auto => execute_web_search(&state, &run, &call).await,
                    _ => {
                        match await_approval(
                            &state,
                            &run,
                            &call,
                            "Web access leaves the machine (query + retrieved pages).",
                            it,
                        )
                        .await
                        {
                            Some(ApprovalDecision::Approved { .. }) => {
                                execute_web_search(&state, &run, &call).await
                            }
                            _ => "(the user denied web search — continue from local knowledge)"
                                .to_string(),
                        }
                    }
                }
            }
        } else {
            execute_local_tool(&state, &run, &ws, &call, approved).await
        };
        let shown: String = output.chars().take(TOOL_OUTPUT_CHARS).collect();
        let tool_failed = tool_output_failed(&output);
        if progress_guard.observe_tool_result(&call.name, &call.args, &output, !tool_failed) {
            response_policy.useful_progress();
            completion_reviews = 0;
        }
        verification.observe(&ws, &call, &output, tool_failed);
        let repeated_failure = failed_calls.observe(&call, tool_failed);
        tool_evidence.push(tool_evidence_line(&call, &shown));
        if tool_evidence.len() > 20 {
            tool_evidence.remove(0);
        }
        transcript.push(ChatTurn::text(
            "user",
            format!("Result of {}:\n{}", call.name, shown),
        ));
        run.emit(AgentEvent::tool_activity(
            if tool_failed {
                "tool_error"
            } else {
                "tool_result"
            },
            S::Observing,
            if tool_failed {
                format!("Action failed: {}", call.name)
            } else {
                tool_action_label(&call, true)
            },
            it,
            call.name.clone(),
            call.args.clone(),
            Some(shown),
            if tool_failed {
                None
            } else {
                mutation_diff(&ws, &call, before)
            },
        ));
        if repeated_failure {
            let message = format!("I stopped after the same {} action failed three times without a correction. Existing changes were kept. Review the recorded error and choose a different approach before continuing.", call.name);
            persist_final(&state, &run, &message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message, it));
            return S::Failed;
        }
        if tool_failed {
            run.emit(AgentEvent::activity(
                "status",
                S::Planning,
                "That step failed. Evaluating another approach…".into(),
                it,
            ));
        }
    }
    let final_message = format!(
        "I checked the partial work, but the agent reached its {}-step safety limit before the task was complete. The existing changes have been kept.",
        limits.max_iterations
    );
    persist_final(&state, &run, &final_message).await;
    run.emit(AgentEvent::activity(
        "error",
        S::Failed,
        final_message,
        limits.max_iterations,
    ));
    S::Failed
}

enum CompletionReview {
    Complete,
    Continue(String),
    Unavailable(String),
}

async fn review_completion(
    client: &SidecarClient,
    cfg: &crate::inference::InferenceConfig,
    task: &str,
    candidate: &str,
    evidence: &[String],
) -> CompletionReview {
    let evidence = if evidence.is_empty() {
        "No tools were used.".to_string()
    } else {
        evidence.join("\n")
    };
    let prompt = format!(
        "You are a strict completion checker for a coding agent. Compare the original task with the candidate final answer and the recorded tool evidence. Do not assume files exist unless the evidence shows them. For change requests, require all requested deliverables plus a post-change inspection, test, build, or other relevant validation. If the task is fully complete or genuinely impossible with an honest explanation, reply exactly COMPLETE. Otherwise reply CONTINUE: followed by one concise description of the missing work.\n\nOriginal task:\n{task}\n\nCandidate final answer:\n{candidate}\n\nTool evidence:\n{evidence}"
    );
    let turns = [ChatTurn::text("user", prompt)];
    match client.chat_turns_without_reasoning(&turns, 220, cfg).await {
        Ok((answer, _)) => parse_completion_review(&answer),
        Err(error) => CompletionReview::Unavailable(error.to_string()),
    }
}

fn parse_completion_review(answer: &str) -> CompletionReview {
    let trimmed = answer.trim();
    let upper = trimmed.to_ascii_uppercase();
    if upper == "COMPLETE" || upper.starts_with("COMPLETE\n") {
        CompletionReview::Complete
    } else if upper.starts_with("CONTINUE") {
        let reason = trimmed
            .split_once(':')
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or("Inspect the current project and finish every requested deliverable.");
        CompletionReview::Continue(reason.to_string())
    } else {
        CompletionReview::Unavailable(
            "The completion checker returned no usable decision. This is not evidence that the requested work is complete."
                .into(),
        )
    }
}

/// Conservative command classification is evidence labeling, not a sandbox or
/// permission bypass. Shell execution follows the user's Ask/Auto mode.
fn simple_command_words(command: &str) -> Vec<String> {
    if command.contains([
        ';', '&', '|', '>', '<', '\n', '\r', '`', '$', '(', ')', '\'', '"',
    ]) {
        return vec![];
    }
    command
        .split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn verification_command(command: &str) -> bool {
    let words = simple_command_words(command);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["npm" | "pnpm" | "yarn" | "bun", "test" | "build" | "check" | "lint" | "typecheck", ..] => {
            true
        }
        ["npm" | "pnpm" | "yarn" | "bun", "run", "test" | "build" | "check" | "lint" | "typecheck", ..] => {
            true
        }
        ["cargo", "test" | "check" | "build" | "clippy", ..] => true,
        ["go", "test" | "vet" | "build", ..] => true,
        ["dotnet", "test" | "build", ..] => true,
        ["pytest" | "pytest.exe" | "ruff" | "mypy", ..] => true,
        ["python" | "python3" | "py", "-m", "pytest" | "unittest" | "compileall", ..] => true,
        ["node", "--test", ..] => true,
        ["node", file, ..] => {
            let file_name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            file.ends_with(".test.js")
                || file.ends_with(".test.mjs")
                || file.ends_with(".test.cjs")
                || file.ends_with(".spec.js")
                || file.ends_with(".spec.mjs")
                || file.ends_with(".spec.cjs")
                || matches!(
                    file_name,
                    "verify.js"
                        | "verify.mjs"
                        | "verify.cjs"
                        | "check.js"
                        | "check.mjs"
                        | "check.cjs"
                        | "test.js"
                        | "test.mjs"
                        | "test.cjs"
                )
        }
        _ => false,
    }
}

fn read_only_command(command: &str) -> bool {
    let words = simple_command_words(command);
    matches!(
        words.first().map(String::as_str),
        Some(
            "echo"
                | "pwd"
                | "ls"
                | "dir"
                | "type"
                | "cat"
                | "get-content"
                | "get-childitem"
                | "get-location"
        )
    ) || matches!(words.as_slice(), [git, sub, ..] if git == "git" && matches!(sub.as_str(), "status" | "diff" | "log" | "show"))
}

#[derive(Default)]
struct VerificationState {
    /// A later mutation re-inserts its path: older reads cannot verify a new revision.
    uninspected: HashSet<PathBuf>,
    unknown_shell_changes: bool,
}

impl VerificationState {
    fn needs_evidence(&self) -> bool {
        !self.uninspected.is_empty() || self.unknown_shell_changes
    }

    fn remaining(&self) -> String {
        if self.unknown_shell_changes {
            "A command may have changed the project. Run a relevant test, build or validation and report its real result; a directory listing or unrelated read is not verification.".into()
        } else {
            format!("Inspect the latest contents of the files you changed ({}), or run a relevant test/build. Do not call a read a passing test.", self.uninspected.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>().join(", "))
        }
    }

    fn observe(
        &mut self,
        ws: &crate::workspace::WorkspaceManager,
        call: &ToolCall,
        output: &str,
        failed: bool,
    ) {
        if call.name == "execute_command" {
            let command = arg_str(call, "command").unwrap_or_default();
            if verification_command(command) {
                // The full host-produced result includes its real process exit.
                // Failed validation cannot discharge earlier mutation evidence.
                if !failed && command_exit_code(output) == Some(0) {
                    if let Ok(cwd) = ws.resolve(arg_str(call, "cwd").unwrap_or(".")) {
                        self.uninspected.retain(|path| !path.starts_with(&cwd));
                        if ws.resolve(".").ok().as_ref() == Some(&cwd) {
                            self.unknown_shell_changes = false;
                        }
                    }
                }
            } else if !read_only_command(command) {
                // A command may partially mutate files even with nonzero exit.
                self.unknown_shell_changes = true;
            }
            return;
        }
        if failed {
            return;
        }
        let path = arg_str(call, "path").and_then(|path| ws.resolve(path).ok());
        match call.name.as_str() {
            "write_file" | "edit_file" => {
                if let Some(path) = path {
                    self.uninspected.insert(path);
                }
            }
            "delete_file" => {
                // Host-checked absence is deletion evidence, never test evidence.
                if let Some(path) = path {
                    if !path.exists() {
                        self.uninspected.remove(&path);
                    } else {
                        self.uninspected.insert(path);
                    }
                }
            }
            "read_file" => {
                if let Some(path) = path {
                    self.uninspected.remove(&path);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct FailedCalls(HashMap<String, u32>);

impl FailedCalls {
    fn observe(&mut self, call: &ToolCall, failed: bool) -> bool {
        let key = format!("{}:{}", call.name, call.args);
        if !failed {
            self.0.remove(&key);
            return false;
        }
        let count = self.0.entry(key).or_default();
        *count += 1;
        *count >= 3
    }
}

fn tool_evidence_line(call: &ToolCall, output: &str) -> String {
    let target = arg_str(call, "path")
        .or_else(|| arg_str(call, "query"))
        .or_else(|| arg_str(call, "command"))
        .unwrap_or("");
    let result = output
        .replace(['\r', '\n'], " ")
        .chars()
        .take(320)
        .collect::<String>();
    let evidence_kind = match call.name.as_str() {
        "read_file" | "list_directory" | "search_text" => "INSPECTION ONLY (not a test)",
        "execute_command" if verification_command(target) => {
            "VALIDATION COMMAND (check actual exit)"
        }
        "execute_command" => "COMMAND (not verification; may mutate)",
        "write_file" | "edit_file" | "delete_file" => "FILE CHANGE (not verification)",
        _ => "ACTION",
    };
    format!(
        "- {evidence_kind}: {} [{}] => {}",
        call.name, target, result
    )
}

fn command_exit_code(output: &str) -> Option<i32> {
    output
        .strip_prefix("Command:\n")?
        .split_once("\n\nExit code:\n")?
        .1
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn tool_output_failed(output: &str) -> bool {
    output.starts_with("(tool reported failure)")
        || output.starts_with("(tool error")
        || output.starts_with("(document failed:")
        || output.starts_with("(web search failed:")
        || output.starts_with("(web search is OFF")
        || output.starts_with("(web search is disabled")
        || output.starts_with("(the user denied web search")
        || (output.starts_with("Command:\n") && command_exit_code(output) != Some(0))
}

/// Pause the loop until the user approves/denies (or the run is stopped).
async fn await_approval(
    state: &crate::api::AppState,
    run: &Arc<LiveRun>,
    call: &ToolCall,
    reason: &str,
    it: u32,
) -> Option<ApprovalDecision> {
    let (tx, rx) = oneshot::channel();
    let pending = PendingTool {
        tool: call.name.clone(),
        args: call.args.clone(),
        reason: reason.into(),
    };
    *run.pending.lock().expect("lock") = Some(pending.clone());
    *run.pending_tx.lock().expect("lock") = Some(tx);
    run.emit({
        let mut ev = AgentEvent::new(
            AgentState::WaitingPermission,
            format!("{} needs approval: {reason}", call.name),
            it,
        );
        ev.pending_tool = Some(pending);
        ev.kind = "permission".into();
        ev.tool = Some(call.name.clone());
        ev.args = Some(call.args.clone());
        ev
    });
    {
        // Publishing before taking the settings-update lock closes both race
        // orders: an Auto setter sees this pending action, or this re-check
        // sees the Auto preference that was persisted just before publication.
        // Never hold this lock (or a policy read guard) while waiting on the user.
        let _update = state.settings_update.lock().await;
        crate::api::resume_auto_approved_runs(state).await;
    }
    let decision = rx.await.ok();
    *run.pending.lock().expect("lock") = None;
    decision
}

async fn execute_local_tool(
    state: &crate::api::AppState,
    run: &LiveRun,
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
    approved: bool,
) -> String {
    let tool_req = crate::tools::ToolRequest {
        name: call.name.clone(),
        args: call.args.clone(),
        approved,
    };
    let output = match crate::tools::execute(&tool_req, ws, true) {
        Ok(r) => {
            let mut text = r.output;
            if !r.ok {
                text = format!("(tool reported failure)\n{text}");
            }
            text
        }
        Err(e) => format!("(tool error, do not retry identically)\n{e}"),
    };
    audit_tool(state, run, call, &output).await;
    output
}

async fn execute_web_search(
    state: &crate::api::AppState,
    run: &LiveRun,
    call: &ToolCall,
) -> String {
    let query = call
        .args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let settings = state.settings.read().await.clone();
    let scfg = crate::search::SearchConfig {
        provider: settings.search.provider.clone(),
        brave_key: settings.search.brave_key.clone(),
        custom_url: settings.search.custom_url.clone(),
        max_results: settings.search.max_results,
        timeout_secs: settings.search.timeout_secs,
    };
    let output = match crate::search::run_search(&query, &scfg).await {
        Ok((results, provider)) => {
            let conv = if run.spec.conversation_id.trim().is_empty() {
                format!("agent:{}", run.id)
            } else {
                run.spec.conversation_id.clone()
            };
            let st = state.storage.lock().await;
            let _ = st.record_search_run(&crate::storage::SearchRun {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv.clone(),
                query: query.chars().take(500).collect(),
                provider: provider.clone(),
                result_count: results.len(),
                created_at: chrono::Utc::now().to_rfc3339(),
            });
            let mut text = format!("Web results via {provider}:\n");
            for (i, r) in results.iter().enumerate() {
                text.push_str(&format!(
                    "{}. {} ({})\n{}\n",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                ));
            }
            let _ = st.record_tool_execution(&crate::storage::ToolExecution {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv,
                tool: "web_search".into(),
                args: call.args.to_string().chars().take(4000).collect(),
                result: text.chars().take(4000).collect(),
                approved: true,
                created_at: chrono::Utc::now().to_rfc3339(),
            });
            text
        }
        Err(e) => format!(
            "(web search failed: {e}. Continue from local knowledge; never invent results.)"
        ),
    };
    output
}

fn short_args(args: &serde_json::Value) -> String {
    let s = args.to_string();
    if s.len() > 160 {
        format!("{}…", s.chars().take(160).collect::<String>())
    } else {
        s
    }
}

fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    call.args.get(key).and_then(|v| v.as_str())
}

fn tool_action_label(call: &ToolCall, completed: bool) -> String {
    let target = arg_str(call, "path")
        .or_else(|| arg_str(call, "query"))
        .or_else(|| arg_str(call, "command"))
        .unwrap_or("");
    let verb = match (call.name.as_str(), completed) {
        ("read_file", false) => "Reading file",
        ("read_file", true) => "Read file",
        ("list_directory", false) => "Listing directory",
        ("list_directory", true) => "Listed directory",
        ("search_text", false) => "Searching code",
        ("search_text", true) => "Searched code",
        ("web_search", false) => "Searching the web",
        ("web_search", true) => "Searched the web",
        ("edit_file", false) => "Modifying file",
        ("edit_file", true) => "Modified file",
        ("write_file", false) => "Writing file",
        ("write_file", true) => "Wrote file",
        ("delete_file", false) => "Deleting file",
        ("delete_file", true) => "Deleted file",
        ("execute_command", false) => "Running command",
        ("execute_command", true) => "Ran command",
        ("git_commit", false) => "Creating commit",
        ("git_commit", true) => "Created commit",
        ("create_document", false) => "Creating document",
        ("create_document", true) => "Created document",
        (_, false) => "Running tool",
        (_, true) => "Finished tool",
    };
    if target.is_empty() {
        format!("{verb}: {}", call.name)
    } else {
        format!("{verb}: {target}")
    }
}

fn mutation_snapshot(
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
) -> Option<Option<String>> {
    if !["edit_file", "write_file", "delete_file"].contains(&call.name.as_str()) {
        return None;
    }
    let path = ws.resolve(arg_str(call, "path")?).ok()?;
    if !path.exists() {
        return Some(None);
    }
    if std::fs::metadata(&path).ok()?.len() > 64_000 {
        return None;
    }
    Some(Some(std::fs::read_to_string(path).ok()?))
}

fn mutation_diff(
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
    before: Option<Option<String>>,
) -> Option<String> {
    let before = before?;
    let after = mutation_snapshot(ws, call)?;
    if before == after {
        return None;
    }
    let path = arg_str(call, "path")?;
    let old = before.as_deref().unwrap_or("");
    let new = after.as_deref().unwrap_or("");
    let removed = old
        .lines()
        .map(|line| format!("-{line}\n"))
        .collect::<String>();
    let added = new
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    Some(format!(
        "--- {}\n+++ {}\n@@ -1,{} +1,{} @@\n{removed}{added}",
        if before.is_some() {
            format!("a/{path}")
        } else {
            "/dev/null".into()
        },
        if after.is_some() {
            format!("b/{path}")
        } else {
            "/dev/null".into()
        },
        old.lines().count(),
        new.lines().count()
    ))
}

async fn audit_tool(state: &crate::api::AppState, run: &LiveRun, call: &ToolCall, output: &str) {
    let conv = if run.spec.conversation_id.trim().is_empty() {
        format!("agent:{}", run.id)
    } else {
        run.spec.conversation_id.clone()
    };
    let st = state.storage.lock().await;
    let _ = st.record_tool_execution(&crate::storage::ToolExecution {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conv,
        tool: call.name.clone(),
        args: call.args.to_string().chars().take(4000).collect(),
        result: output.chars().take(4000).collect(),
        approved: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    });
}

async fn persist_final(state: &crate::api::AppState, run: &LiveRun, content: &str) {
    if run.spec.conversation_id.trim().is_empty() {
        return;
    }
    let st = state.storage.lock().await;
    if !matches!(st.get_conversation(&run.spec.conversation_id), Ok(Some(_))) {
        return;
    }
    let _ = st.update_message_content(&run.spec.conversation_id, &run.id, content);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Explicit opt-in diagnostic: generates text only, never executes tools.
    #[tokio::test]
    #[ignore = "requires an idle local model and COMPANION_ACTION_PROBE_URL"]
    async fn local_model_action_format_probe() {
        let url = std::env::var("COMPANION_ACTION_PROBE_URL").expect("explicit probe URL required");
        assert!(url.starts_with("http://127.0.0.1:"), "local probe only");
        let prompt = system_prompt("C:/isolated-fixture", &crate::tools::registry(), false);
        let task = std::env::var("COMPANION_ACTION_PROBE_TASK").unwrap_or_else(|_| "In this isolated evaluation fixture, inspect calculator.cjs and verify.cjs. Fix add(a,b) so it returns the sum, delete only disposable.txt, then execute node verify.cjs and report its actual result. Do not change verify.cjs, install packages, or use the network. Keep the change minimal.".into());
        let mut turns = vec![ChatTurn::text("system", prompt), ChatTurn::text("user", format!("Task: {task}\n\nAddress this request within its scope. Use the complete tool envelope shown in the system instructions if an action is necessary; otherwise answer directly. Do not invent a new task or mutate files for a question."))];
        let structured = std::env::var("COMPANION_ACTION_PROBE_STRUCTURED").is_ok();
        if structured {
            turns.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        }
        let reply = SidecarClient::new(url)
            .unwrap()
            .agent_chat_turns(
                &turns,
                512,
                &crate::inference::InferenceConfig::default(),
                true,
                structured,
            )
            .await
            .unwrap();
        eprintln!(
            "Visible action only: {:?}; finish: {:?}",
            reply.text, reply.finish_reason
        );
        assert!(!reply.is_truncated());
        assert!(parse_action_response(&reply.text).is_some());
    }

    async fn approval_race_fixture(state: &crate::api::AppState) -> Arc<LiveRun> {
        let (activity_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let run = Arc::new(LiveRun {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            spec: AgentSpec {
                workspace: std::env::temp_dir(),
                task: "Approval race fixture; no execution".into(),
                mode: AgentMode::Agent,
                conversation_id: String::new(),
                search_enabled: false,
            },
            cancel: CancelToken::new(),
            events: Mutex::new(vec![]),
            broadcaster: broadcast::channel(8).0,
            activity_tx,
            pending: Mutex::new(None),
            pending_tx: Mutex::new(None),
            handle: Mutex::new(None),
        });
        state.agents.write().await.insert(run.clone());
        run
    }

    async fn save_auto_for_race_test(state: &crate::api::AppState) {
        // The test caller holds settings_update just as the real setters do.
        let mut settings = state.settings.read().await.clone();
        settings.agent.autonomous_enabled = true;
        state.storage.lock().await.save_settings(&settings).unwrap();
        *state.settings.write().await = settings;
        state.permissions.write().await.autonomy = crate::permissions::AutonomyLevel::Autonomous;
    }

    #[tokio::test]
    async fn approval_rechecks_auto_saved_before_pending_action_is_published() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        {
            let _update = state.settings_update.lock().await;
            save_auto_for_race_test(&state).await;
            assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 0);
        }
        let call = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"echo fixture"}),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            await_approval(&state, &run, &call, "Earlier Ask decision", 1),
        )
        .await
        .expect("a just-published action must see already-saved Auto");
        assert!(matches!(
            result,
            Some(ApprovalDecision::Approved { session: false })
        ));
        assert!(run.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_published_before_auto_save_is_resumed_without_lock_deadlock() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        let mut events = run.broadcaster.subscribe();
        let update = state.settings_update.lock().await;
        let bg_state = state.clone();
        let bg_run = run.clone();
        let waiter = tokio::spawn(async move {
            let call = ToolCall {
                name: "delete_file".into(),
                args: serde_json::json!({"path":"fixture"}),
            };
            await_approval(&bg_state, &bg_run, &call, "Earlier Ask decision", 1).await
        });
        let waiting = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.state, AgentState::WaitingPermission);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            save_auto_for_race_test(&state),
        )
        .await
        .expect("waiting for approval must not retain a permission read lock");
        assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 1);
        drop(update);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result,
            Some(ApprovalDecision::Approved { session: false })
        ));
        assert!(run.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_recheck_keeps_ask_waiting_for_an_explicit_decision() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        let mut events = run.broadcaster.subscribe();
        let bg_state = state.clone();
        let bg_run = run.clone();
        let waiter = tokio::spawn(async move {
            let call = ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"echo fixture"}),
            };
            await_approval(&bg_state, &bg_run, &call, "Ask", 1).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        {
            let _update = state.settings_update.lock().await;
            assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 0);
        }
        assert!(!waiter.is_finished());
        run.pending_tx
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(ApprovalDecision::Denied)
            .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Some(ApprovalDecision::Denied)));
        assert_eq!(
            state.permissions.read().await.autonomy,
            crate::permissions::AutonomyLevel::Assisted
        );
    }

    #[test]
    fn action_response_policy_allows_only_one_recovery_without_useful_progress() {
        let mut policy = ActionResponsePolicy::default();
        assert!(!policy.disable_native_thinking);
        assert_eq!(policy.output_budget(32768, 1000), 2048);
        assert!(
            policy.recover_invalid(),
            "the first invalid response gets one changed-strategy attempt"
        );
        assert!(policy.disable_native_thinking);
        assert_eq!(policy.output_budget(32768, 1000), 4096);
        assert!(
            !policy.recover_invalid(),
            "changing from empty to malformed does not earn another retry"
        );
        assert!(
            !policy.recover_invalid(),
            "repeated calls cannot restart recovery"
        );
        assert!(policy.disable_native_thinking);
    }

    #[test]
    fn action_response_policy_preserves_context_headroom_and_handles_extreme_counts() {
        let mut policy = ActionResponsePolicy::default();
        assert_eq!(policy.output_budget(4096, 2000), 1840);
        assert_eq!(policy.output_budget(4096, 3712), 128);
        assert_eq!(policy.output_budget(4096, 3800), 40);
        assert_eq!(policy.output_budget(4096, 4096), 0);
        assert_eq!(policy.output_budget(0, 0), 0);
        assert_eq!(policy.output_budget(u32::MAX, u32::MAX), 0);
        assert_eq!(policy.output_budget(4096, u32::MAX), 0);
        assert!(policy.recover_invalid());
        assert_eq!(
            policy.output_budget(4096, 2000),
            1840,
            "recovery cannot override context capacity"
        );
        assert_eq!(policy.output_budget(8192, 1000), 4096);
    }

    #[test]
    fn action_response_policy_new_success_renews_retry_but_keeps_native_thinking_disabled() {
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        let args = serde_json::json!({"path":"README.md"});
        assert!(progress.begin_attempt());
        assert!(policy.recover_invalid());
        assert!(progress.begin_attempt());
        assert!(progress.observe_tool_result("read_file", &args, "actual fixture text", true));
        policy.useful_progress();
        assert_eq!(progress.attempts_without_progress(), 0);
        assert!(
            policy.disable_native_thinking,
            "a successful action must not reactivate the strategy that exhausted its budget"
        );
        assert_eq!(policy.output_budget(32768, 1000), 4096);
        assert!(
            policy.recover_invalid(),
            "new host-confirmed evidence starts a new bounded recovery window"
        );
        assert!(!policy.recover_invalid());
        assert!(
            !ActionResponsePolicy::default().disable_native_thinking,
            "the override must not leak into another run"
        );
    }

    #[test]
    fn action_response_policy_failed_or_duplicate_tool_does_not_renew_retry() {
        for succeeded in [false, true] {
            let mut policy = ActionResponsePolicy::default();
            let mut progress = crate::agent_progress::ProgressGuard::new(4);
            let args = serde_json::json!({"path":"README.md"});
            if succeeded {
                assert!(progress.observe_tool_result("read_file", &args, "same text", true));
            }
            assert!(progress.begin_attempt());
            assert!(policy.recover_invalid());
            assert!(progress.begin_attempt());
            if progress.observe_tool_result("read_file", &args, "same text", succeeded) {
                policy.useful_progress();
            }
            assert_eq!(progress.attempts_without_progress(), 2);
            assert!(
                !policy.recover_invalid(),
                "failed or duplicate tool results are not grounds for another invalid-output retry"
            );
        }
    }

    fn action_response_fixture(
        text: &str,
        finish_reason: &str,
    ) -> crate::llamaserver::AgentCompletion {
        crate::llamaserver::AgentCompletion {
            text: text.into(),
            metrics: crate::inference::Metrics::default(),
            finish_reason: Some(finish_reason.into()),
            reasoning_present: false,
            reasoning_tokens: None,
            native_tool_calls_present: false,
        }
    }

    #[test]
    fn continuation_joins_exact_text_and_validates_only_the_completed_action() {
        let prefix = "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"hello.txt\",\"content\":\"Hello ";
        let suffix = "world\\n\"}}\n```";
        let cutoff = action_response_fixture(prefix, "length");
        assert!(can_continue_response(&cutoff));
        assert!(parse_tool_block(prefix).is_none());
        let combined = assemble_continuation(prefix, suffix).unwrap();
        let completed = action_response_fixture(&combined, "stop");
        let parsed = parse_tool_block(&combined).unwrap();
        assert!(action_response_invalid_reason(&completed, Some(&parsed), 4096).is_none());
        assert_eq!(parsed.args["content"], "Hello world\n");
        let still_cutoff = action_response_fixture(&combined, "length");
        assert!(action_response_invalid_reason(&still_cutoff, Some(&parsed), 4096).is_some());
    }

    #[test]
    fn continuation_accepts_exact_prefix_replay_but_not_empty_or_duplicate_actions() {
        assert_eq!(
            assemble_continuation("Hello ", "Hello world").unwrap(),
            "Hello world"
        );
        assert!(assemble_continuation("Hello", "Hello").is_none());
        assert!(assemble_continuation("Hello", " \n").is_none());
        let action = "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```";
        let combined = assemble_continuation(action, &format!("\n{action}")).unwrap();
        assert!(
            parse_tool_block(&combined).is_none(),
            "continuation must not smuggle a second action"
        );
        assert!(assemble_continuation(&"a".repeat(32768), &"b".repeat(32769)).is_none());
    }

    #[test]
    fn continuation_carries_partial_text_in_context_without_mutating_base_history() {
        let base = vec![ChatTurn::text("user", "Create a small file.")];
        let prefix = "x".repeat(4000);
        let request = continuation_turns(&base, &prefix);
        assert_eq!(base.len(), 1);
        assert_eq!(request.len(), 3);
        assert_eq!(request[1].role, "assistant");
        assert_eq!(request[1].content, prefix);
        let before = crate::agent::AgentContextUsage::for_turns(&base, 4096, 0, 0);
        let after = crate::agent::AgentContextUsage::for_turns(&request, 4096, 0, 0);
        assert!(after.estimated_tokens > before.estimated_tokens + 1000);
    }

    #[test]
    fn continuation_is_only_for_bounded_visible_truncation_and_shares_retry_limit() {
        for completion in [
            action_response_fixture("", "length"),
            action_response_fixture("complete", "stop"),
            action_response_fixture(&"x".repeat(32769), "length"),
        ] {
            assert!(!can_continue_response(&completion));
        }
        let mut native = action_response_fixture("some content", "length");
        native.native_tool_calls_present = true;
        assert!(!can_continue_response(&native));
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        assert!(progress.begin_attempt());
        assert!(policy.begin_continuation());
        assert!(policy.disable_native_thinking);
        assert!(progress.begin_attempt());
        assert!(
            !policy.begin_continuation(),
            "a continuation cannot chain another continuation"
        );
        assert!(
            policy.recover_invalid(),
            "a failed continuation permits one clean small-action fallback"
        );
        assert!(progress.begin_attempt());
        assert!(
            !policy.recover_invalid(),
            "a failed clean fallback must stop"
        );
        assert!(
            !policy.begin_continuation(),
            "format failures cannot reset the continuation budget"
        );
        assert_eq!(progress.attempts_without_progress(), 3);
        policy.useful_progress();
        assert!(
            policy.begin_continuation(),
            "only useful progress renews the allowance"
        );
    }

    #[test]
    fn action_response_validation_rejects_cutoff_even_when_the_tool_fence_is_parseable() {
        let text = "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"index.html\",\"content\":\"partial page\"}}\n```";
        let cutoff = action_response_fixture(text, "length");
        let parsed = parse_tool_block(&cutoff.text);
        assert!(
            parsed.is_some(),
            "syntactically complete payload deliberately exercises the provider cutoff guard"
        );
        let reason = action_response_invalid_reason(&cutoff, parsed.as_ref(), 2048).unwrap();
        assert!(reason.contains("2048-token output limit"));
        let mut complete = cutoff;
        complete.finish_reason = Some("stop".into());
        assert!(action_response_invalid_reason(&complete, parsed.as_ref(), 2048).is_none());
    }

    #[test]
    fn action_response_validation_distinguishes_empty_reasoning_native_calls_and_malformed_fences()
    {
        for text in ["", " ", "\n\t"] {
            let empty = action_response_fixture(text, "stop");
            assert!(action_response_invalid_reason(&empty, None, 2048)
                .unwrap()
                .contains("no visible"));
        }
        let mut hidden_only = action_response_fixture("", "stop");
        hidden_only.reasoning_present = true;
        assert!(action_response_invalid_reason(&hidden_only, None, 2048)
            .unwrap()
            .contains("reasoning but no visible"));
        hidden_only.native_tool_calls_present = true;
        assert!(action_response_invalid_reason(&hidden_only, None, 2048)
            .unwrap()
            .contains("unsupported action format"));
        for text in [
            "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"}}",
            "```tool\nnot json\n```",
            "```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n```tool\n{\"name\":\"write_file\",\"args\":{}}\n```",
        ] {
            let malformed = action_response_fixture(text, "stop");
            let parsed = parse_tool_block(text);
            assert!(parsed.is_none());
            assert!(action_response_invalid_reason(&malformed, parsed.as_ref(), 2048).unwrap().contains("incomplete or unreadable"));
        }
        let final_answer =
            action_response_fixture("The README describes a local notes app.", "stop");
        assert!(action_response_invalid_reason(&final_answer, None, 2048).is_none());
        let mut valid_tool = action_response_fixture(
            "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"}}\n```",
            "stop",
        );
        valid_tool.reasoning_present = true;
        let parsed = parse_tool_block(&valid_tool.text);
        assert!(
            action_response_invalid_reason(&valid_tool, parsed.as_ref(), 2048).is_none(),
            "reported reasoning is not itself a failure when the visible action completed"
        );
    }

    #[test]
    fn action_response_validation_alternating_empty_and_malformed_still_stops_after_one_retry() {
        let mut policy = ActionResponsePolicy::default();
        let responses = [
            action_response_fixture("", "length"),
            action_response_fixture("```tool\n{\"name\":\"read_file\"", "stop"),
        ];
        let mut retries = 0;
        for response in &responses {
            let parsed = parse_tool_block(&response.text);
            assert!(action_response_invalid_reason(response, parsed.as_ref(), 2048).is_some());
            if policy.recover_invalid() {
                retries += 1;
            } else {
                break;
            }
        }
        assert_eq!(retries, 1);
        assert_eq!(policy.invalid_since_progress, 2);
    }

    #[test]
    fn unavailable_or_denied_search_is_failed_evidence_not_progress() {
        for output in [
            "(web search is OFF for this run — continue from local knowledge)",
            "(web search is disabled by network policy)",
            "(the user denied web search — continue from local knowledge)",
            "(web search failed: request timeout)",
            "(tool reported failure)\nprocess failed",
        ] {
            assert!(tool_output_failed(output), "{output}");
        }
        assert!(!tool_output_failed(
            "Web results via provider:\n1. Fixture result"
        ));
        assert!(!tool_output_failed("No search matches."));
    }

    #[test]
    fn native_gemma_action_normalizes_to_the_same_validated_tool() {
        let text = "<|tool_call>call:list_directory{args:{\"path\":\".\"}}<tool_call|>";
        let call = parse_action_response(text).unwrap();
        assert_eq!(call.name, "list_directory");
        assert_eq!(call.args, serde_json::json!({"path":"."}));
        assert_eq!(visible_progress(text), "");
        let cutoff = action_response_fixture(text, "length");
        assert!(action_response_invalid_reason(&cutoff, Some(&call), 2048).is_some());
    }

    #[test]
    fn structured_tools_and_final_answers_are_distinct_and_validated() {
        let action = r#"{"kind":"tool","name":"execute_command","args":{"command":"node verify.cjs"},"answer":""}"#;
        let call = parse_action_response(action).unwrap();
        assert_eq!(call.name, "execute_command");
        assert_eq!(call.args["command"], "node verify.cjs");
        assert!(visible_progress(action).is_empty());
        let final_text =
            r#"{"kind":"final","name":"","args":{},"answer":"Verified.\nAll checks passed."}"#;
        assert!(parse_action_response(final_text).is_none());
        assert_eq!(
            parse_structured_reply(final_text).unwrap().answer,
            "Verified.\nAll checks passed."
        );
        assert!(action_response_invalid_reason(
            &action_response_fixture(final_text, "stop"),
            None,
            4096
        )
        .is_none());
    }

    #[test]
    fn structured_envelope_rejects_ambiguous_duplicate_and_incomplete_fields() {
        for reply in [
            r#"{"kind":"tool","name":"read_file","args":[],"answer":""}"#,
            r#"{"kind":"tool","name":"","args":{},"answer":""}"#,
            r#"{"kind":"final","name":"","args":{},"answer":""}"#,
            r#"{"kind":"tool","name":"read_file","args":{},"answer":"","extra":1}"#,
            r#"{"kind":"tool","kind":"final","name":"read_file","args":{},"answer":""}"#,
            r#"{"kind":"tool","name":"read_file","args":{},"answer":""} {}"#,
        ] {
            assert!(parse_structured_reply(reply).is_none(), "accepted: {reply}");
            assert!(action_response_invalid_reason(
                &action_response_fixture(reply, "stop"),
                None,
                4096
            )
            .is_some());
        }
    }

    #[test]
    fn structured_kind_controls_execution_without_rejecting_unused_model_notes() {
        let tool = r#"{"kind":"tool","name":"read_file","args":{"path":"x"},"answer":"I will inspect this file."}"#;
        assert_eq!(parse_action_response(tool).unwrap().name, "read_file");
        let final_text = r#"{"kind":"final","name":"delete_file","args":{"path":"x"},"answer":"I could not complete it."}"#;
        assert!(
            parse_action_response(final_text).is_none(),
            "unused action metadata must never execute in a final response"
        );
        assert_eq!(
            parse_structured_reply(final_text).unwrap().answer,
            "I could not complete it."
        );
    }

    #[test]
    fn only_clean_recovery_enables_schema_and_success_keeps_the_working_protocol() {
        let mut policy = ActionResponsePolicy::default();
        assert!(!policy.structured_fallback);
        assert!(policy.begin_continuation());
        assert!(
            !policy.structured_fallback,
            "suffix continuation cannot be constrained as a whole JSON document"
        );
        assert!(policy.recover_invalid());
        assert!(policy.structured_fallback);
        policy.useful_progress();
        assert!(policy.structured_fallback);
        let base = vec![ChatTurn::text("user", "Test")];
        let mut constrained = base.clone();
        constrained.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        assert!(
            crate::agent::AgentContextUsage::for_turns(&constrained, 32768, 4096, 0)
                .estimated_tokens
                > crate::agent::AgentContextUsage::for_turns(&base, 32768, 4096, 0)
                    .estimated_tokens
        );
    }

    #[test]
    fn native_action_rejects_multiple_quoted_trailing_or_non_object_envelopes() {
        let valid = "<|tool_call>call:read_file{args:{\"path\":\"x\"}}<tool_call|>";
        for text in [
            format!("{valid}{valid}"),
            format!("Example: {valid}"),
            format!("{valid} explanation"),
            format!("```text\n{valid}\n```"),
            "<|tool_call>call:read_file{args:[]}<tool_call|>".into(),
            "<|tool_call>call:read_file{args:{\"path\":\"x\"}}".into(),
            "<|tool_call>call:read_file;command{args:{}}<tool_call|>".into(),
            "<|tool_call>call:read_file{args:{},another:{}}<tool_call|>".into(),
        ] {
            assert!(
                parse_action_response(&text).is_none(),
                "accepted ambiguous native action: {text}"
            );
            let response = action_response_fixture(&text, "stop");
            assert!(action_response_invalid_reason(&response, None, 4096).is_some());
        }
    }

    #[test]
    fn prompt_contains_a_complete_parseable_action_example() {
        let prompt = system_prompt("C:/fixture", &crate::tools::registry(), false);
        let start = prompt.find("```tool\n").unwrap();
        let end = prompt[start + 8..].find("\n```").unwrap() + start + 8 + 4;
        let example = &prompt[start..end];
        let call = parse_tool_block(example).unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "README.md");
        assert_eq!(
            prompt.matches("```").count(),
            2,
            "the prompt has one balanced example only"
        );
    }

    #[test]
    fn source_text_mentioning_failure_markers_is_not_itself_failed_evidence() {
        for output in [
            "fn example() { let message = \"(tool error: fixture)\"; }",
            "Documentation: (tool reported failure) is an error marker.",
            "A test checks for (document failed: timeout).",
            "Search result: (web search failed: timeout) appears in this file.",
        ] {
            assert!(
                !tool_output_failed(output),
                "ordinary successful read/search text was classified as a host error: {output}"
            );
        }
    }

    #[test]
    fn read_only_modes_never_allow_mutating_risks() {
        for mode in [AgentMode::Plan, AgentMode::CodeAssist] {
            assert!(mode.allows_risk(RiskLevel::Safe));
            assert!(!mode.allows_risk(RiskLevel::Moderate));
            assert!(!mode.allows_risk(RiskLevel::Dangerous));
        }
        assert_eq!(
            serde_json::from_str::<AgentMode>("\"plan\"").unwrap(),
            AgentMode::Plan
        );
    }

    #[test]
    fn mutation_diff_records_actual_overwrite_and_not_proposed_changes() {
        let root = std::env::temp_dir().join(format!("companion-diff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let ws = crate::workspace::WorkspaceManager::new(root.clone());
        let call = ToolCall {
            name: "write_file".into(),
            args: serde_json::json!({"path": "sample.txt", "content": "proposed"}),
        };
        std::fs::write(root.join("sample.txt"), "before\n").unwrap();
        let before = mutation_snapshot(&ws, &call);
        assert!(mutation_diff(&ws, &call, before.clone()).is_none());
        std::fs::write(root.join("sample.txt"), "actual\n").unwrap();
        let diff = mutation_diff(&ws, &call, before).unwrap();
        assert!(diff.contains("--- a/sample.txt"));
        assert!(diff.contains("-before"));
        assert!(diff.contains("+actual"));
        assert!(!diff.contains("proposed"));
        std::fs::remove_file(root.join("sample.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn parses_one_standalone_tool_block_with_progress() {
        let text = "I'll inspect the entry point.\n```tool\n{\"name\": \"read_file\", \"args\": {\"path\": \"x\"}}\n```";
        let call = parse_tool_block(text).unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "x");
    }

    #[test]
    fn rejects_multiple_or_unfinished_action_fences() {
        let read = "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```";
        let edit =
            "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"x\",\"content\":\"new\"}}\n```";
        for text in [
            format!("{read}\n{edit}"),
            format!("{read}\nThen edit:\n{edit}"),
            format!("{read}\n```tool\n{{\"name\":\"write_file\""),
            "```tool\n{\"name\":\"read_file\",\"args\":{}}".into(),
            "```tool\n{\"name\":\"read_file\",\"args\":{}}\n``".into(),
        ] {
            assert!(
                parse_tool_block(&text).is_none(),
                "accepted ambiguous action: {text}"
            );
        }
    }

    #[test]
    fn rejects_quoted_nested_inline_and_trailing_example_markup() {
        for text in [
            "> ```tool\n> {\"name\":\"read_file\",\"args\":{}}\n> ```",
            "````markdown\n```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n````",
            "    ```tool\n    {\"name\":\"read_file\",\"args\":{}}\n    ```",
            " \t```tool\n{\"name\":\"read_file\",\"args\":{}}\n```",
            "Example: ```tool\n{\"name\":\"read_file\",\"args\":{}}\n```",
            "```tool\n{\"name\":\"read_file\",\"args\":{}} ```",
            "```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\nThis was an example, not an action.",
        ] {
            assert!(parse_tool_block(text).is_none(), "accepted quoted/example markup: {text}");
        }
    }

    #[test]
    fn tool_parser_preserves_markdown_inside_json_file_contents() {
        let content = "Example:\n```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n";
        let payload =
            serde_json::json!({"name":"write_file","args":{"path":"README.md","content":content}});
        let text = format!("Writing the documentation…\r\n```tool\r\n{payload}\r\n```\r\n");
        let call = parse_tool_block(&text).unwrap();
        assert_eq!(call.name, "write_file");
        assert_eq!(call.args["content"], content);
    }

    #[test]
    fn tool_parser_requires_named_action_and_object_arguments() {
        for payload in [
            r#"{"name":"","args":{}}"#,
            r#"{"name":"read_file"}"#,
            r#"{"name":"read_file","args":null}"#,
            r#"{"name":"read_file","args":[]}"#,
            r#"{"name":"read_file","args":{}} {"name":"write_file","args":{}}"#,
        ] {
            assert!(parse_tool_block(&format!("```tool\n{payload}\n```")).is_none());
        }
    }

    #[test]
    fn no_block_means_final_answer() {
        assert!(parse_tool_block("All done. Summary here.").is_none());
        assert!(parse_tool_block("```json\n{\"name\": \"x\"}\n```").is_none());
    }

    #[test]
    fn visible_progress_strips_tool_payload() {
        let text = "I'll inspect the entry point.\n```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"src/main.rs\"}}\n```";
        assert_eq!(visible_progress(text), "I'll inspect the entry point.");
    }

    #[test]
    fn malformed_block_ignored() {
        assert!(parse_tool_block("```tool\nnot json\n```").is_none());
    }

    #[test]
    fn system_prompt_names_workspace_and_tools() {
        let p = system_prompt("C:/ws", &crate::tools::registry(), true);
        assert!(p.contains("C:/ws"));
        assert!(p.contains("read_file"));
        assert!(p.contains("```tool"));
        assert!(p.contains("web_search"));
        assert!(p.contains("unrelated sibling projects"));
        let q = system_prompt("C:/ws", &crate::tools::registry(), false);
        assert!(!q.contains("web_search"));
    }

    #[test]
    fn follow_up_focus_excludes_unrelated_sibling_projects() {
        let root =
            std::env::temp_dir().join(format!("companion-project-focus-{}", uuid::Uuid::new_v4()));
        let ragev = root.join("RageV");
        let ember = root.join("ember");
        std::fs::create_dir_all(&ragev).unwrap();
        std::fs::create_dir_all(&ember).unwrap();
        std::fs::write(ragev.join("CMakeLists.txt"), "project(RageV)").unwrap();
        std::fs::write(ember.join("Cargo.toml"), "[package]").unwrap();

        let follow_up = vec![
            "What is the RageV project?".to_string(),
            "Find the tasks that are still pending in this project.".to_string(),
        ];
        assert_eq!(
            focused_workspace_root(&root, &follow_up),
            std::fs::canonicalize(&ragev).unwrap()
        );
        assert_eq!(
            focused_workspace_root(&root, &["Compare RageV and ember".into()]),
            root
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn completion_review_is_strict_about_inconclusive_answers() {
        assert!(matches!(
            parse_completion_review("COMPLETE"),
            CompletionReview::Complete
        ));
        assert!(matches!(
            parse_completion_review("CONTINUE: create the missing index.html"),
            CompletionReview::Continue(reason) if reason.contains("index.html")
        ));
        assert!(matches!(
            parse_completion_review("looks fine"),
            CompletionReview::Unavailable(_)
        ));
        assert!(matches!(
            parse_completion_review(""),
            CompletionReview::Unavailable(_)
        ));
    }

    #[test]
    fn tracks_changes_and_verification_actions() {
        let root = std::env::temp_dir().join(format!("verification-{}", uuid::Uuid::new_v4()));
        let ws = crate::workspace::WorkspaceManager::new(root);
        let mut state = VerificationState::default();
        let write = ToolCall {
            name: "write_file".into(),
            args: serde_json::json!({"path":"index.html"}),
        };
        let read = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"index.html"}),
        };
        let test = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"npm test"}),
        };
        state.observe(&ws, &write, "written", false);
        assert!(state.needs_evidence());
        state.observe(
            &ws,
            &ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path":"README.md"}),
            },
            "unrelated",
            false,
        );
        assert!(
            state.needs_evidence(),
            "an unrelated read must not verify a change"
        );
        state.observe(&ws, &read, "contents", false);
        assert!(!state.needs_evidence());
        state.observe(&ws, &write, "rewritten", false);
        assert!(
            state.needs_evidence(),
            "a new revision invalidates prior inspection"
        );
        state.observe(&ws, &test, "Command:\nnpm test\n\nExit code:\n1\n", true);
        assert!(state.needs_evidence());
        state.observe(
            &ws,
            &ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"npm test","cwd":"other"}),
            },
            "Command:\nnpm test\n\nExit code:\n0\n",
            false,
        );
        assert!(
            state.needs_evidence(),
            "tests in another directory do not verify this file"
        );
        state.observe(&ws, &test, "Command:\nnpm test\n\nExit code:\n0\n", false);
        assert!(!state.needs_evidence());
        assert!(tool_output_failed("Command:\nx\n\nExit code:\n1\n"));
        assert!(!tool_output_failed("Command:\nx\n\nExit code:\n0\n"));
        assert!(
            tool_output_failed("Command:\nx\n\nExit code:\n1\n\n--- stdout ---\nExit code:\n0\n"),
            "stdout text cannot forge the host process exit"
        );
    }

    #[test]
    fn named_node_verification_scripts_clear_only_successful_in_scope_evidence() {
        for command in [
            "node verify.cjs",
            "node scripts/check.mjs",
            "node test.js",
            "node feature.spec.cjs",
        ] {
            assert!(verification_command(command));
        }
        for command in [
            "node server.js",
            "node verify.cjs && node rewrite.js",
            "node verify.cjs; del important.txt",
            "echo node verify.cjs",
        ] {
            assert!(!verification_command(command));
        }
        let ws = crate::workspace::WorkspaceManager::new(std::env::temp_dir());
        let mut evidence = VerificationState {
            unknown_shell_changes: true,
            ..Default::default()
        };
        let call = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"node verify.cjs","cwd":"."}),
        };
        evidence.observe(
            &ws,
            &call,
            "Command:\nnode verify.cjs\n\nExit code:\n1\n",
            true,
        );
        assert!(evidence.needs_evidence());
        evidence.observe(
            &ws,
            &call,
            "Command:\nnode verify.cjs\n\nExit code:\n0\n",
            false,
        );
        assert!(!evidence.needs_evidence());
    }

    #[test]
    fn verification_does_not_match_substrings_or_arbitrary_shell_changes() {
        for command in [
            "echo false",
            "echo check",
            "dir",
            "Get-Content test.txt",
            "python mutate.py",
            "npm test; Set-Content x y",
            "node not-a-test.js",
            "test -f x",
        ] {
            assert!(!verification_command(command), "{command}");
        }
        for command in [
            "npm test",
            "npm run typecheck",
            "cargo check",
            "python -m pytest",
            "node calculator.test.js",
            "node --test",
        ] {
            assert!(verification_command(command), "{command}");
        }
        let ws = crate::workspace::WorkspaceManager::new(std::env::temp_dir());
        let mut state = VerificationState::default();
        state.observe(
            &ws,
            &ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"python mutate.py"}),
            },
            "Exit code:\n1\n",
            true,
        );
        assert!(
            state.unknown_shell_changes,
            "even a failed process may partially write"
        );
        state.observe(
            &ws,
            &ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path":"other.txt"}),
            },
            "contents",
            false,
        );
        assert!(state.needs_evidence());
    }

    #[test]
    fn identical_failures_are_bounded_but_corrected_arguments_can_progress() {
        let call = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"missing.txt"}),
        };
        let corrected = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"actual.txt"}),
        };
        let mut failures = FailedCalls::default();
        assert!(!failures.observe(&call, true));
        assert!(!failures.observe(&corrected, true));
        assert!(!failures.observe(&call, true));
        assert!(failures.observe(&call, true));
        assert!(!failures.observe(&call, false));
        assert!(!failures.observe(&call, true));
    }
}
